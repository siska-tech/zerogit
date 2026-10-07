//! Git commit log iteration.
//!
//! This module provides an iterator for traversing commit history in the
//! orders of `git log` ([`LogOrder`]): by default newest committer date
//! first, as Git walks it (returning commits as they are found), or with a
//! parent never before its children (`--date-order`, `--topo-order`),
//! optionally reversed. [`Graph`] lays out the columns of
//! `git log --graph` for the commits it is given.
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
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::objects::{Commit, ObjectStore, ObjectType, Oid, Tree};
use crate::repository::Repository;

/// The order in which [`LogIterator`] returns commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogOrder {
    /// Git's default: newest committer date first, as the history is
    /// walked (a commit may come before one of its children when the
    /// dates are skewed). Commits are returned as they are found, without
    /// reading the whole history first.
    #[default]
    Default,
    /// `--date-order`: no parent before all of its children, otherwise
    /// newest committer date first. Reads the whole history first.
    Date,
    /// `--topo-order`: no parent before all of its children, and the
    /// commits of one line of history together rather than interleaved by
    /// date (the order of `git log --graph`). Reads the whole history
    /// first.
    Topo,
}

/// A commit waiting in the walk: newest committer date first, and among
/// equal dates, the one queued first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued {
    timestamp: i64,
    sequence: u64,
    oid: Oid,
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        // A max-heap: a later date first, then an earlier sequence.
        self.timestamp
            .cmp(&other.timestamp)
            .then_with(|| other.sequence.cmp(&self.sequence))
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
    /// The order of the commits.
    order: LogOrder,
    /// Return the commits in reverse (oldest first).
    reverse: bool,
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

    /// Only include commits committed at or after this date (`--since`),
    /// by committer date as in Git. As in Git, the walk also stops at a
    /// commit older than this, so its parents are not shown even if their
    /// dates are skewed.
    ///
    /// # Arguments
    ///
    /// * `date` - A date string in YYYY-MM-DD format (midnight UTC) or a
    ///   Unix timestamp.
    pub fn since(mut self, date: &str) -> Self {
        self.since = Some(parse_date(date));
        self
    }

    /// Only include commits committed at or before this date (`--until`),
    /// by committer date as in Git.
    ///
    /// # Arguments
    ///
    /// * `date` - A date string in YYYY-MM-DD format (midnight UTC) or a
    ///   Unix timestamp.
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

    /// Sets the order of the commits ([`LogOrder`]; by default Git's).
    pub fn order(mut self, order: LogOrder) -> Self {
        self.order = order;
        self
    }

    /// Returns the commits oldest first (`--reverse`). As in Git, the
    /// order, the filters and [`LogOptions::max_count`] select the commits
    /// first, and then they are reversed, so the whole selection is read
    /// before the first commit is returned.
    pub fn reverse(mut self, reverse: bool) -> Self {
        self.reverse = reverse;
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
    if let [year, month, day] = parts[..] {
        if let (Ok(year), Ok(month @ 1..=12), Ok(day @ 1..=31)) = (
            year.parse::<i64>(),
            month.parse::<u32>(),
            day.parse::<u32>(),
        ) {
            return crate::infra::time::days_from_civil(year, month, day) * 86400;
        }
    }

    // Default to 0 if parsing fails
    0
}

