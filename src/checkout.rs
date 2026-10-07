//! Switching branches and checking out commits.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{self, Index, IndexEntry};
use crate::infra::LockFile;
use crate::objects::tree::FileMode;
use crate::objects::ObjectType;
use crate::objects::Oid;
use crate::refs::reflog::zero_oid;
use crate::repository::{reject_sparse_checkout, Repository};

impl Repository {
    /// Checks out a branch or commit.
    ///
    /// This updates the working tree to match the target and updates HEAD.
    /// As with `git checkout`, only the paths that differ between the
    /// current HEAD and the target are changed: untracked files and changes
    /// to other paths (staged or not) are kept and carried over. Ignored
    /// files in the way are overwritten, as in Git.
    ///
    /// # Arguments
    ///
    /// * `target` - A branch name, which HEAD is attached to (also `-` or
    ///   `@{-<n>}` for an earlier branch), or any revision
    ///   [`Repository::rev_parse`] accepts (`v1.0`, `HEAD~2`, an OID, ...),
    ///   which detaches HEAD at that commit.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// - `Error::RefNotFound` if the target cannot be resolved.
    /// - `Error::LocalChangesWouldBeOverwritten` with the paths concerned if
    ///   switching would overwrite or delete a file with local changes
    ///   (staged or not), or an untracked file that is not ignored. Use
    ///   [`Repository::checkout_with`] and [`CheckoutOptions::force`] to
    ///   discard them.
    /// - `Error::UnmergedPaths` if the index has unresolved merge conflicts.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// // Checkout a branch
    /// repo.checkout("feature/new-feature").unwrap();
    ///
    /// // Checkout a specific commit (detached HEAD)
    /// repo.checkout("abc1234").unwrap();
    /// ```
    pub fn checkout(&self, target: &str) -> Result<()> {
        self.checkout_with(target, &CheckoutOptions::new())
    }

    /// Checks out a branch or commit like [`Repository::checkout`], with
    /// options. With [`CheckoutOptions::force`] (`git checkout -f`), local
    /// changes to tracked files and untracked files in the way are
    /// discarded, and conflicts in the index are dropped.
    ///
    /// # Errors
    ///
    /// The same as [`Repository::checkout`]; with `force`, local changes and
    /// conflicts are not errors.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{CheckoutOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.checkout_with("main", &CheckoutOptions::new().force(true)).unwrap();
    /// ```
    pub fn checkout_with(&self, target: &str, options: &CheckoutOptions) -> Result<()> {
        // Lock the index and HEAD before checking anything, so that a
        // concurrent Git cannot change them and no work tree file is touched
        // when either is locked. Skip-worktree entries (sparse checkout) are
        // not handled; refuse before touching the working tree.
        let (index_lock, mut idx) = self.lock_index()?;
        let mut head_lock = LockFile::acquire(self.git_dir.join("HEAD"))?;
        reject_sparse_checkout(&idx, "checkout")?;
        if idx.has_conflicts() && !options.force {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }

        let store = self.ref_store();

        // A branch name (also as `-` or `@{-<n>}` for an earlier branch)
        // attaches HEAD to the branch, as `git checkout` does; any other
        // revision detaches HEAD at its commit.
        let previous = match target {
            "-" => self.previous_checkout(1)?,
            _ => match target
                .strip_prefix("@{-")
                .and_then(|rest| rest.strip_suffix('}'))
                .and_then(|n| n.parse::<usize>().ok())
            {
                Some(n) => self.previous_checkout(n)?,
                None => None,
            },
        };
        let branch_name = previous.as_deref().unwrap_or(target);
        let branch_ref = format!("refs/heads/{}", branch_name);
        let branch = match store.resolve_recursive(&branch_ref) {
            Ok(resolved) => Some(resolved.oid),
            Err(Error::RefNotFound(_) | Error::InvalidRefName(_)) => None,
            Err(e) => return Err(e),
        };
        let (new_head_content, target_oid, target) = match branch {
            Some(oid) => (format!("ref: {}\n", branch_ref), oid, branch_name),
            None => {
                let revision = if target == "-" { "@{-1}" } else { target };
                let oid = match self.rev_parse(revision) {
                    Ok(oid) => self.peel_to(oid, ObjectType::Commit)?,
                    Err(
                        Error::InvalidRevision { .. }
                        | Error::ObjectNotFound(_)
                        | Error::InvalidOid(_),
                    ) => return Err(Error::RefNotFound(target.to_owned())),
                    Err(e) => return Err(e),
                };
                (format!("{}\n", oid.to_hex()), oid, target)
            }
        };

        // Where HEAD was, for the reflog: the branch name, or the commit.
        let old_oid = self.optional_head_oid()?;
        let from = match store.current_branch()? {
            Some(branch) => branch,
            None => old_oid.map(|oid| oid.to_hex()).unwrap_or_default(),
        };

        // Update the working tree and the index.
        self.switch_trees(&mut idx, old_oid.as_ref(), &target_oid, options.force)?;
        index_lock.write(&idx)?;

        // Update HEAD
        head_lock.write_all(new_head_content.as_bytes())?;
        self.reflog_writer()?.append(
            "HEAD",
            &old_oid.unwrap_or_else(zero_oid),
            &target_oid,
            &self.reflog_identity()?,
            &format!("checkout: moving from {} to {}", from, target),
        )?;
        head_lock.commit()
    }

