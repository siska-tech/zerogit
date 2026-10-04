//! Histogram line diff with Git's hunk placement, used for merges.
//!
//! `git merge` diffs contents with the histogram algorithm. This module
//! implements that algorithm as published with JGit's `HistogramDiff`: in a
//! region, the common run of lines anchored on the line that occurs least
//! often in the old side is taken as unchanged, and the regions before and
//! after it are diffed the same way. A region whose common lines all occur
//! more than [`MAX_CHAIN`] times falls back to the Myers diff.
//!
//! Change groups are then slid the way Git places them: as far down as the
//! repeated lines allow, unless they can line up with a change on the other
//! side, in which case they are placed there.
//!
//! The result matches `git diff --histogram`, except in regions where every
//! common line occurs more than [`MAX_CHAIN`] times: there Git falls back to
//! its own Myers implementation, whose choices among equally short diffs can
//! differ from this crate's.

use std::collections::HashMap;

/// Lines occurring more often than this in a region are not used as
/// anchors.
const MAX_CHAIN: usize = 64;

/// A changed region: `old_len` lines at `old_start` replaced by `new_len`
/// lines at `new_start` (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub(crate) old_start: usize,
    pub(crate) old_len: usize,
    pub(crate) new_start: usize,
    pub(crate) new_len: usize,
}

impl Hunk {
    pub(crate) fn old_end(&self) -> usize {
        self.old_start + self.old_len
    }

    pub(crate) fn new_end(&self) -> usize {
        self.new_start + self.new_len
    }
}

/// Diffs two sequences of lines and returns the changed regions in order.
pub(crate) fn diff_lines<'a>(old: &[&'a [u8]], new: &[&'a [u8]]) -> Vec<Hunk> {
    let mut ids: HashMap<&'a [u8], u32> = HashMap::new();
    let mut intern = |line: &'a [u8]| -> u32 {
        let next = ids.len() as u32;
        *ids.entry(line).or_insert(next)
    };
    let a: Vec<u32> = old.iter().map(|l| intern(l)).collect();
    let b: Vec<u32> = new.iter().map(|l| intern(l)).collect();
    diff_ids(&a, &b, old, new)
}

fn diff_ids(a: &[u32], b: &[u32], old: &[&[u8]], new: &[&[u8]]) -> Vec<Hunk> {
    let mut a_changed = vec![false; a.len()];
    let mut b_changed = vec![false; b.len()];
    histogram(a, b, &mut a_changed, &mut b_changed, old, new);
    compact(a, &mut a_changed, &b_changed);
    compact(b, &mut b_changed, &a_changed);
    hunks(&a_changed, &b_changed)
}

#[derive(Debug, Clone, Copy)]
struct Region {
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
}

/// Marks the lines changed by the histogram diff of `a` and `b`.
fn histogram(
    a: &[u32],
    b: &[u32],
    a_changed: &mut [bool],
    b_changed: &mut [bool],
    old: &[&[u8]],
    new: &[&[u8]],
) {
    let mut pending = vec![Region {
        a_start: 0,
        a_end: a.len(),
        b_start: 0,
        b_end: b.len(),
    }];
    while let Some(region) = pending.pop() {
        if region.a_start == region.a_end || region.b_start == region.b_end {
            a_changed[region.a_start..region.a_end].fill(true);
            b_changed[region.b_start..region.b_end].fill(true);
            continue;
        }
        match find_lcs(a, b, region) {
            Anchor::NoCommon => {
                a_changed[region.a_start..region.a_end].fill(true);
                b_changed[region.b_start..region.b_end].fill(true);
            }
            Anchor::TooFrequent => myers_region(region, a_changed, b_changed, old, new),
            Anchor::Found(lcs) => {
                pending.push(Region {
                    a_start: lcs.a_end,
                    a_end: region.a_end,
                    b_start: lcs.b_end,
                    b_end: region.b_end,
                });
                pending.push(Region {
                    a_start: region.a_start,
                    a_end: lcs.a_start,
                    b_start: region.b_start,
                    b_end: lcs.b_start,
                });
            }
        }
    }
}

/// Diffs a region with the Myers algorithm.
fn myers_region(
    region: Region,
    a_changed: &mut [bool],
    b_changed: &mut [bool],
    old: &[&[u8]],
    new: &[&[u8]],
) {
    let old = &old[region.a_start..region.a_end];
    let new = &new[region.b_start..region.b_end];
    let mut a_kept = vec![false; old.len()];
    let mut b_kept = vec![false; new.len()];
    for (i, j) in super::blob::matching_lines(old, new) {
        a_kept[i] = true;
        b_kept[j] = true;
    }
    for (i, kept) in a_kept.into_iter().enumerate() {
        a_changed[region.a_start + i] = !kept;
    }
    for (j, kept) in b_kept.into_iter().enumerate() {
        b_changed[region.b_start + j] = !kept;
    }
}

