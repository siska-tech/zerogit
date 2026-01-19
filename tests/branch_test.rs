//! Integration tests for branch operations (Issue #026).
//!
//! Test cases: W-006 to W-010

use std::fs;
use tempfile::TempDir;
use zerogit::{Error, Repository};

/// Helper to create a minimal valid .git directory
fn create_git_dir(path: &std::path::Path) {
    let git_dir = path.join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    fs::create_dir_all(git_dir.join("objects")).unwrap();
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
}

/// Helper to set up a repository with an initial commit
fn setup_repo_with_commit(temp: &TempDir) -> (Repository, zerogit::Oid) {
    create_git_dir(temp.path());

    let repo = Repository::open(temp.path()).unwrap();

    // Create a file and commit
    fs::write(temp.path().join("README.md"), "# Test Project").unwrap();
    repo.add("README.md").unwrap();
    let commit_oid = repo
        .create_commit("Initial commit", "Test User", "test@example.com")
        .unwrap();

    (repo, commit_oid)
}

// =============================================================================
// W-006: create_branch tests
// =============================================================================

/// W-006: Create branch at HEAD
#[test]
fn test_w006_create_branch_at_head() {
    let temp = TempDir::new().unwrap();
    let (repo, head_oid) = setup_repo_with_commit(&temp);

    // Create a new branch
    let branch = repo.create_branch("feature/test", None).unwrap();

    // Verify branch points to HEAD
    assert_eq!(branch.name(), "feature/test");
    assert_eq!(branch.oid(), &head_oid);

    // Verify branch file exists
    let branch_path = temp.path().join(".git/refs/heads/feature/test");
    assert!(branch_path.exists());

    // Verify content
    let content = fs::read_to_string(branch_path).unwrap();
    assert_eq!(content.trim(), head_oid.to_hex());
}

/// W-006: Create branch at specific commit
#[test]
fn test_w006_create_branch_at_commit() {
    let temp = TempDir::new().unwrap();
    let (repo, first_oid) = setup_repo_with_commit(&temp);

    // Create second commit
    fs::write(temp.path().join("file.txt"), "Content").unwrap();
    repo.add("file.txt").unwrap();
    let _second_oid = repo
        .create_commit("Second commit", "Test User", "test@example.com")
        .unwrap();

    // Create branch at first commit
    let branch = repo.create_branch("old-state", Some(first_oid)).unwrap();

    assert_eq!(branch.oid(), &first_oid);
}

/// W-006: Cannot create branch with invalid name
#[test]
fn test_w006_create_branch_invalid_name() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Various invalid names
    assert!(matches!(
        repo.create_branch("", None),
        Err(Error::InvalidRefName(_))
    ));
    assert!(matches!(
        repo.create_branch("-branch", None),
        Err(Error::InvalidRefName(_))
    ));
    assert!(matches!(
        repo.create_branch("branch.lock", None),
        Err(Error::InvalidRefName(_))
    ));
    assert!(matches!(
        repo.create_branch("foo..bar", None),
        Err(Error::InvalidRefName(_))
    ));
}

/// W-006: Cannot create branch that already exists
#[test]
fn test_w006_create_branch_exists() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Create first branch
    repo.create_branch("feature", None).unwrap();

    // Try to create same branch again
    let result = repo.create_branch("feature", None);
    assert!(matches!(result, Err(Error::RefAlreadyExists(_))));
}

// =============================================================================
// W-007: delete_branch tests
// =============================================================================

/// W-007: Delete an existing branch
#[test]
fn test_w007_delete_branch() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Create and delete branch
    repo.create_branch("to-delete", None).unwrap();
    assert!(temp.path().join(".git/refs/heads/to-delete").exists());

    repo.delete_branch("to-delete").unwrap();
    assert!(!temp.path().join(".git/refs/heads/to-delete").exists());
}

/// W-007: Delete nested branch cleans up directories
#[test]
fn test_w007_delete_branch_cleanup() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Create nested branch
    repo.create_branch("feature/deep/branch", None).unwrap();

    // Delete it
    repo.delete_branch("feature/deep/branch").unwrap();

    // Empty directories should be cleaned up
    assert!(!temp.path().join(".git/refs/heads/feature/deep").exists());
    assert!(!temp.path().join(".git/refs/heads/feature").exists());
}

// =============================================================================
// W-008: Cannot delete current branch
// =============================================================================

/// W-008: Cannot delete current branch
#[test]
fn test_w008_delete_current_branch() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // main is current branch
    let result = repo.delete_branch("main");
    assert!(matches!(result, Err(Error::CannotDeleteCurrentBranch)));
}

/// W-008: Cannot delete nonexistent branch
#[test]
fn test_w008_delete_nonexistent() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    let result = repo.delete_branch("nonexistent");
    assert!(matches!(result, Err(Error::RefNotFound(_))));
}

