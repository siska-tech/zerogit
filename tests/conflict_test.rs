//! Committing, staging and resetting while the index has merge conflicts (#23).

mod common;

use common::*;
use std::path::{Path, PathBuf};
use zerogit::{Error, Repository};

/// Every loose object file, so a test can check nothing new was written.
fn loose_objects(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir.join(".git/objects")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().len() == 2 {
            for object in std::fs::read_dir(entry.path()).unwrap() {
                files.push(object.unwrap().path());
            }
        }
    }
    files.sort();
    files
}

#[test]
fn commit_with_conflicts_fails_without_writing() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);
    let objects = loose_objects(dir);

    let err = repo.create_commit("Merge", "Test", "test@example.com");
    match err {
        Err(Error::UnmergedPaths(paths)) => assert_eq!(paths, vec![PathBuf::from("file.txt")]),
        other => panic!("expected UnmergedPaths, got {:?}", other),
    }
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(loose_objects(dir), objects);
}

#[test]
fn add_resolves_all_stages_and_commit_is_valid() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    write(dir, "file.txt", "resolved\n");
    repo.add("file.txt").unwrap();

    let lines = git_ls_files(dir);
    let file_lines: Vec<_> = lines.iter().filter(|l| l.ends_with("\tfile.txt")).collect();
    assert_eq!(file_lines.len(), 1);
    assert!(file_lines[0].contains(" 0\t"), "{:?}", file_lines);

    repo.create_commit("Resolve", "Test", "test@example.com")
        .unwrap();
    assert_fsck(dir);
    assert_eq!(git(dir, &["show", "HEAD:file.txt"]), "resolved\n");
}

#[test]
fn add_all_resolves_conflicts() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    write(dir, "file.txt", "resolved\n");
    repo.add_all().unwrap();
    let lines = git_ls_files(dir);
    assert!(lines.iter().all(|l| l.contains(" 0\t")), "{:?}", lines);
    repo.create_commit("Resolve", "Test", "test@example.com")
        .unwrap();
    assert_fsck(dir);
}

#[test]
fn add_of_deleted_conflicted_path_resolves_by_removal() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    std::fs::remove_file(dir.join("file.txt")).unwrap();
    repo.add("file.txt").unwrap();
    assert!(!git_ls_files(dir).iter().any(|l| l.ends_with("\tfile.txt")));
    repo.create_commit("Drop file", "Test", "test@example.com")
        .unwrap();
    assert_fsck(dir);
    assert_eq!(git(dir, &["ls-tree", "--name-only", "HEAD"]), "other.txt\n");
}

#[test]
fn reset_path_restores_head_entry_at_stage_zero() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    repo.reset(Some("file.txt")).unwrap();
    let head_blob = git(dir, &["rev-parse", "HEAD:file.txt"]);
    let lines = git_ls_files(dir);
    let file_lines: Vec<_> = lines.iter().filter(|l| l.ends_with("\tfile.txt")).collect();
    assert_eq!(
        file_lines,
        vec![&format!("100644 {} 0\tfile.txt", head_blob.trim())]
    );
}

#[test]
fn reset_all_clears_conflicts() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    repo.reset(None::<&str>).unwrap();
    let lines = git_ls_files(dir);
    assert!(lines.iter().all(|l| l.contains(" 0\t")), "{:?}", lines);
    // Git agrees the index now matches HEAD.
    git(dir, &["diff", "--cached", "--quiet"]);
}

#[test]
fn checkout_with_conflicts_is_rejected() {
    let temp = conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = git(dir, &["symbolic-ref", "HEAD"]);
    let index = std::fs::read(dir.join(".git/index")).unwrap();

    assert!(matches!(
        repo.checkout("other"),
        Err(Error::UnmergedPaths(_))
    ));
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), head);
    assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), index);
}

#[test]
fn tree_entries_use_git_order_for_directories() {
    let temp = empty_repository();
    let dir = temp.path();
    // Name order puts the directory "foo" first; Git compares it as "foo/",
    // which sorts after "foo.txt" and "foo-bar".
    write(dir, "foo.txt", "a\n");
    write(dir, "foo-bar", "b\n");
    write(dir, "foo/bar.txt", "c\n");
    let repo = Repository::open(dir).unwrap();
    repo.add_all().unwrap();
    let oid = repo
        .create_commit("Initial", "Test", "test@example.com")
        .unwrap();
    assert_fsck(dir);
    let tree = git(dir, &["write-tree"]);
    assert_eq!(git(dir, &["rev-parse", &format!("{}^{{tree}}", oid)]), tree);
}

#[test]
fn status_reports_conflicted_path_once_as_modified() {
    let temp = conflicted_repository();
    let repo = Repository::open(temp.path()).unwrap();
    let status = repo.status().unwrap();
    let conflicted: Vec<_> = status
        .iter()
        .filter(|e| e.path() == Path::new("file.txt"))
        .collect();
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].status(), zerogit::FileStatus::Modified);
}
