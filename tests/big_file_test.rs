//! Files larger than `core.bigFileThreshold` are streamed between the work
//! tree and the object store when they need no conversion (#78). The
//! threshold is set low here, so the streamed paths are taken by small
//! files; the objects and files written must be the ones Git writes.

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Repository, RestoreOptions, StashOptions};

/// Deterministic, poorly compressible bytes.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

/// An empty repository whose big file threshold is 1 KiB.
fn small_threshold() -> tempfile::TempDir {
    let temp = empty_repository();
    git(temp.path(), &["config", "core.bigFileThreshold", "1k"]);
    temp
}

fn hash_object(dir: &Path, path: &str) -> String {
    git(dir, &["hash-object", "--", path]).trim().to_owned()
}

fn cat_blob(dir: &Path, revision: &str) -> Vec<u8> {
    git_output(dir, &["cat-file", "blob", revision]).stdout
}

#[test]
fn added_big_files_get_the_objects_git_writes() {
    let temp = small_threshold();
    let dir = temp.path();
    fs::write(dir.join("big.bin"), noise(1, 300_000)).unwrap();
    // Exactly the threshold is not "larger than" it.
    fs::write(dir.join("edge.bin"), noise(2, 1024)).unwrap();
    fs::write(dir.join("small.txt"), "small\n").unwrap();
    fs::create_dir(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/zeros.bin"), vec![0; 2 << 20]).unwrap();

    let repo = Repository::open(dir).unwrap();
    repo.add("big.bin").unwrap();
    repo.add_all().unwrap();
    for path in ["big.bin", "edge.bin", "small.txt", "sub/zeros.bin"] {
        let staged = git(dir, &["rev-parse", &format!(":{}", path)]);
        assert_eq!(staged.trim(), hash_object(dir, path), "{}", path);
    }
    assert_eq!(git(dir, &["diff", "--name-only"]), "");
    git(dir, &["commit", "-q", "-m", "Big"]);
    assert_fsck(dir);
    assert_eq!(cat_blob(dir, "HEAD:big.bin"), noise(1, 300_000));

    // Adding the same content again finds the object already there.
    repo.add("big.bin").unwrap();
    assert_fsck(dir);
    // No temporary files are left among the objects.
    let leftovers: Vec<_> = fs::read_dir(dir.join(".git/objects"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("tmp_obj_"))
        .collect();
    assert!(leftovers.is_empty(), "{:?}", leftovers);
}

#[test]
fn big_files_that_need_conversion_are_converted() {
    let temp = small_threshold();
    let dir = temp.path();
    git(dir, &["config", "core.autocrlf", "true"]);
    let text = "line\r\n".repeat(1000);
    write(dir, "text.txt", &text);
    write(dir, ".gitattributes", "*.dat binary\n");
    write(dir, "crlf.dat", &text);

    let repo = Repository::open(dir).unwrap();
    repo.add_all().unwrap();
    // Converted, as Git converts it; the binary one is stored as it is.
    let staged = git(dir, &["rev-parse", ":text.txt", ":crlf.dat"]);
    let staged: Vec<&str> = staged.lines().collect();
    assert_eq!(cat_blob(dir, staged[0]), "line\n".repeat(1000).as_bytes());
    assert_eq!(cat_blob(dir, staged[1]), text.as_bytes());
    assert_eq!(staged[0], hash_object(dir, "text.txt"));
    assert_eq!(staged[1], hash_object(dir, "crlf.dat"));

    // Checked out again with CRLF line endings.
    git(dir, &["commit", "-q", "-m", "Text"]);
    fs::remove_file(dir.join("text.txt")).unwrap();
    fs::remove_file(dir.join("crlf.dat")).unwrap();
    repo.restore(&["text.txt", "crlf.dat"], &RestoreOptions::new())
        .unwrap();
    assert_eq!(fs::read_to_string(dir.join("text.txt")).unwrap(), text);
    assert_eq!(fs::read_to_string(dir.join("crlf.dat")).unwrap(), text);
}

#[test]
fn big_files_are_checked_out_and_restored() {
    let temp = small_threshold();
    let dir = temp.path();
    let first = noise(3, 500_000);
    let second = noise(4, 400_000);
    fs::write(dir.join("big.bin"), &first).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "First"]);
    git(dir, &["checkout", "-q", "-b", "other"]);
    fs::write(dir.join("big.bin"), &second).unwrap();
    fs::create_dir(dir.join("dir")).unwrap();
    fs::write(dir.join("dir/new.bin"), &first).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Second"]);
    git(dir, &["checkout", "-q", "main"]);
    // Packed objects are streamed too.
    git(dir, &["gc", "-q"]);

    let repo = Repository::open(dir).unwrap();
    repo.checkout("other").unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), second);
    assert_eq!(fs::read(dir.join("dir/new.bin")).unwrap(), first);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");

    repo.checkout("main").unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), first);
    assert!(!dir.join("dir").exists());

    fs::write(dir.join("big.bin"), b"changed").unwrap();
    repo.restore(&["big.bin"], &RestoreOptions::new().source("other"))
        .unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), second);
    repo.restore(&["big.bin"], &RestoreOptions::new()).unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), first);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[test]
fn untracked_big_files_are_stashed_and_restored() {
    let temp = small_threshold();
    let dir = temp.path();
    write(dir, "file.txt", "tracked\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Initial"]);
    let big = noise(5, 200_000);
    fs::write(dir.join("big.bin"), &big).unwrap();

    let repo = Repository::open(dir).unwrap();
    let options = StashOptions::new().include_untracked(true);
    repo.stash_save("Test", "test@example.com", &options)
        .unwrap()
        .unwrap();
    assert!(!dir.join("big.bin").exists());
    assert_eq!(cat_blob(dir, "stash^3:big.bin"), big);
    assert_fsck(dir);

    repo.stash_pop(0, false).unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), big);
}
