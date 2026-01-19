//! Integration tests for Tree Diff and Commit Diff functionality.

use std::fs;
use std::path::Path;
use zerogit::diff::DiffStatus;
use zerogit::Repository;

/// Path to the diff test fixture
const DIFF_FIXTURE: &str = "tests/fixtures/diff";

/// Path to the rename test fixture
const RENAME_FIXTURE: &str = "tests/fixtures/rename";

/// Path to the simple test fixture
const SIMPLE_FIXTURE: &str = "tests/fixtures/simple";

/// Path to the merge test fixture
const MERGE_FIXTURE: &str = "tests/fixtures/merge";

// TD-001: Detect added files
#[test]
fn test_td001_detect_added() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Check for file3.txt added
    let added: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Added)
        .collect();

    assert!(
        added.iter().any(|d| d.path() == Path::new("file3.txt")),
        "file3.txt should be detected as added"
    );
}

// TD-002: Detect deleted files
#[test]
fn test_td002_detect_deleted() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Check for file2.txt deleted
    let deleted: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Deleted)
        .collect();

    assert!(
        deleted.iter().any(|d| d.path() == Path::new("file2.txt")),
        "file2.txt should be detected as deleted"
    );
}

// TD-003: Detect modified files
#[test]
fn test_td003_detect_modified() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Check for file1.txt modified
    let modified: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Modified)
        .collect();

    assert!(
        modified.iter().any(|d| d.path() == Path::new("file1.txt")),
        "file1.txt should be detected as modified"
    );

    // Also src/main.rs should be modified
    assert!(
        modified
            .iter()
            .any(|d| d.path() == Path::new("src/main.rs")),
        "src/main.rs should be detected as modified"
    );
}

// TD-004: No changes when comparing same tree
#[test]
fn test_td004_same_tree_no_changes() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();

    let head = repo.head().unwrap();
    let commit = repo.commit(&head.oid().to_hex()).unwrap();
    let tree = repo.tree(&commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&tree), &tree).unwrap();

    assert!(diff.is_empty(), "Comparing same tree should produce no changes");
}

// TD-005: Empty old tree (initial commit)
#[test]
fn test_td005_empty_old_tree() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the initial commit (oldest)
    let commits: Vec<_> = repo.log().unwrap().collect();
    let initial_commit = commits.last().unwrap().as_ref().unwrap();
    let tree = repo.tree(&initial_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(None, &tree).unwrap();

    // All files should be marked as added
    assert!(
        diff.deltas().iter().all(|d| d.status() == DiffStatus::Added),
        "All files in initial commit should be Added"
    );

    // Should have file1.txt, file2.txt, and src/main.rs
    let paths: Vec<_> = diff.deltas().iter().map(|d| d.path()).collect();
    assert!(paths.contains(&Path::new("file1.txt")));
    assert!(paths.contains(&Path::new("file2.txt")));
    assert!(paths.contains(&Path::new("src/main.rs")));
}

// TD-006: Same tree comparison returns empty diff
#[test]
fn test_td006_identical_trees_empty_diff() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();

    let head = repo.head().unwrap();
    let commit = repo.commit(&head.oid().to_hex()).unwrap();
    let tree = repo.tree(&commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&tree), &tree).unwrap();

    assert!(diff.is_empty());
    assert_eq!(diff.len(), 0);
}

// TD-007: Nested paths are correctly reported
#[test]
fn test_td007_nested_paths() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Check that src/main.rs has the full path
    let found = diff
        .deltas()
        .iter()
        .any(|d| d.path() == Path::new("src/main.rs"));

    assert!(found, "Nested path src/main.rs should be correctly reported");
}

// TD-008: Multiple changes (A, D, M mixed)
#[test]
fn test_td008_multiple_changes() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();
    let stats = diff.stats();

    // Verify we have different types of changes
    assert!(stats.added >= 1, "Should have at least 1 added file");
    assert!(stats.deleted >= 1, "Should have at least 1 deleted file");
    assert!(stats.modified >= 1, "Should have at least 1 modified file");
}

// TD-009: Rename detection
#[test]
fn test_td009_rename_detection() {
    let repo = Repository::open(RENAME_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Check for rename
    let renamed: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Renamed)
        .collect();

    assert_eq!(renamed.len(), 1, "Should detect exactly one rename");

    let rename = renamed[0];
    assert_eq!(rename.path(), Path::new("new_name.txt"));
    assert_eq!(rename.old_path(), Some(Path::new("old_name.txt")));
}

