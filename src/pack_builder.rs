//! Writing packs of chosen objects, for [`Repository::repack`] and
//! [`Repository::pack_objects`].
//!
//! Each object goes in as it is stored in an existing pack when that can be
//! copied (whole, or a delta whose base the reader will have), or as a new
//! delta found by the delta search (`objects::pack::deltify`), or whole.
//! Bases are written before the deltas against them.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::infra::hash::Sha1State;
use crate::infra::{compress, crc32};
use crate::objects::pack::deltify::{find_deltas, Candidate, DeltaSearch, NewDelta};
use crate::objects::pack::indexer::{entry_header, type_code, IndexedObject};
use crate::objects::pack::{EntryKind, PackFile, RawEntry};
use crate::objects::Oid;
use crate::repository::Repository;

/// Where a pack is written.
pub(crate) trait PackSink {
    fn write(&mut self, data: &[u8]) -> Result<()>;
    /// How many bytes were written so far.
    fn offset(&self) -> u64;
}

/// The pack header: signature, version 2 and the number of objects.
fn pack_header(count: u32) -> Vec<u8> {
    let mut header = b"PACK".to_vec();
    header.extend_from_slice(&2u32.to_be_bytes());
    header.extend_from_slice(&count.to_be_bytes());
    header
}

/// A pack being written to a temporary file, hashed as it goes.
pub(crate) struct PackWriter {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    sha1: Sha1State,
    offset: u64,
}

impl PackWriter {
    pub(crate) fn create(dir: &Path, count: u32) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = dir.join(format!("tmp_pack_zerogit_{}_{}", std::process::id(), nanos));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = PackWriter {
            path,
            file: Some(BufWriter::new(file)),
            sha1: Sha1State::new(),
            offset: 0,
        };
        writer.write(&pack_header(count))?;
        Ok(writer)
    }

    /// Writes the trailer and closes the file; returns the checksum.
    pub(crate) fn finish(mut self) -> Result<(PathBuf, [u8; 20])> {
        let checksum = std::mem::replace(&mut self.sha1, Sha1State::new()).finalize();
        let mut file = self.file.take().expect("open until finished");
        file.write_all(&checksum)?;
        let file = file.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.sync_all()?;
        Ok((std::mem::take(&mut self.path), checksum))
    }
}

impl PackSink for PackWriter {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .expect("open until finished")
            .write_all(data)?;
        self.sha1.update(data);
        self.offset += data.len() as u64;
        Ok(())
    }

    fn offset(&self) -> u64 {
        self.offset
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        // Not finished: remove the partial file.
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A pack written to memory.
pub(crate) struct MemoryPack {
    data: Vec<u8>,
}

impl MemoryPack {
    pub(crate) fn new(count: u32) -> Self {
        MemoryPack {
            data: pack_header(count),
        }
    }

    /// The pack with its trailing checksum.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let mut sha1 = Sha1State::new();
        sha1.update(&self.data);
        let checksum = sha1.finalize();
        self.data.extend_from_slice(&checksum);
        self.data
    }
}

impl PackSink for MemoryPack {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.data.extend_from_slice(data);
        Ok(())
    }

    fn offset(&self) -> u64 {
        self.data.len() as u64
    }
}

/// Encodes the distance back to an OFS_DELTA base, as Git does.
fn ofs_distance(mut distance: u64) -> Vec<u8> {
    let mut bytes = vec![(distance & 0x7f) as u8];
    distance >>= 7;
    while distance > 0 {
        distance -= 1;
        bytes.push(0x80 | (distance & 0x7f) as u8);
        distance >>= 7;
    }
    bytes.reverse();
    bytes
}

/// How an object goes into the pack.
enum Stored {
    /// Copied as it is from an existing pack (the index), where it is whole.
    Copied(usize),
    /// Copied from an existing pack, where it is a delta against an object
    /// the reader will have.
    CopiedDelta(usize, Oid),
    /// Written whole.
    Whole,
    /// Written as a new delta.
    NewDelta(NewDelta),
}

