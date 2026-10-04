//! Stashes made and applied by zerogit compared with `git stash` (#30).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Repository, StashApplyOutcome, StashOptions};

/// A repository with staged, unstaged, deleted, added and untracked changes.
fn dirty_repository() -> tempfile::TempDir {
    let temp = repository(&["a.txt", "b.txt", "c.txt", "dir/d.txt"]);
    let dir = temp.path();
    write(dir, ".gitignore", "*.log\n");
    git(dir, &["add", ".gitignore"]);
    git(dir, &["commit", "-q", "-m", "Ignore logs\n\nWith a body"]);
    write(dir, "a.txt", "staged\n");
    git(dir, &["add", "a.txt"]);
    write(dir, "a.txt", "staged then edited\n");
    write(dir, "b.txt", "unstaged\n");
    fs::remove_file(dir.join("c.txt")).unwrap();
    write(dir, "added.txt", "added\n");
    git(dir, &["add", "added.txt"]);
    write(dir, "dir/untracked.txt", "untracked\n");
    write(dir, "debug.log", "ignored\n");
    temp
}

fn tree_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", &format!("{}^{{tree}}", rev)])
}

/// The index, the work tree files and `git status` of a repository.
type State = (Vec<String>, Vec<(String, Vec<u8>)>, String);

fn state(dir: &Path) -> State {
    (
        git_ls_files(dir),
        worktree_files(dir),
        git(dir, &["status", "--porcelain", "--untracked-files=all"]),
    )
}

#[test]
fn saved_stash_matches_git_stash() {
    for include_untracked in [false, true] {
        let temp = dirty_repository();
        let ours = twin(temp.path());
        let theirs = twin(temp.path());
        let repo = Repository::open(ours.path()).unwrap();
        let options = StashOptions::new().include_untracked(include_untracked);
        let stash = repo
            .stash_save("Test", "test@example.com", &options)
            .unwrap()
            .unwrap();
        let mut args = vec!["stash", "push", "-q"];
        if include_untracked {
            args.push("--include-untracked");
        }
        git(theirs.path(), &args);

        let (o, t) = (ours.path(), theirs.path());
        assert_eq!(git(o, &["rev-parse", "stash"]).trim(), stash.to_hex());
        for rev in ["stash", "stash^1", "stash^2"] {
            assert_eq!(tree_of(o, rev), tree_of(t, rev), "{}", rev);
        }
        if include_untracked {
            assert_eq!(tree_of(o, "stash^3"), tree_of(t, "stash^3"));
            assert_eq!(
                git(o, &["log", "-1", "--format=%s", "stash^3"]),
                git(t, &["log", "-1", "--format=%s", "stash^3"])
            );
        }
        for rev in ["stash", "stash^2"] {
            assert_eq!(
                git(o, &["log", "-1", "--format=%s", rev]),
                git(t, &["log", "-1", "--format=%s", rev])
            );
        }
        assert_eq!(git(o, &["stash", "list"]), git(t, &["stash", "list"]));
        assert_eq!(state(o), state(t), "after saving");
        git(o, &["stash", "show", "-p"]);
        assert_fsck(o);

        // Git can pop zerogit's stash back.
        git(o, &["stash", "pop", "-q", "--index"]);
        git(t, &["stash", "pop", "-q", "--index"]);
        assert_eq!(state(o), state(t), "after git stash pop");
    }
}

#[test]
fn custom_message_and_nothing_to_stash() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    assert_eq!(
        repo.stash_save("Test", "test@example.com", &StashOptions::new())
            .unwrap(),
        None
    );
    assert!(!dir.join(".git/refs/stash").exists());
    write(dir, "a.txt", "changed\n");
    repo.stash_save(
        "Test",
        "test@example.com",
        &StashOptions::new().message("my work"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        git(dir, &["stash", "list"]),
        "stash@{0}: On main: my work\n"
    );
    let list = repo.stash_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message(), "On main: my work");
}

