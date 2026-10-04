//! File operations: `git restore`, `git rm` and `git mv`.
//!
//! Paths are given as pathspecs relative to the repository root: a file, a
//! directory (for everything under it), or a glob in which `*`, `?` and
//! `[...]` also match `/` (as Git's pathspecs do by default). `.` means
//! every path.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{path_key, IndexEntry};
use crate::merge::tree::Flat;
use crate::objects::{ObjectType, Oid};
use crate::pathspec::Pathspec;
use crate::repository::Repository;
use crate::worktree::native_path;

/// What [`Repository::restore`] restores, and from where.
///
/// By default (`git restore <paths>`), the work tree is restored from the
/// index. [`RestoreOptions::staged`] restores the index (from HEAD by
/// default), and [`RestoreOptions::source`] names the commit or tree to
/// restore from.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    source: Option<String>,
    staged: bool,
    worktree: Option<bool>,
}

impl RestoreOptions {
    /// The default options: the work tree, from the index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores from a revision (`--source`): a commit, or anything
    /// [`Repository::rev_parse`] resolves to a commit or tree.
    pub fn source(mut self, revision: impl Into<String>) -> Self {
        self.source = Some(revision.into());
        self
    }

    /// Restores the index (`--staged`). Unless
    /// [`RestoreOptions::worktree`] is also set, the work tree is then left
    /// alone, and the default source is HEAD.
    pub fn staged(mut self, staged: bool) -> Self {
        self.staged = staged;
        self
    }

    /// Restores the work tree (`--worktree`), the default unless
    /// [`RestoreOptions::staged`] is set.
    pub fn worktree(mut self, worktree: bool) -> Self {
        self.worktree = Some(worktree);
        self
    }
}

/// How [`Repository::remove`] removes paths.
#[derive(Debug, Clone, Default)]
pub struct RemoveOptions {
    cached: bool,
    force: bool,
    recursive: bool,
}

impl RemoveOptions {
    /// The default options: remove files from the index and the work tree,
    /// refusing to lose uncommitted changes and to remove directories.
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes paths from the index only, keeping the files (`--cached`).
    pub fn cached(mut self, cached: bool) -> Self {
        self.cached = cached;
        self
    }

    /// Removes paths even if uncommitted changes are lost (`--force`).
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Allows a directory to be given for everything under it (`-r`).
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }
}

/// A `/`-separated path key as a string.
fn key_of(path: &Path) -> String {
    String::from_utf8_lossy(&path_key(path)).into_owned()
}

/// Fails with `Error::PathspecNotMatched` for the first pathspec that
/// matches none of `known`.
fn check_all_matched<'a>(
    spec: &Pathspec,
    known: impl Iterator<Item = &'a String> + Clone,
) -> Result<()> {
    for pattern in spec.patterns() {
        if !known
            .clone()
            .any(|path| Pathspec::matches_pattern(&pattern, path.as_bytes()))
        {
            let shown = if pattern.is_empty() {
                ".".to_owned()
            } else {
                pattern
            };
            return Err(Error::PathspecNotMatched(shown));
        }
    }
    Ok(())
}

