//! `.gitignore` handling compared with Git (#20).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Error, FileStatus, Repository};

fn lines(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output.lines().map(str::to_owned).collect();
    lines.sort();
    lines
}

fn zerogit_untracked(repo: &Repository) -> Vec<String> {
    let mut paths: Vec<String> = repo
        .status()
        .unwrap()
        .into_iter()
        .filter(|e| e.status() == FileStatus::Untracked)
        .map(|e| e.path().to_string_lossy().replace('\\', "/"))
        .collect();
    paths.sort();
    paths
}

/// A working tree exercising nested files, negation, directory-only
/// patterns, `**`, anchoring, escapes and `.git/info/exclude`.
fn fixture() -> tempfile::TempDir {
    let temp = repository(&["README.md"]);
    let dir = temp.path();
    write(
        dir,
        ".gitignore",
        "# build output\n\
         *.log\n\
         !important.log\n\
         /root-only.txt\n\
         build/\n\
         **/cache/**\n\
         docs/**/*.tmp\n\
         \\#literal\n\
         trailing.txt   \n\
         space\\ \n\
         *.o\n\
         [Tt]emp*\n",
    );
    write(
        dir,
        "sub/.gitignore",
        "!keep.o\nlocal-only\n/anchored.txt\n",
    );
    write(dir, ".git/info/exclude", "secret.env\n");
    for path in [
        "app.log",
        "important.log",
        "sub/nested.log",
        "root-only.txt",
        "sub/root-only.txt",
        "build/out.bin",
        "sub/build/out.bin",
        "build.txt",
        "a/cache/x",
        "a/b/cache/y/z",
        "cachefile",
        "docs/a.tmp",
        "docs/x/y/b.tmp",
        "other/c.tmp",
        "#literal",
        "trailing.txt",
        "space ",
        "main.o",
        "sub/keep.o",
        "sub/local-only",
        "local-only",
        "sub/anchored.txt",
        "sub/deeper/anchored.txt",
        "Temp1",
        "temp2",
        "secret.env",
        "sub/secret.env",
        ".github/workflows/ci.yml",
        ".env.example",
        ".editorconfig",
        "src/lib.rs",
    ] {
        if cfg!(windows) && path.ends_with(' ') {
            continue;
        }
        write(dir, path, path);
    }
    temp
}

#[test]
fn untracked_files_match_git_exclude_standard() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let expected = lines(&git(dir, &["ls-files", "--others", "--exclude-standard"]));
    assert_eq!(zerogit_untracked(&repo), expected);
}

#[test]
fn ignored_files_match_git() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let expected = lines(&git(
        dir,
        &["ls-files", "--others", "--ignored", "--exclude-standard"],
    ));
    let actual: Vec<String> = repo
        .ignored_files()
        .unwrap()
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn add_all_matches_git_add_all() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    repo.add_all().unwrap();
    let ours = git_ls_files(dir);

    git(dir, &["read-tree", "HEAD"]);
    git(dir, &["add", "-A"]);
    assert_eq!(ours, git_ls_files(dir));
}

#[test]
fn dotfiles_are_reported_and_added() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let untracked = zerogit_untracked(&repo);
    for path in [".github/workflows/ci.yml", ".env.example", ".editorconfig"] {
        assert!(untracked.contains(&path.to_owned()), "{}", path);
    }
    repo.add_all().unwrap();
    let staged = git(dir, &["ls-files"]);
    assert!(staged.contains(".github/workflows/ci.yml"));
}

#[test]
fn tracked_ignored_file_changes_are_reported() {
    let temp = repository(&["config.local", "README.md"]);
    let dir = temp.path();
    write(dir, ".gitignore", "config.local\n");
    git(dir, &["add", ".gitignore"]);
    git(dir, &["commit", "-q", "-m", "Ignore"]);
    write(dir, "config.local", "changed\n");

    let repo = Repository::open(dir).unwrap();
    let status = repo.status().unwrap();
    assert!(status
        .iter()
        .any(|e| e.path() == Path::new("config.local") && e.status() == FileStatus::Modified));

    // add_all keeps tracking it and stages the change, as git add -A does.
    repo.add_all().unwrap();
    let ours = git_ls_files(dir);
    git(dir, &["read-tree", "HEAD"]);
    git(dir, &["add", "-A"]);
    assert_eq!(ours, git_ls_files(dir));
}

#[test]
fn is_ignored_matches_check_ignore() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    for path in [
        "app.log",
        "important.log",
        "build/out.bin",
        "build",
        "a/b/cache/y/z",
        "docs/x/y/b.tmp",
        "other/c.tmp",
        "sub/keep.o",
        "sub/anchored.txt",
        "sub/deeper/anchored.txt",
        "secret.env",
        "src/lib.rs",
        "README.md",
    ] {
        let git_says = git_output(dir, &["check-ignore", "-q", "--no-index", path])
            .status
            .success();
        assert_eq!(repo.is_ignored(path).unwrap(), git_says, "{}", path);
    }
}

#[test]
fn add_rejects_ignored_untracked_file_unless_forced() {
    let temp = fixture();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let before = fs::read(dir.join(".git/index")).unwrap();
    assert!(matches!(repo.add("app.log"), Err(Error::IgnoredPath(_))));
    assert_eq!(fs::read(dir.join(".git/index")).unwrap(), before);
    assert!(!git_output(dir, &["add", "app.log"]).status.success());

    repo.add_force("app.log").unwrap();
    assert!(git(dir, &["ls-files"]).contains("app.log"));
    // Once tracked, a plain add works.
    write(dir, "app.log", "more\n");
    repo.add("app.log").unwrap();
}

#[test]
fn core_excludes_file_is_used() {
    let temp = repository(&["README.md"]);
    let dir = temp.path();
    let excludes = dir.join(".git").join("my-excludes");
    fs::write(&excludes, "*.bak\n").unwrap();
    git(
        dir,
        &[
            "config",
            "core.excludesFile",
            &excludes.to_string_lossy().replace('\\', "/"),
        ],
    );
    write(dir, "file.bak", "x");
    write(dir, "file.txt", "x");
    let repo = Repository::open(dir).unwrap();
    let expected = lines(&git(dir, &["ls-files", "--others", "--exclude-standard"]));
    assert_eq!(expected, ["file.txt"]);
    assert_eq!(zerogit_untracked(&repo), expected);
}
