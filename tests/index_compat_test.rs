//! Index versions 2-4 and extensions written by Git, read and rewritten by zerogit.
//!
//! Git is used only to create indexes and to check the results.

use std::path::Path;
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::index::parse;
use zerogit::{Error, FileStatus, Repository};

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Background auto-maintenance would rewrite files concurrently with the test.
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
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn repository(files: &[&str]) -> TempDir {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    for path in files {
        write(dir, path, &format!("content of {}\n", path));
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Initial"]);
    temp
}

fn index_bytes(dir: &Path) -> Vec<u8> {
    fs::read(dir.join(".git/index")).unwrap()
}

fn index_version(dir: &Path) -> u32 {
    let data = index_bytes(dir);
    u32::from_be_bytes([data[4], data[5], data[6], data[7]])
}

/// `git ls-files -s` lines, in the order Git prints them.
fn git_ls_files(dir: &Path) -> Vec<String> {
    git(dir, &["ls-files", "-s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The same lines from zerogit's parse of the index file.
fn zerogit_ls_files(dir: &Path) -> Vec<String> {
    parse(&index_bytes(dir))
        .unwrap()
        .entries()
        .iter()
        .map(|e| {
            format!(
                "{} {} {}\t{}",
                e.mode().as_octal(),
                e.oid(),
                e.stage(),
                e.path().to_str().unwrap().replace('\\', "/")
            )
        })
        .collect()
}

/// Git can still read the index and the repository after zerogit wrote it.
fn assert_git_healthy(dir: &Path) {
    git(dir, &["status", "--porcelain"]);
    git(dir, &["fsck", "--no-dangling"]);
    assert_eq!(zerogit_ls_files(dir), git_ls_files(dir));
}

const LONG_PREFIX: &str = "documents/projects/2026/specification/chapters/section";

fn sample_files() -> Vec<String> {
    let mut files = vec![
        "README.md".to_owned(),
        "docs/設計書.md".to_owned(),
        "docs/設計書の補足.md".to_owned(),
        "src/lib.rs".to_owned(),
    ];
    for i in 0..5 {
        files.push(format!("{}/{:02}/本文.md", LONG_PREFIX, i));
    }
    files
}

#[test]
fn git_written_indexes_match_ls_files_in_every_version() {
    for files in [vec!["README.md".to_owned()], sample_files()] {
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let temp = repository(&refs);
        let dir = temp.path();
        for version in ["2", "3", "4"] {
            git(dir, &["update-index", "--index-version", version]);
            // Git writes v2 when "3" is requested but no entry needs extended flags.
            if version != "3" {
                assert_eq!(index_version(dir).to_string(), version);
            }
            assert_eq!(zerogit_ls_files(dir), git_ls_files(dir), "v{}", version);
        }
    }
}

#[test]
fn conflict_stages_are_read() {
    let temp = repository(&["file.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "other"]);
    write(dir, "file.txt", "other side\n");
    git(dir, &["commit", "-q", "-am", "Other"]);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "file.txt", "main side\n");
    git(dir, &["commit", "-q", "-am", "Main"]);
    assert!(!git_output(dir, &["merge", "-q", "other"]).status.success());
    git(dir, &["update-index", "--index-version", "4"]);
    let stages: Vec<_> = zerogit_ls_files(dir)
        .iter()
        .map(|l| l.split(' ').nth(2).unwrap().to_owned())
        .collect();
    assert_eq!(stages.len(), 3);
    assert_eq!(zerogit_ls_files(dir), git_ls_files(dir));
}

#[test]
fn operations_on_a_v4_index_keep_it_valid_for_git() {
    let files = sample_files();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let temp = repository(&refs);
    let dir = temp.path();
    git(dir, &["update-index", "--index-version", "4"]);
    git(dir, &["branch", "feature"]);
    let repo = Repository::open(dir).unwrap();

    // status: clean, then one modification.
    assert!(repo.status().unwrap().is_empty());
    write(dir, "docs/設計書.md", "edited\n");
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status(), FileStatus::Modified);

    // add (existing and new paths), keeping v4.
    repo.add("docs/設計書.md").unwrap();
    write(dir, &format!("{}/99/追加.md", LONG_PREFIX), "new\n");
    repo.add(format!("{}/99/追加.md", LONG_PREFIX)).unwrap();
    assert_eq!(index_version(dir), 4);
    assert_git_healthy(dir);

    // reset a path and the whole index.
    repo.reset(Some("docs/設計書.md")).unwrap();
    assert_git_healthy(dir);
    repo.reset(None::<&str>).unwrap();
    assert_eq!(index_version(dir), 4);
    assert_git_healthy(dir);

    // commit and checkout.
    repo.add("docs/設計書.md").unwrap();
    repo.add(format!("{}/99/追加.md", LONG_PREFIX)).unwrap();
    repo.create_commit("Edit", "Test", "test@example.com")
        .unwrap();
    assert_git_healthy(dir);
    repo.checkout("feature").unwrap();
    assert_eq!(index_version(dir), 4);
    assert_git_healthy(dir);
    assert!(git(dir, &["status", "--porcelain"]).is_empty());
    repo.checkout("main").unwrap();
    assert_git_healthy(dir);
    assert!(git(dir, &["status", "--porcelain"]).is_empty());
}

#[test]
fn reset_and_checkout_keep_file_modes() {
    let temp = repository(&["run.sh", "plain.txt"]);
    let dir = temp.path();
    // Make the file executable on disk too, so the working tree is clean.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(dir, &["update-index", "--chmod=+x", "run.sh"]);
    git(dir, &["commit", "-q", "-m", "Executable"]);
    git(dir, &["branch", "feature"]);
    let repo = Repository::open(dir).unwrap();
    let before = git_ls_files(dir);
    repo.reset(None::<&str>).unwrap();
    assert_eq!(git_ls_files(dir), before);
    repo.reset(Some("run.sh")).unwrap();
    assert_eq!(git_ls_files(dir), before);
    repo.checkout("feature").unwrap();
    assert_eq!(git_ls_files(dir), before);
    assert!(before
        .iter()
        .any(|l| l.starts_with("100755") && l.ends_with("run.sh")));
}

#[test]
fn intent_to_add_is_kept_and_not_committed() {
    let temp = repository(&["tracked.txt"]);
    let dir = temp.path();
    write(dir, "planned.txt", "not staged yet\n");
    git(dir, &["add", "-N", "planned.txt"]);
    let repo = Repository::open(dir).unwrap();
    let entry = parse(&index_bytes(dir)).unwrap();
    assert!(entry.entries().iter().any(|e| e.intent_to_add()));

    // Rewriting the index keeps the flag; committing leaves the path out.
    write(dir, "tracked.txt", "changed\n");
    repo.add("tracked.txt").unwrap();
    assert!(git(dir, &["diff", "--cached", "--name-only"]).trim() == "tracked.txt");
    repo.create_commit("Commit", "Test", "test@example.com")
        .unwrap();
    assert!(!git(dir, &["ls-tree", "-r", "--name-only", "HEAD"]).contains("planned.txt"));
    assert!(parse(&index_bytes(dir))
        .unwrap()
        .entries()
        .iter()
        .any(|e| e.intent_to_add()));
    assert_git_healthy(dir);
    // Git still treats it as intent-to-add: tracked, not staged.
    assert!(git(dir, &["diff", "--cached", "--name-only"]).is_empty());
}

#[test]
fn skip_worktree_entries_are_kept_and_not_reported_missing() {
    let temp = repository(&["kept.txt", "sparse/away.txt"]);
    let dir = temp.path();
    git(dir, &["update-index", "--skip-worktree", "sparse/away.txt"]);
    fs::remove_file(dir.join("sparse/away.txt")).unwrap();
    let repo = Repository::open(dir).unwrap();
    assert!(repo.status().unwrap().is_empty());
    assert!(repo.diff_index_to_workdir().unwrap().is_empty());

    write(dir, "kept.txt", "changed\n");
    repo.add("kept.txt").unwrap();
    assert!(git(dir, &["ls-files", "-v"]).contains("S sparse/away.txt"));
    assert_git_healthy(dir);

    // Rebuilding the index would end the sparse checkout: refused up front.
    assert!(matches!(
        repo.reset(None::<&str>),
        Err(Error::UnsupportedIndex { .. })
    ));
    git(dir, &["commit", "-q", "-am", "Kept"]);
    git(dir, &["branch", "other"]);
    assert!(matches!(
        repo.checkout("other"),
        Err(Error::UnsupportedIndex { .. })
    ));
    assert!(git(dir, &["ls-files", "-v"]).contains("S sparse/away.txt"));
}

#[test]
fn split_and_sparse_indexes_are_rejected_explicitly() {
    let temp = repository(&["a.txt", "dir/b.txt", "outside/c.txt"]);
    let dir = temp.path();
    git(dir, &["update-index", "--split-index"]);
    let repo = Repository::open(dir).unwrap();
    for result in [repo.status().map(|_| ()), repo.add("a.txt")] {
        match result {
            Err(Error::UnsupportedIndex { reason, .. }) => assert!(reason.contains("split index")),
            other => panic!("expected UnsupportedIndex, got {:?}", other),
        }
    }
    git(dir, &["update-index", "--no-split-index"]);
    assert!(repo.status().is_ok());

    git(
        dir,
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
    );
    // Collapses the committed "outside/" directory into a sparse directory entry.
    git(dir, &["sparse-checkout", "set", "dir"]);
    assert!(git(dir, &["ls-files", "--sparse"]).contains("outside/"));
    for result in [
        parse(&index_bytes(dir)).map(|_| ()),
        repo.status().map(|_| ()),
    ] {
        match result {
            Err(Error::UnsupportedIndex { reason, .. }) => {
                assert!(reason.contains("sparse index"), "{}", reason)
            }
            other => panic!("expected UnsupportedIndex, got {:?}", other),
        }
    }
}

#[test]
fn optional_extensions_and_skip_hash_are_accepted() {
    let temp = repository(&["a.txt", "dir/b.txt"]);
    let dir = temp.path();
    git(dir, &["config", "core.untrackedCache", "true"]);
    git(dir, &["update-index", "--untracked-cache"]);
    write(dir, "untracked.txt", "u\n");
    git(dir, &["status", "--porcelain"]);
    let repo = Repository::open(dir).unwrap();
    assert_eq!(zerogit_ls_files(dir), git_ls_files(dir));
    write(dir, "a.txt", "changed\n");
    repo.add("a.txt").unwrap();
    assert_git_healthy(dir);

    git(dir, &["config", "index.skipHash", "true"]);
    write(dir, "dir/b.txt", "changed\n");
    git(dir, &["add", "dir/b.txt"]);
    let data = index_bytes(dir);
    assert_eq!(data[data.len() - 20..], [0u8; 20]);
    assert_eq!(zerogit_ls_files(dir), git_ls_files(dir));
}
