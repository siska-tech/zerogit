//! Git commit log iteration.
//!
//! This module provides an iterator for traversing commit history
//! in reverse chronological order (newest first).
//!
//! # Filtering
//!
//! Use [`LogOptions`] to filter commits by various criteria:
//!
//! ```no_run
//! use zerogit::{Repository, log::LogOptions};
//!
//! let repo = Repository::open("path/to/repo").unwrap();
//!
//! // Get last 10 commits that modified src/ directory
//! let log = repo.log_with_options(
//!     LogOptions::new()
//!         .path("src/")
//!         .max_count(10)
//! ).unwrap();
//!
//! for commit in log {
//!     println!("{}", commit.unwrap().summary());
//! }
//! ```

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::objects::{Commit, LooseObjectStore, ObjectType, Oid, Tree};

/// A pending commit in the priority queue.
///
/// Commits are ordered by timestamp (descending) for traversal.
#[derive(Debug, Clone)]
struct PendingCommit {
    /// The commit OID.
    oid: Oid,
    /// The author timestamp for ordering.
    timestamp: i64,
}

impl PartialEq for PendingCommit {
    fn eq(&self, other: &Self) -> bool {
        self.oid == other.oid
    }
}

impl Eq for PendingCommit {}

impl PartialOrd for PendingCommit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingCommit {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher timestamp = more recent = higher priority
        self.timestamp.cmp(&other.timestamp)
    }
}

/// Options for filtering commit log output.
///
/// Use the builder pattern to construct filtering options.
///
/// # Example
///
/// ```
/// use zerogit::log::LogOptions;
///
/// let options = LogOptions::new()
///     .path("src/main.rs")
///     .max_count(10)
///     .author("John");
/// ```
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// Filter by paths (commit must touch at least one of these).
    paths: Vec<PathBuf>,
    /// Maximum number of commits to return.
    max_count: Option<usize>,
    /// Only include commits after this timestamp.
    since: Option<i64>,
    /// Only include commits before this timestamp.
    until: Option<i64>,
    /// Only follow the first parent of merge commits.
    first_parent: bool,
    /// Filter by author name (substring match).
    author: Option<String>,
    /// Starting commit OID (defaults to HEAD if not specified).
    from: Option<Oid>,
}

impl LogOptions {
    /// Creates a new `LogOptions` with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a path to filter by.
    ///
    /// Only commits that modify files at this path will be included.
    /// Can be called multiple times to add multiple paths.
    ///
    /// # Arguments
    ///
    /// * `path` - A file or directory path to filter by.
    pub fn path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.paths.push(path.as_ref().to_path_buf());
        self
    }

    /// Adds multiple paths to filter by.
    ///
    /// Only commits that modify files at any of these paths will be included.
    ///
    /// # Arguments
    ///
    /// * `paths` - An iterator of file or directory paths.
    pub fn paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        self.paths
            .extend(paths.into_iter().map(|p| p.as_ref().to_path_buf()));
        self
    }

    /// Sets the maximum number of commits to return.
    ///
    /// # Arguments
    ///
    /// * `n` - The maximum count.
    pub fn max_count(mut self, n: usize) -> Self {
        self.max_count = Some(n);
        self
    }

    /// Only include commits after this date.
    ///
    /// # Arguments
    ///
    /// * `date` - A date string in YYYY-MM-DD format or a Unix timestamp.
    pub fn since(mut self, date: &str) -> Self {
        self.since = Some(parse_date(date));
        self
    }

    /// Only include commits before this date.
    ///
    /// # Arguments
    ///
    /// * `date` - A date string in YYYY-MM-DD format or a Unix timestamp.
    pub fn until(mut self, date: &str) -> Self {
        self.until = Some(parse_date(date));
        self
    }

    /// Sets the since timestamp directly.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - Unix timestamp.
    pub fn since_timestamp(mut self, timestamp: i64) -> Self {
        self.since = Some(timestamp);
        self
    }

    /// Sets the until timestamp directly.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - Unix timestamp.
    pub fn until_timestamp(mut self, timestamp: i64) -> Self {
        self.until = Some(timestamp);
        self
    }

    /// Only follow the first parent of merge commits.
    ///
    /// This is useful for seeing the history of a single branch
    /// without the commits that were merged in.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable first-parent mode.
    pub fn first_parent(mut self, enabled: bool) -> Self {
        self.first_parent = enabled;
        self
    }

    /// Filter commits by author name.
    ///
    /// Uses substring matching on the author name.
    ///
    /// # Arguments
    ///
    /// * `name` - The author name pattern to match.
    pub fn author(mut self, name: &str) -> Self {
        self.author = Some(name.to_string());
        self
    }

    /// Sets the starting commit OID.
    ///
    /// By default, iteration starts from HEAD.
    ///
    /// # Arguments
    ///
    /// * `oid` - The OID of the commit to start from.
    pub fn from(mut self, oid: Oid) -> Self {
        self.from = Some(oid);
        self
    }

    /// Returns true if path filtering is enabled.
    pub fn has_path_filter(&self) -> bool {
        !self.paths.is_empty()
    }

    /// Returns the configured paths.
    pub fn get_paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Returns the configured starting commit OID.
    pub fn get_from(&self) -> Option<&Oid> {
        self.from.as_ref()
    }
}

