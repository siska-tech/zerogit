//! `Repository::cherry_pick` / `Repository::revert` compared with
//! `git cherry-pick` / `git revert` (#62).

mod common;

use common::*;
use std::fs;
use std::path::{Path, PathBuf};
use zerogit::{CherryPickOptions, CommitOptions, Error, PickOutcome, Repository, Signature};

/// main and topic diverge. On topic: "Topic one" (adds a.txt), "Topic two"
/// (changes f.txt, conflicting with main) and "Topic three" (adds c.txt,
/// with a trailer). main is checked out.
fn diverged() -> tempfile::TempDir {
    let temp = repository(&["f.txt", "shared.txt"]);
    let dir = temp.path();
    write(dir, "f.txt", "1\n2\n3\n");
    git(dir, &["commit", "-q", "-am", "Base"]);
    git(dir, &["checkout", "-q", "-b", "topic"]);
    write(dir, "a.txt", "topic one\n");
    git(dir, &["add", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "Topic one\n\nWith a body."]);
    write(dir, "f.txt", "1\ntopic\n3\n");
    git(dir, &["commit", "-q", "-am", "Topic two"]);
    write(dir, "c.txt", "topic three\n");
    git(dir, &["add", "c.txt"]);
    git(
        dir,
        &[
            "commit",
            "-q",
            "-m",
            "Topic three\n\nSigned-off-by: Test <test@example.com>",
        ],
    );
    git(dir, &["checkout", "-q", "main"]);
    write(dir, "f.txt", "1\nmain\n3\n");
    git(dir, &["commit", "-q", "-am", "Main change"]);
    write(dir, "shared.txt", "main shared\n");
    git(dir, &["commit", "-q", "-am", "Main shared"]);
    git(dir, &["tag", "base"]);
    temp
}

/// Tree, author and message of each commit after `base`, oldest first;
/// with `dates`, the author date too.
fn history(dir: &Path, base: &str, dates: bool) -> String {
    let format = if dates {
        "--format=%T %an <%ae> %ad%n%B--"
    } else {
        "--format=%T %an <%ae>%n%B--"
    };
    git(
        dir,
        &["log", "--reverse", format, &format!("{}..HEAD", base)],
    )
}

fn assert_same(ours: &Path, theirs: &Path, base: &str, dates: bool) {
    assert_eq!(
        history(ours, base, dates),
        history(theirs, base, dates),
        "history"
    );
    assert_eq!(git_ls_files(ours), git_ls_files(theirs), "index");
    assert_eq!(worktree_files(ours), worktree_files(theirs), "work tree");
    assert_eq!(
        git(ours, &["status", "--porcelain"]),
        git(theirs, &["status", "--porcelain"]),
        "status"
    );
}

fn state_file(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(".git").join(name)).ok()
}

/// The files Git keeps for a stopped cherry-pick or revert.
const STATE_FILES: &[&str] = &[
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "MERGE_MSG",
    "sequencer/head",
    "sequencer/todo",
    "sequencer/opts",
];

fn assert_same_state(ours: &Path, theirs: &Path) {
    for name in STATE_FILES {
        assert_eq!(state_file(ours, name), state_file(theirs, name), "{}", name);
    }
    // The commit the sequence last made: each tool made its own.
    let safety = |dir: &Path| {
        state_file(dir, "sequencer/abort-safety").map(|oid| oid == git(dir, &["rev-parse", "HEAD"]))
    };
    assert_eq!(safety(ours), safety(theirs), "sequencer/abort-safety");
    assert_eq!(
        ours.join(".git/sequencer").exists(),
        theirs.join(".git/sequencer").exists()
    );
}

fn reflog(dir: &Path, n: usize) -> String {
    git(dir, &["reflog", &format!("-{}", n), "--format=%gs"])
}

fn open(dir: &Path) -> Repository {
    Repository::open(dir).unwrap()
}

fn pick(dir: &Path, commits: &[&str], record_origin: bool) -> PickOutcome {
    open(dir)
        .cherry_pick(
            commits,
            &CherryPickOptions::new().record_origin(record_origin),
            "Test",
            "test@example.com",
        )
        .unwrap()
}

/// Runs a Git command that continues an operation (and may open an
/// editor, which keeps the message).
fn git_continue(dir: &Path, args: &[&str]) {
    let mut all = vec!["-c", "core.editor=true"];
    all.extend_from_slice(args);
    git(dir, &all);
}

