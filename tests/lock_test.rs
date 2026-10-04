//! Writers take Git's lock files (`<path>.lock`), so zerogit and Git never
//! overwrite each other's updates of the index or of references.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};

use common::{git, repository, write};
use zerogit::{Error, Oid, Repository};

fn head_state(dir: &Path) -> (Vec<u8>, String) {
    (
        fs::read(dir.join(".git/index")).unwrap(),
        git(dir, &["rev-parse", "HEAD"]),
    )
}

fn assert_locked<T: std::fmt::Debug>(result: zerogit::Result<T>, lock: &Path) {
    match result {
        Err(Error::Locked(path)) => {
            assert_eq!(path.canonicalize().unwrap(), lock.canonicalize().unwrap())
        }
        other => panic!("expected Locked({}), got {:?}", lock.display(), other),
    }
}

#[test]
fn index_lock_blocks_every_index_writer() {
    let temp = repository(&["a.txt", "b.txt"]);
    let dir = temp.path();
    git(dir, &["branch", "other"]);
    write(dir, "a.txt", "changed\n");
    write(dir, "new.txt", "new\n");
    let repo = Repository::open(dir).unwrap();

    let lock = dir.join(".git/index.lock");
    fs::write(&lock, b"held by another process").unwrap();
    let before = head_state(dir);

    assert_locked(repo.add("a.txt"), &lock);
    assert_locked(repo.add_all(), &lock);
    assert_locked(repo.reset(None::<&str>), &lock);
    assert_locked(repo.reset(Some("a.txt")), &lock);
    assert_locked(repo.create_commit("msg", "T", "t@example.com"), &lock);
    assert_locked(repo.checkout("other"), &lock);

    assert_eq!(head_state(dir), before);
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main\n");
    // Another process's lock is never removed.
    assert_eq!(fs::read(&lock).unwrap(), b"held by another process");
}

#[test]
fn branch_lock_blocks_updates_of_that_branch() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a.txt").unwrap();
    let head = repo.head().unwrap().oid().to_owned();

    let lock = dir.join(".git/refs/heads/main.lock");
    fs::write(&lock, b"").unwrap();
    assert_locked(repo.create_commit("msg", "T", "t@example.com"), &lock);
    assert_locked(
        repo.update_reference("refs/heads/main", Some(&head), None, "test"),
        &lock,
    );
    assert_locked(
        repo.update_reference("refs/heads/main", None, None, "test"),
        &lock,
    );
    assert_eq!(repo.head().unwrap().oid(), &head);
    // The index lock of the failed commit is released.
    assert!(!dir.join(".git/index.lock").exists());

    fs::remove_file(&lock).unwrap();
    repo.create_commit("msg", "T", "t@example.com").unwrap();
}

#[test]
fn head_lock_blocks_checkout_before_the_work_tree_changes() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "other"]);
    write(dir, "a.txt", "other\n");
    git(dir, &["commit", "-q", "-am", "Other"]);
    git(dir, &["checkout", "-q", "main"]);
    let repo = Repository::open(dir).unwrap();

    let lock = dir.join(".git/HEAD.lock");
    fs::write(&lock, b"").unwrap();
    assert_locked(repo.checkout("other"), &lock);
    assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"content of a.txt\n");
    assert!(!dir.join(".git/index.lock").exists());
}

#[test]
fn concurrent_compare_and_swap_has_one_winner() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let base = repo.head().unwrap().oid().to_owned();
    // Distinct commits to move the branch to.
    let targets: Vec<Oid> = (0..8)
        .map(|i| {
            let out = git(
                dir,
                &[
                    "commit-tree",
                    "HEAD^{tree}",
                    "-p",
                    "HEAD",
                    "-m",
                    &i.to_string(),
                ],
            );
            Oid::from_hex(out.trim()).unwrap()
        })
        .collect();

    let barrier = Arc::new(Barrier::new(targets.len()));
    let handles: Vec<_> = targets
        .into_iter()
        .map(|target| {
            let barrier = Arc::clone(&barrier);
            let dir = dir.to_path_buf();
            std::thread::spawn(move || {
                let repo = Repository::open(&dir).unwrap();
                barrier.wait();
                repo.update_reference("refs/heads/main", Some(&target), Some(Some(base)), "cas")
                    .map(|()| target)
            })
        })
        .collect();

    let mut winners = Vec::new();
    for handle in handles {
        match handle.join().unwrap() {
            Ok(target) => winners.push(target),
            Err(Error::StaleReference(_) | Error::Locked(_)) => {}
            Err(e) => panic!("unexpected error: {:?}", e),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one update must win");
    assert_eq!(repo.head().unwrap().oid(), &winners[0]);
    assert!(!dir.join(".git/refs/heads/main.lock").exists());
    // One reflog entry for the winner, after the initial commit.
    assert_eq!(repo.reflog("refs/heads/main").unwrap().len(), 2);
}

#[test]
fn failed_operations_leave_no_lock_files() {
    let temp = common::conflicted_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    assert!(matches!(
        repo.create_commit("msg", "T", "t@example.com"),
        Err(Error::UnmergedPaths(_))
    ));
    assert!(matches!(
        repo.add("missing.txt"),
        Err(Error::PathNotFound(_))
    ));
    let head = repo.head().unwrap().oid().to_owned();
    assert!(matches!(
        repo.update_reference("refs/heads/main", Some(&head), Some(None), "x"),
        Err(Error::StaleReference(_))
    ));
    assert!(matches!(
        repo.create_branch("main", None),
        Err(Error::RefAlreadyExists(_))
    ));

    let mut leftovers = Vec::new();
    let mut stack = vec![dir.join(".git")];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else if entry.path().extension().is_some_and(|e| e == "lock") {
                leftovers.push(entry.path());
            }
        }
    }
    assert!(leftovers.is_empty(), "lock files left: {:?}", leftovers);
}

#[test]
fn git_sees_zerogit_updates_as_its_own() {
    // The index and references written through the locks are read by Git.
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "a.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a.txt").unwrap();
    let commit = repo.create_commit("Change", "T", "t@example.com").unwrap();
    repo.create_branch("topic", None).unwrap();

    assert_eq!(git(dir, &["rev-parse", "HEAD"]).trim(), commit.to_hex());
    assert_eq!(git(dir, &["rev-parse", "topic"]).trim(), commit.to_hex());
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
    common::assert_fsck(dir);
}
