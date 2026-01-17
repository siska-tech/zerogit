//! Git index (staging area) operations.
//!
//! The index file (`.git/index`) is a binary file that acts as a staging
//! area between the working tree and the repository.

mod reader;
mod writer;

use std::path::{Path, PathBuf};

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
}

impl Index {
    /// Creates a new empty index with the given version.
    pub fn empty(version: u32) -> Self {
        Self {
            version,
            entries: Vec::new(),
        }
    }

    /// Creates a new Index from parsed data.
    pub(crate) fn new(version: u32, entries: Vec<IndexEntry>) -> Self {
        Self { version, entries }
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
    /// # Arguments
    ///
    /// * `path` - The path to search for.
    ///
    /// # Returns
    ///
    /// The entry if found, or `None` if not found.
    pub fn get(&self, path: &Path) -> Option<&IndexEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Returns an iterator over the entries.
    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter()
    }

    /// Adds or updates an entry in the index.
    ///
    /// If an entry with the same path already exists, it is replaced.
    /// Entries are kept sorted by path.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry to add or update.
    pub fn add(&mut self, entry: IndexEntry) {
        // Find or insert position
        match self.entries.binary_search_by(|e| e.path.cmp(&entry.path)) {
            Ok(pos) => {
                // Replace existing entry
                self.entries[pos] = entry;
            }
            Err(pos) => {
                // Insert at correct position to maintain sort order
                self.entries.insert(pos, entry);
            }
        }
    }

    /// Removes an entry from the index by path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path of the entry to remove.
    ///
    /// # Returns
    ///
    /// `true` if an entry was removed, `false` if no entry was found.
    pub fn remove(&mut self, path: &Path) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.path == path) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Clears all entries from the index.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
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
}
