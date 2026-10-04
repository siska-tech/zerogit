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
//! side, in which case they are placed there. As in Git 2.54 and later, a
//! group that moved or joined others is diffed again when the other side
//! has changes in the same place.
//!
//! The result matches `git diff --histogram`, except where a region falls
//! back to the Myers diff and needs hundreds of edits: Git then cuts its
//! search short with heuristics this crate does not use.

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
    diff_lines_with(old, new, true)
}

/// Diffs with or without the re-diff of joined groups that Git does since
/// 2.54 (see [`compact`]).
fn diff_lines_with<'a>(old: &[&'a [u8]], new: &[&'a [u8]], rediff: bool) -> Vec<Hunk> {
    let mut ids: HashMap<&'a [u8], u32> = HashMap::new();
    let mut intern = |line: &'a [u8]| -> u32 {
        let next = ids.len() as u32;
        *ids.entry(line).or_insert(next)
    };
    let a: Vec<u32> = old.iter().map(|l| intern(l)).collect();
    let b: Vec<u32> = new.iter().map(|l| intern(l)).collect();
    diff_ids(&a, &b, rediff)
}

fn diff_ids(a: &[u32], b: &[u32], rediff: bool) -> Vec<Hunk> {
    let mut a_changed = vec![false; a.len()];
    let mut b_changed = vec![false; b.len()];
    histogram(a, b, &mut a_changed, &mut b_changed);
    compact(a, &mut a_changed, b, &mut b_changed, rediff);
    compact(b, &mut b_changed, a, &mut a_changed, rediff);
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
fn histogram(a: &[u32], b: &[u32], a_changed: &mut [bool], b_changed: &mut [bool]) {
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
            Anchor::TooFrequent => myers_region(region, a, b, a_changed, b_changed),
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

/// Diffs a region with the Myers algorithm, as Git's histogram diff does
/// for regions it does not handle itself.
fn myers_region(
    region: Region,
    a: &[u32],
    b: &[u32],
    a_changed: &mut [bool],
    b_changed: &mut [bool],
) {
    let (a_region, b_region) = classic_diff(
        &a[region.a_start..region.a_end],
        &b[region.b_start..region.b_end],
    );
    a_changed[region.a_start..region.a_end].copy_from_slice(&a_region);
    b_changed[region.b_start..region.b_end].copy_from_slice(&b_region);
}

/// Git's classic (Myers) diff of two sequences, returning the changed
/// lines of each.
///
/// The common first and last lines are set aside, lines that do not occur
/// on the other side are marked changed up front (as are lines occurring
/// there often enough that keeping them is pointless amid changed lines),
/// and the rest is split recursively at the middle of an optimal path
/// (Myers, "An O(ND) Difference Algorithm and its Variations"), the forward
/// search preferring deletions and both searches running from the highest
/// diagonal down. Git cuts the search short with heuristics once a region
/// costs hundreds of edits; this function always finds a minimal path.
fn classic_diff(a: &[u32], b: &[u32]) -> (Vec<bool>, Vec<bool>) {
    let mut a_changed = vec![false; a.len()];
    let mut b_changed = vec![false; b.len()];

    let limit = a.len().min(b.len());
    let mut prefix = 0;
    while prefix < limit && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < limit - prefix && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }

    let counts = |lines: &[u32]| {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for &line in lines {
            *counts.entry(line).or_insert(0) += 1;
        }
        counts
    };
    let (a_counts, b_counts) = (counts(a), counts(b));
    let a_kept = kept_lines(
        &a[prefix..a.len() - suffix],
        &b_counts,
        a.len(),
        &mut a_changed[prefix..a.len() - suffix],
    );
    let b_kept = kept_lines(
        &b[prefix..b.len() - suffix],
        &a_counts,
        b.len(),
        &mut b_changed[prefix..b.len() - suffix],
    );

    let ka: Vec<u32> = a_kept.iter().map(|&i| a[prefix + i]).collect();
    let kb: Vec<u32> = b_kept.iter().map(|&j| b[prefix + j]).collect();
    let size = ka.len() + kb.len() + 3;
    let mut myers = Myers {
        a: &ka,
        b: &kb,
        forward: vec![0; size],
        backward: vec![0; size],
        offset: kb.len() as isize + 1,
        a_changed: vec![false; ka.len()],
        b_changed: vec![false; kb.len()],
    };
    myers.compare(0, ka.len(), 0, kb.len());
    for (k, &i) in a_kept.iter().enumerate() {
        a_changed[prefix + i] = myers.a_changed[k];
    }
    for (k, &j) in b_kept.iter().enumerate() {
        b_changed[prefix + j] = myers.b_changed[k];
    }
    (a_changed, b_changed)
}

/// Picks the lines of one side worth diffing: a line missing from the
/// other side is changed; a line found there at least about √`total` times
/// is changed too when it sits among changed lines rather than kept ones.
/// Marks the dropped lines in `changed` and returns the indexes of the rest.
fn kept_lines(
    lines: &[u32],
    other_counts: &HashMap<u32, usize>,
    total: usize,
    changed: &mut [bool],
) -> Vec<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum Class {
        Missing,
        Kept,
        Frequent,
    }
    let mut frequent_limit = 1usize;
    let mut n = total;
    while n > 0 {
        frequent_limit <<= 1;
        n >>= 2;
    }
    let frequent_limit = frequent_limit.min(1024);
    let class: Vec<Class> = lines
        .iter()
        .map(|line| match other_counts.get(line).copied().unwrap_or(0) {
            0 => Class::Missing,
            n if n < frequent_limit => Class::Kept,
            _ => Class::Frequent,
        })
        .collect();

    // A frequent line is dropped when the nearby lines on both sides that
    // are not kept include missing ones, and are mostly missing ones.
    const WINDOW: usize = 100;
    let among_missing = |i: usize| {
        let mut missing_before = 0;
        let mut frequent = 1;
        for &c in class[i.saturating_sub(WINDOW)..i].iter().rev() {
            match c {
                Class::Missing => missing_before += 1,
                Class::Frequent => frequent += 1,
                Class::Kept => break,
            }
        }
        if missing_before == 0 {
            return false;
        }
        let mut missing_after = 0;
        for &c in &class[i + 1..class.len().min(i + WINDOW + 1)] {
            match c {
                Class::Missing => missing_after += 1,
                Class::Frequent => frequent += 1,
                Class::Kept => break,
            }
        }
        if missing_after == 0 {
            return false;
        }
        let missing = missing_before + missing_after;
        frequent * 4 < frequent + missing
    };

    let mut kept = Vec::new();
    for (i, &c) in class.iter().enumerate() {
        let keep = match c {
            Class::Missing => false,
            Class::Kept => true,
            Class::Frequent => !among_missing(i),
        };
        if keep {
            kept.push(i);
        } else {
            changed[i] = true;
        }
    }
    kept
}

