//! Git index file writer.
//!
//! This module implements writing of the Git index file format (version 2).

use std::path::Path;

use crate::infra::hash::sha1;
use crate::objects::tree::FileMode;

use super::{Index, IndexEntry};

/// The magic signature at the start of an index file: "DIRC"
const INDEX_SIGNATURE: &[u8; 4] = b"DIRC";

/// Writes the index to bytes in Git index format.
///
/// # Arguments
///
/// * `index` - The index to write.
///
/// # Returns
///
/// The serialized index as a byte vector.
pub fn write(index: &Index) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Write header
    write_header(&mut buffer, index.version(), index.len() as u32);

    // Write entries
    for entry in index.entries() {
        write_entry(&mut buffer, entry);
    }

    // Calculate and append checksum
    let checksum = sha1(&buffer);
    buffer.extend_from_slice(&checksum);

    buffer
}

/// Writes the index header.
///
/// The header consists of:
/// - 4 bytes: signature ("DIRC")
/// - 4 bytes: version number (big-endian)
/// - 4 bytes: number of entries (big-endian)
fn write_header(buffer: &mut Vec<u8>, version: u32, entry_count: u32) {
    buffer.extend_from_slice(INDEX_SIGNATURE);
    buffer.extend_from_slice(&version.to_be_bytes());
    buffer.extend_from_slice(&entry_count.to_be_bytes());
}

/// Writes a single index entry.
///
/// Each entry has:
/// - Fixed fields (62 bytes for v2)
/// - Variable-length name (NUL-terminated)
/// - Padding to 8-byte boundary
fn write_entry(buffer: &mut Vec<u8>, entry: &IndexEntry) {
    let entry_start = buffer.len();

    // ctime (seconds and nanoseconds)
    buffer.extend_from_slice(&(entry.ctime() as u32).to_be_bytes());
    buffer.extend_from_slice(&0u32.to_be_bytes()); // ctime_nsec

    // mtime (seconds and nanoseconds)
    buffer.extend_from_slice(&(entry.mtime() as u32).to_be_bytes());
    buffer.extend_from_slice(&0u32.to_be_bytes()); // mtime_nsec

    // dev
    buffer.extend_from_slice(&entry.dev().to_be_bytes());

    // ino
    buffer.extend_from_slice(&entry.ino().to_be_bytes());

    // mode
    let mode = file_mode_to_u32(entry.mode());
    buffer.extend_from_slice(&mode.to_be_bytes());

    // uid
    buffer.extend_from_slice(&entry.uid().to_be_bytes());

    // gid
    buffer.extend_from_slice(&entry.gid().to_be_bytes());

    // file size
    buffer.extend_from_slice(&entry.size().to_be_bytes());

    // SHA-1
    buffer.extend_from_slice(entry.oid().as_bytes());

    // flags (name length in lower 12 bits, stage in bits 12-13)
    let path_bytes = path_to_unix_bytes(entry.path());
    let name_len = path_bytes.len().min(0xFFF) as u16;
    let stage = (entry.stage() as u16) << 12;
    let flags = name_len | stage;
    buffer.extend_from_slice(&flags.to_be_bytes());

    // name
    buffer.extend_from_slice(&path_bytes);

    // Padding to 8-byte boundary
    // Entry size is padded to a multiple of 8 bytes
    // There's at least 1 NUL byte after the name
    let entry_size = buffer.len() - entry_start;
    let padding = (8 - (entry_size % 8)) % 8;
    let padding = if padding == 0 { 8 } else { padding };
    buffer.extend(std::iter::repeat(0u8).take(padding));
}

/// Converts a FileMode to its u32 representation.
fn file_mode_to_u32(mode: FileMode) -> u32 {
    match mode {
        FileMode::Regular => 0o100644,
        FileMode::Executable => 0o100755,
        FileMode::Symlink => 0o120000,
        FileMode::Directory => 0o040000,
        FileMode::Submodule => 0o160000,
    }
}

