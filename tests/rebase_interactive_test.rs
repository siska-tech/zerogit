//! `rebase_interactive` runs a todo list like `git rebase -i` (#34): pick,
//! reword, edit, squash, fixup and drop, reordering and leaving out
//! commits, stopping on conflicts and edits, and continuing from either
//! tool.
//!
//! Git is used only to build fixtures and to produce expected values: its
//! sequence editor copies the same todo list in place.

mod common;

use common::*;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use zerogit::{Error, Oid, RebaseOutcome, RebaseStep, Repository};

/// A base commit, then four commits each changing its own file (the
/// fourth also changes the third's file, so it depends on it).
fn history() -> (TempDir, Vec<Oid>) {
    let temp = empty_repository();
    let dir = temp.path();
    write(dir, "base.txt", "base\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Base"]);
    git(dir, &["branch", "base"]);
    let mut commits = Vec::new();
    for (i, (file, content)) in [
        ("one.txt", "one\n"),
        ("two.txt", "two\n"),
        ("three.txt", "three\n"),
        ("three.txt", "three, changed by four\n"),
    ]
    .iter()
    .enumerate()
    {
        write(dir, file, content);
        git(dir, &["add", "-A"]);
        git(
            dir,
            &[
                "commit",
                "-q",
                "-m",
                &format!("Commit {}\n\nBody of {}.", i + 1, i + 1),
            ],
        );
        commits.push(Oid::from_hex(git(dir, &["rev-parse", "HEAD"]).trim()).unwrap());
    }
    (temp, commits)
}

/// The todo list Git reads for `steps`.
fn todo_text(steps: &[RebaseStep]) -> String {
    steps
        .iter()
        .map(|step| {
            let command = match step {
                RebaseStep::Pick(_) => "pick",
                RebaseStep::Reword(..) => "reword",
                RebaseStep::Edit(_) => "edit",
                RebaseStep::Squash(_) => "squash",
                RebaseStep::Fixup(_) => "fixup",
                RebaseStep::Drop(_) => "drop",
            };
            format!("{} {}\n", command, step.commit().to_hex())
        })
        .collect()
}