/// Linear-space Myers diff over diagonals `k = i - j`.
struct Myers<'a> {
    a: &'a [u32],
    b: &'a [u32],
    /// The furthest `i` reached on each diagonal from the start.
    forward: Vec<isize>,
    /// The lowest `i` reached on each diagonal from the end.
    backward: Vec<isize>,
    offset: isize,
    a_changed: Vec<bool>,
    b_changed: Vec<bool>,
}

impl Myers<'_> {
    fn compare(
        &mut self,
        mut a_start: usize,
        mut a_end: usize,
        mut b_start: usize,
        mut b_end: usize,
    ) {
        while a_start < a_end && b_start < b_end && self.a[a_start] == self.b[b_start] {
            a_start += 1;
            b_start += 1;
        }
        while a_start < a_end && b_start < b_end && self.a[a_end - 1] == self.b[b_end - 1] {
            a_end -= 1;
            b_end -= 1;
        }
        if a_start == a_end {
            self.b_changed[b_start..b_end].fill(true);
        } else if b_start == b_end {
            self.a_changed[a_start..a_end].fill(true);
        } else {
            let (i, j) = self.split(a_start, a_end, b_start, b_end);
            self.compare(a_start, i, b_start, j);
            self.compare(i, a_end, j, b_end);
        }
    }

    fn f(&mut self, k: isize) -> &mut isize {
        &mut self.forward[(k + self.offset) as usize]
    }

    fn r(&mut self, k: isize) -> &mut isize {
        &mut self.backward[(k + self.offset) as usize]
    }

    /// Finds where the forward and backward searches meet: the end of the
    /// forward snake or the start of the backward one that overlaps.
    fn split(
        &mut self,
        a_start: usize,
        a_end: usize,
        b_start: usize,
        b_end: usize,
    ) -> (usize, usize) {
        let (off1, lim1) = (a_start as isize, a_end as isize);
        let (off2, lim2) = (b_start as isize, b_end as isize);
        let (k_min, k_max) = (off1 - lim2, lim1 - off2);
        let (f_mid, b_mid) = (off1 - off2, lim1 - lim2);
        let odd = (f_mid - b_mid) & 1 != 0;
        let (mut f_lo, mut f_hi) = (f_mid, f_mid);
        let (mut b_lo, mut b_hi) = (b_mid, b_mid);
        *self.f(f_mid) = off1;
        *self.r(b_mid) = lim1;
        loop {
            // Widen the range of diagonals by one each way, inside the box;
            // the diagonals just outside it hold values that never win.
            if f_lo > k_min {
                f_lo -= 1;
                *self.f(f_lo - 1) = -1;
            } else {
                f_lo += 1;
            }
            if f_hi < k_max {
                f_hi += 1;
                *self.f(f_hi + 1) = -1;
            } else {
                f_hi -= 1;
            }
            let mut k = f_hi;
            while k >= f_lo {
                let mut i = if *self.f(k - 1) >= *self.f(k + 1) {
                    *self.f(k - 1) + 1
                } else {
                    *self.f(k + 1)
                };
                let mut j = i - k;
                while i < lim1 && j < lim2 && self.a[i as usize] == self.b[j as usize] {
                    i += 1;
                    j += 1;
                }
                *self.f(k) = i;
                if odd && b_lo <= k && k <= b_hi && *self.r(k) <= i {
                    return (i as usize, j as usize);
                }
                k -= 2;
            }

            if b_lo > k_min {
                b_lo -= 1;
                *self.r(b_lo - 1) = isize::MAX;
            } else {
                b_lo += 1;
            }
            if b_hi < k_max {
                b_hi += 1;
                *self.r(b_hi + 1) = isize::MAX;
            } else {
                b_hi -= 1;
            }
            let mut k = b_hi;
            while k >= b_lo {
                let mut i = if *self.r(k - 1) < *self.r(k + 1) {
                    *self.r(k - 1)
                } else {
                    *self.r(k + 1) - 1
                };
                let mut j = i - k;
                while i > off1 && j > off2 && self.a[i as usize - 1] == self.b[j as usize - 1] {
                    i -= 1;
                    j -= 1;
                }
                *self.r(k) = i;
                if !odd && f_lo <= k && k <= f_hi && i <= *self.f(k) {
                    return (i as usize, j as usize);
                }
                k -= 2;
            }
        }
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
///
/// With `rediff` (Git 2.54 and later), a group that moved or joined others
/// and faces a non-empty group on the other side is diffed again with the
/// Myers algorithm, since the joined lines may now have matches there.
fn compact(
    lines: &[u32],
    changed: &mut [bool],
    other_lines: &[u32],
    other_changed: &mut [bool],
    rediff: bool,
) {
    let mut other_gaps = gaps(other_changed);
    let other_nonempty =
        |gaps: &[(usize, usize)], gap: usize| matches!(gaps.get(gap), Some(&(s, e)) if s < e);

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
        let original = (start, end);

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
            if other_nonempty(&other_gaps, gap) {
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
                if other_nonempty(&other_gaps, gap) {
                    aligned_end = Some(end);
                }
            }
            if size == end - start {
                break;
            }
        }
        if end != earliest_end && aligned_end.is_some() {
            while !other_nonempty(&other_gaps, gap) {
                changed[start - 1] = true;
                changed[end - 1] = false;
                start -= 1;
                end -= 1;
                gap -= 1;
            }
        }
        if rediff && other_nonempty(&other_gaps, gap) && (start, end) != original {
            let (o_start, o_end) = other_gaps[gap];
            myers_region(
                Region {
                    a_start: start,
                    a_end: end,
                    b_start: o_start,
                    b_end: o_end,
                },
                lines,
                other_lines,
                changed,
                other_changed,
            );
            // Lines matched inside the group open new gaps on both sides;
            // the groups between them are left where the diff put them.
            gap += changed[start..end].iter().filter(|&&c| !c).count();
            other_gaps = gaps(other_changed);
        }
        pos = end;
    }
}

