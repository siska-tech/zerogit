//! Regression tests for shared object access and error propagation.

use std::fs;
use tempfile::TempDir;
use zerogit::{Error, Repository};

#[test]
fn corrupt_head_is_an_error_for_status_and_history() {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(temp.path().join("document.txt"), "first\n").unwrap();
    repo.add("document.txt").unwrap();
    let oid = repo
        .create_commit("Initial", "Test", "test@example.com")
        .unwrap();
    let hex = oid.to_hex();
    let path = repo
        .git_dir()
        .join("objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    fs::write(&path, "corrupt").unwrap();
    assert!(matches!(repo.status(), Err(Error::DecompressionFailed)));
    assert!(matches!(repo.log(), Err(Error::DecompressionFailed)));
    assert!(matches!(repo.commit(&hex), Err(Error::DecompressionFailed)));
    let index_before = fs::read(repo.git_dir().join("index")).unwrap();
    assert!(matches!(repo.add_all(), Err(Error::DecompressionFailed)));
    assert!(matches!(
        repo.reset::<&str>(None),
        Err(Error::DecompressionFailed)
    ));
    assert_eq!(
        fs::read(repo.git_dir().join("index")).unwrap(),
        index_before
    );
    fs::remove_file(path).unwrap();
    assert!(matches!(repo.status(), Err(Error::ObjectNotFound(_))));
}

#[test]
fn unborn_head_still_reports_untracked_files() {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(temp.path().join("document.txt"), "first\n").unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), zerogit::FileStatus::Untracked);
}

#[test]
fn malformed_head_is_not_treated_as_unborn() {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(repo.git_dir().join("HEAD"), "invalid").unwrap();
    assert!(matches!(repo.status(), Err(Error::InvalidOid(_))));
}

#[test]
fn missing_head_file_is_not_treated_as_unborn() {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::remove_file(repo.git_dir().join("HEAD")).unwrap();
    assert!(matches!(repo.status(), Err(Error::RefNotFound(_))));
}
