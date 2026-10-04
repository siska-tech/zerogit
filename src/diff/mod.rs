//! Tree diff implementation.
//!
//! This module provides functionality to compute differences between two Git trees,
//! as well as between the working tree, index, and HEAD.
//! It supports detecting added, deleted, modified, renamed, and copied files.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::index::Index;
use crate::infra::{hash_object, list_working_tree, read_file};
use crate::objects::{Blob, Commit, FileMode, Oid, Tree};

pub mod blob;
pub mod rename;

use crate::Repository;
pub use blob::{
    BlobDiff, BlobDiffContent, DiffHunk, DiffLine, DiffOptions, LineEnding, LineKind,
    NonTextReason, SkipReason,
};
use rename::detect_similar_renames;
pub use rename::{RenameDetection, RenameLimit, RenameOptions};

/// The status of a file in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// File was added.
    Added,
    /// File was deleted.
    Deleted,
    /// File was modified.
    Modified,
    /// File was renamed.
    Renamed,
    /// File was copied.
    Copied,
}

impl DiffStatus {
    /// Returns a single character representing the status.
    pub fn as_char(&self) -> char {
        match self {
            DiffStatus::Added => 'A',
            DiffStatus::Deleted => 'D',
            DiffStatus::Modified => 'M',
            DiffStatus::Renamed => 'R',
            DiffStatus::Copied => 'C',
        }
    }
}

/// Statistics about a diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStats {
    /// Number of added files.
    pub added: usize,
    /// Number of deleted files.
    pub deleted: usize,
    /// Number of modified files.
    pub modified: usize,
    /// Number of renamed files.
    pub renamed: usize,
    /// Number of copied files.
    pub copied: usize,
}

impl DiffStats {
    /// Returns the total number of changed files.
    pub fn total(&self) -> usize {
        self.added + self.deleted + self.modified + self.renamed + self.copied
    }
}

/// A single entry in a diff representing one file change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDelta {
    /// The type of change.
    status: DiffStatus,
    /// The file path (new path for renames/copies, otherwise the path).
    path: PathBuf,
    /// The original path for renames/copies.
    old_path: Option<PathBuf>,
    /// The OID before the change (for deleted/modified files).
    old_oid: Option<Oid>,
    /// The OID after the change (for added/modified files).
    new_oid: Option<Oid>,
    /// The file mode before the change.
    old_mode: Option<FileMode>,
    /// The file mode after the change.
    new_mode: Option<FileMode>,
    /// Similarity in percent for renames (100 for exact renames).
    similarity: Option<u8>,
}

impl DiffDelta {
    /// Returns the status of this delta.
    pub fn status(&self) -> DiffStatus {
        self.status
    }

    /// Returns the file path.
    ///
    /// For renames and copies, this returns the new path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the original path for renames and copies.
    pub fn old_path(&self) -> Option<&Path> {
        self.old_path.as_deref()
    }

    /// Returns the OID before the change.
    pub fn old_oid(&self) -> Option<&Oid> {
        self.old_oid.as_ref()
    }

    /// Returns the OID after the change.
    pub fn new_oid(&self) -> Option<&Oid> {
        self.new_oid.as_ref()
    }

    /// Returns the file mode before the change.
    pub fn old_mode(&self) -> Option<FileMode> {
        self.old_mode
    }

    /// Returns the file mode after the change.
    pub fn new_mode(&self) -> Option<FileMode> {
        self.new_mode
    }

    /// Returns the similarity in percent for renames: 100 for exact renames,
    /// the score for similarity renames, and `None` for other changes.
    pub fn similarity(&self) -> Option<u8> {
        self.similarity
    }

    /// Returns a single character representing the status.
    pub fn status_char(&self) -> char {
        self.status.as_char()
    }

    /// Creates a new Added delta.
    fn added(path: PathBuf, oid: Oid, mode: FileMode) -> Self {
        DiffDelta {
            status: DiffStatus::Added,
            path,
            old_path: None,
            old_oid: None,
            new_oid: Some(oid),
            old_mode: None,
            new_mode: Some(mode),
            similarity: None,
        }
    }

