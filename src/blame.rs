//! Which commit last changed each line of a file (`git blame`).
//!
//! [`Repository::blame`] starts from the file at a commit and hands each
//! line back through history: a line that a parent has unchanged (by the
//! diff Git uses for blame: Myers with the indent heuristic) becomes that
//! parent's, and a line no parent has is blamed on the commit. Commits are
//! visited newest first, parents in order; a parent with the identical file
//! takes every line, and a file missing from a parent is followed through a
//! whole-file rename, as `git blame` does by default.

use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};

use crate::diff::histogram::{diff_bytes, split_lines, Algorithm};
use crate::diff::rename::{pair_similar, RenameOptions};
use crate::error::{Error, Result};
use crate::objects::{FileMode, ObjectType, Oid};
use crate::repository::Repository;

/// Options for [`Repository::blame`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameOptions {
    revision: Option<String>,
    first_parent: bool,
    lines: Option<(usize, usize)>,
    follow_renames: bool,
    algorithm: DiffAlgorithm,
}

impl Default for BlameOptions {
    fn default() -> Self {
        BlameOptions {
            revision: None,
            first_parent: false,
            lines: None,
            follow_renames: true,
            algorithm: DiffAlgorithm::Myers,
        }
    }
}

impl BlameOptions {
    /// The default options: the file as of HEAD, all its lines, following
    /// renames through every parent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Blames the file as of `revision` (`git blame <revision> -- <path>`)
    /// instead of HEAD.
    pub fn revision<S: Into<String>>(mut self, revision: S) -> Self {
        self.revision = Some(revision.into());
        self
    }

    /// Follows only the first parent of merges (`--first-parent`): the
    /// changes a merge brings are blamed on the merge.
    pub fn first_parent(mut self, first_parent: bool) -> Self {
        self.first_parent = first_parent;
        self
    }

    /// Blames only lines `start` to `end` (1-based, inclusive; `-L start,end`).
    pub fn lines(mut self, start: usize, end: usize) -> Self {
        self.lines = Some((start, end));
        self
    }

    /// The line diff versions are compared with (`--diff-algorithm`;
    /// default [`DiffAlgorithm::Myers`], as in Git).
    pub fn diff_algorithm(mut self, algorithm: DiffAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// Whether a file missing from a parent is looked for under another
    /// name (default true).
    pub fn follow_renames(mut self, follow: bool) -> Self {
        self.follow_renames = follow;
        self
    }
}

/// The line diff [`Repository::blame`] compares versions with
/// (`git blame --diff-algorithm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DiffAlgorithm {
    /// Git's default (`myers`).
    #[default]
    Myers,
    /// A minimal diff (`minimal`), slower on large changes.
    Minimal,
    /// The histogram diff (`histogram`).
    Histogram,
}

impl DiffAlgorithm {
    fn internal(self) -> Algorithm {
        match self {
            DiffAlgorithm::Myers => Algorithm::Myers,
            DiffAlgorithm::Minimal => Algorithm::Minimal,
            DiffAlgorithm::Histogram => Algorithm::Histogram,
        }
    }
}

/// The commit a line of the file comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    commit: Oid,
    path: PathBuf,
    original_line: usize,
    final_line: usize,
    content: Vec<u8>,
}

impl BlameLine {
    /// The commit that last changed the line.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The file's path in that commit (it differs after a rename).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The line's number (from 1) in the file as of that commit.
    pub fn original_line(&self) -> usize {
        self.original_line
    }

    /// The line's number (from 1) in the blamed file.
    pub fn final_line(&self) -> usize {
        self.final_line
    }

    /// The line's text, with its line ending.
    pub fn content(&self) -> &[u8] {
        &self.content
    }
}

/// Consecutive lines from the same commit, consecutive there too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameHunk {
    commit: Oid,
    path: PathBuf,
    original_start: usize,
    final_start: usize,
    lines: usize,
}

impl BlameHunk {
    /// The commit the lines come from.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The file's path in that commit.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The number (from 1) of the first line in the file as of the commit.
    pub fn original_start(&self) -> usize {
        self.original_start
    }

    /// The number (from 1) of the first line in the blamed file.
    pub fn final_start(&self) -> usize {
        self.final_start
    }

    /// How many lines the hunk has.
    pub fn lines(&self) -> usize {
        self.lines
    }
}

/// The result of [`Repository::blame`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blame {
    commit: Oid,
    path: PathBuf,
    lines: Vec<BlameLine>,
}

impl Blame {
    /// The commit whose file was blamed.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The blamed path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The blamed lines, in order.
    pub fn lines(&self) -> &[BlameLine] {
        &self.lines
    }

