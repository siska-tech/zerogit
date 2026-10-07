//! Pack file reading and delta resolution.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::delta;
use super::index::PackIndex;
use crate::error::{Error, Result};
use crate::infra::hash::Sha1State;
use crate::infra::{crc32, decompress_exact, hash_object};
use crate::objects::reader::{CrcReader, Inflater, ObjectReader, Source};
use crate::objects::{ObjectType, Oid, RawObject};

const HEADER_SIZE: u64 = 12;
const TRAILER_SIZE: u64 = 20;
/// Deflate cannot expand its input by more than about 1032:1.
const MAX_DEFLATE_RATIO: u64 = 1032;
/// A 10-byte type/size header followed by a 20-byte REF_DELTA base, rounded up.
const MAX_ENTRY_HEADER: u64 = 32;

/// Resource limits applied while reading packed objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackLimits {
    /// Largest inflated entry or reconstructed object, in bytes.
    pub max_object_size: u64,
    /// Longest delta chain followed to reconstruct one object.
    pub max_delta_depth: usize,
    /// Bytes of reconstructed objects kept to shorten later delta chains; 0 disables.
    pub delta_cache_size: usize,
}

impl Default for PackLimits {
    /// 1 GiB objects, 10,000 deltas (well above Git's own 4,095 depth cap)
    /// and a 32 MiB delta base cache.
    fn default() -> Self {
        Self {
            max_object_size: 1 << 30,
            max_delta_depth: 10_000,
            delta_cache_size: 32 << 20,
        }
    }
}

/// A reconstructed object and the depth of the delta chain that produced it.
type CachedObject = (ObjectType, Arc<[u8]>, usize);

/// Bounded FIFO cache of objects reconstructed from this pack, keyed by offset.
///
/// Each object keeps its delta chain depth so that depth limits apply the same
/// whether or not part of a chain was cached.
#[derive(Debug, Default)]
struct DeltaCache {
    objects: HashMap<u64, CachedObject>,
    order: VecDeque<u64>,
    bytes: usize,
}

impl DeltaCache {
    fn insert(&mut self, capacity: usize, offset: u64, object: (ObjectType, &[u8], usize)) {
        let (object_type, content, depth) = object;
        // A single large object must not flush everything else.
        if content.len() > capacity / 4 || self.objects.contains_key(&offset) {
            return;
        }
        while self.bytes + content.len() > capacity {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some((_, old, _)) = self.objects.remove(&evicted) {
                self.bytes -= old.len();
            }
        }
        self.bytes += content.len();
        self.order.push_back(offset);
        self.objects
            .insert(offset, (object_type, Arc::from(content), depth));
    }
}

/// Supplies REF_DELTA bases that are not in the pack itself.
///
/// Receives the base OID and the remaining delta depth, which a store must
/// pass on when the base is itself a delta in another pack, so that chains
/// spanning packs stay bounded. `Ok(None)` means the base does not exist.
pub type BaseResolver<'a> = dyn FnMut(&Oid, usize) -> Result<Option<RawObject>> + 'a;

/// What a pack entry holds: a whole object, or a delta against a base at
/// an earlier offset of the same pack or named by its ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Base(ObjectType),
    OfsDelta(u64),
    RefDelta(Oid),
}

/// A pack entry as stored: its kind, inflated size and compressed data.
#[derive(Debug, Clone)]
/// The checked header of an entry, with the bytes read to parse it.
struct EntryHeader {
    kind: EntryKind,
    size: u64,
    header_len: usize,
    /// The length of the whole entry (header and compressed data).
    len: u64,
    crc: u32,
    /// The first bytes of the entry.
    prefix: Vec<u8>,
}

pub(crate) struct RawEntry {
    pub(crate) kind: EntryKind,
    pub(crate) size: u64,
    pub(crate) compressed: Vec<u8>,
}