impl Repository {
    /// Restores files in the work tree and/or the index, like
    /// `git restore [--source=<rev>] [--staged] [--worktree] <paths>`.
    ///
    /// - Work tree (the default): matching files get their content from the
    ///   index, or from [`RestoreOptions::source`]; with a source, tracked
    ///   files the source does not have are removed. Content is written as
    ///   `checkout` writes it (line endings, symbolic links, executable
    ///   bit), and the index records the new stat data, so `status()` is
    ///   clean for them right away.
    /// - Index ([`RestoreOptions::staged`]): matching entries are set as in
    ///   the source (HEAD by default), or removed when the source does not
    ///   have them; this resolves conflicts.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// - `Error::PathspecNotMatched` if a pathspec matches nothing in the
    ///   index or the source.
    /// - `Error::UnmergedPaths` when restoring the work tree from the index
    ///   for conflicted paths (as Git refuses without `--ours`/`--theirs`).
    /// - `Error::InvalidRevision` if the source cannot be resolved, or
    ///   `Error::TypeMismatch` if it is neither a commit nor a tree.
    /// - `Error::Locked` if the index is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{Repository, RestoreOptions};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Discard changes to src/ in the work tree.
    /// repo.restore(&["src"], &RestoreOptions::new()).unwrap();
    /// // Unstage a file.
    /// repo.restore(&["notes.txt"], &RestoreOptions::new().staged(true)).unwrap();
    /// // Bring back a file as it was two commits ago, staged and on disk.
    /// let both = RestoreOptions::new().source("HEAD~2").staged(true).worktree(true);
    /// repo.restore(&["README.md"], &both).unwrap();
    /// ```
    pub fn restore<P: AsRef<Path>>(&self, paths: &[P], options: &RestoreOptions) -> Result<()> {
        let spec = Pathspec::new(paths);
        let to_index = options.staged;
        let to_worktree = options.worktree.unwrap_or(!options.staged);
        let (lock, mut idx) = self.lock_index()?;

        // The source tree: the given revision, HEAD for the index, or none
        // (the index itself) for the work tree.
        let source: Option<Flat> = match (&options.source, to_index) {
            (Some(revision), _) => {
                let tree = self.peel_to(self.rev_parse(revision)?, ObjectType::Tree)?;
                Some(self.flat_of_tree(&tree)?)
            }
            (None, true) => Some(self.flat_tree(self.optional_head_oid()?.as_ref())?),
            (None, false) => None,
        };

        let index_paths: BTreeSet<String> = idx
            .entries()
            .iter()
            .map(|e| key_of(e.path()))
            .filter(|path| spec.matches(path.as_bytes()))
            .collect();
        let source_paths: BTreeSet<String> = match &source {
            Some(tree) => tree
                .keys()
                .filter(|path| spec.matches(path.as_bytes()))
                .cloned()
                .collect(),
            None => index_paths.clone(),
        };
        let paths: BTreeSet<String> = index_paths.union(&source_paths).cloned().collect();
        check_all_matched(&spec, paths.iter())?;

        if to_worktree && source.is_none() {
            let unmerged: Vec<PathBuf> = paths
                .iter()
                .filter(|p| {
                    idx.get(Path::new(p.as_str()))
                        .is_some_and(IndexEntry::is_conflicted)
                })
                .map(PathBuf::from)
                .collect();
            if !unmerged.is_empty() {
                return Err(Error::UnmergedPaths(unmerged));
            }
        }

        if to_index {
            let tree = source.as_ref().expect("the index is restored from a tree");
            for path in &paths {
                let index_path = Path::new(path);
                match tree.get(path) {
                    Some((oid, mode)) => {
                        let unchanged =
                            idx.get_stage(index_path, 0).is_some_and(|e| {
                                e.oid() == oid && e.mode() == *mode && !e.intent_to_add()
                            }) && !idx.get(index_path).is_some_and(IndexEntry::is_conflicted);
                        if !unchanged {
                            idx.remove(index_path);
                            idx.add(IndexEntry::new(
                                0,
                                0,
                                0,
                                0,
                                *mode,
                                0,
                                0,
                                0,
                                *oid,
                                PathBuf::from(path),
                                0,
                            ));
                        }
                    }
                    None => {
                        idx.remove(index_path);
                    }
                }
            }
        }

        if to_worktree {
            let mut worktree = self.worktree()?;
            let store = self.object_store();
            // Check every conversion first, so an unsupported attribute
            // changes nothing.
            for path in &paths {
                worktree.check_supported(&native_path(Path::new(path)))?;
            }
            for path in &paths {
                let native = native_path(Path::new(path));
                let entry = idx.get_stage(Path::new(path), 0).cloned();
                if entry
                    .as_ref()
                    .is_some_and(|e| e.skip_worktree() || e.intent_to_add())
                    && source.is_none()
                {
                    continue;
                }
                let wanted = match &source {
                    Some(tree) => tree.get(path).copied(),
                    None => entry.as_ref().map(|e| (*e.oid(), e.mode())),
                };
                match wanted {
                    Some((oid, mode)) => {
                        worktree.write(&native, &store.read(&oid)?.content, mode)?;
                        // The index entry, if it has this content, gets the
                        // new stat data.
                        if entry.is_some_and(|e| *e.oid() == oid && e.mode() == mode) {
                            idx.refresh(worktree.stat_entry(
                                &native,
                                PathBuf::from(path),
                                oid,
                                mode,
                            )?);
                        }
                    }
                    None => worktree.remove(&native)?,
                }
            }
        }
        lock.write(&idx)
    }