    /// The lines grouped into hunks of consecutive lines from the same
    /// commit (and consecutive in it).
    pub fn hunks(&self) -> Vec<BlameHunk> {
        let mut hunks: Vec<BlameHunk> = Vec::new();
        for line in &self.lines {
            if let Some(last) = hunks.last_mut() {
                if last.commit == line.commit
                    && last.path == line.path
                    && last.final_start + last.lines == line.final_line
                    && last.original_start + last.lines == line.original_line
                {
                    last.lines += 1;
                    continue;
                }
            }
            hunks.push(BlameHunk {
                commit: line.commit,
                path: line.path.clone(),
                original_start: line.original_line,
                final_start: line.final_line,
                lines: 1,
            });
        }
        hunks
    }
}

/// A version of the file: at a path in a commit.
struct Origin {
    commit: Oid,
    path: String,
    blob: Oid,
    /// The lines of the blob, read when needed.
    content: Option<Vec<u8>>,
    /// Final lines it is suspected of, with their line in this version
    /// (both from 0).
    pending: Vec<(usize, usize)>,
    queued: bool,
}

/// A commit waiting to hand its lines on: the newest first, then in
/// insertion order.
#[derive(PartialEq, Eq)]
struct Queued {
    time: i64,
    order: std::cmp::Reverse<u64>,
    origin: usize,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.time, self.order).cmp(&(other.time, other.order))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The commit time, parents and tree of a commit.
struct CommitInfo {
    time: i64,
    parents: Vec<Oid>,
    tree: Oid,
}

struct Blamer<'a> {
    repo: &'a Repository,
    options: &'a BlameOptions,
    origins: Vec<Origin>,
    by_key: HashMap<(Oid, String), usize>,
    commits: HashMap<Oid, CommitInfo>,
    queue: BinaryHeap<Queued>,
    counter: u64,
    /// For each final line: the commit, path and line (from 0) blamed.
    result: Vec<Option<(Oid, usize, usize)>>,
}

/// Whether a tree entry is a file blame can read.
fn is_file(mode: FileMode) -> bool {
    matches!(
        mode,
        FileMode::Regular | FileMode::Executable | FileMode::Symlink
    )
}

impl<'a> Blamer<'a> {
    fn commit(&mut self, oid: &Oid) -> Result<&CommitInfo> {
        if !self.commits.contains_key(oid) {
            let commit = self.repo.commit(&oid.to_hex())?;
            let mut parents = commit.parents().to_vec();
            if self.options.first_parent {
                parents.truncate(1);
            }
            self.commits.insert(
                *oid,
                CommitInfo {
                    time: commit.committer().timestamp(),
                    parents,
                    tree: *commit.tree(),
                },
            );
        }
        Ok(&self.commits[oid])
    }

