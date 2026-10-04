//! Merges follow a file renamed on one side, as Git's `ort` strategy does
//! (#35): the other side's changes are applied at the new path.
//!
//! Each scenario is merged with Git in one copy and with zerogit in the
//! other; the outcome, index, work tree and status must agree.

mod common;

use common::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zerogit::{MergeOptions, MergeOutcome, Repository};

/// Ten lines, so that a rename with a one-line change stays similar.
const TEXT: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

fn replace_line(text: &str, line: &str, with: &str) -> String {
    text.lines()
        .map(|l| if l == line { with } else { l })
        .map(|l| format!("{}\n", l))
        .collect()
}

/// Writes files (`"-"` deletes one) and stages everything.
fn apply(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        if *content == "-" {
            fs::remove_file(dir.join(path)).unwrap();
        } else {
            write(dir, path, content);
        }
    }
    git(dir, &["add", "-A"]);
}

/// `main` and `topic` diverge from a base commit.
fn diverged(
    base: &[(&str, &str)],
    main: &[(&str, &str)],
    topic: &[(&str, &str)],
    config: &[(&str, &str)],
) -> TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    for (key, value) in config {
        git(dir, &["config", key, value]);
    }
    apply(dir, base);
    git(dir, &["commit", "-q", "-m", "Base"]);
    git(dir, &["checkout", "-q", "-b", "topic"]);
    apply(dir, topic);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "Topic"]);
    git(dir, &["checkout", "-q", "main"]);
    apply(dir, main);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "Main"]);
    temp
}

/// Merges `topic` into `main` with Git and with zerogit and compares the
/// results. Returns zerogit's outcome and the zerogit copy.
fn merge_both(source: &Path) -> (MergeOutcome, TempDir) {
    let ours = twin(source);
    let theirs = twin(source);
    let git_clean = git_output(theirs.path(), &["merge", "-q", "--no-edit", "topic"])
        .status
        .success();
    let outcome = Repository::open(ours.path())
        .unwrap()
        .merge("topic", "Test", "test@example.com", &MergeOptions::new())
        .unwrap();
    assert_eq!(
        matches!(outcome, MergeOutcome::Merged(_)),
        git_clean,
        "{:?}",
        outcome
    );
    assert_eq!(
        git_ls_files(ours.path()),
        git_ls_files(theirs.path()),
        "index"
    );
    assert_eq!(
        worktree_files(ours.path()),
        worktree_files(theirs.path()),
        "work tree"
    );
    assert_eq!(
        git(ours.path(), &["status", "--porcelain"]),
        git(theirs.path(), &["status", "--porcelain"]),
        "status"
    );
    if git_clean {
        assert_eq!(
            git(ours.path(), &["rev-parse", "HEAD^{tree}"]),
            git(theirs.path(), &["rev-parse", "HEAD^{tree}"])
        );
    }
    (outcome, ours)
}

fn files(dir: &Path) -> Vec<String> {
    worktree_files(dir).into_iter().map(|(p, _)| p).collect()
}

#[test]
fn rename_on_our_side_takes_their_changes() {
    let changed = replace_line(TEXT, "2", "two (topic)");
    let temp = diverged(
        &[("a.txt", TEXT)],
        &[("a.txt", "-"), ("b.txt", TEXT)],
        &[("a.txt", &changed)],
        &[],
    );
    let (outcome, ours) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Merged(_)));
    assert_eq!(files(ours.path()), ["b.txt"]);
    assert_eq!(
        fs::read_to_string(ours.path().join("b.txt")).unwrap(),
        changed
    );
}

#[test]
fn rename_on_their_side_takes_our_changes() {
    let changed = replace_line(TEXT, "2", "two (main)");
    let temp = diverged(
        &[("a.txt", TEXT)],
        &[("a.txt", &changed)],
        &[("a.txt", "-"), ("dir/b.txt", TEXT)],
        &[],
    );
    let (outcome, ours) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Merged(_)));
    assert_eq!(files(ours.path()), ["dir/b.txt"]);
}

#[test]
fn similar_renames_merge_line_by_line() {
    // Renamed and changed on main, changed elsewhere on topic.
    let main_text = replace_line(TEXT, "9", "nine (main)");
    let topic_text = replace_line(TEXT, "2", "two (topic)");
    let temp = diverged(
        &[("a.txt", TEXT), ("keep.txt", "keep\n")],
        &[("a.txt", "-"), ("b.txt", &main_text)],
        &[("a.txt", &topic_text), ("keep.txt", "kept\n")],
        &[],
    );
    let (outcome, ours) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Merged(_)));
    let merged = fs::read_to_string(ours.path().join("b.txt")).unwrap();
    assert!(merged.contains("two (topic)") && merged.contains("nine (main)"));
}

#[test]
fn conflicts_at_the_new_path_name_both_paths() {
    let main_text = replace_line(TEXT, "5", "five (main)");
    let topic_text = replace_line(TEXT, "5", "five (topic)");
    for style in ["merge", "diff3"] {
        let temp = diverged(
            &[("a.txt", TEXT)],
            &[("a.txt", "-"), ("b.txt", &main_text)],
            &[("a.txt", &topic_text)],
            &[("merge.conflictStyle", style)],
        );
        let (outcome, ours) = merge_both(temp.path());
        assert!(matches!(outcome, MergeOutcome::Conflicts(ref p) if p == &[Path::new("b.txt")]));
        let content = fs::read_to_string(ours.path().join("b.txt")).unwrap();
        assert!(content.contains("<<<<<<< HEAD:b.txt"), "{}", content);
        assert!(content.contains(">>>>>>> topic:a.txt"), "{}", content);
    }
}

