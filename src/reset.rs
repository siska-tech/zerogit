//! `git reset`: moving HEAD to a commit with the index and work tree
//! kept (soft), matched in the index only (mixed) or matched everywhere
//! (hard), and resetting index paths to a commit.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{path_key, Index, IndexEntry};
use crate::objects::{ObjectType, Oid};
use crate::repository::{reject_sparse_checkout, Repository};

/// What [`Repository::reset_to`] changes besides HEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// `--soft`: only HEAD (or its branch) moves; the index and the work
    /// tree are kept, so the changes since the commit appear staged.
    Soft,
    /// `--mixed` (Git's default): the index is set to the commit's tree as
    /// well; the work tree is kept, so changes appear unstaged.
    Mixed,
    /// `--hard`: the index and the tracked files are set to the commit;
    /// changes to tracked files are discarded and untracked files are kept.
    Hard,
}

impl Repository {
    /// Moves HEAD (the current branch, or HEAD itself when detached) to a
    /// commit, like `git reset --soft`, `--mixed` or `--hard <revision>`,
    /// and returns the commit.
    ///
    /// `revision` is anything [`Repository::rev_parse`] accepts (`HEAD~1`,
    /// `origin/main`, ...); a tag is peeled to its commit. As in Git, the
    /// previous HEAD is saved as `ORIG_HEAD`, the reflogs of HEAD and the
    /// branch record `reset: moving to <revision>`, and a mixed or hard
    /// reset ends a merge in progress (`MERGE_HEAD` and its files are
    /// removed; resolved or not, the conflicts are replaced by the commit's
    /// content).
    ///
    /// A mixed reset keeps the stat data of index entries whose content is
    /// unchanged and refreshes the others from the work tree, as Git does,
    /// so `status` stays fast.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// - `Error::InvalidRevision` if the revision cannot be resolved, or
    ///   `Error::TypeMismatch` if it is not a commit.
    /// - `Error::MergeInProgress` or `Error::UnmergedPaths` for a soft reset
    ///   during a merge or with conflicts in the index, which Git refuses.
    /// - `Error::Locked` if HEAD, the branch or the index is locked.
    /// - `Error::UnsupportedIndex` for a mixed or hard reset in a sparse
    ///   checkout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{Repository, ResetMode};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Undo the last commit, keeping its changes staged.
    /// repo.reset_to("HEAD~1", ResetMode::Soft).unwrap();
    /// ```
    pub fn reset_to(&self, revision: &str, mode: ResetMode) -> Result<Oid> {
        let target = self.peel_to(self.rev_parse(revision)?, ObjectType::Commit)?;
        // Lock HEAD before touching anything, so a locked HEAD changes
        // nothing.
        let head = self.lock_head()?;
        match mode {
            ResetMode::Soft => {
                if !self.merge_heads()?.is_empty() {
                    return Err(Error::MergeInProgress);
                }
                let idx = self.read_index()?;
                if idx.has_conflicts() {
                    return Err(Error::UnmergedPaths(idx.conflicted_paths()));
                }
            }
            ResetMode::Mixed => self.reset_index_to(&target)?,
            ResetMode::Hard => self.reset_hard_to(&self.flat_tree(Some(&target))?)?,
        }
        if let Some(old) = head.old {
            self.write_orig_head(&old)?;
        }
        let who = self.reflog_identity()?;
        head.update(
            self,
            &target,
            &who,
            &format!("reset: moving to {}", revision),
        )?;
        if mode != ResetMode::Soft {
            self.clear_merge_state()?;
        }
        Ok(target)
    }

    /// Sets index entries to their content in a commit, like
    /// `git reset <revision> -- <paths>`: each path (a file, or a directory
    /// for everything under it) is staged as it is in the commit, or
    /// removed from the index when the commit does not have it. HEAD and the
    /// work tree are not changed. An empty path or `.` means every path.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRevision` if the revision cannot be resolved, or
    ///   `Error::TypeMismatch` if it is not a commit.
    /// - `Error::Locked` if the index is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Stage src/ as it was two commits ago.
    /// repo.reset_paths("HEAD~2", &["src"]).unwrap();
    /// ```
    pub fn reset_paths<P: AsRef<Path>>(&self, revision: &str, paths: &[P]) -> Result<()> {
        let commit = self.peel_to(self.rev_parse(revision)?, ObjectType::Commit)?;
        let target = self.flat_tree(Some(&commit))?;
        let (lock, mut idx) = self.lock_index()?;
        let prefixes: Vec<String> = paths
            .iter()
            .map(|p| {
                let key = String::from_utf8_lossy(&path_key(p.as_ref())).into_owned();
                key.trim_end_matches('/')
                    .trim_start_matches("./")
                    .to_owned()
            })
            .collect();
        let matches = |path: &str| {
            prefixes.iter().any(|prefix| {
                prefix.is_empty()
                    || prefix == "."
                    || path == prefix
                    || (path.starts_with(prefix.as_str())
                        && path.as_bytes().get(prefix.len()) == Some(&b'/'))
            })
        };

        // Entries the commit does not have leave the index.
        let mut gone: Vec<PathBuf> = idx
            .entries()
            .iter()
            .map(|e| String::from_utf8_lossy(&path_key(e.path())).into_owned())
            .filter(|path| matches(path) && !target.contains_key(path))
            .map(PathBuf::from)
            .collect();
        gone.dedup();
        for path in gone {
            idx.remove(&path);
        }
        // The others get the commit's content; unchanged entries keep their
        // stat data.
        for (path, (oid, mode)) in target.iter().filter(|(path, _)| matches(path)) {
            let index_path = Path::new(path);
            let unchanged = idx
                .get_stage(index_path, 0)
                .is_some_and(|e| e.oid() == oid && e.mode() == *mode && !e.intent_to_add())
                && !idx.get(index_path).is_some_and(IndexEntry::is_conflicted);
            if !unchanged {
                idx.remove(index_path);
                idx.add(unstated_entry(path, *oid, *mode));
            }
        }
        let mut worktree = self.worktree()?;
        self.refresh_entries(&mut idx, &mut worktree)?;
        lock.write(&idx)
    }

    /// Sets the index to a commit's tree (the index part of a mixed reset).
    fn reset_index_to(&self, commit: &Oid) -> Result<()> {
        let (lock, idx) = self.lock_index()?;
        // Rebuilding would drop skip-worktree flags and end the sparse
        // checkout.
        reject_sparse_checkout(&idx, "mixed reset")?;
        let target = self.flat_tree(Some(commit))?;
        let mut new = Index::empty(idx.version());
        for (path, (oid, mode)) in &target {
            match idx.get_stage(Path::new(path), 0) {
                Some(e) if e.oid() == oid && e.mode() == *mode && !e.intent_to_add() => {
                    new.add(e.clone())
                }
                _ => new.add(unstated_entry(path, *oid, *mode)),
            }
        }
        let mut worktree = self.worktree()?;
        self.refresh_entries(&mut new, &mut worktree)?;
        lock.write(&new)
    }
}

/// An index entry for a blob from a tree, without stat data (so the next
/// comparison reads the file).
fn unstated_entry(path: &str, oid: Oid, mode: crate::objects::FileMode) -> IndexEntry {
    IndexEntry::new(0, 0, 0, 0, mode, 0, 0, 0, oid, PathBuf::from(path), 0)
}
