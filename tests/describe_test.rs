//! `Repository::describe` compared with `git describe` (#77).

mod common;

use common::*;
use std::path::Path;
use zerogit::{DescribeOptions, Error, Repository};

/// Runs Git with the committer and tagger date set to `time`.
fn git_at(dir: &Path, time: i64, args: &[&str]) -> String {
    let date = format!("{} +0000", time);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
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

fn commit_at(dir: &Path, time: i64, file: &str, message: &str) {
    write(dir, file, &format!("{} {}\n", message, time));
    git_at(dir, time, &["add", "-A"]);
    git_at(dir, time, &["commit", "-q", "-m", message]);
}

/// A history with a merge, tags of every kind, several tags on one
/// commit, a tag of a tree, and commit dates out of order.
///
/// ```text
///   A(v0.1) - B - C(light) - D(v0.3, v0.3-rc, light-d) - M - G - H
///              \                                       /
///               E(v0.2-side, nested) - F ------------ +
///                                       \
///                                        X (other, no tags, older date)
/// ```
fn history() -> tempfile::TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    let t = 1_700_000_000;
    commit_at(dir, t, "a.txt", "A");
    git_at(dir, t + 1, &["tag", "-a", "v0.1", "-m", "v0.1"]);
    commit_at(dir, t + 10, "a.txt", "B");
    git_at(dir, t + 10, &["branch", "side"]);
    commit_at(dir, t + 20, "a.txt", "C");
    git_at(dir, t + 20, &["tag", "light"]);
    commit_at(dir, t + 30, "a.txt", "D");
    // On D: an older and a newer annotated tag, and a lightweight one.
    git_at(dir, t + 40, &["tag", "-a", "v0.3", "-m", "v0.3"]);
    git_at(dir, t + 35, &["tag", "-a", "v0.3-rc", "-m", "rc"]);
    git_at(dir, t + 30, &["tag", "light-d"]);
    git_at(dir, t + 30, &["checkout", "-q", "side"]);
    commit_at(dir, t + 15, "b.txt", "E");
    git_at(dir, t + 16, &["tag", "-a", "v0.2-side", "-m", "side"]);
    git_at(
        dir,
        t + 17,
        &["tag", "-a", "nested", "-m", "tag of a tag", "v0.2-side"],
    );
    // Committed with a date older than its parent (clock skew).
    commit_at(dir, t + 5, "b.txt", "F");
    git_at(dir, t + 5, &["checkout", "-q", "-b", "other"]);
    commit_at(dir, t + 2, "c.txt", "X");
    git_at(dir, t + 50, &["checkout", "-q", "main"]);
    git_at(dir, t + 50, &["merge", "-q", "--no-ff", "-m", "M", "side"]);
    commit_at(dir, t + 60, "a.txt", "G");
    commit_at(dir, t + 70, "a.txt", "H");
    let tree = git(dir, &["rev-parse", "HEAD^{tree}"]);
    git_at(
        dir,
        t + 70,
        &["tag", "-a", "tree-tag", "-m", "a tree", tree.trim()],
    );
    temp
}

/// `git describe <args> <commit>` and ours, as text or error.
fn both(dir: &Path, args: &[&str], commit: &str) -> (Option<String>, Option<String>) {
    let mut command = vec!["describe"];
    command.extend_from_slice(args);
    command.push(commit);
    let output = git_output(dir, &command);
    let theirs = output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned());

    let mut options = DescribeOptions::new();
    for arg in args {
        options = match *arg {
            "--tags" => options.tags(true),
            "--all" => options.all(true),
            "--long" => options.long(true),
            "--always" => options.always(true),
            "--first-parent" => options.first_parent(true),
            other => {
                if let Some(n) = other.strip_prefix("--abbrev=") {
                    options.abbrev(n.parse().unwrap())
                } else if let Some(n) = other.strip_prefix("--candidates=") {
                    options.candidates(n.parse().unwrap())
                } else if let Some(p) = other.strip_prefix("--match=") {
                    options.patterns(&[p])
                } else if let Some(p) = other.strip_prefix("--exclude=") {
                    options.exclude(&[p])
                } else {
                    panic!("unknown option {}", other)
                }
            }
        };
    }
    let ours = match Repository::open(dir)
        .unwrap()
        .describe_revision(commit, &options)
    {
        Ok(description) => Some(description.to_string()),
        Err(Error::NoDescription { .. }) => None,
        Err(e) => panic!("{:?} {} {:?}", args, commit, e),
    };
    (ours, theirs)
}

