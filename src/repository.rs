//! Git repository operations.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{self, Index, IndexEntry};
use crate::infra::{read_file, write_file_atomic};
use crate::log::{LogIterator, LogOptions};
use crate::objects::tree::FileMode;
use crate::objects::{Blob, Commit, LooseObjectStore, Object, ObjectType, Oid, TagObject, Tree};
use crate::refs::{Branch, Head, RefStore, RemoteBranch, Tag};
use crate::status::{compute_status, flatten_tree, StatusEntry};

use std::fs;

use std::collections::BTreeMap;

/// A Git repository.
///
/// This is the main entry point for interacting with a Git repository.
/// It provides access to objects, references, and the index.
#[derive(Debug)]
pub struct Repository {
    /// The root directory of the working tree.
    work_dir: PathBuf,
    /// The path to the `.git` directory.
    git_dir: PathBuf,
}

impl Repository {
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

        Ok(Repository { work_dir, git_dir })
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
                return Ok(Repository {
                    work_dir: current,
                    git_dir,
                });
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

    /// Returns a reference to the loose object store.
    fn object_store(&self) -> LooseObjectStore {
        LooseObjectStore::new(self.git_dir.join("objects"))
    }

    /// Resolves a short (abbreviated) OID to a full OID.
    ///
    /// # Arguments
    ///
    /// * `short_oid` - A hexadecimal string of at least 4 characters.
    ///
    /// # Returns
    ///
    /// The full OID if exactly one object matches the prefix.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidOid` if the prefix is too short or contains invalid characters.
    /// - `Error::ObjectNotFound` if no object matches the prefix.
    /// - `Error::InvalidOid` if multiple objects match the prefix (ambiguous).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let full_oid = repo.resolve_short_oid("abc1234").unwrap();
    /// ```
    pub fn resolve_short_oid(&self, short_oid: &str) -> Result<Oid> {
        // If it's already a full OID, just parse it
        if short_oid.len() == 40 {
            return Oid::from_hex(short_oid);
        }

        let store = self.object_store();
        let matches = store.find_objects_by_prefix(short_oid)?;

        match matches.len() {
            0 => Err(Error::ObjectNotFound(short_oid.to_string())),
            1 => Ok(matches.into_iter().next().unwrap()),
            _ => Err(Error::InvalidOid(format!(
                "ambiguous short OID: {} ({} matches)",
                short_oid,
                matches.len()
            ))),
        }
    }

