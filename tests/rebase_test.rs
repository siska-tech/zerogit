//! `Repository::rebase` compared with `git rebase` (#31).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Error, RebaseOutcome, Repository};

/// main and topic diverge; topic has three commits.
fn diverged(conflict: bool) -> tempfile::TempDir {
    let temp = repository(&["f.txt", "shared.txt"]);
    let dir = temp.path();
    write(dir, "f.txt", "1\n2\n3\n");
    git(dir, &["commit", "-q", "-am", "Base"]);
    git(dir, &["checkout", "-q", "-b", "topic"]);
    write(dir, "a.txt", "topic one\n");
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "Topic one\n\nWith a body."]);
    write(dir, "f.txt", "1\ntopic\n3\n");
    git(dir, &["commit", "-q", "-am", "Topic two"]);
    write(dir, "c.txt", "topic three\n");
    git(dir, &["add", "c.txt"]);
    git(dir, &["commit", "-q", "-m", "Topic three"]);
    git(dir, &["checkout", "-q", "main"]);
    if conflict {
        write(dir, "f.txt", "1\nmain\n3\n");
    } else {
        write(dir, "shared.txt", "main change\n");
    }
    git(dir, &["commit", "-q", "-am", "Main change"]);
    git(dir, &["checkout", "-q", "topic"]);
    temp
}

/// Tree, author, date and message of each commit after `base`, oldest first.
fn history(dir: &Path, base: &str) -> String {
    git(
        dir,
        &[
            "log",
            "--reverse",
            "--format=%T %an <%ae> %ad%n%B--",
            &format!("{}..HEAD", base),
        ],
    )
}

fn rebase_both(
    source: &Path,
    git_args: &[&str],
    upstream: &str,
    onto: Option<&str>,
) -> (tempfile::TempDir, tempfile::TempDir, RebaseOutcome) {
    let ours = twin(source);
    let theirs = twin(source);
    let _ = git_output(theirs.path(), git_args);
    let repo = Repository::open(ours.path()).unwrap();
    let outcome = repo
        .rebase(upstream, onto, "Test", "test@example.com")
        .unwrap();
    (ours, theirs, outcome)
}

fn assert_same(ours: &Path, theirs: &Path, base: &str) {
    assert_eq!(history(ours, base), history(theirs, base), "history");
    assert_eq!(git_ls_files(ours), git_ls_files(theirs), "index");
    assert_eq!(worktree_files(ours), worktree_files(theirs), "work tree");
    assert_eq!(
        git(ours, &["status", "--porcelain"]),
        git(theirs, &["status", "--porcelain"])
    );
}

fn state_file(dir: &Path, name: &str) -> Option<String> {
    let content = fs::read_to_string(dir.join(".git").join(name)).ok()?;
    if name.ends_with("/message") {
        // Git 2.46 and later list the conflicts after the message.
        return Some(match content.find("\n# Conflicts:\n") {
            Some(pos) => content[..=pos].to_owned(),
            None => content,
        });
    }
    if !name.ends_with("done") && !name.ends_with("git-rebase-todo") {
        return Some(content);
    }
    // Git 2.50 and later write `pick <oid> # <subject>`, older Git
    // `pick <oid> <subject>`.
    Some(
        content
            .lines()
            .map(|line| match line.splitn(3, ' ').collect::<Vec<_>>()[..] {
                [command, oid, rest] => format!(
                    "{} {} {}\n",
                    command,
                    oid,
                    rest.strip_prefix("# ").unwrap_or(rest)
                ),
                _ => format!("{}\n", line),
            })
            .collect(),
    )
}

