//! `Repository::merge` compared with `git merge` (#29).
//!
//! Each scenario is set up once with Git, copied, and merged with Git in one
//! copy and with zerogit in the other; the index, work tree, HEAD and merge
//! state must agree.

mod common;

use common::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zerogit::{Error, FastForward, MergeOptions, MergeOutcome, Oid, Repository};

fn first_line(dir: &Path, file: &str) -> Option<String> {
    fs::read_to_string(dir.join(".git").join(file))
        .ok()
        .map(|s| s.lines().next().unwrap_or("").to_owned())
}

/// Asserts that the two repositories are in the same state after merging.
fn assert_same_state(ours: &Path, theirs: &Path) {
    assert_eq!(git_ls_files(ours), git_ls_files(theirs), "index");
    assert_eq!(worktree_files(ours), worktree_files(theirs), "work tree");
    assert_eq!(
        git(ours, &["rev-parse", "HEAD^{tree}"]),
        git(theirs, &["rev-parse", "HEAD^{tree}"]),
        "HEAD tree"
    );
    assert_eq!(
        git(ours, &["status", "--porcelain"]),
        git(theirs, &["status", "--porcelain"]),
        "status"
    );
    assert_eq!(
        first_line(ours, "MERGE_HEAD"),
        first_line(theirs, "MERGE_HEAD")
    );
    assert_eq!(
        first_line(ours, "MERGE_MSG"),
        first_line(theirs, "MERGE_MSG")
    );
    assert_eq!(
        first_line(ours, "ORIG_HEAD"),
        first_line(theirs, "ORIG_HEAD")
    );
}

/// Merges `target` with Git in one twin and zerogit in the other.
fn merge_both(
    source: &Path,
    target: &str,
    git_args: &[&str],
    options: &MergeOptions,
) -> (TempDir, TempDir, zerogit::Result<MergeOutcome>) {
    let ours = twin(source);
    let theirs = twin(source);
    let mut args = vec!["merge", "-q", "--no-edit"];
    args.extend_from_slice(git_args);
    args.push(target);
    let _ = git_output(theirs.path(), &args);
    let repo = Repository::open(ours.path()).unwrap();
    let outcome = repo.merge(target, "Test", "test@example.com", options);
    (ours, theirs, outcome)
}

/// main and topic diverge from "base"; `setup` adds commits on each side.
fn diverged(
    main_files: &[(&str, &str)],
    topic_files: &[(&str, &str)],
    base_files: &[(&str, &str)],
) -> TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    for (path, content) in base_files {
        write(dir, path, content);
    }
    write(dir, ".keep", "");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Base"]);
    git(dir, &["checkout", "-q", "-b", "topic"]);
    apply(dir, topic_files);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "Topic"]);
    git(dir, &["checkout", "-q", "main"]);
    apply(dir, main_files);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "Main"]);
    temp
}

/// Writes files; an empty content `"-"` deletes the file.
fn apply(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        if *content == "-" {
            fs::remove_file(dir.join(path)).unwrap();
        } else {
            write(dir, path, content);
        }
    }
    git(dir, &["add", "-A"]);
}

const BASE_TEXT: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";

#[test]
fn fast_forward_matches_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "topic"]);
    write(dir, "a.txt", "changed\n");
    write(dir, "dir/new.txt", "new\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Topic"]);
    git(dir, &["checkout", "-q", "main"]);
    // A local change on an untouched path survives.
    write(dir, "local.txt", "untracked\n");

    let (ours, theirs, outcome) = merge_both(dir, "topic", &[], &MergeOptions::new());
    let topic = Oid::from_hex(git(dir, &["rev-parse", "topic"]).trim()).unwrap();
    assert_eq!(outcome.unwrap(), MergeOutcome::FastForward(topic));
    assert_same_state(ours.path(), theirs.path());
    assert_eq!(
        git(ours.path(), &["rev-parse", "main"]).trim(),
        topic.to_hex()
    );
    assert_eq!(
        git(ours.path(), &["reflog", "-1", "--format=%gs"]),
        "merge topic: Fast-forward\n"
    );
}

#[test]
fn already_merged_is_up_to_date() {
    let temp = diverged(&[("m.txt", "m\n")], &[("t.txt", "t\n")], &[]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let before = git(dir, &["rev-parse", "HEAD"]);
    let base = git(dir, &["rev-parse", "main~1"]);
    assert_eq!(
        repo.merge(
            base.trim(),
            "Test",
            "test@example.com",
            &MergeOptions::new()
        )
        .unwrap(),
        MergeOutcome::UpToDate
    );
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), before);
    assert!(matches!(
        repo.merge(
            "no-such-branch",
            "Test",
            "test@example.com",
            &MergeOptions::new()
        ),
        Err(Error::RefNotFound(_))
    ));
}