    /// Removes paths from the index and the work tree, like
    /// `git rm [--cached] [-f] [-r] <paths>`, and returns the removed paths
    /// (`/`-separated).
    ///
    /// As in Git, a path is refused unless [`RemoveOptions::force`] if its
    /// removal would lose uncommitted changes: staged changes (the index
    /// differs from HEAD) or changes in the work tree (the file differs from
    /// the index), or, with [`RemoveOptions::cached`], only when the index
    /// differs from both. A conflicted path can always be removed.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// - `Error::PathspecNotMatched` if a pathspec matches no tracked file.
    /// - `Error::InvalidPathOperation` if a directory is given without
    ///   [`RemoveOptions::recursive`].
    /// - `Error::UncommittedChanges` for paths whose changes would be lost.
    /// - `Error::Locked` if the index is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{RemoveOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Stop tracking build output but keep the files.
    /// repo.remove(&["build"], &RemoveOptions::new().cached(true).recursive(true))
    ///     .unwrap();
    /// ```
    pub fn remove<P: AsRef<Path>>(
        &self,
        paths: &[P],
        options: &RemoveOptions,
    ) -> Result<Vec<PathBuf>> {
        let spec = Pathspec::new(paths);
        let (lock, mut idx) = self.lock_index()?;
        let mut matched: Vec<String> = idx
            .entries()
            .iter()
            .map(|e| key_of(e.path()))
            .filter(|path| spec.matches(path.as_bytes()))
            .collect();
        matched.dedup();
        check_all_matched(&spec, matched.iter())?;

        if !options.recursive {
            for pattern in spec.patterns() {
                if matched
                    .iter()
                    .any(|path| Pathspec::matches_as_directory(&pattern, path.as_bytes()))
                {
                    return Err(Error::InvalidPathOperation {
                        path: PathBuf::from(&pattern),
                        reason: "not removing a directory recursively without -r".to_owned(),
                    });
                }
            }
        }

        let mut worktree = self.worktree()?;
        if !options.force {
            let head = self.flat_tree(self.optional_head_oid()?.as_ref())?;
            let mut lost = Vec::new();
            for path in &matched {
                let index_path = Path::new(path);
                if idx.get(index_path).is_some_and(IndexEntry::is_conflicted) {
                    continue;
                }
                let Some(entry) = idx.get_stage(index_path, 0) else {
                    continue;
                };
                let staged = entry.intent_to_add()
                    || head.get(path).copied() != Some((*entry.oid(), entry.mode()));
                let native = native_path(index_path);
                let local = match worktree.hash(&native, Some(entry))? {
                    Some(current) => current != (*entry.oid(), entry.mode()),
                    // Already deleted from the work tree: nothing to lose.
                    None => false,
                };
                if (staged && local) || (!options.cached && (staged || local)) {
                    lost.push(PathBuf::from(path));
                }
            }
            if !lost.is_empty() {
                return Err(Error::UncommittedChanges(lost));
            }
        }

        for path in &matched {
            idx.remove(Path::new(path));
        }
        if !options.cached {
            for path in &matched {
                worktree.remove(&native_path(Path::new(path)))?;
            }
        }
        lock.write(&idx)?;
        Ok(matched.into_iter().map(PathBuf::from).collect())
    }