#[test]
fn clean_picks_match_git() {
    let temp = diverged();
    for (commits, record_origin) in [
        (vec!["topic~2"], false),
        (vec!["topic~2"], true),
        (vec!["topic", "topic~2", "topic"], true),
    ] {
        let ours = twin(temp.path());
        let theirs = twin(temp.path());
        let (o, t) = (ours.path(), theirs.path());
        let mut args = vec!["cherry-pick"];
        if record_origin {
            args.push("-x");
        }
        args.extend(commits.iter().copied());
        git(t, &args);
        let outcome = pick(o, &commits, record_origin);
        assert_eq!(
            outcome,
            PickOutcome::Completed(open(o).rev_parse("HEAD").unwrap())
        );
        assert_same(o, t, "base", true);
        assert_eq!(reflog(o, 3), reflog(t, 3));
        assert_eq!(
            git(o, &["reflog", "-2", "--format=%gs", "main"]),
            git(t, &["reflog", "-2", "--format=%gs", "main"])
        );
        assert_same_state(o, t);
        assert_fsck(o);
    }
}

#[test]
fn clean_reverts_match_git() {
    let temp = diverged();
    let dir = temp.path();
    // Revert of a revert is a reapply.
    git(dir, &["revert", "--no-edit", "HEAD~1"]);
    for commits in [vec!["HEAD"], vec!["HEAD~1", "HEAD"]] {
        let ours = twin(dir);
        let theirs = twin(dir);
        let (o, t) = (ours.path(), theirs.path());
        let mut args = vec!["revert", "--no-edit"];
        args.extend(commits.iter().copied());
        git(t, &args);
        let outcome = open(o)
            .revert(&commits, "Test", "test@example.com")
            .unwrap();
        assert!(matches!(outcome, PickOutcome::Completed(_)));
        let base = git(dir, &["rev-parse", "HEAD"]);
        assert_same(o, t, base.trim(), false);
        assert_eq!(reflog(o, 3), reflog(t, 3));
        assert_same_state(o, t);
        assert_fsck(o);
    }
}

#[test]
fn conflict_stops_like_git_and_continues_either_way() {
    let temp = diverged();
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let (o, t) = (ours.path(), theirs.path());
    assert!(!git_output(t, &["cherry-pick", "topic~1"]).status.success());
    let outcome = pick(o, &["topic~1"], false);
    let PickOutcome::Conflicts { commit, paths } = outcome else {
        panic!("expected conflicts, got {:?}", outcome);
    };
    assert_eq!(commit.to_hex(), git(o, &["rev-parse", "topic~1"]).trim());
    assert_eq!(paths, vec![PathBuf::from("f.txt")]);
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_eq!(worktree_files(o), worktree_files(t));
    assert_same_state(o, t);
    assert!(state_file(o, "MERGE_MSG")
        .unwrap()
        .ends_with("\n# Conflicts:\n#\tf.txt\n"));

    // Resolve the same way; continue with zerogit in one, Git in the other.
    let git_continued = twin(t);
    let zerogit_continued = twin(t);
    for dir in [o, t, git_continued.path(), zerogit_continued.path()] {
        write(dir, "f.txt", "1\nmain\ntopic\n3\n");
        git(dir, &["add", "f.txt"]);
    }
    let repo = open(o);
    assert!(matches!(
        repo.cherry_pick_continue("Test", "test@example.com")
            .unwrap(),
        PickOutcome::Completed(_)
    ));
    git_continue(t, &["cherry-pick", "--continue"]);
    assert_same(o, t, "base", true);
    assert_eq!(reflog(o, 2), reflog(t, 2));
    assert_same_state(o, t);

    // Each tool continues what the other started.
    git_continue(git_continued.path(), &["cherry-pick", "--continue"]);
    open(zerogit_continued.path())
        .cherry_pick_continue("Test", "test@example.com")
        .unwrap();
    assert_same(o, git_continued.path(), "base", true);
    assert_same(o, zerogit_continued.path(), "base", true);
    assert_fsck(o);
}

