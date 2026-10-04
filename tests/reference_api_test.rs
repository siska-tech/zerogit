//! Public reference APIs used by remote operations, compared with Git.

mod common;

use common::*;
use zerogit::{Error, Oid, Repository};

#[test]
fn update_reference_matches_git_update_ref() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = Oid::from_hex(git(dir, &["rev-parse", "HEAD"]).trim()).unwrap();

    repo.update_reference(
        "refs/remotes/origin/main",
        Some(&head),
        Some(None),
        "fetch: storing head",
    )
    .unwrap();
    assert_eq!(
        git(dir, &["rev-parse", "origin/main"]).trim(),
        head.to_hex()
    );
    assert_eq!(
        git(dir, &["reflog", "-1", "--format=%gs", "origin/main"]),
        "fetch: storing head\n"
    );
    // A stale expectation is refused.
    assert!(matches!(
        repo.update_reference("refs/remotes/origin/main", None, Some(None), "x"),
        Err(Error::StaleReference(_))
    ));
    assert!(matches!(
        repo.update_reference("main", Some(&head), None, "x"),
        Err(Error::InvalidRefName(_))
    ));

    git(dir, &["pack-refs", "--all"]);
    repo.update_reference("refs/remotes/origin/main", None, Some(Some(head)), "delete")
        .unwrap();
    assert!(
        !git_output(dir, &["rev-parse", "--verify", "-q", "origin/main"])
            .status
            .success()
    );
    assert_eq!(
        repo.find_reference("refs/remotes/origin/main").unwrap(),
        None
    );
    assert_eq!(
        repo.references().unwrap(),
        vec![("refs/heads/main".to_owned(), head)]
    );
}

#[test]
fn symbolic_references_peel_and_reset_hard() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    git(dir, &["branch", "other"]);
    repo.set_symbolic_reference("refs/remotes/origin/HEAD", "refs/heads/other", None)
        .unwrap();
    assert_eq!(
        git(dir, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/heads/other\n"
    );
    repo.set_symbolic_reference("HEAD", "refs/heads/other", Some("switch"))
        .unwrap();
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/other\n");
    assert_eq!(git(dir, &["reflog", "-1", "--format=%gs"]), "switch\n");

    git(dir, &["tag", "-a", "-m", "t", "v1"]);
    let tag = Oid::from_hex(git(dir, &["rev-parse", "v1"]).trim()).unwrap();
    let commit = Oid::from_hex(git(dir, &["rev-parse", "v1^{}"]).trim()).unwrap();
    assert_eq!(repo.peel(&tag).unwrap(), commit);
    assert_eq!(
        repo.object_type(&tag).unwrap(),
        zerogit::objects::ObjectType::Tag
    );
    assert!(repo.has_object(&tag).unwrap());
    assert!(repo.is_ancestor(&commit, &commit).unwrap());

    write(dir, "a.txt", "changed\n");
    write(dir, "new.txt", "staged\n");
    git(dir, &["add", "new.txt"]);
    write(dir, "untracked.txt", "kept\n");
    repo.reset_hard().unwrap();
    assert_eq!(git(dir, &["status", "--porcelain"]), "?? untracked.txt\n");
}

#[test]
fn bare_repositories_open_as_bare() {
    let temp = tempfile::TempDir::new().unwrap();
    let bare = temp.path().join("plain");
    git(
        temp.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    let repo = Repository::open(&bare).unwrap();
    assert!(repo.is_bare());
    let named = temp.path().join("named.git");
    git(
        temp.path(),
        &["init", "-q", "--bare", named.to_str().unwrap()],
    );
    assert!(Repository::open(&named).unwrap().is_bare());
    let work = repository(&["a.txt"]);
    assert!(!Repository::open(work.path()).unwrap().is_bare());
    assert!(!Repository::open(work.path().join(".git"))
        .unwrap()
        .is_bare());
}
