//! Git status implementation.
//!
//! This module implements working tree status detection by comparing
//! HEAD, Index, and the working tree.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::infra::{hash_object, read_file};
use crate::objects::{tree::FileMode, LooseObjectStore, ObjectStore, ObjectType, Oid, Tree};
use crate::worktree::Worktree;

/// The status of a file in the working tree, as reported by
/// [`Repository::status`](crate::Repository::status).
///
/// This is one value per path, so a path with changes on both the index
/// side and the work tree side is reduced to one of them (see
/// `Repository::status` for the rules). [`DetailedStatus`] keeps both.
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
    flatten_tree_with_store(&ObjectStore::from_loose(store), tree_oid, prefix, result)
}

pub(crate) fn flatten_tree_with_store(
    store: &ObjectStore,
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
            flatten_tree_with_store(store, entry.oid(), &entry_path, result)?;
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
    let git_dir = work_dir.join(".git");
    let config = crate::config::load_config(&git_dir)?;
    let store = ObjectStore::from_loose(store);
    let mut worktree = Worktree::load(work_dir, &git_dir, &config, store.clone())?;
    compute_status_with_store(&store, head_tree_oid, index, &mut worktree)
}

pub(crate) fn compute_status_with_store(
    store: &ObjectStore,
    head_tree_oid: Option<&Oid>,
    index: Option<&Index>,
    worktree: &mut Worktree,
) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();

    // Flatten HEAD tree into path -> OID map
    let mut head_files: BTreeMap<PathBuf, Oid> = BTreeMap::new();
    if let Some(tree_oid) = head_tree_oid {
        flatten_tree_with_store(store, tree_oid, Path::new(""), &mut head_files)?;
    }

    // Build index map: path -> IndexEntry. Conflicted paths (stages 1-3)
    // are reported separately.
    let conflicted: HashSet<PathBuf> = index
        .map(|idx| idx.conflicted_paths().into_iter().collect())
        .unwrap_or_default();
    let index_files: BTreeMap<PathBuf, &IndexEntry> = index
        .map(|idx| {
            idx.iter()
                .filter(|e| !e.is_conflicted())
                .map(|e| (e.path().to_path_buf(), e))
                .collect()
        })
        .unwrap_or_default();

    // Get working tree files
    // Get working tree files: tracked ones, and untracked ones not ignored.
    let working_files = worktree.scan(index, false)?.files;

    // Collect all paths
    let mut all_paths: HashSet<PathBuf> = HashSet::new();
    all_paths.extend(head_files.keys().cloned());
    all_paths.extend(index_files.keys().cloned());
    all_paths.extend(working_files.iter().cloned());
    all_paths.extend(conflicted.iter().cloned());

    // Analyze each path
    for path in all_paths {
        let in_head = head_files.get(&path);
        let in_index = index_files.get(&path);
        let in_working = working_files.contains(&path);

        // A conflict still needs resolving in the working tree.
        if conflicted.contains(&path) {
            entries.push(StatusEntry::new(path, FileStatus::Modified));
            continue;
        }

        // Skip-worktree (sparse checkout) entries are not compared with the
        // working tree, only with HEAD, as Git does.
        if let Some(entry) = in_index.filter(|e| e.skip_worktree()) {
            let status = match in_head {
                None => Some(FileStatus::Added),
                Some(head_oid) if head_oid != entry.oid() => Some(FileStatus::StagedModified),
                Some(_) => None,
            };
            if let Some(s) = status {
                entries.push(StatusEntry::new(path, s));
            }
            continue;
        }

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
                // Content or mode (executable bit, symlink) differs from the index.
                let working_modified = worktree.hash(&path, Some(index_entry))?
                    != Some((*index_oid, index_entry.mode()));

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

/// The state of one side of a tracked path, as a column of
/// `git status --porcelain=v2`.
///
/// For the index side (X) the comparison is HEAD against the index; for the
/// work tree side (Y) it is the index against the work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeState {
    /// No change (`.`).
    Unmodified,
    /// The content or the executable bit changed (`M`).
    Modified,
    /// The type changed between file, symlink and submodule (`T`).
    TypeChanged,
    /// The path is new on this side (`A`).
    Added,
    /// The path was removed on this side (`D`).
    Deleted,
}

impl ChangeState {
    /// The porcelain v2 letter: `.`, `M`, `T`, `A` or `D`.
    pub fn as_char(&self) -> char {
        match self {
            ChangeState::Unmodified => '.',
            ChangeState::Modified => 'M',
            ChangeState::TypeChanged => 'T',
            ChangeState::Added => 'A',
            ChangeState::Deleted => 'D',
        }
    }
}

/// The kind of a merge conflict, from the stages present in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// Deleted on both sides (`DD`: stage 1 only).
    BothDeleted,
    /// Added by us (`AU`: stage 2 only).
    AddedByUs,
    /// Deleted by them (`UD`: stages 1 and 2).
    DeletedByThem,
    /// Added by them (`UA`: stage 3 only).
    AddedByThem,
    /// Deleted by us (`DU`: stages 1 and 3).
    DeletedByUs,
    /// Added on both sides (`AA`: stages 2 and 3).
    BothAdded,
    /// Modified on both sides (`UU`: stages 1, 2 and 3).
    BothModified,
}