#[test]
fn same_rename_on_both_sides_and_mode_changes() {
    let changed = replace_line(TEXT, "3", "three (topic)");
    let temp = diverged(
        &[("a.txt", TEXT), ("run.sh", TEXT)],
        &[
            ("a.txt", "-"),
            ("b.txt", TEXT),
            ("run.sh", "-"),
            ("bin/run.sh", TEXT),
        ],
        &[("a.txt", "-"), ("b.txt", &changed)],
        &[],
    );
    // The executable bit set on topic follows main's rename of run.sh.
    git(temp.path(), &["checkout", "-q", "topic"]);
    git(temp.path(), &["update-index", "--chmod=+x", "run.sh"]);
    git(temp.path(), &["commit", "-q", "-m", "Executable"]);
    // Only the index has the executable bit (the file on disk does not);
    // on Unix that is a local change, so leave it behind.
    git(temp.path(), &["checkout", "-q", "-f", "main"]);
    let (outcome, _) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Merged(_)));
}

#[test]
fn renames_are_not_followed_when_disabled() {
    let changed = replace_line(TEXT, "2", "two (topic)");
    let temp = diverged(
        &[("a.txt", TEXT)],
        &[("a.txt", "-"), ("b.txt", TEXT)],
        &[("a.txt", &changed)],
        &[("merge.renames", "false")],
    );
    // Without rename detection, the deletion conflicts with the change.
    let (outcome, _) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
}

#[test]
fn rename_against_delete_conflicts_like_git() {
    let changed = replace_line(TEXT, "2", "two (main)");
    // Renamed (with or without changes) on main, deleted on topic.
    for main_text in [TEXT, changed.as_str()] {
        let temp = diverged(
            &[("a.txt", TEXT), ("keep.txt", "keep\n")],
            &[("a.txt", "-"), ("b.txt", main_text)],
            &[("a.txt", "-")],
            &[],
        );
        let (outcome, _) = merge_both(temp.path());
        assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
    }
    // Deleted on main, renamed on topic.
    let temp = diverged(
        &[("a.txt", TEXT), ("keep.txt", "keep\n")],
        &[("a.txt", "-")],
        &[("a.txt", "-"), ("dir/b.txt", TEXT)],
        &[],
    );
    let (outcome, _) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
}

#[test]
fn different_renames_conflict_like_git() {
    let main_text = replace_line(TEXT, "2", "two (main)");
    let topic_text = replace_line(TEXT, "9", "nine (topic)");
    let main_conflicting = replace_line(TEXT, "5", "five (main)");
    let topic_conflicting = replace_line(TEXT, "5", "five (topic)");
    for (main_text, topic_text) in [
        (TEXT, TEXT),
        (main_text.as_str(), topic_text.as_str()),
        (main_conflicting.as_str(), topic_conflicting.as_str()),
    ] {
        for style in ["merge", "diff3"] {
            let temp = diverged(
                &[("a.txt", TEXT), ("keep.txt", "keep\n")],
                &[("a.txt", "-"), ("b.txt", main_text)],
                &[("a.txt", "-"), ("c.txt", topic_text)],
                &[("merge.conflictStyle", style)],
            );
            let (outcome, _) = merge_both(temp.path());
            assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
        }
    }
}

#[test]
fn renames_onto_the_same_path_are_add_add_conflicts() {
    // Two files renamed to the same path, one on each side.
    let temp = diverged(
        &[("a.txt", TEXT), ("other.txt", "x\ny\nz\n")],
        &[("a.txt", "-"), ("c.txt", TEXT)],
        &[("other.txt", "-"), ("c.txt", "x\ny\nz\n")],
        &[],
    );
    let (outcome, _) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
    // Renamed on one side onto a path the other side added.
    let temp = diverged(
        &[("a.txt", TEXT)],
        &[("a.txt", "-"), ("b.txt", TEXT)],
        &[("b.txt", "new on topic\n")],
        &[],
    );
    let (outcome, _) = merge_both(temp.path());
    assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
}

#[test]
fn rename_conflicts_can_be_aborted_like_git() {
    let temp = diverged(
        &[("a.txt", TEXT), ("keep.txt", "keep\n")],
        &[("a.txt", "-"), ("b.txt", TEXT)],
        &[("a.txt", "-"), ("c.txt", TEXT)],
        &[],
    );
    let (_, ours) = merge_both(temp.path());
    let theirs = twin(temp.path());
    let _ = git_output(theirs.path(), &["merge", "-q", "--no-edit", "topic"]);
    git(theirs.path(), &["merge", "--abort"]);
    Repository::open(ours.path())
        .unwrap()
        .abort_merge()
        .unwrap();
    assert_eq!(git_ls_files(ours.path()), git_ls_files(theirs.path()));
    assert_eq!(worktree_files(ours.path()), worktree_files(theirs.path()));
    assert_eq!(git(ours.path(), &["status", "--porcelain"]), "");
}
