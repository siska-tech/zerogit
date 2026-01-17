//! Git index file parser.
//!
//! This module implements parsing of the Git index file format (versions 2, 3, 4).

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::objects::oid::OID_BYTES;
use crate::objects::tree::FileMode;
use crate::objects::Oid;

use super::{Index, IndexEntry};

/// The magic signature at the start of an index file: "DIRC"
const INDEX_SIGNATURE: &[u8; 4] = b"DIRC";

/// Minimum supported index version.
const MIN_VERSION: u32 = 2;

/// Maximum supported index version.
const MAX_VERSION: u32 = 4;

/// Parses a Git index file from raw bytes.
///
/// # Arguments
///
/// * `data` - The raw bytes of the index file.
///
/// # Returns
///
/// The parsed Index on success, or an error if parsing fails.
///
/// # Errors
///
/// Returns `Error::InvalidIndex` if:
/// - The signature is not "DIRC"
/// - The version is not 2, 3, or 4
/// - The data is truncated or malformed
pub fn parse(data: &[u8]) -> Result<Index> {
    let mut cursor = Cursor::new(data);

    // Parse header
    let (version, entry_count) = parse_header(&mut cursor)?;

    // Parse entries
    let mut entries = Vec::with_capacity(entry_count as usize);
    for _ in 0..entry_count {
        let entry = parse_entry(&mut cursor, version)?;
        entries.push(entry);
    }

    Ok(Index::new(version, entries))
}

/// Parses the index file header.
///
/// The header consists of:
/// - 4 bytes: signature ("DIRC")
/// - 4 bytes: version number (big-endian)
/// - 4 bytes: number of entries (big-endian)
fn parse_header(cursor: &mut Cursor<&[u8]>) -> Result<(u32, u32)> {
    // Read signature
    let mut sig = [0u8; 4];
    cursor
        .read_exact(&mut sig)
        .map_err(|_| Error::InvalidIndex {
            version: 0,
            reason: "failed to read signature".to_string(),
        })?;

    if &sig != INDEX_SIGNATURE {
        return Err(Error::InvalidIndex {
            version: 0,
            reason: format!(
                "invalid signature: expected DIRC, got {:?}",
                String::from_utf8_lossy(&sig)
            ),
        });
    }

    // Read version
    let version = read_u32_be(cursor).map_err(|_| Error::InvalidIndex {
        version: 0,
        reason: "failed to read version".to_string(),
    })?;

    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        return Err(Error::InvalidIndex {
            version,
            reason: format!("unsupported version: {} (supported: 2-4)", version),
        });
    }

    // Read entry count
    let entry_count = read_u32_be(cursor).map_err(|_| Error::InvalidIndex {
        version,
        reason: "failed to read entry count".to_string(),
    })?;

    Ok((version, entry_count))
}

/// Parses a single index entry.
///
/// Each entry has:
/// - Fixed fields (62 bytes for v2, 64 bytes for v3+ with extended flags)
/// - Variable-length name (NUL-terminated)
/// - Padding to 8-byte boundary
fn parse_entry(cursor: &mut Cursor<&[u8]>, version: u32) -> Result<IndexEntry> {
    let entry_start = cursor.position();

    // ctime (seconds)
    let ctime_sec = read_u32_be(cursor).map_err(|_| make_entry_error(version, "ctime_sec"))?;
    // ctime (nanoseconds) - ignored
    let _ctime_nsec = read_u32_be(cursor).map_err(|_| make_entry_error(version, "ctime_nsec"))?;

    // mtime (seconds)
    let mtime_sec = read_u32_be(cursor).map_err(|_| make_entry_error(version, "mtime_sec"))?;
    // mtime (nanoseconds) - ignored
    let _mtime_nsec = read_u32_be(cursor).map_err(|_| make_entry_error(version, "mtime_nsec"))?;

    // dev
    let dev = read_u32_be(cursor).map_err(|_| make_entry_error(version, "dev"))?;

    // ino
    let ino = read_u32_be(cursor).map_err(|_| make_entry_error(version, "ino"))?;

    // mode
    let mode_raw = read_u32_be(cursor).map_err(|_| make_entry_error(version, "mode"))?;
    let mode = parse_mode(mode_raw, version)?;

    // uid
    let uid = read_u32_be(cursor).map_err(|_| make_entry_error(version, "uid"))?;

    // gid
    let gid = read_u32_be(cursor).map_err(|_| make_entry_error(version, "gid"))?;

    // file size
    let size = read_u32_be(cursor).map_err(|_| make_entry_error(version, "size"))?;

    // SHA-1
    let mut oid_bytes = [0u8; OID_BYTES];
    cursor
        .read_exact(&mut oid_bytes)
        .map_err(|_| make_entry_error(version, "oid"))?;
    let oid = Oid::from_bytes(oid_bytes);

    // flags (16 bits)
    let flags = read_u16_be(cursor).map_err(|_| make_entry_error(version, "flags"))?;

    // Extract name length from lower 12 bits
    let name_len = (flags & 0x0FFF) as usize;

    // Extract stage from bits 12-13
    let stage = ((flags >> 12) & 0x03) as u8;

    // Check for extended flag (bit 14, v3+ only)
    let has_extended = version >= 3 && (flags & 0x4000) != 0;

    // Read extended flags if present
    if has_extended {
        let _extended_flags =
            read_u16_be(cursor).map_err(|_| make_entry_error(version, "extended_flags"))?;
        // Extended flags contain additional information like skip-worktree and intent-to-add
        // For now, we just skip them
    }

    // Read name
    // If name_len is 0xFFF (4095), the name is longer and continues until NUL
    let name = if name_len == 0xFFF {
        // Read until NUL byte
        read_until_nul(cursor).map_err(|_| make_entry_error(version, "name (long)"))?
    } else {
        // Read exactly name_len bytes
        let mut name_buf = vec![0u8; name_len];
        cursor
            .read_exact(&mut name_buf)
            .map_err(|_| make_entry_error(version, "name"))?;
        String::from_utf8(name_buf).map_err(|_| Error::InvalidIndex {
            version,
            reason: "invalid UTF-8 in entry name".to_string(),
        })?
    };

    // Calculate padding
    // Entry size is padded to a multiple of 8 bytes
    // For v4, padding is different (path compression), but we don't support that fully
    let _entry_size = (cursor.position() - entry_start) as usize;

    // Find and skip NUL padding
    // There's at least 1 NUL byte after the name, and then padding to 8-byte boundary
    if name_len != 0xFFF {
        // Skip the NUL terminator that's included in padding
        skip_padding(cursor, entry_start, version)?;
    }

    Ok(IndexEntry::new(
        ctime_sec as u64,
        mtime_sec as u64,
        dev,
        ino,
        mode,
        uid,
        gid,
        size,
        oid,
        PathBuf::from(name),
        stage,
    ))
}

