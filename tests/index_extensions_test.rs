//! Optional index extensions survive zerogit's writes: the cache tree
//! (`TREE`) keeps unchanged directories, the resolve-undo data (`REUC`)
//! lets `git checkout -m` recreate a resolved conflict, and extensions that
//! cannot be kept up to date (`UNTR`) are dropped.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;

use common::{conflicted_repository, git, repository, write};
use zerogit::Repository;

/// Finds an extension in the raw index file and returns its data.
fn extension(dir: &Path, signature: &[u8; 4]) -> Option<Vec<u8>> {
    let data = std::fs::read(dir.join(".git/index")).unwrap();
    let body = &data[..data.len() - 20];
    // Extensions follow the entries; the test paths never contain these
    // upper-case signatures, so the first match is the extension.
    let pos = body.windows(4).position(|w| w == signature)?;
    let size = u32::from_be_bytes(body[pos + 4..pos + 8].try_into().unwrap()) as usize;
    Some(body[pos + 8..pos + 8 + size].to_vec())
}

/// The cache tree as `(path, entry count)` pairs; -1 marks an invalid node.
fn cache_tree(dir: &Path) -> Vec<(String, i64)> {
    fn node(data: &[u8], pos: &mut usize, prefix: &str, out: &mut Vec<(String, i64)>) {
        let nul = data[*pos..].iter().position(|&b| b == 0).unwrap();
        let name = String::from_utf8_lossy(&data[*pos..*pos + nul]).into_owned();
        *pos += nul + 1;
        let newline = data[*pos..].iter().position(|&b| b == b'\n').unwrap();
        let header = std::str::from_utf8(&data[*pos..*pos + newline]).unwrap();
        *pos += newline + 1;
        let (count, subtrees) = header.split_once(' ').unwrap();
        let count: i64 = count.parse().unwrap();
        if count >= 0 {
            *pos += 20;
        }
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };
        out.push((path.clone(), count));
        for _ in 0..subtrees.parse::<usize>().unwrap() {
            node(data, pos, &path, out);
        }
    }
    let Some(data) = extension(dir, b"TREE") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    node(&data, &mut 0, "", &mut out);
    out.sort();
    out
}

/// The tree Git builds from the index entries alone, without the cache.
fn tree_without_cache(dir: &Path) -> String {
    let fresh = dir.join(".git/fresh-index");
    let listing = git(dir, &["ls-files", "-s"]);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["update-index", "--index-info"])
        .env("GIT_INDEX_FILE", &fresh)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(listing.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let tree = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("write-tree")
        .env("GIT_INDEX_FILE", &fresh)
        .output()
        .unwrap();
    std::fs::remove_file(&fresh).unwrap();
    String::from_utf8(tree.stdout).unwrap()
}

