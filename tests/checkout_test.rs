//! `checkout` switches like `git checkout`: local changes and untracked
//! files are carried over unless the switch would overwrite them, in which
//! case nothing changes; `force` discards them like `git checkout -f`.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;

use common::{git, git_output, twin, worktree_files, write};
use zerogit::{CheckoutOptions, Error, Repository};

/// `main` and `other` differ in a.txt (changed), b.txt (deleted on other),
/// new.txt, build.log (ignored on main, force-added on other) and
/// dir2/y.txt (added on other); shared.txt and dir/x.txt are the same.
fn two_branches() -> tempfile::TempDir {
    let temp = common::repository(&["a.txt", "b.txt", "shared.txt", "dir/x.txt"]);
    let dir = temp.path();
    write(dir, ".gitignore", "*.log\n");
    git(dir, &["add", ".gitignore"]);
    git(dir, &["commit", "-q", "-m", "Ignore logs"]);
    git(dir, &["checkout", "-q", "-b", "other"]);
    write(dir, "a.txt", "a on other\n");
    std::fs::remove_file(dir.join("b.txt")).unwrap();
    write(dir, "new.txt", "new on other\n");
    write(dir, "build.log", "tracked log\n");
    write(dir, "dir2/y.txt", "y\n");
    git(dir, &["add", "-A"]);
    git(dir, &["add", "-f", "build.log"]);
    git(dir, &["commit", "-q", "-m", "Other"]);
    git(dir, &["checkout", "-q", "main"]);
    temp
}

fn state(dir: &Path) -> Vec<String> {
    vec![
        String::from_utf8(git_output(dir, &["symbolic-ref", "-q", "HEAD"]).stdout).unwrap(),
        git(dir, &["rev-parse", "HEAD"]),
        git(dir, &["ls-files", "-s"]),
        git(dir, &["status", "--porcelain", "-uall", "--ignored"]),
        format!("{:?}", worktree_files(dir)),
    ]
}

/// Sets up local changes with `prepare`, then checks out `other` with
/// zerogit and Git on twins: both succeed with the same result, or both
/// fail and zerogit changed nothing.
fn compare(
    prepare: impl Fn(&Path),
    options: &CheckoutOptions,
    git_flags: &[&str],
) -> zerogit::Result<()> {
    let temp = two_branches();
    prepare(temp.path());
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let before = state(ours.path());
    let result = Repository::open(ours.path())
        .unwrap()
        .checkout_with("other", options);
    let mut args = vec!["checkout", "-q"];
    args.extend_from_slice(git_flags);
    args.push("other");
    let git_ok = git_output(theirs.path(), &args).status.success();
    assert_eq!(result.is_ok(), git_ok, "{:?}", result);
    if git_ok {
        assert_eq!(state(ours.path()), state(theirs.path()));
    } else {
        assert_eq!(
            state(ours.path()),
            before,
            "a refused checkout changed something"
        );
    }
    result
}

fn plain(prepare: impl Fn(&Path)) -> zerogit::Result<()> {
    compare(prepare, &CheckoutOptions::new(), &[])
}

#[test]
fn clean_switch() {
    plain(|_| {}).unwrap();
}

#[test]
fn unrelated_changes_are_carried_over() {
    // An untracked file elsewhere.
    plain(|dir| write(dir, "scratch.tmp", "notes\n")).unwrap();
    // Unstaged and staged changes to paths the switch does not change.
    plain(|dir| write(dir, "shared.txt", "local\n")).unwrap();
    plain(|dir| {
        write(dir, "dir/x.txt", "staged\n");
        git(dir, &["add", "dir/x.txt"]);
    })
    .unwrap();
    // A new staged file the target does not have.
    plain(|dir| {
        write(dir, "added.txt", "added\n");
        git(dir, &["add", "added.txt"]);
    })
    .unwrap();
}

#[test]
fn changes_the_switch_would_overwrite_are_refused() {
    let refused = |prepare: &dyn Fn(&Path)| match plain(prepare) {
        Err(Error::LocalChangesWouldBeOverwritten(paths)) => paths
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>(),
        other => panic!("expected a refusal, got {:?}", other),
    };
    // Unstaged and staged changes to a file the target changes.
    assert_eq!(refused(&|dir| write(dir, "a.txt", "local\n")), ["a.txt"]);
    assert_eq!(
        refused(&|dir| {
            write(dir, "a.txt", "staged\n");
            git(dir, &["add", "a.txt"]);
        }),
        ["a.txt"]
    );
    // A change to a file the target deletes.
    assert_eq!(refused(&|dir| write(dir, "b.txt", "local\n")), ["b.txt"]);
    // An untracked file where the target has one, or needs a directory.
    assert_eq!(refused(&|dir| write(dir, "new.txt", "mine\n")), ["new.txt"]);
    assert_eq!(
        refused(&|dir| write(dir, "dir2", "a file\n")),
        ["dir2/y.txt"]
    );
}

#[test]
fn ignored_files_and_matching_index_entries_do_not_block() {
    // An ignored file in the way is overwritten, as in Git.
    plain(|dir| write(dir, "build.log", "local log\n")).unwrap();
    // The index already has the target's content.
    plain(|dir| {
        write(dir, "a.txt", "a on other\n");
        git(dir, &["add", "a.txt"]);
    })
    .unwrap();
}

#[test]
fn force_discards_local_changes() {
    let force = CheckoutOptions::new().force(true);
    compare(|dir| write(dir, "a.txt", "local\n"), &force, &["-f"]).unwrap();
    compare(
        |dir| {
            write(dir, "b.txt", "local\n");
            write(dir, "new.txt", "mine\n");
            write(dir, "scratch.tmp", "kept\n");
        },
        &force,
        &["-f"],
    )
    .unwrap();
}

#[test]
fn conflicts_block_unless_forced() {
    let temp = common::conflicted_repository();
    let dir = temp.path();
    git(dir, &["branch", "elsewhere", "HEAD~1"]);
    let repo = Repository::open(dir).unwrap();
    assert!(matches!(
        repo.checkout("elsewhere"),
        Err(Error::UnmergedPaths(_))
    ));
    assert!(!git_output(dir, &["checkout", "-q", "elsewhere"])
        .status
        .success());

    let theirs = twin(dir);
    repo.checkout_with("elsewhere", &CheckoutOptions::new().force(true))
        .unwrap();
    git(theirs.path(), &["checkout", "-q", "-f", "elsewhere"]);
    assert_eq!(state(dir), state(theirs.path()));
}