    /// Moves the index and work tree from the tree of `from` to the tree of
    /// `to`, as Git's two-way merge (`read-tree -m -u`) does: paths that are
    /// the same in both commits, or whose index entry already has the
    /// target's content, are left alone with their local changes. Unless
    /// `force`, a switch that would lose local changes or overwrite an
    /// untracked file is refused before anything is changed.
    fn switch_trees(
        &self,
        idx: &mut Index,
        from: Option<&Oid>,
        to: &Oid,
        force: bool,
    ) -> Result<()> {
        use crate::merge::tree::Resolution;

        let ours = self.flat_tree(from)?;
        let target = self.flat_tree(Some(to))?;
        let mut worktree = self.worktree()?;

        // Convert line endings with the target tree's .gitattributes, as Git
        // does.
        let store = self.object_store();
        for path in ours.keys().chain(target.keys()) {
            if path == ".gitattributes" || path.ends_with("/.gitattributes") {
                let dir = path
                    .rsplit_once('/')
                    .map(|(dir, _)| dir)
                    .unwrap_or_default();
                let content = match target.get(path) {
                    Some((oid, _)) => Some(store.read(oid)?.content),
                    None => None,
                };
                worktree
                    .attributes()
                    .set_dir_file(dir.as_bytes(), content.as_deref());
            }
        }

        if force {
            // Every path that differs from the target in the index or the
            // work tree is set to the target.
            let mut paths: Vec<String> = idx
                .entries()
                .iter()
                .map(|e| String::from_utf8_lossy(&index::path_key(e.path())).into_owned())
                .chain(target.keys().cloned())
                .collect();
            paths.sort();
            paths.dedup();
            let mut changed = Vec::new();
            for path in paths {
                let native = crate::worktree::native_path(Path::new(&path));
                let entry = idx.get_stage(Path::new(&path), 0).cloned();
                let wanted = target.get(&path).copied();
                let staged = entry.as_ref().map(|e| (*e.oid(), e.mode()));
                let conflicted = idx
                    .get(Path::new(&path))
                    .is_some_and(IndexEntry::is_conflicted);
                if let Some((oid, FileMode::Submodule)) = wanted {
                    if conflicted || staged != wanted {
                        fs::create_dir_all(self.work_dir.join(&native))?;
                        idx.remove(Path::new(&path));
                        idx.add(worktree.stat_entry(
                            &native,
                            PathBuf::from(&path),
                            oid,
                            FileMode::Submodule,
                        )?);
                    }
                    continue;
                }
                let on_disk = worktree.hash(&native, entry.as_ref())?;
                if conflicted || staged != wanted || on_disk != wanted {
                    changed.push(path);
                }
            }
            self.restore_paths(idx, &mut worktree, &target, &changed)?;
            idx.clear_resolve_undo();
            return Ok(());
        }

        let mut result = crate::merge::two_way(&ours, &target);
        // An index entry that already has the target's content (or a path
        // already removed from the index) is kept as it is, with the work
        // tree file, as in Git.
        result.paths.retain(|path, resolution| match resolution {
            Resolution::Clean(entry) => {
                idx.get_stage(Path::new(path), 0)
                    .map(|e| (*e.oid(), e.mode()))
                    != *entry
            }
            Resolution::Conflict { .. } => true,
        });
        // Submodules are checked out as empty directories, as Git does.
        let submodules: Vec<(String, Oid)> = result
            .paths
            .iter()
            .filter_map(|(path, resolution)| match resolution {
                Resolution::Clean(Some((oid, FileMode::Submodule))) => Some((path.clone(), *oid)),
                _ => None,
            })
            .collect();
        self.check_overwrites(idx, &ours, &result, &mut worktree)?;
        for (path, _) in &submodules {
            result.paths.remove(path);
        }
        self.apply_merge(idx, &mut worktree, &ours, &result)?;
        for (path, oid) in submodules {
            let native = crate::worktree::native_path(Path::new(&path));
            fs::create_dir_all(self.work_dir.join(&native))?;
            idx.remove(Path::new(&path));
            idx.add(worktree.stat_entry(
                &native,
                PathBuf::from(&path),
                oid,
                FileMode::Submodule,
            )?);
        }
        Ok(())
    }
}

