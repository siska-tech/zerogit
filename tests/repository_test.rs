//! Integration tests for Repository module.

use std::fs;
use std::path::Path;
use zerogit::error::Error;
use zerogit::repository::Repository;

/// Path to the simple test fixture
const SIMPLE_FIXTURE: &str = "tests/fixtures/simple";

/// Path to the branches test fixture
const BRANCHES_FIXTURE: &str = "tests/fixtures/branches";

/// Path to the empty test fixture
const EMPTY_FIXTURE: &str = "tests/fixtures/empty";

// RP-001: Repository::open with valid repository returns Ok
#[test]
fn test_rp001_open_valid_repository() {
    let repo = Repository::open(SIMPLE_FIXTURE);
    assert!(repo.is_ok(), "Should open valid repository");
}

// RP-002: Repository::open with .git directory path returns Ok
#[test]
fn test_rp002_open_git_dir_path() {
    let git_dir = Path::new(SIMPLE_FIXTURE).join(".git");
    let repo = Repository::open(&git_dir);
    assert!(repo.is_ok(), "Should open repository via .git path");

    let repo = repo.unwrap();
    assert!(
        repo.git_dir().ends_with(".git"),
        "git_dir should end with .git"
    );
}

// RP-003: Repository::open with invalid path returns NotARepository
#[test]
fn test_rp003_open_invalid_path() {
    // Path that exists but is not a git repository
    let repo = Repository::open("tests/fixtures");
    assert!(
        matches!(repo, Err(Error::NotARepository(_))),
        "Should return NotARepository for non-git directory"
    );

    // Path that doesn't exist
    let repo = Repository::open("/nonexistent/path");
    assert!(
        matches!(repo, Err(Error::NotARepository(_))),
        "Should return NotARepository for nonexistent path"
    );
}

// RP-004: Repository::discover from subdirectory finds root
#[test]
fn test_rp004_discover_from_subdir() {
    // Create a subdirectory in the simple fixture if it doesn't exist
    let subdir = Path::new(SIMPLE_FIXTURE).join("subdir");
    fs::create_dir_all(&subdir).ok();

    let repo = Repository::discover(&subdir);
    assert!(repo.is_ok(), "Should discover repository from subdirectory");

    let repo = repo.unwrap();
    // The path should point to the simple fixture root
    let simple_path = Path::new(SIMPLE_FIXTURE).canonicalize().unwrap();
    assert_eq!(
        repo.path().canonicalize().unwrap(),
        simple_path,
        "Discovered repository should be the simple fixture"
    );

    // Clean up
    fs::remove_dir(&subdir).ok();
}

// RP-005: Repository::discover with no repository returns NotARepository
#[test]
fn test_rp005_discover_no_repository() {
    // Use temp directory that doesn't contain a git repo
    let temp = tempfile::TempDir::new().unwrap();
    let repo = Repository::discover(temp.path());
    assert!(
        matches!(repo, Err(Error::NotARepository(_))),
        "Should return NotARepository when no repo found"
    );
}

// RP-006: Repository::path returns repository root
#[test]
fn test_rp006_path_returns_root() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();
    let expected_path = Path::new(SIMPLE_FIXTURE).canonicalize().unwrap();

    assert_eq!(
        repo.path().canonicalize().unwrap(),
        expected_path,
        "path() should return repository root"
    );

    // path() should not include .git
    assert!(
        !repo.path().ends_with(".git"),
        "path() should not end with .git"
    );
}

// RP-007: Repository::git_dir returns .git path
#[test]
fn test_rp007_git_dir_returns_dot_git() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();

    assert!(
        repo.git_dir().ends_with(".git"),
        "git_dir() should end with .git"
    );

    let expected_git_dir = Path::new(SIMPLE_FIXTURE)
        .join(".git")
        .canonicalize()
        .unwrap();
    assert_eq!(
        repo.git_dir().canonicalize().unwrap(),
        expected_git_dir,
        "git_dir() should return .git directory path"
    );
}

// Additional: Test with branches fixture
#[test]
fn test_open_branches_fixture() {
    let repo = Repository::open(BRANCHES_FIXTURE);
    assert!(repo.is_ok(), "Should open branches fixture");
}

// Additional: Test with empty fixture
#[test]
fn test_open_empty_fixture() {
    let repo = Repository::open(EMPTY_FIXTURE);
    assert!(repo.is_ok(), "Should open empty fixture");
}

// Additional: discover from current directory
#[test]
fn test_discover_from_repo_root() {
    let repo = Repository::discover(SIMPLE_FIXTURE);
    assert!(
        repo.is_ok(),
        "Should discover repository when starting from root"
    );
}

// Additional: Test with deep subdirectory
#[test]
fn test_discover_from_deep_subdir() {
    // Create a deep subdirectory
    let deep_subdir = Path::new(SIMPLE_FIXTURE).join("a/b/c/d");
    fs::create_dir_all(&deep_subdir).ok();

    let repo = Repository::discover(&deep_subdir);
    assert!(
        repo.is_ok(),
        "Should discover repository from deep subdirectory"
    );

    let repo = repo.unwrap();
    let simple_path = Path::new(SIMPLE_FIXTURE).canonicalize().unwrap();
    assert_eq!(
        repo.path().canonicalize().unwrap(),
        simple_path,
        "Discovered repository should be the simple fixture"
    );

    // Clean up
    fs::remove_dir_all(Path::new(SIMPLE_FIXTURE).join("a")).ok();
}
