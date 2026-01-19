//! Integration tests for Repository module.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
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

// RP-008: Repository::init creates a new repository
#[test]
fn test_rp008_init_creates_repository() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("new-repo");

    let repo = Repository::init(&repo_path);
    assert!(repo.is_ok(), "Should initialize new repository");

    let repo = repo.unwrap();

    // Check that the repository paths are correct
    assert_eq!(
        repo.path().canonicalize().unwrap(),
        repo_path.canonicalize().unwrap(),
        "path() should return the repository root"
    );
    assert!(
        repo.git_dir().ends_with(".git"),
        "git_dir() should end with .git"
    );

    // Verify .git directory structure
    let git_dir = repo_path.join(".git");
    assert!(git_dir.is_dir(), ".git directory should exist");
    assert!(git_dir.join("HEAD").is_file(), "HEAD file should exist");
    assert!(
        git_dir.join("objects").is_dir(),
        "objects directory should exist"
    );
    assert!(
        git_dir.join("refs/heads").is_dir(),
        "refs/heads directory should exist"
    );
    assert!(
        git_dir.join("refs/tags").is_dir(),
        "refs/tags directory should exist"
    );
    assert!(git_dir.join("config").is_file(), "config file should exist");

    // Verify HEAD content
    let head_content = fs::read_to_string(git_dir.join("HEAD")).unwrap();
    assert_eq!(
        head_content, "ref: refs/heads/main\n",
        "HEAD should point to main branch"
    );

    // Verify config content
    let config_content = fs::read_to_string(git_dir.join("config")).unwrap();
    assert!(
        config_content.contains("bare = false"),
        "config should indicate non-bare repository"
    );
}

// RP-009: Repository::init fails if repository already exists
#[test]
fn test_rp009_init_fails_if_exists() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("existing-repo");

    // First init should succeed
    let repo = Repository::init(&repo_path);
    assert!(repo.is_ok(), "First init should succeed");

    // Second init should fail
    let repo = Repository::init(&repo_path);
    assert!(
        matches!(repo, Err(Error::AlreadyARepository(_))),
        "Second init should return AlreadyARepository error"
    );
}

// RP-010: Repository::init_bare creates a bare repository
#[test]
fn test_rp010_init_bare_creates_bare_repository() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("bare-repo.git");

    let repo = Repository::init_bare(&repo_path);
    assert!(repo.is_ok(), "Should initialize bare repository");

    let repo = repo.unwrap();

    // For bare repositories, git_dir and work_dir should be the same
    assert_eq!(
        repo.path().canonicalize().unwrap(),
        repo.git_dir().canonicalize().unwrap(),
        "For bare repos, path() and git_dir() should be the same"
    );

    // Verify directory structure (directly in repo_path, not in .git subdirectory)
    assert!(repo_path.join("HEAD").is_file(), "HEAD file should exist");
    assert!(
        repo_path.join("objects").is_dir(),
        "objects directory should exist"
    );
    assert!(
        repo_path.join("refs/heads").is_dir(),
        "refs/heads directory should exist"
    );
    assert!(
        repo_path.join("refs/tags").is_dir(),
        "refs/tags directory should exist"
    );
    assert!(repo_path.join("config").is_file(), "config file should exist");

    // Verify there's no .git subdirectory
    assert!(
        !repo_path.join(".git").exists(),
        "Bare repository should not have .git subdirectory"
    );

    // Verify config content
    let config_content = fs::read_to_string(repo_path.join("config")).unwrap();
    assert!(
        config_content.contains("bare = true"),
        "config should indicate bare repository"
    );
}

// RP-011: Repository::init_bare fails if repository already exists
#[test]
fn test_rp011_init_bare_fails_if_exists() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("existing-bare.git");

    // First init should succeed
    let repo = Repository::init_bare(&repo_path);
    assert!(repo.is_ok(), "First init_bare should succeed");

    // Second init should fail
    let repo = Repository::init_bare(&repo_path);
    assert!(
        matches!(repo, Err(Error::AlreadyARepository(_))),
        "Second init_bare should return AlreadyARepository error"
    );
}

// RP-012: Initialized repository can be opened
#[test]
fn test_rp012_init_then_open() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("init-open-repo");

    // Initialize
    let init_repo = Repository::init(&repo_path).unwrap();
    let init_path = init_repo.path().canonicalize().unwrap();
    drop(init_repo);

    // Open the same repository
    let open_repo = Repository::open(&repo_path);
    assert!(open_repo.is_ok(), "Should be able to open initialized repository");

    let open_repo = open_repo.unwrap();
    assert_eq!(
        open_repo.path().canonicalize().unwrap(),
        init_path,
        "Opened repository should have the same path"
    );
}

// RP-013: Repository::init creates parent directories
#[test]
fn test_rp013_init_creates_parent_dirs() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("deep/nested/path/repo");

    // Parent directories don't exist yet
    assert!(!temp.path().join("deep").exists());

    let repo = Repository::init(&repo_path);
    assert!(repo.is_ok(), "Should create parent directories");

    // Verify the repository was created
    assert!(repo_path.join(".git/HEAD").is_file());
}