#[test]
fn sequence_stops_continues_and_skips_like_git() {
    let temp = diverged();
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let (o, t) = (ours.path(), theirs.path());
    let commits = ["topic~2", "topic~1", "topic"];
    let mut args = vec!["cherry-pick", "-x"];
    args.extend(commits);
    assert!(!git_output(t, &args).status.success());
    assert!(matches!(
        pick(o, &commits, true),
        PickOutcome::Conflicts { .. }
    ));
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_same_state(o, t);
    assert_eq!(reflog(o, 2), reflog(t, 2));
    let at_stop = twin(o);

    // Continue: Git in one, zerogit in the other, on Git's state.
    let from_git = twin(t);
    for dir in [o, t, from_git.path()] {
        write(dir, "f.txt", "resolved\n");
        git(dir, &["add", "f.txt"]);
    }
    open(o)
        .cherry_pick_continue("Test", "test@example.com")
        .unwrap();
    git_continue(t, &["cherry-pick", "--continue"]);
    open(from_git.path())
        .cherry_pick_continue("Test", "test@example.com")
        .unwrap();
    assert_same(o, t, "base", true);
    assert_same(from_git.path(), t, "base", true);
    assert_eq!(reflog(o, 4), reflog(t, 4));
    assert_same_state(o, t);
    assert!(!o.join(".git/sequencer").exists());

    // Skip drops the conflicting commit and picks the rest.
    let skipped = twin(at_stop.path());
    let (o, t) = (at_stop.path(), skipped.path());
    git(t, &["cherry-pick", "--skip"]);
    let outcome = open(o)
        .cherry_pick_skip("Test", "test@example.com")
        .unwrap();
    assert!(matches!(outcome, PickOutcome::Completed(_)));
    assert_same(o, t, "base", true);
    assert_eq!(reflog(o, 3), reflog(t, 3));
    assert_eq!(
        git(o, &["log", "--format=%s", "base..HEAD"]),
        "Topic three\nTopic one\n"
    );
    assert_same_state(o, t);
}

#[test]
fn abort_returns_like_git() {
    let temp = diverged();
    let dir = temp.path();
    // An unrelated local change survives the abort, as with Git.
    write(dir, "shared.txt", "local change\n");
    let before = git(dir, &["rev-parse", "HEAD"]);
    for commits in [vec!["topic~1"], vec!["topic~2", "topic~1", "topic"]] {
        let ours = twin(dir);
        let theirs = twin(dir);
        let (o, t) = (ours.path(), theirs.path());
        let mut args = vec!["cherry-pick"];
        args.extend(commits.iter().copied());
        assert!(!git_output(t, &args).status.success());
        assert!(matches!(
            pick(o, &commits, false),
            PickOutcome::Conflicts { .. }
        ));
        // zerogit aborts Git's cherry-pick and its own; Git aborts zerogit's.
        let git_aborts = twin(o);
        let zerogit_aborts = twin(t);
        open(o).cherry_pick_abort().unwrap();
        git(t, &["cherry-pick", "--abort"]);
        // Copying changed the files' stat data; let Git see they are unchanged.
        let _ = git_output(git_aborts.path(), &["update-index", "-q", "--refresh"]);
        git(git_aborts.path(), &["cherry-pick", "--abort"]);
        open(zerogit_aborts.path()).cherry_pick_abort().unwrap();
        for other in [t, git_aborts.path(), zerogit_aborts.path()] {
            assert_eq!(git(other, &["rev-parse", "HEAD"]), before);
            assert_same(o, other, "base", true);
            assert_same_state(o, other);
        }
        assert_eq!(git(o, &["rev-parse", "HEAD"]), before);
        assert_eq!(reflog(o, 4), reflog(t, 4));
        // Where the abort came from: in a sequence, the commit each tool
        // made for the first pick, whose ID depends on when it was made.
        let orig_head = |dir: &Path| {
            git(
                dir,
                &["log", "-1", "--format=%T %P %an %ae %ad %s", "ORIG_HEAD"],
            )
        };
        assert_eq!(orig_head(o), orig_head(t));
        assert_eq!(
            fs::read_to_string(o.join("shared.txt")).unwrap(),
            "local change\n"
        );
    }
}