    /// Creates a new Deleted delta.
    fn deleted(path: PathBuf, oid: Oid, mode: FileMode) -> Self {
        DiffDelta {
            status: DiffStatus::Deleted,
            path,
            old_path: None,
            old_oid: Some(oid),
            new_oid: None,
            old_mode: Some(mode),
            new_mode: None,
            similarity: None,
        }
    }

    /// Creates a new Modified delta.
    fn modified(
        path: PathBuf,
        old_oid: Oid,
        new_oid: Oid,
        old_mode: FileMode,
        new_mode: FileMode,
    ) -> Self {
        DiffDelta {
            status: DiffStatus::Modified,
            path,
            old_path: None,
            old_oid: Some(old_oid),
            new_oid: Some(new_oid),
            old_mode: Some(old_mode),
            new_mode: Some(new_mode),
            similarity: None,
        }
    }

    /// Creates a new Renamed delta for an exact (same OID) rename.
    fn renamed(
        old_path: PathBuf,
        new_path: PathBuf,
        oid: Oid,
        old_mode: FileMode,
        new_mode: FileMode,
    ) -> Self {
        DiffDelta {
            status: DiffStatus::Renamed,
            path: new_path,
            old_path: Some(old_path),
            old_oid: Some(oid),
            new_oid: Some(oid),
            old_mode: Some(old_mode),
            new_mode: Some(new_mode),
            similarity: Some(100),
        }
    }
}

/// The result of comparing two trees.
#[derive(Debug, Clone)]
pub struct TreeDiff {
    /// The list of changes.
    deltas: Vec<DiffDelta>,
    /// Limits that left similarity rename detection incomplete.
    rename_limits: Vec<RenameLimit>,
}

impl TreeDiff {
    /// Returns the deltas (changes) in this diff.
    pub fn deltas(&self) -> &[DiffDelta] {
        &self.deltas
    }

    /// Returns the limits that left similarity rename detection incomplete.
    ///
    /// Empty when detection was complete or not requested. Files affected
    /// by a limit are still reported as additions and deletions.
    pub fn rename_limits(&self) -> &[RenameLimit] {
        &self.rename_limits
    }

    /// Computes statistics about this diff.
    pub fn stats(&self) -> DiffStats {
        let mut stats = DiffStats::default();
        for delta in &self.deltas {
            match delta.status {
                DiffStatus::Added => stats.added += 1,
                DiffStatus::Deleted => stats.deleted += 1,
                DiffStatus::Modified => stats.modified += 1,
                DiffStatus::Renamed => stats.renamed += 1,
                DiffStatus::Copied => stats.copied += 1,
            }
        }
        stats
    }

    /// Returns true if there are no changes.
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// Returns the number of changes.
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Returns an iterator over the deltas.
    pub fn iter(&self) -> impl Iterator<Item = &DiffDelta> {
        self.deltas.iter()
    }
}

impl IntoIterator for TreeDiff {
    type Item = DiffDelta;
    type IntoIter = std::vec::IntoIter<DiffDelta>;

    fn into_iter(self) -> Self::IntoIter {
        self.deltas.into_iter()
    }
}

impl<'a> IntoIterator for &'a TreeDiff {
    type Item = &'a DiffDelta;
    type IntoIter = std::slice::Iter<'a, DiffDelta>;

    fn into_iter(self) -> Self::IntoIter {
        self.deltas.iter()
    }
}

/// Entry in a flattened tree.
#[derive(Debug, Clone)]
pub(crate) struct FlatEntry {
    pub(crate) oid: Oid,
    pub(crate) mode: FileMode,
}

impl Repository {
    /// Computes the diff between two trees.
    ///
    /// # Arguments
    ///
    /// * `old_tree` - The old tree (None for initial commits).
    /// * `new_tree` - The new tree.
    ///
    /// # Returns
    ///
    /// A `TreeDiff` containing all the changes between the two trees.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let old_tree = repo.tree("abc1234").unwrap();
    /// let new_tree = repo.tree("def5678").unwrap();
    /// let diff = repo.diff_trees(Some(&old_tree), &new_tree).unwrap();
    ///
    /// for delta in diff.deltas() {
    ///     println!("{} {}", delta.status_char(), delta.path().display());
    /// }
    /// ```
    pub fn diff_trees(&self, old_tree: Option<&Tree>, new_tree: &Tree) -> Result<TreeDiff> {
        self.diff_trees_with_options(old_tree, new_tree, &RenameOptions::default())
    }

