//! Git repository operations.

// This module holds the `Repository` structure (opening, discovering and
// creating repositories) and what the other modules share: the object and
// reference stores, HEAD, the index and their locks, and the validation of
// reference names. Each feature adds its operations in its own module with
// an `impl Repository` block (for example `checkout`, `commit`, `log`,
// `staging`, `status` and `refs`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{self, Index, IndexEntry};
use crate::infra::{read_file, LockFile};
use crate::objects::{ObjectStore, Oid, Signature};
use crate::refs::reflog::{zero_oid, Reflog};
use crate::refs::{Head, RefStore};
use crate::worktree::Worktree;

pub use crate::checkout::CheckoutOptions;

/// A Git repository.
///
/// This is the main entry point for interacting with a Git repository.
/// It provides access to objects, references, and the index.
///
/// Objects are read from loose files and pack files. Pack indexes are parsed
/// on first use and reused for the lifetime of the `Repository` (and of the
/// log iterators created from it); packs added or replaced by an external
/// `git repack` or `git gc` are picked up when a lookup misses.
#[derive(Debug)]
pub struct Repository {
    /// The root directory of the working tree.
    pub(crate) work_dir: PathBuf,
    /// The path to the `.git` directory.
    pub(crate) git_dir: PathBuf,
    /// Loose and packed objects, shared with iterators.
    objects: ObjectStore,
}

impl Repository {
    fn from_dirs(work_dir: PathBuf, git_dir: PathBuf) -> Result<Self> {
        Self::check_repository_format(&git_dir)?;
        let objects = ObjectStore::new(git_dir.join("objects"));
        Ok(Repository {
            work_dir,
            git_dir,
            objects,
        })
    }

    /// Rejects repository formats this library cannot read correctly.
    ///
    /// Only SHA-1 objects and the files reference backend are supported, so a
    /// SHA-256 or reftable repository fails here rather than misreading data.
    fn check_repository_format(git_dir: &Path) -> Result<()> {
        let path = git_dir.join("config");
        if !path.is_file() {
            return Ok(());
        }
        let config = crate::config::Config::from_file(path)?;
        let unsupported = |what: String| Err(Error::UnsupportedRepositoryFormat(what));
        if let Some(version) = config.get("core", "repositoryformatversion") {
            if !matches!(version.trim(), "0" | "1") {
                return unsupported(format!("repositoryformatversion {}", version));
            }
        }
        if let Some(format) = config.get("extensions", "objectformat") {
            if !format.eq_ignore_ascii_case("sha1") {
                return unsupported(format!("objectFormat {}", format));
            }
        }
        if let Some(storage) = config.get("extensions", "refstorage") {
            if !storage.eq_ignore_ascii_case("files") {
                return unsupported(format!("refStorage {}", storage));
            }
        }
        Ok(())
    }

    /// Validates that a directory is a valid Git directory.
    ///
    /// A valid `.git` directory must contain at least:
    /// - `HEAD` file
    /// - `objects/` directory
    /// - `refs/` directory
    fn validate_git_dir(git_dir: &Path) -> Result<()> {
        // Check that .git directory exists and is a directory
        if !git_dir.is_dir() {
            return Err(Error::NotARepository(git_dir.to_path_buf()));
        }

        // Check for HEAD file
        let head_path = git_dir.join("HEAD");
        if !head_path.is_file() {
            return Err(Error::NotARepository(git_dir.to_path_buf()));
        }

        // Check for objects directory
        let objects_path = git_dir.join("objects");
        if !objects_path.is_dir() {
            return Err(Error::NotARepository(git_dir.to_path_buf()));
        }

        // Check for refs directory
        let refs_path = git_dir.join("refs");
        if !refs_path.is_dir() {
            return Err(Error::NotARepository(git_dir.to_path_buf()));
        }

        Ok(())
    }

    /// Opens an existing Git repository.
    ///
    /// The path can point to either:
    /// - The repository root (containing `.git/`)
    /// - The `.git` directory itself
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the repository root or `.git` directory.
    ///
    /// # Returns
    ///
    /// A `Repository` instance, or an error if the path is not a valid Git repository.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// // Open by repository root
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// // Open by .git directory
    /// let repo = Repository::open("path/to/repo/.git").unwrap();
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Canonicalize the path to resolve any symlinks and get absolute path
        let abs_path = path
            .canonicalize()
            .map_err(|_| Error::NotARepository(path.to_path_buf()))?;

