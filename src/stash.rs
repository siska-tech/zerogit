//! Stashing work tree and index changes (`git stash`).
//!
//! Stashes are stored the way Git stores them, so either tool can use the
//! other's stashes: a stash is a commit `W` holding the work tree state of
//! the tracked files, whose parents are the commit it was made on, a commit
//! `I` holding the index state, and (when untracked files are included) a
//! parentless commit `U` holding them. `refs/stash` points to the newest
//! stash and its reflog lists them all (`stash@{0}`, `stash@{1}`, ...).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::merge::file::Labels;
use crate::merge::index_flat;
use crate::merge::tree::{merge_trees, Flat, Resolution};
use crate::objects::{ObjectType, Oid, Signature};
use crate::refs::reflog::zero_oid;
use crate::repository::Repository;
use crate::worktree::native_path;

/// Options for [`Repository::stash_save`].
#[derive(Debug, Clone, Default)]
pub struct StashOptions {
    message: Option<String>,
    include_untracked: bool,
}

impl StashOptions {
    /// The default options: tracked files only, Git's `WIP on ...` message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the stash message (`git stash push -m`); the stash is then
    /// described as `On <branch>: <message>`.
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Also stashes untracked files that are not ignored, and removes them
    /// (`git stash push --include-untracked`).
    pub fn include_untracked(mut self, include: bool) -> Self {
        self.include_untracked = include;
        self
    }
}

/// One stash, as listed by [`Repository::stash_list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    index: usize,
    oid: Oid,
    message: String,
}

impl StashEntry {
    /// The position in the list: `n` for `stash@{n}` (0 is the newest).
    pub fn index(&self) -> usize {
        self.index
    }

    /// The stash commit.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// The description, such as `WIP on main: 1234567 Subject`.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// The result of applying a stash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StashApplyOutcome {
    /// The changes were applied without conflicts.
    Applied,
    /// The changes conflicted at these paths; they are recorded in the
    /// index (stages 1-3) and the work tree has conflict markers. A popped
    /// stash is kept.
    Conflicts(Vec<PathBuf>),
}