    /// Computes the diff between two trees with explicit rename detection.
    ///
    /// [`RenameDetection::Exact`] (the default) matches [`Repository::diff_trees`];
    /// [`RenameDetection::Similar`] additionally pairs edited moves, reading
    /// the contents of added and deleted files. See [`RenameOptions`] for the
    /// similarity definition and limits.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{RenameDetection, RenameOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let old_tree = repo.tree("abc1234").unwrap();
    /// let new_tree = repo.tree("def5678").unwrap();
    /// let options = RenameOptions::new().detection(RenameDetection::Similar);
    /// let diff = repo
    ///     .diff_trees_with_options(Some(&old_tree), &new_tree, &options)
    ///     .unwrap();
    /// for delta in diff.deltas() {
    ///     println!("{} {} {:?}", delta.status_char(), delta.path().display(), delta.similarity());
    /// }
    /// if !diff.rename_limits().is_empty() {
    ///     println!("rename detection was limited: {:?}", diff.rename_limits());
    /// }
    /// ```
    pub fn diff_trees_with_options(
        &self,
        old_tree: Option<&Tree>,
        new_tree: &Tree,
        options: &RenameOptions,
    ) -> Result<TreeDiff> {
        // Flatten both trees
        let old_map = match old_tree {
            Some(tree) => self.flatten_tree(tree, PathBuf::new())?,
            None => HashMap::new(),
        };
        let new_map = self.flatten_tree(new_tree, PathBuf::new())?;

        // Collect all paths
        let mut all_paths: BTreeSet<PathBuf> = BTreeSet::new();
        all_paths.extend(old_map.keys().cloned());
        all_paths.extend(new_map.keys().cloned());

        // Compare entries
        let mut deltas = Vec::new();
        for path in all_paths {
            let old_entry = old_map.get(&path);
            let new_entry = new_map.get(&path);

            match (old_entry, new_entry) {
                (None, Some(entry)) => {
                    // Added
                    deltas.push(DiffDelta::added(path, entry.oid, entry.mode));
                }
                (Some(entry), None) => {
                    // Deleted
                    deltas.push(DiffDelta::deleted(path, entry.oid, entry.mode));
                }
                (Some(old), Some(new)) => {
                    // Check if modified
                    if old.oid != new.oid || old.mode != new.mode {
                        deltas.push(DiffDelta::modified(
                            path, old.oid, new.oid, old.mode, new.mode,
                        ));
                    }
                    // If OID and mode are the same, no change
                }
                (None, None) => unreachable!(),
            }
        }

        let mut rename_limits = Vec::new();
        match options.get_detection() {
            RenameDetection::Off => {}
            RenameDetection::Exact => detect_renames(&mut deltas),
            RenameDetection::Similar => {
                detect_renames(&mut deltas);
                rename_limits = detect_similar_renames(&mut deltas, options, &mut |oid| {
                    Ok(self.blob(&oid.to_hex())?.content().to_vec())
                })?;
            }
        }

        Ok(TreeDiff {
            deltas,
            rename_limits,
        })
    }

    /// Computes the diff for a commit against its first parent.
    ///
    /// For root commits (no parent), all files are shown as Added.
    /// For merge commits (multiple parents), only the diff against the first parent is returned.
    ///
    /// # Arguments
    ///
    /// * `commit` - The commit to compute the diff for.
    ///
    /// # Returns
    ///
    /// A `TreeDiff` containing all the changes in this commit.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let commit = repo.commit("abc1234").unwrap();
    /// let diff = repo.commit_diff(&commit).unwrap();
    ///
    /// for delta in diff.deltas() {
    ///     println!("{} {}", delta.status_char(), delta.path().display());
    /// }
    /// ```
    pub fn commit_diff(&self, commit: &Commit) -> Result<TreeDiff> {
        self.commit_diff_with_options(commit, &RenameOptions::default())
    }