fn shell_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Runs Git with an editor that keeps what it is given (or writes
/// `message`) and a sequence editor that writes `steps`.
fn git_with_editors(
    dir: &Path,
    args: &[&str],
    steps: &[RebaseStep],
    message: Option<&str>,
) -> bool {
    let scratch = dir.join(".git").join("test-editors");
    std::fs::create_dir_all(&scratch).unwrap();
    let todo = scratch.join("todo");
    std::fs::write(&todo, todo_text(steps)).unwrap();
    let editor = match message {
        Some(message) => {
            let file = scratch.join("message");
            std::fs::write(&file, message).unwrap();
            format!("cp '{}'", shell_path(&file))
        }
        None => "true".to_owned(),
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_SEQUENCE_EDITOR", format!("cp '{}'", shell_path(&todo)))
        .env("GIT_EDITOR", editor)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
    output.status.success()
}

/// Commits since `base` (tree, author, message), the index, the work
/// tree, the status and whether a rebase is still in progress.
fn state(dir: &Path) -> Vec<String> {
    vec![
        git(
            dir,
            &[
                "log",
                "--reverse",
                "--format=%T %an %ad%n%B--",
                "base..HEAD",
            ],
        ),
        format!("{:?}", git_ls_files(dir)),
        format!("{:?}", worktree_files(dir)),
        git(dir, &["status", "--porcelain"]),
        format!("{}", dir.join(".git/rebase-merge").is_dir()),
    ]
}

/// Runs `steps` with zerogit and Git on twins of `source`, compares, and
/// returns zerogit's outcome with both twins.
fn rebase_both(source: &Path, steps: &[RebaseStep]) -> (RebaseOutcome, TempDir, TempDir) {
    let ours = twin(source);
    let theirs = twin(source);
    git_with_editors(theirs.path(), &["rebase", "-q", "-i", "base"], steps, None);
    let outcome = Repository::open(ours.path())
        .unwrap()
        .rebase_interactive("base", None, steps, "Test", "test@example.com")
        .unwrap();
    assert_eq!(state(ours.path()), state(theirs.path()), "{:?}", steps);
    (outcome, ours, theirs)
}

#[test]
fn plan_lists_the_commits_like_git() {
    let (temp, commits) = history();
    let steps = Repository::open(temp.path())
        .unwrap()
        .rebase_plan("base")
        .unwrap();
    assert_eq!(
        steps,
        commits
            .iter()
            .map(|c| RebaseStep::Pick(*c))
            .collect::<Vec<_>>()
    );
}

#[test]
fn reorder_drop_squash_and_fixup_match_git() {
    let (temp, c) = history();
    let cases = [
        // Reordered, with a commit dropped and another left out.
        vec![
            RebaseStep::Pick(c[1]),
            RebaseStep::Pick(c[0]),
            RebaseStep::Drop(c[2]),
        ],
        // Squash and fixup.
        vec![
            RebaseStep::Pick(c[0]),
            RebaseStep::Squash(c[1]),
            RebaseStep::Pick(c[2]),
            RebaseStep::Fixup(c[3]),
        ],
        // A chain: fixup, then squash into the same commit.
        vec![
            RebaseStep::Pick(c[0]),
            RebaseStep::Fixup(c[1]),
            RebaseStep::Squash(c[2]),
            RebaseStep::Pick(c[3]),
        ],
    ];
    for steps in cases {
        let (outcome, _, _) = rebase_both(temp.path(), &steps);
        assert!(
            matches!(outcome, RebaseOutcome::Completed(_)),
            "{:?}",
            outcome
        );
    }
}

#[test]
fn reword_matches_git() {
    let (temp, c) = history();
    let message = "New subject\n\nA new body,\nover two lines.\n";
    let steps = [
        RebaseStep::Pick(c[0]),
        RebaseStep::Reword(c[1], message.to_owned()),
    ];
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    assert!(git_with_editors(
        theirs.path(),
        &["rebase", "-q", "-i", "base"],
        &steps,
        Some(message)
    ));
    Repository::open(ours.path())
        .unwrap()
        .rebase_interactive("base", None, &steps, "Test", "test@example.com")
        .unwrap();
    assert_eq!(state(ours.path()), state(theirs.path()));
}

#[test]
fn edit_stops_and_continues_with_staged_changes_like_git() {
    let (temp, c) = history();
    let steps = [RebaseStep::Edit(c[0]), RebaseStep::Pick(c[1])];
    let (outcome, ours, theirs) = rebase_both(temp.path(), &steps);
    assert_eq!(outcome, RebaseOutcome::Stopped { commit: c[0] });

    // Add a file to the stopped commit and continue.
    for dir in [ours.path(), theirs.path()] {
        write(dir, "added.txt", "added while editing\n");
        git(dir, &["add", "added.txt"]);
    }
    git_with_editors(theirs.path(), &["rebase", "--continue"], &[], None);
    let outcome = Repository::open(ours.path())
        .unwrap()
        .rebase_continue("Test", "test@example.com")
        .unwrap();
    assert!(matches!(outcome, RebaseOutcome::Completed(_)));
    assert_eq!(state(ours.path()), state(theirs.path()));
}

#[test]
fn conflicts_stop_and_git_or_zerogit_continue() {
    let (temp, c) = history();
    // The fourth commit changes the third's file; without the third first,
    // it conflicts (add/add of three.txt).
    let steps = [RebaseStep::Pick(c[3]), RebaseStep::Squash(c[0])];
    let (outcome, ours, theirs) = rebase_both(temp.path(), &steps);
    assert!(
        matches!(outcome, RebaseOutcome::Conflicts { .. }),
        "{:?}",
        outcome
    );

    // Resolve the same way in both; zerogit continues one, Git the other.
    let git_side = twin(ours.path());
    for dir in [ours.path(), theirs.path(), git_side.path()] {
        write(dir, "three.txt", "resolved\n");
        git(dir, &["add", "three.txt"]);
    }
    git_with_editors(theirs.path(), &["rebase", "--continue"], &[], None);
    git_with_editors(git_side.path(), &["rebase", "--continue"], &[], None);
    Repository::open(ours.path())
        .unwrap()
        .rebase_continue("Test", "test@example.com")
        .unwrap();
    assert_eq!(state(ours.path()), state(theirs.path()));
    assert_eq!(state(git_side.path()), state(theirs.path()));
}

#[test]
fn a_leading_squash_is_refused() {
    let (temp, c) = history();
    let repo = Repository::open(temp.path()).unwrap();
    for steps in [
        vec![RebaseStep::Squash(c[0])],
        vec![RebaseStep::Drop(c[0]), RebaseStep::Fixup(c[1])],
    ] {
        assert!(matches!(
            repo.rebase_interactive("base", None, &steps, "T", "t@example.com"),
            Err(Error::UnsupportedRebase(_))
        ));
        assert!(!repo.is_rebasing());
    }
}