    /// The blob at `path` in a tree, if it is a file.
    fn blob_at(&self, tree: &Oid, path: &str) -> Result<Option<Oid>> {
        let mut tree = *tree;
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            let entries = self.repo.tree(&tree.to_hex())?;
            let Some(entry) = entries.get(part) else {
                return Ok(None);
            };
            if parts.peek().is_none() {
                return Ok(is_file(entry.mode()).then_some(*entry.oid()));
            }
            if entry.mode() != FileMode::Directory {
                return Ok(None);
            }
            tree = *entry.oid();
        }
        Ok(None)
    }

    /// Every file of a tree, by `/`-separated path.
    fn files(
        &self,
        tree: &Oid,
        prefix: &str,
        out: &mut HashMap<String, (Oid, FileMode)>,
    ) -> Result<()> {
        for entry in self.repo.tree(&tree.to_hex())?.entries() {
            let path = format!("{}{}", prefix, entry.name());
            if entry.mode() == FileMode::Directory {
                self.files(entry.oid(), &format!("{}/", path), out)?;
            } else if is_file(entry.mode()) {
                out.insert(path, (*entry.oid(), entry.mode()));
            }
        }
        Ok(())
    }

    /// The origin for `path` in `commit`, created if new.
    fn origin(&mut self, commit: Oid, path: &str, blob: Oid) -> usize {
        let key = (commit, path.to_owned());
        if let Some(&index) = self.by_key.get(&key) {
            return index;
        }
        self.origins.push(Origin {
            commit,
            path: path.to_owned(),
            blob,
            content: None,
            pending: Vec::new(),
            queued: false,
        });
        self.by_key.insert(key, self.origins.len() - 1);
        self.origins.len() - 1
    }

    fn content(&mut self, origin: usize) -> Result<&[u8]> {
        if self.origins[origin].content.is_none() {
            let raw = self.repo.object_store().read(&self.origins[origin].blob)?;
            self.origins[origin].content = Some(raw.content);
        }
        Ok(self.origins[origin].content.as_deref().expect("read above"))
    }

    /// Hands lines to `origin` and queues it.
    fn give(&mut self, origin: usize, lines: Vec<(usize, usize)>) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        self.origins[origin].pending.extend(lines);
        if !self.origins[origin].queued {
            self.origins[origin].queued = true;
            let commit = self.origins[origin].commit;
            let time = self.commit(&commit)?.time;
            self.counter += 1;
            self.queue.push(Queued {
                time,
                order: std::cmp::Reverse(self.counter),
                origin,
            });
        }
        Ok(())
    }

    /// The version of the file in `parent` that `origin` comes from: the
    /// same path, or (with `renamed`) a file the commit removed that it
    /// renamed to the path.
    fn find_in_parent(
        &mut self,
        origin: usize,
        parent: Oid,
        renamed: bool,
    ) -> Result<Option<usize>> {
        let parent_tree = self.commit(&parent)?.tree;
        let path = self.origins[origin].path.clone();
        if !renamed {
            return Ok(self
                .blob_at(&parent_tree, &path)?
                .map(|blob| self.origin(parent, &path, blob)));
        }
        let commit = self.origins[origin].commit;
        let tree = self.commit(&commit)?.tree;
        let (mut before, mut after) = (HashMap::new(), HashMap::new());
        self.files(&parent_tree, "", &mut before)?;
        self.files(&tree, "", &mut after)?;
        let Some(&(target_blob, target_mode)) = after.get(&path) else {
            return Ok(None);
        };
        // Files the commit removed: the candidates, in path order.
        let mut sources: Vec<(String, Oid, FileMode)> = before
            .into_iter()
            .filter(|(p, _)| !after.contains_key(p))
            .map(|(p, (oid, mode))| (p, oid, mode))
            .collect();
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        // An identical file first, one of the same name if there are several.
        let name = path.rsplit('/').next().unwrap_or(&path);
        let identical: Vec<&(String, Oid, FileMode)> = sources
            .iter()
            .filter(|(_, oid, _)| *oid == target_blob)
            .collect();
        let exact = identical
            .iter()
            .find(|(p, _, _)| p.rsplit('/').next() == Some(name))
            .or_else(|| identical.first());
        if let Some((source, blob, _)) = exact {
            let (source, blob) = (source.clone(), *blob);
            return Ok(Some(self.origin(parent, &source, blob)));
        }
        let store = self.repo.object_store();
        let pairs = pair_similar(
            &sources,
            &[(path.clone(), target_blob, target_mode)],
            &RenameOptions::new(),
            &mut |oid| Ok(store.read(oid)?.content),
        )?;
        for (source, _) in pairs {
            if let Some((_, blob, _)) = sources.iter().find(|(p, _, _)| *p == source) {
                let blob = *blob;
                return Ok(Some(self.origin(parent, &source, blob)));
            }
        }
        Ok(None)
    }

    /// Hands the lines of `origin` on to its parents, keeping the rest.
    fn pass(&mut self, origin: usize) -> Result<()> {
        let mut pending = std::mem::take(&mut self.origins[origin].pending);
        self.origins[origin].queued = false;
        if pending.is_empty() {
            return Ok(());
        }
        let commit = self.origins[origin].commit;
        let parents = self.commit(&commit)?.parents.clone();
        let blob = self.origins[origin].blob;

        let mut found: Vec<Option<usize>> = vec![None; parents.len()];
        let passes: &[bool] = if self.options.follow_renames {
            &[false, true]
        } else {
            &[false]
        };
        for &renamed in passes {
            for (i, parent) in parents.iter().enumerate() {
                if found[i].is_some() {
                    continue;
                }
                let Some(parent_origin) = self.find_in_parent(origin, *parent, renamed)? else {
                    continue;
                };
                let parent_blob = self.origins[parent_origin].blob;
                if parent_blob == blob {
                    // The same file: every line comes from the parent.
                    return self.give(parent_origin, pending);
                }
                // A parent with the same file as an earlier one adds nothing.
                let same = found[..i]
                    .iter()
                    .flatten()
                    .any(|&o| self.origins[o].blob == parent_blob);
                if !same {
                    found[i] = Some(parent_origin);
                }
            }
        }

        for parent_origin in found.into_iter().flatten() {
            if pending.is_empty() {
                break;
            }
            // Lines of this version the parent has unchanged, mapped there.
            let unchanged = {
                let algorithm = self.options.algorithm.internal();
                let parent_content = self.content(parent_origin)?.to_vec();
                let content = self.content(origin)?;
                let new = split_lines(content);
                let mut map: Vec<Option<usize>> = vec![None; new.len()];
                let (mut i, mut j) = (0, 0);
                for hunk in diff_bytes(&parent_content, content, algorithm) {
                    while j < hunk.new_start {
                        map[j] = Some(i);
                        i += 1;
                        j += 1;
                    }
                    i = hunk.old_end();
                    j = hunk.new_end();
                }
                while j < new.len() {
                    map[j] = Some(i);
                    i += 1;
                    j += 1;
                }
                map
            };
            let mut passed = Vec::new();
            pending.retain(
                |&(final_line, line)| match unchanged.get(line).copied().flatten() {
                    Some(parent_line) => {
                        passed.push((final_line, parent_line));
                        false
                    }
                    None => true,
                },
            );
            self.give(parent_origin, passed)?;
        }
        // What no parent has comes from this commit.
        for (final_line, line) in pending {
            self.result[final_line] = Some((commit, origin, line));
        }
        Ok(())
    }
}

