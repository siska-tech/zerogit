//! `restore`, `remove` and `move_path` leave the index and the work tree
//! as `git restore`, `git rm` and `git mv` do, and refuse what Git refuses
//! without changing anything.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;

use common::{git, git_output, twin, worktree_files, write};
use zerogit::{Error, RemoveOptions, Repository, RestoreOptions};

/// Two commits, then a staged change (b.txt), an unstaged change (a.txt),
/// a deleted file (dir/c.txt) and an untracked file (new.txt).
fn busy_repository() -> tempfile::TempDir {
    let temp = common::repository(&["a.txt", "b.txt", "dir/c.txt", "dir/sub/d.txt", "e.rs"]);
    let dir = temp.path();
    write(dir, "a.txt", "a two\n");
    write(dir, "dir/c.txt", "c two\n");
    git(dir, &["commit", "-q", "-am", "Second"]);
    write(dir, "b.txt", "staged\n");
    git(dir, &["add", "b.txt"]);
    write(dir, "a.txt", "unstaged\n");
    std::fs::remove_file(dir.join("dir/c.txt")).unwrap();
    write(dir, "new.txt", "untracked\n");
    temp
}

fn state(dir: &Path) -> Vec<String> {
    vec![
        git(dir, &["ls-files", "-s"]),
        git(dir, &["status", "--porcelain", "-uall"]),
        format!("{:?}", worktree_files(dir)),
    ]
}

/// Runs the zerogit operation and the Git command on twins of `source`
/// and compares the results; both must succeed or both must fail, and a
/// failure must change nothing.
fn compare<T: std::fmt::Debug>(
    source: &Path,
    ours: impl FnOnce(&Repository) -> zerogit::Result<T>,
    git_args: &[&str],
) -> zerogit::Result<T> {
    let mine = twin(source);
    let theirs = twin(source);
    let before = state(mine.path());
    let result = ours(&Repository::open(mine.path()).unwrap());
    let git_ok = git_output(theirs.path(), git_args).status.success();
    assert_eq!(result.is_ok(), git_ok, "{:?}: {:?}", git_args, result);
    if result.is_ok() {
        assert_eq!(state(mine.path()), state(theirs.path()), "{:?}", git_args);
    } else {
        assert_eq!(
            state(mine.path()),
            before,
            "{:?} changed something",
            git_args
        );
    }
    result
}

#[test]
fn restore_matches_git_restore() {
    let temp = busy_repository();
    let src = temp.path();
    let cases: Vec<(&[&str], RestoreOptions, Vec<&str>)> = vec![
        (&["a.txt"], RestoreOptions::new(), vec![]),
        (&["dir"], RestoreOptions::new(), vec![]),
        (&["*.txt"], RestoreOptions::new(), vec![]),
        (&["."], RestoreOptions::new(), vec![]),
        (
            &["b.txt"],
            RestoreOptions::new().staged(true),
            vec!["--staged"],
        ),
        (&["."], RestoreOptions::new().staged(true), vec!["--staged"]),
        (
            &["a.txt"],
            RestoreOptions::new().source("HEAD~1"),
            vec!["--source=HEAD~1"],
        ),
        (
            &["dir"],
            RestoreOptions::new()
                .source("HEAD~1")
                .staged(true)
                .worktree(true),
            vec!["--source=HEAD~1", "--staged", "--worktree"],
        ),
        (
            &["."],
            RestoreOptions::new().source("HEAD~1"),
            vec!["--source=HEAD~1"],
        ),
        (
            &["b.txt"],
            RestoreOptions::new().source("HEAD^{tree}").staged(true),
            vec!["--source=HEAD^{tree}", "--staged"],
        ),
    ];
    for (paths, options, flags) in cases {
        let mut args = vec!["restore"];
        args.extend(flags);
        args.push("--");
        args.extend_from_slice(paths);
        compare(src, |repo| repo.restore(paths, &options), &args).unwrap();
    }
}

#[test]
fn restore_refuses_unknown_paths_and_conflicts() {
    let temp = busy_repository();
    assert!(matches!(
        compare(
            temp.path(),
            |repo| repo.restore(&["nosuch"], &RestoreOptions::new()),
            &["restore", "--", "nosuch"],
        ),
        Err(Error::PathspecNotMatched(spec)) if spec == "nosuch"
    ));
    let conflicted = common::conflicted_repository();
    assert!(matches!(
        compare(
            conflicted.path(),
            |repo| repo.restore(&["file.txt"], &RestoreOptions::new()),
            &["restore", "--", "file.txt"],
        ),
        Err(Error::UnmergedPaths(_))
    ));
    // Restoring the index resolves the conflict, in Git as well.
    compare(
        conflicted.path(),
        |repo| repo.restore(&["file.txt"], &RestoreOptions::new().staged(true)),
        &["restore", "--staged", "--", "file.txt"],
    )
    .unwrap();
}

