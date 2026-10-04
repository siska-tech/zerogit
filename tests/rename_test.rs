//! Exact rename detection with mode changes, compared with `git diff --raw`.

use std::path::Path;
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::{DiffStatus, FileMode, RenameDetection, RenameOptions, Repository};

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

fn repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "core.autocrlf", "false"]);
    temp
}

fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["commit", "-q", "-m", message]);
}

/// Returns `(status, old path, path, old mode, new mode)` for HEAD's diff.
fn head_diff(dir: &Path) -> Vec<(DiffStatus, Option<String>, String, String, String)> {
    let repo = Repository::open(dir).unwrap();
    let head = repo.commit(&repo.head().unwrap().oid().to_hex()).unwrap();
    let octal = |mode: Option<FileMode>| mode.map_or("000000", |m| m.as_octal()).to_owned();
    let slash = |path: &Path| path.to_str().unwrap().replace('\\', "/");
    repo.commit_diff(&head)
        .unwrap()
        .deltas()
        .iter()
        .map(|d| {
            (
                d.status(),
                d.old_path().map(slash),
                slash(d.path()),
                octal(d.old_mode()),
                octal(d.new_mode()),
            )
        })
        .collect()
}

/// Parses `git diff --raw` lines for HEAD into the same shape.
fn git_raw(dir: &Path) -> Vec<(DiffStatus, Option<String>, String, String, String)> {
    let raw = git(
        dir,
        &["diff", "--raw", "--no-abbrev", "-M100%", "HEAD~1", "HEAD"],
    );
    let normalize = |mode: &str| if mode == "040000" { "40000" } else { mode }.to_owned();
    let mut rows: Vec<_> = raw
        .lines()
        .map(|line| {
            let (meta, paths) = line.split_once('\t').unwrap();
            let fields: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
            let paths: Vec<&str> = paths.split('\t').collect();
            let (status, old_path, path) = match &fields[4][..1] {
                "R" => (
                    DiffStatus::Renamed,
                    Some(paths[0].to_owned()),
                    paths[1].to_owned(),
                ),
                "A" => (DiffStatus::Added, None, paths[0].to_owned()),
                "D" => (DiffStatus::Deleted, None, paths[0].to_owned()),
                "M" | "T" => (DiffStatus::Modified, None, paths[0].to_owned()),
                other => panic!("unexpected status {}", other),
            };
            (
                status,
                old_path,
                path,
                normalize(fields[0]),
                normalize(fields[1]),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.2.cmp(&b.2));
    rows
}

#[test]
fn rename_with_executable_bit_change_keeps_both_modes() {
    let temp = repository();
    let dir = temp.path();
    write(dir, "run.sh", "#!/bin/sh\necho run\n");
    write(dir, "plain.txt", "unchanged mode\n");
    write(dir, "mode_only.sh", "echo mode\n");
    git(dir, &["add", "-A"]);
    commit(dir, "Initial");

    git(dir, &["mv", "run.sh", "bin_run.sh"]);
    git(dir, &["update-index", "--chmod=+x", "bin_run.sh"]);
    git(dir, &["mv", "plain.txt", "moved.txt"]);
    git(dir, &["update-index", "--chmod=+x", "mode_only.sh"]);
    commit(dir, "Move and chmod");

    let actual = head_diff(dir);
    assert_eq!(actual, git_raw(dir));
    assert!(actual.contains(&(
        DiffStatus::Renamed,
        Some("run.sh".into()),
        "bin_run.sh".into(),
        "100644".into(),
        "100755".into()
    )));
    assert!(actual.contains(&(
        DiffStatus::Modified,
        None,
        "mode_only.sh".into(),
        "100644".into(),
        "100755".into()
    )));
}

#[test]
fn duplicate_contents_pair_one_to_one_and_deterministically() {
    let temp = repository();
    let dir = temp.path();
    for path in ["a/same.txt", "b/other.txt", "c/third.txt"] {
        write(dir, path, "identical content\n");
    }
    git(dir, &["add", "-A"]);
    commit(dir, "Initial");
    git(dir, &["rm", "-q", "-r", "a", "b", "c"]);
    for path in ["x/same.txt", "y/fresh.txt"] {
        write(dir, path, "identical content\n");
    }
    git(dir, &["add", "-A"]);
    commit(dir, "Shuffle");

    let first = head_diff(dir);
    assert_eq!(first, head_diff(dir));
    let renamed: Vec<_> = first
        .iter()
        .filter(|d| d.0 == DiffStatus::Renamed)
        .map(|d| (d.1.clone().unwrap(), d.2.clone()))
        .collect();
    assert_eq!(
        renamed,
        [
            ("a/same.txt".to_owned(), "x/same.txt".to_owned()),
            ("b/other.txt".to_owned(), "y/fresh.txt".to_owned())
        ]
    );
    let deleted: Vec<_> = first
        .iter()
        .filter(|d| d.0 == DiffStatus::Deleted)
        .map(|d| d.2.clone())
        .collect();
    assert_eq!(deleted, ["c/third.txt"]);
}

#[test]
fn symlinks_and_gitlinks_only_rename_to_their_own_kind() {
    let temp = repository();
    let dir = temp.path();
    // A regular file whose content equals a symlink target, and a gitlink.
    write(dir, "plain.txt", "target");
    git(dir, &["add", "plain.txt"]);
    let blob = git(dir, &["hash-object", "plain.txt"]).trim().to_owned();
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{},link", blob),
        ],
    );
    commit(dir, "Initial");
    let head = git(dir, &["rev-parse", "HEAD"]).trim().to_owned();
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},sub", head),
        ],
    );
    commit(dir, "Add gitlink");

    // plain.txt becomes a symlink elsewhere; link and sub move.
    git(dir, &["rm", "-q", "--cached", "plain.txt", "link", "sub"]);
    for (mode, path) in [("120000", "from_plain"), ("120000", "moved_link")] {
        git(
            dir,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("{},{},{}", mode, blob, path),
            ],
        );
    }
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},moved_sub", head),
        ],
    );
    commit(dir, "Move");

    let actual = head_diff(dir);
    assert_eq!(actual, git_raw(dir));
    // The symlink pairs with the first symlink by path; the regular file with
    // identical content never pairs with a symlink.
    let summary: Vec<_> = actual
        .iter()
        .map(|d| (d.0, d.1.as_deref(), d.2.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (DiffStatus::Renamed, Some("link"), "from_plain"),
            (DiffStatus::Added, None, "moved_link"),
            (DiffStatus::Renamed, Some("sub"), "moved_sub"),
            (DiffStatus::Deleted, None, "plain.txt"),
        ]
    );
}

