//! Git index (staging area) operations.
//!
//! The index file (`.git/index`) is a binary file that acts as a staging
//! area between the working tree and the repository.

pub(crate) mod cache_tree;
mod reader;
pub(crate) mod resolve_undo;
mod writer;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cache_tree::CacheTree;
use resolve_undo::Stages;

use crate::objects::tree::FileMode;
use crate::objects::Oid;

pub use reader::parse;
pub use writer::write;

/// A Git index (staging area).
///
/// The index contains information about the files that will be included
/// in the next commit.
#[derive(Debug, Clone)]
pub struct Index {
    /// Index file format version (2, 3, or 4).
    version: u32,
    /// The entries in the index.
    entries: Vec<IndexEntry>,
    /// The cache tree (`TREE` extension), if the index has one.
    cache_tree: Option<CacheTree>,
    /// Resolved conflicts (`REUC` extension), by `/`-separated path.
    resolve_undo: BTreeMap<Vec<u8>, Stages>,
}

impl Index {
    /// Creates a new empty index with the given version.
    pub fn empty(version: u32) -> Self {
        Self {
            version,
            entries: Vec::new(),
            cache_tree: None,
            resolve_undo: BTreeMap::new(),
        }
    }

    /// Creates a new Index from parsed data.
    pub(crate) fn new(version: u32, entries: Vec<IndexEntry>) -> Self {
        Self {
            version,
            entries,
            cache_tree: None,
            resolve_undo: BTreeMap::new(),
        }
    }

    /// Returns the index format version.
    ///
    /// Git currently supports versions 2, 3, and 4.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns the number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the index has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns a slice of all entries in the index.
    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// Finds an entry by path.
    ///
    /// For a conflicted path (stages 1-3) this returns the entry with the
    /// lowest stage; use [`Index::get_stage`] to pick a specific stage.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to search for.
    ///
    /// # Returns
    ///
    /// The entry if found, or `None` if not found.
    pub fn get(&self, path: &Path) -> Option<&IndexEntry> {
        let range = self.path_range(path);
        self.entries[range].first()
    }

    /// Finds the entry for a path at a specific stage (0 for a normal entry,
    /// 1-3 for the base, ours and theirs sides of a conflict).
    pub fn get_stage(&self, path: &Path, stage: u8) -> Option<&IndexEntry> {
        let range = self.path_range(path);
        self.entries[range].iter().find(|e| e.stage == stage)
    }