        // A bare repository: the directory itself is the Git directory.
        if !abs_path.join(".git").exists() && Self::validate_git_dir(&abs_path).is_ok() {
            let bare = crate::config::Config::from_file(abs_path.join("config"))
                .map(|c| c.get_bool_or("core", "bare", false))
                .unwrap_or(false);
            if bare || !abs_path.ends_with(".git") {
                return Self::from_dirs(abs_path.clone(), abs_path);
            }
        }

        // Determine if we're given the .git directory or the work tree
        let (work_dir, git_dir) = if abs_path.ends_with(".git") {
            // Given the .git directory directly
            let git_dir = abs_path.clone();
            let work_dir = abs_path
                .parent()
                .ok_or_else(|| Error::NotARepository(path.to_path_buf()))?
                .to_path_buf();
            (work_dir, git_dir)
        } else {
            // Given the work tree, .git should be a subdirectory
            let git_dir = abs_path.join(".git");
            (abs_path, git_dir)
        };

        // Validate that it's a proper git directory
        Self::validate_git_dir(&git_dir)?;

        Self::from_dirs(work_dir, git_dir)
    }

    /// Discovers a Git repository by searching upward from the given path.
    ///
    /// Starting from `path`, this function walks up the directory tree
    /// looking for a `.git` directory until it finds one or reaches the
    /// filesystem root.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to start searching from.
    ///
    /// # Returns
    ///
    /// A `Repository` instance, or an error if no repository is found.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// // Discover repository from a subdirectory
    /// let repo = Repository::discover("path/to/repo/src/lib").unwrap();
    /// ```
    pub fn discover<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Canonicalize the starting path
        let mut current = path
            .canonicalize()
            .map_err(|_| Error::NotARepository(path.to_path_buf()))?;

        loop {
            let git_dir = current.join(".git");

            // Check if .git exists and is valid
            if git_dir.is_dir() && Self::validate_git_dir(&git_dir).is_ok() {
                return Self::from_dirs(current, git_dir);
            }

            // Move to parent directory
            match current.parent() {
                Some(parent) => {
                    current = parent.to_path_buf();
                }
                None => {
                    // Reached filesystem root without finding a repository
                    return Err(Error::NotARepository(path.to_path_buf()));
                }
            }
        }
    }

    /// Initializes a new Git repository at the given path.
    ///
    /// Creates a new repository with the standard `.git` directory structure,
    /// including the necessary subdirectories and files for a functional Git repository.
    ///
    /// # Arguments
    ///
    /// * `path` - Path where the repository should be created.
    ///
    /// # Returns
    ///
    /// A `Repository` instance pointing to the newly created repository.
    ///
    /// # Errors
    ///
    /// - `Error::AlreadyARepository` if a `.git` directory already exists at the path.
    /// - `Error::Io` if directory or file creation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::init("path/to/new/repo").unwrap();
    /// println!("Created repository at: {}", repo.path().display());
    /// ```
    pub fn init<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::init_impl(path.as_ref(), false)
    }

    /// Initializes a new bare Git repository at the given path.
    ///
    /// Creates a bare repository (without a working directory) at the specified path.
    /// Bare repositories are typically used as remote repositories and contain
    /// the Git database directly in the specified directory rather than in a `.git` subdirectory.
    ///
    /// # Arguments
    ///
    /// * `path` - Path where the bare repository should be created.
    ///
    /// # Returns
    ///
    /// A `Repository` instance pointing to the newly created bare repository.
    ///
    /// # Errors
    ///
    /// - `Error::AlreadyARepository` if a Git repository already exists at the path.
    /// - `Error::Io` if directory or file creation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::init_bare("path/to/bare.git").unwrap();
    /// println!("Created bare repository at: {}", repo.path().display());
    /// ```
    pub fn init_bare<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::init_impl(path.as_ref(), true)
    }

    /// Internal implementation for repository initialization.
    fn init_impl(path: &Path, bare: bool) -> Result<Self> {
        // Create the base directory if it doesn't exist
        fs::create_dir_all(path)?;

        // Determine git_dir and work_dir based on bare flag
        let (work_dir, git_dir) = if bare {
            // For bare repositories, git_dir is the path itself
            let abs_path = path.canonicalize()?;
            (abs_path.clone(), abs_path)
        } else {
            // For normal repositories, git_dir is path/.git
            let abs_path = path.canonicalize()?;
            let git_dir = abs_path.join(".git");
            (abs_path, git_dir)
        };

        // Check if repository already exists
        if git_dir.join("HEAD").exists() {
            return Err(Error::AlreadyARepository(path.to_path_buf()));
        }

        // Create the .git directory for non-bare repositories
        if !bare {
            fs::create_dir_all(&git_dir)?;
        }

        // Create required subdirectories
        fs::create_dir_all(git_dir.join("objects"))?;
        fs::create_dir_all(git_dir.join("refs/heads"))?;
        fs::create_dir_all(git_dir.join("refs/tags"))?;

        // Create HEAD file pointing to main branch
        let head_content = "ref: refs/heads/main\n";
        fs::write(git_dir.join("HEAD"), head_content)?;

        // Create minimal config file
        let config_content = if bare {
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = true\n"
        } else {
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n"
        };
        fs::write(git_dir.join("config"), config_content)?;

        Self::from_dirs(work_dir, git_dir)
    }

    /// Returns whether the repository is bare (has no work tree).
    pub fn is_bare(&self) -> bool {
        self.work_dir == self.git_dir
    }

    /// Returns the path to the repository root (working directory).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// println!("Repository root: {}", repo.path().display());
    /// ```
    pub fn path(&self) -> &Path {
        &self.work_dir
    }

    /// Returns the path to the `.git` directory.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// println!(".git directory: {}", repo.git_dir().display());
    /// ```
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Returns the repository configuration.
    ///
    /// This loads configuration from all levels (system, global, local) with
    /// proper precedence (local overrides global, global overrides system).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let config = repo.config().unwrap();
    ///
    /// if let Some(name) = config.get("user", "name") {
    ///     println!("User: {}", name);
    /// }
    /// ```
    pub fn config(&self) -> Result<crate::config::Config> {
        crate::config::load_config(&self.git_dir)
    }

    /// Returns only the repository-local configuration.
    ///
    /// This loads only the `.git/config` file, ignoring global and system configs.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let config = repo.config_local().unwrap();
    /// ```
    pub fn config_local(&self) -> Result<crate::config::Config> {
        crate::config::Config::from_file(self.git_dir.join("config"))
    }

    /// Returns the object store shared by this repository and its iterators.
    pub(crate) fn object_store(&self) -> ObjectStore {
        self.objects.clone()
    }

    /// Resolves HEAD while allowing a valid symbolic reference to an unborn branch.
    pub(crate) fn optional_head_oid(&self) -> Result<Option<Oid>> {
        match self.head() {
            Ok(head) => Ok(Some(*head.oid())),
            Err(Error::RefNotFound(_)) => {
                // A missing HEAD file itself is not an unborn branch.
                self.ref_store().read_ref_file("HEAD")?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn head_tree_oid(&self) -> Result<Option<Oid>> {
        self.optional_head_oid()?
            .map(|oid| self.commit(&oid.to_hex()).map(|commit| *commit.tree()))
            .transpose()
    }

    /// Returns a reference to the ref store.
    pub(crate) fn ref_store(&self) -> RefStore {
        RefStore::new(&self.git_dir)
    }

    /// Returns the current HEAD state.
    ///
    /// # Returns
    ///
    /// A `Head` enum representing the current HEAD state (branch or detached).
    ///
    /// # Errors
    ///
    /// - `Error::RefNotFound` if HEAD doesn't exist or points to an unborn branch.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let head = repo.head().unwrap();
    /// if head.is_detached() {
    ///     println!("HEAD is detached at {}", head.oid().short());
    /// } else {
    ///     println!("On branch {}", head.branch_name().unwrap());
    /// }
    /// ```
    pub fn head(&self) -> Result<Head> {
        let store = self.ref_store();

        // Check if HEAD is symbolic or direct
        match store.read_ref_file("HEAD")? {
            crate::refs::RefValue::Symbolic(target) => {
                // HEAD points to a branch
                let resolved = store.resolve_recursive(&target)?;
                let branch_name = target
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&target)
                    .to_string();
                Ok(Head::branch(branch_name, resolved.oid))
            }
            crate::refs::RefValue::Direct(oid) => {
                // HEAD is detached
                Ok(Head::detached(oid))
            }
        }
    }

    /// Loads the working tree: its ignore rules (`.gitignore` files,
    /// `.git/info/exclude`, `core.excludesFile`) and checkout settings.
    pub(crate) fn worktree(&self) -> Result<Worktree> {
        Worktree::load(
            &self.work_dir,
            &self.git_dir,
            &self.config()?,
            self.object_store(),
        )
    }

    /// Reads the current index, or creates an empty one if it doesn't exist.
    pub(crate) fn read_index(&self) -> Result<Index> {
        let index_path = self.git_dir.join("index");
        if index_path.exists() {
            let index_data = read_file(&index_path)?;
            index::parse(&index_data)
        } else {
            Ok(Index::empty(2))
        }
    }

    /// Locks the index (`index.lock`, as Git does) and then reads it, for an
    /// operation that changes it. Holding the lock from the read to the
    /// write keeps a concurrent Git (or zerogit) from changing the index in
    /// between; the change is saved with [`IndexLock::write`], and dropping
    /// the lock without writing leaves the index as it was.
    ///
    /// # Errors
    ///
    /// `Error::Locked` if `index.lock` already exists.
    pub(crate) fn lock_index(&self) -> Result<(IndexLock, Index)> {
        let lock = IndexLock(LockFile::acquire(self.git_dir.join("index"))?);
        let idx = self.read_index()?;
        Ok((lock, idx))
    }

    /// Updates HEAD to point to a new commit, recording the update in the
    /// reflogs of HEAD and of the branch it points to.
    ///
    /// If HEAD points to a branch, updates the branch reference.
    /// If HEAD is detached, updates HEAD directly.
    ///
    /// `expected` is the commit HEAD pointed to when the caller read it
    /// (`None` for an unborn branch). HEAD and the branch are locked, as Git
    /// does, and the update is refused with `Error::StaleReference` if HEAD
    /// has moved since, so a concurrent update is never overwritten.
    pub(crate) fn update_head(
        &self,
        new_oid: &Oid,
        expected: Option<Oid>,
        committer: &Signature,
        message: &str,
    ) -> Result<()> {
        let lock = self.lock_head()?;
        if lock.old != expected {
            return Err(Error::StaleReference(
                lock.branch
                    .map(|(name, _)| name)
                    .unwrap_or_else(|| "HEAD".to_owned()),
            ));
        }
        lock.update(self, new_oid, committer, message)
    }

    /// Locks HEAD and the branch it points to (if any), as Git does before
    /// moving HEAD, and reads the commit HEAD points to under the lock.
    /// Taking the lock first lets an operation fail before changing
    /// anything when another process holds it.
    ///
    /// # Errors
    ///
    /// `Error::Locked` if HEAD or the branch is locked.
    pub(crate) fn lock_head(&self) -> Result<HeadLock> {
        let head = LockFile::acquire(self.git_dir.join("HEAD"))?;
        let branch = match self.ref_store().read_ref_file("HEAD")? {
            crate::refs::RefValue::Symbolic(target) => {
                let lock = LockFile::acquire(self.git_dir.join(&target))?;
                Some((target, lock))
            }
            crate::refs::RefValue::Direct(_) => None,
        };
        let old = self.optional_head_oid()?;
        Ok(HeadLock { head, branch, old })
    }

    /// The reflog writer, configured by `core.logAllRefUpdates`.
    pub(crate) fn reflog_writer(&self) -> Result<Reflog> {
        Ok(Reflog::new(
            &self.git_dir,
            &self.config()?,
            self.work_dir == self.git_dir,
        ))
    }

    /// The identity recorded in reflogs for operations that do not create a
    /// commit: `GIT_COMMITTER_NAME`/`GIT_COMMITTER_EMAIL`, then `user.name`/
    /// `user.email`, with the current time.
    pub(crate) fn reflog_identity(&self) -> Result<Signature> {
        let config = self.config()?;
        let name = std::env::var("GIT_COMMITTER_NAME")
            .ok()
            .or_else(|| config.get("user", "name").map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        let email = std::env::var("GIT_COMMITTER_EMAIL")
            .ok()
            .or_else(|| config.get("user", "email").map(str::to_owned))
            .unwrap_or_default();
        Ok(Signature::now(name, email))
    }
}

/// Validates a branch or tag name (the part after `refs/heads/` or
/// `refs/tags/`) with the rules of `git check-ref-format`, plus Git's
/// refusal of names starting with `-`.
///
/// A name is rejected when it is empty, starts with `-` or `/`, ends with
/// `/` or `.`, contains `//`, `..`, `@{`, a space, a control character or
/// one of `~ ^ : ? * [ \`, is `@`, or has a component that starts with `.`
/// or ends with `.lock`.
pub(crate) fn validate_ref_name(kind: &str, name: &str) -> Result<()> {
    let invalid = |reason: &str| {
        Err(Error::InvalidRefName(format!(
            "{} name {}: {}",
            kind, reason, name
        )))
    };
    if name.is_empty() {
        return invalid("cannot be empty");
    }
    if name.starts_with('-') {
        return invalid("cannot start with '-'");
    }
    if name.starts_with('/') || name.ends_with('/') {
        return invalid("cannot start or end with '/'");
    }
    if name.ends_with('.') {
        return invalid("cannot end with '.'");
    }
    // `git branch` reads "@" as HEAD and refuses a branch named HEAD.
    if kind == "branch" && (name == "@" || name == "HEAD") {
        return invalid("is reserved");
    }
    for sequence in ["//", "..", "@{"] {
        if name.contains(sequence) {
            return invalid(&format!("cannot contain '{}'", sequence));
        }
    }
    for c in ['~', '^', ':', '?', '*', '[', '\\', ' '] {
        if name.contains(c) {
            return invalid(&format!("contains invalid character '{}'", c));
        }
    }
    if name.chars().any(|c| c.is_ascii_control()) {
        return invalid("cannot contain control characters");
    }
    for component in name.split('/') {
        if component.starts_with('.') {
            return invalid("has a component starting with '.'");
        }
        if component.ends_with(".lock") {
            return invalid("has a component ending with '.lock'");
        }
    }
    Ok(())
}

/// The locks on HEAD and its branch taken by [`Repository::lock_head`].
pub(crate) struct HeadLock {
    head: LockFile,
    /// The branch HEAD points to, with its lock; `None` when detached.
    branch: Option<(String, LockFile)>,
    /// The commit HEAD pointed to when locked (`None`: unborn branch).
    pub(crate) old: Option<Oid>,
}

impl HeadLock {
    /// Moves HEAD (its branch, or HEAD itself when detached) to `new_oid`,
    /// recording `message` in the reflogs of the branch and HEAD while the
    /// locks are held, and releases the locks.
    pub(crate) fn update(
        mut self,
        repo: &Repository,
        new_oid: &Oid,
        committer: &Signature,
        message: &str,
    ) -> Result<()> {
        let reflog = repo.reflog_writer()?;
        let old_oid = self.old.unwrap_or_else(zero_oid);
        if self.old == Some(*new_oid) {
            // Nothing moves. As in Git, HEAD's reflog still records the
            // operation when HEAD points to a branch (whose own reflog does
            // not), and a detached HEAD records nothing; dropping the locks
            // leaves the files as they are.
            if self.branch.is_some() {
                reflog.append("HEAD", &old_oid, new_oid, committer, message)?;
            }
            return Ok(());
        }
        let content = format!("{}\n", new_oid.to_hex());
        match &mut self.branch {
            Some((target, lock)) => {
                lock.write_all(content.as_bytes())?;
                reflog.append(target, &old_oid, new_oid, committer, message)?;
            }
            None => self.head.write_all(content.as_bytes())?,
        }
        reflog.append("HEAD", &old_oid, new_oid, committer, message)?;
        match self.branch {
            // HEAD itself is unchanged; dropping its lock leaves it as it is.
            Some((_, lock)) => lock.commit(),
            None => self.head.commit(),
        }
    }
}

/// The lock on the index taken by [`Repository::lock_index`].
pub(crate) struct IndexLock(LockFile);

impl IndexLock {
    /// Saves `idx` as the new index and releases the lock. Entries of files
    /// modified this second are smudged first (see
    /// [`Index::smudge_racy_entries`]).
    pub(crate) fn write(mut self, idx: &Index) -> Result<()> {
        let mut idx = idx.clone();
        idx.smudge_racy_entries(crate::infra::time::now() as u64);
        self.0.write_all(&index::write(&idx))?;
        self.0.commit()
    }
}

/// Rejects operations that would rebuild the index and silently end a
/// sparse checkout by dropping skip-worktree flags.
pub(crate) fn reject_sparse_checkout(index: &Index, operation: &str) -> Result<()> {
    if index.entries().iter().any(IndexEntry::skip_worktree) {
        return Err(Error::UnsupportedIndex {
            version: index.version(),
            reason: format!("{} with skip-worktree entries (sparse checkout)", operation),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // RP-001: Repository::open with valid repository returns Ok
    #[test]
    fn test_open_valid_repository() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path());
        assert!(repo.is_ok());
    }

    // RP-002: Repository::open with .git directory path returns Ok
    #[test]
    fn test_open_git_dir_path() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let git_dir = temp.path().join(".git");
        let repo = Repository::open(&git_dir);
        assert!(repo.is_ok());

        let repo = repo.unwrap();
        assert!(repo.git_dir().ends_with(".git"));
    }

    // RP-003: Repository::open with invalid path returns NotARepository
    #[test]
    fn test_open_invalid_path() {
        let temp = TempDir::new().unwrap();
        // Don't create .git directory

        let repo = Repository::open(temp.path());
        assert!(matches!(repo, Err(Error::NotARepository(_))));
    }

    // RP-003: Repository::open with nonexistent path returns NotARepository
    #[test]
    fn test_open_nonexistent_path() {
        let repo = Repository::open("/nonexistent/path/to/repo");
        assert!(matches!(repo, Err(Error::NotARepository(_))));
    }

    // RP-004: Repository::discover from subdirectory finds root
    #[test]
    fn test_discover_from_subdir() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        // Create a subdirectory
        let subdir = temp.path().join("src").join("lib");
        fs::create_dir_all(&subdir).unwrap();

        let repo = Repository::discover(&subdir);
        assert!(repo.is_ok());

        let repo = repo.unwrap();
        assert_eq!(
            repo.path().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    // RP-005: Repository::discover with no repository returns NotARepository
    #[test]
    fn test_discover_no_repository() {
        let temp = TempDir::new().unwrap();
        // Don't create .git directory

        let repo = Repository::discover(temp.path());
        assert!(matches!(repo, Err(Error::NotARepository(_))));
    }

    // RP-006: Repository::path returns repository root
    #[test]
    fn test_path_returns_root() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        assert_eq!(
            repo.path().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    // RP-007: Repository::git_dir returns .git path
    #[test]
    fn test_git_dir_returns_dot_git() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        assert!(repo.git_dir().ends_with(".git"));
        assert_eq!(
            repo.git_dir().canonicalize().unwrap(),
            temp.path().join(".git").canonicalize().unwrap()
        );
    }

    // Additional: validate_git_dir rejects directory without HEAD
    #[test]
    fn test_validate_git_dir_missing_head() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::create_dir_all(git_dir.join("refs")).unwrap();
        // Don't create HEAD

        let result = Repository::validate_git_dir(&git_dir);
        assert!(matches!(result, Err(Error::NotARepository(_))));
    }

    // Additional: validate_git_dir rejects directory without objects
    #[test]
    fn test_validate_git_dir_missing_objects() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(git_dir.join("refs")).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        // Don't create objects

        let result = Repository::validate_git_dir(&git_dir);
        assert!(matches!(result, Err(Error::NotARepository(_))));
    }

    // Additional: validate_git_dir rejects directory without refs
    #[test]
    fn test_validate_git_dir_missing_refs() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        // Don't create refs

        let result = Repository::validate_git_dir(&git_dir);
        assert!(matches!(result, Err(Error::NotARepository(_))));
    }

    // Additional: discover from repository root
    #[test]
    fn test_discover_from_root() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::discover(temp.path());
        assert!(repo.is_ok());
    }

    // Additional: head() returns branch state
    #[test]
    fn test_head_returns_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Initial");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            format!("{}\n", commit_oid.to_hex()),
        )
        .unwrap();

        let repo = Repository::open(temp.path()).unwrap();
        let head = repo.head().unwrap();

        assert!(head.is_branch());
        assert_eq!(head.branch_name(), Some("main"));
        assert_eq!(head.oid(), &commit_oid);
    }

    // Additional: head() returns detached state
    #[test]
    fn test_head_returns_detached() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Initial");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        // Make HEAD point directly to commit
        fs::write(git_dir.join("HEAD"), format!("{}\n", commit_oid.to_hex())).unwrap();

        let repo = Repository::open(temp.path()).unwrap();
        let head = repo.head().unwrap();

        assert!(head.is_detached());
        assert_eq!(head.branch_name(), None);
        assert_eq!(head.oid(), &commit_oid);
    }
}