    /// Computes the diff for a commit against its first parent with explicit
    /// rename detection (see [`Repository::diff_trees_with_options`]).
    pub fn commit_diff_with_options(
        &self,
        commit: &Commit,
        options: &RenameOptions,
    ) -> Result<TreeDiff> {
        // Get the tree of the current commit
        let new_tree = self.tree(&commit.tree().to_hex())?;

        // Get the tree of the first parent (if any)
        let old_tree = if let Some(parent_oid) = commit.parent() {
            let parent_commit = self.commit(&parent_oid.to_hex())?;
            Some(self.tree(&parent_commit.tree().to_hex())?)
        } else {
            // Root commit: no parent, all files are added
            None
        };

        // Compute the diff between the trees
        self.diff_trees_with_options(old_tree.as_ref(), &new_tree, options)
    }

    /// Computes the line diff between two blobs.
    ///
    /// `None` stands for a missing side, such as the old side of an added
    /// file; it is compared as empty content but reported as missing.
    ///
    /// # Errors
    ///
    /// Returns an error if a blob cannot be read or an OID is not a blob.
    /// Binary content and exceeded limits are not errors; they are reported
    /// in [`BlobDiff::content`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{BlobDiffContent, DiffOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let commit = repo.commit("abc1234").unwrap();
    /// for delta in repo.commit_diff(&commit).unwrap().deltas() {
    ///     let diff = repo
    ///         .diff_blobs(delta.old_oid(), delta.new_oid(), &DiffOptions::new())
    ///         .unwrap();
    ///     match diff.content() {
    ///         BlobDiffContent::Text(hunks) => {
    ///             for hunk in hunks {
    ///                 println!("{}", hunk.header());
    ///                 for line in hunk.lines() {
    ///                     println!("{:?} {:?} {}", line.old_lineno(), line.new_lineno(), line.text());
    ///                 }
    ///             }
    ///         }
    ///         BlobDiffContent::NonText(reason) => println!("not text: {:?}", reason),
    ///         BlobDiffContent::Skipped(reason) => println!("skipped: {}", reason),
    ///     }
    /// }
    /// ```
    pub fn diff_blobs(
        &self,
        old: Option<&Oid>,
        new: Option<&Oid>,
        options: &DiffOptions,
    ) -> Result<BlobDiff> {
        let read = |oid: Option<&Oid>| -> Result<Option<Blob>> {
            oid.map(|oid| self.blob(&oid.to_hex())).transpose()
        };
        let (old, new) = (read(old)?, read(new)?);
        Ok(BlobDiff::compute(
            old.as_ref().map(Blob::content),
            new.as_ref().map(Blob::content),
            options,
        ))
    }

    /// Flattens a tree into a map of path -> (oid, mode).
    pub(crate) fn flatten_tree(
        &self,
        tree: &Tree,
        prefix: PathBuf,
    ) -> Result<HashMap<PathBuf, FlatEntry>> {
        let mut result = HashMap::new();

        for entry in tree.entries() {
            let path = if prefix.as_os_str().is_empty() {
                PathBuf::from(entry.name())
            } else {
                prefix.join(entry.name())
            };

            if entry.is_directory() {
                // Recursively flatten subtree
                let subtree = self.tree(&entry.oid().to_hex())?;
                result.extend(self.flatten_tree(&subtree, path)?);
            } else {
                // File entry
                result.insert(
                    path,
                    FlatEntry {
                        oid: *entry.oid(),
                        mode: entry.mode(),
                    },
                );
            }
        }

        Ok(result)
    }

    /// Computes the diff between the index and the working tree.
    ///
    /// This is equivalent to `git diff` (without arguments), showing unstaged changes.
    ///
    /// # Returns
    ///
    /// A `TreeDiff` containing all changes between the index and working tree.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let diff = repo.diff_index_to_workdir().unwrap();
    ///
    /// println!("Unstaged changes:");
    /// for delta in diff.deltas() {
    ///     println!("  {} {}", delta.status_char(), delta.path().display());
    /// }
    /// ```
    pub fn diff_index_to_workdir(&self) -> Result<TreeDiff> {
        let index = self.read_index_internal()?;
        let index_map = index_to_flat_map(&index);
        let workdir_map = self.workdir_to_flat_map(&index)?;

        Ok(diff_flat_maps(&index_map, &workdir_map))
    }

