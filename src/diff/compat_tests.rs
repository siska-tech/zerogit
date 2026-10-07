//! Black-box compatibility tests: zerogit's line diffs compared with the
//! hunks `git diff -U0` reports for the same inputs.
//!
//! Only Git's observable output is used: stored expected outputs for real
//! file pairs (`tests/data/diff_compat/`, recorded with Git 2.55) and
//! random inputs diffed by the installed Git. Sizes are kept small by
//! default; `COMPAT_SCALE` multiplies the number of random cases.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::histogram::{diff_bytes, Algorithm, Hunk};

/// The `@@ -a,b +c,d` header Git prints for a hunk.
fn header(hunk: &Hunk) -> String {
    let range = |start: usize, len: usize| match len {
        0 => format!("{},0", start),
        1 => format!("{}", start + 1),
        _ => format!("{},{}", start + 1, len),
    };
    format!(
        "@@ -{} +{}",
        range(hunk.old_start, hunk.old_len),
        range(hunk.new_start, hunk.new_len)
    )
}

fn headers(hunks: &[Hunk]) -> Vec<String> {
    hunks.iter().map(header).collect()
}

fn flag(algorithm: Algorithm) -> &'static str {
    match algorithm {
        Algorithm::Myers => "--diff-algorithm=myers",
        Algorithm::Minimal => "--minimal",
        Algorithm::Histogram => "--histogram",
    }
}

/// `git diff --no-index -U0` of two files, as hunk headers.
pub(super) fn git_headers(dir: &Path, old: &str, new: &str, algorithm: Algorithm) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["-c", "core.autocrlf=false", "diff", "--no-index", "-U0"])
        .arg(flag(algorithm))
        .args([old, new])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.starts_with("@@"))
        .map(|l| l.split(" @@").next().unwrap().to_owned())
        .collect()
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/diff_compat")
}

