//! The resolve-undo (`REUC`) index extension: the conflict stages of paths
//! whose conflict was resolved in the index, so that `git checkout -m
//! <path>` can recreate the conflict.

use crate::objects::Oid;

/// The stages 1 to 3 (base, ours, theirs) a resolved path had: mode and
/// blob, or `None` for a missing stage.
pub(crate) type Stages = [Option<(u32, Oid)>; 3];

/// The resolved conflicts of an index, by `/`-separated path.
pub(crate) type ResolveUndo = std::collections::BTreeMap<Vec<u8>, Stages>;

/// Parses the extension data into `(path, stages)` records, in file order.
/// Returns `None` if it is malformed.
pub(crate) fn parse(data: &[u8]) -> Option<Vec<(Vec<u8>, Stages)>> {
    let mut records = Vec::new();
    let mut pos = 0;
    let field = |pos: &mut usize| -> Option<Vec<u8>> {
        let nul = data[*pos..].iter().position(|&b| b == 0)?;
        let value = data[*pos..*pos + nul].to_vec();
        *pos += nul + 1;
        Some(value)
    };
    while pos < data.len() {
        let path = field(&mut pos)?;
        let mut modes = [0u32; 3];
        for mode in &mut modes {
            *mode = u32::from_str_radix(std::str::from_utf8(&field(&mut pos)?).ok()?, 8).ok()?;
        }
        let mut stages: Stages = [None; 3];
        for (stage, mode) in stages.iter_mut().zip(modes) {
            if mode != 0 {
                let oid = data.get(pos..pos + 20)?;
                pos += 20;
                *stage = Some((mode, Oid::from_bytes(oid.try_into().ok()?)));
            }
        }
        records.push((path, stages));
    }
    Some(records)
}

/// Serializes `(path, stages)` records.
pub(crate) fn write<'a>(records: impl Iterator<Item = (&'a [u8], &'a Stages)>, out: &mut Vec<u8>) {
    for (path, stages) in records {
        out.extend_from_slice(path);
        out.push(0);
        for stage in stages {
            let mode = stage.map_or(0, |(mode, _)| mode);
            out.extend_from_slice(format!("{:o}", mode).as_bytes());
            out.push(0);
        }
        for (_, oid) in stages.iter().flatten() {
            out.extend_from_slice(oid.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let a: Stages = [
            Some((0o100644, Oid::from_bytes([1; 20]))),
            Some((0o100644, Oid::from_bytes([2; 20]))),
            None,
        ];
        let b: Stages = [None, Some((0o100755, Oid::from_bytes([3; 20]))), None];
        let mut data = Vec::new();
        write(
            [(&b"dir/a.txt"[..], &a), (&b"b.sh"[..], &b)].into_iter(),
            &mut data,
        );
        assert!(data.starts_with(b"dir/a.txt\x00100644\x00100644\x000\x00"));
        assert_eq!(
            parse(&data),
            Some(vec![(b"dir/a.txt".to_vec(), a), (b"b.sh".to_vec(), b)])
        );
        assert_eq!(parse(b"a\x00100644\x00"), None);
        assert_eq!(parse(b"a\x0010x644\x000\x000\x00"), None);
    }
}
