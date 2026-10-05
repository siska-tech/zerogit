//! Log orders and graph columns compared with `git log` (#65).

mod common;

use common::*;
use std::path::Path;
use std::process::Command;
use zerogit::log::{Graph, LogOptions, LogOrder};
use zerogit::{Oid, Repository};

/// Commits on the current branch with a committer and author date.
fn commit_at(dir: &Path, message: &str, date: i64) {
    write(dir, &format!("{}.txt", message.replace(' ', "_")), message);
    git(dir, &["add", "-A"]);
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-q", "-m", message])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", format!("@{} +0000", date))
        .env("GIT_COMMITTER_DATE", format!("@{} +0000", date))
        .status()
        .unwrap();
    assert!(status.success());
}

fn merge_at(dir: &Path, branches: &[&str], message: &str, date: i64) {
    let mut args = vec!["merge", "-q", "--no-ff", "-m", message];
    args.extend_from_slice(branches);
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(&args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", format!("@{} +0000", date))
        .env("GIT_COMMITTER_DATE", format!("@{} +0000", date))
        .status()
        .unwrap();
    assert!(status.success());
}

/// Branches merged back, a criss-cross merge, an octopus merge, a child
/// dated before its parent, commits with equal dates, and author dates
/// that differ from the committer dates' order.
fn branchy() -> tempfile::TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    commit_at(dir, "root", 1000);
    commit_at(dir, "m1", 1100);
    git(dir, &["tag", "m1"]);
    git(dir, &["branch", "a"]);
    git(dir, &["branch", "b"]);
    commit_at(dir, "m2", 1200);
    git(dir, &["tag", "m2"]);
    git(dir, &["checkout", "-q", "a"]);
    commit_at(dir, "a1", 1150);
    // Older than its parent.
    commit_at(dir, "a2", 1050);
    git(dir, &["checkout", "-q", "b"]);
    // Equal dates.
    commit_at(dir, "b1", 1300);
    commit_at(dir, "b2", 1300);
    git(dir, &["checkout", "-q", "main"]);
    commit_at(dir, "m3", 1300);
    merge_at(dir, &["a"], "merge a", 1400);
    // Criss-cross between main and b.
    git(dir, &["checkout", "-q", "b"]);
    git(dir, &["branch", "b-before"]);
    merge_at(dir, &["main~1"], "b takes main", 1500);
    git(dir, &["checkout", "-q", "main"]);
    merge_at(dir, &["b-before"], "main takes b", 1500);
    git(dir, &["checkout", "-q", "-b", "c", "m1"]);
    commit_at(dir, "c1", 1600);
    git(dir, &["checkout", "-q", "-b", "d", "m2"]);
    commit_at(dir, "d1", 1650);
    git(dir, &["checkout", "-q", "main"]);
    merge_at(dir, &["b", "c", "d"], "octopus", 1700);
    commit_at(dir, "m4", 1800);
    temp
}

fn git_log(dir: &Path, args: &[&str]) -> Vec<String> {
    let mut all = vec!["log", "--format=%H"];
    all.extend_from_slice(args);
    git(dir, &all).lines().map(str::to_owned).collect()
}

fn our_log(dir: &Path, options: LogOptions) -> Vec<String> {
    Repository::open(dir)
        .unwrap()
        .log_with_options(options)
        .unwrap()
        .map(|commit| commit.unwrap().oid().to_hex())
        .collect()
}