impl Repository {
    /// Finds the commit that last changed each line of a file, like
    /// `git blame <revision> -- <path>`.
    ///
    /// The file is taken as of HEAD, or [`BlameOptions::revision`]. Each
    /// line is handed back through history while a parent has it
    /// unchanged, compared with the diff `git blame` uses (Myers with the
    /// indent heuristic); it is blamed on the first commit, going back,
    /// whose parents do not have it. Merges try their parents in order. A
    /// file missing from a parent is looked for there under the name it
    /// was renamed from: an identical file the commit removed, or the most
    /// similar one (at least 50% alike by zerogit's measure, which can pick
    /// differently from Git's when files are close). Copies from other
    /// files (`-C`) and moved lines (`-M`) are not detected.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRevision` / `Error::RefNotFound` if the revision
    ///   cannot be resolved.
    /// - `Error::PathNotFound` if the file is not in that commit.
    /// - `Error::InvalidRevision` if [`BlameOptions::lines`] is outside the
    ///   file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{BlameOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let blame = repo.blame("src/lib.rs", &BlameOptions::new()).unwrap();
    /// for line in blame.lines() {
    ///     println!("{} {}", line.commit().short(), String::from_utf8_lossy(line.content()));
    /// }
    /// ```
    pub fn blame<P: AsRef<Path>>(&self, path: P, options: &BlameOptions) -> Result<Blame> {
        let path_text = path
            .as_ref()
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_owned();
        let revision = options.revision.as_deref().unwrap_or("HEAD");
        let oid = self.rev_parse(revision)?;
        let commit = self.peel_to(oid, ObjectType::Commit)?;

        let mut blamer = Blamer {
            repo: self,
            options,
            origins: Vec::new(),
            by_key: HashMap::new(),
            commits: HashMap::new(),
            queue: BinaryHeap::new(),
            counter: 0,
            result: Vec::new(),
        };
        let tree = blamer.commit(&commit)?.tree;
        let blob = blamer
            .blob_at(&tree, &path_text)?
            .ok_or_else(|| Error::PathNotFound(PathBuf::from(&path_text)))?;
        let start = blamer.origin(commit, &path_text, blob);
        let content = blamer.content(start)?.to_vec();
        let lines = split_lines(&content);
        let (first, last) = match options.lines {
            None => (0, lines.len()),
            Some((s, e)) if s >= 1 && s <= e && e <= lines.len() => (s - 1, e),
            Some((s, e)) => {
                return Err(Error::InvalidRevision {
                    revision: format!("-L {},{}", s, e),
                    reason: format!("the file has {} lines", lines.len()),
                })
            }
        };
        blamer.result = vec![None; lines.len()];
        blamer.give(start, (first..last).map(|l| (l, l)).collect())?;
        while let Some(Queued { origin, .. }) = blamer.queue.pop() {
            blamer.pass(origin)?;
        }

        let mut result = Vec::with_capacity(last - first);
        for (final_line, blamed) in blamer.result.iter().enumerate().take(last).skip(first) {
            let (commit, origin, line) = blamed.expect("every line is blamed");
            result.push(BlameLine {
                commit,
                path: PathBuf::from(&blamer.origins[origin].path),
                original_line: line + 1,
                final_line: final_line + 1,
                content: lines[final_line].to_vec(),
            });
        }
        Ok(Blame {
            commit,
            path: PathBuf::from(path_text),
            lines: result,
        })
    }
}
