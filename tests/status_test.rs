//! Integration tests for status functionality.
//!
//! Test cases: RP-020 to RP-024

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zerogit::repository::Repository;
use zerogit::status::FileStatus;

/// Path to the simple test fixture
const SIMPLE_FIXTURE: &str = "tests/fixtures/simple";

/// Helper to create a minimal git repository for testing.
fn create_test_repo() -> TempDir {
    let temp = TempDir::new().unwrap();
    let path = temp.path();

    // Initialize .git structure
    let git_dir = path.join(".git");
    fs::create_dir_all(git_dir.join("objects")).unwrap();
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

    temp
}

// RP-020: Untracked files are detected
#[test]
fn test_rp020_untracked_files_detected() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create a file in the working tree (not tracked)
    fs::write(path.join("new_file.txt"), "content").unwrap();

    let repo = Repository::open(path).unwrap();
    let status = repo.status().unwrap();

    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path(), Path::new("new_file.txt"));
    assert_eq!(status[0].status(), FileStatus::Untracked);
}

// RP-021: Modified files are detected (using simple fixture)
#[test]
fn test_rp021_modified_files_detected() {
    // Copy the simple fixture to a temp directory to modify it
    let temp = TempDir::new().unwrap();
    let temp_path = temp.path();

    // Copy the fixture
    copy_dir_all(SIMPLE_FIXTURE, temp_path).unwrap();

    // Modify a tracked file
    fs::write(temp_path.join("README.md"), "Modified content\n").unwrap();

    let repo = Repository::open(temp_path).unwrap();
    let status = repo.status().unwrap();

    // Should have at least one modified file
    let modified_files: Vec<_> = status
        .iter()
        .filter(|e| e.status() == FileStatus::Modified)
        .collect();

    assert!(
        !modified_files.is_empty(),
        "Should detect modified file, got: {:?}",
        status
    );
}

// RP-022: Deleted files are detected
#[test]
fn test_rp022_deleted_files_detected() {
    // Copy the simple fixture to a temp directory
    let temp = TempDir::new().unwrap();
    let temp_path = temp.path();

    // Copy the fixture
    copy_dir_all(SIMPLE_FIXTURE, temp_path).unwrap();

    // Delete a tracked file
    fs::remove_file(temp_path.join("README.md")).unwrap();

    let repo = Repository::open(temp_path).unwrap();
    let status = repo.status().unwrap();

    // Should have deleted file
    let deleted_files: Vec<_> = status
        .iter()
        .filter(|e| e.status() == FileStatus::Deleted)
        .collect();

    assert!(
        !deleted_files.is_empty(),
        "Should detect deleted file, got: {:?}",
        status
    );
}

// RP-023: Multiple status types are correctly identified
#[test]
fn test_rp023_multiple_status_types() {
    // Copy the simple fixture to a temp directory
    let temp = TempDir::new().unwrap();
    let temp_path = temp.path();

    // Copy the fixture
    copy_dir_all(SIMPLE_FIXTURE, temp_path).unwrap();

    // Create an untracked file
    fs::write(temp_path.join("untracked.txt"), "untracked").unwrap();

    // Modify a tracked file
    fs::write(temp_path.join("README.md"), "Modified content\n").unwrap();

    let repo = Repository::open(temp_path).unwrap();
    let status = repo.status().unwrap();

    // Count different status types
    let untracked_count = status
        .iter()
        .filter(|e| e.status() == FileStatus::Untracked)
        .count();
    let modified_count = status
        .iter()
        .filter(|e| e.status() == FileStatus::Modified)
        .count();

    assert!(
        untracked_count >= 1,
        "Should have at least 1 untracked file"
    );
    assert!(modified_count >= 1, "Should have at least 1 modified file");
}

// RP-024: Clean working tree returns empty status
#[test]
fn test_rp024_clean_working_tree() {
    // Create a fresh repo with a commit, then verify it's clean
    let temp = create_test_repo();
    let path = temp.path();
    let git_dir = path.join(".git");

    // Create a blob for file content (with LF line ending for consistency)
    let content = b"Hello\n";
    let blob_oid = create_object(&git_dir.join("objects"), content, "blob");

    // Create a tree with this blob
    let tree_content = create_tree_content(&[("file.txt", "100644", &blob_oid)]);
    let tree_oid = create_object(&git_dir.join("objects"), &tree_content, "tree");

    // Create a commit pointing to this tree
    let commit_content = format!(
        "tree {}\nauthor Test <test@test.com> 1700000000 +0000\ncommitter Test <test@test.com> 1700000000 +0000\n\nInitial commit\n",
        tree_oid
    );
    let commit_oid = create_object(
        &git_dir.join("objects"),
        commit_content.as_bytes(),
        "commit",
    );

    // Set HEAD to point to this commit via main branch
    fs::write(git_dir.join("refs/heads/main"), format!("{}\n", commit_oid)).unwrap();

    // Create an index file with the same blob
    let index_data = create_index(&[("file.txt", &blob_oid)]);
    fs::write(git_dir.join("index"), &index_data).unwrap();

    // Create the working tree file with same content
    fs::write(path.join("file.txt"), content).unwrap();

    let repo = Repository::open(path).unwrap();
    let status = repo.status().unwrap();

    // Working tree should be clean
    assert!(
        status.is_empty(),
        "Clean repository should have empty status, got: {:?}",
        status
    );
}

