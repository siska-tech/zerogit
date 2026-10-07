//! Line diffs between two blob contents.
//!
//! [`BlobDiff::compute`] compares the old and new contents of one file and
//! returns hunks of [`DiffLine`]s with 1-based old and new line numbers.
//! Contents that are not UTF-8 text, or that exceed the configured limits,
//! produce an explicit [`BlobDiffContent::NonText`] or
//! [`BlobDiffContent::Skipped`] result instead of a partial line diff.

use std::collections::HashMap;
use std::fmt;

/// Options for computing a [`BlobDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOptions {
    context_lines: usize,
    max_input_size: usize,
    max_cost: u64,
}

impl Default for DiffOptions {
    /// Three context lines, 8 MiB per side and a budget of 50 million steps.
    fn default() -> Self {
        Self {
            context_lines: 3,
            max_input_size: 8 << 20,
            max_cost: 50_000_000,
        }
    }
}

impl DiffOptions {
    /// Creates options with the default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the number of unchanged lines shown around each change.
    pub fn context_lines(mut self, lines: usize) -> Self {
        self.context_lines = lines;
        self
    }

    /// Sets the largest content size, in bytes, compared on either side.
    pub fn max_input_size(mut self, bytes: usize) -> Self {
        self.max_input_size = bytes;
        self
    }

    /// Sets the computation budget, in Myers diagonal and snake steps.
    ///
    /// The cost grows with file length times the number of changed lines.
    /// When it is exceeded the diff is skipped; no approximate result is returned.
    pub fn max_cost(mut self, steps: u64) -> Self {
        self.max_cost = steps;
        self
    }

    /// Returns the number of context lines.
    pub fn get_context_lines(&self) -> usize {
        self.context_lines
    }

    /// Returns the maximum content size per side, in bytes.
    pub fn get_max_input_size(&self) -> usize {
        self.max_input_size
    }

    /// Returns the computation budget.
    pub fn get_max_cost(&self) -> u64 {
        self.max_cost
    }
}

/// How a line relates the old and new contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Present unchanged in both contents.
    Context,
    /// Present only in the new content.
    Added,
    /// Present only in the old content.
    Removed,
}

/// The terminator at the end of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `\n`.
    Lf,
    /// `\r\n`.
    CrLf,
    /// The last line of a content without a trailing newline.
    None,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    kind: LineKind,
    old_lineno: Option<usize>,
    new_lineno: Option<usize>,
    content: String,
}

impl DiffLine {
    /// Returns whether the line is context, added or removed.
    pub fn kind(&self) -> LineKind {
        self.kind
    }

    /// Returns the 1-based line number in the old content; `None` for added lines.
    pub fn old_lineno(&self) -> Option<usize> {
        self.old_lineno
    }

    /// Returns the 1-based line number in the new content; `None` for removed lines.
    pub fn new_lineno(&self) -> Option<usize> {
        self.new_lineno
    }

    /// Returns the line exactly as stored, including its terminator.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns the line without its `\n` or `\r\n` terminator.
    pub fn text(&self) -> &str {
        let len = self.content.len() - self.ending_len();
        &self.content[..len]
    }

    /// Returns the line's terminator.
    pub fn ending(&self) -> LineEnding {
        if self.content.ends_with("\r\n") {
            LineEnding::CrLf
        } else if self.content.ends_with('\n') {
            LineEnding::Lf
        } else {
            LineEnding::None
        }
    }

    fn ending_len(&self) -> usize {
        match self.ending() {
            LineEnding::CrLf => 2,
            LineEnding::Lf => 1,
            LineEnding::None => 0,
        }
    }
}

/// A group of changed lines with surrounding context.
///
/// Starts are 1-based. As in unified diffs, when a side has no lines in the
/// hunk its start is the line after which the change applies, so an insertion
/// at the top of a file has `old_start == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<DiffLine>,
}

