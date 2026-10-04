//! Commits made by zerogit match Git's: local time zone, message clean-up,
//! separate author and committer, and the same OID for the same input.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;
use std::process::Command;

use common::{git, repository, twin, write};
use zerogit::{CommitOptions, Error, Repository, Signature};

/// Runs `git commit` with fixed identities and dates.
fn git_commit_at(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("commit")
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Alice")
        .env("GIT_AUTHOR_EMAIL", "alice@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0900")
        .env("GIT_COMMITTER_NAME", "Bob")
        .env("GIT_COMMITTER_EMAIL", "bob@example.com")
        .env("GIT_COMMITTER_DATE", "1700000100 -0130")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixed_options() -> CommitOptions {
    CommitOptions::new()
        .author(Signature::new(
            "Alice",
            "alice@example.com",
            1700000000,
            540,
        ))
        .committer(Signature::new("Bob", "bob@example.com", 1700000100, -90))
}

#[test]
fn time_zone_matches_git_commit() {
    let temp = repository(&["a.txt"]);
    let ours = temp.path();
    let theirs = twin(ours);
    write(ours, "a.txt", "changed\n");
    write(theirs.path(), "a.txt", "changed\n");

    let repo = Repository::open(ours).unwrap();
    repo.add("a.txt").unwrap();
    repo.create_commit("Change", "T", "t@example.com").unwrap();
    git(theirs.path(), &["commit", "-q", "-am", "Change"]);

    // `git log --format=%ad --date=format:%z` prints the recorded offset.
    let zone = |dir: &Path| git(dir, &["log", "-1", "--format=%ad %cd", "--date=format:%z"]);
    assert_eq!(zone(ours), zone(theirs.path()));
}

#[test]
fn same_input_gives_the_same_oid_as_git_commit() {
    let temp = repository(&["a.txt"]);
    let ours = temp.path();
    let theirs = twin(ours);
    for dir in [ours, theirs.path()] {
        write(dir, "a.txt", "changed\n");
        write(dir, "new.txt", "new\n");
    }
    let message = "\n\n  Subject line  \n\n\n\nBody\t \n# not a comment for -m\n\n";

    let repo = Repository::open(ours).unwrap();
    repo.add_all().unwrap();
    let oid = repo.create_commit_with(message, &fixed_options()).unwrap();
    git(theirs.path(), &["add", "-A"]);
    git_commit_at(theirs.path(), &["-q", "-m", message]);

    assert_eq!(
        oid.to_hex(),
        git(theirs.path(), &["rev-parse", "HEAD"]).trim()
    );
    common::assert_fsck(ours);
}

#[test]
fn author_and_committer_are_recorded_separately() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a.txt").unwrap();
    repo.create_commit_with("Change", &fixed_options()).unwrap();

    let raw = git(dir, &["cat-file", "-p", "HEAD"]);
    assert!(
        raw.contains("\nauthor Alice <alice@example.com> 1700000000 +0900\n"),
        "{}",
        raw
    );
    assert!(
        raw.contains("\ncommitter Bob <bob@example.com> 1700000100 -0130\n"),
        "{}",
        raw
    );
    assert!(raw.ends_with("\n\nChange\n"), "{:?}", raw);
    // The reflog records the committer.
    let entry = &repo.reflog("HEAD").unwrap()[0];
    assert_eq!(entry.committer().name(), "Bob");
}

#[test]
fn create_commit_ends_the_message_with_a_newline() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a.txt").unwrap();
    repo.create_commit("Subject\n\n\nBody  ", "T", "t@example.com")
        .unwrap();
    assert_eq!(
        git(dir, &["log", "-1", "--format=%B"]),
        "Subject\n\nBody\n\n"
    );
}

#[test]
fn empty_commits_need_allow_empty() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = repo.head().unwrap().oid().to_owned();

    assert!(matches!(
        repo.create_commit_with("Nothing", &fixed_options()),
        Err(Error::EmptyCommit)
    ));
    assert_eq!(repo.head().unwrap().oid(), &head);

    let oid = repo
        .create_commit_with("Nothing", &fixed_options().allow_empty(true))
        .unwrap();
    assert_eq!(git(dir, &["rev-parse", "HEAD^"]).trim(), head.to_hex());
    assert_eq!(
        git(dir, &["rev-parse", "HEAD^{tree}"]),
        git(dir, &["rev-parse", "HEAD^^{tree}"])
    );
    assert_eq!(git(dir, &["rev-parse", "HEAD"]).trim(), oid.to_hex());
}

#[test]
fn empty_messages_need_allow_empty_message() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a.txt").unwrap();
    assert!(matches!(
        repo.create_commit_with(" \n\n", &fixed_options()),
        Err(Error::EmptyCommitMessage)
    ));
    repo.create_commit_with("", &fixed_options().allow_empty_message(true))
        .unwrap();
    assert_eq!(git(dir, &["log", "-1", "--format=%B"]), "\n");
    common::assert_fsck(dir);
}

#[test]
fn default_identity_comes_from_the_repository_configuration() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["config", "user.name", "Configured"]);
    git(dir, &["config", "user.email", "configured@example.com"]);
    git(dir, &["config", "committer.name", "Committer"]);
    let repo = Repository::open(dir).unwrap();

    // The process environment may set GIT_AUTHOR_* (as CI or Git hooks
    // do); only check what it leaves to the configuration.
    if std::env::var_os("GIT_AUTHOR_NAME").is_none() {
        assert_eq!(repo.default_author().unwrap().name(), "Configured");
    }
    if std::env::var_os("GIT_COMMITTER_NAME").is_none() {
        assert_eq!(repo.default_committer().unwrap().name(), "Committer");
    }
    if std::env::var_os("GIT_COMMITTER_EMAIL").is_none() {
        assert_eq!(
            repo.default_committer().unwrap().email(),
            "configured@example.com"
        );
    }
}

#[test]
fn annotated_tags_record_the_local_time_zone() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    repo.create_annotated_tag("v1", None, "Release", "T", "t@example.com")
        .unwrap();
    git(dir, &["tag", "-a", "v2", "-m", "Release"]);
    let zone = |name: &str| {
        git(
            dir,
            &["for-each-ref", "--format=%(taggerdate:format:%z)", name],
        )
    };
    assert_eq!(zone("refs/tags/v1"), zone("refs/tags/v2"));
}
