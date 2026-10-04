//! `reset_to`, `reset_paths` and `amend_commit` leave the repository as
//! `git reset` and `git commit --amend` do.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;
use std::process::Command;

use common::{conflicted_repository, git, git_output, twin, worktree_files, write};
use zerogit::{CommitOptions, Error, Repository, ResetMode, Signature};

/// Three commits on main, then a staged change, an unstaged change and an
/// untracked file.
fn busy_repository() -> tempfile::TempDir {
    let temp = common::repository(&["a.txt", "b.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "a two\n");
    write(dir, "dir/c.txt", "c\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Second"]);
    write(dir, "b.txt", "b three\n");
    std::fs::remove_file(dir.join("dir/c.txt")).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Third"]);
    write(dir, "a.txt", "staged\n");
    git(dir, &["add", "a.txt"]);
    write(dir, "b.txt", "unstaged\n");
    write(dir, "untracked.txt", "untracked\n");
    temp
}

/// Everything `git reset` changes, for comparing two repositories.
fn state(dir: &Path) -> Vec<String> {
    let reflog = |name: &str| git(dir, &["reflog", "-1", "--format=%gs", name]);
    vec![
        git(dir, &["rev-parse", "HEAD"]),
        String::from_utf8(git_output(dir, &["symbolic-ref", "-q", "HEAD"]).stdout).unwrap(),
        git(dir, &["ls-files", "-s"]),
        git(dir, &["status", "--porcelain", "-uall"]),
        String::from_utf8(git_output(dir, &["rev-parse", "-q", "--verify", "ORIG_HEAD"]).stdout)
            .unwrap(),
        reflog("HEAD"),
        String::from_utf8(git_output(dir, &["reflog", "-1", "--format=%gs", "main"]).stdout)
            .unwrap(),
        format!("{:?}", worktree_files(dir)),
        format!("{}", dir.join(".git/MERGE_HEAD").exists()),
    ]
}

fn assert_same_reset(source: &Path, revision: &str, mode: ResetMode) {
    let ours = twin(source);
    let theirs = twin(source);
    Repository::open(ours.path())
        .unwrap()
        .reset_to(revision, mode)
        .unwrap();
    let flag = match mode {
        ResetMode::Soft => "--soft",
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    git(theirs.path(), &["reset", "-q", flag, revision]);
    assert_eq!(
        state(ours.path()),
        state(theirs.path()),
        "{} {}",
        flag,
        revision
    );
}

#[test]
fn each_mode_matches_git_reset() {
    let temp = busy_repository();
    for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
        for revision in ["HEAD~2", "HEAD~1", "HEAD", "main~1"] {
            assert_same_reset(temp.path(), revision, mode);
        }
    }
}

#[test]
fn detached_head_moves_itself() {
    let temp = busy_repository();
    git(temp.path(), &["checkout", "-q", "--detach"]);
    for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
        // Moving to the same commit leaves no reflog entry when detached.
        for revision in ["HEAD~1", "HEAD"] {
            assert_same_reset(temp.path(), revision, mode);
        }
    }
}

#[test]
fn mixed_and_hard_resets_end_a_merge() {
    let temp = conflicted_repository();
    assert_same_reset(temp.path(), "HEAD", ResetMode::Mixed);
    assert_same_reset(temp.path(), "HEAD", ResetMode::Hard);

    // Git refuses a soft reset in the middle of a merge; so does zerogit.
    let ours = twin(temp.path());
    let before = state(ours.path());
    assert!(matches!(
        Repository::open(ours.path())
            .unwrap()
            .reset_to("HEAD", ResetMode::Soft),
        Err(Error::MergeInProgress)
    ));
    assert!(!git_output(temp.path(), &["reset", "--soft", "HEAD"])
        .status
        .success());
    assert_eq!(state(ours.path()), before);
}

#[test]
fn failed_resets_change_nothing() {
    let temp = busy_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let before = state(dir);

    assert!(matches!(
        repo.reset_to("nosuch", ResetMode::Hard),
        Err(Error::InvalidRevision { .. })
    ));
    for lock in ["index.lock", "HEAD.lock", "refs/heads/main.lock"] {
        let path = dir.join(".git").join(lock);
        std::fs::write(&path, b"").unwrap();
        for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
            match repo.reset_to("HEAD~2", mode) {
                Err(Error::Locked(_)) => {}
                // A soft reset does not touch the index.
                Ok(_) if lock == "index.lock" && mode == ResetMode::Soft => {
                    repo.reset_to("main@{1}", ResetMode::Soft).unwrap();
                    continue;
                }
                other => panic!("{} {:?}: {:?}", lock, mode, other),
            }
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(state(dir)[2..4], before[2..4], "{}", lock);
        assert_eq!(state(dir)[7], before[7], "{}", lock);
    }
}

