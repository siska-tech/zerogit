//! Git delta instruction decoding.

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