/// Helper to create a loose object and return its hex OID.
fn create_object(objects_dir: &Path, content: &[u8], object_type: &str) -> String {
    use miniz_oxide::deflate::compress_to_vec_zlib;

    let header = format!("{} {}\0", object_type, content.len());
    let mut raw = header.into_bytes();
    raw.extend_from_slice(content);

    // Compute SHA-1
    let hash = sha1_hash(&raw);
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();

    let compressed = compress_to_vec_zlib(&raw, 6);
    let object_path = objects_dir.join(&hex[..2]).join(&hex[2..]);
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(&object_path, &compressed).unwrap();

    hex
}

/// Simple SHA-1 implementation for test helpers.
fn sha1_hash(data: &[u8]) -> [u8; 20] {
    use std::num::Wrapping;

    let mut h = [
        Wrapping(0x67452301u32),
        Wrapping(0xEFCDAB89u32),
        Wrapping(0x98BADCFEu32),
        Wrapping(0x10325476u32),
        Wrapping(0xC3D2E1F0u32),
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), Wrapping(0x5A827999u32)),
                20..=39 => (b ^ c ^ d, Wrapping(0x6ED9EBA1u32)),
                40..=59 => ((b & c) | (b & d) | (c & d), Wrapping(0x8F1BBCDCu32)),
                _ => (b ^ c ^ d, Wrapping(0xCA62C1D6u32)),
            };
            let temp = Wrapping(a.0.rotate_left(5)) + f + e + k + Wrapping(w[i]);
            e = d;
            d = c;
            c = Wrapping(b.0.rotate_left(30));
            b = a;
            a = temp;
        }

        h[0] += a;
        h[1] += b;
        h[2] += c;
        h[3] += d;
        h[4] += e;
    }

    let mut result = [0u8; 20];
    for (i, &Wrapping(val)) in h.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

/// Helper to create tree content.
fn create_tree_content(entries: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut content = Vec::new();
    for (name, mode, oid_hex) in entries {
        content.extend_from_slice(mode.as_bytes());
        content.push(b' ');
        content.extend_from_slice(name.as_bytes());
        content.push(0);
        // Convert hex OID to bytes
        for i in 0..20 {
            let byte = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
            content.push(byte);
        }
    }
    content
}

/// Helper to create a minimal index file.
fn create_index(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut data = Vec::new();

    // Header: DIRC, version 2, entry count
    data.extend_from_slice(b"DIRC");
    data.extend_from_slice(&2u32.to_be_bytes()); // version
    data.extend_from_slice(&(entries.len() as u32).to_be_bytes());

    for (name, oid_hex) in entries {
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
        data.extend_from_slice(&6u32.to_be_bytes()); // "Hello\n" = 6 bytes

        // SHA-1 (convert hex to bytes)
        for i in 0..20 {
            let byte = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
            data.push(byte);
        }

        // flags (name length in lower 12 bits)
        let name_len = name.len().min(0xFFF) as u16;
        data.extend_from_slice(&name_len.to_be_bytes());

        // name
        data.extend_from_slice(name.as_bytes());

        // Padding to 8-byte boundary
        let entry_size = data.len() - entry_start;
        let padding = (8 - (entry_size % 8)) % 8;
        let padding = if padding == 0 { 8 } else { padding };
        data.extend(std::iter::repeat(0u8).take(padding));
    }

    // Checksum (20 bytes) - just zeros for simplicity
    data.extend_from_slice(&[0u8; 20]);

    data
}

// Additional: Test status with empty repository (no commits)
#[test]
fn test_status_empty_repository() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create a file in the working tree
    fs::write(path.join("file.txt"), "content").unwrap();

    let repo = Repository::open(path).unwrap();
    let status = repo.status().unwrap();

    // All files should be untracked since there's no HEAD
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Untracked);
}

// Additional: Test status with nested directories
#[test]
fn test_status_nested_directories() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create nested structure
    fs::create_dir_all(path.join("src/lib")).unwrap();
    fs::write(path.join("src/lib/mod.rs"), "// mod").unwrap();
    fs::write(path.join("src/main.rs"), "fn main() {}").unwrap();

    let repo = Repository::open(path).unwrap();
    let status = repo.status().unwrap();

    // Should detect all untracked files
    assert_eq!(status.len(), 2);
    assert!(status.iter().all(|e| e.status() == FileStatus::Untracked));
}

// Additional: Test status sorting
#[test]
fn test_status_sorted() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create files in non-alphabetical order
    fs::write(path.join("z.txt"), "").unwrap();
    fs::write(path.join("a.txt"), "").unwrap();
    fs::write(path.join("m.txt"), "").unwrap();

    let repo = Repository::open(path).unwrap();
    let status = repo.status().unwrap();

    assert_eq!(status.len(), 3);
    assert_eq!(status[0].path(), Path::new("a.txt"));
    assert_eq!(status[1].path(), Path::new("m.txt"));
    assert_eq!(status[2].path(), Path::new("z.txt"));
}

/// Helper function to recursively copy a directory.
fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.as_ref().join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}