// TD-010: Stats calculation
#[test]
fn test_td010_stats_calculation() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();
    let stats = diff.stats();

    // Verify total
    let expected_total = stats.added + stats.deleted + stats.modified + stats.renamed + stats.copied;
    assert_eq!(stats.total(), expected_total);

    // We know the fixture: 1 added, 1 deleted, 2 modified
    assert_eq!(stats.added, 1, "Expected 1 added file");
    assert_eq!(stats.deleted, 1, "Expected 1 deleted file");
    assert_eq!(stats.modified, 2, "Expected 2 modified files");
}

// Additional: status_char returns correct characters
#[test]
fn test_status_char() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    for delta in diff.deltas() {
        let expected_char = match delta.status() {
            DiffStatus::Added => 'A',
            DiffStatus::Deleted => 'D',
            DiffStatus::Modified => 'M',
            DiffStatus::Renamed => 'R',
            DiffStatus::Copied => 'C',
        };
        assert_eq!(delta.status_char(), expected_char);
    }
}

// Additional: Deltas are sorted by path
#[test]
fn test_deltas_sorted_by_path() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the two commits
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    let paths: Vec<_> = diff.deltas().iter().map(|d| d.path()).collect();
    let mut sorted_paths = paths.clone();
    sorted_paths.sort();

    assert_eq!(paths, sorted_paths, "Deltas should be sorted by path");
}

// Additional: Iterator support
#[test]
fn test_iterator_support() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    // Test iter()
    let count_via_iter = diff.iter().count();
    assert_eq!(count_via_iter, diff.len());

    // Test for loop (IntoIterator for &TreeDiff)
    let mut count = 0;
    for _delta in &diff {
        count += 1;
    }
    assert_eq!(count, diff.len());
}

// Additional: OID accessors
#[test]
fn test_oid_accessors() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let new_commit = commits[0].as_ref().unwrap();
    let old_commit = commits[1].as_ref().unwrap();

    let old_tree = repo.tree(&old_commit.tree().to_hex()).unwrap();
    let new_tree = repo.tree(&new_commit.tree().to_hex()).unwrap();

    let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();

    for delta in diff.deltas() {
        match delta.status() {
            DiffStatus::Added => {
                assert!(delta.old_oid().is_none());
                assert!(delta.new_oid().is_some());
            }
            DiffStatus::Deleted => {
                assert!(delta.old_oid().is_some());
                assert!(delta.new_oid().is_none());
            }
            DiffStatus::Modified => {
                assert!(delta.old_oid().is_some());
                assert!(delta.new_oid().is_some());
                assert_ne!(delta.old_oid(), delta.new_oid());
            }
            DiffStatus::Renamed => {
                assert!(delta.old_oid().is_some());
                assert!(delta.new_oid().is_some());
                assert_eq!(delta.old_oid(), delta.new_oid());
            }
            DiffStatus::Copied => {
                assert!(delta.old_oid().is_some());
                assert!(delta.new_oid().is_some());
            }
        }
    }
}

// ============================================================================
// commit_diff tests
// ============================================================================

// CD-001: Normal commit (single parent)
#[test]
fn test_cd001_normal_commit() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the second commit (has one parent)
    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let commit = commits[0].as_ref().unwrap();

    let diff = repo.commit_diff(commit).unwrap();

    // Should have various changes
    let stats = diff.stats();
    assert!(stats.total() > 0, "commit_diff should return changes");

    // Same results as diff_trees between old and new commit
    assert_eq!(stats.added, 1);
    assert_eq!(stats.deleted, 1);
    assert_eq!(stats.modified, 2);
}

// CD-002: Initial commit (no parent)
#[test]
fn test_cd002_initial_commit() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Get the initial commit (oldest)
    let commits: Vec<_> = repo.log().unwrap().collect();
    let initial_commit = commits.last().unwrap().as_ref().unwrap();

    let diff = repo.commit_diff(initial_commit).unwrap();

    // All files should be marked as added
    assert!(
        diff.deltas().iter().all(|d| d.status() == DiffStatus::Added),
        "All files in initial commit should be Added"
    );

    // Should have file1.txt, file2.txt, and src/main.rs
    let paths: Vec<_> = diff.deltas().iter().map(|d| d.path()).collect();
    assert!(paths.contains(&Path::new("file1.txt")));
    assert!(paths.contains(&Path::new("file2.txt")));
    assert!(paths.contains(&Path::new("src/main.rs")));
}

