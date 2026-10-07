//! Measures `Repository::blame()` against `git blame` on a repository's
//! files: whether every line is blamed on the same commit, line and path,
//! and how long each takes.
//!
//! ```text
//! cargo run --release --example measure_blame -- <repository> [<path>...]
//! ```
//!
//! Without paths, every file Git tracks at HEAD is blamed. Git is used only
//! for the reference results and times; its time includes starting the
//! `git` process for each file, zerogit's does not include opening the
//! repository.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use zerogit::{BlameOptions, Repository};

/// Each line's commit, original line and path from
/// `git blame --line-porcelain`, and the time it took.
fn git_blame(dir: &Path, path: &str) -> (Vec<(String, usize, String)>, Duration) {
    let start = Instant::now();
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["blame", "--line-porcelain", "HEAD", "--", path])
        .output()
        .expect("git must be installed");
    let elapsed = start.elapsed();
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    let mut current = None;
    for line in text.lines() {
        if let Some(file) = line.strip_prefix("filename ") {
            let (commit, original): (String, usize) = current.take().unwrap();
            result.push((commit, original, file.to_owned()));
        } else if !line.starts_with('\t') {
            let fields: Vec<&str> = line.split(' ').collect();
            if fields[0].len() == 40 && fields[0].bytes().all(|b| b.is_ascii_hexdigit()) {
                current = Some((fields[0].to_owned(), fields[1].parse().unwrap()));
            }
        }
    }
    (result, elapsed)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .expect("usage: measure_blame <repository> [<path>...]");
    let dir = Path::new(&dir);
    let mut paths: Vec<String> = args.collect();
    if paths.is_empty() {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["ls-files"])
            .output()
            .expect("git must be installed");
        paths = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
    }
    let repo = Repository::open(dir).unwrap();
    let (mut ours_total, mut git_total) = (Duration::ZERO, Duration::ZERO);
    let (mut files, mut lines, mut differing_files, mut differing_lines) = (0, 0, 0, 0);
    for path in &paths {
        let start = Instant::now();
        let blame = match repo.blame(path, &BlameOptions::new()) {
            Ok(blame) => blame,
            Err(e) => {
                println!("{}: {}", path, e);
                continue;
            }
        };
        ours_total += start.elapsed();
        let ours: Vec<(String, usize, String)> = blame
            .lines()
            .iter()
            .map(|l| {
                (
                    l.commit().to_hex(),
                    l.original_line(),
                    l.path().to_string_lossy().replace('\\', "/"),
                )
            })
            .collect();
        let (theirs, elapsed) = git_blame(dir, path);
        git_total += elapsed;
        files += 1;
        lines += theirs.len();
        if ours != theirs {
            let differing = ours.iter().zip(&theirs).filter(|(a, b)| a != b).count()
                + ours.len().abs_diff(theirs.len());
            differing_files += 1;
            differing_lines += differing;
            println!("{}: {} of {} lines differ", path, differing, theirs.len());
        }
    }
    println!(
        "{} files, {} lines: {} files ({} lines) differ; zerogit {:.1?}, git blame {:.1?}",
        files, lines, differing_files, differing_lines, ours_total, git_total
    );
}