#[test]
fn clean_rebase_matches_git() {
    let temp = diverged(false);
    let (ours, theirs, outcome) = rebase_both(temp.path(), &["rebase", "-q", "main"], "main", None);
    let RebaseOutcome::Completed(new) = outcome else {
        panic!("expected completion, got {:?}", outcome);
    };
    let (o, t) = (ours.path(), theirs.path());
    assert_same(o, t, "main");
    assert_eq!(git(o, &["rev-parse", "topic"]).trim(), new.to_hex());
    assert_eq!(git(o, &["symbolic-ref", "HEAD"]), "refs/heads/topic\n");
    assert_eq!(state_file(o, "ORIG_HEAD"), state_file(t, "ORIG_HEAD"));
    assert!(!o.join(".git/rebase-merge").exists());
    assert_eq!(
        git(o, &["reflog", "-5", "--format=%gs"]),
        git(t, &["reflog", "-5", "--format=%gs"])
    );
    assert_eq!(
        git(o, &["reflog", "-1", "--format=%gs", "topic"]),
        git(t, &["reflog", "-1", "--format=%gs", "topic"])
    );
    assert_fsck(o);
}

#[test]
fn up_to_date_and_refusals() {
    let temp = diverged(false);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let base = git(dir, &["rev-parse", "main~1"]);
    assert_eq!(
        repo.rebase(base.trim(), None, "T", "t@example.com")
            .unwrap(),
        RebaseOutcome::UpToDate
    );
    write(dir, "a.txt", "dirty\n");
    assert!(matches!(
        repo.rebase("main", None, "T", "t@example.com"),
        Err(Error::DirtyWorkingTree)
    ));
    assert!(matches!(
        repo.rebase_continue("T", "t@example.com"),
        Err(Error::NoRebaseInProgress)
    ));
}