/// Converts a path to Unix-style bytes (forward slashes).
fn path_to_unix_bytes(path: &Path) -> Vec<u8> {
    // Convert to Unix-style path with forward slashes
    let path_str = path.to_string_lossy();
    let unix_path = path_str.replace('\\', "/");
    unix_path.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::reader::parse;
    use crate::infra::hash::SHA1_SIZE;
    use crate::objects::Oid;
    use std::path::PathBuf;

    const SHA1_A: [u8; 20] = [
        0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60, 0x18,
        0x90, 0xaf, 0xd8, 0x07, 0x09,
    ];

    const SHA1_B: [u8; 20] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ];

    fn make_entry(path: &str) -> IndexEntry {
        IndexEntry::new(
            1700000000, // ctime
            1700000001, // mtime
            100,        // dev
            12345,      // ino
            FileMode::Regular,
            1000, // uid
            1000, // gid
            42,   // size
            Oid::from_bytes(SHA1_A),
            PathBuf::from(path),
            0, // stage
        )
    }

    fn make_entry_with_oid(path: &str, oid_bytes: [u8; 20]) -> IndexEntry {
        IndexEntry::new(
            1700000000, // ctime
            1700000001, // mtime
            100,        // dev
            12345,      // ino
            FileMode::Regular,
            1000, // uid
            1000, // gid
            42,   // size
            Oid::from_bytes(oid_bytes),
            PathBuf::from(path),
            0, // stage
        )
    }

    // IW-001: Write empty index
    #[test]
    fn test_write_empty_index() {
        let index = Index::new(2, vec![]);
        let data = write(&index);

        // Header (12 bytes) + checksum (20 bytes)
        assert_eq!(data.len(), 12 + 20);

        // Verify signature
        assert_eq!(&data[0..4], b"DIRC");

        // Verify version
        assert_eq!(u32::from_be_bytes(data[4..8].try_into().unwrap()), 2);

        // Verify entry count
        assert_eq!(u32::from_be_bytes(data[8..12].try_into().unwrap()), 0);
    }

    // IW-002: Write single entry index
    #[test]
    fn test_write_single_entry() {
        let entries = vec![make_entry("file.txt")];
        let index = Index::new(2, entries);
        let data = write(&index);

        // Verify entry count
        assert_eq!(u32::from_be_bytes(data[8..12].try_into().unwrap()), 1);

        // Verify we can parse it back
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.entries()[0].path().to_str().unwrap(), "file.txt");
    }

    // IW-003: Write multiple entries
    #[test]
    fn test_write_multiple_entries() {
        let entries = vec![
            make_entry("a.txt"),
            make_entry_with_oid("b.txt", SHA1_B),
            make_entry("dir/c.txt"),
        ];
        let index = Index::new(2, entries);
        let data = write(&index);

        // Verify we can parse it back
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed.entries()[0].path().to_str().unwrap(), "a.txt");
        assert_eq!(parsed.entries()[1].path().to_str().unwrap(), "b.txt");
        assert_eq!(parsed.entries()[2].path().to_str().unwrap(), "dir/c.txt");
    }

    // IW-004: Roundtrip test - write and read back
    #[test]
    fn test_roundtrip() {
        let entries = vec![
            make_entry("file.txt"),
            make_entry_with_oid("nested/deep/file.rs", SHA1_B),
        ];
        let original = Index::new(2, entries);
        let data = write(&original);
        let parsed = parse(&data).unwrap();

        assert_eq!(original.version(), parsed.version());
        assert_eq!(original.len(), parsed.len());

        for (orig, pars) in original.entries().iter().zip(parsed.entries().iter()) {
            assert_eq!(orig.path(), pars.path());
            assert_eq!(orig.oid(), pars.oid());
            assert_eq!(orig.mode(), pars.mode());
            assert_eq!(orig.size(), pars.size());
            assert_eq!(orig.stage(), pars.stage());
        }
    }

    // IW-005: Checksum is correct
    #[test]
    fn test_checksum() {
        let index = Index::new(2, vec![make_entry("test.txt")]);
        let data = write(&index);

        // Extract checksum from last 20 bytes
        let stored_checksum = &data[data.len() - SHA1_SIZE..];

        // Calculate checksum of everything before it
        let calculated_checksum = sha1(&data[..data.len() - SHA1_SIZE]);

        assert_eq!(stored_checksum, &calculated_checksum);
    }

    // IW-006: File mode conversion
    #[test]
    fn test_file_mode_to_u32() {
        assert_eq!(file_mode_to_u32(FileMode::Regular), 0o100644);
        assert_eq!(file_mode_to_u32(FileMode::Executable), 0o100755);
        assert_eq!(file_mode_to_u32(FileMode::Symlink), 0o120000);
        assert_eq!(file_mode_to_u32(FileMode::Directory), 0o040000);
        assert_eq!(file_mode_to_u32(FileMode::Submodule), 0o160000);
    }

    // IW-007: Entry with executable mode
    #[test]
    fn test_write_executable_entry() {
        let entry = IndexEntry::new(
            1700000000,
            1700000001,
            100,
            12345,
            FileMode::Executable,
            1000,
            1000,
            42,
            Oid::from_bytes(SHA1_A),
            PathBuf::from("script.sh"),
            0,
        );
        let index = Index::new(2, vec![entry]);
        let data = write(&index);

        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.entries()[0].mode(), FileMode::Executable);
    }

    // IW-008: Entry with stage number (merge conflict)
    #[test]
    fn test_write_staged_entry() {
        let entry = IndexEntry::new(
            1700000000,
            1700000001,
            100,
            12345,
            FileMode::Regular,
            1000,
            1000,
            42,
            Oid::from_bytes(SHA1_A),
            PathBuf::from("conflict.txt"),
            2, // Stage 2 = "ours" in merge conflict
        );
        let index = Index::new(2, vec![entry]);
        let data = write(&index);

        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.entries()[0].stage(), 2);
        assert!(parsed.entries()[0].is_conflicted());
    }

    // IW-009: Path with Windows separators
    #[test]
    fn test_path_to_unix_bytes() {
        let path = Path::new("dir\\subdir\\file.txt");
        let bytes = path_to_unix_bytes(path);
        assert_eq!(bytes, b"dir/subdir/file.txt");
    }

    // IW-010: Version 3 index
    #[test]
    fn test_write_v3_index() {
        let entries = vec![make_entry("file.txt")];
        let index = Index::new(3, entries);
        let data = write(&index);

        // Verify version
        assert_eq!(u32::from_be_bytes(data[4..8].try_into().unwrap()), 3);

        // Verify we can parse it back
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.version(), 3);
    }

    // IW-011: Padding alignment
    #[test]
    fn test_entry_padding() {
        // Short name should have more padding
        let short_entry = make_entry("a.txt");
        let short_index = Index::new(2, vec![short_entry]);
        let short_data = write(&short_index);

        // Long name should have less padding
        let long_entry = make_entry("very_long_filename_that_needs_less_padding.txt");
        let long_index = Index::new(2, vec![long_entry]);
        let long_data = write(&long_index);

        // Both should be parseable
        assert!(parse(&short_data).is_ok());
        assert!(parse(&long_data).is_ok());
    }
}
