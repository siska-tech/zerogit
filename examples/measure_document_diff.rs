//! Measures line diff cost and packed repository reads for document histories.
//!
//! Run with `cargo run --release --example measure_document_diff`. Git is used
//! only to pack the generated repository; zerogit itself never runs Git.
//! The results back the defaults of `DiffOptions` (see docs/).

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use zerogit::{BlobDiff, BlobDiffContent, DiffOptions, Repository};

/// A deterministic document of `lines` lines, about 60 bytes each.
fn document(lines: usize, seed: usize) -> String {
    (0..lines)
        .map(|i| {
            format!(
                "{:05} 本文の行です。revision {} の内容を含む文章。\n",
                i, seed
            )
        })
        .collect()
}

/// A worst case: lines drawn from a small vocabulary (blank lines, rules,
/// list markers), so most lines also occur on the other side.
fn repetitive(lines: usize, seed: u64) -> String {
    const VOCABULARY: [&str; 8] = [
        "",
        "---",
        "- [ ] 未完了",
        "- [x] 完了",
        "## 見出し",
        "> 引用",
        "本文",
        "```",
    ];
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..lines)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            format!(
                "{}\n",
                VOCABULARY[(state >> 33) as usize % VOCABULARY.len()]
            )
        })
        .collect()
}

/// Rewrites every `step`-th line of `text`.
fn edit(text: &str, step: usize) -> String {
    text.lines()
        .enumerate()
        .map(|(i, line)| {
            if i % step == 0 {
                format!("{} (edited)\n", line)
            } else {
                format!("{}\n", line)
            }
        })
        .collect()
}

/// Returns the smallest power-of-two budget that completes, and the time taken.
fn cost_and_time(old: &str, new: &str) -> (u64, Duration) {
    let mut budget = 1u64 << 10;
    loop {
        let options = DiffOptions::new().max_cost(budget);
        let started = Instant::now();
        let diff = BlobDiff::compute(Some(old.as_bytes()), Some(new.as_bytes()), &options);
        let elapsed = started.elapsed();
        if let BlobDiffContent::Text(_) = diff.content() {
            return (budget, elapsed);
        }
        budget *= 2;
    }
}

fn line_diff_table() {
    println!("## Line diff cost (budget = smallest power of two that completes)\n");
    println!("| lines | changed | bytes | budget | time |");
    println!("|---:|---:|---:|---:|---:|");
    for lines in [1_000, 5_000, 20_000] {
        let old = document(lines, 0);
        let repeated = repetitive(lines, 1);
        for (label, old, new) in [
            ("1%", &old, edit(&old, 100)),
            ("10%", &old, edit(&old, 10)),
            ("100%", &old, document(lines, 1)),
            ("10% repetitive", &repeated, edit(&repeated, 10)),
            ("100% repetitive", &repeated, repetitive(lines, 2)),
        ] {
            let (budget, time) = cost_and_time(old, &new);
            println!(
                "| {} | {} | {} | {} | {:.1} ms |",
                lines,
                label,
                old.len(),
                budget,
                time.as_secs_f64() * 1000.0
            );
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Bench",
            "-c",
            "user.email=bench@example.com",
        ])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .status()
        .expect("git is required to pack the measurement repository");
    assert!(status.success(), "git {:?}", args);
}

fn repository_table(dir: &Path, commits: usize, files: usize) {
    let repo = Repository::init(dir).unwrap();
    let mut texts: Vec<String> = (0..files).map(|f| document(300 + f * 20, f)).collect();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    for c in 0..commits {
        let f = c % files;
        texts[f] = edit(&texts[f], 7 + c % 13).replace("(edited) (edited)", "(re-edited)");
        let path = format!("docs/doc{:03}.md", f);
        std::fs::write(dir.join(&path), &texts[f]).unwrap();
        repo.add(&path).unwrap();
        repo.create_commit(&format!("Edit {}", c), "Bench", "bench@example.com")
            .unwrap();
    }
    git(dir, &["repack", "-a", "-d", "-q"]);
    git(dir, &["pack-refs", "--all", "--prune"]);
    let pack_bytes: u64 = std::fs::read_dir(dir.join(".git/objects/pack"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();

    println!(
        "\n## Packed repository ({} commits, {} documents, pack {} KiB)\n",
        commits,
        files,
        pack_bytes / 1024
    );
    println!("| step | time |");
    println!("|---|---:|");
    let started = Instant::now();
    let repo = Repository::open(dir).unwrap();
    let log: Vec<_> = repo.log().unwrap().map(Result::unwrap).collect();
    println!(
        "| open + full log ({} commits) | {:.1} ms |",
        log.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );

    let started = Instant::now();
    let mut deltas = Vec::new();
    for commit in &log {
        deltas.extend(repo.commit_diff(commit).unwrap().deltas().to_vec());
    }
    println!(
        "| commit_diff for every commit ({} deltas) | {:.1} ms |",
        deltas.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );

    let started = Instant::now();
    let options = DiffOptions::new();
    let mut lines = 0;
    for delta in &deltas {
        let diff = repo
            .diff_blobs(delta.old_oid(), delta.new_oid(), &options)
            .unwrap();
        lines += diff.lines_added().expect("text diff");
    }
    let elapsed = started.elapsed();
    println!(
        "| diff_blobs for every delta ({} added lines) | {:.1} ms ({:.2} ms each) |",
        lines,
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / deltas.len() as f64
    );
}

fn main() {
    line_diff_table();
    let temp = std::env::temp_dir().join(format!("zerogit-measure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    repository_table(&temp, 1_000, 50);
    let _ = std::fs::remove_dir_all(&temp);
}
