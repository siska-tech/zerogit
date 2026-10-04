//! Indexing received packs (`git index-pack`) and writing pack indexes.
//!
//! A pack received from a remote is checked (header, trailer checksum, every
//! entry inflating to its declared size), its deltas are resolved to find
//! each object's ID, and an index (version 2) is written for it. A thin pack,
//! whose deltas refer to objects the receiver already has, is completed by
//! appending those base objects, as `git index-pack --fix-thin` does, so the
//! stored pack is self-contained.

use std::collections::HashMap;

use super::delta;
use crate::error::{Error, Result};
use crate::infra::hash::sha1;
use crate::infra::{compress, crc32, decompress_prefix, hash_object};
use crate::objects::{ObjectType, Oid, RawObject};

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidPack {
        reason: reason.into(),
    }
}

/// One object of an indexed pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexedObject {
    pub(crate) oid: Oid,
    pub(crate) offset: u64,
    pub(crate) crc32: u32,
}

/// A checked pack (completed if it was thin) with its objects.
#[derive(Debug)]
pub(crate) struct IndexedPack {
    /// The pack bytes to store.
    pub(crate) data: Vec<u8>,
    pub(crate) objects: Vec<IndexedObject>,
    /// The pack's trailing checksum.
    pub(crate) checksum: [u8; 20],
}

enum Kind {
    Base(ObjectType),
    OfsDelta(u64),
    RefDelta(Oid),
}

struct Entry {
    offset: u64,
    crc32: u32,
    kind: Kind,
    data: Vec<u8>,
}

pub(crate) fn type_code(object_type: ObjectType) -> u8 {
    match object_type {
        ObjectType::Commit => 1,
        ObjectType::Tree => 2,
        ObjectType::Blob => 3,
        ObjectType::Tag => 4,
    }
}

/// Encodes an entry header (type and inflated size).
pub(crate) fn entry_header(type_code: u8, size: usize) -> Vec<u8> {
    let mut header = Vec::new();
    let mut byte = (type_code << 4) | (size & 0x0f) as u8;
    let mut rest = size >> 4;
    while rest > 0 {
        header.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    header.push(byte);
    header
}

fn parse_entries(data: &[u8], count: u32, end: usize) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(count as usize);
    let mut pos = 12usize;
    for _ in 0..count {
        let offset = pos;
        let corrupt = |what: &str| invalid(format!("{} at offset {}", what, offset));
        let mut next = || -> Result<u8> {
            if pos >= end {
                return Err(corrupt("truncated entry"));
            }
            pos += 1;
            Ok(data[pos - 1])
        };
        let mut byte = next()?;
        let code = (byte >> 4) & 7;
        let mut size = u64::from(byte & 0x0f);
        let mut shift = 4;
        while byte & 0x80 != 0 {
            byte = next()?;
            if shift >= 64 {
                return Err(corrupt("entry size overflow"));
            }
            size |= u64::from(byte & 0x7f) << shift;
            shift += 7;
        }
        let kind = match code {
            1 => Kind::Base(ObjectType::Commit),
            2 => Kind::Base(ObjectType::Tree),
            3 => Kind::Base(ObjectType::Blob),
            4 => Kind::Base(ObjectType::Tag),
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
                match (offset as u64).checked_sub(distance) {
                    Some(base) if distance > 0 => Kind::OfsDelta(base),
                    _ => return Err(corrupt("OFS_DELTA base out of range")),
                }
            }
            7 => {
                let mut base = [0u8; 20];
                for b in &mut base {
                    *b = next()?;
                }
                Kind::RefDelta(Oid::from_bytes(base))
            }
            other => return Err(corrupt(&format!("invalid object type {}", other))),
        };
        let size = usize::try_from(size).map_err(|_| corrupt("entry too large"))?;
        let (inflated, used) =
            decompress_prefix(&data[pos..end], size).map_err(|_| corrupt("corrupt zlib data"))?;
        pos += used;
        entries.push(Entry {
            offset: offset as u64,
            crc32: crc32(&data[offset..pos]),
            kind,
            data: inflated,
        });
    }
    if pos != end {
        return Err(invalid("data after the last entry"));
    }
    Ok(entries)
}