// CD-003: Merge commit (multiple parents)
#[test]
fn test_cd003_merge_commit() {
    let repo = Repository::open(MERGE_FIXTURE).unwrap();

    // Get HEAD which should be the merge commit
    let head = repo.head().unwrap();
    let merge_commit = repo.commit(&head.oid().to_hex()).unwrap();

    // Verify it's a merge commit
    assert!(merge_commit.is_merge(), "HEAD should be a merge commit");

    let diff = repo.commit_diff(&merge_commit).unwrap();

    // Should show diff against first parent (the main branch commit before merge)
    // The first parent has main.txt and main2.txt
    // The merge adds feature.txt from the feature branch
    let paths: Vec<_> = diff.deltas().iter().map(|d| d.path()).collect();

    // feature.txt should be added (relative to first parent)
    assert!(
        paths.contains(&Path::new("feature.txt")),
        "feature.txt should be in diff (added from feature branch)"
    );
}

// CD-004: Empty commit (no changes)
#[test]
fn test_cd004_empty_commit_equivalent() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();

    let head = repo.head().unwrap();
    let commit = repo.commit(&head.oid().to_hex()).unwrap();

    // Get parent
    let parent_commit = repo.commit(&commit.parent().unwrap().to_hex()).unwrap();

    let parent_diff = repo.commit_diff(&parent_commit).unwrap();

    // Initial commit should have only README.md
    assert_eq!(parent_diff.deltas().len(), 1);
    assert_eq!(parent_diff.deltas()[0].path(), Path::new("README.md"));
    assert_eq!(parent_diff.deltas()[0].status(), DiffStatus::Added);
}

// CD-005: Multiple change types (A/D/M mixed)
#[test]
fn test_cd005_mixed_changes() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    let commits: Vec<_> = repo.log().unwrap().take(2).collect();
    let commit = commits[0].as_ref().unwrap();

    let diff = repo.commit_diff(commit).unwrap();

    let mut has_added = false;
    let mut has_deleted = false;
    let mut has_modified = false;

    for delta in diff.deltas() {
        match delta.status() {
            DiffStatus::Added => has_added = true,
            DiffStatus::Deleted => has_deleted = true,
            DiffStatus::Modified => has_modified = true,
            _ => {}
        }
    }

    assert!(has_added, "Should have at least one Added");
    assert!(has_deleted, "Should have at least one Deleted");
    assert!(has_modified, "Should have at least one Modified");
}

// CD-006: Log integration
#[test]
fn test_cd006_log_integration() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Iterate through all commits and get their diffs
    let mut count = 0;
    for commit_result in repo.log().unwrap() {
        let commit = commit_result.unwrap();
        let diff = repo.commit_diff(&commit).unwrap();

        // Each commit should have a valid diff
        let stats = diff.stats();

        // First commit has various changes, second (initial) has 3 added files
        if commit.is_root() {
            assert!(
                diff.deltas().iter().all(|d| d.status() == DiffStatus::Added),
                "Initial commit should only have Added files"
            );
        }

        // Verify total calculation
        assert_eq!(
            stats.total(),
            stats.added + stats.deleted + stats.modified + stats.renamed + stats.copied
        );

        count += 1;
    }

    assert!(count > 0, "Should have processed at least one commit");
}

// ============================================================================
// Working Tree / Index Diff tests (Issue #031)
// ============================================================================

/// Helper to copy a fixture to a temporary directory for modification
fn copy_fixture_to_temp(fixture_path: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let target = temp_dir.path();

    // Copy all files recursively
    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                copy_dir_all(&src_path, &dst_path);
            } else {
                fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    copy_dir_all(Path::new(fixture_path), target);
    temp_dir
}

// WD-001: Unstaged modification
#[test]
fn test_wd001_unstaged_modification() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Modify a file without staging
    let file_path = temp.path().join("file1.txt");
    fs::write(&file_path, "modified content\n").unwrap();

    let diff = repo.diff_index_to_workdir().unwrap();

    let modified: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Modified)
        .collect();

    assert!(
        modified.iter().any(|d| d.path() == Path::new("file1.txt")),
        "file1.txt should be detected as modified in workdir"
    );
}

// WD-002: Unstaged deletion
#[test]
fn test_wd002_unstaged_deletion() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Delete a file without staging
    let file_path = temp.path().join("file1.txt");
    fs::remove_file(&file_path).unwrap();

    let diff = repo.diff_index_to_workdir().unwrap();

    let deleted: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Deleted)
        .collect();

    assert!(
        deleted.iter().any(|d| d.path() == Path::new("file1.txt")),
        "file1.txt should be detected as deleted in workdir"
    );
}