    /// Retrieves a commit by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - The full or abbreviated OID as a hexadecimal string.
    ///
    /// # Returns
    ///
    /// The commit object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a commit.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let commit = repo.commit("abc1234").unwrap();
    /// println!("Author: {}", commit.author().name());
    /// ```
    pub fn commit(&self, oid_str: &str) -> Result<Commit> {
        let oid = self.resolve_short_oid(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Commit {
            return Err(Error::TypeMismatch {
                expected: "commit",
                actual: raw.object_type.as_str(),
            });
        }

        Commit::parse(oid, raw)
    }

    /// Retrieves a tree by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - The full or abbreviated OID as a hexadecimal string.
    ///
    /// # Returns
    ///
    /// The tree object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a tree.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let tree = repo.tree("abc1234").unwrap();
    /// for entry in tree.iter() {
    ///     println!("{}: {}", entry.mode().as_octal(), entry.name());
    /// }
    /// ```
    pub fn tree(&self, oid_str: &str) -> Result<Tree> {
        let oid = self.resolve_short_oid(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Tree {
            return Err(Error::TypeMismatch {
                expected: "tree",
                actual: raw.object_type.as_str(),
            });
        }

        Tree::parse(raw)
    }

    /// Retrieves a blob by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - The full or abbreviated OID as a hexadecimal string.
    ///
    /// # Returns
    ///
    /// The blob object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a blob.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let blob = repo.blob("abc1234").unwrap();
    /// println!("Size: {} bytes", blob.size());
    /// ```
    pub fn blob(&self, oid_str: &str) -> Result<Blob> {
        let oid = self.resolve_short_oid(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Blob {
            return Err(Error::TypeMismatch {
                expected: "blob",
                actual: raw.object_type.as_str(),
            });
        }

        Blob::parse(raw)
    }

    /// Retrieves a Git object by its OID.
    ///
    /// This method returns the object as a unified `Object` enum,
    /// which can be any of blob, tree, or commit.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - The full or abbreviated OID as a hexadecimal string.
    ///
    /// # Returns
    ///
    /// The object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    /// use zerogit::objects::Object;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let obj = repo.object("abc1234").unwrap();
    /// match obj {
    ///     Object::Blob(blob) => println!("Blob: {} bytes", blob.size()),
    ///     Object::Tree(tree) => println!("Tree: {} entries", tree.len()),
    ///     Object::Commit(commit) => println!("Commit: {}", commit.summary()),
    /// }
    /// ```
    pub fn object(&self, oid_str: &str) -> Result<Object> {
        let oid = self.resolve_short_oid(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        match raw.object_type {
            ObjectType::Blob => Ok(Object::Blob(Blob::parse(raw)?)),
            ObjectType::Tree => Ok(Object::Tree(Tree::parse(raw)?)),
            ObjectType::Commit => Ok(Object::Commit(Commit::parse(oid, raw)?)),
            ObjectType::Tag => Err(Error::InvalidObject {
                oid: oid.to_hex(),
                reason: "tag objects are not yet supported".to_string(),
            }),
        }
    }

    /// Returns a reference to the ref store.
    fn ref_store(&self) -> RefStore {
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

    /// Returns an iterator over the commit history starting from HEAD.
    ///
    /// Commits are returned in reverse chronological order (newest first).
    ///
    /// # Returns
    ///
    /// A `LogIterator` that yields commits.
    ///
    /// # Errors
    ///
    /// - `Error::RefNotFound` if HEAD doesn't exist.
    /// - Other errors if the initial commit cannot be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// for result in repo.log().unwrap().take(10) {
    ///     match result {
    ///         Ok(commit) => println!("{}: {}", commit.author().name(), commit.summary()),
    ///         Err(e) => eprintln!("Error: {}", e),
    ///     }
    /// }
    /// ```
    pub fn log(&self) -> Result<LogIterator> {
        let head = self.head()?;
        self.log_from(*head.oid())
    }

    /// Returns an iterator over the commit history starting from a specific commit.
    ///
    /// Commits are returned in reverse chronological order (newest first).
    ///
    /// # Arguments
    ///
    /// * `start_oid` - The OID of the commit to start from.
    ///
    /// # Returns
    ///
    /// A `LogIterator` that yields commits.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    /// use zerogit::objects::Oid;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let oid = Oid::from_hex("abc1234567890abcdef1234567890abcdef12345").unwrap();
    /// for commit in repo.log_from(oid).unwrap().take(5) {
    ///     // ...
    /// }
    /// ```
    pub fn log_from(&self, start_oid: Oid) -> Result<LogIterator> {
        LogIterator::new(self.git_dir.join("objects"), start_oid)
    }

    /// Returns an iterator over the commit history with filtering options.
    ///
    /// This allows filtering commits by path, date, author, and more.
    ///
    /// # Arguments
    ///
    /// * `options` - The filtering options to apply.
    ///
    /// # Returns
    ///
    /// A `LogIterator` that yields only commits matching the filter criteria.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    /// use zerogit::log::LogOptions;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// // Get last 10 commits that modified src/
    /// let log = repo.log_with_options(
    ///     LogOptions::new()
    ///         .path("src/")
    ///         .max_count(10)
    /// ).unwrap();
    ///
    /// for commit in log {
    ///     println!("{}", commit.unwrap().summary());
    /// }
    /// ```
    pub fn log_with_options(&self, options: LogOptions) -> Result<LogIterator> {
        let start_oid = if let Some(oid) = options.get_from() {
            *oid
        } else {
            *self.head()?.oid()
        };
        LogIterator::with_options(self.git_dir.join("objects"), start_oid, options)
    }

    /// Returns the status of the working tree.
    ///
    /// This compares HEAD, the index, and the working tree to detect:
    /// - Untracked files (new files not in Git)
    /// - Modified files (changed since last staged)
    /// - Deleted files (removed from working tree)
    /// - Staged changes (added/modified/deleted in index)
    ///
    /// # Returns
    ///
    /// A vector of `StatusEntry` representing all files with changes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    /// use zerogit::status::FileStatus;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// for entry in repo.status().unwrap() {
    ///     match entry.status() {
    ///         FileStatus::Untracked => println!("?? {}", entry.path().display()),
    ///         FileStatus::Modified => println!(" M {}", entry.path().display()),
    ///         FileStatus::Deleted => println!(" D {}", entry.path().display()),
    ///         FileStatus::Added => println!("A  {}", entry.path().display()),
    ///         FileStatus::StagedModified => println!("M  {}", entry.path().display()),
    ///         FileStatus::StagedDeleted => println!("D  {}", entry.path().display()),
    ///     }
    /// }
    /// ```
    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        let store = self.object_store();

        // Get HEAD tree OID (if HEAD exists and points to a commit)
        let head_tree_oid = self.head().ok().and_then(|head| {
            self.commit(&head.oid().to_hex())
                .ok()
                .map(|commit| *commit.tree())
        });

        // Read and parse index (if exists)
        let index_path = self.git_dir.join("index");
        let parsed_index = if index_path.exists() {
            let index_data = read_file(&index_path)?;
            Some(index::parse(&index_data)?)
        } else {
            None
        };

        compute_status(
            &self.work_dir,
            &store,
            head_tree_oid.as_ref(),
            parsed_index.as_ref(),
        )
    }

    /// Reads the current index, or creates an empty one if it doesn't exist.
    fn read_index(&self) -> Result<Index> {
        let index_path = self.git_dir.join("index");
        if index_path.exists() {
            let index_data = read_file(&index_path)?;
            index::parse(&index_data)
        } else {
            Ok(Index::empty(2))
        }
    }

    /// Writes the index to disk.
    fn write_index(&self, idx: &Index) -> Result<()> {
        let index_path = self.git_dir.join("index");
        let data = index::write(idx);
        write_file_atomic(&index_path, &data)
    }

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
        let path = path.as_ref();
        let full_path = self.work_dir.join(path);

        // Check if file exists
        if !full_path.exists() {
            return Err(Error::PathNotFound(path.to_path_buf()));
        }

        // Read file content
        let content = read_file(&full_path)?;

        // Get file metadata
        let metadata = std::fs::metadata(&full_path)?;

        // Write blob to object store
        let store = self.object_store();
        let oid = store.write(ObjectType::Blob, &content)?;

        // Determine file mode
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 != 0 {
                FileMode::Executable
            } else {
                FileMode::Regular
            }
        };
        #[cfg(not(unix))]
        let mode = FileMode::Regular;

        // Get timestamps
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let ctime = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(mtime);

        // Create index entry
        let entry = IndexEntry::new(
            ctime,
            mtime,
            0,                         // dev (not portable, use 0)
            0,                         // ino (not portable, use 0)
            mode,
            0,                         // uid (not portable, use 0)
            0,                         // gid (not portable, use 0)
            content.len() as u32,
            oid,
            path.to_path_buf(),
            0,                         // stage (normal entry)
        );

        // Read current index, add entry, and write back
        let mut idx = self.read_index()?;
        idx.add(entry);
        self.write_index(&idx)?;

        Ok(())
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
        use std::collections::BTreeMap;

        let store = self.object_store();
        let mut idx = self.read_index()?;

        // Get HEAD tree files (if exists)
        let head_tree_oid = self.head().ok().and_then(|head| {
            self.commit(&head.oid().to_hex())
                .ok()
                .map(|commit| *commit.tree())
        });

        let mut head_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
        if let Some(tree_oid) = head_tree_oid {
            flatten_tree(&store, &tree_oid, Path::new(""), &mut head_files)?;
        }

        // Get working tree files
        let working_files = crate::infra::list_working_tree(&self.work_dir)?;

        // Add all working tree files
        for path in &working_files {
            let full_path = self.work_dir.join(path);
            let content = read_file(&full_path)?;
            let metadata = std::fs::metadata(&full_path)?;

            // Write blob
            let oid = store.write(ObjectType::Blob, &content)?;

            // Determine file mode
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 != 0 {
                    FileMode::Executable
                } else {
                    FileMode::Regular
                }
            };
            #[cfg(not(unix))]
            let mode = FileMode::Regular;

            // Get timestamps
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let ctime = metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(mtime);

            let entry = IndexEntry::new(
                ctime,
                mtime,
                0,
                0,
                mode,
                0,
                0,
                content.len() as u32,
                oid,
                path.clone(),
                0,
            );

            idx.add(entry);
        }

        // Handle deleted files: remove from index files that are in HEAD but not in working tree
        let working_set: std::collections::HashSet<_> = working_files.into_iter().collect();
        for head_path in head_files.keys() {
            if !working_set.contains(head_path) {
                idx.remove(head_path);
            }
        }

        self.write_index(&idx)?;

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
        use std::collections::BTreeMap;

        let store = self.object_store();
        let mut idx = self.read_index()?;

        // Get HEAD tree files
        let head_tree_oid = self.head().ok().and_then(|head| {
            self.commit(&head.oid().to_hex())
                .ok()
                .map(|commit| *commit.tree())
        });

        let mut head_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
        if let Some(tree_oid) = head_tree_oid {
            flatten_tree(&store, &tree_oid, Path::new(""), &mut head_files)?;
        }

        match path {
            Some(p) => {
                // Reset specific path
                let path = p.as_ref();
                if let Some(head_oid) = head_files.get(path) {
                    // File exists in HEAD, restore it to index
                    let raw = store.read(head_oid)?;
                    let entry = IndexEntry::new(
                        0, // ctime (will be updated on next add)
                        0, // mtime
                        0,
                        0,
                        FileMode::Regular, // Simplified: assume regular file
                        0,
                        0,
                        raw.content.len() as u32,
                        *head_oid,
                        path.to_path_buf(),
                        0,
                    );
                    idx.add(entry);
                } else {
                    // File doesn't exist in HEAD, remove from index
                    idx.remove(path);
                }
            }
            None => {
                // Reset all: rebuild index from HEAD
                idx.clear();

                for (path, oid) in &head_files {
                    let raw = store.read(oid)?;
                    let entry = IndexEntry::new(
                        0,
                        0,
                        0,
                        0,
                        FileMode::Regular,
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

        self.write_index(&idx)?;

        Ok(())
    }

    /// Builds a tree object from the current index.
    ///
    /// This creates tree objects for all directories in the index,
    /// constructing them bottom-up to handle nested directories.
    ///
    /// # Returns
    ///
    /// The OID of the root tree object.
    fn build_tree_from_index(&self, idx: &Index) -> Result<Oid> {
        let store = self.object_store();

        // Group entries by directory
        // Key: directory path (empty string for root)
        // Value: Vec<(name, mode, oid)>
        let mut dir_entries: BTreeMap<PathBuf, Vec<(String, FileMode, Oid)>> = BTreeMap::new();

        // First, collect all blobs by their parent directory
        for entry in idx.entries() {
            let path = entry.path();
            let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            dir_entries
                .entry(parent)
                .or_default()
                .push((name, entry.mode(), *entry.oid()));
        }

        // Process directories from deepest to shallowest
        // We need to build subtrees first, then include them in parent trees
        let mut tree_oids: BTreeMap<PathBuf, Oid> = BTreeMap::new();

        // Get all unique directory paths (including intermediate ones)
        let mut all_dirs: Vec<PathBuf> = dir_entries.keys().cloned().collect();

        // Also add parent directories that might only contain subdirectories
        for entry in idx.entries() {
            let mut current = entry.path().parent();
            while let Some(p) = current {
                if !p.as_os_str().is_empty() && !all_dirs.contains(&p.to_path_buf()) {
                    all_dirs.push(p.to_path_buf());
                }
                current = p.parent();
            }
        }

        // Sort directories by depth (deepest first)
        all_dirs.sort_by(|a, b| {
            let depth_a = a.components().count();
            let depth_b = b.components().count();
            depth_b.cmp(&depth_a) // Reverse order: deepest first
        });

        // Add root directory if not present
        if !all_dirs.contains(&PathBuf::new()) {
            all_dirs.push(PathBuf::new());
        }

        // Process each directory
        for dir in all_dirs {
            let mut entries: Vec<(String, FileMode, Oid)> = Vec::new();

            // Add file entries for this directory
            if let Some(file_entries) = dir_entries.get(&dir) {
                entries.extend(file_entries.iter().cloned());
            }

            // Add subdirectory entries (trees we've already built)
            for (subdir_path, tree_oid) in &tree_oids {
                if let Some(parent) = subdir_path.parent() {
                    if parent == dir {
                        let name = subdir_path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        entries.push((name, FileMode::Directory, *tree_oid));
                    }
                }
            }

            // Sort entries by name (Git requires this)
            entries.sort_by(|a, b| a.0.cmp(&b.0));

            // Build tree object content
            let tree_content = Self::build_tree_content(&entries);
            let tree_oid = store.write(ObjectType::Tree, &tree_content)?;

            tree_oids.insert(dir.clone(), tree_oid);
        }

        // Return the root tree OID
        tree_oids
            .get(&PathBuf::new())
            .copied()
            .ok_or_else(|| Error::EmptyCommit)
    }

    /// Builds the binary content of a tree object.
    ///
    /// Tree format: `<mode> <name>\0<20-byte-sha1>` for each entry
    fn build_tree_content(entries: &[(String, FileMode, Oid)]) -> Vec<u8> {
        let mut content = Vec::new();

        for (name, mode, oid) in entries {
            // Mode (without leading zeros for directories)
            let mode_str = match mode {
                FileMode::Directory => "40000",
                _ => mode.as_octal(),
            };
            content.extend_from_slice(mode_str.as_bytes());
            content.push(b' ');
            content.extend_from_slice(name.as_bytes());
            content.push(0);
            content.extend_from_slice(oid.as_bytes());
        }

        content
    }

    /// Formats a commit object.
    ///
    /// # Arguments
    ///
    /// * `tree_oid` - The OID of the tree object.
    /// * `parent_oid` - The OID of the parent commit (None for root commits).
    /// * `author` - The author signature string.
    /// * `committer` - The committer signature string.
    /// * `message` - The commit message.
    ///
    /// # Returns
    ///
    /// The formatted commit content as bytes.
    fn format_commit(
        tree_oid: &Oid,
        parent_oid: Option<&Oid>,
        author: &str,
        committer: &str,
        message: &str,
    ) -> Vec<u8> {
        let mut content = String::new();

        // Tree line
        content.push_str(&format!("tree {}\n", tree_oid.to_hex()));

        // Parent line (if not root commit)
        if let Some(parent) = parent_oid {
            content.push_str(&format!("parent {}\n", parent.to_hex()));
        }

        // Author and committer
        content.push_str(&format!("author {}\n", author));
        content.push_str(&format!("committer {}\n", committer));

        // Blank line and message
        content.push('\n');
        content.push_str(message);

        content.into_bytes()
    }

    /// Updates HEAD to point to a new commit.
    ///
    /// If HEAD points to a branch, updates the branch reference.
    /// If HEAD is detached, updates HEAD directly.
    fn update_head(&self, new_oid: &Oid) -> Result<()> {
        let store = self.ref_store();

        match store.read_ref_file("HEAD")? {
            crate::refs::RefValue::Symbolic(target) => {
                // HEAD points to a branch, update the branch
                let branch_path = self.git_dir.join(&target);
                write_file_atomic(&branch_path, format!("{}\n", new_oid.to_hex()).as_bytes())?;
            }
            crate::refs::RefValue::Direct(_) => {
                // HEAD is detached, update HEAD directly
                let head_path = self.git_dir.join("HEAD");
                write_file_atomic(&head_path, format!("{}\n", new_oid.to_hex()).as_bytes())?;
            }
        }

        Ok(())
    }

    /// Creates a new commit from the staged changes.
    ///
    /// This function:
    /// 1. Builds a tree from the current index
    /// 2. Creates a commit object with the tree and parent
    /// 3. Updates HEAD to point to the new commit
    ///
    /// # Arguments
    ///
    /// * `message` - The commit message.
    /// * `author_name` - The author's name.
    /// * `author_email` - The author's email.
    ///
    /// # Returns
    ///
    /// The OID of the new commit.
    ///
    /// # Errors
    ///
    /// - `Error::EmptyCommit` if there are no staged changes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.add("file.txt").unwrap();
    /// let commit_oid = repo.create_commit(
    ///     "Add file.txt",
    ///     "John Doe",
    ///     "john@example.com"
    /// ).unwrap();
    /// ```
    pub fn create_commit(
        &self,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<Oid> {
        // Read the current index
        let idx = self.read_index()?;

        // Check if there are any staged changes
        if idx.is_empty() {
            return Err(Error::EmptyCommit);
        }

        // Build tree from index
        let tree_oid = self.build_tree_from_index(&idx)?;

        // Get parent commit (current HEAD, if exists)
        let parent_oid = self.head().ok().map(|h| *h.oid());

        // Create timestamp
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Format signature (using +0000 timezone for simplicity)
        let signature = format!(
            "{} <{}> {} +0000",
            author_name, author_email, timestamp
        );

        // Format commit content
        let commit_content = Self::format_commit(
            &tree_oid,
            parent_oid.as_ref(),
            &signature,
            &signature, // Use same for committer
            message,
        );

        // Write commit object
        let store = self.object_store();
        let commit_oid = store.write(ObjectType::Commit, &commit_content)?;

        // Update HEAD
        self.update_head(&commit_oid)?;

        Ok(commit_oid)
    }

    /// Validates a branch name according to Git rules.
    ///
    /// A valid branch name:
    /// - Cannot be empty
    /// - Cannot start or end with `/`
    /// - Cannot contain `..`, `~`, `^`, `:`, `?`, `*`, `[`, `\`, or control characters
    /// - Cannot start with `-`
    /// - Cannot end with `.lock`
    fn validate_branch_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(Error::InvalidRefName("branch name cannot be empty".to_string()));
        }

        if name.starts_with('-') {
            return Err(Error::InvalidRefName(format!(
                "branch name cannot start with '-': {}",
                name
            )));
        }

        if name.starts_with('/') || name.ends_with('/') {
            return Err(Error::InvalidRefName(format!(
                "branch name cannot start or end with '/': {}",
                name
            )));
        }

        if name.ends_with(".lock") {
            return Err(Error::InvalidRefName(format!(
                "branch name cannot end with '.lock': {}",
                name
            )));
        }

        let invalid_chars = ['~', '^', ':', '?', '*', '[', '\\'];
        for c in invalid_chars {
            if name.contains(c) {
                return Err(Error::InvalidRefName(format!(
                    "branch name contains invalid character '{}': {}",
                    c, name
                )));
            }
        }

        if name.contains("..") {
            return Err(Error::InvalidRefName(format!(
                "branch name cannot contain '..': {}",
                name
            )));
        }

        // Check for control characters
        if name.chars().any(|c| c.is_control()) {
            return Err(Error::InvalidRefName(format!(
                "branch name cannot contain control characters: {}",
                name
            )));
        }

        Ok(())
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

        // Check if branch already exists
        let branch_path = self.git_dir.join("refs/heads").join(name);
        if branch_path.exists() {
            return Err(Error::RefAlreadyExists(format!("refs/heads/{}", name)));
        }

        // Ensure parent directories exist (for nested branch names like feature/foo)
        if let Some(parent) = branch_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write the branch ref file
        write_file_atomic(&branch_path, format!("{}\n", target_oid.to_hex()).as_bytes())?;

        Ok(Branch::new(name, target_oid))
    }

    /// Deletes a branch.
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
        // Check if this is the current branch
        let store = self.ref_store();
        if let Ok(Some(current)) = store.current_branch() {
            if current == name {
                return Err(Error::CannotDeleteCurrentBranch);
            }
        }

        // Check if branch exists
        let branch_path = self.git_dir.join("refs/heads").join(name);
        if !branch_path.exists() {
            return Err(Error::RefNotFound(format!("refs/heads/{}", name)));
        }

        // Delete the branch ref file
        fs::remove_file(&branch_path)?;

        // Clean up empty parent directories (for nested branch names)
        let mut parent = branch_path.parent();
        let refs_heads = self.git_dir.join("refs/heads");
        while let Some(dir) = parent {
            if dir == refs_heads {
                break;
            }
            if dir.read_dir()?.next().is_none() {
                fs::remove_dir(dir)?;
            } else {
                break;
            }
            parent = dir.parent();
        }

        Ok(())
    }

    /// Checks if the working tree has uncommitted changes.
    ///
    /// Returns `true` if there are modified, staged, or untracked files.
    fn has_uncommitted_changes(&self) -> Result<bool> {
        let status = self.status()?;
        Ok(!status.is_empty())
    }

    /// Checks out a branch or commit.
    ///
    /// This updates the working tree to match the target and updates HEAD.
    ///
    /// # Arguments
    ///
    /// * `target` - The branch name or commit OID to checkout.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    ///
    /// # Errors
    ///
    /// - `Error::RefNotFound` if the target cannot be resolved.
    /// - `Error::DirtyWorkingTree` if there are uncommitted changes.
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
        // Check for uncommitted changes
        if self.has_uncommitted_changes()? {
            return Err(Error::DirtyWorkingTree);
        }

        let store = self.ref_store();

        // Try to resolve as a branch first
        let branch_ref = format!("refs/heads/{}", target);
        let (new_head_content, target_oid) = if let Ok(resolved) = store.resolve_recursive(&branch_ref) {
            // It's a branch - update HEAD to be symbolic
            (format!("ref: {}\n", branch_ref), resolved.oid)
        } else if let Ok(resolved) = store.resolve(target) {
            // It's a known ref - detached HEAD
            (format!("{}\n", resolved.oid.to_hex()), resolved.oid)
        } else if let Ok(oid) = self.resolve_short_oid(target) {
            // It's a commit OID (full or short) - detached HEAD
            (format!("{}\n", oid.to_hex()), oid)
        } else {
            return Err(Error::RefNotFound(target.to_string()));
        };

        // Get the tree for the target commit
        let commit = self.commit(&target_oid.to_hex())?;
        let tree_oid = *commit.tree();

        // Update working tree and index
        self.checkout_tree(&tree_oid)?;

        // Update HEAD
        let head_path = self.git_dir.join("HEAD");
        write_file_atomic(&head_path, new_head_content.as_bytes())?;

        Ok(())
    }

    /// Updates the working tree and index to match a tree object.
    fn checkout_tree(&self, tree_oid: &Oid) -> Result<()> {
        let store = self.object_store();

        // Get current HEAD tree (if any) to compare
        let current_tree = self.head().ok().and_then(|head| {
            self.commit(&head.oid().to_hex())
                .ok()
                .map(|commit| *commit.tree())
        });

        // Flatten both trees for comparison
        let mut current_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
        if let Some(current_oid) = &current_tree {
            flatten_tree(&store, current_oid, Path::new(""), &mut current_files)?;
        }

        let mut target_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
        flatten_tree(&store, tree_oid, Path::new(""), &mut target_files)?;

        // Remove files that exist in current but not in target
        for path in current_files.keys() {
            if !target_files.contains_key(path) {
                let full_path = self.work_dir.join(path);
                if full_path.exists() {
                    fs::remove_file(&full_path)?;
                }
                // Clean up empty parent directories
                let mut parent = full_path.parent();
                while let Some(dir) = parent {
                    if dir == self.work_dir {
                        break;
                    }
                    if dir.exists() && dir.read_dir()?.next().is_none() {
                        fs::remove_dir(dir)?;
                    } else {
                        break;
                    }
                    parent = dir.parent();
                }
            }
        }

        // Create/update files in target tree
        for (path, oid) in &target_files {
            let full_path = self.work_dir.join(path);

            // Skip if file already has the correct content
            if let Some(current_oid) = current_files.get(path) {
                if current_oid == oid {
                    continue;
                }
            }

            // Ensure parent directories exist
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Read blob content and write to working tree
            let raw = store.read(oid)?;
            write_file_atomic(&full_path, &raw.content)?;
        }

        // Rebuild index from target tree
        let mut idx = Index::empty(2);
        for (path, oid) in &target_files {
            let full_path = self.work_dir.join(path);
            let metadata = fs::metadata(&full_path)?;

            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let ctime = metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(mtime);

            let raw = store.read(oid)?;
            let entry = IndexEntry::new(
                ctime,
                mtime,
                0,
                0,
                FileMode::Regular, // Simplified for now
                0,
                0,
                raw.content.len() as u32,
                *oid,
                path.clone(),
                0,
            );
            idx.add(entry);
        }

        self.write_index(&idx)?;

        Ok(())
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
        let branch_names = store.branches()?;
        let current_branch = store.current_branch()?;

        let mut result = Vec::new();
        for name in branch_names {
            let ref_name = format!("refs/heads/{}", name);
            if let Ok(resolved) = store.resolve_recursive(&ref_name) {
                let is_current = current_branch.as_ref().is_some_and(|c| c == &name);
                let branch = if is_current {
                    Branch::current(name, resolved.oid)
                } else {
                    Branch::new(name, resolved.oid)
                };
                result.push(branch);
            }
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
        let store = self.ref_store();
        let remote_branch_tuples = store.remote_branches()?;

        let mut result = Vec::new();
        for (remote, branch) in remote_branch_tuples {
            let ref_name = format!("refs/remotes/{}/{}", remote, branch);
            if let Ok(resolved) = store.resolve_recursive(&ref_name) {
                result.push(RemoteBranch::new(remote, branch, resolved.oid));
            }
        }

        Ok(result)
    }

    /// Lists all tags in the repository.
    ///
    /// Returns a vector of `Tag` objects representing all tags in `refs/tags/`.
    /// For annotated tags, the message and tagger information are included.
    ///
    /// # Returns
    ///
    /// A vector of tags, sorted by name.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// for tag in repo.tags().unwrap() {
    ///     println!("{} -> {}", tag.name(), tag.target().short());
    ///     if let Some(message) = tag.message() {
    ///         println!("  {}", message);
    ///     }
    /// }
    /// ```
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let ref_store = self.ref_store();
        let object_store = self.object_store();
        let tag_names = ref_store.tags()?;

        let mut result = Vec::new();
        for name in tag_names {
            let ref_name = format!("refs/tags/{}", name);
            if let Ok(resolved) = ref_store.resolve_recursive(&ref_name) {
                // Check if this is a tag object (annotated) or direct commit (lightweight)
                if let Ok(raw) = object_store.read(&resolved.oid) {
                    if raw.object_type == ObjectType::Tag {
                        // Annotated tag - parse tag object
                        if let Ok(tag_obj) = TagObject::parse(raw) {
                            result.push(Tag::annotated(
                                name,
                                *tag_obj.object(),
                                tag_obj.message().to_string(),
                                tag_obj.tagger().clone(),
                            ));
                            continue;
                        }
                    }
                }
                // Lightweight tag or failed to parse - just use the resolved OID
                result.push(Tag::lightweight(name, resolved.oid));
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a minimal valid .git directory
    fn create_git_dir(path: &Path) {
        let git_dir = path.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        fs::create_dir_all(git_dir.join("refs")).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

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

    // =========================================================================
    // Object retrieval tests (RP-010 to RP-013, RP-030 to RP-034)
    // =========================================================================

    use crate::infra::hash_object;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    /// Helper to create a loose object in the .git/objects directory.
    fn create_loose_object(git_dir: &Path, content: &[u8], object_type: &str) -> Oid {
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
    fn make_commit_content(tree_oid: &str, parent_oid: Option<&str>, message: &str) -> String {
        let mut content = format!("tree {}\n", tree_oid);
        if let Some(parent) = parent_oid {
            content.push_str(&format!("parent {}\n", parent));
        }
        content.push_str("author Test User <test@example.com> 1700000000 +0000\n");
        content.push_str("committer Test User <test@example.com> 1700000000 +0000\n");
        content.push_str("\n");
        content.push_str(message);
        content
    }

    // RP-010: repository.commit() with full SHA returns Ok(Commit)
    #[test]
    fn test_commit_full_sha_returns_ok() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create a tree first (empty tree)
        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        // Create a commit
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Initial commit");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.commit(&commit_oid.to_hex());
        assert!(commit.is_ok());

        let commit = commit.unwrap();
        assert_eq!(commit.summary(), "Initial commit");
        assert_eq!(commit.tree().to_hex(), tree_oid.to_hex());
    }

    // RP-011: repository.commit() with short SHA returns Ok(Commit)
    #[test]
    fn test_commit_short_sha_returns_ok() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Test commit");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let short_sha = &commit_oid.to_hex()[..7];
        let commit = repo.commit(short_sha);
        assert!(commit.is_ok());
        assert_eq!(commit.unwrap().summary(), "Test commit");
    }

    // RP-012: repository.commit() with too short SHA returns InvalidOid
    #[test]
    fn test_commit_too_short_sha_returns_error() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.commit("abc");
        assert!(matches!(result, Err(Error::InvalidOid(_))));
    }

    // RP-013: repository.commit() with nonexistent SHA returns ObjectNotFound
    #[test]
    fn test_commit_nonexistent_sha_returns_object_not_found() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.commit("0000000000000000000000000000000000000000");
        assert!(matches!(result, Err(Error::ObjectNotFound(_))));
    }

    // RP-030: repository.object() with blob SHA returns Object::Blob
    #[test]
    fn test_object_blob_returns_blob() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"Hello, World!", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&blob_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Blob(_)));
        if let Object::Blob(blob) = obj {
            assert_eq!(blob.content(), b"Hello, World!");
        }
    }