/// Checks a pack, resolves its deltas and returns its objects.
///
/// `external` supplies delta bases that are not in the pack (a thin pack);
/// they are appended to the returned pack data.
pub(crate) fn index_pack(
    data: &[u8],
    external: &mut dyn FnMut(&Oid) -> Result<Option<RawObject>>,
) -> Result<IndexedPack> {
    if data.len() < 32 || &data[..4] != b"PACK" {
        return Err(invalid("not a pack"));
    }
    let version = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if version != 2 && version != 3 {
        return Err(Error::UnsupportedPackVersion(version));
    }
    let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let end = data.len() - 20;
    if sha1(&data[..end]) != data[end..] {
        return Err(invalid("pack checksum mismatch"));
    }
    let entries = parse_entries(data, count, end)?;

    // Resolve: start from base objects and apply deltas to children.
    let by_offset: HashMap<u64, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.offset, i))
        .collect();
    let mut ofs_children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut ref_children: HashMap<Oid, Vec<usize>> = HashMap::new();
    let mut resolved: Vec<Option<(Oid, ObjectType)>> = vec![None; entries.len()];
    let mut ready: Vec<(usize, ObjectType, Vec<u8>)> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        match &entry.kind {
            Kind::Base(object_type) => ready.push((i, *object_type, entry.data.clone())),
            Kind::OfsDelta(base) => {
                let base = *by_offset
                    .get(base)
                    .ok_or_else(|| invalid(format!("no entry at offset {}", base)))?;
                ofs_children.entry(base).or_default().push(i);
            }
            Kind::RefDelta(base) => ref_children.entry(*base).or_default().push(i),
        }
    }

    let mut appended: Vec<(ObjectType, Vec<u8>)> = Vec::new();
    loop {
        while let Some((i, object_type, content)) = ready.pop() {
            let oid = Oid::from_bytes(hash_object(object_type.as_str(), &content));
            if resolved[i].is_some() {
                continue;
            }
            resolved[i] = Some((oid, object_type));
            let mut children = ofs_children.remove(&i).unwrap_or_default();
            children.extend(ref_children.remove(&oid).unwrap_or_default());
            for child in children {
                let result = delta::apply(&content, &entries[child].data, u64::MAX)?;
                ready.push((child, object_type, result));
            }
        }
        // Deltas against objects outside the pack (a thin pack). A base may
        // also be an in-pack object not resolved yet because it depends on
        // such an outside base, so take the first base found outside.
        if ref_children.is_empty() {
            break;
        }
        let mut bases: Vec<Oid> = ref_children.keys().copied().collect();
        bases.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut found = None;
        for base in &bases {
            if let Some(object) = external(base)? {
                found = Some((*base, object));
                break;
            }
        }
        let Some((base, object)) = found else {
            return Err(invalid(format!("missing delta base {}", bases[0])));
        };
        for child in ref_children.remove(&base).unwrap_or_default() {
            let result = delta::apply(&object.content, &entries[child].data, u64::MAX)?;
            ready.push((child, object.object_type, result));
        }
        appended.push((object.object_type, object.content));
    }
    if resolved.iter().any(Option::is_none) {
        return Err(invalid("delta cycle or unresolved delta"));
    }

    let mut objects: Vec<IndexedObject> = entries
        .iter()
        .zip(&resolved)
        .map(|(entry, r)| IndexedObject {
            oid: r.unwrap().0,
            offset: entry.offset,
            crc32: entry.crc32,
        })
        .collect();

    let (data, checksum) = if appended.is_empty() {
        let mut checksum = [0u8; 20];
        checksum.copy_from_slice(&data[end..]);
        (data.to_vec(), checksum)
    } else {
        let mut completed = data[..end].to_vec();
        for (object_type, content) in &appended {
            let offset = completed.len() as u64;
            let mut entry = entry_header(type_code(*object_type), content.len());
            entry.extend(compress(content));
            objects.push(IndexedObject {
                oid: Oid::from_bytes(hash_object(object_type.as_str(), content)),
                offset,
                crc32: crc32(&entry),
            });
            completed.extend(entry);
        }
        let total = (count as usize + appended.len()) as u32;
        completed[8..12].copy_from_slice(&total.to_be_bytes());
        let checksum = sha1(&completed);
        completed.extend_from_slice(&checksum);
        (completed, checksum)
    };

    let mut seen = std::collections::HashSet::new();
    for object in &objects {
        if !seen.insert(object.oid) {
            return Err(invalid(format!("duplicate object {}", object.oid)));
        }
    }
    Ok(IndexedPack {
        data,
        objects,
        checksum,
    })
}