/// What to write, and how.
pub(crate) struct PackPlan<'a> {
    /// The objects, in the order to write them (bases may come earlier).
    pub(crate) objects: &'a [Oid],
    /// What the delta search needs of the objects; those missing here are
    /// not searched.
    pub(crate) info: HashMap<Oid, Candidate>,
    /// Objects the reader has without this pack, tried as delta bases
    /// (for a thin pack).
    pub(crate) external_bases: Vec<Candidate>,
    /// Objects the reader has without this pack: existing deltas against
    /// them are copied too (for a thin pack).
    pub(crate) reader_has: Option<&'a HashSet<Oid>>,
    /// Packs whose entries may be copied.
    pub(crate) packs: &'a [Arc<PackFile>],
    /// Whether deltas against objects of the pack may name their base by
    /// offset (OFS_DELTA); otherwise every delta names it by ID
    /// (REF_DELTA).
    pub(crate) ofs_delta: bool,
    pub(crate) search: DeltaSearch,
}

/// What [`Repository::write_pack_objects`] wrote.
pub(crate) struct PackStats {
    pub(crate) indexed: Vec<IndexedObject>,
    pub(crate) reused_deltas: usize,
    pub(crate) new_deltas: usize,
}

impl Repository {
    /// Writes the objects of `plan` to `sink` (after the pack header the
    /// sink has written, for `plan.objects.len()` objects).
    pub(crate) fn write_pack_objects(
        &self,
        mut plan: PackPlan<'_>,
        sink: &mut dyn PackSink,
    ) -> Result<PackStats> {
        let store = self.object_store();
        let objects = plan.objects;
        let packed: HashSet<Oid> = objects.iter().copied().collect();
        let reader_has = |oid: &Oid| plan.reader_has.is_some_and(|has| has.contains(oid));
        let too_large = |oid: &Oid| Error::PackLimitExceeded {
            reason: format!("object {} too large for this platform", oid),
        };

        // How each object is stored in the existing packs, and what of that
        // can be copied.
        let mut stored: HashMap<Oid, Stored> = HashMap::with_capacity(objects.len());
        // Offsets of each pack's entries, to name OFS_DELTA bases.
        let mut offsets: HashMap<usize, HashMap<u64, Oid>> = HashMap::new();
        for &oid in objects {
            let found = plan
                .packs
                .iter()
                .enumerate()
                .find(|(_, p)| p.contains(&oid));
            let how = match found {
                None => Stored::Whole,
                Some((pack_no, pack)) => {
                    let (kind, _) = pack
                        .entry_kind(&oid)?
                        .ok_or_else(|| Error::ObjectNotFound(oid.to_hex()))?;
                    let base = match kind {
                        EntryKind::Base(_) => None,
                        EntryKind::RefDelta(base) => Some(base),
                        EntryKind::OfsDelta(offset) => offsets
                            .entry(pack_no)
                            .or_insert_with(|| {
                                pack.index()
                                    .entries()
                                    .iter()
                                    .map(|e| (e.offset, e.oid))
                                    .collect()
                            })
                            .get(&offset)
                            .copied(),
                    };
                    match (kind, base) {
                        (EntryKind::Base(_), _) => Stored::Copied(pack_no),
                        // A delta against an object the reader will have.
                        (_, Some(base))
                            if base != oid && (packed.contains(&base) || reader_has(&base)) =>
                        {
                            Stored::CopiedDelta(pack_no, base)
                        }
                        // The base is not sent: store the object whole.
                        _ => Stored::Whole,
                    }
                }
            };
            stored.insert(oid, how);
        }
        // Copied deltas that lead back to themselves (across packs) cannot
        // be written in order: one of each cycle is stored whole.
        for &oid in objects {
            let mut seen = HashSet::new();
            let mut current = oid;
            while let Some(Stored::CopiedDelta(_, base)) = stored.get(&current) {
                if !seen.insert(current) {
                    stored.insert(current, Stored::Whole);
                    break;
                }
                current = *base;
            }
        }

        // New deltas for the rest.
        let mut candidates: Vec<Candidate> = objects
            .iter()
            .filter_map(|oid| {
                let mut candidate = plan.info.remove(oid)?;
                if let Some(Stored::CopiedDelta(_, base)) = stored.get(oid) {
                    candidate.reused_base = Some(*base);
                }
                Some(candidate)
            })
            .collect();
        candidates.extend(
            std::mem::take(&mut plan.external_bases)
                .into_iter()
                .filter(|c| !packed.contains(&c.oid))
                .map(|c| Candidate {
                    base_only: true,
                    ..c
                }),
        );
        let found = find_deltas(&candidates, &plan.search, &mut |oid| {
            Ok(store.read(oid)?.content)
        })?;
        drop(candidates);
        let new_deltas = found.len();
        for (oid, delta) in found {
            stored.insert(oid, Stored::NewDelta(delta));
        }

        let mut written: HashMap<Oid, u64> = HashMap::new();
        let mut indexed: Vec<IndexedObject> = Vec::with_capacity(objects.len());
        let mut reused_deltas = 0;
        for &start in objects {
            // Bases in the pack go before the deltas against them.
            let mut chain = Vec::new();
            let mut current = start;
            while !written.contains_key(&current) && stored.contains_key(&current) {
                chain.push(current);
                match stored.get(&current) {
                    Some(Stored::CopiedDelta(_, base)) => current = *base,
                    Some(Stored::NewDelta(delta)) => current = delta.base,
                    _ => break,
                }
            }
            for oid in chain.into_iter().rev() {
                let offset = sink.offset();
                // The header of a delta against `base` of `size` bytes.
                let delta_header = |base: &Oid, size: usize| -> Vec<u8> {
                    match written.get(base) {
                        Some(at) if plan.ofs_delta => {
                            let mut bytes = entry_header(6, size);
                            bytes.extend(ofs_distance(offset - at));
                            bytes
                        }
                        _ => {
                            let mut bytes = entry_header(7, size);
                            bytes.extend_from_slice(base.as_bytes());
                            bytes
                        }
                    }
                };
                let raw_entry = |pack_no: usize| -> Result<RawEntry> {
                    plan.packs[pack_no]
                        .raw_entry(&oid)?
                        .ok_or_else(|| Error::ObjectNotFound(oid.to_hex()))
                };
                let mut bytes = Vec::new();
                match stored.remove(&oid).expect("every object is planned once") {
                    Stored::Copied(pack_no) => {
                        let entry = raw_entry(pack_no)?;
                        let EntryKind::Base(object_type) = entry.kind else {
                            unreachable!("only whole entries are copied as they are")
                        };
                        let size = usize::try_from(entry.size).map_err(|_| too_large(&oid))?;
                        bytes.extend(entry_header(type_code(object_type), size));
                        bytes.extend_from_slice(&entry.compressed);
                    }
                    Stored::CopiedDelta(pack_no, base) => {
                        let entry = raw_entry(pack_no)?;
                        let size = usize::try_from(entry.size).map_err(|_| too_large(&oid))?;
                        bytes.extend(delta_header(&base, size));
                        bytes.extend_from_slice(&entry.compressed);
                        reused_deltas += 1;
                    }
                    Stored::NewDelta(delta) => {
                        bytes.extend(delta_header(&delta.base, delta.data.len()));
                        bytes.extend(compress(&delta.data));
                    }
                    Stored::Whole => {
                        let object = store.read(&oid)?;
                        bytes.extend(entry_header(
                            type_code(object.object_type),
                            object.content.len(),
                        ));
                        bytes.extend(compress(&object.content));
                    }
                }
                written.insert(oid, offset);
                indexed.push(IndexedObject {
                    oid,
                    offset,
                    crc32: crc32(&bytes),
                });
                sink.write(&bytes)?;
            }
        }
        Ok(PackStats {
            indexed,
            reused_deltas,
            new_deltas,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ofs_distances_like_git() {
        assert_eq!(ofs_distance(1), [1]);
        assert_eq!(ofs_distance(127), [127]);
        assert_eq!(ofs_distance(128), [0x80, 0]);
        assert_eq!(ofs_distance(16511), [0xff, 0x7f]);
        assert_eq!(ofs_distance(16512), [0x80, 0x80, 0]);
    }
}