    /// Returns an iterator over the entries.
    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter()
    }

    /// Returns true if any entry is in a merge conflict (stage 1-3).
    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(IndexEntry::is_conflicted)
    }

    /// Returns the paths that are in a merge conflict, each once, in index order.
    pub fn conflicted_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in self.entries.iter().filter(|e| e.is_conflicted()) {
            if paths.last().map(PathBuf::as_path) != Some(entry.path()) {
                paths.push(entry.path.clone());
            }
        }
        paths
    }

    /// Adds or updates an entry in the index.
    ///
    /// Adding a stage 0 entry replaces every entry for the same path, which
    /// resolves a conflict the way `git add` does. Adding a conflict stage
    /// (1-3) replaces the stage 0 entry and the entry with the same stage.
    /// Entries are kept in Git's order: by path bytes, then by stage.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry to add or update.
    pub fn add(&mut self, entry: IndexEntry) {
        self.invalidate(&entry.path);
        let range = self.path_range(&entry.path);
        let start = range.start;
        let stage = entry.stage;
        let mut end = range.end;
        if stage == 0 {
            // Staging a conflicted path resolves it; remember the stages.
            self.record_resolve_undo(start..end);
        }
        let mut i = start;
        while i < end {
            let existing = self.entries[i].stage;
            if stage == 0 || existing == 0 || existing == stage {
                self.entries.remove(i);
                end -= 1;
            } else {
                i += 1;
            }
        }
        let pos = start + self.entries[start..end].partition_point(|e| e.stage < stage);
        self.entries.insert(pos, entry);
    }

    /// Removes every entry for a path, including all conflict stages.
    ///
    /// # Arguments
    ///
    /// * `path` - The path of the entry to remove.
    ///
    /// # Returns
    ///
    /// `true` if an entry was removed, `false` if no entry was found.
    pub fn remove(&mut self, path: &Path) -> bool {
        let range = self.path_range(path);
        let removed = !range.is_empty();
        if removed {
            self.invalidate(path);
            // Removing a conflicted path resolves it by deletion.
            self.record_resolve_undo(range.clone());
        }
        self.entries.drain(range);
        removed
    }

    /// Replaces the stage 0 entry at `entry`'s path with `entry`, which has
    /// the same blob and mode but new stat data. Unlike [`Index::add`], the
    /// cache tree stays valid: the content did not change.
    pub(crate) fn refresh(&mut self, entry: IndexEntry) {
        let range = self.path_range(&entry.path);
        if let Some(existing) = self.entries[range].iter_mut().find(|e| e.stage == 0) {
            debug_assert!(existing.oid == entry.oid && existing.mode == entry.mode);
            let (skip_worktree, intent_to_add) = (existing.skip_worktree, existing.intent_to_add);
            *existing = entry;
            existing.skip_worktree = skip_worktree;
            existing.intent_to_add = intent_to_add;
        }
    }

    /// Marks the cached trees of the directories leading to `path` as
    /// changed.
    fn invalidate(&mut self, path: &Path) {
        if let Some(tree) = &mut self.cache_tree {
            tree.invalidate(&path_key(path));
        }
    }

    /// Records the conflict stages among `range` (the entries of one path)
    /// in the resolve-undo data, as Git does when a conflict is resolved.
    fn record_resolve_undo(&mut self, range: std::ops::Range<usize>) {
        let mut stages: Stages = [None; 3];
        let mut path = None;
        for entry in &self.entries[range] {
            if (1..=3).contains(&entry.stage) {
                let mode = u32::from_str_radix(entry.mode.as_octal(), 8).unwrap_or(0o100644);
                stages[usize::from(entry.stage) - 1] = Some((mode, entry.oid));
                path = Some(path_key(&entry.path));
            }
        }
        if let Some(path) = path {
            self.resolve_undo.insert(path, stages);
        }
    }

    /// The cache tree (`TREE` extension), if any.
    pub(crate) fn cache_tree(&self) -> Option<&CacheTree> {
        self.cache_tree.as_ref()
    }

    /// Replaces the cache tree.
    pub(crate) fn set_cache_tree(&mut self, tree: Option<CacheTree>) {
        self.cache_tree = tree;
    }

    /// The resolved conflicts (`REUC` extension), by `/`-separated path.
    pub(crate) fn resolve_undo(&self) -> &BTreeMap<Vec<u8>, Stages> {
        &self.resolve_undo
    }

    /// Forgets the resolved conflicts, as Git does once they are committed
    /// or reset.
    pub(crate) fn clear_resolve_undo(&mut self) {
        self.resolve_undo.clear();
    }

    /// Sets the extensions read from an index file.
    pub(crate) fn with_extensions(
        mut self,
        cache_tree: Option<CacheTree>,
        resolve_undo: BTreeMap<Vec<u8>, Stages>,
    ) -> Self {
        self.cache_tree = cache_tree;
        self.resolve_undo = resolve_undo;
        self
    }

    /// The range of entries for a path, located by binary search in Git's
    /// index order. Falls back to a linear scan for an index that is not
    /// sorted (Git never writes one, but parsing does not reject it).
    fn path_range(&self, path: &Path) -> std::ops::Range<usize> {
        let key = path_key(path);
        let start = self
            .entries
            .partition_point(|e| path_key(&e.path).as_slice() < key.as_slice());
        let end = start
            + self.entries[start..]
                .iter()
                .take_while(|e| path_key(&e.path) == key)
                .count();
        if start < end {
            return start..end;
        }
        match self.entries.iter().position(|e| e.path == path) {
            Some(pos) => pos..pos + 1,
            None => start..start,
        }
    }

    /// Clears all entries from the index.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cache_tree = None;
        self.resolve_undo.clear();
    }

    /// Smudges the entries of files modified at or after `now` (seconds
    /// since the epoch, taken just before the index is written), as Git
    /// does when writing an index: their size is set to 0 so that the next
    /// reader compares their content instead of trusting the stat data.
    ///
    /// Such a file may change again within the timestamp granularity
    /// without its stat data changing. The new index file is not older than
    /// `now`, so readers already treat these entries as racy; smudging
    /// keeps them so after the index is rewritten later.
    pub(crate) fn smudge_racy_entries(&mut self, now: u64) {
        for entry in &mut self.entries {
            if entry.stage == 0 && entry.mtime as u32 >= now as u32 {
                entry.size = 0;
            }
        }
    }
}

/// The `/`-separated path bytes Git sorts index entries by.
pub(crate) fn path_key(path: &Path) -> Vec<u8> {
    path.to_string_lossy().replace('\\', "/").into_bytes()
}