/// Parses a date string into a Unix timestamp.
///
/// Supported formats:
/// - YYYY-MM-DD (interpreted as midnight UTC)
/// - Unix timestamp (numeric string)
fn parse_date(s: &str) -> i64 {
    // Try parsing as Unix timestamp first
    if let Ok(ts) = s.parse::<i64>() {
        return ts;
    }

    // Try parsing YYYY-MM-DD format
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 3 {
        if let (Ok(year), Ok(month), Ok(day)) = (
            parts[0].parse::<i64>(),
            parts[1].parse::<i64>(),
            parts[2].parse::<i64>(),
        ) {
            // Simple calculation for days since epoch
            // This is a rough approximation; for precise dates, use a proper date library
            let days_since_epoch = (year - 1970) * 365
                + (year - 1969) / 4  // leap years
                + match month {
                    1 => 0,
                    2 => 31,
                    3 => 59,
                    4 => 90,
                    5 => 120,
                    6 => 151,
                    7 => 181,
                    8 => 212,
                    9 => 243,
                    10 => 273,
                    11 => 304,
                    12 => 334,
                    _ => 0,
                }
                + day
                - 1;
            return days_since_epoch * 86400; // seconds per day
        }
    }

    // Default to 0 if parsing fails
    0
}

/// An iterator over commits in the repository history.
///
/// Commits are yielded in reverse chronological order (newest first),
/// following parent links. Merge commits are handled by traversing
/// all parent branches.
///
/// # Example
///
/// ```no_run
/// use zerogit::repository::Repository;
///
/// let repo = Repository::open("path/to/repo").unwrap();
/// let log = repo.log().unwrap();
///
/// for result in log.take(10) {
///     match result {
///         Ok(commit) => println!("{}: {}", commit.author().name(), commit.summary()),
///         Err(e) => eprintln!("Error: {}", e),
///     }
/// }
/// ```
pub struct LogIterator {
    /// The loose object store for reading commits.
    store: LooseObjectStore,
    /// Priority queue of pending commits to visit.
    pending: BinaryHeap<PendingCommit>,
    /// Set of already visited commit OIDs to avoid duplicates.
    visited: HashSet<Oid>,
    /// Filtering options.
    options: LogOptions,
    /// Number of commits yielded so far.
    count: usize,
}

impl LogIterator {
    /// Creates a new LogIterator starting from the given OID.
    ///
    /// # Arguments
    ///
    /// * `objects_dir` - Path to the `.git/objects` directory.
    /// * `start_oid` - The OID of the commit to start from.
    pub fn new(objects_dir: PathBuf, start_oid: Oid) -> Result<Self> {
        Self::with_options(objects_dir, start_oid, LogOptions::default())
    }

    /// Creates a new LogIterator with filtering options.
    ///
    /// # Arguments
    ///
    /// * `objects_dir` - Path to the `.git/objects` directory.
    /// * `start_oid` - The OID of the commit to start from.
    /// * `options` - Filtering options.
    pub fn with_options(objects_dir: PathBuf, start_oid: Oid, options: LogOptions) -> Result<Self> {
        let store = LooseObjectStore::new(&objects_dir);
        let mut pending = BinaryHeap::new();
        let visited = HashSet::new();

        // Read the initial commit to get its timestamp
        let raw = store.read(&start_oid)?;
        let commit = Commit::parse(start_oid, raw)?;

        pending.push(PendingCommit {
            oid: start_oid,
            timestamp: commit.author().timestamp(),
        });

        Ok(LogIterator {
            store,
            pending,
            visited,
            options,
            count: 0,
        })
    }