/// Builds a tree object from a flattened tree.
pub(crate) fn write_tree(repo: &Repository, flat: &Flat) -> Result<Oid> {
    let mut idx = Index::empty(2);
    for (path, (oid, mode)) in flat {
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
    repo.build_tree_from_index(&idx)
}

fn write_commit(
    repo: &Repository,
    tree: &Oid,
    parents: &[Oid],
    who: &Signature,
    message: &str,
) -> Result<Oid> {
    let signature = who.to_git_string();
    let content = Repository::format_commit(tree, parents, &signature, &signature, message);
    repo.object_store().write(ObjectType::Commit, &content)
}

/// The flattened tree of a tree object.
pub(crate) fn flat_of_tree(repo: &Repository, tree: &Oid) -> Result<Flat> {
    let mut by_path = std::collections::BTreeMap::new();
    crate::status::flatten_with_modes(&repo.object_store(), tree, "", &mut by_path)?;
    Ok(by_path
        .into_iter()
        .map(|(path, entry)| (path.to_string_lossy().into_owned(), entry))
        .collect())
}

impl Repository {
    /// Saves the changes of the work tree and the index as a new stash and
    /// resets them to HEAD, like `git stash push`.
    ///
    /// The stash records the index (staged changes) and the work tree
    /// state of the tracked files; with
    /// [`StashOptions::include_untracked`], untracked files that are not
    /// ignored are recorded and removed too. The stash commits are written
    /// as Git writes them (`WIP on <branch>: <short id> <subject>`, or
    /// `On <branch>: <message>`), authored by the given name and email,
    /// and listed by `git stash list`.
    ///
    /// # Returns
    ///
    /// The stash commit, or `None` if there was nothing to stash (nothing is
    /// changed then).
    ///
    /// # Errors
    ///
    /// - `Error::UnmergedPaths` if the index has unresolved conflicts.
    /// - `Error::RefNotFound` if HEAD has no commit yet.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{Repository, StashOptions};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.stash_save("John Doe", "john@example.com", &StashOptions::new())
    ///     .unwrap();
    /// ```
    pub fn stash_save(
        &self,
        name: &str,
        email: &str,
        options: &StashOptions,
    ) -> Result<Option<Oid>> {
        let mut idx = self.read_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let head_commit = self.commit(&head.to_hex())?;
        let head_flat = self.flat_tree(Some(&head))?;
        let mut worktree = self.worktree()?;

        // The index state, and the work tree state of every tracked path.
        let index_state = index_flat(&idx);
        let mut work_state = Flat::new();
        for entry in idx.entries() {
            let key = String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned();
            if entry.skip_worktree() {
                work_state.insert(key, (*entry.oid(), entry.mode()));
                continue;
            }
            if let Some(file) = worktree.read(&native_path(entry.path()), Some(entry))? {
                let oid = file.oid();
                if entry.intent_to_add() || (oid, file.mode) != (*entry.oid(), entry.mode()) {
                    self.object_store().write(ObjectType::Blob, &file.content)?;
                }
                work_state.insert(key, (oid, file.mode));
            }
        }
        let untracked: Vec<PathBuf> = if options.include_untracked {
            let scan = worktree.scan(Some(&idx), false)?;
            scan.files
                .into_iter()
                .filter(|p| idx.get(p).is_none())
                .collect()
        } else {
            Vec::new()
        };
        if index_state == head_flat && work_state == head_flat && untracked.is_empty() {
            return Ok(None);
        }

        let branch = self
            .ref_store()
            .current_branch()?
            .unwrap_or_else(|| "(no branch)".to_owned());
        let description = format!(
            "{}: {} {}",
            branch,
            &head.to_hex()[..7],
            head_commit.subject()
        );
        let who = Signature::new(name, email, crate::repository::now(), 0);

        let index_tree = write_tree(self, &index_state)?;
        let index_commit = write_commit(
            self,
            &index_tree,
            &[head],
            &who,
            &format!("index on {}\n", description),
        )?;
        let mut parents = vec![head, index_commit];
        if !untracked.is_empty() {
            let mut files = Flat::new();
            for path in &untracked {
                if let Some(file) = worktree.read(path, None)? {
                    let oid = self.object_store().write(ObjectType::Blob, &file.content)?;
                    files.insert(path.to_string_lossy().replace('\\', "/"), (oid, file.mode));
                }
            }
            let tree = write_tree(self, &files)?;
            parents.push(write_commit(
                self,
                &tree,
                &[],
                &who,
                &format!("untracked files on {}\n", description),
            )?);
        }
        let message = match &options.message {
            Some(message) => format!("On {}: {}", branch, message),
            None => format!("WIP on {}", description),
        };
        let work_tree = write_tree(self, &work_state)?;
        let stash = write_commit(self, &work_tree, &parents, &who, &format!("{}\n", message))?;

        // Record it in refs/stash and its reflog.
        let previous = self
            .stash_list()?
            .first()
            .map(|e| e.oid)
            .unwrap_or_else(zero_oid);
        crate::infra::write_file_atomic(
            self.git_dir().join("refs").join("stash"),
            format!("{}\n", stash.to_hex()).as_bytes(),
        )?;
        self.reflog_writer()?
            .append("refs/stash", &previous, &stash, &who, &message)?;

        // Reset the index and the tracked files to HEAD, and remove the
        // stashed untracked files.
        let mut changed: Vec<String> = Vec::new();
        for path in index_state
            .keys()
            .chain(work_state.keys())
            .chain(head_flat.keys())
        {
            let head_entry = head_flat.get(path);
            if (index_state.get(path) != head_entry || work_state.get(path) != head_entry)
                && !changed.contains(path)
            {
                changed.push(path.clone());
            }
        }
        // Intent-to-add entries are not in the index state; drop them too.
        for entry in idx.entries() {
            let key = String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned();
            if entry.intent_to_add() && !changed.contains(&key) {
                changed.push(key);
            }
        }
        self.restore_paths(&mut idx, &mut worktree, &head_flat, &changed)?;
        for path in &untracked {
            worktree.remove(path)?;
        }
        self.write_index(&idx)?;
        Ok(Some(stash))
    }

    /// Lists the stashes, newest first (`git stash list`).
    pub fn stash_list(&self) -> Result<Vec<StashEntry>> {
        Ok(self
            .reflog("refs/stash")?
            .into_iter()
            .enumerate()
            .map(|(index, entry)| StashEntry {
                index,
                oid: *entry.new_oid(),
                message: entry.message().to_owned(),
            })
            .collect())
    }

    fn stash_at(&self, index: usize) -> Result<StashEntry> {
        self.stash_list()?
            .into_iter()
            .nth(index)
            .ok_or_else(|| Error::RefNotFound(format!("stash@{{{}}}", index)))
    }

    /// Applies `stash@{index}` to the work tree, like `git stash apply`.
    ///
    /// The stashed changes are merged into the current state with the
    /// stash's base as the common ancestor. Without `restore_index`, changes
    /// end up unstaged, except files the stash added, which stay staged;
    /// with `restore_index` (`--index`), the staged state is restored too.
    /// Stashed untracked files are restored as untracked files. Conflicts
    /// are recorded in the index and the work tree as in a merge
    /// (`Updated upstream` / `Stashed changes`).
    ///
    /// # Errors
    ///
    /// Nothing is changed when any of these is returned:
    /// - `Error::RefNotFound` if there is no such stash.
    /// - `Error::MergeInProgress` / `Error::UnmergedPaths` during a merge
    ///   or with unresolved conflicts.
    /// - `Error::LocalChangesWouldBeOverwritten` if applying would change
    ///   files with local changes, or a stashed untracked file exists.
    /// - `Error::UnsupportedMerge` if `restore_index` is set and the staged
    ///   changes conflict.
    pub fn stash_apply(&self, index: usize, restore_index: bool) -> Result<StashApplyOutcome> {
        if !self.merge_heads()?.is_empty() {
            return Err(Error::MergeInProgress);
        }
        let mut idx = self.read_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let stash = self.stash_at(index)?;
        let commit = self.commit(&stash.oid.to_hex())?;
        let parents = commit.parents().to_vec();
        if parents.len() < 2 {
            return Err(Error::InvalidObject {
                oid: stash.oid.to_hex(),
                reason: "not a stash commit".to_owned(),
            });
        }
        let base = self.flat_tree(Some(&parents[0]))?;
        let stashed_index = self.flat_tree(Some(&parents[1]))?;
        let stashed_work = flat_of_tree(self, commit.tree())?;
        let untracked = match parents.get(2) {
            Some(oid) => self.flat_tree(Some(oid))?,
            None => Flat::new(),
        };

        let mut worktree = self.worktree()?;
        let ours = index_flat(&idx);
        let blocked: Vec<PathBuf> = untracked
            .keys()
            .map(|p| native_path(Path::new(p)))
            .filter(|p| matches!(worktree.metadata(p), Ok(Some(_))))
            .collect();
        if !blocked.is_empty() {
            return Err(Error::LocalChangesWouldBeOverwritten(blocked));
        }

        let style = self.conflict_style()?;
        let labels = Labels {
            ours: "Updated upstream",
            base: "Stash base",
            theirs: "Stashed changes",
        };
        let store = self.object_store();
        let index_result = if restore_index {
            let merged = merge_trees(&store, &base, &ours, &stashed_index, labels, style)?;
            if !merged.conflicted_paths().is_empty() {
                return Err(Error::UnsupportedMerge(
                    "the stashed index conflicts with the current index".to_owned(),
                ));
            }
            Some(merged.as_flat())
        } else {
            None
        };
        let result = merge_trees(&store, &base, &ours, &stashed_work, labels, style)?;
        self.check_overwrites(&idx, &ours, &result, &mut worktree)?;
        self.apply_merge(&mut idx, &mut worktree, &ours, &result)?;

        let conflicts = result.conflicted_paths();
        if conflicts.is_empty() {
            // Restore the index: the stashed index state, or the previous
            // index with files the stash added kept as added.
            for (path, resolution) in &result.paths {
                let Resolution::Clean(entry) = resolution else {
                    continue;
                };
                let wanted = match &index_result {
                    Some(staged) => staged.get(path).copied(),
                    None if entry.is_some()
                        && !base.contains_key(path)
                        && !ours.contains_key(path) =>
                    {
                        *entry
                    }
                    None => ours.get(path).copied(),
                };
                let index_path = PathBuf::from(path);
                if idx.get_stage(&index_path, 0).map(|e| (*e.oid(), e.mode())) == wanted {
                    continue;
                }
                idx.remove(&index_path);
                if let Some((oid, mode)) = wanted {
                    idx.add(IndexEntry::new(
                        0, 0, 0, 0, mode, 0, 0, 0, oid, index_path, 0,
                    ));
                }
            }
            if let Some(staged) = &index_result {
                // Staged changes on paths the work tree merge left alone.
                for (path, entry) in staged {
                    if ours.get(path) != Some(entry) && !result.paths.contains_key(path) {
                        let index_path = PathBuf::from(path);
                        idx.remove(&index_path);
                        idx.add(IndexEntry::new(
                            0, 0, 0, 0, entry.1, 0, 0, 0, entry.0, index_path, 0,
                        ));
                    }
                }
            }
        }
        for (path, (oid, mode)) in &untracked {
            worktree.write(
                &native_path(Path::new(path)),
                &store.read(oid)?.content,
                *mode,
            )?;
        }
        self.write_index(&idx)?;
        Ok(if conflicts.is_empty() {
            StashApplyOutcome::Applied
        } else {
            StashApplyOutcome::Conflicts(conflicts.into_iter().map(PathBuf::from).collect())
        })
    }

    /// Applies `stash@{index}` and drops it if it applied without
    /// conflicts, like `git stash pop`. See [`Repository::stash_apply`].
    pub fn stash_pop(&self, index: usize, restore_index: bool) -> Result<StashApplyOutcome> {
        let outcome = self.stash_apply(index, restore_index)?;
        if outcome == StashApplyOutcome::Applied {
            self.stash_drop(index)?;
        }
        Ok(outcome)
    }

    /// Removes `stash@{index}` from the list, like `git stash drop`.
    /// Dropping the last stash deletes `refs/stash`.
    ///
    /// # Errors
    ///
    /// `Error::RefNotFound` if there is no such stash.
    pub fn stash_drop(&self, index: usize) -> Result<()> {
        let reflog = self.reflog_writer()?;
        let mut entries = self.reflog("refs/stash")?;
        if index >= entries.len() {
            return Err(Error::RefNotFound(format!("stash@{{{}}}", index)));
        }
        entries.remove(index);
        // Keep the old values chained, as `git reflog delete --rewrite` does.
        if index > 0 {
            let older = entries
                .get(index)
                .map(|e| *e.new_oid())
                .unwrap_or_else(zero_oid);
            entries[index - 1].set_old_oid(older);
        }
        let ref_path = self.git_dir().join("refs").join("stash");
        reflog.rewrite("refs/stash", &entries)?;
        match entries.first() {
            Some(newest) => crate::infra::write_file_atomic(
                &ref_path,
                format!("{}\n", newest.new_oid().to_hex()).as_bytes(),
            ),
            None => match std::fs::remove_file(&ref_path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }
}