#[test]
fn clean_three_way_merge_matches_git() {
    let temp = diverged(
        &[
            ("text.txt", "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\n"),
            ("main.txt", "m\n"),
            ("gone.txt", "-"),
        ],
        &[
            ("text.txt", "one\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n"),
            ("topic.txt", "t\n"),
        ],
        &[("text.txt", BASE_TEXT), ("gone.txt", "bye\n")],
    );
    let (ours, theirs, outcome) = merge_both(temp.path(), "topic", &[], &MergeOptions::new());
    let MergeOutcome::Merged(commit) = outcome.unwrap() else {
        panic!("expected a merge commit");
    };
    assert_same_state(ours.path(), theirs.path());
    let dir = ours.path();
    assert_eq!(git(dir, &["rev-parse", "HEAD"]).trim(), commit.to_hex());
    assert_eq!(
        git(dir, &["log", "-1", "--format=%P"]).trim(),
        format!(
            "{} {}",
            git(dir, &["rev-parse", "HEAD~1"]).trim(),
            git(dir, &["rev-parse", "topic"]).trim()
        )
    );
    assert_eq!(
        git(dir, &["log", "-1", "--format=%s"]),
        git(theirs.path(), &["log", "-1", "--format=%s"])
    );
    assert_fsck(dir);
}

#[test]
fn content_conflict_matches_git_and_can_be_concluded() {
    let temp = diverged(
        &[(
            "text.txt",
            "one\nTWO-main\nthree\nfour\nfive\nsix\nseven-main\n",
        )],
        &[(
            "text.txt",
            "one\nTWO-topic\nthree\nfour\nfive\nsix\nseven-topic\n",
        )],
        &[("text.txt", BASE_TEXT)],
    );
    let (ours, theirs, outcome) = merge_both(temp.path(), "topic", &[], &MergeOptions::new());
    assert_eq!(
        outcome.unwrap(),
        MergeOutcome::Conflicts(vec![PathBuf::from("text.txt")])
    );
    assert_same_state(ours.path(), theirs.path());

    let dir = ours.path();
    let repo = Repository::open(dir).unwrap();
    assert!(repo.merge_head().unwrap().is_some());
    assert!(matches!(
        repo.merge("topic", "Test", "test@example.com", &MergeOptions::new()),
        Err(Error::MergeInProgress)
    ));
    // Resolve and conclude with zerogit: a commit with two parents.
    write(dir, "text.txt", "resolved\n");
    repo.add("text.txt").unwrap();
    repo.create_commit("Merge branch 'topic'", "Test", "test@example.com")
        .unwrap();
    assert_eq!(
        git(dir, &["log", "-1", "--format=%P"]).split(' ').count(),
        2
    );
    assert!(!dir.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        git(dir, &["reflog", "-1", "--format=%gs"]),
        "commit (merge): Merge branch 'topic'\n"
    );
    assert_fsck(dir);
}

#[test]
fn git_can_continue_or_abort_a_zerogit_merge() {
    let temp = diverged(
        &[("text.txt", "one\nmain\nthree\nfour\nfive\nsix\nseven\n")],
        &[("text.txt", "one\ntopic\nthree\nfour\nfive\nsix\nseven\n")],
        &[("text.txt", BASE_TEXT)],
    );
    let dir = temp.path();
    let head = git(dir, &["rev-parse", "HEAD"]);
    let repo = Repository::open(dir).unwrap();
    repo.merge("topic", "Test", "test@example.com", &MergeOptions::new())
        .unwrap();
    let continued = twin(dir);
    git(dir, &["merge", "--abort"]);
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(dir, &["status", "--porcelain"]), "");

    let dir = continued.path();
    write(dir, "text.txt", "resolved\n");
    git(dir, &["add", "text.txt"]);
    git(dir, &["commit", "-q", "--no-edit"]);
    assert_eq!(
        git(dir, &["log", "-1", "--format=%P"]).split(' ').count(),
        2
    );
    assert_eq!(
        git(dir, &["log", "-1", "--format=%s"]),
        "Merge branch 'topic'\n"
    );
}