#[test]
fn restored_files_are_clean_with_line_ending_conversion() {
    let temp = common::repository(&["a.txt", "b.txt"]);
    let dir = temp.path();
    git(dir, &["config", "core.autocrlf", "true"]);
    write(dir, "a.txt", "changed\r\n");
    write(dir, "b.txt", "changed too\r\n");
    compare(
        dir,
        |repo| {
            repo.restore(&["."], &RestoreOptions::new())?;
            // The new stat data is recorded: status needs no file reads and
            // reports nothing.
            assert!(repo.status()?.is_empty());
            Ok(())
        },
        &["restore", "--", "."],
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn restore_recreates_symlinks() {
    let temp = common::repository(&["target.txt"]);
    let dir = temp.path();
    std::os::unix::fs::symlink("target.txt", dir.join("link")).unwrap();
    git(dir, &["add", "link"]);
    git(dir, &["commit", "-q", "-m", "Link"]);
    std::fs::remove_file(dir.join("link")).unwrap();
    write(dir, "link", "not a link\n");
    compare(
        dir,
        |repo| {
            repo.restore(&["link"], &RestoreOptions::new())?;
            assert!(repo.status()?.is_empty());
            Ok(())
        },
        &["restore", "--", "link"],
    )
    .unwrap();
}

#[test]
fn remove_matches_git_rm() {
    let temp = busy_repository();
    let src = temp.path();
    let new = RemoveOptions::new;
    let cases: Vec<(&[&str], RemoveOptions, Vec<&str>, bool)> = vec![
        // Clean file.
        (&["e.rs"], new(), vec![], true),
        // Unstaged change: refused, but --cached keeps the file.
        (&["a.txt"], new(), vec![], false),
        (&["a.txt"], new().cached(true), vec!["--cached"], true),
        // Staged change: refused; --cached allowed (the file matches).
        (&["b.txt"], new(), vec![], false),
        (&["b.txt"], new().cached(true), vec!["--cached"], true),
        // Directories need -r.
        (&["dir"], new(), vec![], false),
        (&["dir"], new().recursive(true), vec!["-r"], true),
        (
            &["dir/sub"],
            new().recursive(true).cached(true),
            vec!["-r", "--cached"],
            true,
        ),
        // Globs and force.
        (&["*.txt"], new(), vec![], false),
        (&["*.txt"], new().force(true), vec!["-f"], true),
        (
            &["."],
            new().recursive(true).force(true),
            vec!["-r", "-f"],
            true,
        ),
        // Unknown path.
        (&["nosuch"], new(), vec![], false),
    ];
    for (paths, options, flags, ok) in cases {
        let mut args = vec!["rm", "-q"];
        args.extend(flags);
        args.push("--");
        args.extend_from_slice(paths);
        let result = compare(src, |repo| repo.remove(paths, &options), &args);
        assert_eq!(result.is_ok(), ok, "{:?}: {:?}", args, result);
    }

    // Staged and changed again in the work tree: refused even with --cached.
    write(src, "b.txt", "changed again\n");
    assert!(matches!(
        compare(
            src,
            |repo| repo.remove(&["b.txt"], &RemoveOptions::new().cached(true)),
            &["rm", "-q", "--cached", "--", "b.txt"],
        ),
        Err(Error::UncommittedChanges(paths)) if paths == [Path::new("b.txt")]
    ));
}

#[test]
fn remove_resolves_conflicts() {
    let temp = common::conflicted_repository();
    let removed = compare(
        temp.path(),
        |repo| repo.remove(&["file.txt"], &RemoveOptions::new()),
        &["rm", "-q", "--", "file.txt"],
    )
    .unwrap();
    assert_eq!(removed, [Path::new("file.txt")]);
}

#[test]
fn move_matches_git_mv() {
    let temp = busy_repository();
    let src = temp.path();
    let cases: &[(&str, &str, bool)] = &[
        ("e.rs", "renamed.rs", true),
        // dir/c.txt was deleted from the work tree: "bad source".
        ("dir", "moved", false),
        ("e.rs", "dir", true),
        ("dir/sub", "top", true),
        ("b.txt", "b2.txt", true),
        // Refused by Git: the destination exists, the source is untracked,
        // into itself, missing parent directory, missing source.
        ("a.txt", "b.txt", false),
        // dir/c.txt is tracked but deleted: Git lets it be overwritten.
        ("a.txt", "dir/c.txt", true),
        ("new.txt", "x.txt", false),
        ("dir", "dir/sub", false),
        ("e.rs", "nodir/e.rs", false),
        ("dir/c.txt", "c.txt", false),
    ];
    for (from, to, ok) in cases {
        let result = compare(src, |repo| repo.move_path(from, to), &["mv", from, to]);
        assert_eq!(result.is_ok(), *ok, "mv {} {}: {:?}", from, to, result);
    }
    let conflicted = common::conflicted_repository();
    assert!(compare(
        conflicted.path(),
        |repo| repo.move_path("file.txt", "renamed.txt"),
        &["mv", "file.txt", "renamed.txt"],
    )
    .is_err());
}

#[test]
fn moved_entries_keep_their_stat_data() {
    let temp = busy_repository();
    let dir = temp.path();
    // Files written in the second the index is written are smudged (size
    // 0, racy git); let a second pass so the entry keeps its stat data.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    git(dir, &["update-index", "-q", "--refresh"]);
    // The recorded stat data (everything but the path) moves with the file.
    let stat = |path: &str| -> Vec<String> {
        git(dir, &["ls-files", "--debug", path])
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect()
    };
    let before = stat("e.rs");
    let repo = Repository::open(dir).unwrap();
    repo.move_path("e.rs", "renamed.rs").unwrap();
    assert_eq!(stat("renamed.rs"), before);
    assert!(git(dir, &["status", "--porcelain"]).contains("R  e.rs -> renamed.rs"));
}