// =============================================================================
// W-009: checkout tests
// =============================================================================

/// W-009: Checkout switches to existing branch
#[test]
fn test_w009_checkout_branch() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Create new branch
    repo.create_branch("develop", None).unwrap();

    // Checkout the branch
    repo.checkout("develop").unwrap();

    // Verify HEAD points to develop
    let head = repo.head().unwrap();
    assert!(head.is_branch());
    assert_eq!(head.branch_name(), Some("develop"));
}

/// W-009: Checkout to commit creates detached HEAD
#[test]
fn test_w009_checkout_detached() {
    let temp = TempDir::new().unwrap();
    let (repo, first_oid) = setup_repo_with_commit(&temp);

    // Create second commit
    fs::write(temp.path().join("file.txt"), "Content").unwrap();
    repo.add("file.txt").unwrap();
    repo.create_commit("Second commit", "Test User", "test@example.com")
        .unwrap();

    // Checkout first commit
    repo.checkout(&first_oid.to_hex()).unwrap();

    // Verify detached HEAD
    let head = repo.head().unwrap();
    assert!(head.is_detached());
    assert_eq!(head.oid(), &first_oid);
}

/// W-009: Checkout updates working tree
#[test]
fn test_w009_checkout_updates_tree() {
    let temp = TempDir::new().unwrap();
    create_git_dir(temp.path());

    let repo = Repository::open(temp.path()).unwrap();

    // First commit with only README
    fs::write(temp.path().join("README.md"), "# Initial").unwrap();
    repo.add("README.md").unwrap();
    let first_oid = repo
        .create_commit("First", "Test User", "test@example.com")
        .unwrap();

    // Create branch at first commit
    repo.create_branch("v1", Some(first_oid)).unwrap();

    // Second commit adds new file
    fs::write(temp.path().join("new.txt"), "New file").unwrap();
    repo.add("new.txt").unwrap();
    repo.create_commit("Second", "Test User", "test@example.com")
        .unwrap();

    // Both files exist
    assert!(temp.path().join("README.md").exists());
    assert!(temp.path().join("new.txt").exists());

    // Checkout v1
    repo.checkout("v1").unwrap();

    // Only README should exist
    assert!(temp.path().join("README.md").exists());
    assert!(!temp.path().join("new.txt").exists());

    // Checkout main
    repo.checkout("main").unwrap();

    // Both files should exist
    assert!(temp.path().join("README.md").exists());
    assert!(temp.path().join("new.txt").exists());
}

/// W-009: Checkout updates file content
#[test]
fn test_w009_checkout_updates_content() {
    let temp = TempDir::new().unwrap();
    create_git_dir(temp.path());

    let repo = Repository::open(temp.path()).unwrap();

    // First commit
    fs::write(temp.path().join("file.txt"), "Version 1").unwrap();
    repo.add("file.txt").unwrap();
    let first_oid = repo
        .create_commit("First", "Test User", "test@example.com")
        .unwrap();

    // Create branch at first commit
    repo.create_branch("v1", Some(first_oid)).unwrap();

    // Second commit modifies file
    fs::write(temp.path().join("file.txt"), "Version 2").unwrap();
    repo.add("file.txt").unwrap();
    repo.create_commit("Second", "Test User", "test@example.com")
        .unwrap();

    // Verify current content
    assert_eq!(
        fs::read_to_string(temp.path().join("file.txt")).unwrap(),
        "Version 2"
    );

    // Checkout v1
    repo.checkout("v1").unwrap();

    // Content should be Version 1
    assert_eq!(
        fs::read_to_string(temp.path().join("file.txt")).unwrap(),
        "Version 1"
    );

    // Checkout main
    repo.checkout("main").unwrap();

    // Content should be Version 2
    assert_eq!(
        fs::read_to_string(temp.path().join("file.txt")).unwrap(),
        "Version 2"
    );
}

// =============================================================================
// W-010: Checkout with dirty working tree
// =============================================================================

/// W-010: Cannot checkout with modified files
#[test]
fn test_w010_checkout_dirty_modified() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    repo.create_branch("feature", None).unwrap();

    // Modify tracked file
    fs::write(temp.path().join("README.md"), "Modified").unwrap();

    // Should fail
    let result = repo.checkout("feature");
    assert!(matches!(result, Err(Error::DirtyWorkingTree)));
}

/// W-010: Cannot checkout with untracked files
#[test]
fn test_w010_checkout_dirty_untracked() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    repo.create_branch("feature", None).unwrap();

    // Create untracked file
    fs::write(temp.path().join("untracked.txt"), "Untracked").unwrap();

    // Should fail
    let result = repo.checkout("feature");
    assert!(matches!(result, Err(Error::DirtyWorkingTree)));
}