#[test]
fn abort_merge_restores_head_and_keeps_unrelated_changes() {
    let temp = diverged(
        &[("text.txt", "one\nmain\nthree\nfour\nfive\nsix\nseven\n")],
        &[
            ("text.txt", "one\ntopic\nthree\nfour\nfive\nsix\nseven\n"),
            ("added.txt", "new\n"),
        ],
        &[("text.txt", BASE_TEXT), ("other.txt", "other\n")],
    );
    let dir = temp.path();
    write(dir, "other.txt", "local edit\n");
    let repo = Repository::open(dir).unwrap();
    let outcome = repo
        .merge("topic", "Test", "test@example.com", &MergeOptions::new())
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Conflicts(_)));
    repo.abort_merge().unwrap();
    assert_eq!(git(dir, &["status", "--porcelain"]), " M other.txt\n");
    assert!(!dir.join("added.txt").exists());
    assert!(repo.merge_head().unwrap().is_none());
    assert!(matches!(repo.abort_merge(), Err(Error::NoMergeInProgress)));
}

#[test]
fn delete_and_add_conflicts_match_git() {
    let temp = diverged(
        &[
            ("modified.txt", "main change\n"),
            ("deleted-by-them.txt", "main change\n"),
            ("both-added.txt", "main\n"),
            ("mode.sh", "-"),
        ],
        &[
            ("modified.txt", "-"),
            ("deleted-by-them.txt", "-"),
            ("both-added.txt", "topic\n"),
        ],
        &[
            ("modified.txt", "base\n"),
            ("deleted-by-them.txt", "base\n"),
            ("mode.sh", "x\n"),
        ],
    );
    let (ours, theirs, outcome) = merge_both(temp.path(), "topic", &[], &MergeOptions::new());
    assert!(matches!(outcome.unwrap(), MergeOutcome::Conflicts(_)));
    assert_same_state(ours.path(), theirs.path());
}

#[test]
fn diff3_style_matches_git() {
    let temp = diverged(
        &[("text.txt", "one\nmain\nthree\nfour\nfive\nsix\nseven\n")],
        &[("text.txt", "one\ntopic\nthree\nfour\nfive\nsix\nseven\n")],
        &[("text.txt", BASE_TEXT)],
    );
    git(temp.path(), &["config", "merge.conflictStyle", "diff3"]);
    let (ours, theirs, _) = merge_both(temp.path(), "topic", &[], &MergeOptions::new());
    assert_same_state(ours.path(), theirs.path());
    assert!(fs::read_to_string(ours.path().join("text.txt"))
        .unwrap()
        .contains("|||||||"));
}

#[test]
fn fast_forward_options() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "topic"]);
    write(dir, "a.txt", "changed\n");
    git(dir, &["commit", "-q", "-am", "Topic"]);
    git(dir, &["checkout", "-q", "main"]);

    let no_ff = MergeOptions::new().fast_forward(FastForward::Never);
    let (ours, theirs, outcome) = merge_both(dir, "topic", &["--no-ff"], &no_ff);
    assert!(matches!(outcome.unwrap(), MergeOutcome::Merged(_)));
    assert_same_state(ours.path(), theirs.path());
    assert_eq!(
        git(ours.path(), &["log", "-1", "--format=%P"])
            .split(' ')
            .count(),
        2
    );

    let diverged = diverged(&[("m.txt", "m\n")], &[("t.txt", "t\n")], &[]);
    let repo = Repository::open(diverged.path()).unwrap();
    let only = MergeOptions::new().fast_forward(FastForward::Only);
    assert!(matches!(
        repo.merge("topic", "Test", "test@example.com", &only),
        Err(Error::NotFastForward)
    ));
}

#[test]
fn local_changes_in_the_way_are_refused() {
    let temp = diverged(
        &[("main.txt", "m\n")],
        &[("shared.txt", "topic\n"), ("new.txt", "topic\n")],
        &[("shared.txt", "base\n")],
    );
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = git(dir, &["rev-parse", "HEAD"]);

    write(dir, "shared.txt", "local edit\n");
    assert!(matches!(
        repo.merge("topic", "Test", "test@example.com", &MergeOptions::new()),
        Err(Error::LocalChangesWouldBeOverwritten(_))
    ));
    git(dir, &["checkout", "--", "shared.txt"]);

    write(dir, "new.txt", "untracked\n");
    assert!(matches!(
        repo.merge("topic", "Test", "test@example.com", &MergeOptions::new()),
        Err(Error::LocalChangesWouldBeOverwritten(_))
    ));
    fs::remove_file(dir.join("new.txt")).unwrap();

    write(dir, "main.txt", "staged\n");
    git(dir, &["add", "main.txt"]);
    assert!(matches!(
        repo.merge("topic", "Test", "test@example.com", &MergeOptions::new()),
        Err(Error::LocalChangesWouldBeOverwritten(_))
    ));
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert!(!dir.join(".git/MERGE_HEAD").exists());
}