#[test]
fn cache_tree_of_unchanged_directories_survives() {
    let temp = repository(&["a/one.txt", "a/deep/two.txt", "b/three.txt", "top.txt"]);
    let dir = temp.path();
    // `git commit` leaves a complete cache tree.
    assert!(cache_tree(dir).iter().all(|(_, count)| *count >= 0));

    write(dir, "b/three.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("b/three.txt").unwrap();

    assert_eq!(
        cache_tree(dir),
        [
            ("".to_owned(), -1),
            ("a".to_owned(), 2),
            ("a/deep".to_owned(), 1),
            ("b".to_owned(), -1),
        ]
    );
    // Git trusts the cache tree in write-tree: it must match the entries.
    assert_eq!(git(dir, &["write-tree"]), tree_without_cache(dir));
    git(dir, &["commit", "-q", "-m", "Change b"]);
    assert_eq!(
        git(dir, &["rev-parse", "HEAD^{tree}"]),
        tree_without_cache(dir)
    );
}

#[test]
fn file_replacing_a_directory_drops_its_cache() {
    let temp = repository(&["a/one.txt", "b.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    std::fs::remove_dir_all(dir.join("a")).unwrap();
    write(dir, "a", "now a file\n");
    repo.add_all().unwrap();
    assert!(cache_tree(dir).iter().all(|(path, _)| path != "a"));
    assert_eq!(git(dir, &["write-tree"]), tree_without_cache(dir));
}

#[test]
fn zerogit_commit_writes_a_complete_cache_tree() {
    let temp = repository(&["a/one.txt", "a/deep/two.txt", "b/three.txt"]);
    let dir = temp.path();
    write(dir, "a/deep/two.txt", "changed\n");
    write(dir, "c/new.txt", "new\n");
    let repo = Repository::open(dir).unwrap();
    repo.add_all().unwrap();
    let commit = repo.create_commit("Change", "T", "t@example.com").unwrap();

    let tree = cache_tree(dir);
    assert!(tree.iter().all(|(_, count)| *count >= 0), "{:?}", tree);
    assert_eq!(tree[0], ("".to_owned(), 4));
    let head_tree = git(
        dir,
        &["rev-parse", &format!("{}^{{tree}}", commit.to_hex())],
    );
    assert_eq!(git(dir, &["write-tree"]), head_tree);
    assert_eq!(tree_without_cache(dir), head_tree);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");

    // The next commit reuses the cached trees and still matches Git.
    write(dir, "b/three.txt", "changed too\n");
    repo.add("b/three.txt").unwrap();
    let next = repo.create_commit("Again", "T", "t@example.com").unwrap();
    assert_eq!(
        git(dir, &["rev-parse", &format!("{}^{{tree}}", next.to_hex())]),
        tree_without_cache(dir)
    );
    common::assert_fsck(dir);
}

#[test]
fn intent_to_add_entries_keep_the_tree_correct() {
    let temp = repository(&["a/one.txt"]);
    let dir = temp.path();
    write(dir, "a/later.txt", "later\n");
    git(dir, &["add", "-N", "a/later.txt"]);
    write(dir, "a/one.txt", "changed\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("a/one.txt").unwrap();
    let commit = repo.create_commit("Change", "T", "t@example.com").unwrap();
    // The intent-to-add file is not committed.
    assert_eq!(
        git(dir, &["ls-tree", "-r", "--name-only", &commit.to_hex()]),
        "a/one.txt\n"
    );
    assert_eq!(git(dir, &["status", "--porcelain"]), " A a/later.txt\n");
}

#[test]
fn resolved_conflict_can_be_recreated_with_checkout_m() {
    let temp = conflicted_repository();
    let dir = temp.path();
    write(dir, "file.txt", "resolved\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("file.txt").unwrap();
    assert!(extension(dir, b"REUC").is_some());
    assert_eq!(git(dir, &["ls-files", "-u"]), "");

    git(dir, &["checkout", "-m", "file.txt"]);
    let stages: Vec<String> = git(dir, &["ls-files", "-u"])
        .lines()
        .map(|l| {
            l.split('\t')
                .next()
                .unwrap()
                .rsplit(' ')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(stages, ["1", "2", "3"]);
    let content = std::fs::read_to_string(dir.join("file.txt")).unwrap();
    assert!(content.contains("<<<<<<<"), "{}", content);
}

#[test]
fn committing_forgets_resolved_conflicts() {
    let temp = conflicted_repository();
    let dir = temp.path();
    write(dir, "file.txt", "resolved\n");
    let repo = Repository::open(dir).unwrap();
    repo.add("file.txt").unwrap();
    repo.create_commit("Merge", "T", "t@example.com").unwrap();
    assert!(extension(dir, b"REUC").is_none());
}

#[test]
fn untracked_cache_is_dropped_and_git_rebuilds_it() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["config", "core.untrackedCache", "true"]);
    git(dir, &["update-index", "--untracked-cache"]);
    write(dir, "new.txt", "new\n");
    git(dir, &["status", "--porcelain"]);
    assert!(extension(dir, b"UNTR").is_some());

    let repo = Repository::open(dir).unwrap();
    write(dir, "a.txt", "changed\n");
    repo.add("a.txt").unwrap();
    assert!(extension(dir, b"UNTR").is_none());
    // Git reads the index and rebuilds its cache with the right result.
    write(dir, "other.txt", "other\n");
    assert_eq!(
        git(dir, &["status", "--porcelain"]),
        "M  a.txt\n?? new.txt\n?? other.txt\n"
    );
}
