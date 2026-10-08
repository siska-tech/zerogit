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

/// A repository with a big binary file and a big text file committed on
/// `main`.
fn committed() -> tempfile::TempDir {
    let temp = small_threshold();
    let dir = temp.path();
    fs::write(dir.join("big.bin"), noise(6, 300_000)).unwrap();
    write(dir, "big.txt", &"line\n".repeat(2000));
    write(dir, "small.txt", "small\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Big"]);
    temp
}

fn porcelain(repo: &Repository) -> Vec<String> {
    let mut lines: Vec<String> = repo
        .status()
        .unwrap()
        .iter()
        .map(|e| format!("{:?} {}", e.status(), e.path().display()))
        .collect();
    lines.sort();
    lines
}

#[test]
fn status_hashes_big_files_as_it_reads_them() {
    let temp = committed();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    assert!(porcelain(&repo).is_empty());

    // The same size, one byte changed.
    let mut changed = noise(6, 300_000);
    changed[150_000] ^= 1;
    fs::write(dir.join("big.bin"), &changed).unwrap();
    assert_eq!(porcelain(&repo), ["Modified big.bin"]);
    assert_eq!(git(dir, &["status", "--porcelain"]), " M big.bin\n");
    let diff = repo.diff_index_to_workdir().unwrap();
    assert_eq!(diff.len(), 1);
    assert_eq!(
        diff.deltas()[0].new_oid().unwrap().to_hex(),
        hash_object(dir, "big.bin")
    );

    // Back as it was: unchanged again.
    fs::write(dir.join("big.bin"), noise(6, 300_000)).unwrap();
    assert!(porcelain(&repo).is_empty());
}

#[test]
fn line_diffs_of_big_blobs_are_skipped_without_reading_them() {
    use zerogit::diff::{BlobDiffContent, DiffOptions, SkipReason};
    let temp = committed();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let oid = |revision: &str| repo.rev_parse(revision).unwrap();
    let (bin, txt, small) = (
        oid("HEAD:big.bin"),
        oid("HEAD:big.txt"),
        oid("HEAD:small.txt"),
    );
    let options = DiffOptions::new().max_input_size(1000);
    let skipped = BlobDiffContent::Skipped(SkipReason::InputTooLarge { limit: 1000 });

    let diff = repo.diff_blobs(Some(&small), Some(&bin), &options).unwrap();
    assert_eq!(diff.content(), &skipped);
    assert!(!diff.is_identical());
    assert_eq!((diff.old_size(), diff.new_size()), (6, 300_000));
    let diff = repo.diff_blobs(Some(&txt), Some(&txt), &options).unwrap();
    assert_eq!(diff.content(), &skipped);
    assert!(diff.is_identical());
    let diff = repo.diff_blobs(None, Some(&txt), &options).unwrap();
    assert!(!diff.is_identical() && !diff.old_exists());
    // Within the limit, the contents are compared.
    let diff = repo
        .diff_blobs(Some(&small), Some(&txt), &DiffOptions::new())
        .unwrap();
    assert!(matches!(diff.content(), BlobDiffContent::Text(_)));
    let diff = repo
        .diff_blobs(Some(&small), Some(&bin), &DiffOptions::new())
        .unwrap();
    assert!(matches!(diff.content(), BlobDiffContent::NonText(_)));
    // Not a blob.
    let tree = oid("HEAD^{tree}");
    assert!(repo.diff_blobs(Some(&tree), Some(&bin), &options).is_err());
}

#[test]
fn big_text_files_merge_line_by_line_as_in_git() {
    // Git 2.55 merges files above core.bigFileThreshold line by line too.
    let source = committed();
    let dir = source.path();
    git(dir, &["checkout", "-q", "-b", "other"]);
    let mut theirs = "line\n".repeat(2000);
    theirs.replace_range(0..4, "THEM");
    write(dir, "big.txt", &theirs);
    git(dir, &["commit", "-q", "-am", "Theirs"]);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "big.txt", &format!("{}ours\n", "line\n".repeat(2000)));
    git(dir, &["commit", "-q", "-am", "Ours"]);

    let with_git = twin(dir);
    git(with_git.path(), &["merge", "-q", "--no-edit", "other"]);
    let with_zerogit = twin(dir);
    let repo = Repository::open(with_zerogit.path()).unwrap();
    let outcome = repo
        .merge(
            "other",
            "Test",
            "test@example.com",
            &zerogit::MergeOptions::new(),
        )
        .unwrap();
    assert!(
        matches!(outcome, zerogit::MergeOutcome::Merged(_)),
        "{:?}",
        outcome
    );
    let tree = |dir: &Path| git(dir, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(tree(with_zerogit.path()), tree(with_git.path()));
    assert_eq!(
        worktree_files(with_zerogit.path()),
        worktree_files(with_git.path())
    );
}

#[test]
fn big_tracked_changes_are_stashed() {
    let temp = committed();
    let dir = temp.path();
    let changed = noise(7, 250_000);
    fs::write(dir.join("big.bin"), &changed).unwrap();
    let repo = Repository::open(dir).unwrap();
    repo.stash_save("Test", "test@example.com", &StashOptions::new())
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), noise(6, 300_000));
    assert_eq!(cat_blob(dir, "stash@{0}:big.bin"), changed);
    assert_fsck(dir);
    repo.stash_pop(0, false).unwrap();
    assert_eq!(fs::read(dir.join("big.bin")).unwrap(), changed);
}

#[test]
fn big_objects_are_repacked_as_streams() {
    let temp = committed();
    let dir = temp.path();
    // A second version: one big object loose, then both packed by Git.
    let second = noise(8, 400_000);
    fs::write(dir.join("big.bin"), &second).unwrap();
    git(dir, &["commit", "-q", "-am", "Second"]);
    let repo = Repository::open(dir).unwrap();

    // Loose objects, written whole into the new pack.
    repo.gc().unwrap();
    assert_fsck(dir);
    assert_eq!(cat_blob(dir, "HEAD:big.bin"), second);
    assert_eq!(cat_blob(dir, "HEAD~1:big.bin"), noise(6, 300_000));

    // Packed entries, copied from the existing pack.
    let repo = Repository::open(dir).unwrap();
    repo.gc().unwrap();
    assert_fsck(dir);
    let packs: Vec<_> = fs::read_dir(dir.join(".git/objects/pack"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "idx"))
        .collect();
    assert_eq!(packs.len(), 1);
    git(dir, &["verify-pack", packs[0].to_str().unwrap()]);
    assert_eq!(cat_blob(dir, "HEAD:big.bin"), second);
    assert_eq!(cat_blob(dir, "HEAD~1:big.bin"), noise(6, 300_000));

    // A pack written by Git is copied too.
    git(dir, &["gc", "-q"]);
    let repo = Repository::open(dir).unwrap();
    repo.gc().unwrap();
    assert_fsck(dir);
    assert_eq!(cat_blob(dir, "HEAD:big.bin"), second);
}
