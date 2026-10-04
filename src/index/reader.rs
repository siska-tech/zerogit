//! Git index file parser.
//!
//! This module implements parsing of the Git index file format (versions 2, 3, 4).
//!
//! Entries are validated strictly: padding, path terminators, v4 path
//! compression, extended flags and the trailing checksum. Optional extensions
//! (cache tree, resolve-undo, untracked cache, ...) are skipped as Git allows;
//! required extensions such as split index (`link`) and sparse index (`sdir`)
//! are rejected with [`Error::UnsupportedIndex`], because ignoring them would
//! misread the entries and rewriting would corrupt the index.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::hash::sha1;
use crate::objects::oid::OID_BYTES;
use crate::objects::tree::FileMode;
use crate::objects::Oid;

use super::cache_tree::CacheTree;
use super::resolve_undo::ResolveUndo;
use super::{Index, IndexEntry};

/// The magic signature at the start of an index file: "DIRC"
const INDEX_SIGNATURE: &[u8; 4] = b"DIRC";

/// Minimum supported index version.
const MIN_VERSION: u32 = 2;

/// Maximum supported index version.
const MAX_VERSION: u32 = 4;

const HEADER_SIZE: usize = 12;
const CHECKSUM_SIZE: usize = 20;
/// ctime, mtime, dev, ino, mode, uid, gid, size (10 × 4 bytes), OID, flags.
const ENTRY_FIXED_SIZE: usize = 40 + OID_BYTES + 2;

const FLAG_EXTENDED: u16 = 0x4000;
const NAME_MASK: u16 = 0x0FFF;
const EXTENDED_SKIP_WORKTREE: u16 = 0x4000;
const EXTENDED_INTENT_TO_ADD: u16 = 0x2000;

/// Parses a Git index file from raw bytes.
///
/// # Errors
///
/// Returns `Error::InvalidIndex` if the signature, version, checksum or any
/// entry is malformed, and `Error::UnsupportedIndex` for features that
/// cannot be handled safely (split index, sparse index).
pub fn parse(data: &[u8]) -> Result<Index> {
    let (version, entry_count) = parse_header(data)?;
    let invalid = |reason: &str| Error::InvalidIndex {
        version,
        reason: reason.to_owned(),
    };
    if data.len() < HEADER_SIZE + CHECKSUM_SIZE {
        return Err(invalid("truncated index"));
    }
    let body = &data[..data.len() - CHECKSUM_SIZE];
    let checksum = &data[data.len() - CHECKSUM_SIZE..];
    // index.skipHash (Git 2.40+) writes an all-zero checksum.
    if checksum != [0u8; CHECKSUM_SIZE] && sha1(body) != checksum {
        return Err(invalid("checksum mismatch"));
    }

    let mut reader = Reader {
        data: body,
        pos: HEADER_SIZE,
        version,
    };
    // Each entry is at least ENTRY_FIXED_SIZE + 1 bytes; reject absurd counts
    // before allocating.
    if entry_count as usize > body.len() / (ENTRY_FIXED_SIZE + 1) {
        return Err(invalid("entry count exceeds index size"));
    }
    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut previous_name: Vec<u8> = Vec::new();
    for _ in 0..entry_count {
        let entry = reader.entry(&mut previous_name)?;
        entries.push(entry);
    }
    // Checked first: a split index legitimately has nameless entries that
    // refer to its shared index, so it must be reported as unsupported.
    let (cache_tree, resolve_undo) = reader.extensions()?;
    if entries.iter().any(|e| e.path().as_os_str().is_empty()) {
        return Err(invalid("entry with an empty name"));
    }

    Ok(Index::new(version, entries).with_extensions(cache_tree, resolve_undo))
}

/// Parses the index header and returns `(version, entry count)`.
fn parse_header(data: &[u8]) -> Result<(u32, u32)> {
    let header_error = |reason: &str| Error::InvalidIndex {
        version: 0,
        reason: reason.to_owned(),
    };
    let signature = data
        .get(..4)
        .ok_or_else(|| header_error("failed to read signature"))?;
    if signature != INDEX_SIGNATURE {
        return Err(header_error(&format!(
            "invalid signature: expected DIRC, got {:?}",
            String::from_utf8_lossy(signature)
        )));
    }
    let version = data
        .get(4..8)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| header_error("failed to read version"))?;
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(Error::InvalidIndex {
            version,
            reason: format!("unsupported version: {} (supported: 2-4)", version),
        });
    }
    let entry_count = data
        .get(8..12)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error::InvalidIndex {
            version,
            reason: "failed to read entry count".to_owned(),
        })?;
    Ok((version, entry_count))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    version: u32,
}