fn document(lines: std::ops::Range<usize>) -> String {
    lines.map(|i| format!("段落 {:03} の本文\n", i)).collect()
}

/// `(old path, new path)` pairs that Git reports as renames with `-M50%`.
fn git_rename_pairs(dir: &Path) -> Vec<(String, String)> {
    let out = git(dir, &["diff", "--name-status", "-M50%", "HEAD~1", "HEAD"]);
    let mut pairs: Vec<_> = out
        .lines()
        .filter(|line| line.starts_with('R'))
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            (fields[1].to_owned(), fields[2].to_owned())
        })
        .collect();
    pairs.sort_by(|a, b| a.1.cmp(&b.1));
    pairs
}

#[test]
fn similarity_renames_pair_edited_moves_like_git() {
    let temp = repository();
    let dir = temp.path();
    write(dir, "docs/guide.md", &document(0..40));
    write(dir, "docs/notes.md", &document(100..130));
    write(dir, "unrelated.md", &document(200..210));
    git(dir, &["add", "-A"]);
    commit(dir, "Initial");

    // Move and edit two documents; replace the third entirely.
    fs::remove_file(dir.join("docs/guide.md")).unwrap();
    fs::remove_file(dir.join("docs/notes.md")).unwrap();
    fs::remove_file(dir.join("unrelated.md")).unwrap();
    write(
        dir,
        "manual/guide.md",
        &(document(0..36) + "追記した段落\n"),
    );
    write(
        dir,
        "archive/notes.md",
        &(document(100..125) + &document(500..505)),
    );
    write(dir, "fresh.md", &document(300..310));
    git(dir, &["add", "-A"]);
    commit(dir, "Reorganize");

    let repo = Repository::open(dir).unwrap();
    let head = repo.commit(&repo.head().unwrap().oid().to_hex()).unwrap();
    let similar = RenameOptions::new().detection(RenameDetection::Similar);
    let diff = repo.commit_diff_with_options(&head, &similar).unwrap();
    assert!(diff.rename_limits().is_empty());
    let slash = |p: &Path| p.to_str().unwrap().replace('\\', "/");
    let mut pairs: Vec<_> = diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Renamed)
        .map(|d| (slash(d.old_path().unwrap()), slash(d.path())))
        .collect();
    pairs.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(pairs, git_rename_pairs(dir));
    assert_eq!(pairs.len(), 2);
    for delta in diff
        .deltas()
        .iter()
        .filter(|d| d.status() == DiffStatus::Renamed)
    {
        assert_ne!(delta.old_oid(), delta.new_oid());
        assert!(delta.similarity().unwrap() >= 50);
        // The line diff of an edited move uses both OIDs.
        let blob_diff = repo
            .diff_blobs(
                delta.old_oid(),
                delta.new_oid(),
                &zerogit::DiffOptions::new(),
            )
            .unwrap();
        assert!(blob_diff.lines_added().unwrap() > 0);
    }
    assert_eq!(diff.stats().added, 1);
    assert_eq!(diff.stats().deleted, 1);

    // Default and Off keep additions and deletions; results survive packing.
    for options in [
        RenameOptions::new(),
        RenameOptions::new().detection(RenameDetection::Off),
    ] {
        let plain = repo.commit_diff_with_options(&head, &options).unwrap();
        assert_eq!(plain.stats().renamed, 0);
        assert_eq!(plain.stats().added, 3);
        assert_eq!(plain.stats().deleted, 3);
    }
    assert_eq!(
        format!("{:?}", repo.commit_diff(&head).unwrap().deltas()),
        format!(
            "{:?}",
            repo.commit_diff_with_options(&head, &RenameOptions::new())
                .unwrap()
                .deltas()
        )
    );
    git(dir, &["repack", "-a", "-d", "-q"]);
    let packed = Repository::open(dir).unwrap();
    assert_eq!(
        format!(
            "{:?}",
            packed
                .commit_diff_with_options(&head, &similar)
                .unwrap()
                .deltas()
        ),
        format!("{:?}", diff.deltas())
    );
}

#[test]
fn similarity_limits_are_reported_and_keep_changes() {
    let temp = repository();
    let dir = temp.path();
    write(dir, "a.md", &document(0..20));
    write(dir, "b.md", &document(20..40));
    git(dir, &["add", "-A"]);
    commit(dir, "Initial");
    fs::remove_file(dir.join("a.md")).unwrap();
    fs::remove_file(dir.join("b.md")).unwrap();
    write(dir, "moved/a.md", &(document(0..19) + "edit\n"));
    write(dir, "moved/b.md", &(document(20..39) + "edit\n"));
    git(dir, &["add", "-A"]);
    commit(dir, "Move");

    let repo = Repository::open(dir).unwrap();
    let head = repo.commit(&repo.head().unwrap().oid().to_hex()).unwrap();
    let limited = RenameOptions::new()
        .detection(RenameDetection::Similar)
        .max_pairs(3);
    let diff = repo.commit_diff_with_options(&head, &limited).unwrap();
    assert_eq!(
        diff.rename_limits(),
        [zerogit::RenameLimit::TooManyPairs { pairs: 4, limit: 3 }]
    );
    assert_eq!(diff.stats().added, 2);
    assert_eq!(diff.stats().deleted, 2);
}
