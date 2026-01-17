//! Integration tests for staging area operations (add, add_all, reset).
//!
//! Test cases: W-001 to W-003

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zerogit::repository::Repository;
use zerogit::status::FileStatus;

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

/// Helper to create a repository with an initial commit.
fn create_repo_with_commit() -> TempDir {
    let temp = create_test_repo();
    let path = temp.path();
    let git_dir = path.join(".git");

    // Create a file
    fs::write(path.join("file.txt"), "initial content\n").unwrap();

    // Create blob object
    let content = b"initial content\n";
    let blob_oid = create_object(&git_dir.join("objects"), content, "blob");

    // Create tree
    let tree_content = create_tree_content(&[("file.txt", "100644", &blob_oid)]);
    let tree_oid = create_object(&git_dir.join("objects"), &tree_content, "tree");

    // Create commit
    let commit_content = format!(
        "tree {}\nauthor Test <test@test.com> 1700000000 +0000\ncommitter Test <test@test.com> 1700000000 +0000\n\nInitial commit\n",
        tree_oid
    );
    let commit_oid = create_object(&git_dir.join("objects"), commit_content.as_bytes(), "commit");

    // Set HEAD
    fs::write(git_dir.join("refs/heads/main"), format!("{}\n", commit_oid)).unwrap();

    // Create index matching the commit
    let index_data = create_index(&[("file.txt", &blob_oid, content.len())]);
    fs::write(git_dir.join("index"), &index_data).unwrap();

    temp
}

// W-001: Repository::add() stages a single file
#[test]
fn test_w001_add_single_file() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create a new file
    fs::write(path.join("new_file.txt"), "new content").unwrap();

    let repo = Repository::open(path).unwrap();

    // Initially the file should be untracked
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Untracked);

    // Add the file
    repo.add("new_file.txt").unwrap();

    // Now the file should be staged (Added)
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path(), Path::new("new_file.txt"));
    assert_eq!(status[0].status(), FileStatus::Added);
}

// W-001: Repository::add() updates an already staged file
#[test]
fn test_w001_add_updates_staged_file() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create and add a file
    fs::write(path.join("file.txt"), "content v1").unwrap();
    let repo = Repository::open(path).unwrap();
    repo.add("file.txt").unwrap();

    // Modify the file
    fs::write(path.join("file.txt"), "content v2").unwrap();

    // Status should show modified (unstaged change)
    let status = repo.status().unwrap();
    assert!(
        status.iter().any(|e| e.path() == Path::new("file.txt")),
        "File should appear in status"
    );

    // Re-add the file
    repo.add("file.txt").unwrap();

    // Status should now show only staged
    let status = repo.status().unwrap();
    let file_status = status.iter().find(|e| e.path() == Path::new("file.txt"));
    assert!(
        file_status.is_some(),
        "File should be in status after re-add"
    );
    assert_eq!(file_status.unwrap().status(), FileStatus::Added);
}

// W-001: Repository::add() with nested path
#[test]
fn test_w001_add_nested_file() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create nested directory structure
    fs::create_dir_all(path.join("src/lib")).unwrap();
    fs::write(path.join("src/lib/mod.rs"), "// module").unwrap();

    let repo = Repository::open(path).unwrap();
    repo.add("src/lib/mod.rs").unwrap();

    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Added);
}

// W-001: Repository::add() returns error for non-existent file
#[test]
fn test_w001_add_nonexistent_file() {
    let temp = create_test_repo();
    let path = temp.path();

    let repo = Repository::open(path).unwrap();
    let result = repo.add("nonexistent.txt");

    assert!(result.is_err());
}

// W-002: Repository::add_all() stages all changes
#[test]
fn test_w002_add_all_stages_all() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create multiple files
    fs::write(path.join("a.txt"), "content a").unwrap();
    fs::write(path.join("b.txt"), "content b").unwrap();
    fs::create_dir(path.join("dir")).unwrap();
    fs::write(path.join("dir/c.txt"), "content c").unwrap();

    let repo = Repository::open(path).unwrap();

    // All should be untracked initially
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 3);
    assert!(status.iter().all(|e| e.status() == FileStatus::Untracked));

    // Add all
    repo.add_all().unwrap();

    // All should be staged now
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 3);
    assert!(
        status.iter().all(|e| e.status() == FileStatus::Added),
        "All files should be staged as Added, got: {:?}",
        status
    );
}

// W-002: Repository::add_all() handles deleted files
#[test]
fn test_w002_add_all_handles_deletions() {
    let temp = create_repo_with_commit();
    let path = temp.path();

    // Delete the tracked file
    fs::remove_file(path.join("file.txt")).unwrap();

    let repo = Repository::open(path).unwrap();

    // Initially should show as deleted (unstaged)
    let status = repo.status().unwrap();
    assert!(
        status.iter().any(|e| e.status() == FileStatus::Deleted),
        "Should detect deleted file, got: {:?}",
        status
    );

    // Add all (including deletions)
    repo.add_all().unwrap();

    // Should now be staged deletion
    let status = repo.status().unwrap();
    assert!(
        status.iter().any(|e| e.status() == FileStatus::StagedDeleted),
        "Should stage deletion, got: {:?}",
        status
    );
}

