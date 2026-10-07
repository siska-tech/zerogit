# Line diff compatibility data

Pairs of real files from this repository's history (`<name>.old`,
`<name>.new`) with the hunk headers `git diff --no-index -U0` printed for
them, recorded with Git 2.55.0:

- `<name>.myers.expected`: `--diff-algorithm=myers` (the default, which
  `git blame` uses)
- `<name>.minimal.expected`: `--minimal`
- `<name>.histogram.expected`: `--histogram`

They are compared with zerogit's line diffs by `src/diff/compat_tests.rs`,
which also diffs generated inputs with the installed Git. Only Git's
output is used; the expected files are not derived from Git's source.

| Pair | What it exercises |
|---|---|
| `slider-blank` | an inserted group that can sit in several places around blank lines |
| `worktree-minimized` | a tie between equally short diffs, decided by a long common tail |
| `worktree-dd57318` | the full file the previous case was reduced from |
| `readme-1a6093f` | a rewritten section full of repeated blank lines |
| `repository-6ac8a62` | a large rewrite (3,785 lines to 1,024) |