    /// Computes the diff between HEAD and the index.
    ///
    /// This is equivalent to `git diff --staged` or `git diff --cached`,
    /// showing staged changes.
    ///
    /// # Returns
    ///
    /// A `TreeDiff` containing all changes between HEAD and the index.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let diff = repo.diff_head_to_index().unwrap();
    ///
    /// println!("Staged changes:");
    /// for delta in diff.deltas() {
    ///     println!("  {} {}", delta.status_char(), delta.path().display());
    /// }
    /// ```
    pub fn diff_head_to_index(&self) -> Result<TreeDiff> {
        let head_map = self.get_head_flat_map()?;
        let index = self.read_index_internal()?;
        let index_map = index_to_flat_map(&index);

        Ok(diff_flat_maps(&head_map, &index_map))
    }

    /// Computes the diff between HEAD and the working tree.
    ///
    /// This is equivalent to `git diff HEAD`, showing all changes
    /// (both staged and unstaged) since the last commit.
    ///
    /// # Returns
    ///
    /// A `TreeDiff` containing all changes between HEAD and the working tree.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let diff = repo.diff_head_to_workdir().unwrap();
    ///
    /// println!("All changes from HEAD:");
    /// for delta in diff.deltas() {
    ///     println!("  {} {}", delta.status_char(), delta.path().display());
    /// }
    /// ```
    pub fn diff_head_to_workdir(&self) -> Result<TreeDiff> {
        let head_map = self.get_head_flat_map()?;
        let index = self.read_index_internal()?;
        let workdir_map = self.workdir_to_flat_map(&index)?;

        Ok(diff_flat_maps(&head_map, &workdir_map))
    }

    /// Gets the flattened tree map from HEAD.
    fn get_head_flat_map(&self) -> Result<HashMap<PathBuf, FlatEntry>> {
        let head = self.head()?;
        let commit = self.commit(&head.oid().to_hex())?;
        let tree = self.tree(&commit.tree().to_hex())?;
        self.flatten_tree(&tree, PathBuf::new())
    }

    /// Reads the index file.
    fn read_index_internal(&self) -> Result<Index> {
        let index_path = self.git_dir().join("index");
        if index_path.exists() {
            let index_data = read_file(&index_path)?;
            crate::index::parse(&index_data)
        } else {
            Ok(Index::empty(2))
        }
    }

    /// Builds a flat entry map from the working tree.
    ///
    /// This walks the working tree and computes hashes for all files.
    /// For performance, if a file exists in the index with matching
    /// mtime and size, we skip re-hashing and use the index's OID.
    fn workdir_to_flat_map(&self, index: &Index) -> Result<HashMap<PathBuf, FlatEntry>> {
        let mut map = HashMap::new();
        let work_dir = self.path();

        for file_path in list_working_tree(work_dir)? {
            let full_path = work_dir.join(&file_path);
            // Normalize path for cross-platform consistency
            let normalized_path = normalize_path(&file_path);

            // Read file content and compute hash
            let content = read_file(&full_path)?;
            let hash = hash_object("blob", &content);
            let oid = Oid::from_bytes(hash);

            // Get mode from index if available, otherwise detect
            // Try both normalized path and original path for index lookup
            let mode = index
                .get(&file_path)
                .or_else(|| index.get(&normalized_path))
                .map(|e| e.mode())
                .unwrap_or_else(|| detect_file_mode(&full_path));

            map.insert(normalized_path, FlatEntry { oid, mode });
        }

        // Skip-worktree (sparse checkout) entries are taken as up to date,
        // whether or not a file is present, as Git does.
        for entry in index.entries().iter().filter(|e| e.skip_worktree()) {
            map.insert(
                normalize_path(entry.path()),
                FlatEntry {
                    oid: *entry.oid(),
                    mode: entry.mode(),
                },
            );
        }

        Ok(map)
    }
}

