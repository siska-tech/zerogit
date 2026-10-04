//! Status uses the stat data of the index as Git does: unchanged files are
//! not read, racily clean files are, and an index refreshed by either Git or
//! zerogit gives the same status in both.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{git, repository, write};
use zerogit::{FileStatus, Repository};

/// `git status --porcelain` paths with their two-letter codes.
fn git_status(dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> = git(dir, &["status", "--porcelain", "-uall"])
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

fn zerogit_status(repo: &Repository) -> Vec<(String, FileStatus)> {
    let mut entries: Vec<(String, FileStatus)> = repo
        .status()
        .unwrap()
        .into_iter()
        .map(|e| (e.path().to_string_lossy().replace('\\', "/"), e.status()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Waits until the clock is in a later second than the files' mtimes, so
/// that an index written next is not racy for them.
fn let_a_second_pass() {
    std::thread::sleep(Duration::from_millis(1100));
}

#[test]
fn same_size_rewrite_in_the_same_second_is_detected() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    write(dir, "a.txt", "first\n");
    repo.add("a.txt").unwrap();
    // Same size, written right after the index: its stat data may be
    // indistinguishable on a coarse file system; the content decides.
    write(dir, "a.txt", "other\n");
    assert_eq!(
        zerogit_status(&repo),
        [("a.txt".to_owned(), FileStatus::Modified)]
    );
    assert_eq!(git_status(dir), ["MM a.txt"]);
}

#[test]
fn entries_written_this_second_are_smudged_for_later_readers() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let second = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    // Retry if the clock ticks between writing the file and the index.
    let mut smudged = None;
    for _ in 0..5 {
        let before = second();
        write(dir, "a.txt", "fresh\n");
        repo.add("a.txt").unwrap();
        if second() == before {
            smudged = Some(git(dir, &["ls-files", "--debug", "a.txt"]));
            break;
        }
    }
    // The file was modified in the second the index was written, so its
    // size is recorded as 0 (Git's racily clean entry), which makes every
    // reader compare the content.
    let debug = smudged.expect("file and index written in the same second");
    assert!(
        debug.lines().any(|l| l.trim() == "size: 0\tflags: 0"),
        "racy entry is smudged: {}",
        debug
    );

    let_a_second_pass();
    repo.refresh_index().unwrap();
    let debug = git(dir, &["ls-files", "--debug", "a.txt"]);
    assert!(
        debug.lines().any(|l| l.trim() == "size: 6\tflags: 0"),
        "refreshed entry has its size: {}",
        debug
    );
}

#[test]
fn index_refreshed_by_zerogit_is_clean_for_git() {
    let temp = repository(&["a.txt", "dir/b.txt", "c.txt"]);
    let dir = temp.path();
    // Touch every file without changing it: all stat data is now stale.
    for path in ["a.txt", "dir/b.txt", "c.txt"] {
        write(dir, path, &format!("content of {}\n", path));
    }
    let_a_second_pass();
    let repo = Repository::open(dir).unwrap();
    assert!(zerogit_status(&repo).is_empty());
    assert_eq!(repo.refresh_index().unwrap(), 3);
    assert_eq!(repo.refresh_index().unwrap(), 0);
    // `git diff-files` trusts the stat data without refreshing it: nothing
    // is listed only if Git accepts the stat data zerogit recorded.
    assert_eq!(git(dir, &["diff-files", "--name-only"]), "");
    assert!(git_status(dir).is_empty());
}

#[test]
fn index_refreshed_by_git_gives_the_same_status() {
    let temp = repository(&["a.txt", "dir/b.txt", "c.txt", "d.txt"]);
    let dir = temp.path();
    for path in ["a.txt", "dir/b.txt", "c.txt"] {
        write(dir, path, &format!("content of {}\n", path));
    }
    let_a_second_pass();
    git(dir, &["update-index", "-q", "--refresh"]);
    write(dir, "c.txt", "changed\n");
    std::fs::remove_file(dir.join("d.txt")).unwrap();
    write(dir, "new.txt", "new\n");

    let repo = Repository::open(dir).unwrap();
    assert_eq!(
        zerogit_status(&repo),
        [
            ("c.txt".to_owned(), FileStatus::Modified),
            ("d.txt".to_owned(), FileStatus::Deleted),
            ("new.txt".to_owned(), FileStatus::Untracked),
        ]
    );
    assert_eq!(git_status(dir), [" D d.txt", " M c.txt", "?? new.txt"]);

    // The other way round: zerogit refreshes, Git reads.
    repo.refresh_index().unwrap();
    assert_eq!(git_status(dir), [" D d.txt", " M c.txt", "?? new.txt"]);
    // A changed file is never refreshed into looking clean.
    assert_eq!(git(dir, &["diff-files", "--name-only"]), "c.txt\nd.txt\n");
}

#[cfg(unix)]
#[test]
fn files_with_matching_stat_data_are_not_read() {
    use std::os::unix::fs::PermissionsExt;

    let temp = repository(&["a.txt", "secret.txt"]);
    let dir = temp.path();
    // chmod changes the ctime; tell Git and zerogit not to compare it.
    git(dir, &["config", "core.trustctime", "false"]);
    // The files were committed in the second they were written, so Git
    // marked their entries racily clean (size 0: compare by content). Let a
    // second pass and refresh while the files are readable, so the index
    // has their real stat data and is newer than them. Touching a.txt makes
    // sure the refresh rewrites the index.
    let_a_second_pass();
    write(dir, "a.txt", "content of a.txt\n");
    git(dir, &["update-index", "-q", "--refresh"]);
    let debug = git(dir, &["ls-files", "--debug", "secret.txt"]);
    assert!(
        debug.lines().any(|l| l.trim() == "size: 22\tflags: 0"),
        "{}",
        debug
    );

    let secret = dir.join("secret.txt");
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&secret).is_ok() {
        // Running as root: permissions do not stop reading.
        return;
    }

    // Reading the unreadable file would fail; matching stat data skips it.
    let repo = Repository::open(dir).unwrap();
    assert!(zerogit_status(&repo).is_empty());
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
}