#[test]
fn path_reset_matches_git() {
    let temp = busy_repository();
    for paths in [&["a.txt"][..], &["dir"], &["a.txt", "b.txt"], &["."]] {
        let ours = twin(temp.path());
        let theirs = twin(temp.path());
        Repository::open(ours.path())
            .unwrap()
            .reset_paths("HEAD~1", paths)
            .unwrap();
        let mut args = vec!["reset", "-q", "HEAD~1", "--"];
        args.extend_from_slice(paths);
        // `git reset` exits with 1 when changes stay unstaged.
        git_output(theirs.path(), &args);
        assert_eq!(state(ours.path()), state(theirs.path()), "{:?}", paths);
    }
}

/// Runs `git commit --amend` with the committer zerogit is given.
fn git_amend(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-q", "--amend"])
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_COMMITTER_NAME", "Bob")
        .env("GIT_COMMITTER_EMAIL", "bob@example.com")
        .env("GIT_COMMITTER_DATE", "1700000100 -0130")
        .output()
        .unwrap()
}

fn committer() -> CommitOptions {
    CommitOptions::new().committer(Signature::new("Bob", "bob@example.com", 1700000100, -90))
}

#[test]
fn amend_matches_git_commit_amend() {
    let temp = busy_repository();
    for message in [None, Some("  Better subject  \n\n\nBody\n")] {
        let ours = twin(temp.path());
        let theirs = twin(temp.path());
        let oid = Repository::open(ours.path())
            .unwrap()
            .amend_commit(message, &committer())
            .unwrap();
        let output = match message {
            Some(message) => git_amend(theirs.path(), &["-m", message]),
            None => git_amend(theirs.path(), &["--no-edit"]),
        };
        assert!(output.status.success());
        assert_eq!(
            oid.to_hex(),
            git(theirs.path(), &["rev-parse", "HEAD"]).trim(),
            "{:?}",
            message
        );
        assert_eq!(state(ours.path()), state(theirs.path()));
        assert_eq!(
            git(ours.path(), &["reflog", "-1", "--format=%gs"]),
            "commit (amend): Third\n".replace(
                "Third",
                if message.is_some() {
                    "Better subject"
                } else {
                    "Third"
                }
            )
        );
    }
}

#[test]
fn amend_can_change_the_author_and_keeps_merge_parents() {
    let temp = conflicted_repository();
    let dir = temp.path();
    write(dir, "file.txt", "resolved\n");
    git(dir, &["add", "file.txt"]);
    git(dir, &["commit", "-q", "--no-edit"]);
    let parents = git(dir, &["rev-parse", "HEAD^1", "HEAD^2"]);

    let repo = Repository::open(dir).unwrap();
    let alice = Signature::new("Alice", "alice@example.com", 1700000000, 540);
    // A merge commit can be amended even without changes.
    repo.amend_commit(Some("Merge, amended"), &committer().author(alice))
        .unwrap();
    assert_eq!(git(dir, &["rev-parse", "HEAD^1", "HEAD^2"]), parents);
    assert_eq!(
        git(dir, &["log", "-1", "--format=%an <%ae> %ad", "--date=raw"]),
        "Alice <alice@example.com> 1700000000 +0900\n"
    );
    common::assert_fsck(dir);
}

#[test]
fn amend_refuses_what_git_refuses() {
    // Amending to the parent's tree would make the commit empty.
    let temp = busy_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    repo.reset_paths("HEAD~1", &["."]).unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);
    assert!(matches!(
        repo.amend_commit(None, &committer()),
        Err(Error::EmptyCommit)
    ));
    assert!(!git_amend(twin(dir).path(), &["--no-edit"]).status.success());
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    repo.amend_commit(None, &committer().allow_empty(true))
        .unwrap();
    assert_eq!(
        git(dir, &["rev-parse", "HEAD^{tree}"]),
        git(dir, &["rev-parse", "HEAD^^{tree}"])
    );

    // Nothing to amend in an empty repository, nor during a merge.
    let empty = common::empty_repository();
    assert!(matches!(
        Repository::open(empty.path())
            .unwrap()
            .amend_commit(None, &committer()),
        Err(Error::RefNotFound(_))
    ));
    let merging = conflicted_repository();
    write(merging.path(), "file.txt", "resolved\n");
    git(merging.path(), &["add", "file.txt"]);
    assert!(matches!(
        Repository::open(merging.path())
            .unwrap()
            .amend_commit(None, &committer()),
        Err(Error::MergeInProgress)
    ));
}