impl DiffHunk {
    /// Returns the first old line number of the hunk (see the type docs for empty ranges).
    pub fn old_start(&self) -> usize {
        self.old_start
    }

    /// Returns the number of old lines (context and removed) in the hunk.
    pub fn old_lines(&self) -> usize {
        self.old_lines
    }

    /// Returns the first new line number of the hunk (see the type docs for empty ranges).
    pub fn new_start(&self) -> usize {
        self.new_start
    }

    /// Returns the number of new lines (context and added) in the hunk.
    pub fn new_lines(&self) -> usize {
        self.new_lines
    }

    /// Returns the hunk's lines in order; removed lines precede added ones within a change.
    pub fn lines(&self) -> &[DiffLine] {
        &self.lines
    }

    /// Returns the unified diff header, such as `@@ -1,3 +1,4 @@`.
    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        )
    }
}

/// Why contents were not compared as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonTextReason {
    /// A side contains a NUL byte, Git's indicator of binary content.
    ContainsNul,
    /// A side is not valid UTF-8. It is never lossily converted.
    InvalidUtf8,
}

/// Why a text diff was not computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// A side is larger than [`DiffOptions::max_input_size`].
    InputTooLarge {
        /// The configured limit in bytes.
        limit: usize,
    },
    /// The diff needs more than [`DiffOptions::max_cost`] steps.
    TooComplex {
        /// The configured budget.
        limit: u64,
    },
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::InputTooLarge { limit } => write!(f, "content larger than {} bytes", limit),
            SkipReason::TooComplex { limit } => write!(f, "diff needs more than {} steps", limit),
        }
    }
}

/// The comparison result of a [`BlobDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobDiffContent {
    /// A complete line diff; empty when the contents are identical.
    Text(Vec<DiffHunk>),
    /// The contents were not compared line by line.
    NonText(NonTextReason),
    /// The line diff was skipped because of a limit.
    Skipped(SkipReason),
}

/// The difference between the old and new contents of one file.
///
/// A missing side (an added or deleted file) is compared as empty content
/// but stays distinguishable from an existing empty blob through
/// [`BlobDiff::old_exists`] and [`BlobDiff::new_exists`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobDiff {
    old_exists: bool,
    new_exists: bool,
    old_size: usize,
    new_size: usize,
    identical: bool,
    content: BlobDiffContent,
}

impl BlobDiff {
    /// Compares two contents; `None` means the file does not exist on that side.
    ///
    /// Checks are applied in order: size limit, NUL bytes, UTF-8 validity,
    /// then the line diff and its cost budget. The result is deterministic
    /// for the same inputs and options.
    pub fn compute(old: Option<&[u8]>, new: Option<&[u8]>, options: &DiffOptions) -> Self {
        let (old_bytes, new_bytes) = (old.unwrap_or_default(), new.unwrap_or_default());
        BlobDiff {
            old_exists: old.is_some(),
            new_exists: new.is_some(),
            old_size: old_bytes.len(),
            new_size: new_bytes.len(),
            identical: old.is_some() == new.is_some() && old_bytes == new_bytes,
            content: compare(old_bytes, new_bytes, options),
        }
    }

    /// The result for blobs of which one is larger than `limit` bytes, from
    /// their IDs and sizes alone, without reading them: equal IDs are
    /// identical content.
    pub(crate) fn too_large(
        old: Option<(&crate::objects::Oid, u64)>,
        new: Option<(&crate::objects::Oid, u64)>,
        limit: usize,
    ) -> Self {
        let size = |side: Option<(_, u64)>| {
            side.map_or(0, |(_, size)| usize::try_from(size).unwrap_or(usize::MAX))
        };
        BlobDiff {
            old_exists: old.is_some(),
            new_exists: new.is_some(),
            old_size: size(old),
            new_size: size(new),
            identical: old.map(|(oid, _)| oid) == new.map(|(oid, _)| oid),
            content: BlobDiffContent::Skipped(SkipReason::InputTooLarge { limit }),
        }
    }

