//! Creating, deleting and listing branches.

use crate::error::{Error, Result};
use crate::objects::Oid;
use crate::refs::reflog::zero_oid;
use crate::refs::{Branch, RemoteBranch};
use crate::repository::{validate_ref_name, Repository};

impl Repository {
    /// Validates a branch name according to Git rules.
    fn validate_branch_name(name: &str) -> Result<()> {
        validate_ref_name("branch", name)
    }

    /// Creates a new branch pointing to the specified commit.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the branch to create (without `refs/heads/` prefix).
    /// * `target` - The commit OID to point to. If `None`, uses current HEAD.
    ///
    /// # Returns
    ///
    /// The created `Branch` on success.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRefName` if the branch name is invalid.
    /// - `Error::RefAlreadyExists` if a branch with this name already exists.
    /// - `Error::RefNotFound` if HEAD cannot be resolved (when target is None).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// // Create a branch at current HEAD
    /// let branch = repo.create_branch("feature/new-feature", None).unwrap();
    ///
    /// // Create a branch at a specific commit
    /// use zerogit::objects::Oid;
    /// let oid = Oid::from_hex("abc1234567890abcdef1234567890abcdef12345").unwrap();
    /// let branch = repo.create_branch("hotfix", Some(oid)).unwrap();
    /// ```
    pub fn create_branch(&self, name: &str, target: Option<Oid>) -> Result<Branch> {
        // Validate branch name
        Self::validate_branch_name(name)?;

        // Get target OID
        let target_oid = match target {
            Some(oid) => oid,
            None => *self.head()?.oid(),
        };

        // Check if branch already exists (or conflicts as a directory)
        let ref_name = self.check_new_ref("heads", name)?;

        // Write the branch ref file and its reflog under the ref's lock.
        let mut lock = self.lock_new_ref(&ref_name)?;
        lock.write_all(format!("{}\n", target_oid.to_hex()).as_bytes())?;
        // Git names the start point as given, or the current branch.
        let start = match target {
            Some(oid) => oid.to_hex(),
            None => self
                .ref_store()
                .current_branch()?
                .unwrap_or_else(|| "HEAD".to_owned()),
        };
        self.reflog_writer()?.append(
            &ref_name,
            &zero_oid(),
            &target_oid,
            &self.reflog_identity()?,
            &format!("branch: Created from {}", start),
        )?;
        lock.commit()?;

        Ok(Branch::new(name, target_oid))
    }

    /// Deletes a branch (loose, packed in `packed-refs`, or both) and its
    /// reflog.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the branch to delete (without `refs/heads/` prefix).
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Errors
    ///
    /// - `Error::RefNotFound` if the branch does not exist.
    /// - `Error::CannotDeleteCurrentBranch` if trying to delete the current branch.
    /// - `Error::Locked` if the branch or `packed-refs` is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.delete_branch("feature/old-feature").unwrap();
    /// ```
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        Self::validate_branch_name(name)?;
        // Check if this is the current branch
        let store = self.ref_store();
        if store.current_branch()?.as_deref() == Some(name) {
            return Err(Error::CannotDeleteCurrentBranch);
        }