#[test]
fn orders_match_git_log() {
    let temp = branchy();
    let dir = temp.path();
    let cases: Vec<(&[&str], LogOptions)> = vec![
        (&[], LogOptions::new()),
        (&["--date-order"], LogOptions::new().order(LogOrder::Date)),
        (&["--topo-order"], LogOptions::new().order(LogOrder::Topo)),
        (&["--reverse"], LogOptions::new().reverse(true)),
        (
            &["--topo-order", "--reverse"],
            LogOptions::new().order(LogOrder::Topo).reverse(true),
        ),
        (&["--first-parent"], LogOptions::new().first_parent(true)),
        (
            &["--first-parent", "--topo-order"],
            LogOptions::new().first_parent(true).order(LogOrder::Topo),
        ),
        (
            &["--first-parent", "--date-order", "--reverse"],
            LogOptions::new()
                .first_parent(true)
                .order(LogOrder::Date)
                .reverse(true),
        ),
        (
            &["-n", "5", "--reverse"],
            LogOptions::new().max_count(5).reverse(true),
        ),
        (
            &["-n", "7", "--topo-order"],
            LogOptions::new().max_count(7).order(LogOrder::Topo),
        ),
    ];
    for (args, options) in cases {
        assert_eq!(
            our_log(dir, options),
            git_log(dir, args),
            "git log {:?}",
            args
        );
    }
    // From another starting point.
    let b = Oid::from_hex(git(dir, &["rev-parse", "b"]).trim()).unwrap();
    assert_eq!(
        our_log(dir, LogOptions::new().from(b).order(LogOrder::Topo)),
        git_log(dir, &["--topo-order", "b"])
    );
}

/// The column of each commit in `git log --graph`, from the position of
/// its `*`.
fn git_graph(dir: &Path, args: &[&str]) -> Vec<(String, usize)> {
    let mut all = vec!["log", "--graph", "--format=%H"];
    all.extend_from_slice(args);
    git(dir, &all)
        .lines()
        .filter_map(|line| {
            let star = line.find('*')?;
            let hash = line.split_whitespace().last()?.to_owned();
            assert_eq!(star % 2, 0, "{}", line);
            Some((hash, star / 2))
        })
        .collect()
}

fn our_graph(dir: &Path, first_parent: bool) -> Vec<(String, usize)> {
    let repo = Repository::open(dir).unwrap();
    let options = LogOptions::new()
        .order(LogOrder::Topo)
        .first_parent(first_parent);
    let mut graph = Graph::new();
    repo.log_with_options(options)
        .unwrap()
        .map(|commit| {
            let commit = commit.unwrap();
            let parents = commit.parents();
            let parents = if first_parent {
                &parents[..parents.len().min(1)]
            } else {
                parents
            };
            let row = graph.push(*commit.oid(), parents);
            assert_eq!(row.commit(), commit.oid());
            assert_eq!(row.parent_columns().len(), parents.len());
            (commit.oid().to_hex(), row.column())
        })
        .collect()
}

#[test]
fn graph_columns_match_git_log_graph() {
    let temp = branchy();
    let dir = temp.path();
    assert_eq!(our_graph(dir, false), git_graph(dir, &[]));
    assert_eq!(our_graph(dir, true), git_graph(dir, &["--first-parent"]));

    // Two unrelated histories merged, and branch tips starting columns.
    let other = branchy();
    let o = other.path();
    git(o, &["checkout", "-q", "--orphan", "island"]);
    git(o, &["rm", "-rq", "--cached", "."]);
    commit_at(o, "island root", 1900);
    commit_at(o, "island next", 1950);
    git(o, &["checkout", "-q", "-f", "main"]);
    let status = Command::new("git")
        .arg("-C")
        .arg(o)
        .args([
            "merge",
            "-q",
            "--no-ff",
            "--allow-unrelated-histories",
            "-m",
            "join",
            "island",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", "@2000 +0000")
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(our_graph(o, false), git_graph(o, &[]));
}

#[test]
fn graph_rows_describe_the_lines() {
    let temp = branchy();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let mut graph = Graph::new();
    for commit in repo
        .log_with_options(LogOptions::new().order(LogOrder::Topo))
        .unwrap()
    {
        let commit = commit.unwrap();
        let row = graph.push(*commit.oid(), commit.parents());
        // The commit is on its column, or starts a line just past the end.
        match row.columns_before().get(row.column()) {
            Some(oid) => assert_eq!(oid, commit.oid()),
            None => assert_eq!(row.column(), row.columns_before().len()),
        }
        // Every other line goes on, and every parent has a line below.
        for (i, target) in row.column_mapping().iter().enumerate() {
            if i != row.column() {
                assert_eq!(
                    row.columns_after()[target.unwrap()],
                    row.columns_before()[i]
                );
            }
        }
        for (parent, column) in commit.parents().iter().zip(row.parent_columns()) {
            assert_eq!(&row.columns_after()[column], parent);
        }
    }
}