    /// Returns whether the file exists in the old content.
    pub fn old_exists(&self) -> bool {
        self.old_exists
    }

    /// Returns whether the file exists in the new content.
    pub fn new_exists(&self) -> bool {
        self.new_exists
    }

    /// Returns the old content size in bytes (0 when missing).
    pub fn old_size(&self) -> usize {
        self.old_size
    }

    /// Returns the new content size in bytes (0 when missing).
    pub fn new_size(&self) -> usize {
        self.new_size
    }

    /// Returns whether both sides exist with identical bytes, or both are missing.
    ///
    /// This is exact for every result kind, including non-text and skipped ones.
    pub fn is_identical(&self) -> bool {
        self.identical
    }

    /// Returns the comparison result.
    pub fn content(&self) -> &BlobDiffContent {
        &self.content
    }

    /// Returns the hunks of a complete text diff, or `None` otherwise.
    pub fn hunks(&self) -> Option<&[DiffHunk]> {
        match &self.content {
            BlobDiffContent::Text(hunks) => Some(hunks),
            _ => None,
        }
    }

    /// Returns the number of added lines of a text diff.
    pub fn lines_added(&self) -> Option<usize> {
        self.count(LineKind::Added)
    }

    /// Returns the number of removed lines of a text diff.
    pub fn lines_removed(&self) -> Option<usize> {
        self.count(LineKind::Removed)
    }

    fn count(&self, kind: LineKind) -> Option<usize> {
        self.hunks().map(|hunks| {
            hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == kind)
                .count()
        })
    }
}

fn compare(old: &[u8], new: &[u8], options: &DiffOptions) -> BlobDiffContent {
    if old.len() > options.max_input_size || new.len() > options.max_input_size {
        return BlobDiffContent::Skipped(SkipReason::InputTooLarge {
            limit: options.max_input_size,
        });
    }
    if old.contains(&0) || new.contains(&0) {
        return BlobDiffContent::NonText(NonTextReason::ContainsNul);
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return BlobDiffContent::NonText(NonTextReason::InvalidUtf8);
    };
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    match line_ops(&old_lines, &new_lines, options.max_cost) {
        Some(ops) => {
            BlobDiffContent::Text(hunks(&ops, &old_lines, &new_lines, options.context_lines))
        }
        None => BlobDiffContent::Skipped(SkipReason::TooComplex {
            limit: options.max_cost,
        }),
    }
}

/// Splits after each `\n`, keeping terminators; a final unterminated line is kept.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Computes edit operations, or `None` if the cost budget runs out.
fn line_ops(old: &[&str], new: &[&str], max_cost: u64) -> Option<Vec<Op>> {
    let old: Vec<&[u8]> = old.iter().map(|line| line.as_bytes()).collect();
    let new: Vec<&[u8]> = new.iter().map(|line| line.as_bytes()).collect();
    byte_line_ops(&old, &new, max_cost)
}