#[test]
fn conflict_stops_like_git_and_continues() {
    let temp = diverged(true);
    let (ours, theirs, outcome) = rebase_both(temp.path(), &["rebase", "main"], "main", None);
    let RebaseOutcome::Conflicts { commit, paths } = outcome else {
        panic!("expected conflicts, got {:?}", outcome);
    };
    let (o, t) = (ours.path(), theirs.path());
    assert_eq!(paths, vec![std::path::PathBuf::from("f.txt")]);
    assert_eq!(git(o, &["rev-parse", "topic~1"]).trim(), commit.to_hex());
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_eq!(worktree_files(o), worktree_files(t));
    for name in [
        "rebase-merge/head-name",
        "rebase-merge/onto",
        "rebase-merge/orig-head",
        "rebase-merge/msgnum",
        "rebase-merge/end",
        "rebase-merge/done",
        "rebase-merge/git-rebase-todo",
        "rebase-merge/stopped-sha",
        "rebase-merge/message",
        "rebase-merge/author-script",
        "REBASE_HEAD",
    ] {
        assert_eq!(state_file(o, name), state_file(t, name), "{}", name);
    }
    assert!(fs::read_to_string(o.join(".git/rebase-merge/message"))
        .unwrap()
        .ends_with("\n# Conflicts:\n#\tf.txt\n"));
    assert!(Repository::open(o).unwrap().is_rebasing());

    // Resolve the same way in both and continue with each tool.
    for dir in [o, t] {
        write(dir, "f.txt", "1\nmain\ntopic\n3\n");
        git(dir, &["add", "f.txt"]);
    }
    let repo = Repository::open(o).unwrap();
    assert!(matches!(
        repo.rebase_continue("Test", "test@example.com").unwrap(),
        RebaseOutcome::Completed(_)
    ));
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(t)
        .args(["rebase", "--continue"])
        .env("GIT_EDITOR", "true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_same(o, t, "main");
    assert_eq!(
        git(o, &["reflog", "-6", "--format=%gs"]),
        git(t, &["reflog", "-6", "--format=%gs"])
    );
    assert_fsck(o);
}

#[test]
fn git_continues_and_aborts_a_zerogit_rebase() {
    let temp = diverged(true);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let before = git(dir, &["rev-parse", "topic"]);
    assert!(matches!(
        repo.rebase("main", None, "Test", "test@example.com")
            .unwrap(),
        RebaseOutcome::Conflicts { .. }
    ));
    let aborted = twin(dir);

    write(dir, "f.txt", "resolved\n");
    git(dir, &["add", "f.txt"]);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rebase", "--continue"])
        .env("GIT_EDITOR", "true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(dir, &["log", "--format=%s", "main..topic"]),
        "Topic three\nTopic two\nTopic one\n"
    );

    let dir = aborted.path();
    git(dir, &["rebase", "--abort"]);
    assert_eq!(git(dir, &["rev-parse", "topic"]), before);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[test]
fn zerogit_continues_skips_and_aborts_a_git_rebase() {
    let temp = diverged(true);
    let dir = temp.path();
    let before = git(dir, &["rev-parse", "topic"]);
    assert!(!git_output(dir, &["rebase", "main"]).status.success());
    let skipped = twin(dir);
    let aborted = twin(dir);

    write(dir, "f.txt", "resolved\n");
    git(dir, &["add", "f.txt"]);
    let repo = Repository::open(dir).unwrap();
    assert!(matches!(
        repo.rebase_continue("Test", "test@example.com").unwrap(),
        RebaseOutcome::Completed(_)
    ));
    assert_eq!(
        git(dir, &["log", "--format=%s", "main..topic"]),
        "Topic three\nTopic two\nTopic one\n"
    );

    // Skip drops the conflicting commit, as git rebase --skip does.
    let ours = skipped.path();
    let theirs_temp = twin(ours);
    let theirs = theirs_temp.path();
    Repository::open(ours)
        .unwrap()
        .rebase_skip("Test", "test@example.com")
        .unwrap();
    git(theirs, &["rebase", "--skip"]);
    assert_same(ours, theirs, "main");
    assert_eq!(
        git(ours, &["log", "--format=%s", "main..topic"]),
        "Topic three\nTopic one\n"
    );

    let dir = aborted.path();
    Repository::open(dir).unwrap().rebase_abort().unwrap();
    assert_eq!(git(dir, &["rev-parse", "topic"]), before);
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/topic\n");
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
    assert!(!dir.join(".git/rebase-merge").exists());
}

#[test]
fn onto_with_commit_ids_matches_git() {
    let temp = diverged(false);
    let dir = temp.path();
    git(dir, &["branch", "other", "main~1"]);
    git(dir, &["checkout", "-q", "other"]);
    write(dir, "other.txt", "other\n");
    git(dir, &["add", "other.txt"]);
    git(dir, &["commit", "-q", "-m", "Other"]);
    git(dir, &["checkout", "-q", "topic"]);
    let upstream = git(dir, &["rev-parse", "topic~2"]).trim().to_owned();
    let (ours, theirs, outcome) = rebase_both(
        dir,
        &["rebase", "-q", "--onto", "other", &upstream],
        &upstream,
        Some("other"),
    );
    assert!(matches!(outcome, RebaseOutcome::Completed(_)));
    assert_same(ours.path(), theirs.path(), "other");
    assert_eq!(
        git(ours.path(), &["log", "--format=%s", "other..topic"]),
        "Topic three\nTopic two\n"
    );
}

#[test]
fn applied_changes_and_merges_are_left_out_like_git() {
    let temp = diverged(false);
    let dir = temp.path();
    // main already has topic's first change (as a cherry-pick).
    git(dir, &["checkout", "-q", "main"]);
    git(dir, &["cherry-pick", "topic~2"]);
    git(dir, &["checkout", "-q", "topic"]);
    // A merge commit on topic is dropped.
    git(dir, &["checkout", "-q", "-b", "side", "topic~1"]);
    write(dir, "side.txt", "side\n");
    git(dir, &["add", "side.txt"]);
    git(dir, &["commit", "-q", "-m", "Side"]);
    git(dir, &["checkout", "-q", "topic"]);
    git(dir, &["merge", "-q", "--no-edit", "side"]);

    let (ours, theirs, outcome) = rebase_both(dir, &["rebase", "-q", "main"], "main", None);
    assert!(matches!(outcome, RebaseOutcome::Completed(_)));
    assert_same(ours.path(), theirs.path(), "main");
    assert_eq!(
        git(ours.path(), &["log", "--format=%s", "main..topic"]),
        git(theirs.path(), &["log", "--format=%s", "main..topic"])
    );
}
