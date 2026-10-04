//! Three-way merge of file contents, line by line.
//!
//! Both sides are diffed against the base with the histogram diff, as
//! `git merge` does. Changes made on only one side, or identically on both,
//! are taken; changes that overlap or touch in the base conflict. With the
//! default `merge` conflict style, conflicts are then refined the way Git's
//! "zealous" merge level does: the two sides of a conflict are diffed, lines
//! on which they agree are moved out of the conflict, and conflicts separated
//! by at most three unchanged lines are joined. The `diff3` style shows the
//! base and does no refinement, as in Git.

use std::ops::Range;

use crate::diff::histogram::{diff_lines, split_lines, Hunk};

/// How conflicts are written (`merge.conflictStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictStyle {
    /// `<<<<<<<`, `=======`, `>>>>>>>`.
    Merge,
    /// Also shows the base after `|||||||`.
    Diff3,
}

/// The names written after the conflict markers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Labels<'a> {
    pub(crate) ours: &'a str,
    pub(crate) base: &'a str,
    pub(crate) theirs: &'a str,
}

/// The result of merging one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMerge {
    /// The merged content, with conflict markers if there are conflicts.
    pub(crate) content: Vec<u8>,
    /// The number of conflict regions.
    pub(crate) conflicts: usize,
}