fn byte_line_ops<'a>(old: &[&'a [u8]], new: &[&'a [u8]], max_cost: u64) -> Option<Vec<Op>> {
    // Intern lines so comparisons are integer comparisons.
    let mut ids: HashMap<&'a [u8], u32> = HashMap::new();
    let mut intern = |line: &'a [u8]| {
        let next = ids.len() as u32;
        *ids.entry(line).or_insert(next)
    };
    let a: Vec<u32> = old.iter().map(|line| intern(line)).collect();
    let b: Vec<u32> = new.iter().map(|line| intern(line)).collect();

    // A line that never occurs on the other side cannot be matched. Dropping
    // such lines keeps the longest common subsequence, so the edit stays
    // minimal, while rewrites of whole documents become cheap.
    let mut in_a = vec![false; ids.len()];
    let mut in_b = vec![false; ids.len()];
    a.iter().for_each(|&id| in_a[id as usize] = true);
    b.iter().for_each(|&id| in_b[id as usize] = true);
    let a_kept: Vec<usize> = (0..a.len()).filter(|&i| in_b[a[i] as usize]).collect();
    let b_kept: Vec<usize> = (0..b.len()).filter(|&j| in_a[b[j] as usize]).collect();
    let fa: Vec<u32> = a_kept.iter().map(|&i| a[i]).collect();
    let fb: Vec<u32> = b_kept.iter().map(|&j| b[j]).collect();

    let mut myers = Myers {
        a: &fa,
        b: &fb,
        vf: V::new(fa.len() + fb.len()),
        vb: V::new(fa.len() + fb.len()),
        budget: max_cost,
        ops: Vec::with_capacity(fa.len().max(fb.len())),
    };
    myers.conquer(0, fa.len(), 0, fb.len())?;
    let matches = myers.ops.iter().filter_map(|op| match *op {
        Op::Equal(i, j) => Some((a_kept[i], b_kept[j])),
        _ => None,
    });
    Some(ops_from_matches(matches, a.len(), b.len()))
}

/// Expands matched line pairs into operations, with deletions before
/// insertions between consecutive matches.
fn ops_from_matches(matches: impl Iterator<Item = (usize, usize)>, n: usize, m: usize) -> Vec<Op> {
    let mut ops = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    for (x, y) in matches {
        ops.extend((i..x).map(Op::Delete));
        ops.extend((j..y).map(Op::Insert));
        ops.push(Op::Equal(x, y));
        i = x + 1;
        j = y + 1;
    }
    ops.extend((i..n).map(Op::Delete));
    ops.extend((j..m).map(Op::Insert));
    ops
}

/// A diagonal-indexed vector for Myers' algorithm.
struct V {
    offset: isize,
    values: Vec<usize>,
}

impl V {
    fn new(max_d: usize) -> Self {
        let size = max_d + 2;
        Self {
            offset: size as isize,
            values: vec![0; 2 * size + 1],
        }
    }

    fn get(&self, k: isize) -> usize {
        self.values[(k + self.offset) as usize]
    }

    fn set(&mut self, k: isize, value: usize) {
        self.values[(k + self.offset) as usize] = value;
    }
}

/// Linear-space Myers diff (divide and conquer on the middle snake).
struct Myers<'a> {
    a: &'a [u32],
    b: &'a [u32],
    vf: V,
    vb: V,
    budget: u64,
    ops: Vec<Op>,
}