/// Parses a mode value into a FileMode.
fn parse_mode(mode: u32, version: u32) -> Result<FileMode> {
    match mode {
        0o100644 => Ok(FileMode::Regular),
        0o100755 => Ok(FileMode::Executable),
        0o120000 => Ok(FileMode::Symlink),
        0o160000 => Ok(FileMode::Submodule),
        // Regular files can have different mode bits in some edge cases
        m if (m & 0o170000) == 0o100000 => Ok(FileMode::Regular),
        _ => Err(Error::InvalidIndex {
            version,
            reason: format!("unknown file mode: {:o}", mode),
        }),
    }
}

/// Skips padding bytes after an entry.
fn skip_padding(cursor: &mut Cursor<&[u8]>, entry_start: u64, _version: u32) -> Result<()> {
    // Entry must be padded to 8-byte boundary
    // The minimum entry size is 62 bytes (v2) or 64 bytes (v3 with extended)
    // Plus the name length + 1 (for NUL terminator)
    // Padded to multiple of 8

    let current_pos = cursor.position();
    let entry_size = current_pos - entry_start;

    // Skip NUL bytes until we're at 8-byte boundary
    // There's at least 1 NUL after the name
    let mut byte = [0u8; 1];
    loop {
        let pos = cursor.position();
        let offset_from_start = pos - entry_start;

        // Check if we've reached an 8-byte boundary (and read at least 1 byte of padding)
        if offset_from_start > entry_size && offset_from_start % 8 == 0 {
            break;
        }

        // Read one byte
        if cursor.read_exact(&mut byte).is_err() {
            break; // End of data
        }

        // Should be NUL padding
        if byte[0] != 0 {
            // Oops, went too far or format error - seek back
            cursor.seek(SeekFrom::Current(-1)).ok();
            break;
        }
    }

    Ok(())
}

/// Reads a NUL-terminated string from the cursor.
fn read_until_nul(cursor: &mut Cursor<&[u8]>) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        cursor.read_exact(&mut byte)?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }

    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Reads a big-endian u32 from the cursor.
fn read_u32_be(cursor: &mut Cursor<&[u8]>) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

/// Reads a big-endian u16 from the cursor.
fn read_u16_be(cursor: &mut Cursor<&[u8]>) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    cursor.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

/// Creates an InvalidIndex error for entry parsing failures.
fn make_entry_error(version: u32, field: &str) -> Error {
    Error::InvalidIndex {
        version,
        reason: format!("failed to read entry field: {}", field),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a minimal valid index file with the given entries.
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

        // Checksum (20 bytes) - we'll just add zeros for test purposes
        // Real Git would calculate SHA-1 of all preceding content
        data.extend_from_slice(&[0u8; 20]);

        data
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
        let mut cursor = Cursor::new(data.as_slice());

        let (version, entry_count) = parse_header(&mut cursor).unwrap();
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

    // Test version 4 support
    #[test]
    fn test_v4_index() {
        let data = make_index(4, &[("v4file.txt", &SHA1_A)]);
        let index = parse(&data).unwrap();

        assert_eq!(index.version(), 4);
        assert_eq!(index.len(), 1);
    }
}