#[test]
fn merge_message_and_targets_match_git() {
    let temp = diverged(&[("m.txt", "m\n")], &[("t.txt", "t\n")], &[]);
    let dir = temp.path();
    git(dir, &["tag", "-a", "-m", "tag", "v1", "topic"]);
    git(dir, &["checkout", "-q", "-b", "feature", "main"]);
    for target in ["topic", "v1"] {
        let (ours, theirs, outcome) = merge_both(dir, target, &[], &MergeOptions::new());
        assert!(matches!(outcome.unwrap(), MergeOutcome::Merged(_)));
        assert_eq!(
            git(ours.path(), &["log", "-1", "--format=%s"]),
            git(theirs.path(), &["log", "-1", "--format=%s"]),
            "{}",
            target
        );
        assert_same_state(ours.path(), theirs.path());
    }
}

#[test]
fn criss_cross_bases_match_git() {
    let temp = diverged(
        &[("a.txt", "one\nmain\nthree\nfour\nfive\nsix\nseven\n")],
        &[("b.txt", "topic\n")],
        &[("a.txt", BASE_TEXT)],
    );
    let dir = temp.path();
    // Merge each way to create two best common ancestors.
    git(dir, &["checkout", "-q", "-b", "m2", "main"]);
    git(dir, &["merge", "-q", "--no-edit", "topic"]);
    git(dir, &["checkout", "-q", "-b", "t2", "topic"]);
    git(dir, &["merge", "-q", "--no-edit", "main"]);
    write(dir, "a.txt", "one\nmain\nthree\nfour\nfive\nsix\nSEVEN\n");
    git(dir, &["commit", "-q", "-am", "t2 change"]);
    git(dir, &["checkout", "-q", "m2"]);
    write(dir, "c.txt", "m2\n");
    git(dir, &["add", "c.txt"]);
    git(dir, &["commit", "-q", "-m", "m2 change"]);

    let repo = Repository::open(dir).unwrap();
    let a = Oid::from_hex(git(dir, &["rev-parse", "m2"]).trim()).unwrap();
    let b = Oid::from_hex(git(dir, &["rev-parse", "t2"]).trim()).unwrap();
    let mut ours: Vec<String> = repo
        .merge_bases(&a, &b)
        .unwrap()
        .iter()
        .map(Oid::to_hex)
        .collect();
    ours.sort();
    let mut expected: Vec<String> = git(dir, &["merge-base", "--all", "m2", "t2"])
        .lines()
        .map(str::to_owned)
        .collect();
    expected.sort();
    assert_eq!(ours, expected);
    assert_eq!(ours.len(), 2);

    let (ours, theirs, outcome) = merge_both(dir, "t2", &[], &MergeOptions::new());
    assert!(matches!(outcome.unwrap(), MergeOutcome::Merged(_)));
    assert_same_state(ours.path(), theirs.path());
}

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn mutate(rng: &mut Rng, base: &[String], tag: &str) -> String {
    let mut out = String::new();
    for line in base {
        match rng.below(8) {
            0 => {}
            1 => out.push_str(&format!("{}{}\n", tag, rng.below(3))),
            2 => {
                out.push_str(line);
                out.push_str(&format!("{}{}\n", tag, rng.below(3)));
            }
            _ => out.push_str(line),
        }
    }
    out
}

#[test]
fn random_content_merges_match_git() {
    let mut rng = Rng(0x243F6A8885A308D3);
    let cases: usize = std::env::var("MERGE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    for case in 0..cases {
        let alphabet = [4, 12, 40][case % 3];
        let len = 3 + rng.below(20) as usize;
        let base: Vec<String> = (0..len)
            .map(|_| format!("l{}\n", rng.below(alphabet)))
            .collect();
        let main_text = mutate(&mut rng, &base, "m");
        let topic_text = mutate(&mut rng, &base, "t");
        let base_text = base.concat();
        let temp = diverged(
            &[("f.txt", &main_text), ("main-only.txt", "m\n")],
            &[("f.txt", &topic_text)],
            &[("f.txt", &base_text)],
        );
        let (ours, theirs, outcome) = merge_both(temp.path(), "topic", &[], &MergeOptions::new());
        outcome.unwrap();
        assert_eq!(
            worktree_files(ours.path()),
            worktree_files(theirs.path()),
            "case {}:\n--- base\n{}--- main\n{}--- topic\n{}",
            case,
            base_text,
            main_text,
            topic_text
        );
        assert_same_state(ours.path(), theirs.path());
    }
}