/// Serializes a version 2 pack index.
pub(crate) fn write_index(objects: &[IndexedObject], pack_checksum: &[u8; 20]) -> Vec<u8> {
    let mut sorted = objects.to_vec();
    sorted.sort_by(|a, b| a.oid.as_bytes().cmp(b.oid.as_bytes()));
    let mut out = Vec::with_capacity(8 + 1024 + sorted.len() * 28 + 40);
    out.extend_from_slice(b"\xfftOc");
    out.extend_from_slice(&2u32.to_be_bytes());
    let mut fanout = [0u32; 256];
    for object in &sorted {
        fanout[object.oid.as_bytes()[0] as usize] += 1;
    }
    let mut total = 0;
    for count in fanout {
        total += count;
        out.extend_from_slice(&total.to_be_bytes());
    }
    for object in &sorted {
        out.extend_from_slice(object.oid.as_bytes());
    }
    for object in &sorted {
        out.extend_from_slice(&object.crc32.to_be_bytes());
    }
    let mut large = Vec::new();
    for object in &sorted {
        if object.offset < 0x8000_0000 {
            out.extend_from_slice(&(object.offset as u32).to_be_bytes());
        } else {
            out.extend_from_slice(&(0x8000_0000u32 | large.len() as u32).to_be_bytes());
            large.push(object.offset);
        }
    }
    for offset in large {
        out.extend_from_slice(&offset.to_be_bytes());
    }
    out.extend_from_slice(pack_checksum);
    let checksum = sha1(&out);
    out.extend_from_slice(&checksum);
    out
}

/// Builds a pack (version 2, no deltas) from objects.
pub(crate) fn write_pack(objects: &[(ObjectType, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for (object_type, content) in objects {
        out.extend(entry_header(type_code(*object_type), content.len()));
        out.extend(compress(content));
    }
    let checksum = sha1(&out);
    out.extend_from_slice(&checksum);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::pack::index::PackIndex;

    #[test]
    fn test_written_pack_indexes_and_round_trips() {
        let objects = vec![
            (ObjectType::Blob, b"hello\n".to_vec()),
            (ObjectType::Blob, vec![b'x'; 5000]),
            (ObjectType::Blob, Vec::new()),
        ];
        let pack = write_pack(&objects);
        let indexed = index_pack(&pack, &mut |_| Ok(None)).unwrap();
        assert_eq!(indexed.objects.len(), 3);
        assert_eq!(indexed.data, pack);
        let idx = write_index(&indexed.objects, &indexed.checksum);
        let parsed = PackIndex::parse(&idx).unwrap();
        assert_eq!(parsed.len(), 3);
        let oid = Oid::from_bytes(hash_object("blob", b"hello\n"));
        assert_eq!(parsed.get(&oid).unwrap().offset, 12);
    }

    #[test]
    fn test_corrupt_packs_are_rejected() {
        let mut pack = write_pack(&[(ObjectType::Blob, b"data".to_vec())]);
        let len = pack.len();
        pack[len - 1] ^= 1;
        assert!(index_pack(&pack, &mut |_| Ok(None)).is_err());
        assert!(index_pack(b"PACKxxxx", &mut |_| Ok(None)).is_err());
    }
}
