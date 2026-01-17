//! Git status implementation.
//!
//! This module implements working tree status detection by comparing
//! HEAD, Index, and the working tree.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::infra::{hash_object, list_working_tree, read_file};
use crate::objects::{LooseObjectStore, ObjectType, Oid, Tree};

/// The status of a file in the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// File is new and not tracked by Git.
    Untracked,
    /// File has been added to the index (staged for commit).
    Added,
    /// File has been modified in the working tree compared to the index.
    Modified,
    /// File has been deleted from the working tree.
    Deleted,
    /// File has been modified and staged.
    StagedModified,
    /// File has been deleted and staged.
    StagedDeleted,
}

impl FileStatus {
    /// Returns true if the file is staged (in index but different from HEAD).
    pub fn is_staged(&self) -> bool {
        matches!(
            self,
            FileStatus::Added | FileStatus::StagedModified | FileStatus::StagedDeleted
        )
    }

    /// Returns true if the file has unstaged changes.
    pub fn is_unstaged(&self) -> bool {
        matches!(
            self,
            FileStatus::Modified | FileStatus::Deleted | FileStatus::Untracked
        )
    }
}

/// A status entry representing a file and its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// The path of the file relative to the repository root.
    path: PathBuf,
    /// The status of the file.
    status: FileStatus,
}

impl StatusEntry {
    /// Creates a new StatusEntry.
    pub fn new(path: PathBuf, status: FileStatus) -> Self {
        Self { path, status }
    }

    /// Returns the path of the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the status of the file.
    pub fn status(&self) -> FileStatus {
        self.status
    }
}

/// Flattens a tree into a map of path -> Oid.
///
/// This recursively walks the tree and collects all blob entries
/// with their full paths.
pub fn flatten_tree(
    store: &LooseObjectStore,
    tree_oid: &Oid,
    prefix: &Path,
    result: &mut BTreeMap<PathBuf, Oid>,
) -> Result<()> {
    let raw = store.read(tree_oid)?;

    if raw.object_type != ObjectType::Tree {
        return Err(Error::TypeMismatch {
            expected: "tree",
            actual: raw.object_type.as_str(),
        });
    }

    let tree = Tree::parse(raw)?;

    for entry in tree.iter() {
        let entry_path = prefix.join(entry.name());

        if entry.is_directory() {
            // Recursively flatten subdirectory
            flatten_tree(store, entry.oid(), &entry_path, result)?;
        } else {
            // Add blob entry
            result.insert(entry_path, *entry.oid());
        }
    }

    Ok(())
}

/// Checks if a file in the working tree has been modified compared to a blob OID.
///
/// This computes the SHA-1 hash of the file content and compares it with
/// the expected OID.
pub fn file_modified(work_dir: &Path, path: &Path, expected_oid: &Oid) -> Result<bool> {
    let full_path = work_dir.join(path);

    // If file doesn't exist, it's definitely different
    if !full_path.exists() {
        return Ok(true);
    }

    // Read file content and compute hash
    let content = read_file(&full_path)?;
    let actual_hash = hash_object("blob", &content);
    let actual_oid = Oid::from_bytes(actual_hash);

    Ok(&actual_oid != expected_oid)
}

