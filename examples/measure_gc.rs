//! Measures `Repository::gc()`: the objects a history written by zerogit
//! leaves, and the speed of reading it, before and after.
//!
//! ```text
//! cargo run --release --example measure_gc -- [--commits N] [--files N]
//! ```
//!
//! A repository of `--files` files (default 200, about 4 KB each) is
//! committed with Git, then zerogit makes `--commits` commits (default
//! 2,000), each changing three files, so every object of the history is
//! a loose object. The example reports the number and size of the objects,
//! the time of `log()` over the whole history and of `status()`, then runs
//! `gc()` and reports the same again. `git gc` on a copy gives the size
//! Git reaches with deltas, for reference. Git is used only to build the
//! repository and for that reference.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use zerogit::Repository;

const RUNS: usize = 5;

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

/// Loose objects, packs, and the total size of `objects/`.
fn objects(dir: &Path) -> (usize, usize, u64) {
    fn walk(dir: &Path, loose: &mut usize, packs: &mut usize, size: &mut u64) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, loose, packs, size);
                continue;
            }
            *size += entry.metadata().unwrap().len();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() == 38 {
                *loose += 1;
            } else if name.ends_with(".pack") {
                *packs += 1;
            }
        }
    }
    let (mut loose, mut packs, mut size) = (0, 0, 0);
    walk(&dir.join(".git/objects"), &mut loose, &mut packs, &mut size);
    (loose, packs, size)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn report(label: &str, dir: &Path, repo: &Repository, commits: usize) {
    let (loose, packs, size) = objects(dir);
    let log = median(|| assert_eq!(repo.log().unwrap().count(), commits + 1));
    let status = median(|| assert!(repo.status().unwrap().is_empty()));
    println!(
        "{:<10} {:>6} loose  {:>2} packs  {:>8.1} MB   log() {:>8.1?}   status() {:>8.1?}",
        label,
        loose,
        packs,
        size as f64 / 1_000_000.0,
        log,
        status
    );
}

fn main() {
    let mut commits = 2000;
    let mut files = 200;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("missing value");
        match arg.as_str() {
            "--commits" => commits = value().parse().expect("--commits N"),
            "--files" => files = value().parse().expect("--files N"),
            other => panic!("unknown argument: {}", other),
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    let line = "zerogit gc benchmark line with some text in it.\n";
    let mut contents: Vec<String> = (0..files)
        .map(|i| {
            let mut content = format!("file {}\n", i);
            while content.len() < 4000 {
                content.push_str(line);
            }
            content
        })
        .collect();
    let path = |i: usize| format!("dir{:02}/file{:04}.txt", i / 50, i);
    for (i, content) in contents.iter().enumerate() {
        let path = dir.join(path(i));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Generated"]);
    git(dir, &["gc", "-q"]);

    println!("writing {} commits with zerogit...", commits);
    let repo = Repository::open(dir).unwrap();
    for c in 0..commits {
        for k in 0..3 {
            let i = (c * 7 + k * 31) % files;
            contents[i].push_str(&format!("change {} of commit {}\n", k, c));
            std::fs::write(dir.join(path(i)), &contents[i]).unwrap();
            repo.add(path(i)).unwrap();
        }
        repo.create_commit(&format!("Commit {}", c), "Bench", "bench@example.com")
            .unwrap();
    }

    report("before", dir, &repo, commits);
    // The same history, for `git gc` to pack with deltas of its own.
    let reference = tempfile::tempdir().unwrap();
    copy_dir(dir, reference.path());
    let start = Instant::now();
    let summary = repo.gc().unwrap();
    println!(
        "gc()       {:.1?}: {} objects in one pack, {} deltas reused, {} new deltas, {} loose removed",
        start.elapsed(),
        summary.repack().objects(),
        summary.repack().reused_deltas(),
        summary.repack().new_deltas(),
        summary.repack().removed_loose()
    );
    report("after gc()", dir, &repo, commits);

    let start = Instant::now();
    git(reference.path(), &["gc", "-q"]);
    let elapsed = start.elapsed();
    let (_, packs, size) = objects(reference.path());
    println!(
        "git gc     {:.1?}: {} packs, {:.1} MB (with new deltas)",
        elapsed,
        packs,
        size as f64 / 1_000_000.0
    );
}