/// An iterator over commits in the repository history.
///
/// Commits are yielded in the order of [`LogOptions::order`] (by default
/// Git's: newest committer date first, as the history is walked),
/// following all parents of merge commits (or only the first, with
/// [`LogOptions::first_parent`]).
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
    /// The object store for reading commits.
    store: ObjectStore,
    /// Commits waiting in the walk, by committer date.
    pending: BinaryHeap<Queued>,
    /// The commits of `pending`, read when they were queued.
    queued: HashMap<Oid, Commit>,
    /// Commits queued so far: each is walked once, as Git marks them seen.
    seen: HashSet<Oid>,
    /// Counter keeping commits with equal dates in the order queued.
    sequence: u64,
    /// For the orders that need the whole history, or `reverse`: the
    /// commits to return, in order.
    sorted: Option<VecDeque<Commit>>,
    /// Whether `sorted` is filtered (and limited) already.
    selected: bool,
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
        Self::with_store(ObjectStore::new(&objects_dir), start_oid, options)
    }

    /// Creates a LogIterator that reuses a repository's object store and pack indexes.
    pub(crate) fn with_store(
        store: ObjectStore,
        start_oid: Oid,
        options: LogOptions,
    ) -> Result<Self> {
        let mut log = LogIterator {
            store,
            pending: BinaryHeap::new(),
            queued: HashMap::new(),
            seen: HashSet::new(),
            sequence: 0,
            sorted: None,
            selected: false,
            options,
            count: 0,
        };
        log.enqueue(start_oid)?;
        Ok(log)
    }

    /// Queues a commit for the walk, unless it was queued before.
    fn enqueue(&mut self, oid: Oid) -> Result<()> {
        if !self.seen.insert(oid) {
            return Ok(());
        }
        let commit = self.read_commit(&oid)?;
        self.pending.push(Queued {
            timestamp: commit.committer().timestamp(),
            sequence: self.sequence,
            oid,
        });
        self.sequence += 1;
        self.queued.insert(oid, commit);
        Ok(())
    }

    /// The parents the walk follows: the first only with `first_parent`.
    fn walked_parents<'a>(&self, commit: &'a Commit) -> &'a [Oid] {
        let parents = commit.parents();
        if self.options.first_parent {
            &parents[..parents.len().min(1)]
        } else {
            parents
        }
    }

    /// The next commit of Git's default walk: the newest queued, whose
    /// parents are queued in turn.
    fn walk_next(&mut self) -> Result<Option<Commit>> {
        let Some(next) = self.pending.pop() else {
            return Ok(None);
        };
        let commit = self
            .queued
            .remove(&next.oid)
            .expect("queued commits are kept until walked");
        // As in Git's default walk, the walk does not go past a commit
        // older than `since`: its history is older still (dates
        // permitting). The sorted orders walk everything and leave that
        // history out afterwards.
        let too_old = self.options.order == LogOrder::Default
            && self
                .options
                .since
                .is_some_and(|since| next.timestamp < since);
        if !too_old {
            for parent in self.walked_parents(&commit).to_vec() {
                self.enqueue(parent)?;
            }
        }
        Ok(Some(commit))
    }

    /// Sorts the walked commits as Git's `sort_in_topological_order`
    /// does: a parent comes after all of its children; the commits ready
    /// to come next are taken by date (`Date`) or last ready first
    /// (`Topo`, which keeps a line of history together).
    fn topological(&self, walked: Vec<Commit>) -> VecDeque<Commit> {
        let index: HashMap<Oid, usize> = walked
            .iter()
            .enumerate()
            .map(|(i, commit)| (*commit.oid(), i))
            .collect();
        // One plus the number of children among the walked commits; 0 once
        // the commit is out.
        let mut indegree = vec![1usize; walked.len()];
        for commit in &walked {
            for parent in self.walked_parents(commit) {
                if let Some(&i) = index.get(parent) {
                    indegree[i] += 1;
                }
            }
        }
        let by_date = self.options.order == LogOrder::Date;
        let mut stack: Vec<usize> = Vec::new();
        let mut heap: BinaryHeap<Queued> = BinaryHeap::new();
        let mut sequence = 0;
        let mut put = |i: usize, stack: &mut Vec<usize>, heap: &mut BinaryHeap<Queued>| {
            if by_date {
                heap.push(Queued {
                    timestamp: walked[i].committer().timestamp(),
                    sequence,
                    oid: *walked[i].oid(),
                });
                sequence += 1;
            } else {
                stack.push(i);
            }
        };
        // The tips, in walk order.
        let tips: Vec<usize> = (0..walked.len()).filter(|&i| indegree[i] == 1).collect();
        for i in tips {
            put(i, &mut stack, &mut heap);
        }
        // The first tip is taken first.
        stack.reverse();

        let mut order = Vec::with_capacity(walked.len());
        loop {
            let i = if by_date {
                match heap.pop() {
                    Some(next) => index[&next.oid],
                    None => break,
                }
            } else {
                match stack.pop() {
                    Some(i) => i,
                    None => break,
                }
            };
            for parent in self.walked_parents(&walked[i]) {
                let Some(&p) = index.get(parent) else {
                    continue;
                };
                if indegree[p] == 0 {
                    continue;
                }
                indegree[p] -= 1;
                // Ready once all its children are out.
                if indegree[p] == 1 {
                    put(p, &mut stack, &mut heap);
                }
            }
            indegree[i] = 0;
            order.push(i);
        }
        let mut slots: Vec<Option<Commit>> = walked.into_iter().map(Some).collect();
        order
            .into_iter()
            .map(|i| slots[i].take().expect("each commit comes out once"))
            .collect()
    }

    /// The next commit in the chosen order, before filtering.
    fn next_ordered(&mut self) -> Result<Option<Commit>> {
        if self.options.order == LogOrder::Default {
            return self.walk_next();
        }
        if self.sorted.is_none() {
            let mut walked = Vec::new();
            while let Some(commit) = self.walk_next()? {
                walked.push(commit);
            }
            // As Git's limited walk does, a commit older than `since`
            // makes its whole history uninteresting, even commits within
            // the dates that are also reached another way.
            let mut excluded: HashSet<Oid> = HashSet::new();
            if let Some(since) = self.options.since {
                let parents: HashMap<Oid, Vec<Oid>> = walked
                    .iter()
                    .map(|c| (*c.oid(), self.walked_parents(c).to_vec()))
                    .collect();
                let mut stack: Vec<Oid> = walked
                    .iter()
                    .filter(|c| c.committer().timestamp() < since)
                    .map(|c| *c.oid())
                    .collect();
                while let Some(oid) = stack.pop() {
                    if excluded.insert(oid) {
                        stack.extend(parents.get(&oid).into_iter().flatten().copied());
                    }
                }
            }
            // Commits outside the dates are left out before sorting, not
            // after, as in Git.
            walked.retain(|c| !excluded.contains(c.oid()) && self.within_dates(c));
            self.sorted = Some(self.topological(walked));
        }
        Ok(self.sorted.as_mut().and_then(VecDeque::pop_front))
    }

    /// The next commit that passes the filters, within `max_count`.
    fn next_selected(&mut self) -> Result<Option<Commit>> {
        if let Some(max) = self.options.max_count {
            if self.count >= max {
                return Ok(None);
            }
        }
        while let Some(commit) = self.next_ordered()? {
            if self.passes_filters(&commit)? {
                self.count += 1;
                return Ok(Some(commit));
            }
        }
        Ok(None)
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

    /// Flattens a tree into a map of path -> OID.
    ///
    /// Recursively traverses the tree structure to get all file paths.
    fn flatten_tree_for_diff(
        &self,
        tree: &Tree,
        prefix: PathBuf,
    ) -> Result<std::collections::HashMap<PathBuf, Oid>> {
        let mut result = std::collections::HashMap::new();

        for entry in tree.entries() {
            let path = if prefix.as_os_str().is_empty() {
                PathBuf::from(entry.name())
            } else {
                prefix.join(entry.name())
            };

            if entry.is_directory() {
                // Recursively flatten subtree
                let subtree = self.read_tree(entry.oid())?;
                result.extend(self.flatten_tree_for_diff(&subtree, path)?);
            } else {
                // File entry
                result.insert(path, *entry.oid());
            }
        }

        Ok(result)
    }

    /// Checks if a path matches any of the configured filter paths.
    ///
    /// A path matches if:
    /// - It exactly matches a filter path
    /// - It starts with a filter path (filter path is a directory prefix)
    fn path_matches_filter(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        for filter_path in &self.options.paths {
            let filter_str = filter_path.to_string_lossy();
            // Normalize to forward slashes for comparison
            let path_normalized = path_str.replace('\\', "/");
            let filter_normalized = filter_str.replace('\\', "/");

            // Exact match
            if path_normalized == filter_normalized {
                return true;
            }

            // Directory prefix match: filter ends with "/" or path starts with filter + "/"
            if filter_normalized.ends_with('/') {
                if path_normalized.starts_with(&filter_normalized) {
                    return true;
                }
            } else {
                // Check if filter is a directory prefix (path starts with filter/)
                let prefix = format!("{}/", filter_normalized);
                if path_normalized.starts_with(&prefix) {
                    return true;
                }
            }
        }
        false
    }

    /// Checks if a commit touches any of the configured filter paths.
    ///
    /// A commit "touches" a path if any file under that path differs between
    /// the commit's tree and its parent's tree. This properly handles:
    /// - Exact file paths (e.g., "src/lib.rs")
    /// - Directory prefixes (e.g., "src/" matches all files under src)
    /// - Nested subdirectories (e.g., "src/utils/helpers/mod.rs")
    fn commit_touches_paths(&self, commit: &Commit) -> Result<bool> {
        let current_tree = self.read_tree(commit.tree())?;
        let current_map = self.flatten_tree_for_diff(&current_tree, PathBuf::new())?;

        // Get parent tree map (empty if no parent)
        let parent_map = if let Some(parent_oid) = commit.parents().first() {
            let parent_commit = self.read_commit(parent_oid)?;
            let parent_tree = self.read_tree(parent_commit.tree())?;
            self.flatten_tree_for_diff(&parent_tree, PathBuf::new())?
        } else {
            std::collections::HashMap::new()
        };

        // Collect all paths from both trees
        let mut all_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        all_paths.extend(current_map.keys().cloned());
        all_paths.extend(parent_map.keys().cloned());

        // Check if any changed path matches our filter
        for path in all_paths {
            let current_oid = current_map.get(&path);
            let parent_oid = parent_map.get(&path);

            // If the path changed (added, deleted, or modified)
            if current_oid != parent_oid {
                // Check if this path matches any of our filter paths
                if self.path_matches_filter(&path) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Whether a commit's committer date is within `since` and `until`,
    /// the dates Git compares.
    fn within_dates(&self, commit: &Commit) -> bool {
        let timestamp = commit.committer().timestamp();
        self.options.since.map_or(true, |since| timestamp >= since)
            && self.options.until.map_or(true, |until| timestamp <= until)
    }

    /// Checks if a commit passes all configured filters.
    fn passes_filters(&self, commit: &Commit) -> Result<bool> {
        if !self.within_dates(commit) {
            return Ok(false);
        }

        // Check author filter
        if let Some(ref author) = self.options.author {
            if !commit.author().name().contains(author) {
                return Ok(false);
            }
        }

        // Check path filter
        if self.options.has_path_filter() && !self.commit_touches_paths(commit)? {
            return Ok(false);
        }

        Ok(true)
    }
}

impl Iterator for LogIterator {
    type Item = Result<Commit>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.options.reverse && !self.selected {
            // Select everything first, then return it oldest first.
            let mut selection = VecDeque::new();
            loop {
                match self.next_selected() {
                    Ok(Some(commit)) => selection.push_front(commit),
                    Ok(None) => break,
                    Err(e) => return Some(Err(e)),
                }
            }
            self.sorted = Some(selection);
            self.selected = true;
        }
        if self.selected {
            return self.sorted.as_mut().and_then(VecDeque::pop_front).map(Ok);
        }
        self.next_selected().transpose()
    }
}

/// One commit's row of a history graph (see [`Graph`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    commit: Oid,
    column: usize,
    before: Vec<Oid>,
    after: Vec<Oid>,
    parents: Vec<Oid>,
}

impl GraphRow {
    /// The commit.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// The column the commit is drawn in (`*` at character `2 * column` in
    /// `git log --graph`).
    pub fn column(&self) -> usize {
        self.column
    }

    /// The lines of history on this row: the commit each column leads to,
    /// left to right. The commit's own column is past the end when it
    /// starts a new line (a branch tip).
    pub fn columns_before(&self) -> &[Oid] {
        &self.before
    }

    /// The lines of history below this row, for the next one: the commit's
    /// column continues to its parents, and lines leading to the same
    /// commit are joined into the leftmost.
    pub fn columns_after(&self) -> &[Oid] {
        &self.after
    }

    /// For each parent given for the commit, the column of
    /// [`GraphRow::columns_after`] its line continues in.
    pub fn parent_columns(&self) -> Vec<usize> {
        self.parents
            .iter()
            .map(|parent| {
                self.after
                    .iter()
                    .position(|oid| oid == parent)
                    .expect("every parent has a column after its child")
            })
            .collect()
    }

    /// Where each column of [`GraphRow::columns_before`] (other than the
    /// commit's) goes in [`GraphRow::columns_after`]: lines are drawn from
    /// column `i` above to `mapping[i]` below.
    pub fn column_mapping(&self) -> Vec<Option<usize>> {
        self.before
            .iter()
            .map(|oid| {
                if *oid == self.commit {
                    None
                } else {
                    self.after.iter().position(|after| after == oid)
                }
            })
            .collect()
    }
}

/// Lays out a history graph as `git log --graph` does, without drawing
/// it: for each commit, given in the order shown (use [`LogOrder::Topo`],
/// as `--graph` does), the column it goes in and how the lines of history
/// continue to its parents. Columns are assigned as Git's `graph.c` does,
/// so a drawing built from the rows has the shape of Git's.
///
/// # Examples
///
/// ```no_run
/// use zerogit::log::{Graph, LogOptions, LogOrder};
/// use zerogit::Repository;
///
/// let repo = Repository::open("path/to/repo").unwrap();
/// let mut graph = Graph::new();
/// for commit in repo.log_with_options(LogOptions::new().order(LogOrder::Topo)).unwrap() {
///     let commit = commit.unwrap();
///     let row = graph.push(*commit.oid(), commit.parents());
///     let lane: String = (0..row.columns_before().len().max(row.column() + 1))
///         .map(|i| if i == row.column() { "* " } else { "| " })
///         .collect();
///     println!("{}{}", lane, commit.summary());
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Graph {
    columns: Vec<Oid>,
}

impl Graph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the next commit with the parents its lines lead to (all of
    /// them, or only the first for a first-parent history), and returns
    /// its row.
    pub fn push(&mut self, commit: Oid, parents: &[Oid]) -> GraphRow {
        let before = std::mem::take(&mut self.columns);
        let mut after: Vec<Oid> = Vec::with_capacity(before.len() + parents.len());
        let insert = |after: &mut Vec<Oid>, oid: Oid| {
            if !after.contains(&oid) {
                after.push(oid);
            }
        };
        let mut column = None;
        // The existing lines in order; the commit's own line is replaced by
        // its parents. A commit no line leads to starts one at the end.
        for i in 0..=before.len() {
            let line = match before.get(i) {
                Some(oid) => *oid,
                None if column.is_some() => break,
                None => commit,
            };
            if line == commit {
                column = Some(i);
                for parent in parents {
                    insert(&mut after, *parent);
                }
            } else {
                insert(&mut after, line);
            }
        }
        self.columns = after.clone();
        GraphRow {
            commit,
            column: column.expect("the commit gets a column"),
            before,
            after,
            parents: parents.to_vec(),
        }
    }
}

impl Repository {
    /// Returns an iterator over the commit history starting from HEAD.
    ///
    /// Commits are returned as `git log` returns them: newest committer
    /// date first, as the history is walked.
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
    /// Commits are returned as `git log` returns them: newest committer
    /// date first, as the history is walked.
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
        LogIterator::with_store(self.object_store(), start_oid, LogOptions::default())
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
        LogIterator::with_store(self.object_store(), start_oid, options)
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
        content.push('\n');
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
        content.push('\n');
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

        let c2 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Commit 2",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Commit 3",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c3_oid.to_hex()),
            "Commit 4",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        let c5 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c4_oid.to_hex()),
            "Commit 5",
            5000,
        );
        let c5_oid = create_loose_object(&objects_dir, c5.as_bytes(), "commit");

        // Get only 3 commits
        let log =
            LogIterator::with_options(objects_dir, c5_oid, LogOptions::new().max_count(3)).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].summary(), "Commit 5");
        assert_eq!(commits[1].summary(), "Commit 4");
        assert_eq!(commits[2].summary(), "Commit 3");
    }

    #[test]
    fn test_parse_date_is_midnight_utc() {
        assert_eq!(parse_date("1970-01-01"), 0);
        // Before and after a leap day.
        assert_eq!(parse_date("2024-02-28"), 1_709_078_400);
        assert_eq!(parse_date("2024-03-01"), 1_709_251_200);
        assert_eq!(parse_date("2023-03-01"), 1_677_628_800);
        assert_eq!(parse_date("1234567890"), 1_234_567_890);
        assert_eq!(parse_date("2024-13-01"), 0);
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

        let c2 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Middle commit",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "New commit",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits since timestamp 2000
        let log =
            LogIterator::with_options(objects_dir, c3_oid, LogOptions::new().since_timestamp(2000))
                .unwrap();
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

        let c2 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Middle commit",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "New commit",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits until timestamp 2000
        let log =
            LogIterator::with_options(objects_dir, c3_oid, LogOptions::new().until_timestamp(2000))
                .unwrap();
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

        let c2 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "In range",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Also in range",
            2500,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&c3_oid.to_hex()),
            "Too new",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Only commits in range [1500, 3000]
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new()
                .since_timestamp(1500)
                .until_timestamp(3000),
        )
        .unwrap();
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

        let b = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&a_oid.to_hex()),
            "Branch B",
            2000,
        );
        let b_oid = create_loose_object(&objects_dir, b.as_bytes(), "commit");

        let c = make_commit_content_with_time(
            &tree_oid.to_hex(),
            Some(&a_oid.to_hex()),
            "Branch C",
            2500,
        );
        let c_oid = create_loose_object(&objects_dir, c.as_bytes(), "commit");

        let m = format!(
            "tree {}\nparent {}\nparent {}\nauthor Test <t@t.com> 3000 +0000\ncommitter Test <t@t.com> 3000 +0000\n\nMerge",
            tree_oid.to_hex(),
            b_oid.to_hex(),
            c_oid.to_hex()
        );
        let m_oid = create_loose_object(&objects_dir, m.as_bytes(), "commit");

        // With first_parent, should only follow B (first parent of M)
        let log =
            LogIterator::with_options(objects_dir, m_oid, LogOptions::new().first_parent(true))
                .unwrap();
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

        let c1 =
            make_commit_content_with_author(&tree_oid.to_hex(), None, "By Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_author(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "By Bob",
            2000,
            "Bob",
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_author(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Also by Alice",
            3000,
            "Alice",
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Only commits by Alice
        let log = LogIterator::with_options(objects_dir, c3_oid, LogOptions::new().author("Alice"))
            .unwrap();
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

        let c1 =
            make_commit_content_with_author(&tree_oid.to_hex(), None, "Old Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_author(
            &tree_oid.to_hex(),
            Some(&c1_oid.to_hex()),
            "Recent Bob",
            2000,
            "Bob",
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_author(
            &tree_oid.to_hex(),
            Some(&c2_oid.to_hex()),
            "Recent Alice",
            3000,
            "Alice",
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_author(
            &tree_oid.to_hex(),
            Some(&c3_oid.to_hex()),
            "Very recent Alice",
            4000,
            "Alice",
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Alice's commits since 2000, max 2
        let log = LogIterator::with_options(
            objects_dir,
            c4_oid,
            LogOptions::new()
                .author("Alice")
                .since_timestamp(2000)
                .max_count(2),
        )
        .unwrap();
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

        let c1 =
            make_commit_content_with_author(&tree_oid.to_hex(), None, "By Alice", 1000, "Alice");
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        // Look for Bob, but only Alice committed
        let log = LogIterator::with_options(objects_dir, c1_oid, LogOptions::new().author("Bob"))
            .unwrap();
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
        let tree2 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v1"), ("main.rs", b"v1")]);
        let tree3 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v1")]);
        let tree4 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v2")]);

        // Commit 1: Initial with README.md
        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        // Commit 2: Add main.rs
        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add main.rs",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        // Commit 3: Update README.md
        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Update README",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Commit 4: Update main.rs
        let c4 = make_commit_content_with_time(
            &tree4.to_hex(),
            Some(&c3_oid.to_hex()),
            "Update main.rs",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by README.md - should return commits 1 and 3
        let log =
            LogIterator::with_options(objects_dir, c4_oid, LogOptions::new().path("README.md"))
                .unwrap();
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
        let tree2 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v1"), ("main.rs", b"v1")]);
        let tree3 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v1")]);
        let tree4 =
            create_tree_with_files(&objects_dir, &[("README.md", b"v2"), ("main.rs", b"v2")]);

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add main.rs",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Update README",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree4.to_hex(),
            Some(&c3_oid.to_hex()),
            "Update main.rs",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by main.rs - should return commits 2 and 4
        let log = LogIterator::with_options(objects_dir, c4_oid, LogOptions::new().path("main.rs"))
            .unwrap();
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
        let tree3 = create_tree_with_files(
            &objects_dir,
            &[("a.txt", b"v1"), ("b.txt", b"v1"), ("c.txt", b"v1")],
        );

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add a.txt", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add b.txt",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Add c.txt",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        // Filter by a.txt OR b.txt - should return commits 1 and 2
        let log = LogIterator::with_options(
            objects_dir,
            c3_oid,
            LogOptions::new().path("a.txt").path("b.txt"),
        )
        .unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Add b.txt");
        assert_eq!(commits[1].summary(), "Add a.txt");
    }

    /// Helper to create a tree with nested directory structure.
    fn create_nested_tree(objects_dir: &std::path::Path, files: &[(&str, &[u8])]) -> Oid {
        use std::collections::BTreeMap;

        // Build directory structure
        let mut root_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut subdirs: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();

        for (path, content) in files {
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() == 1 {
                // Root level file
                let blob_oid = create_loose_object(objects_dir, content, "blob");
                let mut entry = Vec::new();
                entry.extend_from_slice(b"100644 ");
                entry.extend_from_slice(parts[0].as_bytes());
                entry.push(0);
                entry.extend_from_slice(blob_oid.as_bytes());
                root_entries.insert(parts[0].to_string(), entry);
            } else {
                // Nested file - for simplicity, handle one level of nesting
                let dir_name = parts[0];
                let file_name = parts[1..].join("/");
                let blob_oid = create_loose_object(objects_dir, content, "blob");

                let subdir = subdirs.entry(dir_name.to_string()).or_default();
                let mut entry = Vec::new();
                entry.extend_from_slice(b"100644 ");
                entry.extend_from_slice(file_name.as_bytes());
                entry.push(0);
                entry.extend_from_slice(blob_oid.as_bytes());
                subdir.insert(file_name, entry);
            }
        }

        // Create subtrees and add them to root
        for (dir_name, entries) in subdirs {
            let mut subtree_content = Vec::new();
            for (_, entry) in entries {
                subtree_content.extend(entry);
            }
            let subtree_oid = create_loose_object(objects_dir, &subtree_content, "tree");

            let mut entry = Vec::new();
            entry.extend_from_slice(b"40000 ");
            entry.extend_from_slice(dir_name.as_bytes());
            entry.push(0);
            entry.extend_from_slice(subtree_oid.as_bytes());
            root_entries.insert(dir_name, entry);
        }

        // Build root tree
        let mut tree_content = Vec::new();
        for (_, entry) in root_entries {
            tree_content.extend(entry);
        }
        create_loose_object(objects_dir, &tree_content, "tree")
    }

    // LO-011: path filter with subdirectory file
    #[test]
    fn test_log_options_path_filter_subdirectory_file() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create trees with files in src/ directory
        let tree1 = create_nested_tree(&objects_dir, &[("README.md", b"v1")]);
        let tree2 =
            create_nested_tree(&objects_dir, &[("README.md", b"v1"), ("src/lib.rs", b"v1")]);
        let tree3 =
            create_nested_tree(&objects_dir, &[("README.md", b"v2"), ("src/lib.rs", b"v1")]);
        let tree4 =
            create_nested_tree(&objects_dir, &[("README.md", b"v2"), ("src/lib.rs", b"v2")]);

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add src/lib.rs",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Update README",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree4.to_hex(),
            Some(&c3_oid.to_hex()),
            "Update src/lib.rs",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by src/lib.rs - should return commits 2 and 4
        let log =
            LogIterator::with_options(objects_dir, c4_oid, LogOptions::new().path("src/lib.rs"))
                .unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Update src/lib.rs");
        assert_eq!(commits[1].summary(), "Add src/lib.rs");
    }

    // LO-012: path filter with directory prefix
    #[test]
    fn test_log_options_path_filter_directory_prefix() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create trees with files in src/ directory
        let tree1 = create_nested_tree(&objects_dir, &[("README.md", b"v1")]);
        let tree2 =
            create_nested_tree(&objects_dir, &[("README.md", b"v1"), ("src/lib.rs", b"v1")]);
        let tree3 =
            create_nested_tree(&objects_dir, &[("README.md", b"v2"), ("src/lib.rs", b"v1")]);
        let tree4 = create_nested_tree(
            &objects_dir,
            &[
                ("README.md", b"v2"),
                ("src/lib.rs", b"v1"),
                ("src/main.rs", b"v1"),
            ],
        );

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add src/lib.rs",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Update README only",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree4.to_hex(),
            Some(&c3_oid.to_hex()),
            "Add src/main.rs",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by "src/" - should return commits 2 and 4 (not 3 which only changed README)
        let log =
            LogIterator::with_options(objects_dir, c4_oid, LogOptions::new().path("src/")).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Add src/main.rs");
        assert_eq!(commits[1].summary(), "Add src/lib.rs");
    }

    // LO-013: path filter with directory (without trailing slash)
    #[test]
    fn test_log_options_path_filter_directory_no_slash() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let tree1 = create_nested_tree(&objects_dir, &[("README.md", b"v1")]);
        let tree2 =
            create_nested_tree(&objects_dir, &[("README.md", b"v1"), ("src/lib.rs", b"v1")]);
        let tree3 =
            create_nested_tree(&objects_dir, &[("README.md", b"v2"), ("src/lib.rs", b"v1")]);
        let tree4 =
            create_nested_tree(&objects_dir, &[("README.md", b"v2"), ("src/lib.rs", b"v2")]);

        let c1 = make_commit_content_with_time(&tree1.to_hex(), None, "Add README", 1000);
        let c1_oid = create_loose_object(&objects_dir, c1.as_bytes(), "commit");

        let c2 = make_commit_content_with_time(
            &tree2.to_hex(),
            Some(&c1_oid.to_hex()),
            "Add src/lib.rs",
            2000,
        );
        let c2_oid = create_loose_object(&objects_dir, c2.as_bytes(), "commit");

        let c3 = make_commit_content_with_time(
            &tree3.to_hex(),
            Some(&c2_oid.to_hex()),
            "Update README only",
            3000,
        );
        let c3_oid = create_loose_object(&objects_dir, c3.as_bytes(), "commit");

        let c4 = make_commit_content_with_time(
            &tree4.to_hex(),
            Some(&c3_oid.to_hex()),
            "Update src/lib.rs",
            4000,
        );
        let c4_oid = create_loose_object(&objects_dir, c4.as_bytes(), "commit");

        // Filter by "src" (without slash) - should also match files under src/
        let log =
            LogIterator::with_options(objects_dir, c4_oid, LogOptions::new().path("src")).unwrap();
        let commits: Vec<_> = log.filter_map(Result::ok).collect();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary(), "Update src/lib.rs");
        assert_eq!(commits[1].summary(), "Add src/lib.rs");
    }
}

#[cfg(test)]
mod repository_tests {
    use crate::repository::test_support::*;
    use crate::repository::Repository;
    use std::fs;
    use tempfile::TempDir;

    // =========================================================================
    // Log iterator tests (RP-014 to RP-016)
    // =========================================================================

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
}
