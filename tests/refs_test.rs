//! Integration tests for remote branches and tags.

use std::path::PathBuf;
use zerogit::Repository;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ============================================================================
// Remote Branches Tests (RB-001 to RB-004)
// ============================================================================

// RB-001: リモートブランチ一覧 - refs/remotes/origin/* が存在すれば全リモートブランチを取得
#[test]
fn test_rb001_remote_branches_list() {
    let repo = Repository::open(fixtures_path().join("remotes")).unwrap();
    let branches = repo.remote_branches().unwrap();

    // origin/main, origin/develop, origin/feature/xyz, upstream/main が存在するはず
    let names: Vec<String> = branches.iter().map(|b| b.full_name()).collect();

    assert!(names.contains(&"origin/main".to_string()));
    assert!(names.contains(&"origin/develop".to_string()));
    assert!(names.contains(&"origin/feature/xyz".to_string()));
    assert!(names.contains(&"upstream/main".to_string()));
}

// RB-002: 複数リモート - origin, upstream が存在する場合、両方のブランチを取得
#[test]
fn test_rb002_multiple_remotes() {
    let repo = Repository::open(fixtures_path().join("remotes")).unwrap();
    let branches = repo.remote_branches().unwrap();

    let remotes: Vec<&str> = branches.iter().map(|b| b.remote()).collect();
    assert!(remotes.contains(&"origin"));
    assert!(remotes.contains(&"upstream"));
}

// RB-003: ネストしたブランチ - feature/xyz 形式を正しくパース
#[test]
fn test_rb003_nested_branch_name() {
    let repo = Repository::open(fixtures_path().join("remotes")).unwrap();
    let branches = repo.remote_branches().unwrap();

    let nested = branches
        .iter()
        .find(|b| b.name() == "feature/xyz")
        .expect("feature/xyz should exist");

    assert_eq!(nested.remote(), "origin");
    assert_eq!(nested.name(), "feature/xyz");
    assert_eq!(nested.full_name(), "origin/feature/xyz");
    assert_eq!(
        nested.reference_name(),
        "refs/remotes/origin/feature/xyz"
    );
}

// RB-004: リモートなし - refs/remotes が空の場合、空のVecを返す
#[test]
fn test_rb004_no_remotes() {
    let repo = Repository::open(fixtures_path().join("simple")).unwrap();
    let branches = repo.remote_branches().unwrap();

    assert!(branches.is_empty());
}

// Additional: RemoteBranch accessor methods
#[test]
fn test_remote_branch_accessors() {
    let repo = Repository::open(fixtures_path().join("remotes")).unwrap();
    let branches = repo.remote_branches().unwrap();

    let origin_main = branches
        .iter()
        .find(|b| b.full_name() == "origin/main")
        .expect("origin/main should exist");

    assert_eq!(origin_main.remote(), "origin");
    assert_eq!(origin_main.name(), "main");
    assert_eq!(origin_main.short_oid().len(), 7);
    assert!(!origin_main.oid().to_hex().is_empty());
}

// ============================================================================
// Tags Tests (T-001 to T-003)
// ============================================================================

// T-001: 軽量タグ一覧 - refs/tags/* が存在すれば全タグを取得
#[test]
fn test_t001_lightweight_tags() {
    let repo = Repository::open(fixtures_path().join("tags")).unwrap();
    let tags = repo.tags().unwrap();

    let names: Vec<&str> = tags.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"v1.0.0"));
}

// T-002: 注釈付きタグ - tag objectが存在する場合、message, taggerを取得
#[test]
fn test_t002_annotated_tag() {
    let repo = Repository::open(fixtures_path().join("tags")).unwrap();
    let tags = repo.tags().unwrap();

    let annotated = tags
        .iter()
        .find(|t| t.name() == "v1.0.1")
        .expect("v1.0.1 should exist");

    assert!(annotated.is_annotated());
    assert!(annotated.message().is_some());
    assert!(annotated.message().unwrap().contains("Annotated tag"));
    assert!(annotated.tagger().is_some());
    assert_eq!(annotated.tagger().unwrap().name(), "Test User");
}

// T-003: 混在 - 軽量・注釈付き両方を正しく取得
#[test]
fn test_t003_mixed_tags() {
    let repo = Repository::open(fixtures_path().join("tags")).unwrap();
    let tags = repo.tags().unwrap();

    // v1.0.0 は軽量タグ
    let lightweight = tags
        .iter()
        .find(|t| t.name() == "v1.0.0")
        .expect("v1.0.0 should exist");
    assert!(!lightweight.is_annotated());
    assert!(lightweight.message().is_none());
    assert!(lightweight.tagger().is_none());

    // v1.0.1 は注釈付きタグ
    let annotated = tags
        .iter()
        .find(|t| t.name() == "v1.0.1")
        .expect("v1.0.1 should exist");
    assert!(annotated.is_annotated());
}

// Additional: Tags are sorted
#[test]
fn test_tags_sorted() {
    let repo = Repository::open(fixtures_path().join("tags")).unwrap();
    let tags = repo.tags().unwrap();

    let names: Vec<&str> = tags.iter().map(|t| t.name()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

// Additional: Tag target points to a valid commit
#[test]
fn test_tag_target_valid() {
    let repo = Repository::open(fixtures_path().join("tags")).unwrap();
    let tags = repo.tags().unwrap();

    for tag in &tags {
        // Both lightweight and annotated tags should have a target
        assert!(!tag.target().to_hex().is_empty());
        assert_eq!(tag.target().to_hex().len(), 40);
    }
}

// Additional: No tags returns empty vec
#[test]
fn test_no_tags() {
    let repo = Repository::open(fixtures_path().join("simple")).unwrap();
    let tags = repo.tags().unwrap();

    assert!(tags.is_empty());
}

// ============================================================================
// RefStore Tests
// ============================================================================

#[test]
fn test_ref_store_remote_branches() {
    use zerogit::refs::RefStore;

    let git_dir = fixtures_path().join("remotes").join(".git");
    let store = RefStore::new(&git_dir);

    let branches = store.remote_branches().unwrap();

    // Should return tuples of (remote, branch)
    assert!(branches.contains(&("origin".to_string(), "main".to_string())));
    assert!(branches.contains(&("origin".to_string(), "develop".to_string())));
    assert!(branches.contains(&("origin".to_string(), "feature/xyz".to_string())));
    assert!(branches.contains(&("upstream".to_string(), "main".to_string())));
}

#[test]
fn test_ref_store_remotes() {
    use zerogit::refs::RefStore;

    let git_dir = fixtures_path().join("remotes").join(".git");
    let store = RefStore::new(&git_dir);

    let remotes = store.remotes().unwrap();

    assert!(remotes.contains(&"origin".to_string()));
    assert!(remotes.contains(&"upstream".to_string()));
    assert_eq!(remotes.len(), 2);
}