/// An entry in the Git index.
///
/// Each entry represents a file that is staged for the next commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The ctime (metadata change time) in seconds since epoch.
    ctime: u64,
    /// The mtime (modification time) in seconds since epoch.
    mtime: u64,
    /// The device ID.
    dev: u32,
    /// The inode number.
    ino: u32,
    /// The file mode.
    mode: FileMode,
    /// The user ID.
    uid: u32,
    /// The group ID.
    gid: u32,
    /// The file size in bytes.
    size: u32,
    /// The object ID (SHA-1 hash) of the blob.
    oid: Oid,
    /// The path of the file relative to the repository root.
    path: PathBuf,
    /// Stage number (0 for normal, 1-3 for merge conflicts).
    stage: u8,
    /// Nanosecond parts of ctime and mtime, kept so rewriting the index does not
    /// lose precision Git relies on to detect changes.
    ctime_nsec: u32,
    mtime_nsec: u32,
    /// Extended flags (index v3+).
    skip_worktree: bool,
    intent_to_add: bool,
}

impl IndexEntry {
    /// Creates a new IndexEntry.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctime: u64,
        mtime: u64,
        dev: u32,
        ino: u32,
        mode: FileMode,
        uid: u32,
        gid: u32,
        size: u32,
        oid: Oid,
        path: PathBuf,
        stage: u8,
    ) -> Self {
        Self {
            ctime,
            mtime,
            dev,
            ino,
            mode,
            uid,
            gid,
            size,
            oid,
            path,
            stage,
            ctime_nsec: 0,
            mtime_nsec: 0,
            skip_worktree: false,
            intent_to_add: false,
        }
    }

    /// Returns the ctime (metadata change time) in seconds since epoch.
    pub fn ctime(&self) -> u64 {
        self.ctime
    }

    /// Returns the mtime (modification time) in seconds since epoch.
    pub fn mtime(&self) -> u64 {
        self.mtime
    }

    /// Returns the device ID.
    pub fn dev(&self) -> u32 {
        self.dev
    }

    /// Returns the inode number.
    pub fn ino(&self) -> u32 {
        self.ino
    }

    /// Returns the file mode.
    pub fn mode(&self) -> FileMode {
        self.mode
    }

    /// Returns the user ID.
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// Returns the group ID.
    pub fn gid(&self) -> u32 {
        self.gid
    }

    /// Returns the file size in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Returns the object ID (SHA-1 hash) of the blob.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Returns the path of the file relative to the repository root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the stage number.
    ///
    /// - 0: Normal entry
    /// - 1: Base version in a merge conflict
    /// - 2: "Ours" version in a merge conflict
    /// - 3: "Theirs" version in a merge conflict
    pub fn stage(&self) -> u8 {
        self.stage
    }

    /// Returns true if this entry is in a merge conflict.
    pub fn is_conflicted(&self) -> bool {
        self.stage != 0
    }

    /// Returns the nanosecond part of ctime.
    pub fn ctime_nsec(&self) -> u32 {
        self.ctime_nsec
    }

    /// Returns the nanosecond part of mtime.
    pub fn mtime_nsec(&self) -> u32 {
        self.mtime_nsec
    }

    /// Returns whether the entry is marked skip-worktree (sparse checkout):
    /// its working tree file is intentionally absent or not to be compared.
    pub fn skip_worktree(&self) -> bool {
        self.skip_worktree
    }

    /// Returns whether the entry is marked intent-to-add (`git add -N`): the
    /// path is tracked but its content is not staged and is not committed.
    pub fn intent_to_add(&self) -> bool {
        self.intent_to_add
    }

    /// The same entry (blob, mode, stat data, flags) at another path.
    pub(crate) fn with_path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    pub(crate) fn with_nanos(mut self, ctime_nsec: u32, mtime_nsec: u32) -> Self {
        self.ctime_nsec = ctime_nsec;
        self.mtime_nsec = mtime_nsec;
        self
    }

    pub(crate) fn with_extended_flags(mut self, skip_worktree: bool, intent_to_add: bool) -> Self {
        self.skip_worktree = skip_worktree;
        self.intent_to_add = intent_to_add;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1_A: [u8; 20] = [
        0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60, 0x18,
        0x90, 0xaf, 0xd8, 0x07, 0x09,
    ];

    fn make_entry(path: &str) -> IndexEntry {
        IndexEntry::new(
            1700000000, // ctime
            1700000001, // mtime
            100,        // dev
            12345,      // ino
            FileMode::Regular,
            1000, // uid
            1000, // gid
            42,   // size
            Oid::from_bytes(SHA1_A),
            PathBuf::from(path),
            0, // stage
        )
    }

    #[test]
    fn test_index_basic() {
        let entries = vec![make_entry("file.txt"), make_entry("dir/file2.txt")];
        let index = Index::new(2, entries);

        assert_eq!(index.version(), 2);
        assert_eq!(index.len(), 2);
        assert!(!index.is_empty());
    }

    #[test]
    fn test_index_empty() {
        let index = Index::new(2, vec![]);
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_index_get() {
        let entries = vec![make_entry("file.txt"), make_entry("dir/file2.txt")];
        let index = Index::new(2, entries);

        let entry = index.get(Path::new("file.txt")).unwrap();
        assert_eq!(entry.path(), Path::new("file.txt"));

        let entry = index.get(Path::new("dir/file2.txt")).unwrap();
        assert_eq!(entry.path(), Path::new("dir/file2.txt"));

        assert!(index.get(Path::new("nonexistent")).is_none());
    }

    #[test]
    fn test_index_iter() {
        let entries = vec![make_entry("a.txt"), make_entry("b.txt")];
        let index = Index::new(2, entries);

        let paths: Vec<_> = index.iter().map(|e| e.path().to_path_buf()).collect();
        assert_eq!(paths, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
    }

    #[test]
    fn test_entry_accessors() {
        let entry = IndexEntry::new(
            1700000000,
            1700000001,
            100,
            12345,
            FileMode::Executable,
            1000,
            1001,
            42,
            Oid::from_bytes(SHA1_A),
            PathBuf::from("script.sh"),
            0,
        );

        assert_eq!(entry.ctime(), 1700000000);
        assert_eq!(entry.mtime(), 1700000001);
        assert_eq!(entry.dev(), 100);
        assert_eq!(entry.ino(), 12345);
        assert_eq!(entry.mode(), FileMode::Executable);
        assert_eq!(entry.uid(), 1000);
        assert_eq!(entry.gid(), 1001);
        assert_eq!(entry.size(), 42);
        assert_eq!(entry.oid(), &Oid::from_bytes(SHA1_A));
        assert_eq!(entry.path(), Path::new("script.sh"));
        assert_eq!(entry.stage(), 0);
        assert!(!entry.is_conflicted());
    }

    #[test]
    fn test_entry_conflict() {
        let mut entry = make_entry("file.txt");
        assert!(!entry.is_conflicted());

        // Simulate conflict stage
        entry.stage = 1;
        assert!(entry.is_conflicted());
    }

    fn staged(path: &str, stage: u8) -> IndexEntry {
        let mut entry = make_entry(path);
        entry.stage = stage;
        entry
    }

    fn listing(index: &Index) -> Vec<(String, u8)> {
        index
            .iter()
            .map(|e| (e.path().to_string_lossy().into_owned(), e.stage()))
            .collect()
    }

    #[test]
    fn test_add_keeps_git_order() {
        let mut index = Index::empty(2);
        // Path bytes order: '-' < '.' < '/'.
        for path in ["a/b", "a.b", "a-b", "a"] {
            index.add(make_entry(path));
        }
        let paths: Vec<_> = listing(&index).into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["a", "a-b", "a.b", "a/b"]);
    }

    #[test]
    fn test_conflict_stages_and_resolution() {
        let mut index = Index::empty(2);
        index.add(make_entry("a.txt"));
        index.add(staged("file.txt", 3));
        index.add(staged("file.txt", 1));
        index.add(staged("file.txt", 2));
        index.add(make_entry("z.txt"));
        assert!(index.has_conflicts());
        assert_eq!(index.conflicted_paths(), vec![PathBuf::from("file.txt")]);
        assert_eq!(index.get(Path::new("file.txt")).unwrap().stage(), 1);
        assert_eq!(
            index.get_stage(Path::new("file.txt"), 3).unwrap().stage(),
            3
        );
        assert_eq!(
            listing(&index),
            [
                ("a.txt".to_owned(), 0),
                ("file.txt".to_owned(), 1),
                ("file.txt".to_owned(), 2),
                ("file.txt".to_owned(), 3),
                ("z.txt".to_owned(), 0)
            ]
        );

        // Staging the path at stage 0 resolves the conflict.
        index.add(make_entry("file.txt"));
        assert!(!index.has_conflicts());
        assert_eq!(index.len(), 3);
        assert_eq!(index.get(Path::new("file.txt")).unwrap().stage(), 0);
    }

    #[test]
    fn test_remove_drops_all_stages() {
        let mut index = Index::empty(2);
        for stage in 1..=3 {
            index.add(staged("file.txt", stage));
        }
        assert!(index.remove(Path::new("file.txt")));
        assert!(index.is_empty());
        assert!(!index.remove(Path::new("file.txt")));
    }
}