/// Converts an Index to a flat entry map.
///
/// Intent-to-add entries carry no staged content, so they are left out: like
/// Git, they do not appear against HEAD and show as new in the working tree.
fn index_to_flat_map(index: &Index) -> HashMap<PathBuf, FlatEntry> {
    index
        .entries()
        .iter()
        .filter(|e| !e.intent_to_add())
        .map(|e| {
            (
                normalize_path(e.path()),
                FlatEntry {
                    oid: *e.oid(),
                    mode: e.mode(),
                },
            )
        })
        .collect()
}

/// Normalizes a path to use forward slashes consistently.
///
/// This ensures paths from the index (which use `/`) match paths from
/// the working tree (which may use `\` on Windows).
fn normalize_path(path: &Path) -> PathBuf {
    // Convert to string and replace backslashes with forward slashes
    let path_str = path.to_string_lossy();
    PathBuf::from(path_str.replace('\\', "/"))
}

/// Detects the file mode of a file in the working tree.
#[allow(unused_variables)]
fn detect_file_mode(path: &Path) -> FileMode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.permissions().mode() & 0o111 != 0 {
                return FileMode::Executable;
            }
        }
    }
    FileMode::Regular
}

/// Groups modes whose contents may be renamed into each other.
///
/// Regular and executable files are interchangeable (a rename may also flip
/// the executable bit); symlinks and gitlinks only match their own kind.
fn rename_kind(mode: FileMode) -> u8 {
    match mode {
        FileMode::Regular | FileMode::Executable => 0,
        FileMode::Symlink => 1,
        FileMode::Submodule => 2,
        FileMode::Directory => 3,
    }
}

/// Detects exact renames by pairing deleted and added entries with the same OID.
///
/// Pairing is one-to-one and deterministic: added paths are visited in path
/// order, and each takes the unmatched deleted path of the same OID and kind
/// that has the same file name, or else the first one in path order.
fn detect_renames(deltas: &mut Vec<DiffDelta>) {
    deltas.sort_by(|a, b| a.path.cmp(&b.path));
    let mut sources: HashMap<(Oid, u8), Vec<usize>> = HashMap::new();
    for (i, delta) in deltas.iter().enumerate() {
        if let (DiffStatus::Deleted, Some(oid), Some(mode)) =
            (delta.status, delta.old_oid, delta.old_mode)
        {
            sources.entry((oid, rename_kind(mode))).or_default().push(i);
        }
    }

    let mut matched = vec![false; deltas.len()];
    let mut renames = Vec::new();
    for (i, added) in deltas.iter().enumerate() {
        let (DiffStatus::Added, Some(oid), Some(new_mode)) =
            (added.status, added.new_oid, added.new_mode)
        else {
            continue;
        };
        let Some(candidates) = sources.get(&(oid, rename_kind(new_mode))) else {
            continue;
        };
        let source = candidates
            .iter()
            .copied()
            .filter(|&s| !matched[s])
            .min_by_key(|&s| (deltas[s].path.file_name() != added.path.file_name(), s));
        if let Some(source) = source {
            matched[source] = true;
            matched[i] = true;
            let deleted = &deltas[source];
            renames.push(DiffDelta::renamed(
                deleted.path.clone(),
                added.path.clone(),
                oid,
                deleted.old_mode.unwrap_or(FileMode::Regular),
                new_mode,
            ));
        }
    }

    let mut index = 0;
    deltas.retain(|_| {
        index += 1;
        !matched[index - 1]
    });
    deltas.extend(renames);
    deltas.sort_by(|a, b| a.path.cmp(&b.path));
}