enum Anchor {
    NoCommon,
    TooFrequent,
    Found(Region),
}

/// Finds the longest common run anchored on the least frequent line of `a`
/// within the region.
fn find_lcs(a: &[u32], b: &[u32], region: Region) -> Anchor {
    // Occurrences of each line of `a` in the region: the first position,
    // the count, and for each position the next one with the same line.
    let mut first: HashMap<u32, (usize, usize)> = HashMap::new();
    let mut next = vec![None; region.a_end - region.a_start];
    for pos in (region.a_start..region.a_end).rev() {
        let entry = first.entry(a[pos]).or_insert((pos, 0));
        if entry.1 > 0 {
            next[pos - region.a_start] = Some(entry.0);
        }
        entry.0 = pos;
        entry.1 += 1;
    }
    let count = |id: u32| first.get(&id).map_or(0, |e| e.1);

    let mut has_common = false;
    let mut best: Option<Region> = None;
    let mut best_count = MAX_CHAIN + 1;
    let mut b_pos = region.b_start;
    while b_pos < region.b_end {
        let mut b_next = b_pos + 1;
        if let Some(&(first_pos, occurrences)) = first.get(&b[b_pos]) {
            has_common = true;
            if occurrences <= best_count {
                let mut candidate = Some(first_pos);
                while let Some(start) = candidate {
                    let (mut a_s, mut b_s) = (start, b_pos);
                    let (mut a_e, mut b_e) = (start + 1, b_pos + 1);
                    let mut run_count = occurrences;
                    while a_s > region.a_start && b_s > region.b_start && a[a_s - 1] == b[b_s - 1] {
                        a_s -= 1;
                        b_s -= 1;
                        if run_count > 1 {
                            run_count = run_count.min(count(a[a_s]));
                        }
                    }
                    while a_e < region.a_end && b_e < region.b_end && a[a_e] == b[b_e] {
                        if run_count > 1 {
                            run_count = run_count.min(count(a[a_e]));
                        }
                        a_e += 1;
                        b_e += 1;
                    }
                    b_next = b_next.max(b_e);
                    let best_len = best.map_or(0, |r| r.a_end - r.a_start);
                    if best_len < a_e - a_s || run_count < best_count {
                        best = Some(Region {
                            a_start: a_s,
                            a_end: a_e,
                            b_start: b_s,
                            b_end: b_e,
                        });
                        best_count = run_count;
                    }
                    // The next occurrence that is not inside the run just
                    // examined (the line right after the run is still tried).
                    let mut following = next[start - region.a_start];
                    while let Some(pos) = following {
                        if pos >= a_e {
                            break;
                        }
                        following = next[pos - region.a_start];
                    }
                    candidate = following;
                }
            }
        }
        b_pos = b_next;
    }
    match best {
        Some(lcs) if best_count <= MAX_CHAIN => Anchor::Found(lcs),
        _ if has_common => Anchor::TooFrequent,
        _ => Anchor::NoCommon,
    }
}

/// Slides the change groups of one side (`lines`, `changed`) to Git's
/// positions, given the changes of the other side.
///
/// Unchanged lines of both sides correspond in order, so a group sits in the
/// gap after a given number of unchanged lines, and the other side has a
/// (possibly empty) group in the same gap. A group can move down when its
/// first line equals the line after it, and up when its last line equals the
/// line before it; moving into a neighbouring group joins them. Each group
/// goes as far down as possible, then back up to the lowest position where
/// the other side's group in the same gap is not empty, if there is one.
fn compact(lines: &[u32], changed: &mut [bool], other_changed: &[bool]) {
    // Whether the other side's group in each gap is non-empty.
    let mut other_gaps = Vec::new();
    let mut gap_nonempty = false;
    for &c in other_changed {
        if c {
            gap_nonempty = true;
        } else {
            other_gaps.push(gap_nonempty);
            gap_nonempty = false;
        }
    }
    other_gaps.push(gap_nonempty);
    let other_nonempty = |gap: usize| other_gaps.get(gap).copied().unwrap_or(false);

    let n = lines.len();
    let mut pos = 0;
    let mut gap = 0;
    while pos < n {
        if !changed[pos] {
            pos += 1;
            gap += 1;
            continue;
        }
        let mut start = pos;
        let mut end = pos;
        while end < n && changed[end] {
            end += 1;
        }

        let mut earliest_end;
        let mut aligned_end;
        loop {
            let size = end - start;
            aligned_end = None;
            // Up as far as possible, joining earlier groups.
            while start > 0 && lines[start - 1] == lines[end - 1] {
                changed[start - 1] = true;
                changed[end - 1] = false;
                start -= 1;
                end -= 1;
                gap -= 1;
                while start > 0 && changed[start - 1] {
                    start -= 1;
                }
            }
            earliest_end = end;
            if other_nonempty(gap) {
                aligned_end = Some(end);
            }
            // Down as far as possible, joining later groups.
            while end < n && lines[start] == lines[end] {
                changed[start] = false;
                changed[end] = true;
                start += 1;
                end += 1;
                gap += 1;
                while end < n && changed[end] {
                    end += 1;
                }
                if other_nonempty(gap) {
                    aligned_end = Some(end);
                }
            }
            if size == end - start {
                break;
            }
        }
        if end != earliest_end && aligned_end.is_some() {
            while !other_nonempty(gap) {
                changed[start - 1] = true;
                changed[end - 1] = false;
                start -= 1;
                end -= 1;
                gap -= 1;
            }
        }
        pos = end;
    }
}

