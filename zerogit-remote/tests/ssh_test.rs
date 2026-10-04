//! clone, fetch and push over SSH against a real `sshd`, compared with Git
//! (#36).
//!
//! These tests need an SSH server and are ignored by default. The `ssh` job
//! of the CI starts `sshd` on localhost and runs them with `--ignored`;
//! locally, set up the same and run
//! `cargo test --test ssh_test -- --ignored`. They read:
//!
//! - `ZEROGIT_SSH_HOST`: a host alias from `~/.ssh/config` that logs in to
//!   this machine with a key and a known host key (no prompts).
//! - `ZEROGIT_SSH_USER`, `ZEROGIT_SSH_PORT`: the same server as
//!   `ssh://<user>@localhost:<port>/...`.
//!
//! Git uses the same `~/.ssh/config`, so both run the same SSH client.

#![cfg(unix)]

mod common;

use common::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zerogit::Repository;
use zerogit_remote::{clone, fetch, push, CloneOptions, Error, PushOptions};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "{} is not set: these tests need an SSH server (see tests/ssh_test.rs)",
            name
        )
    })
}

/// The SSH URLs of a repository on this machine, in each form Git accepts.
fn urls(path: &Path) -> Vec<String> {
    let host = var("ZEROGIT_SSH_HOST");
    let path = path.to_str().unwrap();
    vec![
        format!("ssh://{}{}", host, path),
        format!("{}:{}", host, path),
        format!(
            "ssh://{}@localhost:{}{}",
            var("ZEROGIT_SSH_USER"),
            var("ZEROGIT_SSH_PORT"),
            path
        ),
    ]
}

fn assert_same_clone(ours: &Path, theirs: &Path) {
    assert_eq!(refs(ours), refs(theirs));
    assert_eq!(worktree_files(ours), worktree_files(theirs));
    assert_eq!(
        git(ours, &["config", "remote.origin.url"]),
        git(theirs, &["config", "remote.origin.url"])
    );
    git(ours, &["fsck", "--strict"]);
}

#[test]
#[ignore = "needs an SSH server; see the module documentation"]
fn clone_matches_git_for_each_url_form() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    for (i, url) in urls(&bare).into_iter().enumerate() {
        let ours = root.join(format!("ours{}", i));
        let theirs = root.join(format!("theirs{}", i));
        let repo =
            clone(&url, &ours, &CloneOptions::new()).unwrap_or_else(|e| panic!("{}: {}", url, e));
        git(root, &["clone", "-q", &url, theirs.to_str().unwrap()]);
        assert_same_clone(&ours, &theirs);
        assert!(repo.status().unwrap().is_empty());
    }
}

#[test]
#[ignore = "needs an SSH server; see the module documentation"]
fn paths_with_spaces_and_quotes_are_quoted() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let odd: PathBuf = root.join("dir with space").join("it's here.git");
    std::fs::create_dir_all(odd.parent().unwrap()).unwrap();
    std::fs::rename(&bare, &odd).unwrap();
    let url = urls(&odd).remove(0);
    let ours = root.join("ours");
    let theirs = root.join("theirs");
    clone(&url, &ours, &CloneOptions::new()).unwrap();
    git(root, &["clone", "-q", &url, theirs.to_str().unwrap()]);
    assert_same_clone(&ours, &theirs);
}

#[test]
#[ignore = "needs an SSH server; see the module documentation"]
fn fetch_and_push_match_git() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let url = urls(&bare).remove(1);
    let ours = root.join("ours");
    let theirs = root.join("theirs");
    let repo = clone(&url, &ours, &CloneOptions::new()).unwrap();
    git(root, &["clone", "-q", &url, theirs.to_str().unwrap()]);

    // New history on the server, fetched by both.
    let work = root.join("work");
    write(&work, "later.txt", "later\n");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Later"]);
    git(&work, &["tag", "v2"]);
    git(&work, &["push", "-q", bare.to_str().unwrap(), "main", "v2"]);
    fetch(&repo, "origin").unwrap();
    git(&theirs, &["fetch", "-q", "origin"]);
    assert_eq!(refs(&ours), refs(&theirs));

    // A push from zerogit is what Git then sees on the server.
    git(&ours, &["merge", "-q", "--ff-only", "origin/main"]);
    write(&ours, "mine.txt", "mine\n");
    git(&ours, &["add", "-A"]);
    git(&ours, &["commit", "-q", "-m", "Mine"]);
    push(&repo, "origin", &["main"], &PushOptions::new()).unwrap();
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main"]),
        git(&ours, &["rev-parse", "main"])
    );
    assert_eq!(
        git(&ours, &["rev-parse", "refs/remotes/origin/main"]),
        git(&ours, &["rev-parse", "main"])
    );
    git(&bare, &["fsck", "--strict"]);
}

#[test]
#[ignore = "needs an SSH server; see the module documentation"]
fn core_ssh_command_is_used() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let url = urls(&bare).remove(0);
    let ours = root.join("ours");
    let repo = clone(&url, &ours, &CloneOptions::new()).unwrap();

    // A wrapper that records its use, then runs ssh.
    let log = root.join("wrapper.log");
    let wrapper = root.join("ssh wrapper.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec ssh \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A command line with a quoted path, run by the shell as in Git.
    git(
        &ours,
        &[
            "config",
            "core.sshCommand",
            &format!("'{}' -o BatchMode=yes", wrapper.display()),
        ],
    );
    fetch(&repo, "origin").unwrap();
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("-o BatchMode=yes"), "{}", logged);
    assert!(logged.contains("git-upload-pack"), "{}", logged);
}

#[test]
#[ignore = "needs an SSH server; see the module documentation"]
fn failed_logins_report_the_client_messages() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let path = bare.to_str().unwrap();

    // Authentication fails for a user without the key.
    let url = format!(
        "ssh://no-such-user@localhost:{}{}",
        var("ZEROGIT_SSH_PORT"),
        path
    );
    let error = clone(&url, &root.join("a"), &CloneOptions::new()).unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{:?}", error);
    assert!(error.to_string().contains("Permission denied"), "{}", error);

    // The host key does not match the known one.
    let known = root.join("known_hosts");
    let key = root.join("other_key");
    let status = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .unwrap();
    assert!(status.success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let mut fields = public.split_whitespace();
    std::fs::write(
        &known,
        format!("* {} {}\n", fields.next().unwrap(), fields.next().unwrap()),
    )
    .unwrap();
    let work = root.join("b");
    Repository::init(&work).unwrap();
    git(&work, &["remote", "add", "origin", &urls(&bare).remove(0)]);
    git(
        &work,
        &[
            "config",
            "core.sshCommand",
            &format!(
                "ssh -o UserKnownHostsFile='{}' -o StrictHostKeyChecking=yes",
                known.display()
            ),
        ],
    );
    let repo = Repository::open(&work).unwrap();
    let error = fetch(&repo, "origin").unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{:?}", error);
    assert!(
        error.to_string().contains("Host key verification failed"),
        "{}",
        error
    );
}