/// Computes diff between two flat entry maps.
///
/// This is an internal helper function used by the various diff methods
/// to compare two maps of path -> (oid, mode).
fn diff_flat_maps(
    old_map: &HashMap<PathBuf, FlatEntry>,
    new_map: &HashMap<PathBuf, FlatEntry>,
) -> TreeDiff {
    // Collect all paths
    let mut all_paths: BTreeSet<PathBuf> = BTreeSet::new();
    all_paths.extend(old_map.keys().cloned());
    all_paths.extend(new_map.keys().cloned());

    // Compare entries
    let mut deltas = Vec::new();
    for path in all_paths {
        let old_entry = old_map.get(&path);
        let new_entry = new_map.get(&path);

        match (old_entry, new_entry) {
            (None, Some(entry)) => {
                // Added
                deltas.push(DiffDelta::added(path, entry.oid, entry.mode));
            }
            (Some(entry), None) => {
                // Deleted
                deltas.push(DiffDelta::deleted(path, entry.oid, entry.mode));
            }
            (Some(old), Some(new)) => {
                // Check if modified
                if old.oid != new.oid || old.mode != new.mode {
                    deltas.push(DiffDelta::modified(
                        path, old.oid, new.oid, old.mode, new.mode,
                    ));
                }
                // If OID and mode are the same, no change
            }
            (None, None) => unreachable!(),
        }
    }

    // Detect renames
    detect_renames(&mut deltas);

    TreeDiff {
        deltas,
        rename_limits: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_status_as_char() {
        assert_eq!(DiffStatus::Added.as_char(), 'A');
        assert_eq!(DiffStatus::Deleted.as_char(), 'D');
        assert_eq!(DiffStatus::Modified.as_char(), 'M');
        assert_eq!(DiffStatus::Renamed.as_char(), 'R');
        assert_eq!(DiffStatus::Copied.as_char(), 'C');
    }

    #[test]
    fn test_diff_stats_total() {
        let stats = DiffStats {
            added: 2,
            deleted: 1,
            modified: 3,
            renamed: 1,
            copied: 0,
        };
        assert_eq!(stats.total(), 7);
    }

    #[test]
    fn test_diff_stats_default() {
        let stats = DiffStats::default();
        assert_eq!(stats.added, 0);
        assert_eq!(stats.deleted, 0);
        assert_eq!(stats.modified, 0);
        assert_eq!(stats.renamed, 0);
        assert_eq!(stats.copied, 0);
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn test_tree_diff_is_empty() {
        let diff = TreeDiff {
            deltas: vec![],
            rename_limits: Vec::new(),
        };
        assert!(diff.is_empty());
        assert_eq!(diff.len(), 0);
    }

    #[test]
    fn test_tree_diff_stats() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let deltas = vec![
            DiffDelta::added(PathBuf::from("new.txt"), oid, FileMode::Regular),
            DiffDelta::deleted(PathBuf::from("old.txt"), oid, FileMode::Regular),
            DiffDelta::modified(
                PathBuf::from("changed.txt"),
                oid,
                oid,
                FileMode::Regular,
                FileMode::Regular,
            ),
        ];
        let diff = TreeDiff {
            deltas,
            rename_limits: Vec::new(),
        };

        let stats = diff.stats();
        assert_eq!(stats.added, 1);
        assert_eq!(stats.deleted, 1);
        assert_eq!(stats.modified, 1);
        assert_eq!(stats.total(), 3);
    }

    #[test]
    fn test_diff_delta_accessors() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let delta = DiffDelta::added(PathBuf::from("test.txt"), oid, FileMode::Regular);

        assert_eq!(delta.status(), DiffStatus::Added);
        assert_eq!(delta.path(), Path::new("test.txt"));
        assert!(delta.old_path().is_none());
        assert!(delta.old_oid().is_none());
        assert_eq!(delta.new_oid(), Some(&oid));
        assert!(delta.old_mode().is_none());
        assert_eq!(delta.new_mode(), Some(FileMode::Regular));
        assert_eq!(delta.status_char(), 'A');
    }

    #[test]
    fn test_diff_delta_renamed() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let delta = DiffDelta::renamed(
            PathBuf::from("old.txt"),
            PathBuf::from("new.txt"),
            oid,
            FileMode::Regular,
            FileMode::Executable,
        );

        assert_eq!(delta.status(), DiffStatus::Renamed);
        assert_eq!(delta.path(), Path::new("new.txt"));
        assert_eq!(delta.old_path(), Some(Path::new("old.txt")));
        assert_eq!(delta.old_oid(), Some(&oid));
        assert_eq!(delta.new_oid(), Some(&oid));
        assert_eq!(delta.old_mode(), Some(FileMode::Regular));
        assert_eq!(delta.new_mode(), Some(FileMode::Executable));
        assert_eq!(delta.status_char(), 'R');
    }

    fn summary(deltas: &[DiffDelta]) -> Vec<String> {
        deltas
            .iter()
            .map(|d| {
                format!(
                    "{} {:?} -> {} {:?} {:?}",
                    d.status_char(),
                    d.old_path(),
                    d.path().display(),
                    d.old_mode(),
                    d.new_mode()
                )
            })
            .collect()
    }

    #[test]
    fn test_detect_renames_keeps_both_modes() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let mut deltas = vec![
            DiffDelta::deleted(PathBuf::from("run.sh"), oid, FileMode::Regular),
            DiffDelta::added(PathBuf::from("bin/run.sh"), oid, FileMode::Executable),
        ];
        detect_renames(&mut deltas);
        assert_eq!(
            summary(&deltas),
            ["R Some(\"run.sh\") -> bin/run.sh Some(Regular) Some(Executable)"]
        );
    }

    #[test]
    fn test_detect_renames_pairs_duplicates_deterministically() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let build = |order: &[usize]| {
            let all = [
                DiffDelta::deleted(PathBuf::from("a/x.txt"), oid, FileMode::Regular),
                DiffDelta::deleted(PathBuf::from("b/y.txt"), oid, FileMode::Regular),
                DiffDelta::deleted(PathBuf::from("c/z.txt"), oid, FileMode::Regular),
                DiffDelta::added(PathBuf::from("d/y.txt"), oid, FileMode::Regular),
                DiffDelta::added(PathBuf::from("e/new.txt"), oid, FileMode::Regular),
            ];
            let mut deltas: Vec<_> = order.iter().map(|&i| all[i].clone()).collect();
            detect_renames(&mut deltas);
            summary(&deltas)
        };
        let expected = build(&[0, 1, 2, 3, 4]);
        // The same file name wins, then path order; leftovers stay deletions.
        assert_eq!(
            expected,
            [
                "D None -> c/z.txt Some(Regular) None",
                "R Some(\"b/y.txt\") -> d/y.txt Some(Regular) Some(Regular)",
                "R Some(\"a/x.txt\") -> e/new.txt Some(Regular) Some(Regular)",
            ]
        );
        assert_eq!(build(&[4, 3, 2, 1, 0]), expected);
        assert_eq!(build(&[2, 4, 0, 3, 1]), expected);
    }

    #[test]
    fn test_detect_renames_respects_file_kinds() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        for (old, new) in [
            (FileMode::Regular, FileMode::Symlink),
            (FileMode::Symlink, FileMode::Executable),
            (FileMode::Submodule, FileMode::Regular),
        ] {
            let mut deltas = vec![
                DiffDelta::deleted(PathBuf::from("old"), oid, old),
                DiffDelta::added(PathBuf::from("new"), oid, new),
            ];
            detect_renames(&mut deltas);
            assert_eq!(deltas.len(), 2, "{:?} -> {:?}", old, new);
        }
        for mode in [FileMode::Symlink, FileMode::Submodule] {
            let mut deltas = vec![
                DiffDelta::deleted(PathBuf::from("old"), oid, mode),
                DiffDelta::added(PathBuf::from("new"), oid, mode),
            ];
            detect_renames(&mut deltas);
            assert_eq!(deltas.len(), 1);
            assert_eq!(deltas[0].status(), DiffStatus::Renamed);
        }
    }

    #[test]
    fn test_detect_renames() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let mut deltas = vec![
            DiffDelta::deleted(PathBuf::from("old.txt"), oid, FileMode::Regular),
            DiffDelta::added(PathBuf::from("new.txt"), oid, FileMode::Regular),
        ];

        detect_renames(&mut deltas);

        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].status(), DiffStatus::Renamed);
        assert_eq!(deltas[0].path(), Path::new("new.txt"));
        assert_eq!(deltas[0].old_path(), Some(Path::new("old.txt")));
    }

    #[test]
    fn test_detect_renames_no_match() {
        let oid1 = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let oid2 = Oid::from_hex("0000000000000000000000000000000000000000").unwrap();
        let mut deltas = vec![
            DiffDelta::deleted(PathBuf::from("old.txt"), oid1, FileMode::Regular),
            DiffDelta::added(PathBuf::from("new.txt"), oid2, FileMode::Regular),
        ];

        detect_renames(&mut deltas);

        // No rename detected, still separate add and delete
        assert_eq!(deltas.len(), 2);
    }
}
