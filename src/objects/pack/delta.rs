//! Git delta instructions: decoding ([`apply`]) and encoding ([`create`]).
//!
//! A delta starts with the base size and the result size, then has two
//! kinds of instructions: copy (a range of the base) and insert (literal
//! bytes). The encoder indexes the base at block boundaries with a rolling
//! hash, slides the same hash over the target, and copies the longest
//! verified match at each position, extended backwards over bytes not yet
//! encoded; everything else is inserted.

use crate::error::{Error, Result};

fn invalid(reason: &str) -> Error {
    Error::InvalidPack {
        reason: format!("invalid delta: {}", reason),
    }
}

/// Reads a little-endian base-128 size from the start of a delta.
fn read_size(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *data.get(*pos).ok_or_else(|| invalid("truncated size"))?;
        *pos += 1;
        if shift > 63 || (shift > 0 && u64::from(byte & 0x7f) >> (64 - shift) != 0) {
            return Err(invalid("size overflow"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

/// Returns the result size declared by a delta without applying it.
pub(crate) fn result_size(delta: &[u8]) -> Result<u64> {
    let mut pos = 0;
    read_size(delta, &mut pos)?;
    read_size(delta, &mut pos)
}

/// Applies `delta` to `base`, rejecting results larger than `max_size`.
pub(crate) fn apply(base: &[u8], delta: &[u8], max_size: u64) -> Result<Vec<u8>> {
    let mut pos = 0;
    let base_size = read_size(delta, &mut pos)?;
    if base_size != base.len() as u64 {
        return Err(invalid("base size mismatch"));
    }
    let size = read_size(delta, &mut pos)?;
    if size > max_size {
        return Err(Error::PackLimitExceeded {
            reason: format!("delta result of {} bytes exceeds {}", size, max_size),
        });
    }
    let size = usize::try_from(size).map_err(|_| invalid("result size overflow"))?;
    let mut output = Vec::with_capacity(size);
    while pos < delta.len() {
        let op = delta[pos];
        pos += 1;
        let chunk = if op & 0x80 != 0 {
            let mut fields = [0u64; 2];
            for (field, (first_bit, width)) in fields.iter_mut().zip([(0, 4), (4, 3)]) {
                for i in 0..width {
                    if op & (1 << (first_bit + i)) != 0 {
                        let byte = *delta
                            .get(pos)
                            .ok_or_else(|| invalid("truncated copy instruction"))?;
                        pos += 1;
                        *field |= u64::from(byte) << (8 * i);
                    }
                }
            }
            let [offset, length] = fields;
            let length = if length == 0 { 0x10000 } else { length };
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= base.len() as u64)
                .ok_or_else(|| invalid("copy out of base range"))?;
            &base[offset as usize..end as usize]
        } else if op != 0 {
            let end = pos + usize::from(op);
            let data = delta
                .get(pos..end)
                .ok_or_else(|| invalid("truncated insert instruction"))?;
            pos = end;
            data
        } else {
            return Err(invalid("reserved opcode 0"));
        };
        if chunk.len() > size - output.len() {
            return Err(invalid("result exceeds declared size"));
        }
        output.extend_from_slice(chunk);
    }
    if output.len() != size {
        return Err(invalid("result shorter than declared size"));
    }
    Ok(output)
}

/// The bytes hashed together: the base is indexed at multiples of this, and
/// a match is at least this long.
const BLOCK: usize = 16;
/// The odd multiplier of the rolling hash.
const MULTIPLIER: u32 = 0x0100_0193;
/// The base offsets tried for one target position.
const MAX_CANDIDATES: usize = 64;
/// The longest copy instruction written. The format allows 0xffffff, but
/// copies of at most 64 KiB are what every Git version reads.
const MAX_COPY: usize = 0x10000;
/// A copy this long is good enough: no longer match is looked for inside it.
const LONG_MATCH: usize = 4096;
/// The longest insert instruction.
const MAX_INSERT: usize = 0x7f;

/// `MULTIPLIER` to the power `BLOCK - 1`, to take the oldest byte out of
/// the rolling hash.
fn top_power() -> u32 {
    (1..BLOCK).fold(1u32, |power, _| power.wrapping_mul(MULTIPLIER))
}

/// The hash of one block.
fn block_hash(block: &[u8]) -> u32 {
    block.iter().fold(0u32, |hash, &byte| {
        hash.wrapping_mul(MULTIPLIER).wrapping_add(u32::from(byte))
    })
}

/// Where the base offsets of blocks with the hash `hash` are chained.
fn bucket(hash: u32, bits: u32) -> usize {
    // The low bits of a polynomial hash mod 2^32 mix poorly: spread the
    // whole hash into the top bits and take those.
    (hash ^ (hash >> 16)).wrapping_mul(0x9e37_79b1) as usize >> (32 - bits)
}

/// The blocks of a delta base, found by hash.
pub(crate) struct DeltaIndex {
    bits: u32,
    /// The first base offset (plus one; 0 ends a chain) of each bucket.
    heads: Vec<u32>,
    /// The next offset (plus one) in the chain of each block.
    next: Vec<u32>,
}

impl DeltaIndex {
    /// Indexes `base`, or returns `None` if it is too large for a delta
    /// (offsets must fit in 32 bits).
    pub(crate) fn new(base: &[u8]) -> Option<Self> {
        if base.len() > u32::MAX as usize - 1 {
            return None;
        }
        let blocks = base.len() / BLOCK;
        let bits = (blocks.max(16).next_power_of_two().trailing_zeros()).min(30);
        let mut index = DeltaIndex {
            bits,
            heads: vec![0; 1 << bits],
            next: vec![0; blocks],
        };
        // Last block first, so each chain lists offsets in base order.
        for block in (0..blocks).rev() {
            let offset = block * BLOCK;
            let slot = bucket(block_hash(&base[offset..offset + BLOCK]), bits);
            index.next[block] = index.heads[slot];
            index.heads[slot] = offset as u32 + 1;
        }
        Some(index)
    }

    /// The longest match of `target[at..]` in `base` that starts with a
    /// block of hash `hash` and reaches past `target[..past]`: (base
    /// offset, length), or a length of 0.
    fn longest_match(
        &self,
        base: &[u8],
        target: &[u8],
        at: usize,
        hash: u32,
        past: usize,
    ) -> (usize, usize) {
        let mut best = (0, 0);
        let mut link = self.heads[bucket(hash, self.bits)];
        let mut tried = 0;
        while link != 0 && tried < MAX_CANDIDATES {
            let offset = link as usize - 1;
            link = self.next[offset / BLOCK];
            tried += 1;
            // Most candidates fail on the first byte that matters.
            if past > at && base.get(offset + past - at) != target.get(past) {
                continue;
            }
            let length = base[offset..]
                .iter()
                .zip(&target[at..])
                .take_while(|(a, b)| a == b)
                .count();
            if length >= BLOCK && at + length > past && length > best.1 {
                best = (offset, length);
                if at + length == target.len() {
                    break;
                }
            }
        }
        best
    }
}

/// Appends a base-128 size, low bits first.
fn write_size(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn write_inserts(out: &mut Vec<u8>, mut data: &[u8]) {
    while !data.is_empty() {
        let length = data.len().min(MAX_INSERT);
        out.push(length as u8);
        out.extend_from_slice(&data[..length]);
        data = &data[length..];
    }
}

fn write_copies(out: &mut Vec<u8>, mut offset: usize, mut length: usize) {
    while length > 0 {
        let chunk = length.min(MAX_COPY);
        let at = out.len();
        out.push(0x80);
        for byte in 0..4 {
            let value = (offset >> (8 * byte)) as u8;
            if value != 0 {
                out[at] |= 1 << byte;
                out.push(value);
            }
        }
        // A size of 0x10000 is written as no size bytes.
        if chunk != MAX_COPY {
            for byte in 0..3 {
                let value = (chunk >> (8 * byte)) as u8;
                if value != 0 {
                    out[at] |= 0x10 << byte;
                    out.push(value);
                }
            }
        }
        offset += chunk;
        length -= chunk;
    }
}

/// Encodes `target` as a delta against `base` (indexed by `index`), or
/// returns `None` if the delta would be larger than `max_size` bytes.
pub(crate) fn create(
    base: &[u8],
    index: &DeltaIndex,
    target: &[u8],
    max_size: usize,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    write_size(&mut out, base.len());
    write_size(&mut out, target.len());
    let top = top_power();
    // The last copy (base offset, target start, length) is held back, so
    // that a longer match found inside or after it can take over its end.
    // It ends at `literal`; target bytes from `literal` to `at` are to be
    // inserted.
    let mut copy: Option<(usize, usize, usize)> = None;
    let mut literal = 0;
    let mut at = 0;
    let mut hash = if target.len() >= BLOCK {
        block_hash(&target[..BLOCK])
    } else {
        0
    };
    while at + BLOCK <= target.len() {
        let (offset, length) = index.longest_match(base, target, at, hash, literal);
        if length > 0 && at + length > literal {
            // The bytes just before may match too: those to be inserted
            // and the held-back copy.
            let floor = copy.map_or(literal, |(_, start, _)| start);
            let back = base[..offset]
                .iter()
                .rev()
                .zip(target[floor..at].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let start = at - back;
            if let Some((held_offset, held_start, held)) = copy.take() {
                let kept = (start - held_start).min(held);
                if kept > 0 {
                    write_copies(&mut out, held_offset, kept);
                }
            }
            if start > literal {
                write_inserts(&mut out, &target[literal..start]);
            }
            if out.len() > max_size {
                return None;
            }
            copy = Some((offset - back, start, length + back));
            literal = at + length;
        } else if at >= literal && out.len() + at + 1 - literal > max_size {
            // Inserting what is pending already makes the delta too large.
            return None;
        }
        // Inside a short copy, keep looking for a longer match at every
        // byte; past a long one, go on after it.
        // Nothing can reach past the end of the target.
        let long = copy.is_some_and(|(_, _, held)| held >= LONG_MATCH) || literal == target.len();
        if long && at < literal {
            at = literal;
            if at + BLOCK <= target.len() {
                hash = block_hash(&target[at..at + BLOCK]);
            }
        } else {
            if at + BLOCK < target.len() {
                hash = hash
                    .wrapping_sub(u32::from(target[at]).wrapping_mul(top))
                    .wrapping_mul(MULTIPLIER)
                    .wrapping_add(u32::from(target[at + BLOCK]));
            }
            at += 1;
        }
    }
    if let Some((held_offset, _, held)) = copy {
        write_copies(&mut out, held_offset, held);
    }
    write_inserts(&mut out, &target[literal..]);
    (out.len() <= max_size).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn delta(base: u64, result: u64, ops: &[u8]) -> Vec<u8> {
        let mut out = size(base);
        out.extend(size(result));
        out.extend_from_slice(ops);
        out
    }

    fn reason(result: Result<Vec<u8>>) -> String {
        match result {
            Err(Error::InvalidPack { reason }) => reason,
            other => panic!("expected InvalidPack, got {:?}", other),
        }
    }

    /// A deterministic pseudo-random byte stream.
    fn noise(seed: u32, len: usize) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect()
    }

    fn round_trip(base: &[u8], target: &[u8]) -> usize {
        let index = DeltaIndex::new(base).unwrap();
        let delta = create(base, &index, target, usize::MAX).unwrap();
        assert_eq!(result_size(&delta).unwrap(), target.len() as u64);
        assert_eq!(apply(base, &delta, u64::MAX).unwrap(), target);
        delta.len()
    }

    #[test]
    fn created_deltas_reproduce_the_target() {
        let text: Vec<u8> = (0..400)
            .flat_map(|i| format!("line {} of a text file\n", i).into_bytes())
            .collect();
        // An edit in the middle, appended and removed lines: small deltas.
        let mut edited = text.clone();
        edited.splice(3000..3010, b"changed!".iter().copied());
        edited.extend_from_slice(b"one more line\n");
        assert!(round_trip(&text, &edited) < 60);
        assert!(round_trip(&text, &text[500..]) < 20);
        assert!(round_trip(&edited, &text) < 60);
        // Unrelated, empty and tiny inputs, and repeated blocks.
        let random = noise(1, 5000);
        assert!(round_trip(&text, &random) > 5000);
        round_trip(&[], &text);
        round_trip(&text, &[]);
        round_trip(b"abc", b"abcd");
        round_trip(&vec![0u8; 100_000], &vec![0u8; 150_001]);
        // Copies longer than 64 KiB, at offsets past 16 MiB.
        let mut big = noise(2, 17 << 20);
        let target: Vec<u8> = big[(16 << 20) + 5..].to_vec();
        assert!(round_trip(&big, &target) < 200);
        big.truncate(1 << 20);
        let mut target = noise(3, 300);
        target.extend_from_slice(&big[1000..200_000]);
        target.extend_from_slice(&noise(4, 300));
        round_trip(&big, &target);
    }

    #[test]
    fn random_edits_round_trip() {
        for seed in 0..50 {
            let base = noise(seed, 2000 + seed as usize * 37);
            let mut target = base.clone();
            let ops = noise(seed + 1000, 30);
            for pair in ops.chunks(3) {
                let at = usize::from(pair[0]) * target.len() / 256;
                match pair[1] % 3 {
                    0 => target.splice(at..at, noise(seed + u32::from(pair[2]), 40)),
                    1 => target.splice(at..(at + 50).min(target.len()), Vec::new()),
                    _ => target.splice(at..at, base[..usize::from(pair[2])].to_vec()),
                };
            }
            round_trip(&base, &target);
        }
    }

    #[test]
    fn too_large_deltas_are_abandoned() {
        let base = noise(5, 4000);
        let index = DeltaIndex::new(&base).unwrap();
        assert!(create(&base, &index, &noise(6, 4000), 1000).is_none());
        let mut target = base.clone();
        target[2000] ^= 1;
        let delta = create(&base, &index, &target, 1000).unwrap();
        assert!(create(&base, &index, &target, delta.len() - 1).is_none());
    }

    #[test]
    fn applies_copy_and_insert() {
        let base = b"hello, world";
        // copy offset 7 length 5, insert ", ", copy offset 0 length 5
        let ops = [0x91, 7, 5, 2, b',', b' ', 0x90, 5];
        let d = delta(12, 12, &ops);
        assert_eq!(result_size(&d).unwrap(), 12);
        assert_eq!(apply(base, &d, 100).unwrap(), b"world, hello");
    }

    #[test]
    fn copy_length_zero_means_64k() {
        let base = vec![7u8; 0x10000 + 3];
        let d = delta(base.len() as u64, 0x10000 + 1, &[0x81, 3, 1, b'x']);
        let out = apply(&base, &d, u64::MAX).unwrap();
        assert_eq!(out.len(), 0x10001);
        assert_eq!(out.last(), Some(&b'x'));
    }

    #[test]
    fn multi_byte_copy_fields() {
        let mut base = vec![0u8; 0x20000];
        base[0x1_0203] = 0xab;
        // offset bytes 0..3, size bytes 0..1
        let d = delta(0x20000, 2, &[0xb7, 0x03, 0x02, 0x01, 0x02, 0x00]);
        assert_eq!(apply(&base, &d, 10).unwrap(), [0xab, 0]);
    }

    #[test]
    fn rejects_malformed_deltas() {
        let base = b"abcdef";
        assert!(reason(apply(base, &delta(5, 1, &[1, b'x']), 10)).contains("base size"));
        assert!(reason(apply(base, &delta(6, 1, &[0]), 10)).contains("reserved opcode"));
        assert!(reason(apply(base, &delta(6, 4, &[0x91, 4, 4]), 10)).contains("out of base"));
        assert!(reason(apply(base, &delta(6, 2, &[3, b'x']), 10)).contains("truncated insert"));
        assert!(reason(apply(base, &delta(6, 2, &[0x91, 0]), 10)).contains("truncated copy"));
        assert!(reason(apply(base, &delta(6, 1, &[2, b'x', b'y']), 10)).contains("exceeds"));
        assert!(reason(apply(base, &delta(6, 3, &[2, b'x', b'y']), 10)).contains("shorter"));
        assert!(reason(apply(base, &[0x80], 10)).contains("truncated size"));
        assert!(reason(apply(base, &[0xff; 11], 10)).contains("overflow"));
        let huge_copy = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert!(reason(apply(base, &delta(6, 1, &huge_copy), 10)).contains("out of base"));
        assert!(matches!(
            apply(base, &delta(6, 11, &[]), 10),
            Err(Error::PackLimitExceeded { .. })
        ));
        let d = delta(6, 4, &[0x91, 1, 2, 2, b'x', b'y']);
        assert_eq!(apply(base, &d, 10).unwrap(), b"bcxy");
        for len in 0..d.len() {
            assert!(apply(base, &d[..len], 10).is_err());
        }
    }
}