impl ConflictKind {
    /// Classifies a conflict from the stages present (1 = base, 2 = ours,
    /// 3 = theirs). Returns `None` if no conflict stage is present.
    pub fn from_stages(base: bool, ours: bool, theirs: bool) -> Option<Self> {
        Some(match (base, ours, theirs) {
            (true, false, false) => ConflictKind::BothDeleted,
            (false, true, false) => ConflictKind::AddedByUs,
            (true, true, false) => ConflictKind::DeletedByThem,
            (false, false, true) => ConflictKind::AddedByThem,
            (true, false, true) => ConflictKind::DeletedByUs,
            (false, true, true) => ConflictKind::BothAdded,
            (true, true, true) => ConflictKind::BothModified,
            (false, false, false) => return None,
        })
    }

    /// The porcelain v2 XY code, such as `UU`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictKind::BothDeleted => "DD",
            ConflictKind::AddedByUs => "AU",
            ConflictKind::DeletedByThem => "UD",
            ConflictKind::AddedByThem => "UA",
            ConflictKind::DeletedByUs => "DU",
            ConflictKind::BothAdded => "AA",
            ConflictKind::BothModified => "UU",
        }
    }
}

/// What [`DetailedStatusEntry`] reports for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailedStatus {
    /// A tracked path with a change on at least one side
    /// (porcelain v2 line `1 XY ...`).
    Changed {
        /// HEAD compared with the index (X).
        index: ChangeState,
        /// The index compared with the work tree (Y).
        worktree: ChangeState,
    },
    /// A path with unresolved merge conflict stages (line `u XY ...`).
    Unmerged(ConflictKind),
    /// An untracked file that is not ignored (line `? path`).
    Untracked,
}

impl DetailedStatus {
    /// The porcelain v2 status code: `XY` for changed and unmerged paths
    /// (for example `M.`, `.D`, `UU`), `?` for untracked files.
    pub fn code(&self) -> String {
        match self {
            DetailedStatus::Changed { index, worktree } => {
                format!("{}{}", index.as_char(), worktree.as_char())
            }
            DetailedStatus::Unmerged(kind) => kind.as_str().to_owned(),
            DetailedStatus::Untracked => "?".to_owned(),
        }
    }

    /// Returns true if the index differs from HEAD (staged changes).
    pub fn is_staged(&self) -> bool {
        matches!(self, DetailedStatus::Changed { index, .. } if *index != ChangeState::Unmodified)
    }

    /// Returns true if the work tree differs from the index, or the file is
    /// untracked.
    pub fn is_unstaged(&self) -> bool {
        match self {
            DetailedStatus::Changed { worktree, .. } => *worktree != ChangeState::Unmodified,
            DetailedStatus::Untracked => true,
            DetailedStatus::Unmerged(_) => false,
        }
    }
}

/// One line of a detailed status: a path and its state on each side.
///
/// A path can appear twice: a file deleted from the index (`D.`) that still
/// exists in the work tree is also reported as untracked, as Git does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailedStatusEntry {
    path: PathBuf,
    status: DetailedStatus,
    head_mode: Option<FileMode>,
    index_mode: Option<FileMode>,
    worktree_mode: Option<FileMode>,
}

impl DetailedStatusEntry {
    /// The path relative to the repository root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The state of the path.
    pub fn status(&self) -> DetailedStatus {
        self.status
    }

    /// The mode in HEAD, if the path is in HEAD.
    pub fn head_mode(&self) -> Option<FileMode> {
        self.head_mode
    }

    /// The mode in the index (stage 0), if the path is in the index.
    pub fn index_mode(&self) -> Option<FileMode> {
        self.index_mode
    }

    /// The mode in the work tree, if the file exists.
    pub fn worktree_mode(&self) -> Option<FileMode> {
        self.worktree_mode
    }
}

/// Distinguishes files, symlinks and submodules for `T` (type changed).
fn type_class(mode: FileMode) -> u8 {
    match mode {
        FileMode::Regular | FileMode::Executable => 0,
        FileMode::Symlink => 1,
        FileMode::Submodule => 2,
        FileMode::Directory => 3,
    }
}

fn compare(old: Option<(Oid, FileMode)>, new: Option<(Oid, FileMode)>) -> ChangeState {
    match (old, new) {
        (None, None) => ChangeState::Unmodified,
        (None, Some(_)) => ChangeState::Added,
        (Some(_), None) => ChangeState::Deleted,
        (Some((_, old_mode)), Some((_, new_mode)))
            if type_class(old_mode) != type_class(new_mode) =>
        {
            ChangeState::TypeChanged
        }
        (Some(old), Some(new)) if old != new => ChangeState::Modified,
        _ => ChangeState::Unmodified,
    }
}