impl Myers<'_> {
    fn spend(&mut self, steps: usize) -> Option<()> {
        self.budget = self.budget.checked_sub(steps as u64 + 1)?;
        Some(())
    }

    fn conquer(
        &mut self,
        mut a_start: usize,
        mut a_end: usize,
        mut b_start: usize,
        mut b_end: usize,
    ) -> Option<()> {
        while a_start < a_end && b_start < b_end && self.a[a_start] == self.b[b_start] {
            self.ops.push(Op::Equal(a_start, b_start));
            a_start += 1;
            b_start += 1;
        }
        let mut suffix = 0;
        while a_start < a_end && b_start < b_end && self.a[a_end - 1] == self.b[b_end - 1] {
            a_end -= 1;
            b_end -= 1;
            suffix += 1;
        }
        if a_start == a_end {
            self.ops.extend((b_start..b_end).map(Op::Insert));
        } else if b_start == b_end {
            self.ops.extend((a_start..a_end).map(Op::Delete));
        } else {
            let (x, y) = self.middle_snake(a_start, a_end, b_start, b_end)?;
            self.conquer(a_start, x, b_start, y)?;
            self.conquer(x, a_end, y, b_end)?;
        }
        self.ops
            .extend((0..suffix).map(|i| Op::Equal(a_end + i, b_end + i)));
        Some(())
    }

    /// Returns a point on an optimal path that splits the problem in two.
    fn middle_snake(
        &mut self,
        a_start: usize,
        a_end: usize,
        b_start: usize,
        b_end: usize,
    ) -> Option<(usize, usize)> {
        let n = a_end - a_start;
        let m = b_end - b_start;
        let delta = n as isize - m as isize;
        let odd = delta & 1 == 1;
        self.vf.set(1, 0);
        self.vb.set(1, 0);
        let d_max = ((n + m + 1) / 2 + 1) as isize;
        for d in 0..d_max {
            for k in (-d..=d).rev().step_by(2) {
                let mut x = if k == -d || (k != d && self.vf.get(k - 1) < self.vf.get(k + 1)) {
                    self.vf.get(k + 1)
                } else {
                    self.vf.get(k - 1) + 1
                };
                let y = (x as isize - k) as usize;
                let (x0, y0) = (x, y);
                let mut advance = 0;
                while x + advance < n
                    && y + advance < m
                    && self.a[a_start + x + advance] == self.b[b_start + y + advance]
                {
                    advance += 1;
                }
                self.spend(advance)?;
                x += advance;
                self.vf.set(k, x);
                if odd && (k - delta).abs() < d && self.vf.get(k) + self.vb.get(delta - k) >= n {
                    return Some((a_start + x0, b_start + y0));
                }
            }
            for k in (-d..=d).rev().step_by(2) {
                let mut x = if k == -d || (k != d && self.vb.get(k - 1) < self.vb.get(k + 1)) {
                    self.vb.get(k + 1)
                } else {
                    self.vb.get(k - 1) + 1
                };
                let mut y = (x as isize - k) as usize;
                let mut advance = 0;
                while x + advance < n
                    && y + advance < m
                    && self.a[a_end - 1 - x - advance] == self.b[b_end - 1 - y - advance]
                {
                    advance += 1;
                }
                self.spend(advance)?;
                x += advance;
                y += advance;
                self.vb.set(k, x);
                if !odd && (k - delta).abs() <= d && self.vb.get(k) + self.vf.get(delta - k) >= n {
                    return Some((a_end - x, b_end - y));
                }
            }
        }
        // Unreachable for valid inputs: the paths always meet within d_max.
        None
    }
}