#[test]
fn describe_matches_git_describe() {
    let temp = history();
    let dir = temp.path();
    let commits: Vec<String> = git(dir, &["rev-list", "--all"])
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(commits.len(), 10);
    let option_sets: &[&[&str]] = &[
        &[],
        &["--tags"],
        &["--all"],
        &["--long"],
        &["--tags", "--long"],
        &["--abbrev=0"],
        &["--abbrev=4"],
        &["--abbrev=12"],
        &["--first-parent"],
        &["--tags", "--first-parent"],
        &["--candidates=1"],
        &["--tags", "--candidates=2"],
        &["--candidates=0"],
        &["--tags", "--candidates=0"],
        &["--always"],
        &["--match=v0.2*"],
        &["--tags", "--match=light*"],
        &["--exclude=v0.3*"],
        &["--tags", "--exclude=v*"],
        &["--all", "--match=main"],
        &["--all", "--exclude=*"],
        &["--match=nomatch", "--always"],
    ];
    let mut described = 0;
    for args in option_sets {
        for commit in &commits {
            let (ours, theirs) = both(dir, args, commit);
            assert_eq!(ours, theirs, "describe {:?} {}", args, commit);
            described += usize::from(theirs.is_some());
        }
        // HEAD and a revision expression too.
        for revision in ["HEAD", "HEAD~2", "side", "v0.3"] {
            let (ours, theirs) = both(dir, args, revision);
            assert_eq!(ours, theirs, "describe {:?} {}", args, revision);
        }
    }
    assert!(described > 150);

    // The error tells whether lightweight tags would have helped.
    let repo = Repository::open(dir).unwrap();
    let options = DescribeOptions::new().patterns(&["light"]);
    assert!(matches!(
        repo.describe_revision("light", &options),
        Err(Error::NoDescription {
            unannotated_tags: true,
            ..
        })
    ));
    let description = repo
        .describe_revision("main", &DescribeOptions::new())
        .unwrap();
    assert_eq!(description.tag(), Some("v0.3"));
    assert_eq!(description.distance(), 5);
}

#[test]
fn dirty_suffix_like_git() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let options = DescribeOptions::new().dirty(Some("-dirty"));
    let ours = |repo: &Repository| repo.describe(&options).unwrap().to_string();
    let theirs = || git(dir, &["describe", "--dirty"]).trim().to_owned();

    // Untracked files do not count.
    write(dir, "untracked.txt", "new\n");
    assert_eq!(ours(&repo), theirs());
    assert!(!repo.describe(&options).unwrap().is_dirty());
    // A changed tracked file does, staged or not.
    write(dir, "a.txt", "changed\n");
    assert_eq!(ours(&repo), theirs());
    assert!(ours(&repo).ends_with("-dirty"));
    git(dir, &["add", "a.txt"]);
    assert_eq!(ours(&repo), theirs());
    let custom = DescribeOptions::new().dirty(Some(".mod"));
    assert!(repo
        .describe(&custom)
        .unwrap()
        .to_string()
        .ends_with(".mod"));

    // Only HEAD can be dirty.
    assert!(matches!(
        repo.describe_revision("HEAD", &options),
        Err(Error::InvalidRevision { .. })
    ));
}

#[test]
fn abbreviations_follow_core_abbrev() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    for value in ["4", "9", "40"] {
        git(dir, &["config", "core.abbrev", value]);
        let theirs = git(dir, &["describe", "--tags"]).trim().to_owned();
        assert_eq!(
            repo.describe(&DescribeOptions::new().tags(true))
                .unwrap()
                .to_string(),
            theirs
        );
    }
}