/// Stashes with Git, then applies with both tools in twins.
fn apply_both(
    prepare: impl Fn(&Path),
    git_args: &[&str],
    restore_index: bool,
    pop: bool,
) -> (tempfile::TempDir, tempfile::TempDir, StashApplyOutcome) {
    let temp = dirty_repository();
    git(temp.path(), &["stash", "push", "-q", "--include-untracked"]);
    prepare(temp.path());
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let _ = git_output(theirs.path(), git_args);
    let repo = Repository::open(ours.path()).unwrap();
    let outcome = if pop {
        repo.stash_pop(0, restore_index).unwrap()
    } else {
        repo.stash_apply(0, restore_index).unwrap()
    };
    (ours, theirs, outcome)
}

#[test]
fn apply_matches_git_stash_apply() {
    let (ours, theirs, outcome) = apply_both(|_| {}, &["stash", "apply", "-q"], false, false);
    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(state(ours.path()), state(theirs.path()));
    assert_eq!(
        git(ours.path(), &["stash", "list"]),
        git(theirs.path(), &["stash", "list"])
    );
}

#[test]
fn apply_with_index_matches_git() {
    let (ours, theirs, outcome) =
        apply_both(|_| {}, &["stash", "apply", "-q", "--index"], true, false);
    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(state(ours.path()), state(theirs.path()));
}

#[test]
fn pop_drops_the_stash_like_git() {
    let (ours, theirs, outcome) = apply_both(|_| {}, &["stash", "pop", "-q"], false, true);
    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(state(ours.path()), state(theirs.path()));
    assert_eq!(git(ours.path(), &["stash", "list"]), "");
    assert!(!ours.path().join(".git/refs/stash").exists());
}

#[test]
fn apply_onto_changed_head_matches_git() {
    let change = |dir: &Path| {
        write(dir, "dir/d.txt", "changed upstream\n");
        git(dir, &["commit", "-q", "-am", "Upstream"]);
    };
    let (ours, theirs, outcome) = apply_both(change, &["stash", "apply", "-q"], false, false);
    assert_eq!(outcome, StashApplyOutcome::Applied);
    assert_eq!(state(ours.path()), state(theirs.path()));
}

#[test]
fn conflicting_pop_keeps_the_stash_like_git() {
    let conflict = |dir: &Path| {
        write(dir, "b.txt", "upstream\n");
        git(dir, &["commit", "-q", "-am", "Upstream"]);
    };
    let (ours, theirs, outcome) = apply_both(conflict, &["stash", "pop"], false, true);
    assert_eq!(
        outcome,
        StashApplyOutcome::Conflicts(vec![std::path::PathBuf::from("b.txt")])
    );
    assert_eq!(state(ours.path()), state(theirs.path()));
    assert_eq!(
        git(ours.path(), &["stash", "list"]),
        git(theirs.path(), &["stash", "list"])
    );
    assert!(fs::read_to_string(ours.path().join("b.txt"))
        .unwrap()
        .contains("<<<<<<< Updated upstream"));
}

#[test]
fn drop_keeps_the_rest_of_the_list_like_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    for i in 0..3 {
        write(dir, "a.txt", &format!("change {}\n", i));
        git(dir, &["stash", "push", "-q", "-m", &format!("stash {}", i)]);
    }
    let ours = twin(dir);
    let theirs = twin(dir);
    let repo = Repository::open(ours.path()).unwrap();
    repo.stash_drop(1).unwrap();
    git(theirs.path(), &["stash", "drop", "-q", "stash@{1}"]);
    assert_eq!(
        git(ours.path(), &["stash", "list"]),
        git(theirs.path(), &["stash", "list"])
    );
    assert_eq!(
        git(ours.path(), &["rev-parse", "stash@{1}"]),
        git(theirs.path(), &["rev-parse", "stash@{1}"])
    );
    repo.stash_drop(0).unwrap();
    repo.stash_drop(0).unwrap();
    assert_eq!(git(ours.path(), &["stash", "list"]), "");
    assert!(repo.stash_drop(0).is_err());
}

#[test]
fn untracked_file_in_the_way_is_refused() {
    let temp = dirty_repository();
    let dir = temp.path();
    git(dir, &["stash", "push", "-q", "--include-untracked"]);
    write(dir, "dir/untracked.txt", "in the way\n");
    let repo = Repository::open(dir).unwrap();
    let before = state(dir);
    assert!(matches!(
        repo.stash_apply(0, false),
        Err(zerogit::Error::LocalChangesWouldBeOverwritten(_))
    ));
    assert_eq!(state(dir), before);
}
