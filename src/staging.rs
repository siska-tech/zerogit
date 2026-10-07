//! Staging: adding files to the index and resetting index entries.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::IndexEntry;
use crate::objects::tree::FileMode;
use crate::objects::{ObjectType, Oid};
use crate::repository::{reject_sparse_checkout, Repository};
use crate::status::flatten_tree_with_store as flatten_tree;

impl Repository {
    /// Adds a file to the staging area (index).
    ///
    /// This reads the file from the working tree, creates a blob object,
    /// and updates the index with the file's information.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file, relative to the repository root.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Errors
    ///
    /// - `Error::PathNotFound` if the file does not exist.
    /// - `Error::IgnoredPath` if the file is not tracked and is ignored by
    ///   `.gitignore` (as `git add` refuses it); use [`Repository::add_force`]
    ///   to add it anyway. A tracked file is added even if it is ignored.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.add("src/main.rs").unwrap();
    /// ```
    pub fn add<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.add_impl(path.as_ref(), false)
    }

    /// Adds a file to the staging area even if it is ignored, like
    /// `git add -f`.
    ///
    /// Otherwise the same as [`Repository::add`].
    pub fn add_force<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.add_impl(path.as_ref(), true)
    }

    fn add_impl(&self, path: &Path, force: bool) -> Result<()> {
        let (index_lock, mut idx) = self.lock_index()?;
        let mut worktree = self.worktree()?;
        let tracked = idx.get(path).cloned();
        let tracked_mode = tracked.as_ref().map(IndexEntry::mode);

        // A tracked file that was deleted has its removal staged, as
        // `git add` does; this also resolves a conflict by deletion.
        let Some(file) = worktree.read(path, tracked.as_ref())? else {
            if idx.remove(path) {
                return index_lock.write(&idx);
            }
            return Err(Error::PathNotFound(path.to_path_buf()));
        };

        if !force && tracked_mode.is_none() && worktree.rules().is_ignored(path, false)? {
            return Err(Error::IgnoredPath(path.to_path_buf()));
        }

        // Write the blob and stage it.
        let oid = self.object_store().write(ObjectType::Blob, &file.content)?;
        idx.add(file.index_entry(path.to_path_buf(), oid));
        index_lock.write(&idx)
    }

    /// Adds all modified and untracked files to the staging area.
    ///
    /// This is equivalent to `git add -A`.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.add_all().unwrap();
    /// ```
    pub fn add_all(&self) -> Result<()> {
        let store = self.object_store();
        let (index_lock, mut idx) = self.lock_index()?;

        // Report a missing or corrupt HEAD commit before touching the index.
        self.head_tree_oid()?;

        // Get working tree files: tracked ones, and untracked ones that are
        // not ignored.
        let mut worktree = self.worktree()?;
        let working_files = worktree.scan(Some(&idx), false)?.files;

        // Add all working tree files
        for path in &working_files {
            let tracked = idx.get(path).cloned();
            let Some(file) = worktree.read(path, tracked.as_ref())? else {
                continue;
            };
            let oid = store.write(ObjectType::Blob, &file.content)?;
            idx.add(file.index_entry(path.clone(), oid));
        }

        // Handle deleted files: remove index entries (including conflict
        // stages) whose file is gone from the working tree. Skip-worktree
        // entries are absent on purpose and stay.
        let working_set: std::collections::HashSet<_> = working_files.into_iter().collect();
        let deleted: Vec<PathBuf> = idx
            .entries()
            .iter()
            .filter(|e| !e.skip_worktree() && !working_set.contains(e.path()))
            .map(|e| e.path().to_path_buf())
            .collect();
        for path in &deleted {
            idx.remove(path);
        }

        index_lock.write(&idx)?;

        Ok(())
    }

    /// Resets the staging area to match HEAD.
    ///
    /// This removes all staged changes, reverting the index to the state
    /// of the current HEAD commit.
    ///
    /// # Arguments
    ///
    /// * `paths` - Optional list of paths to reset. If `None`, resets all staged changes.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// // Reset all staged changes
    /// repo.reset(None::<&str>).unwrap();
    ///
    /// // Reset specific file
    /// repo.reset(Some("src/main.rs")).unwrap();
    /// ```
    pub fn reset<P: AsRef<Path>>(&self, path: Option<P>) -> Result<()> {
        let store = self.object_store();
        let (index_lock, mut idx) = self.lock_index()?;

        // Get HEAD tree files
        let head_tree_oid = self.head_tree_oid()?;

        let mut head_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
        if let Some(tree_oid) = head_tree_oid {
            flatten_tree(&store, &tree_oid, Path::new(""), &mut head_files)?;
        }
        let head_modes = self.tree_modes(head_tree_oid.as_ref())?;
        let mode_of = |path: &Path| {
            head_modes
                .get(&normalize_index_path(path))
                .copied()
                .unwrap_or(FileMode::Regular)
        };

        match path {
            Some(p) => {
                // Reset specific path
                let path = p.as_ref();
                let skip_worktree = idx.get(path).is_some_and(|e| e.skip_worktree());
                if let Some(head_oid) = head_files.get(path) {
                    // File exists in HEAD, restore it to index
                    let raw = store.read(head_oid)?;
                    let entry = IndexEntry::new(
                        0, // ctime (will be updated on next add)
                        0, // mtime
                        0,
                        0,
                        mode_of(path),
                        0,
                        0,
                        raw.content.len() as u32,
                        *head_oid,
                        path.to_path_buf(),
                        0,
                    )
                    // A sparse checkout entry stays outside the working tree.
                    .with_extended_flags(skip_worktree, false);
                    idx.add(entry);
                } else {
                    // File doesn't exist in HEAD, remove from index
                    idx.remove(path);
                }
            }
            None => {
                // Rebuilding would drop skip-worktree flags and end the sparse checkout.
                reject_sparse_checkout(&idx, "reset of the whole index")?;
                // Reset all: rebuild index from HEAD
                idx.clear();

                for (path, oid) in &head_files {
                    let raw = store.read(oid)?;
                    let entry = IndexEntry::new(
                        0,
                        0,
                        0,
                        0,
                        mode_of(path),
                        0,
                        0,
                        raw.content.len() as u32,
                        *oid,
                        path.clone(),
                        0,
                    );
                    idx.add(entry);
                }
            }
        }

        index_lock.write(&idx)?;

        Ok(())
    }

    /// Returns the mode of every file in a tree, keyed by `/`-separated path.
    fn tree_modes(&self, tree_oid: Option<&Oid>) -> Result<HashMap<PathBuf, FileMode>> {
        let Some(oid) = tree_oid else {
            return Ok(HashMap::new());
        };
        let tree = self.tree(&oid.to_hex())?;
        Ok(self
            .flatten_tree(&tree, PathBuf::new())?
            .into_iter()
            .map(|(path, entry)| (normalize_index_path(&path), entry.mode))
            .collect())
    }
}

/// Converts a path to the `/`-separated form used for index and tree paths.
fn normalize_index_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}
