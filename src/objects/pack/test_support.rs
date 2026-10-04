//! Builders for synthetic packs used by unit tests.

use std::path::{Path, PathBuf};

use crate::infra::hash::sha1;
use crate::infra::{compress, crc32, hash_object};
use crate::objects::{ObjectType, Oid};

const HEADER_SIZE: u64 = 12;

pub(crate) fn oid_of(object_type: ObjectType, content: &[u8]) -> Oid {
    Oid::from_bytes(hash_object(object_type.as_str(), content))
}

pub(crate) fn size(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

/// A delta that copies all of `base` and appends `suffix`.
pub(crate) fn append_delta(base: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut delta = size(base.len());
    delta.extend(size(base.len() + suffix.len()));
    if !base.is_empty() {
        delta.push(0xf0);
        delta.extend_from_slice(&(base.len() as u32).to_le_bytes()[..3]);
    }
    for chunk in suffix.chunks(127) {
        delta.push(chunk.len() as u8);
        delta.extend_from_slice(chunk);
    }
    delta
}

pub(crate) fn ofs(mut distance: u64) -> Vec<u8> {
    let mut out = vec![(distance & 0x7f) as u8];
    distance >>= 7;
    while distance != 0 {
        distance -= 1;
        out.insert(0, 0x80 | (distance & 0x7f) as u8);
        distance >>= 7;
    }
    out
}

#[derive(Default)]
pub(crate) struct Builder {
    pub(crate) data: Vec<u8>,
    pub(crate) index: Vec<(Oid, u64, u32)>,
}

impl Builder {
    pub(crate) fn next_offset(&self) -> u64 {
        HEADER_SIZE + self.data.len() as u64
    }

    pub(crate) fn entry(
        &mut self,
        oid: Oid,
        type_code: u8,
        size: u64,
        base: &[u8],
        payload: &[u8],
    ) -> u64 {
        let offset = self.next_offset();
        let mut raw = Vec::new();
        let mut byte = (type_code << 4) | (size & 0x0f) as u8;
        let mut rest = size >> 4;
        while rest != 0 {
            raw.push(byte | 0x80);
            byte = (rest & 0x7f) as u8;
            rest >>= 7;
        }
        raw.push(byte);
        raw.extend_from_slice(base);
        raw.extend_from_slice(payload);
        self.index.push((oid, offset, crc32(&raw)));
        self.data.extend(raw);
        offset
    }

    pub(crate) fn object(&mut self, object_type: ObjectType, content: &[u8]) -> (Oid, u64) {
        let oid = oid_of(object_type, content);
        let code = match object_type {
            ObjectType::Commit => 1,
            ObjectType::Tree => 2,
            ObjectType::Blob => 3,
            ObjectType::Tag => 4,
        };
        let offset = self.entry(oid, code, content.len() as u64, &[], &compress(content));
        (oid, offset)
    }

    pub(crate) fn ofs_delta(&mut self, oid: Oid, base: u64, delta: &[u8]) -> u64 {
        let distance = ofs(self.next_offset() - base);
        self.entry(oid, 6, delta.len() as u64, &distance, &compress(delta))
    }

    pub(crate) fn ref_delta(&mut self, oid: Oid, base: Oid, delta: &[u8]) -> u64 {
        self.entry(
            oid,
            7,
            delta.len() as u64,
            base.as_bytes(),
            &compress(delta),
        )
    }

    /// Writes the pack (with `version` and the builder's count) and an index.
    pub(crate) fn write(&self, dir: &Path, version: u32) -> PathBuf {
        self.write_named(dir, "test", version)
    }

    /// Writes `<name>.pack` and `<name>.idx` into `dir`.
    pub(crate) fn write_named(&self, dir: &Path, name: &str, version: u32) -> PathBuf {
        let mut body = b"PACK".to_vec();
        body.extend_from_slice(&version.to_be_bytes());
        body.extend_from_slice(&(self.index.len() as u32).to_be_bytes());
        body.extend_from_slice(&self.data);
        write_pack_named(dir, name, body, &self.index)
    }
}

/// Seals `body` with its checksum and writes `test.pack` with a matching v2 index.
pub(crate) fn write_pack(dir: &Path, body: Vec<u8>, entries: &[(Oid, u64, u32)]) -> PathBuf {
    write_pack_named(dir, "test", body, entries)
}

fn write_pack_named(
    dir: &Path,
    name: &str,
    mut body: Vec<u8>,
    entries: &[(Oid, u64, u32)],
) -> PathBuf {
    let checksum = sha1(&body);
    body.extend_from_slice(&checksum);
    let mut sorted = entries.to_vec();
    sorted.sort();
    let mut idx = vec![0xff, b't', b'O', b'c', 0, 0, 0, 2];
    for first in 0..=255u8 {
        let count = sorted.iter().filter(|e| e.0.as_bytes()[0] <= first).count();
        idx.extend_from_slice(&(count as u32).to_be_bytes());
    }
    for (oid, _, _) in &sorted {
        idx.extend_from_slice(oid.as_bytes());
    }
    for (_, _, crc) in &sorted {
        idx.extend_from_slice(&crc.to_be_bytes());
    }
    for (_, offset, _) in &sorted {
        idx.extend_from_slice(&(*offset as u32).to_be_bytes());
    }
    idx.extend_from_slice(&checksum);
    let idx_checksum = sha1(&idx);
    idx.extend_from_slice(&idx_checksum);
    let path = dir.join(format!("{}.pack", name));
    std::fs::write(&path, body).unwrap();
    std::fs::write(path.with_extension("idx"), idx).unwrap();
    path
}