// WD-003: Untracked file (shows as Added in diff_index_to_workdir)
#[test]
fn test_wd003_untracked_file() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Create a new untracked file
    let new_file = temp.path().join("new_untracked.txt");
    fs::write(&new_file, "new content\n").unwrap();

    let diff = repo.diff_index_to_workdir().unwrap();

    let added: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Added)
        .collect();

    assert!(
        added
            .iter()
            .any(|d| d.path() == Path::new("new_untracked.txt")),
        "new_untracked.txt should be detected as added in workdir diff"
    );
}

// WD-004: Staged modification (diff_head_to_index)
#[test]
fn test_wd004_staged_modification() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Modify and stage a file
    let file_path = temp.path().join("file1.txt");
    fs::write(&file_path, "staged modification\n").unwrap();
    repo.add("file1.txt").unwrap();

    let diff = repo.diff_head_to_index().unwrap();

    let modified: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Modified)
        .collect();

    assert!(
        modified.iter().any(|d| d.path() == Path::new("file1.txt")),
        "file1.txt should be detected as modified in staged diff"
    );
}

// WD-005: Staged addition (new file added to index)
#[test]
fn test_wd005_staged_addition() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Create and stage a new file
    let new_file = temp.path().join("new_staged.txt");
    fs::write(&new_file, "new staged content\n").unwrap();
    repo.add("new_staged.txt").unwrap();

    let diff = repo.diff_head_to_index().unwrap();

    let added: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Added)
        .collect();

    assert!(
        added.iter().any(|d| d.path() == Path::new("new_staged.txt")),
        "new_staged.txt should be detected as added in staged diff"
    );
}

// WD-006: Clean state (no changes)
#[test]
fn test_wd006_clean_state() {
    let repo = Repository::open(DIFF_FIXTURE).unwrap();

    // Fixture is clean, all diffs should be empty
    let unstaged = repo.diff_index_to_workdir().unwrap();
    let staged = repo.diff_head_to_index().unwrap();
    let all_changes = repo.diff_head_to_workdir().unwrap();

    assert!(
        unstaged.is_empty(),
        "diff_index_to_workdir should be empty for clean repo"
    );
    assert!(
        staged.is_empty(),
        "diff_head_to_index should be empty for clean repo"
    );
    assert!(
        all_changes.is_empty(),
        "diff_head_to_workdir should be empty for clean repo"
    );
}

// WD-007: HEAD diff (combined staged + unstaged)
#[test]
fn test_wd007_head_diff_combined() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Stage one change
    let file1 = temp.path().join("file1.txt");
    fs::write(&file1, "staged change\n").unwrap();
    repo.add("file1.txt").unwrap();

    // Make another unstaged change
    let file3 = temp.path().join("file3.txt");
    fs::write(&file3, "unstaged change\n").unwrap();

    let head_diff = repo.diff_head_to_workdir().unwrap();

    // Both changes should appear
    let paths: Vec<_> = head_diff.deltas().iter().map(|d| d.path()).collect();
    assert!(
        paths.contains(&Path::new("file1.txt")),
        "file1.txt should be in HEAD diff"
    );
    assert!(
        paths.contains(&Path::new("file3.txt")),
        "file3.txt should be in HEAD diff"
    );
}

// WD-008: Status consistency
#[test]
fn test_wd008_status_consistency() {
    let temp = copy_fixture_to_temp(DIFF_FIXTURE);
    let repo = Repository::open(temp.path()).unwrap();

    // Create various changes
    // 1. New untracked file
    fs::write(temp.path().join("untracked.txt"), "untracked\n").unwrap();

    // 2. Modified file
    fs::write(temp.path().join("file1.txt"), "modified\n").unwrap();

    // Get status and diffs
    let status = repo.status().unwrap();
    let unstaged_diff = repo.diff_index_to_workdir().unwrap();

    // Count modified in status
    let status_modified: Vec<_> = status
        .iter()
        .filter(|e| e.status() == zerogit::FileStatus::Modified)
        .collect();

    // Count modified in diff
    let diff_modified: Vec<_> = unstaged_diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Modified)
        .collect();

    // Should match
    assert_eq!(
        status_modified.len(),
        diff_modified.len(),
        "Modified count should match between status() and diff_index_to_workdir()"
    );

    // Count untracked in status
    let status_untracked: Vec<_> = status
        .iter()
        .filter(|e| e.status() == zerogit::FileStatus::Untracked)
        .collect();

    // Count added in diff (untracked files appear as Added in diff)
    let diff_added: Vec<_> = unstaged_diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Added)
        .collect();

    // Untracked in status should correspond to Added in diff
    assert!(
        diff_added.len() >= status_untracked.len(),
        "Added count in diff should be >= untracked count in status"
    );
}
