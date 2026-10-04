//! Measures `Repository::status()` against `git status` on a large, clean
//! working tree.
//!
//! ```text
//! cargo run --release --example measure_status -- [--files N] [--size BYTES] [--repo PATH]
//! ```
//!
//! Without `--repo`, a repository of `--files` files (default 4,000) of
//! about `--size` bytes each (default 55,000, so 220 MB in all) is generated
//! in a temporary directory and committed with Git. Git is used only to
//! build the repository and as the reference timing; zerogit itself never
//! runs Git.
//!
//! Each case reports the median of several runs:
//! - the index as Git wrote it,
//! - the index after zerogit rewrote it (`add_all`),
//! - `git status --porcelain` on the same tree.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use zerogit::Repository;

const RUNS: usize = 5;

struct Args {
    files: usize,
    size: usize,
    repo: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        files: 4000,
        size: 55_000,
        repo: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().expect("missing value");
        match arg.as_str() {
            "--files" => args.files = value().parse().expect("--files N"),
            "--size" => args.size = value().parse().expect("--size BYTES"),
            "--repo" => args.repo = Some(PathBuf::from(value())),
            other => panic!("unknown argument: {}", other),
        }
    }
    args
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.autocrlf=false", "-c", "gc.auto=0"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "Bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        .status()
        .expect("git must be installed");
    assert!(status.success(), "git {:?} failed", args);
}

/// Writes `files` files of about `size` bytes in directories of 100.
fn generate(dir: &Path, files: usize, size: usize) {
    git(dir, &["init", "-q"]);
    let line = "zerogit status benchmark line with some text to hash.\n";
    for i in 0..files {
        let sub = dir.join(format!("dir{:03}", i / 100));
        std::fs::create_dir_all(&sub).unwrap();
        let mut content = format!("file {}\n", i);
        while content.len() < size {
            content.push_str(line);
        }
        std::fs::write(sub.join(format!("file{:05}.txt", i)), content).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Generated"]);
}

fn median(mut f: impl FnMut()) -> Duration {
    // One warm-up run fills the file system cache.
    f();
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[RUNS / 2]
}

fn main() {
    let args = parse_args();
    let temp;
    let dir = match &args.repo {
        Some(repo) => repo.clone(),
        None => {
            temp = tempfile::tempdir().unwrap();
            println!(
                "generating {} files of ~{} bytes ({} MB)...",
                args.files,
                args.size,
                args.files * args.size / 1_000_000
            );
            generate(temp.path(), args.files, args.size);
            temp.path().to_path_buf()
        }
    };
    let repo = Repository::open(&dir).unwrap();

    let git_status = median(|| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(output.status.success());
    });
    let index_by_git = median(|| {
        let entries = repo.status().unwrap();
        assert!(entries.is_empty(), "{:?}", &entries[..entries.len().min(5)]);
    });
    repo.add_all().unwrap();
    let index_by_zerogit = median(|| {
        let entries = repo.status().unwrap();
        assert!(entries.is_empty(), "{:?}", &entries[..entries.len().min(5)]);
    });

    let ratio = |d: Duration| d.as_secs_f64() / git_status.as_secs_f64();
    println!("git status --porcelain         {:>8.1?}", git_status);
    println!(
        "status() (index by Git)        {:>8.1?}  ({:.1}x git)",
        index_by_git,
        ratio(index_by_git)
    );
    println!(
        "status() (index by zerogit)    {:>8.1?}  ({:.1}x git)",
        index_by_zerogit,
        ratio(index_by_zerogit)
    );
}
