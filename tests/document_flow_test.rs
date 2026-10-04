//! The document diff flow used by KazeNhanh (see examples/document_diff.rs):
//! pin OIDs, list changes from trees, then read blobs and diff lines.
//!
//! Git is used only to build fixtures.

use std::path::Path;
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::objects::pack::PackIndex;
use zerogit::{BlobDiffContent, DiffOptions, Error, FileMode, Oid, Repository, SkipReason, Tree};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Background auto-maintenance would repack concurrently with the test.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, path: &str, content: &[u8]) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A packed repository with packed-refs: documents, a rename with a mode
/// change, a symlink, a gitlink, a binary file and a merge.
fn document_repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    write(dir, "docs/仕様.md", "# 仕様\n\n概要\n詳細\n".as_bytes());
    write(dir, "tools/build.sh", b"echo build\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Initial"]);
    let initial = git(dir, &["rev-parse", "HEAD"]).trim().to_owned();

    write(
        dir,
        "docs/仕様.md",
        "# 仕様\n\n概要を更新\n詳細\n追記\n".as_bytes(),
    );
    git(dir, &["mv", "tools/build.sh", "build.sh"]);
    write(dir, "image.bin", b"\x00\x01\x02");
    git(dir, &["add", "-A"]);
    // After `add -A`: with core.filemode (Linux/macOS) it would reset the mode.
    git(dir, &["update-index", "--chmod=+x", "build.sh"]);
    let target = git(dir, &["hash-object", "-w", "docs/仕様.md"])
        .trim()
        .to_owned();
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{},spec-link", target),
        ],
    );
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},vendor/lib", initial),
        ],
    );
    git(dir, &["commit", "-q", "-m", "Edit documents"]);

    git(dir, &["checkout", "-q", "-b", "review"]);
    write(dir, "docs/review.md", b"review notes\n");
    // The symlink and gitlink exist only in the index; `add -A` would delete them.
    git(dir, &["add", "docs/review.md"]);
    git(dir, &["commit", "-q", "-m", "Review"]);
    git(dir, &["tag", "review-tip"]);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "docs/main.md", b"main side\n");
    git(dir, &["add", "docs/main.md"]);
    git(dir, &["commit", "-q", "-m", "Main side"]);
    git(
        dir,
        &["merge", "-q", "--no-ff", "-m", "Merge review", "review"],
    );

    git(dir, &["repack", "-a", "-d", "-q"]);
    git(dir, &["pack-refs", "--all", "--prune"]);
    temp
}

fn tree_of(repo: &Repository, commit: &Oid) -> Tree {
    repo.tree(&repo.commit(&commit.to_hex()).unwrap().tree().to_hex())
        .unwrap()
}

/// The change list and line diffs between two pinned commits (None = empty tree).
fn compare(repo: &Repository, base: Option<&Oid>, target: &Oid) -> Vec<String> {
    let base_tree = base.map(|oid| tree_of(repo, oid));
    let changes = repo
        .diff_trees(base_tree.as_ref(), &tree_of(repo, target))
        .unwrap();
    // Delta paths use the platform separator; Git paths always use '/'.
    let slash = |path: &Path| path.to_str().unwrap().replace('\\', "/");
    let mut out = Vec::new();
    for delta in changes.deltas() {
        out.push(format!(
            "{} {:?} {:?} {:?} {:?}",
            delta.status_char(),
            delta.old_path().map(slash),
            slash(delta.path()),
            delta.old_mode(),
            delta.new_mode()
        ));
        if delta.new_mode() == Some(FileMode::Submodule)
            || delta.old_mode() == Some(FileMode::Submodule)
        {
            continue; // gitlinks are handled without reading blobs
        }
        let diff = repo
            .diff_blobs(delta.old_oid(), delta.new_oid(), &DiffOptions::new())
            .unwrap();
        match diff.content() {
            BlobDiffContent::Text(hunks) => {
                for hunk in hunks {
                    out.push(hunk.header());
                    for line in hunk.lines() {
                        out.push(format!(
                            "{:?} {:?} {:?} {}",
                            line.kind(),
                            line.old_lineno(),
                            line.new_lineno(),
                            line.text()
                        ));
                    }
                }
            }
            other => out.push(format!("{:?}", other)),
        }
    }
    out
}

/// Returns (initial, edit, review tip, merge) by walking from HEAD.
fn commits(repo: &Repository) -> (Oid, Oid, Oid, Oid) {
    let head = *repo.head().unwrap().oid();
    let merge = repo.commit(&head.to_hex()).unwrap();
    let main_side = repo.commit(&merge.parents()[0].to_hex()).unwrap();
    let review = merge.parents()[1];
    let edit = *main_side.parent().unwrap();
    let initial = *repo.commit(&edit.to_hex()).unwrap().parent().unwrap();
    (initial, edit, review, head)
}

