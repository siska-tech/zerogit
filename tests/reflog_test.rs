//! Reflogs written by zerogit compared with the ones Git writes (#25).

mod common;

use common::*;
use std::path::Path;
use zerogit::Repository;

/// `%gs` (message) of each entry of a reflog, newest first.
fn messages(dir: &Path, refname: &str) -> Vec<String> {
    git(dir, &["reflog", "show", "--format=%gs", refname])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Old and new OIDs of each entry, newest first, read from the log file.
fn transitions(dir: &Path, refname: &str) -> Vec<(String, String)> {
    let content = std::fs::read_to_string(dir.join(".git/logs").join(refname)).unwrap();
    let mut result: Vec<(String, String)> = content
        .lines()
        .map(|l| (l[..40].to_owned(), l[41..81].to_owned()))
        .collect();
    result.reverse();
    result
}

/// Runs the same sequence of operations with Git and with zerogit.
fn scenario(dir: &Path, use_git: bool) -> String {
    let repo = Repository::open(dir).unwrap();
    let commit = |msg: &str| {
        if use_git {
            git(dir, &["add", "-A"]);
            git(dir, &["commit", "-q", "-m", msg]);
        } else {
            repo.add_all().unwrap();
            repo.create_commit(msg, "Test", "test@example.com").unwrap();
        }
    };
    write(dir, "a.txt", "1\n");
    commit("First commit");
    write(dir, "a.txt", "2\n");
    commit("Second commit\n\nWith a body.");
    let first = git(dir, &["rev-parse", "HEAD~1"]).trim().to_owned();
    if use_git {
        git(dir, &["branch", "topic"]);
        git(dir, &["branch", "old", &first]);
        git(dir, &["checkout", "-q", "topic"]);
    } else {
        repo.create_branch("topic", None).unwrap();
        repo.create_branch("old", Some(zerogit::Oid::from_hex(&first).unwrap()))
            .unwrap();
        repo.checkout("topic").unwrap();
    }
    write(dir, "b.txt", "topic\n");
    commit("Topic work");
    if use_git {
        git(dir, &["checkout", "-q", "main"]);
        git(dir, &["checkout", "-q", &first]);
        git(dir, &["checkout", "-q", "main"]);
        git(dir, &["branch", "-q", "-D", "old"]);
    } else {
        repo.checkout("main").unwrap();
        repo.checkout(&first).unwrap();
        repo.checkout("main").unwrap();
        repo.delete_branch("old").unwrap();
    }
    first
}

#[test]
fn reflogs_match_git() {
    let ours = empty_repository();
    let theirs = empty_repository();
    let first_ours = scenario(ours.path(), false);
    scenario(theirs.path(), true);

    for refname in ["HEAD", "main", "topic"] {
        let mut expected = messages(theirs.path(), refname);
        let actual = messages(ours.path(), refname);
        // Detached checkouts name the commit, which differs between the runs.
        let first_theirs = git(theirs.path(), &["rev-parse", "HEAD~1"]);
        for line in &mut expected {
            *line = line.replace(first_theirs.trim(), &first_ours);
        }
        assert_eq!(actual, expected, "{}", refname);
    }

    // Old and new values chain correctly and match the branch history.
    let dir = ours.path();
    let head = transitions(dir, "HEAD");
    for pair in head.windows(2) {
        assert_eq!(pair[0].0, pair[1].1);
    }
    assert_eq!(head.last().unwrap().0, "0".repeat(40));
    assert_eq!(
        head[0].1,
        git(dir, &["rev-parse", "HEAD"]).trim(),
        "newest HEAD entry"
    );
    let topic = transitions(dir, "refs/heads/topic");
    assert_eq!(topic[0].1, git(dir, &["rev-parse", "topic"]).trim());
    // The deleted branch's log is gone.
    assert!(!dir.join(".git/logs/refs/heads/old").exists());

    // Git accepts the logs.
    assert_eq!(
        git(dir, &["rev-parse", "HEAD@{1}"]).trim(),
        head[1].1,
        "HEAD@{{1}}"
    );
    assert_fsck(dir);
    git(dir, &["reflog", "expire", "--all"]);
    git(
        dir,
        &["reflog", "expire", "--all", "--expire=now", "--dry-run"],
    );
}

#[test]
fn reflog_api_reads_git_logs() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    git(dir, &["commit", "-q", "-am", "Change"]);
    let repo = Repository::open(dir).unwrap();
    let entries = repo.reflog("HEAD").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].message(), "commit: Change");
    assert_eq!(entries[1].message(), "commit (initial): Initial");
    assert_eq!(entries[0].old_oid(), entries[1].new_oid());
    assert_eq!(entries[0].committer().email(), "test@example.com");
    assert_eq!(repo.reflog("main").unwrap(), entries);
    assert!(repo.reflog("missing").unwrap().is_empty());
}

#[test]
fn bare_like_config_disables_logs() {
    let temp = empty_repository();
    let dir = temp.path();
    git(dir, &["config", "core.logAllRefUpdates", "false"]);
    write(dir, "a.txt", "1\n");
    let repo = Repository::open(dir).unwrap();
    repo.add_all().unwrap();
    repo.create_commit("First", "Test", "test@example.com")
        .unwrap();
    assert!(!dir.join(".git/logs/HEAD").exists());
}