impl<'a> Reader<'a> {
    fn error(&self, reason: impl Into<String>) -> Error {
        Error::InvalidIndex {
            version: self.version,
            reason: reason.into(),
        }
    }

    fn bytes(&mut self, len: usize, what: &str) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| self.error(format!("failed to read entry field: {}", what)))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn u32(&mut self, what: &str) -> Result<u32> {
        let b = self.bytes(4, what)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u16(&mut self, what: &str) -> Result<u16> {
        let b = self.bytes(2, what)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Reads bytes up to (and consumes) the next NUL.
    fn until_nul(&mut self, what: &str) -> Result<&'a [u8]> {
        let rest = &self.data[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.error(format!("unterminated {}", what)))?;
        self.pos += len + 1;
        Ok(&rest[..len])
    }

    /// Decodes Git's offset varint used by v4 path compression.
    fn varint(&mut self) -> Result<usize> {
        let mut byte = self.bytes(1, "path prefix length")?[0];
        let mut value = u64::from(byte & 0x7f);
        while byte & 0x80 != 0 {
            byte = self.bytes(1, "path prefix length")?[0];
            value = value
                .checked_add(1)
                .and_then(|v| v.checked_mul(128))
                .map(|v| v | u64::from(byte & 0x7f))
                .ok_or_else(|| self.error("path prefix length overflow"))?;
        }
        usize::try_from(value).map_err(|_| self.error("path prefix length overflow"))
    }

    fn entry(&mut self, previous_name: &mut Vec<u8>) -> Result<IndexEntry> {
        let start = self.pos;
        let ctime_sec = self.u32("ctime_sec")?;
        let ctime_nsec = self.u32("ctime_nsec")?;
        let mtime_sec = self.u32("mtime_sec")?;
        let mtime_nsec = self.u32("mtime_nsec")?;
        let dev = self.u32("dev")?;
        let ino = self.u32("ino")?;
        let mode_raw = self.u32("mode")?;
        let uid = self.u32("uid")?;
        let gid = self.u32("gid")?;
        let size = self.u32("size")?;
        let mut oid_bytes = [0u8; OID_BYTES];
        oid_bytes.copy_from_slice(self.bytes(OID_BYTES, "oid")?);
        let flags = self.u16("flags")?;

        let mut fixed = ENTRY_FIXED_SIZE;
        let (mut skip_worktree, mut intent_to_add) = (false, false);
        if flags & FLAG_EXTENDED != 0 {
            if self.version < 3 {
                return Err(self.error("extended flags in a version 2 index"));
            }
            let extended = self.u16("extended_flags")?;
            if extended & !(EXTENDED_SKIP_WORKTREE | EXTENDED_INTENT_TO_ADD) != 0 {
                return Err(self.error(format!("unknown extended flags: {:#06x}", extended)));
            }
            skip_worktree = extended & EXTENDED_SKIP_WORKTREE != 0;
            intent_to_add = extended & EXTENDED_INTENT_TO_ADD != 0;
            fixed += 2;
        }
        let mode = parse_mode(mode_raw, self.version)?;
        let name_len = usize::from(flags & NAME_MASK);
        let stage = ((flags >> 12) & 0x03) as u8;

        let name: Vec<u8> = if self.version >= 4 {
            // v4: drop N bytes from the previous path, append a NUL-terminated suffix.
            let strip = self.varint()?;
            let keep = previous_name
                .len()
                .checked_sub(strip)
                .ok_or_else(|| self.error("path prefix longer than previous path"))?;
            let suffix = self.until_nul("entry name")?;
            let mut name = previous_name[..keep].to_vec();
            name.extend_from_slice(suffix);
            name
        } else {
            let name = if name_len < usize::from(NAME_MASK) {
                let name = self.bytes(name_len, "name")?;
                if self.bytes(1, "name terminator")? != [0] {
                    return Err(self.error("entry name is not NUL-terminated"));
                }
                name
            } else {
                // 0xFFF means the name is at least that long.
                self.until_nul("entry name")?
            };
            // v2/v3 entries are NUL-padded to a multiple of 8 bytes (1..=8 NULs).
            let padded = (fixed + name.len() + 8) & !7;
            let padding = self.bytes(start + padded - self.pos, "padding")?;
            if padding.iter().any(|&b| b != 0) {
                return Err(self.error("non-zero entry padding"));
            }
            name.to_vec()
        };
        if name_len < usize::from(NAME_MASK) && name.len() != name_len {
            return Err(self.error("entry name length does not match its flags"));
        }
        let path = String::from_utf8(name.clone())
            .map_err(|_| self.error("invalid UTF-8 in entry name"))?;
        *previous_name = name;

        Ok(IndexEntry::new(
            u64::from(ctime_sec),
            u64::from(mtime_sec),
            dev,
            ino,
            mode,
            uid,
            gid,
            size,
            Oid::from_bytes(oid_bytes),
            PathBuf::from(path),
            stage,
        )
        .with_nanos(ctime_nsec, mtime_nsec)
        .with_extended_flags(skip_worktree, intent_to_add))
    }

    /// Validates the extensions after the entries.
    ///
    /// Git requires readers to understand extensions whose signature does
    /// not start with an uppercase letter; the others are optional caches.
    /// Reads the extensions, keeping the cache tree (`TREE`) and the
    /// resolve-undo data (`REUC`). Other optional extensions (such as the
    /// untracked cache `UNTR` or the fsmonitor data `FSMN`) are dropped: they
    /// describe the work tree or the file layout, which zerogit cannot keep
    /// up to date, and Git rebuilds them. A malformed `TREE` or `REUC` is
    /// dropped the same way.
    fn extensions(&mut self) -> Result<(Option<CacheTree>, ResolveUndo)> {
        let mut cache_tree = None;
        let mut resolve_undo = ResolveUndo::new();
        while self.pos < self.data.len() {
            let signature = self.bytes(4, "extension signature")?;
            let size = self.u32("extension size")? as usize;
            if !signature[0].is_ascii_uppercase() {
                let name = String::from_utf8_lossy(signature);
                let feature = match signature {
                    b"link" => "split index",
                    b"sdir" => "sparse index",
                    _ => "unknown required extension",
                };
                return Err(Error::UnsupportedIndex {
                    version: self.version,
                    reason: format!("{} (extension '{}')", feature, name),
                });
            }
            let data = self.bytes(size, "extension data")?;
            match signature {
                b"TREE" => cache_tree = CacheTree::parse(data),
                b"REUC" => {
                    resolve_undo = super::resolve_undo::parse(data)
                        .map(|records| records.into_iter().collect())
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
        Ok((cache_tree, resolve_undo))
    }
}

/// Parses a mode value into a FileMode.
fn parse_mode(mode: u32, version: u32) -> Result<FileMode> {
    match mode {
        0o100644 => Ok(FileMode::Regular),
        0o100755 => Ok(FileMode::Executable),
        0o120000 => Ok(FileMode::Symlink),
        0o160000 => Ok(FileMode::Submodule),
        // Sparse directory entries only appear in a sparse index.
        0o040000 => Err(Error::UnsupportedIndex {
            version,
            reason: "sparse directory entry (sparse index)".to_owned(),
        }),
        // Regular files can have different mode bits in some edge cases
        m if (m & 0o170000) == 0o100000 => Ok(FileMode::Regular),
        _ => Err(Error::InvalidIndex {
            version,
            reason: format!("unknown file mode: {:o}", mode),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a minimal valid v2/v3 index with the given entries.
    fn make_index(version: u32, entries: &[(&str, &[u8; 20])]) -> Vec<u8> {
        let mut data = Vec::new();

        // Header
        data.extend_from_slice(INDEX_SIGNATURE);
        data.extend_from_slice(&version.to_be_bytes());
        data.extend_from_slice(&(entries.len() as u32).to_be_bytes());

        // Entries
        for (name, sha1) in entries {
            let entry_start = data.len();

            // ctime_sec, ctime_nsec
            data.extend_from_slice(&1700000000u32.to_be_bytes());
            data.extend_from_slice(&0u32.to_be_bytes());
            // mtime_sec, mtime_nsec
            data.extend_from_slice(&1700000001u32.to_be_bytes());
            data.extend_from_slice(&0u32.to_be_bytes());
            // dev
            data.extend_from_slice(&100u32.to_be_bytes());
            // ino
            data.extend_from_slice(&12345u32.to_be_bytes());
            // mode (100644 = regular file)
            data.extend_from_slice(&0o100644u32.to_be_bytes());
            // uid
            data.extend_from_slice(&1000u32.to_be_bytes());
            // gid
            data.extend_from_slice(&1000u32.to_be_bytes());
            // size
            data.extend_from_slice(&42u32.to_be_bytes());
            // SHA-1
            data.extend_from_slice(*sha1);
            // flags (name length in lower 12 bits)
            let name_len = name.len().min(0xFFF) as u16;
            data.extend_from_slice(&name_len.to_be_bytes());
            // name
            data.extend_from_slice(name.as_bytes());

            // Padding to 8-byte boundary
            let entry_size = data.len() - entry_start;
            let padding = (8 - (entry_size % 8)) % 8;
            // At least 1 NUL byte is required
            let padding = if padding == 0 { 8 } else { padding };
            data.extend(std::iter::repeat(0u8).take(padding));
        }

        // An all-zero checksum, as written with index.skipHash.
        data.extend_from_slice(&[0u8; 20]);

        data
    }

    /// Replaces the trailing checksum with the real SHA-1 of the body.
    fn seal(mut data: Vec<u8>) -> Vec<u8> {
        let body_len = data.len() - CHECKSUM_SIZE;
        let checksum = sha1(&data[..body_len]);
        data[body_len..].copy_from_slice(&checksum);
        data
    }

    /// Inserts an extension (declaring `size` bytes) between the entries and the checksum.
    fn extension_with_size(
        data: Vec<u8>,
        signature: &[u8; 4],
        size: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut out = data[..data.len() - CHECKSUM_SIZE].to_vec();
        out.extend_from_slice(signature);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&[0u8; CHECKSUM_SIZE]);
        seal(out)
    }

    fn with_extension(data: Vec<u8>, signature: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        extension_with_size(data, signature, payload.len() as u32, payload)
    }

    const SHA1_A: [u8; 20] = [
        0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60, 0x18,
        0x90, 0xaf, 0xd8, 0x07, 0x09,
    ];

    const SHA1_B: [u8; 20] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ];

    // I-001: Parse v2 index
    #[test]
    fn test_parse_v2_index() {
        let data = make_index(2, &[("file.txt", &SHA1_A)]);
        let index = parse(&data).unwrap();

        assert_eq!(index.version(), 2);
        assert_eq!(index.len(), 1);

        let entry = &index.entries()[0];
        assert_eq!(entry.path().to_str().unwrap(), "file.txt");
        assert_eq!(entry.oid(), &Oid::from_bytes(SHA1_A));
        assert_eq!(entry.mode(), FileMode::Regular);
    }

    // I-002: Get entry by path
    #[test]
    fn test_get_entry_by_path() {
        let data = make_index(2, &[("file.txt", &SHA1_A), ("dir/nested.txt", &SHA1_B)]);
        let index = parse(&data).unwrap();

        let entry = index.get(std::path::Path::new("file.txt")).unwrap();
        assert_eq!(entry.path().to_str().unwrap(), "file.txt");

        let entry = index.get(std::path::Path::new("dir/nested.txt")).unwrap();
        assert_eq!(entry.path().to_str().unwrap(), "dir/nested.txt");
    }

    // I-003: Get non-existent entry returns None
    #[test]
    fn test_get_nonexistent_entry() {
        let data = make_index(2, &[("file.txt", &SHA1_A)]);
        let index = parse(&data).unwrap();

        assert!(index.get(std::path::Path::new("nonexistent")).is_none());
    }

    // I-004: Invalid signature
    #[test]
    fn test_invalid_signature() {
        let mut data = make_index(2, &[]);
        data[0..4].copy_from_slice(b"XXXX");

        let result = parse(&data);
        assert!(matches!(
            result,
            Err(Error::InvalidIndex { version: 0, .. })
        ));
    }

    // I-005: Unsupported version
    #[test]
    fn test_unsupported_version() {
        // Version 5 is not supported
        let mut data = make_index(2, &[]);
        data[4..8].copy_from_slice(&5u32.to_be_bytes());

        let result = parse(&data);
        assert!(matches!(
            result,
            Err(Error::InvalidIndex { version: 5, .. })
        ));

        // Version 1 is not supported
        data[4..8].copy_from_slice(&1u32.to_be_bytes());
        let result = parse(&data);
        assert!(matches!(
            result,
            Err(Error::InvalidIndex { version: 1, .. })
        ));
    }

    // Test parsing header only
    #[test]
    fn test_parse_header() {
        let data = make_index(3, &[("test.txt", &SHA1_A)]);
        let (version, entry_count) = parse_header(&data).unwrap();
        assert_eq!(version, 3);
        assert_eq!(entry_count, 1);
    }

    // Test multiple entries
    #[test]
    fn test_multiple_entries() {
        let data = make_index(
            2,
            &[("a.txt", &SHA1_A), ("b.txt", &SHA1_B), ("c/d.txt", &SHA1_A)],
        );
        let index = parse(&data).unwrap();

        assert_eq!(index.len(), 3);
        assert_eq!(index.entries()[0].path().to_str().unwrap(), "a.txt");
        assert_eq!(index.entries()[1].path().to_str().unwrap(), "b.txt");
        assert_eq!(index.entries()[2].path().to_str().unwrap(), "c/d.txt");
    }

    // Test empty index
    #[test]
    fn test_empty_index() {
        let data = make_index(2, &[]);
        let index = parse(&data).unwrap();

        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    // Test entry metadata
    #[test]
    fn test_entry_metadata() {
        let data = make_index(2, &[("file.txt", &SHA1_A)]);
        let index = parse(&data).unwrap();

        let entry = &index.entries()[0];
        assert_eq!(entry.ctime(), 1700000000);
        assert_eq!(entry.mtime(), 1700000001);
        assert_eq!(entry.dev(), 100);
        assert_eq!(entry.ino(), 12345);
        assert_eq!(entry.uid(), 1000);
        assert_eq!(entry.gid(), 1000);
        assert_eq!(entry.size(), 42);
        assert_eq!(entry.stage(), 0);
        assert!(!entry.is_conflicted());
    }

    // Test truncated data
    #[test]
    fn test_truncated_header() {
        // Just "DIR" without the C
        let data = b"DIR";
        let result = parse(data);
        assert!(result.is_err());
    }

    // Test version 3 support
    #[test]
    fn test_v3_index() {
        let data = make_index(3, &[("v3file.txt", &SHA1_A)]);
        let index = parse(&data).unwrap();

        assert_eq!(index.version(), 3);
        assert_eq!(index.len(), 1);
    }

    // A v2-layout entry under a v4 header must not parse as a valid path.
    #[test]
    fn test_v2_layout_with_v4_header_is_rejected() {
        let mut data = make_index(2, &[("v4file.txt", &SHA1_A), ("v4other.txt", &SHA1_B)]);
        data[4..8].copy_from_slice(&4u32.to_be_bytes());
        assert!(matches!(parse(&data), Err(Error::InvalidIndex { .. })));
        let mut single = make_index(2, &[("README.md", &SHA1_A)]);
        single[4..8].copy_from_slice(&4u32.to_be_bytes());
        assert!(matches!(parse(&single), Err(Error::InvalidIndex { .. })));
    }

    #[test]
    fn test_checksum_is_verified() {
        let sealed = seal(make_index(2, &[("file.txt", &SHA1_A)]));
        assert!(parse(&sealed).is_ok());
        let mut corrupt = sealed.clone();
        corrupt[HEADER_SIZE + 3] ^= 1;
        assert!(matches!(parse(&corrupt), Err(Error::InvalidIndex { .. })));
    }

    #[test]
    fn test_extensions() {
        let base = make_index(2, &[("file.txt", &SHA1_A)]);
        // Optional extensions (uppercase signature) are skipped.
        let tree = with_extension(base.clone(), b"TREE", b"\0-1 0\n");
        assert_eq!(parse(&tree).unwrap().len(), 1);
        // Required extensions are rejected explicitly.
        for (signature, feature) in [(b"link", "split index"), (b"sdir", "sparse index")] {
            match parse(&with_extension(base.clone(), signature, &[0; 20])) {
                Err(Error::UnsupportedIndex { reason, .. }) => assert!(reason.contains(feature)),
                other => panic!("expected UnsupportedIndex, got {:?}", other),
            }
        }
        // An extension declaring more data than present is invalid.
        let truncated = extension_with_size(base, b"TREE", 9, b"abc");
        assert!(matches!(parse(&truncated), Err(Error::InvalidIndex { .. })));
    }

    #[test]
    fn test_padding_and_terminators_are_validated() {
        let data = make_index(2, &[("file.txt", &SHA1_A)]);
        let name_end = HEADER_SIZE + ENTRY_FIXED_SIZE + "file.txt".len();
        let mut bad_padding = data.clone();
        bad_padding[name_end + 1] = b'x';
        assert!(matches!(
            parse(&bad_padding),
            Err(Error::InvalidIndex { .. })
        ));
        let mut bad_terminator = data;
        bad_terminator[name_end] = b'x';
        assert!(matches!(
            parse(&bad_terminator),
            Err(Error::InvalidIndex { .. })
        ));
    }

    #[test]
    fn test_truncation_never_panics() {
        let data = seal(make_index(3, &[("a.txt", &SHA1_A), ("dir/b.txt", &SHA1_B)]));
        for len in 0..data.len() {
            assert!(parse(&data[..len]).is_err(), "length {}", len);
        }
    }
}