    /// Reads a commit by its OID.
    fn read_commit(&self, oid: &Oid) -> Result<Commit> {
        let raw = self.store.read(oid)?;

        if raw.object_type != ObjectType::Commit {
            return Err(crate::error::Error::TypeMismatch {
                expected: "commit",
                actual: raw.object_type.as_str(),
            });
        }

        Commit::parse(*oid, raw)
    }

    /// Reads a tree by its OID.
    fn read_tree(&self, oid: &Oid) -> Result<Tree> {
        let raw = self.store.read(oid)?;

        if raw.object_type != ObjectType::Tree {
            return Err(crate::error::Error::TypeMismatch {
                expected: "tree",
                actual: raw.object_type.as_str(),
            });
        }

        Tree::parse(raw)
    }

    /// Gets the OID of an entry at a given path in a tree.
    ///
    /// Returns None if the path doesn't exist.
    fn get_entry_oid_at_path(&self, tree: &Tree, path: &Path) -> Option<Oid> {
        let mut components = path.components().peekable();
        let mut current_tree = tree.clone();

        while let Some(component) = components.next() {
            let name = component.as_os_str().to_str()?;
            let entry = current_tree.get(name)?;

            if components.peek().is_none() {
                // Last component - return the OID
                return Some(*entry.oid());
            } else {
                // Not the last component - must be a directory
                if !entry.is_directory() {
                    return None;
                }
                // Read the subtree
                current_tree = self.read_tree(entry.oid()).ok()?;
            }
        }

        None
    }

