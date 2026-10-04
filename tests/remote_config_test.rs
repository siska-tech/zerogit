//! Remote configuration and upstreams compared with Git (#32).

mod common;

use common::*;
use zerogit::{Error, Refspec, Repository};

#[test]
fn add_remote_matches_git_remote_add() {
    let ours = repository(&["a.txt"]);
    let theirs = repository(&["a.txt"]);
    let repo = Repository::open(ours.path()).unwrap();
    let remote = repo
        .add_remote("origin", "https://example.com/repo.git")
        .unwrap();
    git(
        theirs.path(),
        &["remote", "add", "origin", "https://example.com/repo.git"],
    );

    for dir in [ours.path(), theirs.path()] {
        assert_eq!(
            git(dir, &["remote", "-v"]),
            "origin\thttps://example.com/repo.git (fetch)\norigin\thttps://example.com/repo.git (push)\n"
        );
    }
    assert_eq!(
        git(ours.path(), &["config", "--get-all", "remote.origin.fetch"]),
        git(
            theirs.path(),
            &["config", "--get-all", "remote.origin.fetch"]
        )
    );
    assert_eq!(remote.url(), Some("https://example.com/repo.git"));
    assert_eq!(
        remote.tracking_ref("refs/heads/main").as_deref(),
        Some("refs/remotes/origin/main")
    );
    assert!(matches!(
        repo.add_remote("origin", "x"),
        Err(Error::RemoteAlreadyExists(_))
    ));
    assert!(matches!(
        repo.add_remote("bad name", "x"),
        Err(Error::InvalidRefName(_))
    ));
}

#[test]
fn reads_remotes_configured_by_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["remote", "add", "origin", "/srv/repo.git"]);
    git(
        dir,
        &["remote", "add", "upstream", "git@example.com:org/repo.git"],
    );
    git(
        dir,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/tags/*:refs/tags/*",
        ],
    );
    git(
        dir,
        &["config", "--add", "remote.origin.fetch", "^refs/heads/skip"],
    );
    git(
        dir,
        &[
            "config",
            "remote.upstream.pushurl",
            "ssh://push.example.com/repo.git",
        ],
    );

    let repo = Repository::open(dir).unwrap();
    let names: Vec<String> = repo
        .remotes()
        .unwrap()
        .iter()
        .map(|r| r.name().to_owned())
        .collect();
    assert_eq!(names, ["origin", "upstream"]);
    let origin = repo.remote("origin").unwrap();
    assert_eq!(
        origin
            .fetch_refspecs()
            .iter()
            .map(Refspec::to_string)
            .collect::<Vec<_>>(),
        [
            "+refs/heads/*:refs/remotes/origin/*",
            "+refs/tags/*:refs/tags/*",
            "^refs/heads/skip"
        ]
    );
    assert_eq!(origin.tracking_ref("refs/heads/skip"), None);
    assert_eq!(
        repo.remote("upstream").unwrap().push_url(),
        Some("ssh://push.example.com/repo.git")
    );
    assert!(matches!(repo.remote("none"), Err(Error::RemoteNotFound(_))));
}

#[test]
fn remove_remote_matches_git() {
    let setup = |dir: &std::path::Path| {
        git(dir, &["remote", "add", "origin", "/srv/repo.git"]);
        git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(dir, &["update-ref", "refs/remotes/origin/topic/x", "HEAD"]);
        git(dir, &["update-ref", "refs/remotes/other/main", "HEAD"]);
        git(dir, &["config", "branch.main.remote", "origin"]);
        git(dir, &["config", "branch.main.merge", "refs/heads/main"]);
        git(dir, &["pack-refs", "--all"]);
        git(dir, &["update-ref", "refs/remotes/origin/loose", "HEAD"]);
    };
    let ours = repository(&["a.txt"]);
    let theirs = repository(&["a.txt"]);
    setup(ours.path());
    setup(theirs.path());
    Repository::open(ours.path())
        .unwrap()
        .remove_remote("origin")
        .unwrap();
    git(theirs.path(), &["remote", "remove", "origin"]);
    for args in [
        &["for-each-ref", "--format=%(refname)"][..],
        &["config", "--list", "--local"][..],
    ] {
        assert_eq!(
            git(ours.path(), args),
            git(theirs.path(), args),
            "{:?}",
            args
        );
    }
    assert!(!ours.path().join(".git/refs/remotes/origin").exists());
}

#[test]
fn upstream_round_trips_with_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["remote", "add", "origin", "/srv/repo.git"]);
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let repo = Repository::open(dir).unwrap();
    assert_eq!(repo.branch_upstream("main").unwrap(), None);
    repo.set_branch_upstream("main", Some(("origin", "refs/heads/main")))
        .unwrap();
    assert_eq!(
        git(dir, &["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main\n"
    );
    git(
        dir,
        &["branch", "-q", "--set-upstream-to=origin/main", "main"],
    );
    assert_eq!(
        repo.branch_upstream("main").unwrap(),
        Some(("origin".to_owned(), "refs/heads/main".to_owned()))
    );
    repo.set_branch_upstream("main", None).unwrap();
    assert!(!git_output(dir, &["rev-parse", "main@{upstream}"])
        .status
        .success());
}