    /// Moves or renames a tracked file or directory in the work tree and
    /// the index, like `git mv <source> <destination>`. If the destination
    /// is an existing directory, the source is moved into it.
    ///
    /// The moved entries keep their content and stat data, so nothing is
    /// read or rehashed.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// `Error::InvalidPathOperation` when Git refuses the move: the source is
    /// not tracked, is conflicted or (any of its files) is missing from the
    /// work tree; the destination exists in the work tree (a tracked path
    /// deleted from it may be overwritten, as in Git), is inside the source,
    /// or is in a directory that does not exist. `Error::Locked` if the index
    /// is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.move_path("old_name.rs", "src/new_name.rs").unwrap();
    /// ```
    pub fn move_path(&self, source: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<()> {
        let refuse = |path: &str, reason: &str| Error::InvalidPathOperation {
            path: PathBuf::from(path),
            reason: reason.to_owned(),
        };
        let normalize = |p: &Path| {
            let key = key_of(p);
            let key = key
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_owned();
            key
        };
        let source = normalize(source.as_ref());
        let mut destination = normalize(destination.as_ref());
        let (lock, mut idx) = self.lock_index()?;

        let moved: Vec<IndexEntry> = idx
            .entries()
            .iter()
            .filter(|e| {
                let path = key_of(e.path());
                path == source
                    || (path.starts_with(&source)
                        && path.as_bytes().get(source.len()) == Some(&b'/'))
            })
            .cloned()
            .collect();
        if source.is_empty() || moved.is_empty() {
            return Err(refuse(&source, "not under version control"));
        }
        if moved.iter().any(IndexEntry::is_conflicted) {
            return Err(refuse(&source, "conflicted"));
        }
        let work_dir = self.path();
        let source_full = work_dir.join(native_path(Path::new(&source)));
        if fs::symlink_metadata(&source_full).is_err() {
            return Err(refuse(&source, "bad source: missing from the work tree"));
        }
        // Every tracked file of a directory must be there, as Git checks.
        for entry in &moved {
            let path = key_of(entry.path());
            let full = work_dir.join(native_path(Path::new(&path)));
            if fs::symlink_metadata(full).is_err() {
                return Err(refuse(&path, "bad source: missing from the work tree"));
            }
        }
        let destination_full = work_dir.join(native_path(Path::new(&destination)));
        if destination.is_empty() || destination_full.is_dir() {
            let name = source.rsplit('/').next().unwrap_or(&source);
            destination = if destination.is_empty() {
                name.to_owned()
            } else {
                format!("{}/{}", destination, name)
            };
        }
        if destination == source
            || (destination.starts_with(&source)
                && destination.as_bytes().get(source.len()) == Some(&b'/'))
        {
            return Err(refuse(&destination, "cannot move a directory into itself"));
        }
        let destination_full = work_dir.join(native_path(Path::new(&destination)));
        // A tracked path deleted from the work tree may be overwritten; its
        // index entry is replaced, as in Git.
        if fs::symlink_metadata(&destination_full).is_ok() {
            return Err(refuse(&destination, "destination exists"));
        }
        if let Some(parent) = destination_full.parent() {
            if !parent.is_dir() {
                return Err(refuse(&destination, "destination directory does not exist"));
            }
        }

        fs::rename(&source_full, &destination_full)?;
        for entry in moved {
            let old = key_of(entry.path());
            let new = format!("{}{}", destination, &old[source.len()..]);
            idx.remove(Path::new(&old));
            idx.add(entry.with_path(PathBuf::from(new)));
        }
        if let Err(e) = lock.write(&idx) {
            // Put the files back so the work tree matches the index again.
            let _ = fs::rename(&destination_full, &source_full);
            return Err(e);
        }
        Ok(())
    }

    /// The flattened entries of a tree object.
    fn flat_of_tree(&self, tree: &Oid) -> Result<Flat> {
        crate::stash::flat_of_tree(self, tree)
    }
}