/// Flattens a tree into path -> (oid, mode), keyed like the index paths.
pub(crate) fn flatten_with_modes(
    store: &ObjectStore,
    tree_oid: &Oid,
    prefix: &str,
    result: &mut BTreeMap<PathBuf, (Oid, FileMode)>,
) -> Result<()> {
    let raw = store.read(tree_oid)?;
    if raw.object_type != ObjectType::Tree {
        return Err(Error::TypeMismatch {
            expected: "tree",
            actual: raw.object_type.as_str(),
        });
    }
    for entry in Tree::parse(raw)?.iter() {
        let path = if prefix.is_empty() {
            entry.name().to_owned()
        } else {
            format!("{}/{}", prefix, entry.name())
        };
        if entry.is_directory() {
            flatten_with_modes(store, entry.oid(), &path, result)?;
        } else {
            result.insert(PathBuf::from(path), (*entry.oid(), entry.mode()));
        }
    }
    Ok(())
}

pub(crate) fn compute_detailed_status(
    store: &ObjectStore,
    head_tree_oid: Option<&Oid>,
    index: Option<&Index>,
    worktree: &mut Worktree,
) -> Result<Vec<DetailedStatusEntry>> {
    let mut head: BTreeMap<PathBuf, (Oid, FileMode)> = BTreeMap::new();
    if let Some(tree_oid) = head_tree_oid {
        flatten_with_modes(store, tree_oid, "", &mut head)?;
    }

    // Index entries by `/`-separated path: stage 0 entries, and the
    // conflict stages present for each unmerged path.
    let mut staged: BTreeMap<PathBuf, &IndexEntry> = BTreeMap::new();
    let mut conflicts: BTreeMap<PathBuf, [bool; 3]> = BTreeMap::new();
    for entry in index.map(Index::entries).unwrap_or_default() {
        let key = PathBuf::from(
            String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned(),
        );
        match entry.stage() {
            0 => {
                staged.insert(key, entry);
            }
            stage => conflicts.entry(key).or_default()[usize::from(stage.min(3)) - 1] = true,
        }
    }

    let scan = worktree.scan(index, false)?;
    let present: HashSet<PathBuf> = scan
        .files
        .iter()
        .map(|p| PathBuf::from(p.to_string_lossy().replace('\\', "/")))
        .collect();

    let mut entries = Vec::new();
    let mut paths: BTreeMap<&PathBuf, ()> = BTreeMap::new();
    for path in head.keys().chain(staged.keys()).chain(conflicts.keys()) {
        paths.insert(path, ());
    }

    for path in paths.keys() {
        let path = *path;
        if let Some(stages) = conflicts.get(path) {
            if let Some(kind) = ConflictKind::from_stages(stages[0], stages[1], stages[2]) {
                entries.push(DetailedStatusEntry {
                    path: path.clone(),
                    status: DetailedStatus::Unmerged(kind),
                    head_mode: head.get(path).map(|h| h.1),
                    index_mode: None,
                    worktree_mode: None,
                });
                continue;
            }
        }
        let head_side = head.get(path).copied();
        let entry = staged.get(path).copied();
        // An intent-to-add entry has nothing staged: the file is new in the
        // work tree only (".A").
        let index_side = entry
            .filter(|e| !e.intent_to_add())
            .map(|e| (*e.oid(), e.mode()));
        let index_state = compare(head_side, index_side);

        let mut worktree_mode = None;
        let worktree_state = match entry {
            None => ChangeState::Unmodified,
            // Skip-worktree entries are not compared with the work tree.
            Some(e) if e.skip_worktree() => ChangeState::Unmodified,
            Some(e) => {
                let current = if present.contains(path) {
                    worktree.hash(&crate::worktree::native_path(path), Some(e))?
                } else {
                    None
                };
                worktree_mode = current.map(|c| c.1);
                if e.intent_to_add() {
                    if current.is_some() {
                        ChangeState::Added
                    } else {
                        ChangeState::Deleted
                    }
                } else {
                    compare(Some((*e.oid(), e.mode())), current)
                }
            }
        };

        if index_state != ChangeState::Unmodified || worktree_state != ChangeState::Unmodified {
            entries.push(DetailedStatusEntry {
                path: path.clone(),
                status: DetailedStatus::Changed {
                    index: index_state,
                    worktree: worktree_state,
                },
                head_mode: head_side.map(|h| h.1),
                index_mode: entry.map(IndexEntry::mode),
                worktree_mode,
            });
        }
    }

    // Untracked: present in the work tree with no index entry at any stage.
    for path in &present {
        if !staged.contains_key(path) && !conflicts.contains_key(path) {
            entries.push(DetailedStatusEntry {
                path: path.clone(),
                status: DetailedStatus::Untracked,
                head_mode: None,
                index_mode: None,
                worktree_mode: worktree.mode(&crate::worktree::native_path(path), None)?,
            });
        }
    }

    entries.sort_by(|a, b| {
        crate::index::path_key(&a.path)
            .cmp(&crate::index::path_key(&b.path))
            .then_with(|| {
                matches!(a.status, DetailedStatus::Untracked)
                    .cmp(&matches!(b.status, DetailedStatus::Untracked))
            })
    });
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