/// W-010: Cannot checkout nonexistent ref
#[test]
fn test_w010_checkout_nonexistent() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    let result = repo.checkout("nonexistent");
    assert!(matches!(result, Err(Error::RefNotFound(_))));
}

// =============================================================================
// Integration: Full workflow test
// =============================================================================

/// Integration: Complete branch workflow
#[test]
fn test_integration_branch_workflow() {
    let temp = TempDir::new().unwrap();
    create_git_dir(temp.path());

    let repo = Repository::open(temp.path()).unwrap();

    // Initial commit on main
    fs::write(temp.path().join("main.txt"), "Main content").unwrap();
    repo.add("main.txt").unwrap();
    repo.create_commit("Initial on main", "Test User", "test@example.com")
        .unwrap();

    // Create feature branch
    let feature = repo.create_branch("feature", None).unwrap();
    assert_eq!(feature.name(), "feature");

    // Checkout feature branch
    repo.checkout("feature").unwrap();
    assert_eq!(repo.head().unwrap().branch_name(), Some("feature"));

    // Make commit on feature branch
    fs::write(temp.path().join("feature.txt"), "Feature content").unwrap();
    repo.add("feature.txt").unwrap();
    repo.create_commit("Add feature", "Test User", "test@example.com")
        .unwrap();

    // Both files exist
    assert!(temp.path().join("main.txt").exists());
    assert!(temp.path().join("feature.txt").exists());

    // Switch back to main
    repo.checkout("main").unwrap();
    assert_eq!(repo.head().unwrap().branch_name(), Some("main"));

    // Only main.txt should exist
    assert!(temp.path().join("main.txt").exists());
    assert!(!temp.path().join("feature.txt").exists());

    // Delete feature branch (now that we're on main)
    repo.delete_branch("feature").unwrap();

    // Branch should not exist
    assert!(!temp.path().join(".git/refs/heads/feature").exists());
}

// =============================================================================
// Repository::branches() tests (Issue #2)
// =============================================================================

/// Test listing branches returns all local branches
#[test]
fn test_branches_list() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Initially only main branch exists
    let branches = repo.branches().unwrap();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].name(), "main");
    assert!(branches[0].is_current());

    // Create additional branches
    repo.create_branch("develop", None).unwrap();
    repo.create_branch("feature/test", None).unwrap();

    // List branches again
    let branches = repo.branches().unwrap();
    assert_eq!(branches.len(), 3);

    // Branches should be sorted by name
    assert_eq!(branches[0].name(), "develop");
    assert_eq!(branches[1].name(), "feature/test");
    assert_eq!(branches[2].name(), "main");

    // main should still be current
    assert!(!branches[0].is_current());
    assert!(!branches[1].is_current());
    assert!(branches[2].is_current());
}

/// Test branches() marks current branch correctly after checkout
#[test]
fn test_branches_current_after_checkout() {
    let temp = TempDir::new().unwrap();
    let (repo, _) = setup_repo_with_commit(&temp);

    // Create a feature branch
    repo.create_branch("feature", None).unwrap();

    // Checkout feature branch
    repo.checkout("feature").unwrap();

    // List branches - feature should now be current
    let branches = repo.branches().unwrap();
    assert_eq!(branches.len(), 2);

    let feature = branches.iter().find(|b| b.name() == "feature").unwrap();
    let main = branches.iter().find(|b| b.name() == "main").unwrap();

    assert!(feature.is_current());
    assert!(!main.is_current());
}

/// Test branches() returns empty when no branches exist (before first commit)
#[test]
fn test_branches_empty_repo() {
    let temp = TempDir::new().unwrap();
    create_git_dir(temp.path());

    let repo = Repository::open(temp.path()).unwrap();

    // No branches exist yet (unborn HEAD)
    let branches = repo.branches().unwrap();
    assert!(branches.is_empty());
}

/// Test branches() includes OID for each branch
#[test]
fn test_branches_include_oid() {
    let temp = TempDir::new().unwrap();
    let (repo, first_oid) = setup_repo_with_commit(&temp);

    // Create second commit
    fs::write(temp.path().join("file.txt"), "Content").unwrap();
    repo.add("file.txt").unwrap();
    let second_oid = repo
        .create_commit("Second commit", "Test User", "test@example.com")
        .unwrap();

    // Create branch at first commit
    repo.create_branch("old-branch", Some(first_oid)).unwrap();

    // List branches
    let branches = repo.branches().unwrap();

    let main = branches.iter().find(|b| b.name() == "main").unwrap();
    let old = branches.iter().find(|b| b.name() == "old-branch").unwrap();

    assert_eq!(main.oid(), &second_oid);
    assert_eq!(old.oid(), &first_oid);
}