#[test]
fn revert_conflicts_continue_and_abort_like_git() {
    let temp = diverged();
    let dir = temp.path();
    git(dir, &["checkout", "-q", "topic"]);
    write(dir, "f.txt", "1\nlater\n3\n");
    git(dir, &["commit", "-q", "-am", "Later change"]);
    let before = git(dir, &["rev-parse", "HEAD"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    let commits = ["topic~1", "topic~2"];
    assert!(
        !git_output(t, &["revert", "--no-edit", "topic~1", "topic~2"])
            .status
            .success()
    );
    let outcome = open(o)
        .revert(&commits, "Test", "test@example.com")
        .unwrap();
    assert!(matches!(outcome, PickOutcome::Conflicts { .. }));
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_eq!(worktree_files(o), worktree_files(t));
    assert_same_state(o, t);
    let aborted = twin(o);

    // Cherry-pick operations refuse to act on a revert.
    assert!(matches!(
        open(o).cherry_pick_continue("Test", "test@example.com"),
        Err(Error::RevertInProgress)
    ));
    let from_git = twin(t);
    for dir in [o, t, from_git.path()] {
        write(dir, "f.txt", "1\n2\n3\n");
        git(dir, &["add", "f.txt"]);
    }
    open(o).revert_continue("Test", "test@example.com").unwrap();
    git_continue(t, &["revert", "--continue"]);
    open(from_git.path())
        .revert_continue("Test", "test@example.com")
        .unwrap();
    assert_same(o, t, before.trim(), false);
    assert_same(from_git.path(), t, before.trim(), false);
    assert_eq!(reflog(o, 2), reflog(t, 2));
    assert_same_state(o, t);

    let dir = aborted.path();
    open(dir).revert_abort().unwrap();
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
    assert!(!dir.join(".git/sequencer").exists());
    assert!(!dir.join(".git/REVERT_HEAD").exists());
}

#[test]
fn empty_picks_stop_like_git() {
    let temp = diverged();
    let dir = temp.path();
    // main already has topic's first change.
    git(dir, &["cherry-pick", "topic~2"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    assert!(!git_output(t, &["cherry-pick", "topic~2", "topic"])
        .status
        .success());
    let outcome = pick(o, &["topic~2", "topic"], false);
    let PickOutcome::Empty { commit } = outcome else {
        panic!("expected an empty stop, got {:?}", outcome);
    };
    assert_eq!(commit.to_hex(), git(o, &["rev-parse", "topic~2"]).trim());
    assert_same_state(o, t);
    assert_eq!(git(o, &["status", "--porcelain"]), "");

    // Continuing does not commit an empty change.
    assert_eq!(
        open(o)
            .cherry_pick_continue("Test", "test@example.com")
            .unwrap(),
        PickOutcome::Empty { commit }
    );
    assert_same_state(o, t);

    // Committing it anyway concludes it, as `git commit --allow-empty`
    // does, and the sequence goes on.
    let kept = twin(o);
    let git_kept = twin(t);
    let committer = Signature::now("Test", "test@example.com");
    open(kept.path())
        .create_commit_with(
            &state_file(o, "MERGE_MSG").unwrap(),
            &CommitOptions::new()
                .allow_empty(true)
                .committer(committer.clone()),
        )
        .unwrap();
    git(
        git_kept.path(),
        &["commit", "-q", "--allow-empty", "--no-edit"],
    );
    assert_same(kept.path(), git_kept.path(), "base", true);
    assert_eq!(reflog(kept.path(), 1), reflog(git_kept.path(), 1));
    assert_same_state(kept.path(), git_kept.path());
    open(kept.path())
        .cherry_pick_continue("Test", "test@example.com")
        .unwrap();
    git_continue(git_kept.path(), &["cherry-pick", "--continue"]);
    assert_same(kept.path(), git_kept.path(), "base", true);
    assert_same_state(kept.path(), git_kept.path());

    git(t, &["cherry-pick", "--skip"]);
    assert!(matches!(
        open(o)
            .cherry_pick_skip("Test", "test@example.com")
            .unwrap(),
        PickOutcome::Completed(_)
    ));
    assert_same(o, t, "base", true);
    assert_same_state(o, t);
}

#[test]
fn concluding_with_a_commit_keeps_the_author() {
    let temp = diverged();
    let dir = temp.path();
    git(dir, &["config", "user.name", "Someone Else"]);
    git(dir, &["config", "user.email", "else@example.com"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    let commits = ["topic~1", "topic"];
    assert!(!git_output(t, &["cherry-pick", "topic~1", "topic"])
        .status
        .success());
    assert!(matches!(
        pick(o, &commits, false),
        PickOutcome::Conflicts { .. }
    ));
    let repo = open(o);
    // Neither a merge nor an amend can happen before it is concluded.
    assert!(matches!(
        repo.merge("topic", "T", "t@example.com", &Default::default()),
        Err(Error::CherryPickInProgress)
    ));
    for dir in [o, t] {
        write(dir, "f.txt", "resolved\n");
        git(dir, &["add", "f.txt"]);
    }
    assert!(matches!(
        repo.amend_commit(None, &CommitOptions::new()),
        Err(Error::CherryPickInProgress)
    ));
    repo.create_commit_with("Topic two", &CommitOptions::new())
        .unwrap();
    git(t, &["commit", "-q", "-m", "Topic two"]);
    assert_same(o, t, "base", true);
    assert_eq!(reflog(o, 1), reflog(t, 1));
    assert_same_state(o, t);
    assert!(!o.join(".git/CHERRY_PICK_HEAD").exists());
    assert!(!o.join(".git/MERGE_MSG").exists());

    // The rest of the sequence follows.
    repo.cherry_pick_continue("Test", "test@example.com")
        .unwrap();
    git_continue(t, &["cherry-pick", "--continue"]);
    assert_same(o, t, "base", true);
    assert_same_state(o, t);
}

#[test]
fn refusals_change_nothing() {
    let temp = diverged();
    let dir = temp.path();
    let repo = open(dir);
    let snapshot = |dir: &Path| {
        (
            git(dir, &["rev-parse", "HEAD"]),
            git(dir, &["status", "--porcelain"]),
            git_ls_files(dir),
            worktree_files(dir),
            fs::read_dir(dir.join(".git"))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>(),
        )
    };
    let check = |result: zerogit::Result<PickOutcome>, before| {
        let e = result.unwrap_err();
        assert_eq!(snapshot(dir), before, "{:?}", e);
        e
    };
    let opts = CherryPickOptions::new();

    // Staged changes, even to other paths.
    write(dir, "shared.txt", "staged\n");
    git(dir, &["add", "shared.txt"]);
    let before = snapshot(dir);
    let e = check(
        repo.cherry_pick(&["topic~2", "topic"], &opts, "T", "t@example.com"),
        before,
    );
    assert!(matches!(e, Error::LocalChangesWouldBeOverwritten(_)));
    git(dir, &["reset", "-q"]);

    // A local change to a path the commit changes.
    write(dir, "f.txt", "local\n");
    let before = snapshot(dir);
    let e = check(repo.revert(&["HEAD~1"], "T", "t@example.com"), before);
    assert!(matches!(e, Error::LocalChangesWouldBeOverwritten(_)));
    // An untracked file the commit would create.
    git(dir, &["checkout", "--", "f.txt"]);
    write(dir, "a.txt", "untracked\n");
    let before = snapshot(dir);
    let e = check(
        repo.cherry_pick(&["topic~2", "topic"], &opts, "T", "t@example.com"),
        before,
    );
    assert!(matches!(e, Error::LocalChangesWouldBeOverwritten(_)));
    fs::remove_file(dir.join("a.txt")).unwrap();

    // A merge commit, a locked index, nothing to pick.
    git(dir, &["merge", "-q", "--no-edit", "topic~2"]);
    let before = snapshot(dir);
    let e = check(
        repo.cherry_pick(&["HEAD"], &opts, "T", "t@example.com"),
        before.clone(),
    );
    assert!(matches!(e, Error::UnsupportedCherryPick(_)));
    let e = check(repo.cherry_pick(&[], &opts, "T", "t@example.com"), before);
    assert!(matches!(e, Error::UnsupportedCherryPick(_)));
    fs::write(dir.join(".git/index.lock"), "").unwrap();
    let before = snapshot(dir);
    let e = check(
        repo.cherry_pick(&["topic~2", "topic"], &opts, "T", "t@example.com"),
        before,
    );
    assert!(matches!(e, Error::Locked(_)));
    fs::remove_file(dir.join(".git/index.lock")).unwrap();

    // Nothing in progress.
    assert!(matches!(
        repo.cherry_pick_continue("T", "t@example.com"),
        Err(Error::NoCherryPickInProgress)
    ));
    assert!(matches!(
        repo.revert_skip("T", "t@example.com"),
        Err(Error::NoRevertInProgress)
    ));
    assert!(matches!(
        repo.cherry_pick_abort(),
        Err(Error::NoCherryPickInProgress)
    ));

    // During a merge, and during a stopped cherry-pick.
    let merging = twin(dir);
    assert!(!git_output(merging.path(), &["merge", "topic"])
        .status
        .success());
    assert!(matches!(
        open(merging.path()).cherry_pick(&["topic"], &opts, "T", "t@example.com"),
        Err(Error::MergeInProgress)
    ));
    assert!(matches!(
        repo.cherry_pick(&["topic~1"], &opts, "T", "t@example.com")
            .unwrap(),
        PickOutcome::Conflicts { .. }
    ));
    write(dir, "f.txt", "resolved\n");
    git(dir, &["add", "f.txt"]);
    let before = snapshot(dir);
    let e = check(repo.revert(&["HEAD"], "T", "t@example.com"), before);
    assert!(matches!(e, Error::CherryPickInProgress));
}

#[test]
fn hard_reset_ends_a_stopped_pick_like_git() {
    let temp = diverged();
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let (o, t) = (ours.path(), theirs.path());
    assert!(!git_output(t, &["cherry-pick", "topic~1"]).status.success());
    pick(o, &["topic~1"], false);
    open(o).reset_to("HEAD", zerogit::ResetMode::Hard).unwrap();
    git(t, &["reset", "-q", "--hard", "HEAD"]);
    assert_same(o, t, "base", true);
    assert_same_state(o, t);
    assert!(matches!(
        open(o).cherry_pick_continue("T", "t@example.com"),
        Err(Error::NoCherryPickInProgress)
    ));
}

#[test]
fn picks_follow_renames_like_git() {
    let temp = diverged();
    let dir = temp.path();
    git(dir, &["mv", "f.txt", "g.txt"]);
    git(dir, &["commit", "-q", "-m", "Rename f"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    // Topic two changes f.txt; main renamed it and changed the same line.
    assert!(!git_output(t, &["cherry-pick", "topic~1"]).status.success());
    assert!(matches!(
        pick(o, &["topic~1"], false),
        PickOutcome::Conflicts { .. }
    ));
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_eq!(worktree_files(o), worktree_files(t));
    assert_same_state(o, t);
}

#[test]
fn second_stop_after_continue_matches_git() {
    let temp = diverged();
    let dir = temp.path();
    // Another change to the same line: it conflicts after the first stop
    // is resolved.
    git(dir, &["checkout", "-q", "-b", "other", "topic~3"]);
    write(dir, "f.txt", "1\nother\n3\n");
    git(dir, &["commit", "-q", "-am", "Other change"]);
    git(dir, &["checkout", "-q", "main"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    // A commit given twice is picked once, as in Git.
    let commits = ["topic~1", "topic~2", "topic~1", "other"];
    let mut args = vec!["cherry-pick"];
    args.extend(commits);
    assert!(!git_output(t, &args).status.success());
    assert!(matches!(
        pick(o, &commits, false),
        PickOutcome::Conflicts { .. }
    ));
    for dir in [o, t] {
        write(dir, "f.txt", "1\nmain\ntopic\n3\n");
        git(dir, &["add", "f.txt"]);
    }
    assert!(matches!(
        open(o)
            .cherry_pick_continue("Test", "test@example.com")
            .unwrap(),
        PickOutcome::Conflicts { .. }
    ));
    assert!(
        !git_output(t, &["-c", "core.editor=true", "cherry-pick", "--continue"])
            .status
            .success()
    );
    assert_eq!(git_ls_files(o), git_ls_files(t));
    assert_eq!(worktree_files(o), worktree_files(t));
    assert_same_state(o, t);
    assert_eq!(reflog(o, 4), reflog(t, 4));

    // Aborting now goes back to the start.
    let before = git(temp.path(), &["rev-parse", "HEAD"]);
    open(o).cherry_pick_abort().unwrap();
    git(t, &["cherry-pick", "--abort"]);
    assert_eq!(git(o, &["rev-parse", "HEAD"]), before);
    assert_same(o, t, "base", true);
    assert_same_state(o, t);
}
