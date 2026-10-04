//! Helpers shared by integration tests that compare zerogit with Git.
//!
//! Git is used only to create fixtures and to check the results.

#![allow(dead_code)]

pub mod fixtures;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Runs Git in `dir` with an isolated configuration and returns its output.
pub fn git_output(dir: &Path, args: &[&str]) -> Output {
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

/// Runs Git in `dir`, asserts it succeeded and returns its standard output.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Writes a file in the working tree, creating parent directories.
pub fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Creates an empty repository on `main` with line ending conversion off.
pub fn empty_repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    temp
}

/// Creates a repository on `main` with one commit containing `files`.
pub fn repository(files: &[&str]) -> TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    for path in files {
        write(dir, path, &format!("content of {}\n", path));
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Initial"]);
    temp
}

/// `git ls-files -s` lines, in the order Git prints them.
pub fn git_ls_files(dir: &Path) -> Vec<String> {
    git(dir, &["ls-files", "-s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Asserts that `git fsck --strict` finds no problems.
pub fn assert_fsck(dir: &Path) {
    git(dir, &["fsck", "--strict", "--no-dangling"]);
}

/// Creates a repository whose `main` branch has a merge conflict on
/// `file.txt` (both sides modified) left in the index by `git merge`.
pub fn conflicted_repository() -> TempDir {
    let temp = repository(&["file.txt", "other.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "other"]);
    write(dir, "file.txt", "other side\n");
    git(dir, &["commit", "-q", "-am", "Other"]);
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "file.txt", "main side\n");
    git(dir, &["commit", "-q", "-am", "Main"]);
    assert!(!git_output(dir, &["merge", "-q", "other"]).status.success());
    temp
}

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Copies a prepared repository so Git and zerogit can each merge in one.
pub fn twin(source: &Path) -> TempDir {
    let temp = TempDir::new().unwrap();
    copy_dir(source, temp.path());
    temp
}

/// Files of the work tree (excluding .git) with their contents.
pub fn worktree_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.push((
                    rel.to_string_lossy().replace('\\', "/"),
                    fs::read(&path).unwrap(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}