    /// Checks if a commit touches any of the configured filter paths.
    ///
    /// A commit "touches" a path if the entry at that path differs between
    /// the commit's tree and its parent's tree.
    fn commit_touches_paths(&self, commit: &Commit) -> Result<bool> {
        let current_tree = self.read_tree(commit.tree())?;

        // Get parent tree (empty if no parent)
        let parent_tree = if let Some(parent_oid) = commit.parents().first() {
            let parent_commit = self.read_commit(parent_oid)?;
            Some(self.read_tree(parent_commit.tree())?)
        } else {
            None
        };

        for path in &self.options.paths {
            let current_oid = self.get_entry_oid_at_path(&current_tree, path);
            let parent_oid = parent_tree
                .as_ref()
                .and_then(|t| self.get_entry_oid_at_path(t, path));

            // Check if the path changed
            if current_oid != parent_oid {
                return Ok(true);
            }

            // Also check if any file under a directory path changed
            // by comparing directory OIDs
            if current_oid.is_none() && parent_oid.is_none() {
                // Path doesn't exist in either - check if it's a prefix of any changed path
                // For simplicity, we check if the path as a directory entry exists
                let path_str = path.to_string_lossy();
                for entry in current_tree.entries() {
                    if entry.name().starts_with(&*path_str)
                        || path_str.starts_with(entry.name())
                    {
                        // Entry might be relevant - do a deeper check
                        let current_entry_oid = Some(*entry.oid());
                        let parent_entry_oid = parent_tree
                            .as_ref()
                            .and_then(|t| t.get(entry.name()))
                            .map(|e| *e.oid());
                        if current_entry_oid != parent_entry_oid {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        Ok(false)
    }

    /// Checks if a commit passes all configured filters.
    fn passes_filters(&self, commit: &Commit) -> Result<bool> {
        // Check date filters
        let timestamp = commit.author().timestamp();

        if let Some(since) = self.options.since {
            if timestamp < since {
                return Ok(false);
            }
        }

        if let Some(until) = self.options.until {
            if timestamp > until {
                return Ok(false);
            }
        }

        // Check author filter
        if let Some(ref author) = self.options.author {
            if !commit.author().name().contains(author) {
                return Ok(false);
            }
        }

        // Check path filter
        if self.options.has_path_filter() {
            if !self.commit_touches_paths(commit)? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl Iterator for LogIterator {
    type Item = Result<Commit>;

    fn next(&mut self) -> Option<Self::Item> {
        // Check max_count limit
        if let Some(max) = self.options.max_count {
            if self.count >= max {
                return None;
            }
        }

        // Find the next unvisited commit that passes filters
        while let Some(pending) = self.pending.pop() {
            if self.visited.contains(&pending.oid) {
                continue;
            }

            // Mark as visited
            self.visited.insert(pending.oid);

            // Read the commit
            let commit = match self.read_commit(&pending.oid) {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };

            // Add parents to the pending queue
            if self.options.first_parent {
                // Only add the first parent
                if let Some(parent_oid) = commit.parents().first() {
                    if !self.visited.contains(parent_oid) {
                        match self.read_commit(parent_oid) {
                            Ok(parent_commit) => {
                                self.pending.push(PendingCommit {
                                    oid: *parent_oid,
                                    timestamp: parent_commit.author().timestamp(),
                                });
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
            } else {
                // Add all parents
                for parent_oid in commit.parents() {
                    if !self.visited.contains(parent_oid) {
                        match self.read_commit(parent_oid) {
                            Ok(parent_commit) => {
                                self.pending.push(PendingCommit {
                                    oid: *parent_oid,
                                    timestamp: parent_commit.author().timestamp(),
                                });
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }
            }

            // Check if commit passes all filters
            match self.passes_filters(&commit) {
                Ok(true) => {
                    self.count += 1;
                    return Some(Ok(commit));
                }
                Ok(false) => {
                    // Commit doesn't pass filters, continue to next
                    continue;
                }
                Err(e) => return Some(Err(e)),
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::hash_object;
    use miniz_oxide::deflate::compress_to_vec_zlib;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a loose object in the objects directory.
    fn create_loose_object(
        objects_dir: &std::path::Path,
        content: &[u8],
        object_type: &str,
    ) -> Oid {
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

    /// Helper to create commit content with specific timestamp.
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

    // L-001: LogIterator starts from given commit
    #[test]
    fn test_log_iterator_single_commit() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");
        let commit_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "Initial commit", 1000);
        let commit_oid = create_loose_object(&objects_dir, commit_content.as_bytes(), "commit");

        let log = LogIterator::new(objects_dir, commit_oid).unwrap();
        let commits: Vec<_> = log.collect();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].as_ref().unwrap().summary(), "Initial commit");
    }

    // L-002: LogIterator follows parent chain
    #[test]
    fn test_log_iterator_follows_parents() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        // Create commit chain: C3 -> C2 -> C1
        let c1_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "First commit", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1_content.as_bytes(), "commit");

        let c2_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Second commit",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2_content.as_bytes(), "commit");

        let c3_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Third commit",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3_content.as_bytes(), "commit");

        let log = LogIterator::new(objects_dir, c3_oid).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].summary(), "Third commit");
        assert_eq!(commits[1].summary(), "Second commit");
        assert_eq!(commits[2].summary(), "First commit");
    }

    // L-003: LogIterator returns commits in time order (newest first)
    #[test]
    fn test_log_iterator_time_order() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "First commit", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1_content.as_bytes(), "commit");

        let c2_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Second commit",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2_content.as_bytes(), "commit");

        let log = LogIterator::new(objects_dir, c2_oid).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        // Verify descending timestamp order
        for window in commits.windows(2) {
            assert!(window[0].author().timestamp() >= window[1].author().timestamp());
        }
    }

    // L-004: LogIterator handles merge commits
    #[test]
    fn test_log_iterator_merge_commit() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        // Create a merge scenario:
        //       M (merge)
        //      / \
        //     B   C
        //      \ /
        //       A (root)
        let a_content =
            make_commit_content_with_time(&tree_oid.to_hex(), None, "Root commit A", 1000);
        let a_oid = create_loose_object(&objects_dir, a_content.as_bytes(), "commit");

        let b_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&a_oid.to_hex()),
            "Branch commit B",
            2000,
        );
        let b_oid = create_loose_object(&objects_dir, b_content.as_bytes(), "commit");

        let c_content = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&a_oid.to_hex()),
            "Branch commit C",
            2500,
        );
        let c_oid = create_loose_object(&objects_dir, c_content.as_bytes(), "commit");

        // Merge commit with two parents
        let m_content = format!(
            "tree {}\nparent {}\nparent {}\nauthor Test <t@t.com> 3000 +0000\ncommitter Test <t@t.com> 3000 +0000\n\nMerge commit",
            tree_oid.to_hex(),
            b_oid.to_hex(),
            c_oid.to_hex()
        );
        let m_oid = create_loose_object(&objects_dir, m_content.as_bytes(), "commit");

        let log = LogIterator::new(objects_dir, m_oid).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        // Should have all 4 commits
        assert_eq!(commits.len(), 4);

        // First should be the merge
        assert_eq!(commits[0].summary(), "Merge commit");

        // Root commit A should be last
        assert_eq!(commits[3].summary(), "Root commit A");

        // Verify no duplicates - A appears only once even though both B and C have A as parent
        let summaries: Vec<_> = commits.iter().map(|c| c.summary()).collect();
        assert_eq!(
            summaries.iter().filter(|s| *s == &"Root commit A").count(),
            1
        );
    }

    // L-005: LogIterator doesn't visit same commit twice
    #[test]
    fn test_log_iterator_no_duplicates() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        // Diamond pattern
        let root = make_commit_content_with_time(&tree_oid.to_hex(), None, "root", 1000);
        let root_oid = create_loose_object(&objects_dir, root.as_bytes(), "commit");

        let left = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&root_oid.to_hex()),
            "left",
            2000,
        );
        let left_oid = create_loose_object(&objects_dir, left.as_bytes(), "commit");

        let right = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&root_oid.to_hex()),
            "right",
            2500,
        );
        let right_oid = create_loose_object(&objects_dir, right.as_bytes(), "commit");