/// The (possibly empty) group of changed lines in each gap between
/// unchanged lines, as ranges.
fn gaps(changed: &[bool]) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut start = 0;
    for (i, &c) in changed.iter().enumerate() {
        if !c {
            result.push((start, i));
            start = i + 1;
        }
    }
    result.push((start, changed.len()));
    result
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

    #[test]
    fn test_joined_group_is_diffed_again() {
        // Sliding joins the changes after old line 14 into one group; Git
        // 2.54 and later diff it again and find the l0 lines unchanged.
        let old = lines(
            "l0\nl0\nl1\nl1\nl1\nl1\nl1\nl0\nl1\nl0\nl1\n\
             l1\nl1\nl1\nl1\nl0\nl0\nl0\nl1\nl0\nl1\nl0\n",
        );
        let new = lines(
            "l0\nl0\nl1\nl1\nl1\nn2\nl1\nn0\nl0\nl1\nl0\nl1\nl1\n\
             n0\nl1\nl1\nn2\nl0\nl0\nn0\nl0\nl0\nn0\nl1\nl0\n",
        );
        let hunk = |old_start, old_len, new_start, new_len| Hunk {
            old_start,
            old_len,
            new_start,
            new_len,
        };
        let joined = vec![hunk(5, 0, 5, 1), hunk(6, 1, 7, 1), hunk(12, 0, 13, 1)];
        let mut before = joined.clone();
        before.push(hunk(14, 6, 16, 7));
        assert_eq!(diff_lines_with(&old, &new, false), before);
        let mut after = joined;
        after.extend([
            hunk(14, 1, 16, 1),
            hunk(17, 0, 19, 1),
            hunk(18, 1, 21, 0),
            hunk(20, 0, 22, 1),
        ]);
        assert_eq!(diff_lines(&old, &new), after);
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

    /// Whether the installed Git re-diffs joined groups (2.54 and later).
    fn git_rediffs() -> bool {
        let output = Command::new("git").arg("--version").output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let mut version = text
            .trim()
            .trim_start_matches("git version ")
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        let major = version.next().unwrap_or(0);
        let minor = version.next().unwrap_or(0);
        (major, minor) >= (2, 54)
    }

    #[test]
    fn random_diffs_match_git_histogram() {
        let rediff = git_rediffs();
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
            let ours = diff_lines_with(&lines(&old), &lines(&new), rediff);
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