/// Groups operations into hunks with `context` lines around changes.
fn hunks(ops: &[Op], old: &[&str], new: &[&str], context: usize) -> Vec<DiffHunk> {
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Equal(..)))
        .map(|(i, _)| i)
        .collect();
    // Changes separated by at most 2 * context unchanged lines share a hunk.
    let mut groups = Vec::new();
    let mut j = 0;
    while j < changes.len() {
        let first = changes[j];
        let mut last = first;
        while j + 1 < changes.len() && changes[j + 1] - last <= 2 * context + 1 {
            j += 1;
            last = changes[j];
        }
        groups.push((first, last));
        j += 1;
    }

    // Old and new lines consumed before each operation index.
    let mut old_before = Vec::with_capacity(ops.len() + 1);
    let mut new_before = Vec::with_capacity(ops.len() + 1);
    let (mut o, mut n) = (0, 0);
    for op in ops {
        old_before.push(o);
        new_before.push(n);
        match op {
            Op::Equal(..) => {
                o += 1;
                n += 1;
            }
            Op::Delete(_) => o += 1,
            Op::Insert(_) => n += 1,
        }
    }
    old_before.push(o);
    new_before.push(n);

    groups
        .into_iter()
        .map(|(first, last)| {
            let begin = first.saturating_sub(context);
            let end = (last + context + 1).min(ops.len());
            let lines: Vec<DiffLine> = ops[begin..end]
                .iter()
                .map(|op| match *op {
                    Op::Equal(a, b) => DiffLine {
                        kind: LineKind::Context,
                        old_lineno: Some(a + 1),
                        new_lineno: Some(b + 1),
                        content: old[a].to_owned(),
                    },
                    Op::Delete(a) => DiffLine {
                        kind: LineKind::Removed,
                        old_lineno: Some(a + 1),
                        new_lineno: None,
                        content: old[a].to_owned(),
                    },
                    Op::Insert(b) => DiffLine {
                        kind: LineKind::Added,
                        old_lineno: None,
                        new_lineno: Some(b + 1),
                        content: new[b].to_owned(),
                    },
                })
                .collect();
            let old_lines = old_before[end] - old_before[begin];
            let new_lines = new_before[end] - new_before[begin];
            let start = |before: usize, count: usize| if count == 0 { before } else { before + 1 };
            DiffHunk {
                old_start: start(old_before[begin], old_lines),
                old_lines,
                new_start: start(new_before[begin], new_lines),
                new_lines,
                lines,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies hunks to `old`, checking every context and removed line.
    fn apply(old: &str, hunks: &[DiffHunk]) -> String {
        let old_lines = split_lines(old);
        let mut out = String::new();
        let mut next = 0;
        for hunk in hunks {
            let at = if hunk.old_lines == 0 {
                hunk.old_start
            } else {
                hunk.old_start - 1
            };
            assert!(at >= next, "overlapping hunks");
            out.extend(old_lines[next..at].iter().copied());
            next = at;
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Context | LineKind::Removed => {
                        assert_eq!(line.old_lineno, Some(next + 1));
                        assert_eq!(line.content, old_lines[next]);
                        if line.kind == LineKind::Context {
                            out.push_str(&line.content);
                        }
                        next += 1;
                    }
                    LineKind::Added => out.push_str(&line.content),
                }
            }
        }
        out.extend(old_lines[next..].iter().copied());
        out
    }

    /// Checks reconstruction, new line numbers and hunk counts.
    fn check(old: &str, new: &str, context: usize) -> BlobDiff {
        let options = DiffOptions::new().context_lines(context);
        let result = BlobDiff::compute(Some(old.as_bytes()), Some(new.as_bytes()), &options);
        let hunks = result.hunks().expect("text diff");
        assert_eq!(apply(old, hunks), new, "{:?} -> {:?}", old, new);
        for hunk in hunks {
            let mut new_line = if hunk.new_lines == 0 {
                hunk.new_start
            } else {
                hunk.new_start - 1
            };
            for line in &hunk.lines {
                if line.kind != LineKind::Removed {
                    new_line += 1;
                    assert_eq!(line.new_lineno, Some(new_line));
                }
            }
            let count = |kinds: &[LineKind]| {
                hunk.lines
                    .iter()
                    .filter(|l| kinds.contains(&l.kind))
                    .count()
            };
            assert_eq!(
                hunk.old_lines,
                count(&[LineKind::Context, LineKind::Removed])
            );
            assert_eq!(hunk.new_lines, count(&[LineKind::Context, LineKind::Added]));
        }
        assert_eq!(hunks.is_empty(), old == new);
        assert_eq!(result.is_identical(), old == new);
        result
    }

    fn lcs(a: &[&str], b: &[&str]) -> usize {
        let mut row = vec![0; b.len() + 1];
        for x in a {
            let mut diagonal = 0;
            for (j, y) in b.iter().enumerate() {
                let above = row[j + 1];
                row[j + 1] = if x == y {
                    diagonal + 1
                } else {
                    above.max(row[j])
                };
                diagonal = above;
            }
        }
        row[b.len()]
    }

    #[test]
    fn random_inputs_reconstruct_minimally_and_deterministically() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut random = |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        for _ in 0..500 {
            let mut make = || {
                (0..random(25))
                    .map(|_| format!("{}\n", ["a", "b", "c", "d", ""][random(5) as usize]))
                    .collect::<String>()
            };
            let (old, new) = (make(), make());
            for context in [0, 1, 3] {
                let result = check(&old, &new, context);
                let (a, b) = (split_lines(&old), split_lines(&new));
                let common = lcs(&a, &b);
                assert_eq!(result.lines_removed(), Some(a.len() - common));
                assert_eq!(result.lines_added(), Some(b.len() - common));
                assert_eq!(result, check(&old, &new, context));
            }
        }
    }

    #[test]
    fn line_numbers_and_hunk_starts() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n10\n";
        let result = check(old, new, 3);
        let hunks = result.hunks().unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].header(), "@@ -2,7 +2,7 @@");
        let changed: Vec<_> = hunks[0]
            .lines()
            .iter()
            .filter(|l| l.kind() != LineKind::Context)
            .map(|l| (l.kind(), l.old_lineno(), l.new_lineno(), l.text()))
            .collect();
        assert_eq!(
            changed,
            vec![
                (LineKind::Removed, Some(5), None, "5"),
                (LineKind::Added, None, Some(5), "five"),
            ]
        );

        // Insertion at the top: the old range is empty and starts at 0.
        let top = check("a\nb\n", "new\na\nb\n", 0);
        assert_eq!(top.hunks().unwrap()[0].header(), "@@ -0,0 +1,1 @@");
        // Insertion after line 2 with no context.
        let middle = check("a\nb\nc\n", "a\nb\nnew\nc\n", 0);
        assert_eq!(middle.hunks().unwrap()[0].header(), "@@ -2,0 +3,1 @@");
        // Deletion at the end: the new range is empty.
        let end = check("a\nb\nc\n", "a\nb\n", 1);
        assert_eq!(end.hunks().unwrap()[0].header(), "@@ -2,2 +2,1 @@");
        let gone = check("a\nb\n", "", 3);
        assert_eq!(gone.hunks().unwrap()[0].header(), "@@ -1,2 +0,0 @@");
    }

    #[test]
    fn distant_changes_split_hunks_and_near_ones_merge() {
        let old: String = (1..=20).map(|i| format!("{}\n", i)).collect();
        let split = old.replace("3\n", "x\n").replace("17\n", "y\n");
        assert_eq!(check(&old, &split, 3).hunks().unwrap().len(), 2);
        let near = old.replace("3\n", "x\n").replace("10\n", "y\n");
        assert_eq!(check(&old, &near, 3).hunks().unwrap().len(), 1);
        assert_eq!(check(&old, &near, 2).hunks().unwrap().len(), 2);
    }

    #[test]
    fn repeated_lines_full_replacement_and_newline_only_changes() {
        check("x\nx\nx\n", "x\nx\n", 3);
        check("x\ny\nx\ny\n", "y\nx\ny\nx\n", 3);
        let replaced = check("a\nb\nc\n", "d\ne\n", 3);
        assert_eq!(replaced.lines_removed(), Some(3));
        assert_eq!(replaced.lines_added(), Some(2));
        let kinds: Vec<_> = replaced.hunks().unwrap()[0]
            .lines()
            .iter()
            .map(|l| l.kind())
            .collect();
        assert_eq!(
            kinds,
            [
                LineKind::Removed,
                LineKind::Removed,
                LineKind::Removed,
                LineKind::Added,
                LineKind::Added
            ]
        );

        // Only the terminator changes: CRLF to LF, and a missing final newline.
        let crlf = check("a\r\nb\r\n", "a\r\nb\n", 3);
        let lines = crlf.hunks().unwrap()[0].lines();
        assert_eq!(lines[1].ending(), LineEnding::CrLf);
        assert_eq!(lines[2].ending(), LineEnding::Lf);
        assert_eq!((lines[1].text(), lines[2].text()), ("b", "b"));
        let eof = check("a\nb", "a\nb\n", 3);
        let lines = eof.hunks().unwrap()[0].lines();
        assert_eq!(lines[1].ending(), LineEnding::None);
        assert_eq!(lines[1].content(), "b");
        assert_eq!(lines[2].content(), "b\n");
    }

    #[test]
    fn japanese_text_is_compared_by_line() {
        let result = check(
            "日本語の文書\n二行目\n",
            "日本語の文書\n二行目を編集\n三行目\n",
            3,
        );
        let added: Vec<_> = result.hunks().unwrap()[0]
            .lines()
            .iter()
            .filter(|l| l.kind() == LineKind::Added)
            .map(DiffLine::text)
            .collect();
        assert_eq!(added, ["二行目を編集", "三行目"]);
    }

    #[test]
    fn missing_sides_differ_from_empty_blobs() {
        let options = DiffOptions::new();
        let added = BlobDiff::compute(None, Some(b"a\n"), &options);
        assert!(!added.old_exists() && added.new_exists());
        assert_eq!(added.hunks().unwrap()[0].header(), "@@ -0,0 +1,1 @@");

        let created_empty = BlobDiff::compute(None, Some(b""), &options);
        assert!(!created_empty.is_identical());
        assert_eq!(created_empty.hunks(), Some(&[][..]));
        let both_empty = BlobDiff::compute(Some(b""), Some(b""), &options);
        assert!(both_empty.is_identical());
        assert!(both_empty.old_exists() && both_empty.new_exists());
        let deleted = BlobDiff::compute(Some(b"a\n"), None, &options);
        assert!(deleted.old_exists() && !deleted.new_exists());
        assert_eq!(deleted.lines_removed(), Some(1));
    }

    #[test]
    fn non_text_content_is_reported_not_converted() {
        let options = DiffOptions::new();
        let nul = BlobDiff::compute(Some(b"a\0b"), Some(b"a\0c"), &options);
        assert_eq!(
            nul.content(),
            &BlobDiffContent::NonText(NonTextReason::ContainsNul)
        );
        assert!(!nul.is_identical());
        assert_eq!(nul.lines_added(), None);
        let invalid = BlobDiff::compute(Some(b"ok\n"), Some(b"\xff\xfe\n"), &options);
        assert_eq!(
            invalid.content(),
            &BlobDiffContent::NonText(NonTextReason::InvalidUtf8)
        );
        let same = BlobDiff::compute(Some(b"\xff"), Some(b"\xff"), &options);
        assert!(same.is_identical());
        assert_eq!(same.old_size(), 1);
    }

    #[test]
    fn limits_skip_instead_of_returning_partial_results() {
        let old: String = (0..200).map(|i| format!("old {}\n", i)).collect();
        let new: String = (0..200).map(|i| format!("new {}\n", i)).collect();
        let small = DiffOptions::new().max_input_size(100);
        let large = BlobDiff::compute(Some(old.as_bytes()), Some(new.as_bytes()), &small);
        assert_eq!(
            large.content(),
            &BlobDiffContent::Skipped(SkipReason::InputTooLarge { limit: 100 })
        );
        assert!(!large.is_identical());

        // Lines unique to one side are cheap; repeated lines need real work.
        let rewrite = check(&old, &new, 3);
        assert_eq!(rewrite.lines_removed(), Some(200));
        // Same vocabulary on both sides, in a different order.
        let repeated_old: String = (0..400).map(|i| format!("{}\n", i % 5)).collect();
        let repeated_new: String = (0..400).map(|i| format!("{}\n", i * 2 % 5)).collect();
        let cheap = DiffOptions::new().max_cost(1_000);
        let complex = BlobDiff::compute(
            Some(repeated_old.as_bytes()),
            Some(repeated_new.as_bytes()),
            &cheap,
        );
        assert_eq!(
            complex.content(),
            &BlobDiffContent::Skipped(SkipReason::TooComplex { limit: 1_000 })
        );
        assert_eq!(complex.hunks(), None);
        check(&repeated_old, &repeated_new, 3);

        // Identical inputs remain identical even when skipped.
        let same = BlobDiff::compute(Some(old.as_bytes()), Some(old.as_bytes()), &small);
        assert!(same.is_identical());
    }
}