        let merge = format!(
            "tree {}\nparent {}\nparent {}\nauthor Test <t@t.com> 3000 +0000\ncommitter Test <t@t.com> 3000 +0000\n\nmerge",
            tree_oid.to_hex(),
            left_oid.to_hex(),
            right_oid.to_hex()
        );
        let merge_oid = create_loose_object(&objects_dir, merge.as_bytes(), "commit");

        let log = LogIterator::new(objects_dir, merge_oid).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        // Exactly 4 unique commits
        assert_eq!(commits.len(), 4);
    }

    /// Helper to create commit content with specific author and timestamp.
    fn make_commit_content_with_author(
        tree_oid: &str,
        parent_oid: Option<&str>,
        message: &str,
        timestamp: i64,
        author_name: &str,
    ) -> String {
        let mut content = format!("tree {}\n", tree_oid);
        if let Some(parent) = parent_oid {
            content.push_str(&format!("parent {}\n", parent));
        }
        content.push_str(&format!(
            "author {} <{}@example.com> {} +0000\n",
            author_name,
            author_name.to_lowercase().replace(' ', "."),
            timestamp
        ));
        content.push_str(&format!(
            "committer {} <{}@example.com> {} +0000\n",
            author_name,
            author_name.to_lowercase().replace(' ', "."),
            timestamp
        ));
        content.push_str("\n");
        content.push_str(message);
        content
    }

    // LO-001: max_count limits results
    #[test]
    fn test_log_options_max_count() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        // Create 5 commits
        let c1 = make_commit_content_with_time(&tree_oid.to_hex(), None, "Commit 1", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "Commit 2", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "Commit 3", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c3_oid.to_hex()), "Commit 4", 4000);
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        let c5 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c4_oid.to_hex()), "Commit 5", 5000);
        let c5_oid = create_loose_object(&objects_dir, c5.as_bytes(), "commit");

        // Get only 3 commits
        let log = LogIterator::with_options(
            objects_dir,
            c5_oid,
            LogOptions::new().max_count(3),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].summary(), "Commit 5");
        assert_eq!(commits[1].summary(), "Commit 4");
        assert_eq!(commits[2].summary(), "Commit 3");
    }

    // LO-004: since filter
    #[test]
    fn test_log_options_since() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_time(&tree_oid.to_hex(), None, "Old commit", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "Middle commit", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "New commit", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits since timestamp 2000
        let log = LogIterator::with_options(
            objects_dir,
            c3_oid,
            LogOptions::new().since_timestamp(2000),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "New commit");
        assert_eq!(commits[1].summary(), "Middle commit");
    }

    // LO-005: until filter
    #[test]
    fn test_log_options_until() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_time(&tree_oid.to_hex(), None, "Old commit", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "Middle commit", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "New commit", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits until timestamp 2000
        let log = LogIterator::with_options(
            objects_dir,
            c3_oid,
            LogOptions::new().until_timestamp(2000),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Middle commit");
        assert_eq!(commits[1].summary(), "Old commit");
    }

    // LO-006: since + until (date range)
    #[test]
    fn test_log_options_date_range() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_time(&tree_oid.to_hex(), None, "Very old", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "In range", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "Also in range", 2500);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(&tree_oid.to_hex(), Some(&c3_oid.to_hex()), "Too new", 4000);
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Only commits in range [1500, 3000]
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new().since_timestamp(1500).until_timestamp(3000),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Also in range");
        assert_eq!(commits[1].summary(), "In range");
    }

    // LO-007: first_parent
    #[test]
    fn test_log_options_first_parent() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        // Create merge scenario:
        //       M
        //      / \
        //     B   C
        //      \ /
        //       A
        let a = make_commit_content_with_time(&tree_oid.to_hex(), None, "Root A", 1000);
        let a_oid = create_loose_object(&objects_dir, a.as_bytes(), "commit");

        let b = make_commit_content_with_time(&tree_oid.to_hex(), Some(&a_oid.to_hex()), "Branch B", 2000);
        let b_oid = create_loose_object(&objects_dir, b.as_bytes(), "commit");

        let c = make_commit_content_with_time(&tree_oid.to_hex(), Some(&a_oid.to_hex()), "Branch C", 2500);
        let c_oid = create_loose_object(&objects_dir, c.as_bytes(), "commit");

        let m = format!(
            "tree {}\nparent {}\nparent {}\nauthor Test <t@t.com> 3000 +0000\ncommitter Test <t@t.com> 3000 +0000\n\nMerge",
            tree_oid.to_hex(),
            b_oid.to_hex(),
            c_oid.to_hex()
        );
        let m_oid = create_loose_object(&objects_dir, m.as_bytes(), "commit");

        // With first_parent, should only follow B (first parent of M)
        let log = LogIterator::with_options(
            objects_dir,
            m_oid,
            LogOptions::new().first_parent(true),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].summary(), "Merge");
        assert_eq!(commits[1].summary(), "Branch B");
        assert_eq!(commits[2].summary(), "Root A");
        // Branch C should NOT be included
        assert!(commits.iter().all(|c| c.summary() != "Branch C"));
    }

    // LO-008: author filter
    #[test]
    fn test_log_options_author() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_author(&tree_oid.to_hex(), None, "By Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_author(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "By Bob", 2000, "Bob");
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_author(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "Also by Alice", 3000, "Alice");
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits by Alice
        let log = LogIterator::with_options(
            objects_dir,
            c3_oid,
            LogOptions::new().author("Alice"),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Also by Alice");
        assert_eq!(commits[1].summary(), "By Alice");
    }

    // LO-009: combined filters
    #[test]
    fn test_log_options_combined() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_author(&tree_oid.to_hex(), None, "Old Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_author(&tree_oid.to_hex(), Some(&c1_oid.to_hex()), "Recent Bob", 2000, "Bob");
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_author(&tree_oid.to_hex(), Some(&c2_oid.to_hex()), "Recent Alice", 3000, "Alice");
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_author(&tree_oid.to_hex(), Some(&c3_oid.to_hex()), "Very recent Alice", 4000, "Alice");
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Alice's commits since 2000, max 2
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new()
                .author("Alice")
                .since_timestamp(2000)
                .max_count(2),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Very recent Alice");
        assert_eq!(commits[1].summary(), "Recent Alice");
    }

    // LO-010: no matches
    #[test]
    fn test_log_options_no_matches() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree_oid = create_loose_object(&objects_dir, b"", "tree");

        let c1 = make_commit_content_with_author(&tree_oid.to_hex(), None, "By Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        // Look for Bob, but only Alice committed
        let log = LogIterator::with_options(
            objects_dir,
            c1_oid,
            LogOptions::new().author("Bob"),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 0);
    }

    // Test parse_date with YYYY-MM-DD format
    #[test]
    fn test_parse_date() {
        // Unix timestamp
        assert_eq!(parse_date("1704067200"), 1704067200);

        // YYYY-MM-DD (approximate)
        let ts = parse_date("2024-01-01");
        // Should be approximately Jan 1, 2024 in seconds since epoch
        // 2024 is 54 years after 1970, so roughly 54*365*86400 = ~1,703,376,000
        assert!(ts > 1_700_000_000 && ts < 1_710_000_000);

        // Invalid format returns 0
        assert_eq!(parse_date("invalid"), 0);
    }

    // Test LogOptions builder
    #[test]
    fn test_log_options_builder() {
        let options = LogOptions::new()
            .path("src/")
            .path("tests/")
            .max_count(10)
            .author("Alice")
            .first_parent(true);

        assert!(options.has_path_filter());
        assert_eq!(options.get_paths().len(), 2);
    }

    /// Helper to create a tree with file entries.
    fn create_tree_with_files(objects_dir: &std::path::Path, files: &[(&str, &[u8])]) -> Oid {
        let mut tree_content = Vec::new();
        for (name, content) in files {
            // Create blob for the file
            let blob_oid = create_loose_object(objects_dir, content, "blob");

            // Add entry: mode SP name NUL sha1
            tree_content.extend_from_slice(b"100644 ");
            tree_content.extend_from_slice(name.as_bytes());
            tree_content.push(0);
            tree_content.extend_from_slice(blob_oid.as_bytes());
        }
        create_loose_object(objects_dir, &tree_content, "tree")
    }

    // LO-002: path filter (single file)
    #[test]
    fn test_log_options_path_filter_single() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create different trees for each commit
        let tree1 = create_tree_with_files(&objects_dir, &[("README.md", b"v1")]);
        let tree2 = create_tree_with_files(&objects_dir, &[("README.md", b"v1"), ("main.rs", b"v1")]);
        let tree3 = create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v1")]);
        let tree4 = create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v2")]);

        // Commit 1: Initial with README.md
        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        // Commit 2: Add main.rs
        let c2 = make_commit_content_with_time(&tree2.to_hex(), Some(&c1_oid.to_hex()), "Add main.rs", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        // Commit 3: Update README.md
        let c3 = make_commit_content_with_time(&tree3.to_hex(), Some(&c2_oid.to_hex()), "Update README", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Commit 4: Update main.rs
        let c4 = make_commit_content_with_time(&tree4.to_hex(), Some(&c3_oid.to_hex()), "Update main.rs", 4000);
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by README.md - should return commits 1 and 3
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new().path("README.md"),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Update README");
        assert_eq!(commits[1].summary(), "Add README");
    }

    // LO-002: path filter (single file) - filter main.rs
    #[test]
    fn test_log_options_path_filter_another_file() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree1 = create_tree_with_files(&objects_dir, &[("README.md", b"v1")]);
        let tree2 = create_tree_with_files(&objects_dir, &[("README.md", b"v1"), ("main.rs", b"v1")]);
        let tree3 = create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v1")]);
        let tree4 = create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v2")]);

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree2.to_hex(), Some(&c1_oid.to_hex()), "Add main.rs", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree3.to_hex(), Some(&c2_oid.to_hex()), "Update README", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(&tree4.to_hex(), Some(&c3_oid.to_hex()), "Update main.rs", 4000);
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by main.rs - should return commits 2 and 4
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new().path("main.rs"),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Update main.rs");
        assert_eq!(commits[1].summary(), "Add main.rs");
    }

    // LO-003: multiple path filter
    #[test]
    fn test_log_options_path_filter_multiple() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree1 = create_tree_with_files(&objects_dir, &[("a.txt", b"v1")]);
        let tree2 = create_tree_with_files(&objects_dir, &[("a.txt", b"v1"), ("b.txt", b"v1")]);
        let tree3 = create_tree_with_files(&objects_dir, &[("a.txt", b"v1"), ("b.txt", b"v1"), ("c.txt", b"v1")]);

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add a.txt", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(&tree2.to_hex(), Some(&c1_oid.to_hex()), "Add b.txt", 2000);
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(&tree3.to_hex(), Some(&c2_oid.to_hex()), "Add c.txt", 3000);
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Filter by a.txt OR b.txt - should return commits 1 and 2
        let log = LogIterator::with_options(
            objects_dir,
            c3_oid,
            LogOptions::new().path("a.txt").path("b.txt"),
        ).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Add b.txt");
        assert_eq!(commits[1].summary(), "Add a.txt");
    }
}