/// Computes the status of the working tree.
///
/// This compares three trees:
/// - HEAD tree: The tree of the current commit
/// - Index: The staging area
/// - Working tree: The actual files on disk
///
/// # Arguments
///
/// * `work_dir` - The root of the working tree.
/// * `store` - The object store for reading trees and blobs.
/// * `head_tree_oid` - The OID of the HEAD commit's tree (None if no commits yet).
/// * `index` - The parsed index file (None if no index exists).
///
/// # Returns
///
/// A vector of StatusEntry representing all files with changes.
pub fn compute_status(
    work_dir: &Path,
    store: &LooseObjectStore,
    head_tree_oid: Option<&Oid>,
    index: Option<&Index>,
) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();

    // Flatten HEAD tree into path -> OID map
    let mut head_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
    if let Some(tree_oid) = head_tree_oid {
        flatten_tree(store, tree_oid, Path::new(""), &mut head_files)?;
    }

    // Build index map: path -> IndexEntry
    let index_files: BTreeMap<PathBuf, &IndexEntry> = index
        .map(|idx| idx.iter().map(|e| (e.path().to_path_buf(), e)).collect())
        .unwrap_or_default();

    // Get working tree files
    let working_files: HashSet<PathBuf> = list_working_tree(work_dir)?.into_iter().collect();

    // Collect all paths
    let mut all_paths: HashSet<PathBuf> = HashSet::new();
    all_paths.extend(head_files.keys().cloned());
    all_paths.extend(index_files.keys().cloned());
    all_paths.extend(working_files.iter().cloned());

    // Analyze each path
    for path in all_paths {
        let in_head = head_files.get(&path);
        let in_index = index_files.get(&path);
        let in_working = working_files.contains(&path);

        let status = match (in_head, in_index, in_working) {
            // Untracked: not in HEAD, not in index, but in working tree
            (None, None, true) => Some(FileStatus::Untracked),

            // Added (staged): not in HEAD, in index
            (None, Some(_), true) => Some(FileStatus::Added),
            (None, Some(_), false) => {
                // Added to index but then deleted from working tree
                // This is technically staged add + unstaged delete, but we'll report as deleted
                Some(FileStatus::Deleted)
            }

            // Deleted from working tree (unstaged)
            (Some(_), Some(_), false) => Some(FileStatus::Deleted),

            // Staged delete: in HEAD, not in index
            (Some(_), None, false) => Some(FileStatus::StagedDeleted),
            (Some(_), None, true) => {
                // Deleted from index but file still exists
                // This is a staged delete with the file recreated
                Some(FileStatus::StagedDeleted)
            }

            // File exists in all three places - check for modifications
            (Some(head_oid), Some(index_entry), true) => {
                let index_oid = index_entry.oid();
                let head_modified = head_oid != index_oid;
                let working_modified = file_modified(work_dir, &path, index_oid)?;

                match (head_modified, working_modified) {
                    (false, false) => None, // No changes
                    (false, true) => Some(FileStatus::Modified),
                    (true, false) => Some(FileStatus::StagedModified),
                    (true, true) => {
                        // Both staged and unstaged changes
                        // For simplicity, report as modified (unstaged takes precedence)
                        Some(FileStatus::Modified)
                    }
                }
            }

            // Not anywhere - shouldn't happen
            (None, None, false) => None,
        };

        if let Some(s) = status {
            entries.push(StatusEntry::new(path, s));
        }
    }

    // Sort by path for consistent output
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::hash_object;
    use crate::objects::tree::FileMode;
    use miniz_oxide::deflate::compress_to_vec_zlib;
    use std::fs;
    use tempfile::TempDir;

    /// Creates a loose object and returns its OID.
    fn create_object(objects_dir: &Path, content: &[u8], object_type: &str) -> Oid {
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

    /// Creates a tree object with the given entries.
    fn create_tree(objects_dir: &Path, entries: &[(&str, FileMode, &Oid)]) -> Oid {
        let mut content = Vec::new();
        for (name, mode, oid) in entries {
            content.extend_from_slice(mode.as_octal().as_bytes());
            content.push(b' ');
            content.extend_from_slice(name.as_bytes());
            content.push(0);
            content.extend_from_slice(oid.as_bytes());
        }
        create_object(objects_dir, &content, "tree")
    }

    // Test flatten_tree
    #[test]
    fn test_flatten_tree_simple() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create a blob
        let blob_oid = create_object(&objects_dir, b"hello", "blob");

        // Create a tree with one entry
        let tree_oid = create_tree(&objects_dir, &[("file.txt", FileMode::Regular, &blob_oid)]);

        let store = LooseObjectStore::new(&objects_dir);
        let mut result = BTreeMap::new();
        flatten_tree(&store, &tree_oid, Path::new(""), &mut result).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(Path::new("file.txt")), Some(&blob_oid));
    }

    #[test]
    fn test_flatten_tree_nested() {
        let temp = TempDir::new().unwrap();
        let objects_dir = temp.path().join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create blobs
        let blob1_oid = create_object(&objects_dir, b"content1", "blob");
        let blob2_oid = create_object(&objects_dir, b"content2", "blob");

        // Create subtree
        let subtree_oid = create_tree(
            &objects_dir,
            &[("nested.txt", FileMode::Regular, &blob2_oid)],
        );

        // Create root tree
        let root_tree_oid = create_tree(
            &objects_dir,
            &[
                ("file.txt", FileMode::Regular, &blob1_oid),
                ("subdir", FileMode::Directory, &subtree_oid),
            ],
        );

        let store = LooseObjectStore::new(&objects_dir);
        let mut result = BTreeMap::new();
        flatten_tree(&store, &root_tree_oid, Path::new(""), &mut result).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(Path::new("file.txt")), Some(&blob1_oid));
        assert!(
            result.get(Path::new("subdir/nested.txt")) == Some(&blob2_oid)
                || result.get(Path::new("subdir\\nested.txt")) == Some(&blob2_oid)
        );
    }

    // Test file_modified
    #[test]
    fn test_file_modified_unchanged() {
        let temp = TempDir::new().unwrap();
        let work_dir = temp.path();

        let content = b"hello world";
        fs::write(work_dir.join("file.txt"), content).unwrap();

        let expected_oid = Oid::from_bytes(hash_object("blob", content));

        let modified = file_modified(work_dir, Path::new("file.txt"), &expected_oid).unwrap();
        assert!(!modified);
    }

    #[test]
    fn test_file_modified_changed() {
        let temp = TempDir::new().unwrap();
        let work_dir = temp.path();

        fs::write(work_dir.join("file.txt"), b"new content").unwrap();

        let old_oid = Oid::from_bytes(hash_object("blob", b"old content"));

        let modified = file_modified(work_dir, Path::new("file.txt"), &old_oid).unwrap();
        assert!(modified);
    }

    #[test]
    fn test_file_modified_deleted() {
        let temp = TempDir::new().unwrap();
        let work_dir = temp.path();

        let oid = Oid::from_bytes(hash_object("blob", b"content"));

        // File doesn't exist
        let modified = file_modified(work_dir, Path::new("nonexistent.txt"), &oid).unwrap();
        assert!(modified);
    }

    // Test compute_status scenarios
    #[test]
    fn test_compute_status_untracked() {
        let temp = TempDir::new().unwrap();
        let work_dir = temp.path();
        let objects_dir = work_dir.join(".git/objects");
        fs::create_dir_all(&objects_dir).unwrap();

        // Create a file in working tree
        fs::write(work_dir.join("new_file.txt"), b"content").unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let entries = compute_status(work_dir, &store, None, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("new_file.txt"));
        assert_eq!(entries[0].status(), FileStatus::Untracked);
    }

    #[test]
    fn test_file_status_methods() {
        assert!(FileStatus::Added.is_staged());
        assert!(FileStatus::StagedModified.is_staged());
        assert!(FileStatus::StagedDeleted.is_staged());
        assert!(!FileStatus::Modified.is_staged());
        assert!(!FileStatus::Deleted.is_staged());
        assert!(!FileStatus::Untracked.is_staged());

        assert!(FileStatus::Modified.is_unstaged());
        assert!(FileStatus::Deleted.is_unstaged());
        assert!(FileStatus::Untracked.is_unstaged());
        assert!(!FileStatus::Added.is_unstaged());
        assert!(!FileStatus::StagedModified.is_unstaged());
        assert!(!FileStatus::StagedDeleted.is_unstaged());
    }

    #[test]
    fn test_status_entry() {
        let entry = StatusEntry::new(PathBuf::from("test.txt"), FileStatus::Modified);
        assert_eq!(entry.path(), Path::new("test.txt"));
        assert_eq!(entry.status(), FileStatus::Modified);
    }
}