/// Options for [`Repository::checkout_with`].
#[derive(Debug, Clone, Default)]
pub struct CheckoutOptions {
    force: bool,
}

impl CheckoutOptions {
    /// The default options: local changes are kept, and a switch that
    /// would lose them is refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards local changes to tracked files and untracked files in the
    /// way (`git checkout -f`). Untracked files elsewhere are still kept.
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::*;
    use std::fs;
    use tempfile::TempDir;

    // W-009: checkout switches to an existing branch
    #[test]
    fn test_checkout_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a new branch
        repo.create_branch("feature", None).unwrap();

        // Checkout the new branch
        repo.checkout("feature").unwrap();

        // Verify HEAD now points to the new branch
        let head = repo.head().unwrap();
        assert!(head.is_branch());
        assert_eq!(head.branch_name(), Some("feature"));
    }

    // W-009: checkout to commit creates detached HEAD
    #[test]
    fn test_checkout_commit_detached() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, first_oid) = setup_repo_with_commit(&temp);

        // Create second commit
        fs::write(temp.path().join("file2.txt"), "Content 2").unwrap();
        repo.add("file2.txt").unwrap();
        let _second_oid = repo
            .create_commit("Second commit", "Test User", "test@example.com")
            .unwrap();

        // Checkout the first commit by full OID
        repo.checkout(&first_oid.to_hex()).unwrap();

        // Verify HEAD is detached
        let head = repo.head().unwrap();
        assert!(head.is_detached());
        assert_eq!(head.oid(), &first_oid);
    }

    // W-009: checkout updates working tree
    #[test]
    fn test_checkout_updates_working_tree() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create first commit with file1
        fs::write(temp.path().join("file1.txt"), "Content 1").unwrap();
        repo.add("file1.txt").unwrap();
        let first_oid = repo
            .create_commit("First commit", "Test User", "test@example.com")
            .unwrap();

        // Create a branch at first commit
        repo.create_branch("branch1", Some(first_oid)).unwrap();

        // Create second commit with file2
        fs::write(temp.path().join("file2.txt"), "Content 2").unwrap();
        repo.add("file2.txt").unwrap();
        let _second_oid = repo
            .create_commit("Second commit", "Test User", "test@example.com")
            .unwrap();

        // Both files exist
        assert!(temp.path().join("file1.txt").exists());
        assert!(temp.path().join("file2.txt").exists());

        // Checkout branch1 (first commit)
        repo.checkout("branch1").unwrap();

        // Only file1 should exist
        assert!(temp.path().join("file1.txt").exists());
        assert!(!temp.path().join("file2.txt").exists());

        // Checkout main (second commit)
        repo.checkout("main").unwrap();

        // Both files should exist again
        assert!(temp.path().join("file1.txt").exists());
        assert!(temp.path().join("file2.txt").exists());
    }

    // W-010: a modified file that the switch does not change is carried over
    #[test]
    fn test_checkout_dirty_working_tree() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a new branch
        repo.create_branch("feature", None).unwrap();

        // Modify a file without committing
        fs::write(temp.path().join("test.txt"), "Modified content").unwrap();

        // The branch has the same commit: the change is kept, as in Git.
        repo.checkout("feature").unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("test.txt")).unwrap(),
            "Modified content"
        );
    }

    // W-010: an untracked file not in the way is kept
    #[test]
    fn test_checkout_with_untracked_files() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a new branch
        repo.create_branch("feature", None).unwrap();

        // Create an untracked file
        fs::write(temp.path().join("untracked.txt"), "Untracked").unwrap();

        repo.checkout("feature").unwrap();
        assert!(temp.path().join("untracked.txt").exists());
    }

    // Additional: checkout nonexistent target returns RefNotFound
    #[test]
    fn test_checkout_nonexistent() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        let result = repo.checkout("nonexistent");
        assert!(matches!(result, Err(Error::RefNotFound(_))));
    }
}
