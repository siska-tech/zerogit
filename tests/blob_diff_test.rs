//! Line diffs of real commits compared with `git diff --numstat --minimal`.

use std::path::Path;
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::{BlobDiff, BlobDiffContent, DiffOptions, LineKind, NonTextReason, Repository};

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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Commits a sequence of edits that cover the tricky line diff cases.
fn history() -> TempDir {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    let revisions: Vec<Vec<(&str, &[u8])>> = vec![
        vec![
            (
                "doc.md",
                "# 見出し\n\n本文の一行目\n本文の二行目\n".as_bytes(),
            ),
            ("crlf.txt", b"one\r\ntwo\r\nthree\r\n"),
            ("noeol.txt", b"first\nlast"),
            ("repeat.txt", b"x\nx\nx\ny\nx\n"),
            ("binary.bin", b"\x00\x01\x02"),
            ("empty.txt", b""),
        ],
        vec![
            (
                "doc.md",
                "# 見出し\n\n本文の一行目を修正\n追加した行\n本文の二行目\n".as_bytes(),
            ),
            ("crlf.txt", b"one\r\ntwo\nthree\r\n"),
            ("noeol.txt", b"first\nlast\n"),
            ("repeat.txt", b"x\ny\nx\nx\nx\n"),
            ("binary.bin", b"\x00\x01\x03"),
            ("empty.txt", b"no longer empty\n"),
            ("new.txt", b"added file\n"),
        ],
        vec![
            ("doc.md", "全置換\n".as_bytes()),
            ("crlf.txt", b""),
            ("repeat.txt", b"top\nx\ny\nx\nx\nx\nbottom\n"),
        ],
    ];
    for (i, files) in revisions.iter().enumerate() {
        for (path, content) in files {
            fs::write(dir.join(path), content).unwrap();
        }
        if i == 2 {
            fs::remove_file(dir.join("new.txt")).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Revision {}", i)]);
    }
    temp
}

fn apply(old: &[u8], diff: &BlobDiff) -> Vec<u8> {
    let old = std::str::from_utf8(old).unwrap();
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut next = 0;
    for hunk in diff.hunks().unwrap() {
        let at = hunk.old_start() - usize::from(hunk.old_lines() > 0);
        out.extend(old_lines[next..at].iter().copied());
        next = at;
        for line in hunk.lines() {
            match line.kind() {
                LineKind::Context => {
                    out.push_str(line.content());
                    next += 1;
                }
                LineKind::Removed => next += 1,
                LineKind::Added => out.push_str(line.content()),
            }
        }
    }
    out.extend(old_lines[next..].iter().copied());
    out.into_bytes()
}

fn read(repo: &Repository, oid: Option<&zerogit::Oid>) -> Vec<u8> {
    oid.map(|oid| repo.blob(&oid.to_hex()).unwrap().content().to_vec())
        .unwrap_or_default()
}

/// Checks every changed file of every commit; returns a printable summary.
fn check_all(repo: &Repository) -> Vec<String> {
    let mut summary = Vec::new();
    let options = DiffOptions::new();
    for commit in repo.log().unwrap() {
        let commit = commit.unwrap();
        let parent = commit
            .parent()
            .map(|p| p.to_hex())
            // Git knows the empty tree without it being stored.
            .unwrap_or_else(|| "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned());
        let numstat = git(
            repo.path(),
            &[
                "diff",
                "--numstat",
                "--minimal",
                "--no-renames",
                &parent,
                &commit.oid().to_hex(),
            ],
        );
        for delta in repo.commit_diff(&commit).unwrap().deltas() {
            let diff = repo
                .diff_blobs(delta.old_oid(), delta.new_oid(), &options)
                .unwrap();
            assert_eq!(diff.old_exists(), delta.old_oid().is_some());
            assert_eq!(diff.new_exists(), delta.new_oid().is_some());
            let path = delta.path().to_str().unwrap().replace('\\', "/");
            let expected = numstat
                .lines()
                .find(|line| line.ends_with(&format!("\t{}", path)))
                .unwrap_or_else(|| panic!("{} missing from {}", path, numstat));
            let actual = match diff.content() {
                BlobDiffContent::Text(_) => {
                    let (old, new) = (read(repo, delta.old_oid()), read(repo, delta.new_oid()));
                    assert_eq!(apply(&old, &diff), new, "{}", path);
                    format!(
                        "{}\t{}\t{}",
                        diff.lines_added().unwrap(),
                        diff.lines_removed().unwrap(),
                        path
                    )
                }
                BlobDiffContent::NonText(NonTextReason::ContainsNul) => format!("-\t-\t{}", path),
                other => panic!("{}: {:?}", path, other),
            };
            assert_eq!(actual, expected, "{}", path);
            let headers: Vec<String> = diff
                .hunks()
                .map(|hunks| hunks.iter().map(|h| h.header()).collect())
                .unwrap_or_default();
            summary.push(format!("{} {} {:?}", commit.oid(), actual, headers));
        }
    }
    summary
}

#[test]
fn line_diffs_match_git_and_reconstruct_new_content() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    let loose = check_all(&repo);
    assert_eq!(loose.len(), 6 + 7 + 4);

    git(temp.path(), &["repack", "-a", "-d"]);
    assert_eq!(check_all(&Repository::open(temp.path()).unwrap()), loose);
}

#[test]
fn diff_blobs_reports_type_errors_for_non_blobs() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    let head = *repo.head().unwrap().oid();
    assert!(repo
        .diff_blobs(Some(&head), None, &DiffOptions::new())
        .is_err());
}