        self.delete_ref(&format!("refs/heads/{}", name), "refs/heads")
    }

    /// Lists all local branches in the repository.
    ///
    /// Returns a vector of `Branch` objects representing all branches
    /// in `refs/heads/`. The current branch (if any) is marked with
    /// `is_current() == true`.
    ///
    /// # Returns
    ///
    /// A vector of branches, sorted by name.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// for branch in repo.branches().unwrap() {
    ///     let marker = if branch.is_current() { "* " } else { "  " };
    ///     println!("{}{}", marker, branch.name());
    /// }
    /// ```
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let store = self.ref_store();
        let current_branch = store.current_branch()?;
        let mut result = Vec::new();
        for resolved in store.resolved_refs("refs/heads/")? {
            let name = resolved.name.strip_prefix("refs/heads/").unwrap();
            let branch = if current_branch.as_deref() == Some(name) {
                Branch::current(name, resolved.oid)
            } else {
                Branch::new(name, resolved.oid)
            };
            result.push(branch);
        }
        Ok(result)
    }

    /// Lists all remote-tracking branches in the repository.
    ///
    /// Returns a vector of `RemoteBranch` objects representing all branches
    /// in `refs/remotes/`.
    ///
    /// # Returns
    ///
    /// A vector of remote branches, sorted by full name (remote/branch).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// for rb in repo.remote_branches().unwrap() {
    ///     println!("{}/{}", rb.remote(), rb.name());
    /// }
    /// ```
    pub fn remote_branches(&self) -> Result<Vec<RemoteBranch>> {
        let mut result = Vec::new();
        for resolved in self.ref_store().resolved_refs("refs/remotes/")? {
            if let Some((remote, branch)) = resolved
                .name
                .strip_prefix("refs/remotes/")
                .unwrap()
                .split_once('/')
            {
                result.push(RemoteBranch::new(remote, branch, resolved.oid));
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::*;
    use std::fs;
    use tempfile::TempDir;

    // =========================================================================
    // Branch operations tests (W-006 to W-010)
    // =========================================================================

    // W-006: create_branch creates a new branch at HEAD
    #[test]
    fn test_create_branch_at_head() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, commit_oid) = setup_repo_with_commit(&temp);

        // Create a new branch
        let branch = repo.create_branch("feature", None).unwrap();

        assert_eq!(branch.name(), "feature");
        assert_eq!(branch.oid(), &commit_oid);

        // Verify the ref file was created
        let git_dir = temp.path().join(".git");
        let branch_ref = fs::read_to_string(git_dir.join("refs/heads/feature")).unwrap();
        assert_eq!(branch_ref.trim(), commit_oid.to_hex());
    }

    // W-006: create_branch creates a nested branch
    #[test]
    fn test_create_branch_nested() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, commit_oid) = setup_repo_with_commit(&temp);

        // Create a nested branch
        let branch = repo.create_branch("feature/my-feature", None).unwrap();

        assert_eq!(branch.name(), "feature/my-feature");
        assert_eq!(branch.oid(), &commit_oid);

        // Verify the ref file was created in nested directory
        let git_dir = temp.path().join(".git");
        let branch_ref = fs::read_to_string(git_dir.join("refs/heads/feature/my-feature")).unwrap();
        assert_eq!(branch_ref.trim(), commit_oid.to_hex());
    }

    // W-006: create_branch at specific commit
    #[test]
    fn test_create_branch_at_specific_commit() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, first_oid) = setup_repo_with_commit(&temp);

        // Create second commit
        fs::write(temp.path().join("file2.txt"), "Content 2").unwrap();
        repo.add("file2.txt").unwrap();
        let _second_oid = repo
            .create_commit("Second commit", "Test User", "test@example.com")
            .unwrap();

        // Create a branch at the first commit
        let branch = repo.create_branch("old-branch", Some(first_oid)).unwrap();

        assert_eq!(branch.name(), "old-branch");
        assert_eq!(branch.oid(), &first_oid);
    }

    // W-006: create_branch with invalid name returns InvalidRefName
    #[test]
    fn test_create_branch_invalid_name() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Empty name
        let result = repo.create_branch("", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));

        // Starts with -
        let result = repo.create_branch("-invalid", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));

        // Contains ..
        let result = repo.create_branch("foo..bar", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));

        // Ends with .lock
        let result = repo.create_branch("branch.lock", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));

        // Contains ~
        let result = repo.create_branch("branch~1", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));

        // Contains ^
        let result = repo.create_branch("branch^2", None);
        assert!(matches!(result, Err(Error::InvalidRefName(_))));
    }

    // W-006: create_branch with existing name returns RefAlreadyExists
    #[test]
    fn test_create_branch_already_exists() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a branch
        repo.create_branch("feature", None).unwrap();

        // Try to create same branch again
        let result = repo.create_branch("feature", None);
        assert!(matches!(result, Err(Error::RefAlreadyExists(_))));
    }

    // W-007: delete_branch deletes an existing branch
    #[test]
    fn test_delete_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create and then delete a branch
        repo.create_branch("feature", None).unwrap();

        let git_dir = temp.path().join(".git");
        assert!(git_dir.join("refs/heads/feature").exists());

        repo.delete_branch("feature").unwrap();
        assert!(!git_dir.join("refs/heads/feature").exists());
    }

    // W-007: delete_branch deletes nested branch and cleans up directories
    #[test]
    fn test_delete_branch_nested_cleanup() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a nested branch
        repo.create_branch("feature/my-feature", None).unwrap();

        let git_dir = temp.path().join(".git");
        assert!(git_dir.join("refs/heads/feature/my-feature").exists());
        assert!(git_dir.join("refs/heads/feature").is_dir());

        repo.delete_branch("feature/my-feature").unwrap();
        assert!(!git_dir.join("refs/heads/feature/my-feature").exists());
        // Directory should be cleaned up
        assert!(!git_dir.join("refs/heads/feature").exists());
    }

    // W-008: delete_branch on current branch returns CannotDeleteCurrentBranch
    #[test]
    fn test_delete_current_branch_fails() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // main is the current branch (HEAD points to it)
        let result = repo.delete_branch("main");
        assert!(matches!(result, Err(Error::CannotDeleteCurrentBranch)));
    }

    // W-008: delete_branch on nonexistent branch returns RefNotFound
    #[test]
    fn test_delete_nonexistent_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        let result = repo.delete_branch("nonexistent");
        assert!(matches!(result, Err(Error::RefNotFound(_))));
    }

    // Additional: validate_branch_name tests
    #[test]
    fn test_validate_branch_name() {
        // Valid names
        assert!(Repository::validate_branch_name("main").is_ok());
        assert!(Repository::validate_branch_name("feature/foo").is_ok());
        assert!(Repository::validate_branch_name("fix-123").is_ok());
        assert!(Repository::validate_branch_name("a/b/c").is_ok());

        // Invalid names
        assert!(Repository::validate_branch_name("").is_err());
        assert!(Repository::validate_branch_name("-start").is_err());
        assert!(Repository::validate_branch_name("/slash").is_err());
        assert!(Repository::validate_branch_name("slash/").is_err());
        assert!(Repository::validate_branch_name("foo..bar").is_err());
        assert!(Repository::validate_branch_name("foo.lock").is_err());
        assert!(Repository::validate_branch_name("foo~bar").is_err());
        assert!(Repository::validate_branch_name("foo^bar").is_err());
        assert!(Repository::validate_branch_name("foo:bar").is_err());
        assert!(Repository::validate_branch_name("foo?bar").is_err());
        assert!(Repository::validate_branch_name("foo*bar").is_err());
        assert!(Repository::validate_branch_name("foo[bar").is_err());
        assert!(Repository::validate_branch_name("foo\\bar").is_err());
    }
}