/// Collects the changed regions of both sides into hunks.
fn hunks(a_changed: &[bool], b_changed: &[bool]) -> Vec<Hunk> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a_changed.len() || j < b_changed.len() {
        let a_run = a_changed[i..].iter().take_while(|&&c| c).count();
        let b_run = b_changed[j..].iter().take_while(|&&c| c).count();
        if a_run > 0 || b_run > 0 {
            result.push(Hunk {
                old_start: i,
                old_len: a_run,
                new_start: j,
                new_len: b_run,
            });
            i += a_run;
            j += b_run;
        } else {
            i += 1;
            j += 1;
        }
    }
    result
}

/// Splits content into lines, each keeping its `\n`.
pub(crate) fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            lines.push(&content[start..=i]);
            start = i + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn lines(text: &str) -> Vec<&[u8]> {
        split_lines(text.as_bytes())
    }

    #[test]
    fn test_simple_hunks() {
        let old = lines("a\nb\nc\n");
        let new = lines("a\nB\nc\nd\n");
        assert_eq!(
            diff_lines(&old, &new),
            vec![
                Hunk {
                    old_start: 1,
                    old_len: 1,
                    new_start: 1,
                    new_len: 1
                },
                Hunk {
                    old_start: 3,
                    old_len: 0,
                    new_start: 3,
                    new_len: 1
                },
            ]
        );
        assert!(diff_lines(&old, &old).is_empty());
        assert_eq!(diff_lines(&[], &new).len(), 1);
    }

    #[test]
    fn test_insertion_slides_down_through_repeats() {
        // Inserting one of three identical lines is shown at the end.
        let old = lines("x\na\na\ny\n");
        let new = lines("x\na\na\na\ny\n");
        assert_eq!(
            diff_lines(&old, &new),
            vec![Hunk {
                old_start: 3,
                old_len: 0,
                new_start: 3,
                new_len: 1
            }]
        );
    }

    /// `-U0` hunk headers from `git diff --histogram`, as hunks.
    fn git_hunks(dir: &std::path::Path) -> Vec<Hunk> {
        let output = Command::new("git")
            .current_dir(dir)
            .args(["diff", "--no-index", "--histogram", "-U0", "old", "new"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let range = |s: &str| -> (usize, usize) {
            let (start, len) = match s.split_once(',') {
                Some((start, len)) => (start.parse().unwrap(), len.parse().unwrap()),
                None => (s.parse().unwrap(), 1),
            };
            // Git numbers lines from 1 and gives the line before an empty range.
            let start: usize = start;
            (if len == 0 { start } else { start - 1 }, len)
        };
        text.lines()
            .filter_map(|l| l.strip_prefix("@@ -"))
            .map(|l| {
                let mut parts = l.split(' ');
                let (old_start, old_len) = range(parts.next().unwrap());
                let (new_start, new_len) = range(parts.next().unwrap().trim_start_matches('+'));
                Hunk {
                    old_start,
                    old_len,
                    new_start,
                    new_len,
                }
            })
            .collect()
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn random_diffs_match_git_histogram() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();
        let mut rng = Rng(0x9E3779B97F4A7C15);
        let mut mismatches = Vec::new();
        let cases: usize = std::env::var("HCASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let max_len: u64 = std::env::var("HLEN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(25);
        for case in 0..cases {
            let alphabet = [2, 3, 6, 20][case % 4];
            let len = 1 + rng.below(max_len) as usize;
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
            std::fs::write(dir.join("old"), &old).unwrap();
            std::fs::write(dir.join("new"), &new).unwrap();
            let ours = diff_lines(&lines(&old), &lines(&new));
            let theirs = git_hunks(dir);
            if ours != theirs {
                mismatches.push(format!(
                    "case {}:\n--- old\n{}--- new\n{}git: {:?}\nours: {:?}",
                    case, old, new, theirs, ours
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} mismatches, first:\n{}",
            mismatches.len(),
            mismatches[0]
        );
    }
}
