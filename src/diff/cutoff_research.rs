//! Research tools for finding out when and how Git's default line diff
//! stops looking for a minimal diff, from Git's output only. Not tests of
//! zerogit: `#[ignore]`d, run on demand, writing to `CUTOFF_OUT`.
//!
//! See `docs/diff-compat-research.md`, finding 4.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::compat_tests::{git_headers, Rng};
use super::histogram::Algorithm;

/// The number of changed lines (both sides) in hunk headers.
fn changed(headers: &[String]) -> usize {
    headers
        .iter()
        .map(|h| {
            let mut total = 0;
            for part in h.trim_start_matches("@@ ").split(' ') {
                let range = part.trim_start_matches(['-', '+']);
                total += match range.split_once(',') {
                    Some((_, len)) => len.parse::<usize>().unwrap(),
                    None => 1,
                };
            }
            total
        })
        .sum()
}

/// Random pairs of growing size and edit density: the minimal number of
/// changed lines (`--minimal`) and the default's.
#[test]
#[ignore]
fn cutoff_dataset() {
    let out = PathBuf::from(std::env::var("CUTOFF_OUT").expect("CUTOFF_OUT"));
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut table = String::from("len\talphabet\tdensity\tminimal\tdefault\n");
    for &len in &[50usize, 100, 200, 400, 800, 1600, 3200] {
        for &alphabet in &[4u64, 16, 64, 100_000] {
            for &density in &[5u64, 10, 20, 35, 50] {
                for _ in 0..3 {
                    let old: Vec<String> = (0..len)
                        .map(|_| format!("l{}\n", rng.below(alphabet)))
                        .collect();
                    let mut new = Vec::new();
                    for line in &old {
                        if rng.below(100) < density {
                            match rng.below(3) {
                                0 => {}
                                1 => new.push(format!("l{}\n", rng.below(alphabet))),
                                _ => {
                                    new.push(line.clone());
                                    new.push(format!("l{}\n", rng.below(alphabet)));
                                }
                            }
                        } else {
                            new.push(line.clone());
                        }
                    }
                    std::fs::write(dir.join("old"), old.concat()).unwrap();
                    std::fs::write(dir.join("new"), new.concat()).unwrap();
                    let minimal = changed(&git_headers(dir, "old", "new", Algorithm::Minimal));
                    let default = changed(&git_headers(dir, "old", "new", Algorithm::Myers));
                    writeln!(
                        table,
                        "{}\t{}\t{}\t{}\t{}",
                        len, alphabet, density, minimal, default
                    )
                    .unwrap();
                }
            }
        }
    }
    std::fs::write(out.join("cutoff_dataset.tsv"), table).unwrap();
}
