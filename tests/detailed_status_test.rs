//! `Repository::detailed_status` compared with `git status --porcelain=v2` (#24).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{ChangeState, ConflictKind, DetailedStatus, Repository};

/// `(code, path)` pairs from Git, sorted.
fn git_status(dir: &Path) -> Vec<(String, String)> {
    let output = git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=none",
        ],
    );
    let mut result: Vec<(String, String)> = output
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            match fields[0] {
                "1" => (fields[1].to_owned(), fields[8..].join(" ")),
                "u" => (fields[1].to_owned(), fields[10..].join(" ")),
                "?" => ("?".to_owned(), fields[1..].join(" ")),
                other => panic!("unexpected line kind {}: {}", other, line),
            }
        })
        .collect();
    result.sort();
    result
}

fn zerogit_status(dir: &Path) -> Vec<(String, String)> {
    let repo = Repository::open(dir).unwrap();
    let mut result: Vec<(String, String)> = repo
        .detailed_status()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e.status().code(),
                e.path().to_string_lossy().replace('\\', "/"),
            )
        })
        .collect();
    result.sort();
    result
}

#[test]
fn staged_unstaged_and_untracked_changes_match_git() {
    let temp = repository(&[
        "staged.txt",
        "unstaged.txt",
        "both.txt",
        "staged-delete.txt",
        "unstaged-delete.txt",
        "untracked-again.txt",
        "unchanged.txt",
        "dir/nested.txt",
    ]);
    let dir = temp.path();
    write(dir, ".gitignore", "*.log\n");
    write(dir, "staged.txt", "staged\n");
    git(dir, &["add", "staged.txt", ".gitignore"]);
    write(dir, "unstaged.txt", "unstaged\n");
    write(dir, "both.txt", "first\n");
    git(dir, &["add", "both.txt"]);
    write(dir, "both.txt", "second\n");
    git(dir, &["rm", "-q", "staged-delete.txt"]);
    fs::remove_file(dir.join("unstaged-delete.txt")).unwrap();
    git(dir, &["rm", "-q", "--cached", "untracked-again.txt"]);
    write(dir, "added.txt", "new\n");
    git(dir, &["add", "added.txt"]);
    write(dir, "added-then-deleted.txt", "new\n");
    git(dir, &["add", "added-then-deleted.txt"]);
    fs::remove_file(dir.join("added-then-deleted.txt")).unwrap();
    write(dir, "intent.txt", "intent\n");
    git(dir, &["add", "-N", "intent.txt"]);
    write(dir, "dir/nested.txt", "changed\n");
    write(dir, "new/untracked.txt", "untracked\n");
    write(dir, "debug.log", "ignored\n");

    let expected = git_status(dir);
    assert_eq!(zerogit_status(dir), expected);
    // The fixture covers each kind of line.
    for code in ["M.", ".M", "MM", "A.", "AD", "D.", ".D", ".A", "?"] {
        assert!(expected.iter().any(|(c, _)| c == code), "{}", code);
    }
}

#[test]
fn clean_repository_has_no_entries() {
    let temp = repository(&["a.txt", "b/c.txt"]);
    assert!(Repository::open(temp.path())
        .unwrap()
        .detailed_status()
        .unwrap()
        .is_empty());
}

#[test]
fn conflict_kinds_match_git() {
    let temp = repository(&["both.txt", "deleted-by-us.txt", "deleted-by-them.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "other"]);
    write(dir, "both.txt", "theirs\n");
    write(dir, "deleted-by-us.txt", "theirs\n");
    git(dir, &["rm", "-q", "deleted-by-them.txt"]);
    write(dir, "added-both.txt", "theirs\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Theirs"]);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "both.txt", "ours\n");
    git(dir, &["rm", "-q", "deleted-by-us.txt"]);
    write(dir, "deleted-by-them.txt", "ours\n");
    write(dir, "added-both.txt", "ours\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Ours"]);
    assert!(!git_output(dir, &["merge", "-q", "other"]).status.success());

    let expected = git_status(dir);
    assert_eq!(zerogit_status(dir), expected);

    let repo = Repository::open(dir).unwrap();
    let kinds: Vec<ConflictKind> = repo
        .detailed_status()
        .unwrap()
        .iter()
        .filter_map(|e| match e.status() {
            DetailedStatus::Unmerged(kind) => Some(kind),
            _ => None,
        })
        .collect();
    for kind in [
        ConflictKind::BothModified,
        ConflictKind::BothAdded,
        ConflictKind::DeletedByUs,
        ConflictKind::DeletedByThem,
    ] {
        assert!(kinds.contains(&kind), "{:?}", kind);
    }
}

#[test]
fn status_accessors() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    let entries = repo.detailed_status().unwrap();
    assert_eq!(entries.len(), 1);
    let status = entries[0].status();
    assert_eq!(
        status,
        DetailedStatus::Changed {
            index: ChangeState::Unmodified,
            worktree: ChangeState::Modified
        }
    );
    assert!(!status.is_staged());
    assert!(status.is_unstaged());
    assert_eq!(entries[0].head_mode(), Some(zerogit::FileMode::Regular));
}

#[cfg(unix)]
#[test]
fn type_and_mode_changes_match_git() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = repository(&["to-link.txt", "staged-link.txt", "run.sh", "target.txt"]);
    let dir = temp.path();
    fs::remove_file(dir.join("to-link.txt")).unwrap();
    symlink("target.txt", dir.join("to-link.txt")).unwrap();
    fs::remove_file(dir.join("staged-link.txt")).unwrap();
    symlink("target.txt", dir.join("staged-link.txt")).unwrap();
    git(dir, &["add", "staged-link.txt"]);
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();

    let expected = git_status(dir);
    assert_eq!(zerogit_status(dir), expected);
    for code in [".T", "T.", ".M"] {
        assert!(expected.iter().any(|(c, _)| c == code), "{}", code);
    }
}
