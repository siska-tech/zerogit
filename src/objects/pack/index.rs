//! SHA-1 pack index v2 parsing and lookup.

use std::collections::HashSet;
use std::path::Path;

use crate::error::{Error, Result};
use crate::infra::{hash::sha1, read_file};
use crate::objects::Oid;

const MAGIC: [u8; 4] = [0xff, b't', b'O', b'c'];
const FANOUT_START: usize = 8;
const OIDS_START: usize = FANOUT_START + 256 * 4;
const TRAILER_SIZE: usize = 40;

/// One object in an index, ordered by its OID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackIndexEntry {
    /// The reconstructed object's SHA-1 object ID.
    pub oid: Oid,
    /// The byte offset of the packed object header, including 64-bit offsets.
    pub offset: u64,
    /// CRC32 of the packed entry, to be checked against the pack's bytes.
    pub crc32: u32,
}

/// A validated SHA-1 pack index v2.
///
/// Parsing validates the index checksum, table geometry, fanout and offsets.
/// The pack checksum and entry CRCs are retained for the pack reader to verify;
/// parsing an index alone does not validate the corresponding pack file.
#[derive(Debug)]
pub struct PackIndex {
    fanout: [u32; 256],
    entries: Vec<PackIndexEntry>,
    pack_checksum: [u8; 20],
    index_checksum: [u8; 20],
}

fn invalid(reason: &str) -> Error {
    Error::InvalidPackIndex {
        reason: reason.to_owned(),
    }
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N]> {
    let end = at
        .checked_add(N)
        .ok_or_else(|| invalid("table position overflow"))?;
    data.get(at..end)
        .ok_or_else(|| invalid("truncated index"))?
        .try_into()
        .map_err(|_| invalid("truncated index"))
}

fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(bytes(data, at)?))
}

fn table_end(start: usize, count: usize, width: usize) -> Result<usize> {
    count
        .checked_mul(width)
        .and_then(|len| start.checked_add(len))
        .ok_or_else(|| invalid("table size overflow"))
}

/// v1 has no magic/version fields; distinguish its layout from a bad v2 magic.
fn looks_like_v1(data: &[u8]) -> bool {
    if data.len() < 256 * 4 + TRAILER_SIZE {
        return false;
    }
    let mut previous = 0;
    for i in 0..256 {
        let Ok(value) = word(data, i * 4) else {
            return false;
        };
        if value < previous {
            return false;
        }
        previous = value;
    }
    let Ok(count) = usize::try_from(previous) else {
        return false;
    };
    table_end(256 * 4 + TRAILER_SIZE, count, 24).ok() == Some(data.len())
}