/// Returns whether Git treats content as binary for merging (a NUL byte in
/// the first 8000 bytes).
pub(crate) fn is_binary(content: &[u8]) -> bool {
    content[..content.len().min(8000)].contains(&0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Overlapping changes.
    Conflict,
    /// Changed on our side only.
    Ours,
    /// Changed on their side only.
    Theirs,
    /// A conflict whose sides turned out identical; ours is taken.
    Same,
}

/// A changed region, with its line ranges in the base and on each side.
#[derive(Debug, Clone)]
struct Change {
    kind: Kind,
    base: Range<usize>,
    ours: Range<usize>,
    theirs: Range<usize>,
}

/// Adds a change, joining it with the previous one when they overlap or
/// touch on either side (a join of different kinds is a conflict).
fn append(changes: &mut Vec<Change>, change: Change) {
    if let Some(last) = changes.last_mut() {
        if change.ours.start <= last.ours.end || change.theirs.start <= last.theirs.end {
            if last.kind != change.kind {
                last.kind = Kind::Conflict;
            }
            last.base.end = change.base.end;
            last.ours.end = change.ours.end;
            last.theirs.end = change.theirs.end;
            return;
        }
    }
    changes.push(change);
}

/// A change on one side, with the matching unchanged range on the other
/// side (`offset` converts base line numbers to the other side's).
fn one_sided(kind: Kind, hunk: &Hunk, offset: isize) -> Change {
    let other_start = (hunk.old_start as isize + offset) as usize;
    let other = other_start..other_start + hunk.old_len;
    let own = hunk.new_start..hunk.new_end();
    let (ours, theirs) = if kind == Kind::Ours {
        (own, other)
    } else {
        (other, own)
    };
    Change {
        kind,
        base: hunk.old_start..hunk.old_end(),
        ours,
        theirs,
    }
}

fn offset(hunk: &Hunk) -> isize {
    hunk.new_start as isize - hunk.old_start as isize
}

/// The lines of the base, ours and theirs.
type SideLines<'a> = (&'a [&'a [u8]], &'a [&'a [u8]], &'a [&'a [u8]]);

/// Combines the base-to-ours and base-to-theirs diffs into changes.
fn combine(ours_diff: &[Hunk], theirs_diff: &[Hunk], lines: SideLines<'_>) -> Vec<Change> {
    let (base, ours, theirs) = lines;
    let mut changes = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < ours_diff.len() && j < theirs_diff.len() {
        let x = &ours_diff[i];
        let y = &theirs_diff[j];
        if x.old_end() < y.old_start {
            append(&mut changes, one_sided(Kind::Ours, x, offset(y)));
            i += 1;
            continue;
        }
        if y.old_end() < x.old_start {
            append(&mut changes, one_sided(Kind::Theirs, y, offset(x)));
            j += 1;
            continue;
        }
        let identical = x.old_start == y.old_start
            && x.old_len == y.old_len
            && ours[x.new_start..x.new_end()] == theirs[y.new_start..y.new_end()];
        if !identical {
            // Extend each side over the base lines only the other side changed.
            let before = x.old_start as isize - y.old_start as isize;
            let after = x.old_end() as isize - y.old_end() as isize;
            let ours_start = x.new_start - before.max(0) as usize;
            let theirs_start = y.new_start - (-before).max(0) as usize;
            let ours_end = x.new_end() + (-after).max(0) as usize;
            let theirs_end = y.new_end() + after.max(0) as usize;
            append(
                &mut changes,
                Change {
                    kind: Kind::Conflict,
                    base: x.old_start.min(y.old_start)..x.old_end().max(y.old_end()),
                    ours: ours_start..ours_end,
                    theirs: theirs_start..theirs_end,
                },
            );
        }
        let (x_end, y_end) = (x.old_end(), y.old_end());
        if x_end >= y_end {
            j += 1;
        }
        if y_end >= x_end {
            i += 1;
        }
    }
    let theirs_offset = theirs.len() as isize - base.len() as isize;
    for x in &ours_diff[i..] {
        append(&mut changes, one_sided(Kind::Ours, x, theirs_offset));
    }
    let ours_offset = ours.len() as isize - base.len() as isize;
    for y in &theirs_diff[j..] {
        append(&mut changes, one_sided(Kind::Theirs, y, ours_offset));
    }
    changes
}

/// Splits each conflict at the lines both sides agree on; a conflict whose
/// sides are identical is resolved.
fn refine(changes: Vec<Change>, ours: &[&[u8]], theirs: &[&[u8]]) -> Vec<Change> {
    let mut out = Vec::with_capacity(changes.len());
    for change in changes {
        if change.kind != Kind::Conflict || change.ours.is_empty() || change.theirs.is_empty() {
            out.push(change);
            continue;
        }
        let hunks = diff_lines(&ours[change.ours.clone()], &theirs[change.theirs.clone()]);
        if hunks.is_empty() {
            out.push(Change {
                kind: Kind::Same,
                ..change
            });
            continue;
        }
        for hunk in hunks {
            let ours_start = change.ours.start + hunk.old_start;
            let theirs_start = change.theirs.start + hunk.new_start;
            out.push(Change {
                kind: Kind::Conflict,
                base: change.base.clone(),
                ours: ours_start..ours_start + hunk.old_len,
                theirs: theirs_start..theirs_start + hunk.new_len,
            });
        }
    }
    out
}

/// Joins consecutive conflicts separated by at most three unchanged lines.
fn join_close_conflicts(changes: Vec<Change>) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::with_capacity(changes.len());
    for change in changes {
        if let Some(last) = out.last_mut() {
            if last.kind == Kind::Conflict
                && change.kind == Kind::Conflict
                && change.ours.start - last.ours.end <= 3
            {
                last.base.end = change.base.end;
                last.ours.end = change.ours.end;
                last.theirs.end = change.theirs.end;
                continue;
            }
        }
        out.push(change);
    }
    out
}

/// Merges `ours` and `theirs`, both derived from `base`.
pub(crate) fn merge_lines(
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    labels: Labels<'_>,
    style: ConflictStyle,
) -> FileMerge {
    let base_lines = split_lines(base);
    let our_lines = split_lines(ours);
    let their_lines = split_lines(theirs);
    let ours_diff = diff_lines(&base_lines, &our_lines);
    let theirs_diff = diff_lines(&base_lines, &their_lines);
    let mut changes = combine(
        &ours_diff,
        &theirs_diff,
        (&base_lines, &our_lines, &their_lines),
    );
    if style == ConflictStyle::Merge {
        changes = join_close_conflicts(refine(changes, &our_lines, &their_lines));
    }

    let mut content = Vec::with_capacity(ours.len().max(theirs.len()));
    let mut conflicts = 0;
    let mut pos = 0;
    let copy =
        |out: &mut Vec<u8>, lines: &[&[u8]]| lines.iter().for_each(|l| out.extend_from_slice(l));
    for change in &changes {
        copy(&mut content, &our_lines[pos..change.ours.start]);
        match change.kind {
            Kind::Ours | Kind::Same => copy(&mut content, &our_lines[change.ours.clone()]),
            Kind::Theirs => copy(&mut content, &their_lines[change.theirs.clone()]),
            Kind::Conflict => {
                conflicts += 1;
                let ours_part = &our_lines[change.ours.clone()];
                let theirs_part = &their_lines[change.theirs.clone()];
                let eol: &[u8] =
                    if uses_crlf(ours_part) && (theirs_part.is_empty() || uses_crlf(theirs_part)) {
                        b"\r\n"
                    } else {
                        b"\n"
                    };
                marker(&mut content, b'<', labels.ours, eol);
                side(&mut content, ours_part, eol);
                if style == ConflictStyle::Diff3 {
                    marker(&mut content, b'|', labels.base, eol);
                    side(&mut content, &base_lines[change.base.clone()], eol);
                }
                marker(&mut content, b'=', "", eol);
                side(&mut content, theirs_part, eol);
                marker(&mut content, b'>', labels.theirs, eol);
            }
        }
        pos = change.ours.end;
    }
    copy(&mut content, &our_lines[pos..]);
    FileMerge { content, conflicts }
}

fn uses_crlf(lines: &[&[u8]]) -> bool {
    lines.first().is_some_and(|l| l.ends_with(b"\r\n"))
}

fn marker(out: &mut Vec<u8>, c: u8, label: &str, eol: &[u8]) {
    out.extend(std::iter::repeat(c).take(7));
    if !label.is_empty() {
        out.push(b' ');
        out.extend_from_slice(label.as_bytes());
    }
    out.extend_from_slice(eol);
}

/// Writes one side of a conflict; a last line without a newline gets one so
/// the next marker starts on its own line.
fn side(out: &mut Vec<u8>, lines: &[&[u8]], eol: &[u8]) {
    for line in lines {
        out.extend_from_slice(line);
    }
    if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
        out.extend_from_slice(eol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS: Labels<'static> = Labels {
        ours: "ours",
        base: "base",
        theirs: "theirs",
    };

    fn merge(base: &str, ours: &str, theirs: &str) -> (String, usize) {
        let result = merge_lines(
            base.as_bytes(),
            ours.as_bytes(),
            theirs.as_bytes(),
            LABELS,
            ConflictStyle::Merge,
        );
        (String::from_utf8(result.content).unwrap(), result.conflicts)
    }

    #[test]
    fn test_clean_merges() {
        assert_eq!(
            merge("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n"),
            ("A\nb\nC\n".into(), 0)
        );
        assert_eq!(
            merge("a\nb\n", "a\nb\n", "a\nx\nb\n"),
            ("a\nx\nb\n".into(), 0)
        );
        assert_eq!(merge("a\nb\n", "a\nB\n", "a\nB\n"), ("a\nB\n".into(), 0));
        assert_eq!(merge("", "", "new\n"), ("new\n".into(), 0));
    }

    #[test]
    fn test_conflict_markers() {
        let (text, conflicts) = merge("a\nb\nc\n", "a\nB1\nc\n", "a\nB2\nc\n");
        assert_eq!(conflicts, 1);
        assert_eq!(
            text,
            "a\n<<<<<<< ours\nB1\n=======\nB2\n>>>>>>> theirs\nc\n"
        );
    }

    #[test]
    fn test_missing_final_newline_in_conflict() {
        let (text, _) = merge("a\n", "b", "c");
        assert_eq!(text, "<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n");
    }
}