/// Helpers shared by the unit tests of the modules that extend [`Repository`].
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a minimal valid .git directory
    pub(crate) fn create_git_dir(path: &Path) {
        let git_dir = path.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::create_dir_all(git_dir.join("refs")).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    pub(crate) use crate::infra::hash_object;
    pub(crate) use miniz_oxide::deflate::compress_to_vec_zlib;

    /// Helper to create a loose object in the .git/objects directory.
    pub(crate) fn create_loose_object(git_dir: &Path, content: &[u8], object_type: &str) -> Oid {
        let objects_dir = git_dir.join("objects");
        let header = format!("{} {}\0", object_type, content.len());
        let mut raw = header.into_bytes();
        raw.extend_from_slice(content);

        let oid = Oid::from_bytes(hash_object(object_type, content));
        let compressed = compress_to_vec_zlib(&raw, 6);

        let hex = oid.to_hex();
        let object_path = objects_dir.join(&hex[..2]).join(&hex[2..]);
        fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        fs::write(&object_path, &compressed).unwrap();

        oid
    }

    /// Helper to create a valid commit object content.
    pub(crate) fn make_commit_content(
        tree_oid: &str,
        parent_oid: Option<&str>,
        message: &str,
    ) -> String {
        let mut content = format!("tree {}\n", tree_oid);
        if let Some(parent) = parent_oid {
            content.push_str(&format!("parent {}\n", parent));
        }
        content.push_str("author Test User <test@example.com> 1700000000 +0000\n");
        content.push_str("committer Test User <test@example.com> 1700000000 +0000\n");
        content.push('\n');
        content.push_str(message);
        content
    }

    /// Helper to create a commit with a specific timestamp.
    pub(crate) fn make_commit_content_with_time(
        tree_oid: &str,
        parent_oid: Option<&str>,
        message: &str,
        timestamp: i64,
    ) -> String {
        let mut content = format!("tree {}\n", tree_oid);
        if let Some(parent) = parent_oid {
            content.push_str(&format!("parent {}\n", parent));
        }
        content.push_str(&format!(
            "author Test User <test@example.com> {} +0000\n",
            timestamp
        ));
        content.push_str(&format!(
            "committer Test User <test@example.com> {} +0000\n",
            timestamp
        ));
        content.push('\n');
        content.push_str(message);
        content
    }

    /// Helper to set up a repository with an initial commit
    pub(crate) fn setup_repo_with_commit(temp: &TempDir) -> (Repository, Oid) {
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create a file and commit
        fs::write(temp.path().join("test.txt"), "Test content").unwrap();
        repo.add("test.txt").unwrap();
        let commit_oid = repo
            .create_commit("Initial commit", "Test User", "test@example.com")
            .unwrap();

        (repo, commit_oid)
    }
}