fn scale() -> usize {
    std::env::var("COMPAT_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

/// Pairs zerogit is known to diff differently from Git, by algorithm:
/// large rewrites, where Git's default diff stops looking for a minimal
/// diff in a way zerogit does not reproduce (`docs/diff-compat-research.md`,
/// finding 4). [`known_differences_still_differ`] keeps track of them.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[("repository-6ac8a62", "myers")];

/// Compares the stored pairs with Git's headers for `algorithm`: each
/// `<name>.old` and `<name>.new` with `<name>.<suffix>.expected`. With
/// `known`, only the pairs in [`KNOWN_DIFFERENCES`], otherwise the others.
fn check_fixtures(algorithm: Algorithm, suffix: &str, known: bool) {
    let dir = data_dir();
    let mut failures = Vec::new();
    let mut pairs = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("old") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if KNOWN_DIFFERENCES.contains(&(name.as_str(), suffix)) != known {
            continue;
        }
        let old = std::fs::read(&path).unwrap();
        let new = std::fs::read(dir.join(format!("{}.new", name))).unwrap();
        let expected: Vec<String> =
            std::fs::read_to_string(dir.join(format!("{}.{}.expected", name, suffix)))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect();
        let ours = headers(&diff_bytes(&old, &new, algorithm));
        pairs += 1;
        if ours != expected {
            let first = ours
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(ours.len().min(expected.len()));
            failures.push(format!(
                "{}: {} hunks, Git {}; first difference at {}: ours {:?}, Git {:?}",
                name,
                ours.len(),
                expected.len(),
                first,
                ours.get(first),
                expected.get(first)
            ));
        }
    }
    assert!(known || pairs >= 4);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fixtures_match_git_myers() {
    check_fixtures(Algorithm::Myers, "myers", false);
}

#[test]
fn fixtures_match_git_minimal() {
    check_fixtures(Algorithm::Minimal, "minimal", false);
}

#[test]
fn fixtures_match_git_histogram() {
    check_fixtures(Algorithm::Histogram, "histogram", false);
}

/// The known differences (see [`KNOWN_DIFFERENCES`]); passes once they are
/// resolved, at which point they should leave the list.
#[test]
#[ignore = "known difference on large rewrites (docs/diff-compat-research.md, finding 4)"]
fn known_differences_still_differ() {
    for algorithm in [Algorithm::Myers, Algorithm::Minimal, Algorithm::Histogram] {
        let suffix = match algorithm {
            Algorithm::Myers => "myers",
            Algorithm::Minimal => "minimal",
            Algorithm::Histogram => "histogram",
        };
        check_fixtures(algorithm, suffix, true);
    }
}

/// A xorshift generator, so that every run uses the same cases.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// Diffs generated pairs with zerogit and Git and reports the mismatches.
fn compare_random(
    algorithm: Algorithm,
    cases: usize,
    mut generate: impl FnMut(&mut Rng, usize) -> (String, String),
    seed: u64,
) {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();
    let mut rng = Rng(seed);
    let mut mismatches = Vec::new();
    for case in 0..cases {
        let (old, new) = generate(&mut rng, case);
        std::fs::write(dir.join("old"), &old).unwrap();
        std::fs::write(dir.join("new"), &new).unwrap();
        let ours = headers(&diff_bytes(old.as_bytes(), new.as_bytes(), algorithm));
        let theirs = git_headers(dir, "old", "new", algorithm);
        if ours != theirs {
            mismatches.push(format!(
                "case {}:\n--- old\n{}--- new\n{}git:  {:?}\nours: {:?}",
                case, old, new, theirs, ours
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches of {}, first:\n{}",
        mismatches.len(),
        cases,
        mismatches[0]
    );
}

/// A C-like function: indented lines, blank lines, a closing brace.
fn code_block(rng: &mut Rng, name: u64) -> String {
    let mut text = format!("fn f{}() {{\n", name);
    for _ in 0..1 + rng.below(4) {
        match rng.below(5) {
            0 => text.push_str("    if x {\n        y();\n    }\n"),
            1 => text.push('\n'),
            2 => text.push_str("\tz();\n"),
            _ => text.push_str(&format!("    call{}();\n", rng.below(3))),
        }
    }
    text.push_str("}\n\n");
    text
}

/// Code-like text with functions added, removed, edited and duplicated.
fn code_pair(rng: &mut Rng) -> (String, String) {
    let blocks: Vec<String> = (0..2 + rng.below(5)).map(|i| code_block(rng, i)).collect();
    let old: String = blocks.concat();
    let mut new = String::new();
    for block in &blocks {
        match rng.below(6) {
            0 => {
                let name = 10 + rng.below(5);
                new.push_str(&code_block(rng, name));
                new.push_str(block);
            }
            1 => {}
            2 => {
                for line in block.lines() {
                    match rng.below(6) {
                        0 => {}
                        1 => new.push_str("    added();\n"),
                        2 => {
                            new.push_str(line);
                            new.push_str("\n\n");
                        }
                        _ => {
                            new.push_str(line);
                            new.push('\n');
                        }
                    }
                }
            }
            3 => {
                let name = 20 + rng.below(5);
                let extra = code_block(rng, name);
                new.push_str(&block[..block.len() - 1]);
                new.push_str(&extra[..extra.len() - 1]);
                new.push('\n');
            }
            4 => {
                new.push_str(block);
                new.push_str(block);
            }
            _ => new.push_str(block),
        }
    }
    (old, new)
}

/// Short indented and blank lines with a slice repeated or removed, where
/// a changed group can sit in several places.
fn slider_pair(rng: &mut Rng) -> (String, String) {
    let vocabulary = ["a\n", "\n", "  b\n", "c\n", "    d\n", "\te\n", "}\n"];
    let old: Vec<&str> = (0..4 + rng.below(12))
        .map(|_| vocabulary[rng.below(vocabulary.len() as u64) as usize])
        .collect();
    let start = rng.below(old.len() as u64) as usize;
    let end = start + 1 + rng.below((old.len() - start) as u64) as usize;
    let mut new = old.clone();
    if rng.below(3) == 0 {
        new.drain(start..end);
    } else {
        let copy: Vec<&str> = old[start..end].to_vec();
        new.splice(end..end, copy);
    }
    (old.concat(), new.concat())
}

#[test]
fn code_and_slider_diffs_match_git_myers() {
    compare_random(
        Algorithm::Myers,
        300 * scale(),
        |rng, case| {
            if case % 2 == 0 {
                code_pair(rng)
            } else {
                slider_pair(rng)
            }
        },
        0x2545F4914F6CDD1D,
    );
}

/// Lines from a small alphabet, edited in place: many equal lines.
fn alphabet_pair(rng: &mut Rng, len: usize, alphabet: u64) -> (String, String) {
    let old: String = (0..len)
        .map(|_| format!("l{}\n", rng.below(alphabet)))
        .collect();
    let mut new = String::new();
    for line in old.lines() {
        match rng.below(8) {
            0 => {}
            1 => new.push_str(&format!("l{}\n", rng.below(alphabet))),
            2 => {
                new.push_str(line);
                new.push('\n');
                new.push_str(&format!("n{}\n", rng.below(3)));
            }
            _ => {
                new.push_str(line);
                new.push('\n');
            }
        }
    }
    (old, new)
}

#[test]
fn small_diffs_match_git_minimal() {
    compare_random(
        Algorithm::Minimal,
        300 * scale(),
        |rng, case| {
            let len = 1 + rng.below(40) as usize;
            alphabet_pair(rng, len, [2, 3, 6, 20][case % 4])
        },
        0x9E3779B97F4A7C15,
    );
}

/// Large rewrites: long files where a large part changes, so that the
/// search is long enough for Git to stop looking for a minimal diff.
fn rewrite_pair(rng: &mut Rng) -> (String, String) {
    let len = 400 + rng.below(1600) as usize;
    let alphabet = [8, 40, 400][rng.below(3) as usize];
    let old: Vec<String> = (0..len)
        .map(|_| format!("line {}\n", rng.below(alphabet)))
        .collect();
    let mut new = Vec::new();
    for line in &old {
        match rng.below(10) {
            0..=2 => new.push(format!("line {}\n", rng.below(alphabet))),
            3 => {}
            4 => {
                new.push(line.clone());
                for _ in 0..rng.below(6) {
                    new.push(format!("new {}\n", rng.below(alphabet)));
                }
            }
            _ => new.push(line.clone()),
        }
    }
    (old.concat(), new.concat())
}

#[test]
#[ignore = "known difference on large rewrites (docs/diff-compat-research.md, finding 4)"]
fn large_rewrites_match_git_myers() {
    compare_random(
        Algorithm::Myers,
        20 * scale(),
        |rng, _| rewrite_pair(rng),
        0xD1B54A32D192ED03,
    );
}

/// Issue #37: histogram diffs whose regions fall back to Myers and need
/// hundreds of edits.
#[test]
#[ignore = "issue #37: known difference on large rewrites (docs/diff-compat-research.md, finding 4)"]
fn large_histogram_diffs_match_git() {
    compare_random(
        Algorithm::Histogram,
        20 * scale(),
        |rng, case| {
            let len = 1500 + rng.below(1000) as usize;
            alphabet_pair(rng, len, [2, 3, 6, 20][case % 4])
        },
        0x8BB84B93962EACC9,
    );
}
