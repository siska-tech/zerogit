//! `Repository::blame` compared with `git blame --line-porcelain` (#77).

mod common;

use common::*;
use std::path::Path;
use zerogit::{BlameOptions, DiffAlgorithm, Repository};

/// Runs Git with the commit dates set to `time`.
fn git_at(dir: &Path, time: i64, args: &[&str]) -> String {
    let date = format!("{} +0000", time);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.autocrlf=false", "-c", "merge.renames=true"])
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
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

fn commit_all(dir: &Path, time: i64, message: &str) {
    git_at(dir, time, &["add", "-A"]);
    git_at(dir, time, &["commit", "-q", "-m", message]);
}

/// A C-like function.
fn function(name: &str, body: &[&str]) -> String {
    let mut text = format!("int {}(void)\n{{\n", name);
    for line in body {
        text.push_str(&format!("\t{}\n", line));
    }
    text.push_str("}\n\n");
    text
}

/// A history with edits of every kind, renames (with and without
/// changes), merges (one bringing a rename, one changing lines itself),
/// duplicated blocks, a reverted line, a file deleted and added again, and
/// commit dates out of order.
fn history() -> tempfile::TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    let t = 1_700_000_000;
    let main_c = |parts: &[String]| parts.concat();
    let mut parts = vec![
        "#include <stdio.h>\n\n".to_owned(),
        function("one", &["a = 1;", "b = 2;", "return a + b;"]),
        function("two", &["return 2;"]),
        function("main", &["one();", "two();", "return 0;"]),
    ];
    write(dir, "src/main.c", &main_c(&parts));
    write(dir, "README", "Title\n\nSome text.\nMore text.\n");
    write(dir, "src/util.c", &function("util", &["return 42;"]));
    commit_all(dir, t, "Initial");

    // A new function between others, and an edited line.
    parts.insert(2, function("between", &["x = 0;", "return x;"]));
    parts[1] = function("one", &["a = 1;", "b = 3;", "return a + b;"]);
    write(dir, "src/main.c", &main_c(&parts));
    commit_all(dir, t + 10, "Add between");

    // A feature branch: edit the top, then rename with an edit.
    git_at(dir, t + 10, &["checkout", "-q", "-b", "feature"]);
    let mut feature = parts.clone();
    feature[0] = "#include <stdio.h>\n#include <stdlib.h>\n\n".to_owned();
    write(dir, "src/main.c", &main_c(&feature));
    commit_all(dir, t + 20, "Include stdlib");
    git_at(dir, t + 25, &["mv", "src/main.c", "src/app.c"]);
    feature[2] = function("between", &["x = 1;", "return x;"]);
    write(dir, "src/app.c", &main_c(&feature));
    commit_all(dir, t + 25, "Rename to app.c");

    // Main: edit the bottom (dated before the feature commits).
    git_at(dir, t + 15, &["checkout", "-q", "main"]);
    let mut main = parts.clone();
    main[4] = function("main", &["one();", "two();", "between();", "return 0;"]);
    write(dir, "src/main.c", &main_c(&main));
    write(dir, "README", "Title\n\nSome text.\nChanged text.\n");
    commit_all(dir, t + 15, "Call between");

    // Merge the feature (with its rename), changing a line in the merge.
    git_at(dir, t + 30, &["merge", "-q", "--no-commit", "feature"]);
    let mut merged = feature.clone();
    merged[4] = main[4].clone();
    merged[3] = function("two", &["return 22;"]);
    write(dir, "src/app.c", &main_c(&merged));
    commit_all(dir, t + 30, "Merge feature");

    // A block duplicated, a line reverted to an older text.
    merged.insert(3, function("two", &["return 22;"]));
    merged[1] = function("one", &["a = 1;", "b = 2;", "return a + b;"]);
    write(dir, "src/app.c", &main_c(&merged));
    commit_all(dir, t + 40, "Duplicate two, revert b");

    // A pure rename, then a delete and an add again.
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    git_at(dir, t + 50, &["mv", "src/app.c", "lib/app.c"]);
    commit_all(dir, t + 50, "Move to lib");
    git_at(dir, t + 60, &["rm", "-q", "src/util.c"]);
    commit_all(dir, t + 60, "Remove util");
    write(dir, "src/util.c", &function("util", &["return 42;"]));
    commit_all(dir, t + 70, "Add util again");

    // A side branch that does not touch app.c, merged: one parent has
    // the identical file.
    git_at(dir, t + 70, &["checkout", "-q", "-b", "docs"]);
    write(dir, "README", "Title\n\nSome text.\nChanged text.\nDocs.\n");
    commit_all(dir, t + 80, "Docs");
    git_at(dir, t + 85, &["checkout", "-q", "main"]);
    let mut last = merged.clone();
    last.push(function("tail", &["return -1;"]));
    write(dir, "lib/app.c", &main_c(&last));
    commit_all(dir, t + 85, "Add tail");
    git_at(
        dir,
        t + 90,
        &["merge", "-q", "--no-ff", "-m", "Merge docs", "docs"],
    );
    temp
}