impl PackIndex {
    /// Reads and validates an index file. Only SHA-1 idx v2 is supported.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::parse(&read_file(path)?)
    }

    /// Parses an entire idx file, preserving all prefix matches for callers.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if bytes::<4>(data, 0)? != MAGIC {
            return Err(if looks_like_v1(data) {
                Error::UnsupportedPackIndexVersion(1)
            } else {
                invalid("invalid magic")
            });
        }
        let version = word(data, 4)?;
        if version != 2 {
            return Err(Error::UnsupportedPackIndexVersion(version));
        }
        if data.len() < OIDS_START + TRAILER_SIZE {
            return Err(invalid("truncated index"));
        }
        let index_checksum = bytes::<20>(data, data.len() - 20)?;
        if sha1(&data[..data.len() - 20]) != index_checksum {
            return Err(invalid("index checksum mismatch"));
        }
        let trailer = data.len() - TRAILER_SIZE;
        let pack_checksum = bytes::<20>(data, trailer)?;
        let mut fanout = [0u32; 256];
        let mut previous = 0;
        for (i, value) in fanout.iter_mut().enumerate() {
            *value = word(data, FANOUT_START + i * 4)?;
            if *value < previous {
                return Err(invalid("fanout is not monotonic"));
            }
            previous = *value;
        }
        let count = usize::try_from(fanout[255]).map_err(|_| invalid("object count overflow"))?;
        let crc_start = table_end(OIDS_START, count, 20)?;
        let offset_start = table_end(crc_start, count, 4)?;
        let large_start = table_end(offset_start, count, 4)?;
        let large_size = trailer
            .checked_sub(large_start)
            .ok_or_else(|| invalid("truncated tables"))?;
        if large_size % 8 != 0 {
            return Err(invalid("invalid 64-bit offset table size"));
        }
        let large_count = large_size / 8;
        if large_count > count {
            return Err(invalid("too many 64-bit offsets"));
        }

        // Allocation happens only after all declared tables fit in the input.
        let mut entries: Vec<PackIndexEntry> = Vec::with_capacity(count);
        let mut actual_fanout = [0u32; 256];
        let mut used_large = vec![false; large_count];
        let mut offsets = HashSet::with_capacity(count);
        for i in 0..count {
            let oid = Oid::from_bytes(bytes(data, OIDS_START + i * 20)?);
            if entries.last().is_some_and(|entry| entry.oid >= oid) {
                return Err(invalid("OIDs are not strictly sorted"));
            }
            actual_fanout[oid.as_bytes()[0] as usize] += 1;
            let encoded = word(data, offset_start + i * 4)?;
            let offset = if encoded & 0x8000_0000 == 0 {
                u64::from(encoded)
            } else {
                let slot = usize::try_from(encoded & 0x7fff_ffff)
                    .map_err(|_| invalid("64-bit offset slot overflow"))?;
                let used = used_large
                    .get_mut(slot)
                    .ok_or_else(|| invalid("64-bit offset slot out of range"))?;
                if *used {
                    return Err(invalid("duplicate 64-bit offset slot"));
                }
                *used = true;
                u64::from_be_bytes(bytes(data, large_start + slot * 8)?)
            };
            if offset < 12 {
                return Err(invalid("offset overlaps pack header"));
            }
            if !offsets.insert(offset) {
                return Err(invalid("duplicate object offset"));
            }
            entries.push(PackIndexEntry {
                oid,
                offset,
                crc32: word(data, crc_start + i * 4)?,
            });
        }
        if used_large.iter().any(|used| !used) {
            return Err(invalid("unreferenced 64-bit offset"));
        }
        let mut cumulative = 0u32;
        for (actual, declared) in actual_fanout.into_iter().zip(fanout) {
            cumulative = cumulative
                .checked_add(actual)
                .ok_or_else(|| invalid("fanout count overflow"))?;
            if cumulative != declared {
                return Err(invalid("fanout does not match OID table"));
            }
        }
        Ok(Self {
            fanout,
            entries,
            pack_checksum,
            index_checksum,
        })
    }

    /// Returns the number of indexed objects.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the index contains no objects.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns all index entries in ascending OID order.
    pub fn entries(&self) -> &[PackIndexEntry] {
        &self.entries
    }

    /// Returns the expected SHA-1 trailer checksum of the corresponding pack.
    pub fn pack_checksum(&self) -> &[u8; 20] {
        &self.pack_checksum
    }

    /// Returns the verified SHA-1 checksum of this index's preceding bytes.
    pub fn index_checksum(&self) -> &[u8; 20] {
        &self.index_checksum
    }

    fn bucket(&self, first: u8) -> &[PackIndexEntry] {
        let start = if first == 0 {
            0
        } else {
            self.fanout[first as usize - 1] as usize
        };
        let end = self.fanout[first as usize] as usize;
        &self.entries[start..end]
    }

    /// Finds an exact OID using fanout and binary search.
    pub fn get(&self, oid: &Oid) -> Option<&PackIndexEntry> {
        let bucket = self.bucket(oid.as_bytes()[0]);
        bucket
            .binary_search_by_key(oid, |entry| entry.oid)
            .ok()
            .map(|i| &bucket[i])
    }

    /// Returns all matches for a 4–40 digit hexadecimal prefix, case-insensitively.
    ///
    /// Odd-length prefixes are supported. Ambiguous prefixes return every match
    /// so an object store can resolve ambiguity across loose objects and packs.
    pub fn find_objects_by_prefix(&self, prefix: &str) -> Result<Vec<Oid>> {
        if !(4..=40).contains(&prefix.len()) || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidOid(prefix.to_owned()));
        }
        let prefix = prefix.to_ascii_lowercase();
        let low = Oid::from_hex(&format!("{:0<40}", prefix))?;
        let high = Oid::from_hex(&format!("{:f<40}", prefix))?;
        let bucket = self.bucket(low.as_bytes()[0]);
        let start = bucket.partition_point(|entry| entry.oid < low);
        let end = bucket.partition_point(|entry| entry.oid <= high);
        Ok(bucket[start..end].iter().map(|entry| entry.oid).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(hex: &str) -> Oid {
        Oid::from_hex(hex).unwrap()
    }

    /// Appends the index checksum to a body.
    fn seal(mut body: Vec<u8>) -> Vec<u8> {
        let checksum = sha1(&body);
        body.extend_from_slice(&checksum);
        body
    }

    /// Builds a v2 index body without its checksum; offsets at or above 2^31
    /// go to the 64-bit table, followed by `extra_large` unreferenced slots.
    fn body(objects: &[(Oid, u64, u32)], extra_large: &[u8]) -> Vec<u8> {
        let mut sorted = objects.to_vec();
        sorted.sort_by_key(|(oid, _, _)| *oid);
        let mut data = MAGIC.to_vec();
        data.extend_from_slice(&2u32.to_be_bytes());
        for first in 0..256usize {
            let count = sorted
                .iter()
                .filter(|(oid, _, _)| oid.as_bytes()[0] as usize <= first)
                .count() as u32;
            data.extend_from_slice(&count.to_be_bytes());
        }
        for (oid, _, _) in &sorted {
            data.extend_from_slice(oid.as_bytes());
        }
        for (_, _, crc) in &sorted {
            data.extend_from_slice(&crc.to_be_bytes());
        }
        let mut large = Vec::new();
        for (_, offset, _) in &sorted {
            let encoded = if *offset < 0x8000_0000 {
                *offset as u32
            } else {
                large.push(*offset);
                0x8000_0000 | (large.len() as u32 - 1)
            };
            data.extend_from_slice(&encoded.to_be_bytes());
        }
        for offset in large {
            data.extend_from_slice(&offset.to_be_bytes());
        }
        data.extend_from_slice(extra_large);
        data.extend_from_slice(&[0xaa; 20]);
        data
    }

    fn build(objects: &[(Oid, u64, u32)]) -> Vec<u8> {
        seal(body(objects, &[]))
    }

    /// Edits a sealed index body and recomputes its checksum.
    fn mutate(data: &[u8], f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut body = data[..data.len() - 20].to_vec();
        f(&mut body);
        seal(body)
    }

    fn put(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn reason(result: Result<PackIndex>) -> String {
        match result {
            Err(Error::InvalidPackIndex { reason }) => reason,
            other => panic!("expected InvalidPackIndex, got {:?}", other),
        }
    }

    fn sample() -> Vec<(Oid, u64, u32)> {
        vec![
            (oid("00000000000000000000000000000000000000ff"), 12, 1),
            (oid("abcdef0000000000000000000000000000000001"), 100, 2),
            (oid("abcdef0000000000000000000000000000000002"), 200, 3),
            (oid("abcdee0000000000000000000000000000000003"), 300, 4),
            (oid("ffffffffffffffffffffffffffffffffffffffff"), 400, 5),
        ]
    }

    fn large_pair() -> [(Oid, u64, u32); 2] {
        [
            (
                oid("1111111111111111111111111111111111111111"),
                0x8000_0000,
                1,
            ),
            (
                oid("2222222222222222222222222222222222222222"),
                0x9000_0000,
                2,
            ),
        ]
    }

    #[test]
    fn parses_entries_and_checksums() {
        let data = build(&sample());
        let index = PackIndex::parse(&data).unwrap();
        assert_eq!(index.len(), 5);
        assert!(!index.is_empty());
        assert_eq!(index.pack_checksum(), &[0xaa; 20]);
        assert_eq!(index.index_checksum()[..], data[data.len() - 20..]);
        let oids: Vec<_> = index.entries().iter().map(|e| e.oid).collect();
        let mut expected: Vec<_> = sample().into_iter().map(|(oid, _, _)| oid).collect();
        expected.sort();
        assert_eq!(oids, expected);
        for (oid, offset, crc32) in sample() {
            assert_eq!(
                index.get(&oid),
                Some(&PackIndexEntry { oid, offset, crc32 })
            );
        }
        assert!(index
            .get(&oid("abcdef0000000000000000000000000000000003"))
            .is_none());
    }

    #[test]
    fn parses_empty_index() {
        let index = PackIndex::parse(&build(&[])).unwrap();
        assert!(index.is_empty());
        assert!(index.find_objects_by_prefix("abcd").unwrap().is_empty());
    }

    #[test]
    fn resolves_64bit_offsets() {
        let objects = [
            (
                oid("1111111111111111111111111111111111111111"),
                0x7fff_ffff,
                1,
            ),
            (
                oid("2222222222222222222222222222222222222222"),
                0x8000_0000,
                2,
            ),
            (
                oid("3333333333333333333333333333333333333333"),
                0x1_2345_6789_abcd,
                3,
            ),
        ];
        let index = PackIndex::parse(&build(&objects)).unwrap();
        for (oid, offset, _) in objects {
            assert_eq!(index.get(&oid).unwrap().offset, offset);
        }
    }

    #[test]
    fn finds_objects_by_prefix() {
        let index = PackIndex::parse(&build(&sample())).unwrap();
        assert_eq!(index.find_objects_by_prefix("abcdef").unwrap().len(), 2);
        assert_eq!(index.find_objects_by_prefix("ABCDE").unwrap().len(), 3);
        assert_eq!(
            index
                .find_objects_by_prefix("abcdef0000000000000000000000000000000002")
                .unwrap(),
            vec![oid("abcdef0000000000000000000000000000000002")]
        );
        assert_eq!(index.find_objects_by_prefix("ffff").unwrap().len(), 1);
        assert_eq!(index.find_objects_by_prefix("0000").unwrap().len(), 1);
        assert!(index.find_objects_by_prefix("1234").unwrap().is_empty());
        let too_long = "a".repeat(41);
        for bad in ["abc", "abcg", "", too_long.as_str()] {
            assert!(matches!(
                index.find_objects_by_prefix(bad),
                Err(Error::InvalidOid(_))
            ));
        }
    }

    #[test]
    fn rejects_unsupported_versions() {
        let v3 = mutate(&build(&sample()), |d| put(d, 4, 3));
        assert!(matches!(
            PackIndex::parse(&v3),
            Err(Error::UnsupportedPackIndexVersion(3))
        ));

        // v1: fanout, (offset, oid) records, then pack and index checksums.
        let mut v1 = Vec::new();
        for first in 0..256u32 {
            v1.extend_from_slice(&u32::from(first >= 0x11).to_be_bytes());
        }
        v1.extend_from_slice(&12u32.to_be_bytes());
        v1.extend_from_slice(&[0x11; 20]);
        v1.extend_from_slice(&[0; 40]);
        assert!(matches!(
            PackIndex::parse(&v1),
            Err(Error::UnsupportedPackIndexVersion(1))
        ));
    }

    #[test]
    fn rejects_bad_magic_and_checksum() {
        let data = build(&sample());
        let mut magic = data.clone();
        magic[0] = 0;
        assert_eq!(reason(PackIndex::parse(&magic)), "invalid magic");

        let mut corrupt = data;
        corrupt[OIDS_START] ^= 1;
        assert_eq!(
            reason(PackIndex::parse(&corrupt)),
            "index checksum mismatch"
        );
    }

    #[test]
    fn truncation_never_panics() {
        let data = build(&[
            (oid("1111111111111111111111111111111111111111"), 12, 1),
            (
                oid("2222222222222222222222222222222222222222"),
                0x8000_0000,
                2,
            ),
        ]);
        for len in 0..data.len() {
            assert!(PackIndex::parse(&data[..len]).is_err(), "length {}", len);
            // Also reseal so truncation is caught by geometry, not the checksum.
            if len >= 20 {
                assert!(PackIndex::parse(&seal(data[..len - 20].to_vec())).is_err());
            }
        }
    }

    #[test]
    fn rejects_inconsistent_tables() {
        let data = build(&sample());
        let fanout = |i: usize| FANOUT_START + i * 4;

        let decreasing = mutate(&data, |d| put(d, fanout(10), 9));
        assert_eq!(
            reason(PackIndex::parse(&decreasing)),
            "fanout is not monotonic"
        );

        let shifted = mutate(&data, |d| put(d, fanout(0), 0));
        assert_eq!(
            reason(PackIndex::parse(&shifted)),
            "fanout does not match OID table"
        );

        let huge = mutate(&data, |d| {
            for i in 0..256 {
                put(d, fanout(i), u32::MAX);
            }
        });
        assert_eq!(reason(PackIndex::parse(&huge)), "truncated tables");

        let unsorted = mutate(&data, |d| {
            let (a, b) = (OIDS_START + 20, OIDS_START + 40);
            let first = d[a..b].to_vec();
            d.copy_within(b..b + 20, a);
            d[b..b + 20].copy_from_slice(&first);
        });
        assert_eq!(
            reason(PackIndex::parse(&unsorted)),
            "OIDs are not strictly sorted"
        );
    }

    #[test]
    fn rejects_invalid_offsets() {
        let data = build(&sample());
        let offsets_at = OIDS_START + sample().len() * 24;
        let set = |value: u32| mutate(&data, |d| put(d, offsets_at + 4, value));
        assert_eq!(
            reason(PackIndex::parse(&set(11))),
            "offset overlaps pack header"
        );
        assert_eq!(
            reason(PackIndex::parse(&set(12))),
            "duplicate object offset"
        );
        assert_eq!(
            reason(PackIndex::parse(&set(0x8000_0000))),
            "64-bit offset slot out of range"
        );

        let large = build(&large_pair());
        let offsets_at = OIDS_START + 2 * 24;
        let reuse = mutate(&large, |d| put(d, offsets_at + 4, 0x8000_0000));
        assert_eq!(
            reason(PackIndex::parse(&reuse)),
            "duplicate 64-bit offset slot"
        );

        let too_many = seal(body(&large_pair(), &0xa000_0000u64.to_be_bytes()));
        assert_eq!(
            reason(PackIndex::parse(&too_many)),
            "too many 64-bit offsets"
        );

        let ragged = seal(body(&large_pair(), &[0; 4]));
        assert_eq!(
            reason(PackIndex::parse(&ragged)),
            "invalid 64-bit offset table size"
        );

        let objects = [large_pair()[0], (large_pair()[1].0, 12, 2)];
        let unreferenced = seal(body(&objects, &0x9000_0000u64.to_be_bytes()));
        assert_eq!(
            reason(PackIndex::parse(&unreferenced)),
            "unreferenced 64-bit offset"
        );
    }
}