    // RP-031: repository.object() with tree SHA returns Object::Tree
    #[test]
    fn test_object_tree_returns_tree() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create an empty tree
        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&tree_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Tree(_)));
        if let Object::Tree(tree) = obj {
            assert!(tree.is_empty());
        }
    }

    // RP-032: repository.object() with commit SHA returns Object::Commit
    #[test]
    fn test_object_commit_returns_commit() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Test message");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&commit_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Commit(_)));
        if let Object::Commit(commit) = obj {
            assert_eq!(commit.summary(), "Test message");
        }
    }

    // RP-033: repository.tree() with blob SHA returns TypeMismatch
    #[test]
    fn test_tree_with_blob_sha_returns_type_mismatch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"blob content", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.tree(&blob_oid.to_hex());

        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "tree",
                actual: "blob"
            })
        ));
    }

    // RP-034: repository.blob() with tree SHA returns TypeMismatch
    #[test]
    fn test_blob_with_tree_sha_returns_type_mismatch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.blob(&tree_oid.to_hex());

        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "blob",
                actual: "tree"
            })
        ));
    }

    // Additional: resolve_short_oid with full OID
    #[test]
    fn test_resolve_short_oid_full_oid() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"test", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let resolved = repo.resolve_short_oid(&blob_oid.to_hex()).unwrap();
        assert_eq!(resolved, blob_oid);
    }

    // Additional: resolve_short_oid with nonexistent prefix
    #[test]
    fn test_resolve_short_oid_not_found() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.resolve_short_oid("0000000");
        assert!(matches!(result, Err(Error::ObjectNotFound(_))));
    }

    // Additional: blob() returns correct content
    #[test]
    fn test_blob_returns_content() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let content = b"fn main() { println!(\"Hello\"); }";
        let blob_oid = create_loose_object(&git_dir, content, "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let blob = repo.blob(&blob_oid.to_hex()).unwrap();

        assert_eq!(blob.content(), content);
        assert_eq!(blob.size(), content.len());
    }

    // Additional: tree() parses entries correctly
    #[test]
    fn test_tree_parses_entries() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create a blob first
        let blob_oid = create_loose_object(&git_dir, b"file content", "blob");

        // Create tree content: "100644 file.txt\0<20-byte-sha>"
        let mut tree_content = Vec::new();
        tree_content.extend_from_slice(b"100644 file.txt\0");
        tree_content.extend_from_slice(blob_oid.as_bytes());

        let tree_oid = create_loose_object(&git_dir, &tree_content, "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let tree = repo.tree(&tree_oid.to_hex()).unwrap();

        assert_eq!(tree.len(), 1);
        let entry = tree.get("file.txt").unwrap();
        assert_eq!(entry.name(), "file.txt");
        assert_eq!(entry.oid(), &blob_oid);
    }

    // Additional: commit() with parent
    #[test]
    fn test_commit_with_parent() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let parent_content = make_commit_content(&tree_oid.to_hex(), None, "First commit");
        let parent_oid = create_loose_object(&git_dir, parent_content.as_bytes(), "commit");

        let child_content = make_commit_content(
            &tree_oid.to_hex(),
            Some(&parent_oid.to_hex()),
            "Second commit",
        );
        let child_oid = create_loose_object(&git_dir, child_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.commit(&child_oid.to_hex()).unwrap();

        assert_eq!(commit.summary(), "Second commit");
        assert_eq!(commit.parents().len(), 1);
        assert_eq!(commit.parent().unwrap(), &parent_oid);
    }

    // =========================================================================
    // Log iterator tests (RP-014 to RP-016)
    // =========================================================================

    /// Helper to create a commit with a specific timestamp.
    fn make_commit_content_with_time(
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
        content.push_str("\n");
        content.push_str(message);
        content
    }

    // RP-014: repository.log() returns commits from HEAD
    #[test]
    fn test_log_returns_commits_from_head() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        // Create commit chain
        let c1_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "First commit", 1000);
        let c1_oid = create_loose_object(&git_dir, c1_content.as_bytes(), "commit");

        let c2_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Second commit",
            2000,
        );
        let c2_oid = create_loose_object(&git_dir, c2_content.as_bytes(), "commit");

        // Set up HEAD -> refs/heads/main -> c2
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            format!("{}\n", c2_oid.to_hex()),
        )
        .unwrap();

        let repo = Repository::open(temp.path()).unwrap();
        let log = repo.log().unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Second commit");
        assert_eq!(commits[1].summary(), "First commit");
    }

    // RP-015: repository.log() returns commits in time order (newest first)
    #[test]
    fn test_log_returns_commits_in_time_order() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let c1_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "First commit", 1000);
        let c1_oid = create_loose_object(&git_dir, c1_content.as_bytes(), "commit");

        let c2_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Second commit",
            2000,
        );
        let c2_oid = create_loose_object(&git_dir, c2_content.as_bytes(), "commit");

        let c3_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Third commit",
            3000,
        );
        let c3_oid = create_loose_object(&git_dir, c3_content.as_bytes(), "commit");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            format!("{}\n", c3_oid.to_hex()),
        )
        .unwrap();

        let repo = Repository::open(temp.path()).unwrap();
        let log = repo.log().unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        // Verify descending order
        for window in commits.windows(2) {
            assert!(window[0].author().timestamp() >= window[1].author().timestamp());
        }
    }

    // RP-016: repository.log() handles merge commits correctly
    #[test]
    fn test_log_handles_merge_commits() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        // Create a merge scenario
        let root_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "Root commit", 1000);
        let root_oid = create_loose_object(&git_dir, root_content.as_bytes(), "commit");

        let branch_a_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&root_oid.to_hex()),
            "Branch A commit",
            2000,
        );
        let branch_a_oid = create_loose_object(&git_dir, branch_a_content.as_bytes(), "commit");

        let branch_b_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&root_oid.to_hex()),
            "Branch B commit",
            2500,
        );
        let branch_b_oid = create_loose_object(&git_dir, branch_b_content.as_bytes(), "commit");

        // Merge commit with two parents
        let merge_content = format!(
            "tree {}\nparent {}\nparent {}\nauthor Test <t@t.com> 3000 +0000\ncommitter Test <t@t.com> 3000 +0000\n\nMerge commit",
            tree_oid.to_hex(),
            branch_a_oid.to_hex(),
            branch_b_oid.to_hex()
        );
        let merge_oid = create_loose_object(&git_dir, merge_content.as_bytes(), "commit");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            format!("{}\n", merge_oid.to_hex()),
        )
        .unwrap();

        let repo = Repository::open(temp.path()).unwrap();
        let log = repo.log().unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        // Should have 4 commits: merge, branch_b, branch_a, root
        assert_eq!(commits.len(), 4);
        assert_eq!(commits[0].summary(), "Merge commit");
        // Root should be last
        assert_eq!(commits[3].summary(), "Root commit");
    }

    // Additional: log_from() starts from specific commit
    #[test]
    fn test_log_from_specific_commit() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let c1_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "First commit", 1000);
        let c1_oid = create_loose_object(&git_dir, c1_content.as_bytes(), "commit");

        let c2_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Second commit",
            2000,
        );
        let c2_oid = create_loose_object(&git_dir, c2_content.as_bytes(), "commit");

        let c3_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Third commit",
            3000,
        );
        let _ = create_loose_object(&git_dir, c3_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        // Start from c2, should only get c2 and c1
        let log = repo.log_from(c2_oid).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Second commit");
        assert_eq!(commits[1].summary(), "First commit");
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

    // =========================================================================
    // create_commit tests (W-004 to W-005)
    // =========================================================================

    // W-004: create_commit with staged files creates commit and returns Oid
    #[test]
    fn test_create_commit_with_staged_files() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create refs/heads directory
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create a file and add it
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "Hello, World!").unwrap();
        repo.add("test.txt").unwrap();

        // Create commit
        let commit_oid = repo
            .create_commit("Initial commit", "Test User", "test@example.com")
            .unwrap();

        // Verify commit exists and is valid
        let commit = repo.commit(&commit_oid.to_hex()).unwrap();
        assert_eq!(commit.summary(), "Initial commit");
        assert_eq!(commit.author().name(), "Test User");
        assert_eq!(commit.author().email(), "test@example.com");
        assert!(commit.is_root()); // No parent

        // Verify HEAD is updated
        let head = repo.head().unwrap();
        assert_eq!(head.oid(), &commit_oid);

        // Verify tree contains the file
        let tree = repo.tree(&commit.tree().to_hex()).unwrap();
        assert_eq!(tree.len(), 1);
        let entry = tree.get("test.txt").unwrap();
        assert_eq!(entry.name(), "test.txt");
    }

    // W-005: create_commit with empty index returns EmptyCommit error
    #[test]
    fn test_create_commit_empty_index() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();

        // Try to create commit without staging anything
        let result = repo.create_commit("Empty commit", "Test User", "test@example.com");

        assert!(matches!(result, Err(Error::EmptyCommit)));
    }

    // Additional: create_commit with subdirectories
    #[test]
    fn test_create_commit_with_subdirectories() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create refs/heads directory
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create files in subdirectories
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("README.md"), "# Test").unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();

        repo.add("README.md").unwrap();
        repo.add("src/main.rs").unwrap();

        // Create commit
        let commit_oid = repo
            .create_commit("Add files", "Test User", "test@example.com")
            .unwrap();

        // Verify tree structure
        let commit = repo.commit(&commit_oid.to_hex()).unwrap();
        let tree = repo.tree(&commit.tree().to_hex()).unwrap();

        // Root tree should have README.md and src directory
        assert_eq!(tree.len(), 2);
        assert!(tree.get("README.md").is_some());

        let src_entry = tree.get("src").unwrap();
        assert!(src_entry.is_directory());

        // Verify src subtree
        let src_tree = repo.tree(&src_entry.oid().to_hex()).unwrap();
        assert_eq!(src_tree.len(), 1);
        assert!(src_tree.get("main.rs").is_some());
    }

    // Additional: create_commit chain (parent linking)
    #[test]
    fn test_create_commit_chain() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // First commit
        fs::write(temp.path().join("file1.txt"), "Content 1").unwrap();
        repo.add("file1.txt").unwrap();
        let first_oid = repo
            .create_commit("First commit", "Test User", "test@example.com")
            .unwrap();

        // Second commit
        fs::write(temp.path().join("file2.txt"), "Content 2").unwrap();
        repo.add("file2.txt").unwrap();
        let second_oid = repo
            .create_commit("Second commit", "Test User", "test@example.com")
            .unwrap();

        // Verify parent linking
        let second_commit = repo.commit(&second_oid.to_hex()).unwrap();
        assert_eq!(second_commit.parent().unwrap(), &first_oid);
        assert!(!second_commit.is_root());

        // Verify HEAD points to second commit
        let head = repo.head().unwrap();
        assert_eq!(head.oid(), &second_oid);
    }

    // Additional: create_commit updates branch ref (not just HEAD)
    #[test]
    fn test_create_commit_updates_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create a file and commit
        fs::write(temp.path().join("test.txt"), "Test content").unwrap();
        repo.add("test.txt").unwrap();
        let commit_oid = repo
            .create_commit("Test commit", "Test User", "test@example.com")
            .unwrap();

        // Verify branch ref was created/updated
        let branch_ref = fs::read_to_string(git_dir.join("refs/heads/main")).unwrap();
        assert_eq!(branch_ref.trim(), commit_oid.to_hex());
    }

    // Additional: build_tree_content produces valid tree
    #[test]
    fn test_build_tree_content() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let entries = vec![
            ("file.txt".to_string(), FileMode::Regular, oid),
        ];

        let content = Repository::build_tree_content(&entries);

        // Verify format: "100644 file.txt\0<20-byte-sha>"
        assert_eq!(&content[..7], b"100644 ");
        assert_eq!(&content[7..15], b"file.txt");
        assert_eq!(content[15], 0);
        assert_eq!(&content[16..], oid.as_bytes());
    }

    // Additional: format_commit produces valid commit
    #[test]
    fn test_format_commit() {
        let tree_oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let parent_oid = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();

        let content = Repository::format_commit(
            &tree_oid,
            Some(&parent_oid),
            "Test User <test@example.com> 1234567890 +0000",
            "Test User <test@example.com> 1234567890 +0000",
            "Test message",
        );

        let content_str = String::from_utf8(content).unwrap();
        assert!(content_str.contains(&format!("tree {}", tree_oid.to_hex())));
        assert!(content_str.contains(&format!("parent {}", parent_oid.to_hex())));
        assert!(content_str.contains("author Test User <test@example.com>"));
        assert!(content_str.contains("committer Test User <test@example.com>"));
        assert!(content_str.contains("\n\nTest message"));
    }

    // Additional: format_commit without parent (root commit)
    #[test]
    fn test_format_commit_no_parent() {
        let tree_oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();

        let content = Repository::format_commit(
            &tree_oid,
            None,
            "Test User <test@example.com> 1234567890 +0000",
            "Test User <test@example.com> 1234567890 +0000",
            "Initial commit",
        );

        let content_str = String::from_utf8(content).unwrap();
        assert!(content_str.contains(&format!("tree {}", tree_oid.to_hex())));
        assert!(!content_str.contains("parent")); // No parent line
        assert!(content_str.contains("Initial commit"));
    }

    // =========================================================================
    // Branch operations tests (W-006 to W-010)
    // =========================================================================

    /// Helper to set up a repository with an initial commit
    fn setup_repo_with_commit(temp: &TempDir) -> (Repository, Oid) {
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
        let branch_ref =
            fs::read_to_string(git_dir.join("refs/heads/feature/my-feature")).unwrap();
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

    // W-010: checkout with dirty working tree returns DirtyWorkingTree
    #[test]
    fn test_checkout_dirty_working_tree() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a new branch
        repo.create_branch("feature", None).unwrap();

        // Modify a file without committing
        fs::write(temp.path().join("test.txt"), "Modified content").unwrap();

        // Try to checkout - should fail
        let result = repo.checkout("feature");
        assert!(matches!(result, Err(Error::DirtyWorkingTree)));
    }

    // W-010: checkout with untracked files returns DirtyWorkingTree
    #[test]
    fn test_checkout_with_untracked_files() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let (repo, _) = setup_repo_with_commit(&temp);

        // Create a new branch
        repo.create_branch("feature", None).unwrap();

        // Create an untracked file
        fs::write(temp.path().join("untracked.txt"), "Untracked").unwrap();

        // Try to checkout - should fail
        let result = repo.checkout("feature");
        assert!(matches!(result, Err(Error::DirtyWorkingTree)));
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
