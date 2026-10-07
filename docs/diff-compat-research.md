# Line diff compatibility: black-box findings

zerogit's line diffs aim to produce the same hunks as `git diff -U0` and
`git blame`. Git's diff code is not used or consulted: its behaviour is
inferred from its output on crafted inputs, and the implementation is
based on these findings and on published descriptions of the algorithms
(Myers, "An O(ND) Difference Algorithm and Its Variations", 1986). Each
finding below lists the experiments it rests on, so that it can be
checked again against any Git version. The compatibility tests
(`src/diff/compat_tests.rs`, `tests/blame_test.rs`) check the result.

Experiments were run with Git 2.55.0. A "probe" is a small input whose
two possible outputs reveal one bit of Git's internal state.

## 1. A long common tail is ignored by zero-context diffs

### Probe

Old `A B C` and new `B A B A B D`, followed by a common tail `T`. Both
diffs below are equally short:

- `-0,0 +1` then `-3 +4,3` (A and B match the new lines 2 and 3)
- `-0,0 +1,3` then `-3 +6` (A and B match the new lines 4 and 5)

Git gives the first when `C` also occurs in `T`, and the second when it
does not (`C` then cannot match anything and is set aside, which changes
the search). Placing a copy of `C` at different places in `T` therefore
shows which part of `T` Git takes into account.

### Results (`git diff --no-index -U0`)

| Common tail | Copy of `C` | Seen |
|---|---|---|
| 1,022 / 1,023 bytes (common suffix 1,023 / 1,024) | at the very end | yes / no |
| 5,000 bytes | its line ends 4,094 or more bytes before the end | yes |
| 5,000 bytes | its line ends 4,093 or fewer bytes before the end | no |
| 2,046 bytes after `C` (common suffix 2,048) | right after the changed lines | no |
| 2,045 / 2,047 bytes after `C` (common suffix 2,047 / 2,049) | right after the changed lines | yes |

The same holds for `--minimal`. With `--histogram` the result does not
depend on the copy of `C`.

### Conclusion

With no context lines, for the Myers diffs, Git leaves out the end of the
two inputs: with `S` the length of their longest common suffix in bytes,
the last `1024 * floor(S / 1024)` bytes, except for the part of those
bytes up to and including their first newline, which is kept so that the
cut falls at a line boundary. Every row above follows from this rule:
with `S = 5001`, 4,096 bytes are left out and the first of them is the
newline ending the line before the cut, so a line ending 4,093 bytes
before the end is left out but one ending 4,094 bytes before is kept.

Lines left out are unchanged (they are the same in both inputs); only
the hunks before them can differ.

## 2. Lines repeated often are set aside when they sit among unmatched lines

### Probe

Old: two lines both inputs share, then `a` lines found only in old, a
line `X`, `y` lines `Y1..Yy`, `b` more lines found only in old, and two
more shared lines. New: the shared lines around `f` copies of `X` (each
followed by the `Y` lines) between lines found only in new. If Git
matches the old `X` with a copy, no hunk covers it; if it sets `X` aside,
a single hunk does. The observable is whether `X`'s old line is inside a
hunk of `git diff -U0`.

### Results

How many copies of `X` the new file needs before Git ever sets `X` aside
(`a = b = 5`, `y = 0`), against the length of the old file (padded with
lines found only in old, away from `X`):

| Old lines | 12–15 | 16–62 | 72–255 | 256–912 | 1,112 | 1,100,011 |
|---|---|---|---|---|---|---|
| Copies needed | 4 | 8 | 16 | 32 | 64 | 1,024 |

The new file's length does not matter (padding it changes nothing).

With enough copies, whether `X` is set aside (old file of 12+ lines,
sampled with `a, b` in 1–12):

| `y` | Set aside exactly when `a + b` is at least |
|---|---|
| 0 | 7 (never when `a` or `b` is 0) |
| 1 | 10 |
| 2 | 13 |
| 3 | 16 |

A block of 60 frequent lines starting `d` lines after `X` (3 unmatched
lines before `X`, `d - 1` after it): `X` stays matched up to `d = 76` and
is set aside from `d = 77` on; the same holds before `X`.

`--minimal` never sets a frequent line aside (with 64 copies and an old
file of 1,011 lines, `X` stays matched).

### Conclusion

A line of the old file that occurs in the new file at least `t` times,
where `t` is the smallest power of two whose square exceeds the number of
lines of the old file (at most 1,024), is frequent; symmetrically for
the new file. A frequent line is set aside (treated as changed before the
search) when, among the lines next to it that are unmatched or frequent
(up to the first other line on each side, looking at most 100 lines each
way), there is at least one unmatched line on each side and
`3 * (frequent + 2) < unmatched`, counting the frequent lines among those
neighbours. Lines found only in one input are always set aside (this
includes `--minimal`, as the probe of finding 1 shows: there the line
missing from the new file is set aside under `--minimal` as well).

The flip at `d = 77` matches a window of 100 lines: 3 + 76 unmatched
lines against `3 * (2 + 24)` once 24 frequent lines are within reach.

## 3. Where a group of changed lines that could sit in several places goes

### Probe

A slice of a short file of indented and blank lines is repeated right
after itself, so that the inserted group can sit in several places
("slide"); `git diff -U0` shows where Git puts it. `src/diff/slider_research.rs`
generates such cases, records every position the group could take with
what surrounds its two boundaries (the line at the boundary, the blank
lines before and after it, the indentation of the nearest non-blank lines
before and after), and fits a rule to Git's choices.

### Results

- In 3,000 cases, the group is never put more than its own size plus one
  line above its lowest position, although it could often go higher.
- A rule that scores each boundary with additive penalties cannot
  explain all choices (at best 95%); one where positions are compared in
  turn, from the highest down, each replacing the best so far unless it
  is worse, and where indentation only counts by which of two positions
  has the more indented boundaries, explains all 3,000. The same rule,
  unchanged, explains 2,998 of 3,000 new cases (another random seed), and
  after refitting all 6,000 of both sets together.

### Conclusion

For each position, its two boundaries (just before the group's first
line, and just after its last) are described by: the indentation of the
first non-blank line at or after the boundary (or after the next blank
lines), the indentation of the nearest non-blank line before, the blank
lines before and after, and whether the boundary is at the start or the
end of the file. Two positions compare by `150 * sign(difference of the
summed indentation)` plus the difference of their summed penalties:

| Penalty, per boundary | Weight |
|---|---|
| no line before it (start of the file) | 6 |
| at the end of the file | -6 |
| blank lines around it | -75 each |
| of those, blank lines from it onwards | 15 each |
| indented further than the line before, no blank lines | -9 |
| indented further, with blank lines | 25 |
| indented less, no blank lines | 56 |
| indented less, with blank lines | 41 |
| indented less, and the line after is indented further | 3 more |

These weights are one point of the region that fits all observations
(fitted with a perceptron, then a local search over integer weights);
other points fit as well, so the compatibility tests are the final check.