/// Each line's commit, original line and original path from
/// `git blame --line-porcelain`.
fn git_blame(dir: &Path, args: &[&str]) -> Vec<(String, usize, String)> {
    let mut command = vec!["blame", "--line-porcelain"];
    command.extend_from_slice(args);
    let output = git(dir, &command);
    let mut result = Vec::new();
    let mut current: Option<(String, usize)> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("filename ") {
            let (commit, original) = current.take().unwrap();
            result.push((commit, original, path.to_owned()));
        } else if !line.starts_with('\t') {
            let fields: Vec<&str> = line.split(' ').collect();
            if fields[0].len() == 40 && fields[0].bytes().all(|b| b.is_ascii_hexdigit()) {
                current = Some((fields[0].to_owned(), fields[1].parse().unwrap()));
            }
        }
    }
    result
}

fn ours(dir: &Path, path: &str, options: &BlameOptions) -> Vec<(String, usize, String)> {
    Repository::open(dir)
        .unwrap()
        .blame(path, options)
        .unwrap()
        .lines()
        .iter()
        .map(|line| {
            (
                line.commit().to_hex(),
                line.original_line(),
                line.path().to_string_lossy().replace('\\', "/"),
            )
        })
        .collect()
}

#[test]
fn blame_matches_git_blame_everywhere() {
    let temp = history();
    let dir = temp.path();
    let commits: Vec<String> = git(dir, &["rev-list", "--all"])
        .lines()
        .map(str::to_owned)
        .collect();
    let mut compared = 0;
    for commit in &commits {
        let files = git(dir, &["ls-tree", "-r", "--name-only", commit]);
        for file in files.lines() {
            let theirs = git_blame(dir, &[commit, "--", file]);
            let options = BlameOptions::new().revision(commit.as_str());
            assert_eq!(ours(dir, file, &options), theirs, "{} {}", commit, file);
            let theirs = git_blame(dir, &["--first-parent", commit, "--", file]);
            let options = options.first_parent(true);
            assert_eq!(
                ours(dir, file, &options),
                theirs,
                "--first-parent {} {}",
                commit,
                file
            );
            compared += theirs.len();
            for (name, algorithm) in [
                ("minimal", DiffAlgorithm::Minimal),
                ("histogram", DiffAlgorithm::Histogram),
            ] {
                let flag = format!("--diff-algorithm={}", name);
                let theirs = git_blame(dir, &[&flag, commit, "--", file]);
                let options = BlameOptions::new()
                    .revision(commit.as_str())
                    .diff_algorithm(algorithm);
                assert_eq!(
                    ours(dir, file, &options),
                    theirs,
                    "{} {} {}",
                    flag,
                    commit,
                    file
                );
            }
        }
    }
    assert!(compared > 300, "{}", compared);

    // A range of lines, and the hunks.
    let theirs = git_blame(dir, &["-L", "3,12", "HEAD", "--", "lib/app.c"]);
    assert_eq!(
        ours(dir, "lib/app.c", &BlameOptions::new().lines(3, 12)),
        theirs
    );
    let repo = Repository::open(dir).unwrap();
    let blame = repo.blame("lib/app.c", &BlameOptions::new()).unwrap();
    let hunks = blame.hunks();
    assert_eq!(
        hunks.iter().map(|h| h.lines()).sum::<usize>(),
        blame.lines().len()
    );
    assert!(hunks.len() < blame.lines().len());
    // Lines come from the file under its old names too.
    let paths: std::collections::BTreeSet<String> = blame
        .lines()
        .iter()
        .map(|l| l.path().to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(
        paths.into_iter().collect::<Vec<_>>(),
        ["lib/app.c", "src/app.c", "src/main.c"]
    );
}

#[test]
fn errors() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    assert!(matches!(
        repo.blame("missing.c", &BlameOptions::new()),
        Err(zerogit::Error::PathNotFound(_))
    ));
    assert!(matches!(
        repo.blame("lib", &BlameOptions::new()),
        Err(zerogit::Error::PathNotFound(_))
    ));
    assert!(repo
        .blame("README", &BlameOptions::new().lines(3, 99))
        .is_err());
}

/// The stored real file pairs (`tests/data/diff_compat`), each committed as
/// its old then its new version: blame then depends on the diff matching
/// Git's on large rewrites, repeated lines and indentation.
#[test]
fn blame_matches_git_on_real_rewrites() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/diff_compat");
    let temp = empty_repository();
    let dir = temp.path();
    let mut names: Vec<String> = std::fs::read_dir(&data)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            (path.extension()? == "old")
                .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    for (i, version) in ["old", "new"].iter().enumerate() {
        for name in &names {
            let content = std::fs::read(data.join(format!("{}.{}", name, version))).unwrap();
            std::fs::write(dir.join(name), content).unwrap();
        }
        commit_all(dir, 1_700_000_000 + i as i64, version);
    }
    let mut failures = Vec::new();
    for name in &names {
        for (flag, algorithm) in [
            ("--diff-algorithm=myers", DiffAlgorithm::Myers),
            ("--diff-algorithm=minimal", DiffAlgorithm::Minimal),
            ("--diff-algorithm=histogram", DiffAlgorithm::Histogram),
        ] {
            let theirs = git_blame(dir, &[flag, "HEAD", "--", name]);
            let ours = ours(dir, name, &BlameOptions::new().diff_algorithm(algorithm));
            if ours != theirs {
                let differing = ours.iter().zip(&theirs).filter(|(a, b)| a != b).count();
                failures.push(format!(
                    "{} {}: {} of {} lines differ",
                    name,
                    flag,
                    differing,
                    theirs.len()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
