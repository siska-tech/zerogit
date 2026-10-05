//! Measures `log` orders on a large history: how soon the first commits
//! come, and how long the whole history takes.
//!
//! ```text
//! cargo run --release --example measure_log -- [--commits N]
//! ```
//!
//! A history of `--commits` commits (default 50,000) is generated with
//! `git fast-import`: a main line where, every 50 commits, a side branch of
//! 10 commits is merged. Git is used only to build the repository and as
//! the reference timing (`git log --format=%H`, without a commit-graph
//! file, which zerogit does not use either).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zerogit::log::{LogOptions, LogOrder};
use zerogit::Repository;

fn generate(dir: &Path, commits: usize) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["init", "-q"])
        .status()
        .expect("git must be installed");
    assert!(status.success());
    let mut stream = String::new();
    let mut mark = 0;
    let mut time = 1_000_000_000i64;
    let mut main_tip: Option<usize> = None;
    let mut commit = |stream: &mut String, branch: &str, parents: &[usize], time: i64| {
        mark += 1;
        let message = format!("commit {}", mark);
        stream.push_str(&format!(
            "commit refs/heads/{}\nmark :{}\ncommitter Bench <bench@example.com> {} +0000\ndata {}\n{}\n",
            branch,
            mark,
            time,
            message.len(),
            message
        ));
        if let Some(first) = parents.first() {
            stream.push_str(&format!("from :{}\n", first));
        }
        if let Some(merged) = parents.get(1) {
            stream.push_str(&format!("merge :{}\n", merged));
        }
        stream.push_str(&format!(
            "M 644 inline file{}.txt\ndata {}\n{}\n\n",
            mark % 100,
            message.len(),
            message
        ));
        mark
    };
    let mut made = 0;
    while made < commits {
        if made % 60 == 50 {
            // A side branch of 10 commits, merged into main.
            let base = main_tip.expect("main has commits");
            let mut side = base;
            for _ in 0..9 {
                time += 30;
                side = commit(&mut stream, "side", &[side], time);
            }
            time += 30;
            main_tip = Some(commit(
                &mut stream,
                "main",
                &[main_tip.unwrap(), side],
                time,
            ));
            made += 10;
        } else {
            time += 60;
            let parents: Vec<usize> = main_tip.into_iter().collect();
            main_tip = Some(commit(&mut stream, "main", &parents, time));
            made += 1;
        }
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["symbolic-ref", "HEAD", "refs/heads/main"])
        .status()
        .unwrap();
    assert!(status.success());
}

/// The best of three runs (the first warms the file system cache).
fn time(f: impl Fn() -> usize) -> (Duration, usize) {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            let n = f();
            (start.elapsed(), n)
        })
        .min()
        .unwrap()
}

fn git_log(dir: &Path, args: &[&str]) -> usize {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.commitGraph=false", "log", "--format=%H"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    output.stdout.iter().filter(|&&b| b == b'\n').count()
}

fn main() {
    let mut commits = 50_000;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--commits" => commits = args.next().expect("--commits N").parse().unwrap(),
            other => panic!("unknown argument: {}", other),
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    println!("generating {} commits...", commits);
    generate(dir, commits);
    let repo = Repository::open(dir).unwrap();
    let ours = |order: LogOrder, n: Option<usize>| {
        let mut options = LogOptions::new().order(order);
        if let Some(n) = n {
            options = options.max_count(n);
        }
        time(|| repo.log_with_options(options.clone()).unwrap().count())
    };
    println!("{:<14} {:>14} {:>14}", "", "first 10", "all");
    for (label, order, git_args) in [
        ("default", LogOrder::Default, &[][..]),
        ("--date-order", LogOrder::Date, &["--date-order"][..]),
        ("--topo-order", LogOrder::Topo, &["--topo-order"][..]),
    ] {
        let (first, _) = ours(order, Some(10));
        let (all, n) = ours(order, None);
        assert_eq!(n, git_log(dir, git_args));
        println!("zerogit {:<14} {:>10.1?} {:>12.1?}", label, first, all);
        let mut with_n = vec!["-n", "10"];
        with_n.extend_from_slice(git_args);
        let (git_first, _) = time(|| git_log(dir, &with_n));
        let (git_all, _) = time(|| git_log(dir, git_args));
        println!(
            "git     {:<14} {:>10.1?} {:>12.1?}",
            label, git_first, git_all
        );
    }
}