#[test]
fn packed_repository_yields_changes_documents_and_numbered_lines() {
    let temp = document_repository();
    let repo = Repository::open(temp.path()).unwrap();
    assert!(!repo.git_dir().join("refs/heads/main").exists());
    let (initial, edit, review, merge) = commits(&repo);

    let result = compare(&repo, Some(&initial), &edit);
    let joined = result.join("\n");
    assert!(
        joined.contains("R Some(\"tools/build.sh\") \"build.sh\" Some(Regular) Some(Executable)"),
        "{}",
        joined
    );
    assert!(joined.contains("A None \"spec-link\" None Some(Symlink)"));
    assert!(joined.contains("A None \"vendor/lib\" None Some(Submodule)"));
    assert!(joined.contains("NonText(ContainsNul)"));
    assert!(joined.contains("@@ -1,4 +1,5 @@"));
    assert!(joined.contains("Removed Some(3) None 概要"));
    assert!(joined.contains("Added None Some(3) 概要を更新"));
    assert!(joined.contains("Added None Some(5) 追記"));

    // The document texts themselves, old and new, from the pack.
    let changes = repo
        .diff_trees(Some(&tree_of(&repo, &initial)), &tree_of(&repo, &edit))
        .unwrap();
    let doc = changes
        .deltas()
        .iter()
        .find(|d| d.path() == Path::new("docs/仕様.md"))
        .unwrap();
    let old = repo.blob(&doc.old_oid().unwrap().to_hex()).unwrap();
    let new = repo.blob(&doc.new_oid().unwrap().to_hex()).unwrap();
    assert_eq!(old.content_str().unwrap(), "# 仕様\n\n概要\n詳細\n");
    assert_eq!(
        new.content_str().unwrap(),
        "# 仕様\n\n概要を更新\n詳細\n追記\n"
    );

    // Initial commit: compare with the empty tree; every file is added.
    let root = compare(&repo, None, &initial);
    assert!(root.iter().filter(|l| l.starts_with("A ")).count() == 2);
    assert!(root.iter().any(|l| l == "@@ -0,0 +1,4 @@"));

    // Merge: first parent by default, the merged branch when chosen explicitly.
    let merge_commit = repo.commit(&merge.to_hex()).unwrap();
    let first_parent = merge_commit.parents()[0];
    let default = repo.commit_diff(&merge_commit).unwrap();
    assert_eq!(default.len(), 1);
    assert_eq!(default.deltas()[0].path(), Path::new("docs/review.md"));
    assert_eq!(
        compare(&repo, Some(&first_parent), &merge)[0],
        "A None \"docs/review.md\" None Some(Regular)"
    );
    // Against the merged branch, only the main side's change appears.
    let against_review = compare(&repo, Some(&review), &merge);
    assert_eq!(
        against_review[0],
        "A None \"docs/main.md\" None Some(Regular)"
    );
    assert!(!against_review.iter().any(|l| l.contains("review.md")));
}

#[test]
fn pinned_oids_survive_reference_updates_and_gc() {
    let temp = document_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let (initial, edit, _, _) = commits(&repo);
    let before = compare(&repo, Some(&initial), &edit);

    // Branches move, history is rewritten and objects are repacked meanwhile.
    write(dir, "docs/仕様.md", b"rewritten\n");
    git(dir, &["commit", "-q", "-am", "Later"]);
    git(dir, &["branch", "-f", "review", &initial.to_hex()]);
    git(dir, &["reset", "-q", "--hard", &initial.to_hex()]);
    git(dir, &["gc", "-q"]);
    git(dir, &["pack-refs", "--all", "--prune"]);

    assert_eq!(*repo.head().unwrap().oid(), initial);
    assert_eq!(compare(&repo, Some(&initial), &edit), before);
    assert_eq!(
        compare(&Repository::open(dir).unwrap(), Some(&initial), &edit),
        before
    );
}

#[test]
fn failures_and_skips_are_never_shown_as_empty_diffs() {
    let temp = document_repository();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let (initial, edit, _, _) = commits(&repo);
    let changes = repo
        .diff_trees(Some(&tree_of(&repo, &initial)), &tree_of(&repo, &edit))
        .unwrap();
    let doc = changes
        .deltas()
        .iter()
        .find(|d| d.path() == Path::new("docs/仕様.md"))
        .unwrap()
        .clone();

    // Limits: an explicit skip, never an empty hunk list.
    let tiny = DiffOptions::new().max_input_size(4);
    let skipped = repo
        .diff_blobs(doc.old_oid(), doc.new_oid(), &tiny)
        .unwrap();
    assert_eq!(
        skipped.content(),
        &BlobDiffContent::Skipped(SkipReason::InputTooLarge { limit: 4 })
    );
    assert_eq!(skipped.hunks(), None);
    assert!(!skipped.is_identical());

    // A gitlink is not a blob of this repository: reading it is an error.
    let gitlink = changes
        .deltas()
        .iter()
        .find(|d| d.new_mode() == Some(FileMode::Submodule))
        .unwrap();
    assert!(repo
        .diff_blobs(None, gitlink.new_oid(), &DiffOptions::new())
        .is_err());

    // Corruption inside the pack surfaces as an error, not an empty document.
    let pack_dir = repo.git_dir().join("objects/pack");
    let idx = fs::read_dir(&pack_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "idx"))
        .unwrap();
    let entry = *PackIndex::read(&idx)
        .unwrap()
        .get(doc.new_oid().unwrap())
        .unwrap();
    let pack = idx.with_extension("pack");
    let mut permissions = fs::metadata(&pack).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&pack, permissions).unwrap();
    let mut data = fs::read(&pack).unwrap();
    data[entry.offset as usize + 4] ^= 0xff;
    fs::write(&pack, data).unwrap();
    let fresh = Repository::open(dir).unwrap();
    let result = fresh.diff_blobs(doc.old_oid(), doc.new_oid(), &DiffOptions::new());
    assert!(
        matches!(result, Err(Error::InvalidPack { .. })),
        "{:?}",
        result.map(|d| d.content().clone())
    );

    // An unsupported repository format fails at open instead of looking empty.
    let sha256 = TempDir::new().unwrap();
    git(sha256.path(), &["init", "-q", "--object-format=sha256"]);
    assert!(matches!(
        Repository::open(sha256.path()),
        Err(Error::UnsupportedRepositoryFormat(_))
    ));
}