/// A SHA-1 pack file (version 2 or 3) opened together with its index.
///
/// Opening checks the pack header, object count, trailer checksum against the
/// index and the entry layout, without reading every object. Each read checks
/// the CRC32 of every entry it inflates and the reconstructed object's ID.
/// [`PackFile::verify`] additionally hashes the whole pack on request.
#[derive(Debug)]
pub struct PackFile {
    path: PathBuf,
    file: Mutex<File>,
    index: PackIndex,
    version: u32,
    /// `(offset, crc32)` of every entry in ascending offset order.
    entries_by_offset: Vec<(u64, u32)>,
    /// Offset of the trailer checksum, where the last entry ends.
    data_end: u64,
    limits: PackLimits,
    cache: Mutex<DeltaCache>,
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidPack {
        reason: reason.into(),
    }
}

fn limit(reason: String) -> Error {
    Error::PackLimitExceeded { reason }
}

impl PackFile {
    /// Opens `path` and the `.idx` file next to it with default limits.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(path, PackLimits::default())
    }

    /// Opens `path` and the `.idx` file next to it.
    pub fn open_with_limits(path: impl AsRef<Path>, limits: PackLimits) -> Result<Self> {
        let path = path.as_ref();
        let index = PackIndex::read(path.with_extension("idx"))?;
        Self::with_index(path, index, limits)
    }

    /// Opens a pack using an index that has already been parsed.
    pub fn with_index(
        path: impl AsRef<Path>,
        index: PackIndex,
        limits: PackLimits,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let len = file.metadata()?.len();
        if len < HEADER_SIZE + TRAILER_SIZE {
            return Err(invalid("truncated pack"));
        }
        let mut header = [0u8; HEADER_SIZE as usize];
        file.read_exact(&mut header)?;
        if &header[..4] != b"PACK" {
            return Err(invalid("invalid signature"));
        }
        let version = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        if version != 2 && version != 3 {
            return Err(Error::UnsupportedPackVersion(version));
        }
        let count = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        if u64::from(count) != index.len() as u64 {
            return Err(invalid(format!(
                "pack has {} objects but index has {}",
                count,
                index.len()
            )));
        }
        let data_end = len - TRAILER_SIZE;
        let mut trailer = [0u8; TRAILER_SIZE as usize];
        file.seek(SeekFrom::Start(data_end))?;
        file.read_exact(&mut trailer)?;
        if &trailer != index.pack_checksum() {
            return Err(invalid("pack checksum does not match index"));
        }

        let mut entries_by_offset: Vec<_> = index
            .entries()
            .iter()
            .map(|entry| (entry.offset, entry.crc32))
            .collect();
        entries_by_offset.sort_unstable();
        // Entries are contiguous, so the pack holds exactly the indexed objects.
        let first = entries_by_offset.first().map_or(data_end, |e| e.0);
        if first != HEADER_SIZE {
            return Err(invalid("pack data does not start with an indexed entry"));
        }
        if entries_by_offset.last().is_some_and(|e| e.0 >= data_end) {
            return Err(invalid("index offset beyond pack data"));
        }
        Ok(Self {
            path,
            file: Mutex::new(file),
            index,
            version,
            entries_by_offset,
            data_end,
            limits,
            cache: Mutex::default(),
        })
    }

    /// Returns the pack file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the pack's index.
    pub fn index(&self) -> &PackIndex {
        &self.index
    }

    /// Returns the pack format version (2 or 3).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns the limits applied when reading objects.
    pub fn limits(&self) -> PackLimits {
        self.limits
    }

    /// Returns whether the index lists `oid`.
    pub fn contains(&self, oid: &Oid) -> bool {
        self.index.get(oid).is_some()
    }

    /// Reads an object whose delta bases are all within this pack.
    ///
    /// Returns `Ok(None)` if the pack does not contain `oid`. A REF_DELTA base
    /// missing from the pack is reported as `Error::InvalidPack`.
    pub fn read(&self, oid: &Oid) -> Result<Option<RawObject>> {
        self.read_with_resolver(oid, self.limits.max_delta_depth, &mut |_, _| Ok(None))
    }

    /// Reads an object, asking `resolver` for REF_DELTA bases outside this pack.
    ///
    /// At most `max_depth` deltas (and never more than the pack's limit) are
    /// applied, including those consumed by objects the resolver returns.
    pub fn read_with_resolver(
        &self,
        oid: &Oid,
        max_depth: usize,
        resolver: &mut BaseResolver<'_>,
    ) -> Result<Option<RawObject>> {
        let Some(entry) = self.index.get(oid) else {
            return Ok(None);
        };
        let object = self.resolve(entry.offset, max_depth, resolver)?;
        let actual = Oid::from_bytes(hash_object(object.object_type.as_str(), &object.content));
        if actual != *oid {
            return Err(invalid(format!(
                "object at offset {} is {} but index lists {}",
                entry.offset, actual, oid
            )));
        }
        Ok(Some(object))
    }

    /// Hashes the entire pack and reads every indexed object.
    ///
    /// This is an explicit, full scan; ordinary reads check only the entries
    /// they touch. Packs relying on external delta bases (thin packs) fail.
    pub fn verify(&self) -> Result<()> {
        let mut sha1 = Sha1State::new();
        let mut offset = 0;
        while offset < self.data_end {
            let chunk = (self.data_end - offset).min(1 << 20);
            sha1.update(&self.read_range(offset, chunk)?);
            offset += chunk;
        }
        if &sha1.finalize() != self.index.pack_checksum() {
            return Err(invalid("pack checksum mismatch"));
        }
        for entry in self.index.entries() {
            self.read(&entry.oid)?;
        }
        Ok(())
    }

    fn resolve(
        &self,
        mut offset: u64,
        max_depth: usize,
        resolver: &mut BaseResolver<'_>,
    ) -> Result<RawObject> {
        let max_depth = max_depth.min(self.limits.max_delta_depth);
        let too_deep = || limit(format!("delta chain deeper than {}", max_depth));
        let mut deltas = Vec::new();
        let mut visited = HashSet::new();
        // `depth` is the chain depth of the base; None for external bases,
        // whose results are not cached.
        let (object_type, mut content, mut depth) = loop {
            if let Some((object_type, content, depth)) = self.cached(offset)? {
                if depth + deltas.len() > max_depth {
                    return Err(too_deep());
                }
                break (object_type, content.to_vec(), Some(depth));
            }
            if !visited.insert(offset) {
                return Err(invalid(format!("delta cycle at offset {}", offset)));
            }
            let (kind, data) = self.read_entry(offset)?;
            let next = match kind {
                EntryKind::Base(object_type) => {
                    self.cache(offset, (object_type, &data, 0))?;
                    break (object_type, data, Some(0));
                }
                EntryKind::OfsDelta(base) => Ok(base),
                EntryKind::RefDelta(base) => self.index.get(&base).map(|e| e.offset).ok_or(base),
            };
            deltas.push((offset, data));
            if deltas.len() > max_depth {
                return Err(too_deep());
            }
            match next {
                Ok(base) => offset = base,
                Err(base) => {
                    let object = resolver(&base, max_depth - deltas.len())?
                        .ok_or_else(|| invalid(format!("missing delta base {}", base)))?;
                    break (object.object_type, object.content, None);
                }
            }
        };
        for (offset, data) in deltas.iter().rev() {
            content = delta::apply(&content, data, self.limits.max_object_size)?;
            if let Some(depth) = depth.as_mut() {
                *depth += 1;
                self.cache(*offset, (object_type, &content, *depth))?;
            }
        }
        Ok(RawObject {
            object_type,
            content,
        })
    }

    fn cached(&self, offset: u64) -> Result<Option<CachedObject>> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| invalid("delta cache lock poisoned"))?;
        Ok(cache.objects.get(&offset).cloned())
    }

    fn cache(&self, offset: u64, object: (ObjectType, &[u8], usize)) -> Result<()> {
        if self.limits.delta_cache_size > 0 {
            self.cache
                .lock()
                .map_err(|_| invalid("delta cache lock poisoned"))?
                .insert(self.limits.delta_cache_size, offset, object);
        }
        Ok(())
    }

    /// Reads, CRC-checks and inflates the entry starting at `offset`.
    fn read_entry(&self, offset: u64) -> Result<(EntryKind, Vec<u8>)> {
        let raw = self.raw_entry_at(offset)?;
        let corrupt = |what: &str| invalid(format!("{} at offset {}", what, offset));
        let size = usize::try_from(raw.size).map_err(|_| corrupt("entry size overflow"))?;
        let data =
            decompress_exact(&raw.compressed, size).map_err(|_| corrupt("corrupt zlib data"))?;
        if let EntryKind::OfsDelta(_) | EntryKind::RefDelta(_) = raw.kind {
            let result = delta::result_size(&data)?;
            if result > self.limits.max_object_size {
                return Err(limit(format!(
                    "delta at offset {} produces {} bytes, limit is {}",
                    offset, result, self.limits.max_object_size
                )));
            }
        }
        Ok((raw.kind, data))
    }

    /// The entry of `oid` as stored, CRC-checked but not inflated, so it
    /// can be copied into another pack; `None` if the pack lacks `oid`.
    pub(crate) fn raw_entry(&self, oid: &Oid) -> Result<Option<RawEntry>> {
        match self.index.get(oid) {
            Some(entry) => self.raw_entry_at(entry.offset).map(Some),
            None => Ok(None),
        }
    }

    /// How `oid` is stored (whole, or a delta and against what) and its
    /// stored size, read from the entry header only; `None` if the pack
    /// lacks `oid`.
    pub(crate) fn entry_kind(&self, oid: &Oid) -> Result<Option<(EntryKind, u64)>> {
        match self.index.get(oid) {
            Some(entry) => {
                let header = self.entry_header_at(entry.offset, true)?;
                Ok(Some((header.kind, header.size)))
            }
            None => Ok(None),
        }
    }

    /// Reads and checks the header of the entry starting at `offset`.
    /// A reader inflating the content of `oid` as it is read, when the pack
    /// stores it whole; `None` if the pack lacks `oid` or stores it as a
    /// delta. No size limit applies: the content is not held in memory.
    pub(crate) fn stream_entry(&self, oid: &Oid) -> Result<Option<ObjectReader>> {
        let Some(entry) = self.index.get(oid) else {
            return Ok(None);
        };
        let header = self.entry_header_at(entry.offset, false)?;
        let EntryKind::Base(object_type) = header.kind else {
            return Ok(None);
        };
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut raw = CrcReader::new(file.take(header.len));
        // The CRC covers the entry header as well.
        let mut skipped = vec![0u8; header.header_len];
        raw.read_exact(&mut skipped)?;
        Ok(Some(ObjectReader::streamed(
            *oid,
            object_type,
            header.size,
            Source::Packed {
                inflater: Inflater::new(raw),
                crc: header.crc,
            },
        )))
    }

    /// With `limited`, entries larger than `max_object_size` are refused.
    fn entry_header_at(&self, offset: u64, limited: bool) -> Result<EntryHeader> {
        let position = self
            .entries_by_offset
            .binary_search_by_key(&offset, |e| e.0)
            .map_err(|_| invalid(format!("no entry starts at offset {}", offset)))?;
        let crc = self.entries_by_offset[position].1;
        let end = self
            .entries_by_offset
            .get(position + 1)
            .map_or(self.data_end, |e| e.0);
        let len = end - offset;
        let prefix = self.read_range(offset, len.min(MAX_ENTRY_HEADER))?;

        let corrupt = |what: &str| invalid(format!("{} at offset {}", what, offset));
        let mut bytes = prefix.iter().copied();
        let mut next = || {
            bytes
                .next()
                .ok_or_else(|| corrupt("truncated entry header"))
        };
        let mut byte = next()?;
        let type_code = (byte >> 4) & 7;
        let mut size = u64::from(byte & 0x0f);
        let mut shift = 4;
        while byte & 0x80 != 0 {
            byte = next()?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || (bits << shift) >> shift != bits {
                return Err(corrupt("entry size overflow"));
            }
            size |= bits << shift;
            shift += 7;
        }
        let kind = match type_code {
            1 => EntryKind::Base(ObjectType::Commit),
            2 => EntryKind::Base(ObjectType::Tree),
            3 => EntryKind::Base(ObjectType::Blob),
            4 => EntryKind::Base(ObjectType::Tag),
            6 => {
                byte = next()?;
                let mut distance = u64::from(byte & 0x7f);
                while byte & 0x80 != 0 {
                    byte = next()?;
                    distance = distance
                        .checked_add(1)
                        .and_then(|d| d.checked_mul(128))
                        .ok_or_else(|| corrupt("OFS_DELTA distance overflow"))?
                        | u64::from(byte & 0x7f);
                }
                match offset.checked_sub(distance) {
                    Some(base) if distance > 0 => EntryKind::OfsDelta(base),
                    _ => return Err(corrupt("OFS_DELTA base out of range")),
                }
            }
            7 => {
                let mut base = [0u8; 20];
                for b in &mut base {
                    *b = next()?;
                }
                EntryKind::RefDelta(Oid::from_bytes(base))
            }
            other => return Err(corrupt(&format!("invalid object type {}", other))),
        };
        let header_len = prefix.len() - bytes.len();

        if limited && size > self.limits.max_object_size {
            return Err(limit(format!(
                "entry at offset {} inflates to {} bytes, limit is {}",
                offset, size, self.limits.max_object_size
            )));
        }
        let compressed = len - header_len as u64;
        if size > compressed.saturating_mul(MAX_DEFLATE_RATIO) + 64 {
            return Err(corrupt("declared size too large for compressed data"));
        }
        Ok(EntryHeader {
            kind,
            size,
            header_len,
            len,
            crc,
            prefix,
        })
    }

    /// Reads and CRC-checks the entry starting at `offset`.
    fn raw_entry_at(&self, offset: u64) -> Result<RawEntry> {
        let EntryHeader {
            kind,
            size,
            header_len,
            len,
            crc,
            prefix: mut raw,
        } = self.entry_header_at(offset, true)?;
        let rest = len - raw.len() as u64;
        if rest > 0 {
            raw.extend(self.read_range(offset + raw.len() as u64, rest)?);
        }
        if crc32(&raw) != crc {
            return Err(invalid(format!("CRC32 mismatch at offset {}", offset)));
        }
        raw.drain(..header_len);
        Ok(RawEntry {
            kind,
            size,
            compressed: raw,
        })
    }

    fn read_range(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let len = usize::try_from(len).map_err(|_| invalid("entry too large for this platform"))?;
        let mut buffer = vec![0u8; len];
        let mut file = self
            .file
            .lock()
            .map_err(|_| invalid("pack file lock poisoned"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::infra::compress;
    use tempfile::TempDir;

    fn open(builder: &Builder, limits: PackLimits) -> (TempDir, Result<PackFile>) {
        let temp = TempDir::new().unwrap();
        let path = builder.write(temp.path(), 2);
        let pack = PackFile::open_with_limits(path, limits);
        (temp, pack)
    }

    fn reason<T: std::fmt::Debug>(result: Result<T>) -> String {
        match result {
            Err(Error::InvalidPack { reason }) => reason,
            other => panic!("expected InvalidPack, got {:?}", other),
        }
    }

    fn base_text() -> Vec<u8> {
        b"line of text\n".repeat(20)
    }

    #[test]
    fn reads_base_objects_and_delta_chains() {
        let mut b = Builder::default();
        let mut expected = Vec::new();
        for (object_type, content) in [
            (ObjectType::Commit, b"tree 0\n\ncommit".to_vec()),
            (ObjectType::Tree, b"100644 a\0aaaaaaaaaaaaaaaaaaaa".to_vec()),
            (ObjectType::Tag, b"object 0\ntype commit\n".to_vec()),
            (ObjectType::Blob, Vec::new()),
        ] {
            let (oid, _) = b.object(object_type, &content);
            expected.push((oid, object_type, content));
        }
        let v0 = base_text();
        let (oid0, offset0) = b.object(ObjectType::Blob, &v0);
        let mut previous = (oid0, offset0, v0.clone());
        expected.push((oid0, ObjectType::Blob, v0));
        // Alternate OFS_DELTA and in-pack REF_DELTA to build a four-level chain.
        for level in 0..4 {
            let mut content = previous.2.clone();
            content.extend_from_slice(format!("level {}\n", level).as_bytes());
            let delta = append_delta(&previous.2, &content[previous.2.len()..]);
            let oid = oid_of(ObjectType::Blob, &content);
            let offset = if level % 2 == 0 {
                b.ofs_delta(oid, previous.1, &delta)
            } else {
                b.ref_delta(oid, previous.0, &delta)
            };
            expected.push((oid, ObjectType::Blob, content.clone()));
            previous = (oid, offset, content);
        }

        for version in [2, 3] {
            let temp = TempDir::new().unwrap();
            let pack = PackFile::open(b.write(temp.path(), version)).unwrap();
            assert_eq!(pack.version(), version);
            for (oid, object_type, content) in &expected {
                assert!(pack.contains(oid));
                let object = pack.read(oid).unwrap().unwrap();
                assert_eq!(object.object_type, *object_type);
                assert_eq!(&object.content, content);
            }
            let missing = oid_of(ObjectType::Blob, b"missing");
            assert!(pack.read(&missing).unwrap().is_none());
            pack.verify().unwrap();
        }
    }

    #[test]
    fn external_ref_delta_base_is_delegated() {
        let base = base_text();
        let mut content = base.clone();
        content.extend_from_slice(b"external\n");
        let oid = oid_of(ObjectType::Blob, &content);
        let base_oid = oid_of(ObjectType::Blob, &base);
        let mut b = Builder::default();
        b.ref_delta(oid, base_oid, &append_delta(&base, b"external\n"));
        let (_temp, pack) = open(&b, PackLimits::default());
        let pack = pack.unwrap();

        assert!(reason(pack.read(&oid)).contains("missing delta base"));
        assert!(reason(pack.verify()).contains("missing delta base"));
        let mut calls = Vec::new();
        let object = pack
            .read_with_resolver(&oid, 5, &mut |requested, remaining| {
                calls.push((*requested, remaining));
                Ok(Some(RawObject {
                    object_type: ObjectType::Blob,
                    content: base.clone(),
                }))
            })
            .unwrap()
            .unwrap();
        assert_eq!(object.content, content);
        assert_eq!(calls, vec![(base_oid, 4)]);

        let failed = pack.read_with_resolver(&oid, 5, &mut |_, _| Err(Error::InvalidUtf8));
        assert!(matches!(failed, Err(Error::InvalidUtf8)));
        let shallow = pack.read_with_resolver(&oid, 0, &mut |_, _| unreachable!());
        assert!(matches!(shallow, Err(Error::PackLimitExceeded { .. })));
    }

    #[test]
    fn ref_delta_cycles_are_rejected() {
        let a = Oid::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let c = Oid::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();
        let delta = append_delta(b"", b"x");
        let mut b = Builder::default();
        b.ref_delta(a, c, &delta);
        b.ref_delta(c, a, &delta);
        let (_temp, pack) = open(&b, PackLimits::default());
        assert!(reason(pack.unwrap().read(&a)).contains("delta cycle"));
    }

    #[test]
    fn whole_entries_stream_past_the_size_limit() {
        let mut b = Builder::default();
        let content = base_text().repeat(50);
        let (oid, offset) = b.object(ObjectType::Blob, &content);
        let longer = [content.clone(), b"more\n".to_vec()].concat();
        let delta_oid = oid_of(ObjectType::Blob, &longer);
        b.ofs_delta(delta_oid, offset, &append_delta(&content, b"more\n"));
        let limits = PackLimits {
            max_object_size: 100,
            ..PackLimits::default()
        };
        let (_temp, pack) = open(&b, limits);
        let pack = pack.unwrap();
        // Too large to read whole, but streamed with no limit.
        assert!(matches!(
            pack.read(&oid),
            Err(Error::PackLimitExceeded { .. })
        ));
        let mut reader = pack.stream_entry(&oid).unwrap().unwrap();
        assert_eq!(reader.size(), content.len() as u64);
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read, content);
        // Deltas are not streamed; missing objects are not found.
        assert!(pack.stream_entry(&delta_oid).unwrap().is_none());
        assert!(pack
            .stream_entry(&oid_of(ObjectType::Blob, b"missing"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn corrupt_streamed_entries_fail_at_the_end() {
        let mut b = Builder::default();
        let content = base_text();
        let (oid, _) = b.object(ObjectType::Blob, &content);
        let temp = TempDir::new().unwrap();
        let path = b.write(temp.path(), 2);
        // Damage a byte of the compressed data, keeping the index.
        let mut data = std::fs::read(&path).unwrap();
        data[12 + 3] ^= 0x01;
        std::fs::write(&path, &data).unwrap();
        let pack = PackFile::with_index(
            &path,
            PackIndex::read(path.with_extension("idx")).unwrap(),
            PackLimits::default(),
        )
        .unwrap();
        // Opening checks the layout, not the data: the stream must fail.
        let mut reader = pack.stream_entry(&oid).unwrap().unwrap();
        let mut read = Vec::new();
        let error = reader.read_to_end(&mut read).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn delta_depth_and_size_limits_are_enforced() {
        let mut b = Builder::default();
        let mut content = base_text();
        let (mut oid, mut offset) = b.object(ObjectType::Blob, &content);
        let mut chain = vec![oid];
        for level in 0..3u8 {
            let digit = b'0' + level;
            let delta = append_delta(&content, &[digit]);
            content.push(digit);
            oid = oid_of(ObjectType::Blob, &content);
            offset = b.ofs_delta(oid, offset, &delta);
            chain.push(oid);
        }
        let limits = PackLimits {
            max_delta_depth: 2,
            ..PackLimits::default()
        };
        let (_temp, pack) = open(&b, limits);
        let pack = pack.unwrap();
        assert!(pack.read(&chain[2]).unwrap().is_some());
        assert!(matches!(
            pack.read(&chain[3]),
            Err(Error::PackLimitExceeded { .. })
        ));

        let size = content.len() as u64;
        let limits = PackLimits {
            max_object_size: size - 2,
            ..PackLimits::default()
        };
        let (_temp, pack) = open(&b, limits);
        let pack = pack.unwrap();
        assert!(pack.read(&chain[0]).unwrap().is_some());
        for oid in &chain[2..] {
            assert!(matches!(
                pack.read(oid),
                Err(Error::PackLimitExceeded { .. })
            ));
        }
        let limits = PackLimits {
            max_object_size: size - 4,
            ..PackLimits::default()
        };
        let (_temp, pack) = open(&b, limits);
        assert!(matches!(
            pack.unwrap().read(&chain[0]),
            Err(Error::PackLimitExceeded { .. })
        ));
    }

    #[test]
    fn delta_cache_serves_repeated_reads() {
        let mut b = Builder::default();
        let base = base_text();
        let (_, base_offset) = b.object(ObjectType::Blob, &base);
        let mut content = base.clone();
        content.extend_from_slice(b"cached\n");
        let oid = oid_of(ObjectType::Blob, &content);
        b.ofs_delta(oid, base_offset, &append_delta(&base, b"cached\n"));

        for (cache_size, survives) in [(PackLimits::default().delta_cache_size, true), (0, false)] {
            let limits = PackLimits {
                delta_cache_size: cache_size,
                ..PackLimits::default()
            };
            let (_temp, pack) = open(&b, limits);
            let pack = pack.unwrap();
            assert_eq!(pack.read(&oid).unwrap().unwrap().content, content);
            // Damage both entries on disk; only cached reads avoid the file.
            let mut data = std::fs::read(pack.path()).unwrap();
            for offset in [base_offset as usize, data.len() - 30] {
                data[offset + 2] ^= 0xff;
            }
            std::fs::write(pack.path(), data).unwrap();
            let again = pack.read(&oid);
            assert_eq!(again.is_ok(), survives, "{:?}", again);
            if survives {
                assert_eq!(again.unwrap().unwrap().content, content);
            }
        }
    }

    #[test]
    fn delta_cache_evicts_oldest_within_capacity() {
        let mut cache = DeltaCache::default();
        for offset in 0..5u64 {
            cache.insert(40, offset, (ObjectType::Blob, &[0; 10], 0));
        }
        assert_eq!(cache.bytes, 40);
        let mut kept: Vec<_> = cache.objects.keys().copied().collect();
        kept.sort_unstable();
        assert_eq!(kept, vec![1, 2, 3, 4]);
        cache.insert(40, 9, (ObjectType::Blob, &[0; 11], 0));
        assert!(!cache.objects.contains_key(&9));
    }

    #[test]
    fn rejects_bad_headers_and_trailers() {
        let mut b = Builder::default();
        b.object(ObjectType::Blob, b"content");
        let temp = TempDir::new().unwrap();
        let path = b.write(temp.path(), 4);
        assert!(matches!(
            PackFile::open(&path),
            Err(Error::UnsupportedPackVersion(4))
        ));

        let mut body = b"PACX\0\0\0\x02\0\0\0\x01".to_vec();
        body.extend_from_slice(&b.data);
        assert_eq!(
            reason(PackFile::open(write_pack(temp.path(), body, &b.index))),
            "invalid signature"
        );

        let mut body = b"PACK\0\0\0\x02\0\0\0\x02".to_vec();
        body.extend_from_slice(&b.data);
        assert!(
            reason(PackFile::open(write_pack(temp.path(), body, &b.index))).contains("objects")
        );

        let path = b.write(temp.path(), 2);
        let mut data = std::fs::read(&path).unwrap();
        *data.last_mut().unwrap() ^= 1;
        std::fs::write(&path, &data).unwrap();
        assert!(reason(PackFile::open(&path)).contains("does not match index"));

        // Truncated or bit-flipped packs are rejected at open, read or verify.
        let path = b.write(temp.path(), 2);
        let original = std::fs::read(&path).unwrap();
        let check = |data: &[u8]| {
            std::fs::write(&path, data).unwrap();
            let result = PackFile::open(&path).and_then(|pack| pack.verify());
            assert!(result.is_err(), "{:?}", data);
        };
        for len in 0..original.len() {
            check(&original[..len]);
        }
        for i in 0..original.len() - 20 {
            let mut flipped = original.clone();
            flipped[i] ^= 0x10;
            check(&flipped);
        }
    }

    #[test]
    fn rejects_corrupt_entries() {
        let check = |build: &dyn Fn(&mut Builder) -> Oid, expected: &str| {
            let mut b = Builder::default();
            let oid = build(&mut b);
            let (_temp, pack) = open(&b, PackLimits::default());
            let message = reason(pack.and_then(|pack| pack.read(&oid)));
            assert!(message.contains(expected), "{}: {}", expected, message);
        };
        let blob = oid_of(ObjectType::Blob, b"abc");
        check(
            &|b| {
                b.entry(blob, 3, 3, &[], b"not zlib");
                blob
            },
            "corrupt zlib",
        );
        check(
            &|b| {
                b.entry(blob, 3, 4, &[], &compress(b"abc"));
                blob
            },
            "corrupt zlib",
        );
        check(
            &|b| {
                b.entry(blob, 5, 3, &[], &compress(b"abc"));
                blob
            },
            "invalid object type 5",
        );
        check(
            &|b| {
                b.entry(blob, 3, 1 << 20, &[], &compress(b"abc"));
                blob
            },
            "declared size too large",
        );
        check(
            &|b| {
                b.entry(blob, 3, 3, &[], &compress(b"abd"));
                blob
            },
            "index lists",
        );
        check(
            &|b| {
                b.object(ObjectType::Blob, &base_text());
                b.entry(blob, 6, 2, &ofs(1000), &compress(&[0, 0]));
                blob
            },
            "OFS_DELTA base out of range",
        );
        check(
            &|b| {
                b.object(ObjectType::Blob, &base_text());
                let delta = append_delta(b"abc", b"");
                b.entry(blob, 6, delta.len() as u64, &ofs(1), &compress(&delta));
                blob
            },
            "no entry starts at offset",
        );
        check(
            &|b| {
                let (_, base) = b.object(ObjectType::Blob, b"ab");
                b.ofs_delta(blob, base, &append_delta(b"a", b"bc"));
                blob
            },
            "base size mismatch",
        );
        check(
            &|b| {
                b.entry(blob, 3, 3, &[], &compress(b"abc"));
                b.index[0].2 ^= 1;
                blob
            },
            "CRC32 mismatch",
        );
    }
}