// W-003: Repository::reset() unstages all changes
#[test]
fn test_w003_reset_unstages_all() {
    let temp = create_repo_with_commit();
    let path = temp.path();

    // Create and stage a new file
    fs::write(path.join("new_file.txt"), "new content").unwrap();
    let repo = Repository::open(path).unwrap();
    repo.add("new_file.txt").unwrap();

    // Verify it's staged
    let status = repo.status().unwrap();
    assert!(
        status.iter().any(|e| e.status() == FileStatus::Added),
        "File should be staged"
    );

    // Reset all
    repo.reset(None::<&str>).unwrap();

    // File should now be untracked (not in HEAD, so removed from index)
    let status = repo.status().unwrap();
    let new_file_status = status.iter().find(|e| e.path() == Path::new("new_file.txt"));
    assert!(
        new_file_status.is_some(),
        "new_file.txt should still exist in working tree"
    );
    assert_eq!(
        new_file_status.unwrap().status(),
        FileStatus::Untracked,
        "File should be untracked after reset"
    );
}

// W-003: Repository::reset() with specific path
#[test]
fn test_w003_reset_specific_path() {
    let temp = create_repo_with_commit();
    let path = temp.path();

    // Modify and stage the existing file
    fs::write(path.join("file.txt"), "modified content").unwrap();
    let repo = Repository::open(path).unwrap();
    repo.add("file.txt").unwrap();

    // Also create and stage a new file
    fs::write(path.join("other.txt"), "other content").unwrap();
    repo.add("other.txt").unwrap();

    // Reset only one file
    repo.reset(Some("file.txt")).unwrap();

    // file.txt should be modified (unstaged), other.txt should still be staged
    let status = repo.status().unwrap();

    let file_status = status.iter().find(|e| e.path() == Path::new("file.txt"));
    assert_eq!(
        file_status.map(|e| e.status()),
        Some(FileStatus::Modified),
        "file.txt should be Modified after reset"
    );

    let other_status = status.iter().find(|e| e.path() == Path::new("other.txt"));
    assert_eq!(
        other_status.map(|e| e.status()),
        Some(FileStatus::Added),
        "other.txt should still be Added"
    );
}

// W-003: Repository::reset() on empty repo
#[test]
fn test_w003_reset_empty_repo() {
    let temp = create_test_repo();
    let path = temp.path();

    // Create and stage a file
    fs::write(path.join("file.txt"), "content").unwrap();
    let repo = Repository::open(path).unwrap();
    repo.add("file.txt").unwrap();

    // Reset (no HEAD exists)
    repo.reset(None::<&str>).unwrap();

    // File should be untracked
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Untracked);
}

// Additional: Multiple add operations work correctly
#[test]
fn test_add_multiple_times() {
    let temp = create_test_repo();
    let path = temp.path();

    fs::write(path.join("file.txt"), "content").unwrap();

    let repo = Repository::open(path).unwrap();
    repo.add("file.txt").unwrap();
    repo.add("file.txt").unwrap(); // Add again - should be idempotent

    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Added);
}

// Additional: add() creates blob object
#[test]
fn test_add_creates_blob() {
    let temp = create_test_repo();
    let path = temp.path();
    let git_dir = path.join(".git");

    fs::write(path.join("file.txt"), "test content").unwrap();

    let repo = Repository::open(path).unwrap();
    repo.add("file.txt").unwrap();

    // Check that some object was created
    let objects_dir = git_dir.join("objects");
    let has_objects = fs::read_dir(&objects_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().is_dir() && e.file_name().to_string_lossy().len() == 2);

    assert!(has_objects, "Should have created blob object");
}

// Helper functions for test setup

fn create_object(objects_dir: &Path, content: &[u8], object_type: &str) -> String {
    use miniz_oxide::deflate::compress_to_vec_zlib;

    let header = format!("{} {}\0", object_type, content.len());
    let mut raw = header.into_bytes();
    raw.extend_from_slice(content);

    let hash = sha1_hash(&raw);
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();

    let compressed = compress_to_vec_zlib(&raw, 6);
    let object_path = objects_dir.join(&hex[..2]).join(&hex[2..]);
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(&object_path, &compressed).unwrap();

    hex
}

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

fn create_tree_content(entries: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut content = Vec::new();
    for (name, mode, oid_hex) in entries {
        content.extend_from_slice(mode.as_bytes());
        content.push(b' ');
        content.extend_from_slice(name.as_bytes());
        content.push(0);
        for i in 0..20 {
            let byte = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
            content.push(byte);
        }
    }
    content
}

fn create_index(entries: &[(&str, &str, usize)]) -> Vec<u8> {
    let mut data = Vec::new();

    // Header: DIRC, version 2, entry count
    data.extend_from_slice(b"DIRC");
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(&(entries.len() as u32).to_be_bytes());

    for (name, oid_hex, size) in entries {
        let entry_start = data.len();

        // ctime_sec, ctime_nsec
        data.extend_from_slice(&1700000000u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        // mtime_sec, mtime_nsec
        data.extend_from_slice(&1700000001u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        // dev
        data.extend_from_slice(&0u32.to_be_bytes());
        // ino
        data.extend_from_slice(&0u32.to_be_bytes());
        // mode (100644 = regular file)
        data.extend_from_slice(&0o100644u32.to_be_bytes());
        // uid
        data.extend_from_slice(&0u32.to_be_bytes());
        // gid
        data.extend_from_slice(&0u32.to_be_bytes());
        // size
        data.extend_from_slice(&(*size as u32).to_be_bytes());

        // SHA-1
        for i in 0..20 {
            let byte = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
            data.push(byte);
        }

        // flags
        let name_len = name.len().min(0xFFF) as u16;
        data.extend_from_slice(&name_len.to_be_bytes());

        // name
        data.extend_from_slice(name.as_bytes());

        // Padding
        let entry_size = data.len() - entry_start;
        let padding = (8 - (entry_size % 8)) % 8;
        let padding = if padding == 0 { 8 } else { padding };
        data.extend(std::iter::repeat(0u8).take(padding));
    }

    // Checksum
    let checksum = sha1_hash(&data);
    data.extend_from_slice(&checksum);

    data
}
