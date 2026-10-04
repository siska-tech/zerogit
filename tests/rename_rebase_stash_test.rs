//! Rebases and stash applications follow renames like Git (#35): they use
//! the same tree merge as `merge`, with their own labels and state.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use common::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zerogit::{RebaseOutcome, Repository, StashApplyOutcome};

const TEXT: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

fn replace_line(text: &str, line: &str, with: &str) -> String {
    text.lines()
        .map(|l| if l == line { with } else { l })
        .map(|l| format!("{}\n", l))
        .collect()
}

/// Runs `step` (writes, renames) and commits it.
fn commit(dir: &Path, message: &str, step: impl Fn(&Path)) {
    step(dir);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn rename(dir: &Path, from: &str, to: &str) {
    fs::create_dir_all(dir.join(to).parent().unwrap()).unwrap();
    git(dir, &["mv", from, to]);
}

/// The trees and authors of the commits on HEAD since `base`, and the
/// index, work tree and status.
fn state(dir: &Path, base: &str) -> Vec<String> {
    vec![
        git(
            dir,
            &[
                "log",
                "--reverse",
                "--format=%T %an %ad %s",
                &format!("{}..HEAD", base),
            ],
        ),
        format!("{:?}", git_ls_files(dir)),
        format!("{:?}", worktree_files(dir)),
        git(dir, &["status", "--porcelain"]),
    ]
}

/// `main` with a base commit, and `topic` from it; the closures add each
/// side's commits. `topic` is checked out at the end.
fn branches(main: impl Fn(&Path), topic: impl Fn(&Path)) -> TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    commit(dir, "Base", |d| {
        write(d, "a.txt", TEXT);
        write(d, "keep.txt", "keep\n");
    });
    git(dir, &["branch", "base"]);
    git(dir, &["checkout", "-q", "-b", "topic"]);
    topic(dir);
    git(dir, &["checkout", "-q", "main"]);
    main(dir);
    git(dir, &["checkout", "-q", "topic"]);
    temp
}

/// Rebases `topic` onto `main` with Git and zerogit; returns zerogit's
/// outcome after checking that both left the same state.
fn rebase_both(source: &Path) -> RebaseOutcome {
    let ours = twin(source);
    let theirs = twin(source);
    let _ = git_output(theirs.path(), &["rebase", "-q", "main"]);
    let outcome = Repository::open(ours.path())
        .unwrap()
        .rebase("main", None, "Test", "test@example.com")
        .unwrap();
    assert_eq!(state(ours.path(), "main"), state(theirs.path(), "main"));
    outcome
}

#[test]
fn rebase_carries_changes_across_renames() {
    // The rebased commit renames a file that upstream changed.
    let temp = branches(
        |d| {
            commit(d, "Change", |d| {
                write(d, "a.txt", &replace_line(TEXT, "2", "two"))
            })
        },
        |d| commit(d, "Rename", |d| rename(d, "a.txt", "b.txt")),
    );
    assert!(matches!(
        rebase_both(temp.path()),
        RebaseOutcome::Completed(_)
    ));

    // Upstream renamed the file that the rebased commit changes.
    let temp = branches(
        |d| commit(d, "Rename", |d| rename(d, "a.txt", "dir/b.txt")),
        |d| {
            commit(d, "Change", |d| {
                write(d, "a.txt", &replace_line(TEXT, "9", "nine"))
            })
        },
    );
    assert!(matches!(
        rebase_both(temp.path()),
        RebaseOutcome::Completed(_)
    ));
}

#[test]
fn rebase_stops_on_conflicts_at_renamed_paths_like_git() {
    let temp = branches(
        |d| {
            commit(d, "Main", |d| {
                write(d, "a.txt", &replace_line(TEXT, "5", "five (main)"))
            })
        },
        |d| {
            commit(d, "Rename and change", |d| {
                rename(d, "a.txt", "b.txt");
                write(d, "b.txt", &replace_line(TEXT, "5", "five (topic)"));
            })
        },
    );
    let outcome = rebase_both(temp.path());
    assert!(
        matches!(outcome, RebaseOutcome::Conflicts { .. }),
        "{:?}",
        outcome
    );
}

/// Stashes a change to a.txt on main, renames a.txt to b.txt in a new
/// commit, then applies the stash with Git and zerogit.
fn stash_across_rename(change: &str, renamed_content: Option<&str>) -> (TempDir, TempDir) {
    let temp = empty_repository();
    let dir = temp.path();
    commit(dir, "Base", |d| {
        write(d, "a.txt", TEXT);
        write(d, "keep.txt", "keep\n");
    });
    write(dir, "a.txt", change);
    git(dir, &["stash", "-q"]);
    commit(dir, "Rename", |d| {
        rename(d, "a.txt", "b.txt");
        if let Some(content) = renamed_content {
            write(d, "b.txt", content);
        }
    });
    let ours = twin(dir);
    let theirs = twin(dir);
    let _ = git_output(theirs.path(), &["stash", "apply", "-q"]);
    (ours, theirs)
}

#[test]
fn stash_applies_across_renames() {
    let (ours, theirs) = stash_across_rename(&replace_line(TEXT, "2", "two (stash)"), None);
    let outcome = Repository::open(ours.path())
        .unwrap()
        .stash_apply(0, false)
        .unwrap();
    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(state(ours.path(), "HEAD"), state(theirs.path(), "HEAD"));
    let applied = fs::read_to_string(ours.path().join("b.txt")).unwrap();
    assert!(applied.contains("two (stash)"));
}

#[test]
fn stash_conflicts_at_renamed_paths_like_git() {
    let (ours, theirs) = stash_across_rename(
        &replace_line(TEXT, "5", "five (stash)"),
        Some(&replace_line(TEXT, "5", "five (renamed)")),
    );
    let outcome = Repository::open(ours.path())
        .unwrap()
        .stash_apply(0, false)
        .unwrap();
    assert!(
        matches!(outcome, StashApplyOutcome::Conflicts(_)),
        "{:?}",
        outcome
    );
    assert_eq!(state(ours.path(), "HEAD"), state(theirs.path(), "HEAD"));
}
