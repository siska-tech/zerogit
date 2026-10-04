//! Three-way merge of flattened trees.
//!
//! Paths are compared between the base and each side. A path changed on one
//! side only takes that side's version; a path changed identically on both
//! sides is clean. When both sides changed a regular file, the contents are
//! merged line by line and the executable bit is merged separately. Other
//! cases where both sides changed a path conflict: modified on one side and
//! deleted on the other, both changed but one is binary, changed between
//! file, symlink and submodule, or both changed a symlink or submodule
//! differently. Renames are not detected: a renamed file is a deletion and
//! an addition.

use std::collections::{BTreeMap, BTreeSet};

use super::file::{is_binary, merge_lines, ConflictStyle, Labels};
use crate::error::{Error, Result};
use crate::objects::{tree::FileMode, ObjectStore, ObjectType, Oid};

/// A blob and its mode.
pub(crate) type Entry = (Oid, FileMode);

/// A flattened tree: `/`-separated path to blob.
pub(crate) type Flat = BTreeMap<String, Entry>;

/// What a merge decided for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// The merged version (`None`: deleted).
    Clean(Option<Entry>),
    /// A conflict: the base, ours and theirs versions for index stages 1-3,
    /// and what to leave in the work tree.
    Conflict {
        stages: [Option<Entry>; 3],
        worktree: Option<Entry>,
    },
}

/// The result of merging two trees.
#[derive(Debug, Default)]
pub(crate) struct TreeMerge {
    /// Every path present in any of the three trees, with its resolution.
    pub(crate) paths: BTreeMap<String, Resolution>,
}

impl TreeMerge {
    pub(crate) fn conflicted_paths(&self) -> Vec<String> {
        self.paths
            .iter()
            .filter(|(_, r)| matches!(r, Resolution::Conflict { .. }))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// The tree to record if every conflict were resolved by taking what
    /// is left in the work tree (used for virtual merge bases).
    pub(crate) fn as_flat(&self) -> Flat {
        self.paths
            .iter()
            .filter_map(|(path, resolution)| {
                let entry = match resolution {
                    Resolution::Clean(entry) => *entry,
                    Resolution::Conflict { worktree, .. } => *worktree,
                };
                entry.map(|e| (path.clone(), e))
            })
            .collect()
    }
}

/// Distinguishes files, symlinks and submodules.
fn type_class(mode: FileMode) -> u8 {
    match mode {
        FileMode::Regular | FileMode::Executable => 0,
        FileMode::Symlink => 1,
        FileMode::Submodule => 2,
        FileMode::Directory => 3,
    }
}

/// Merges `ours` and `theirs` against `base`.
///
/// Merged and conflicted contents are written to `store` as blobs. The work
/// tree version of a conflict is the content with conflict markers, the
/// modified side of a modify/delete conflict, and ours otherwise;
/// [`TreeMerge::as_flat`] takes those versions, which is how several merge
/// bases are combined into one.
pub(crate) fn merge_trees(
    store: &ObjectStore,
    base: &Flat,
    ours: &Flat,
    theirs: &Flat,
    labels: Labels<'_>,
    style: ConflictStyle,
) -> Result<TreeMerge> {
    let mut all: BTreeSet<&String> = BTreeSet::new();
    all.extend(base.keys());
    all.extend(ours.keys());
    all.extend(theirs.keys());

    let mut result = TreeMerge::default();
    for path in all {
        let b = base.get(path).copied();
        let o = ours.get(path).copied();
        let t = theirs.get(path).copied();
        let resolution = if o == t || b == t {
            Resolution::Clean(o)
        } else if b == o {
            Resolution::Clean(t)
        } else {
            merge_path(store, b, o, t, labels, style)?
        };
        result.paths.insert(path.clone(), resolution);
    }
    check_directory_file_conflicts(&result)?;
    Ok(result)
}

/// Merges a path changed differently on both sides.
fn merge_path(
    store: &ObjectStore,
    b: Option<Entry>,
    o: Option<Entry>,
    t: Option<Entry>,
    labels: Labels<'_>,
    style: ConflictStyle,
) -> Result<Resolution> {
    let conflict = |worktree: Option<Entry>| Resolution::Conflict {
        stages: [b, o, t],
        worktree,
    };
    let (Some(ours), Some(theirs)) = (o, t) else {
        // Modified on one side, deleted on the other: keep the modified one.
        return Ok(conflict(o.or(t)));
    };
    let regular = |mode: FileMode| type_class(mode) == 0;
    if !regular(ours.1) || !regular(theirs.1) || b.is_some_and(|b| !regular(b.1)) {
        // Symlinks, submodules and type changes are not merged.
        return Ok(conflict(Some(ours)));
    }

    // The executable bit is merged on its own.
    let (mode, mode_conflict) = match b {
        Some((_, base_mode)) if ours.1 == base_mode => (theirs.1, false),
        Some((_, base_mode)) if theirs.1 == base_mode => (ours.1, false),
        _ if ours.1 == theirs.1 => (ours.1, false),
        _ => (ours.1, true),
    };
    if ours.0 == theirs.0 {
        let merged = Some((ours.0, mode));
        return Ok(if mode_conflict {
            conflict(merged)
        } else {
            Resolution::Clean(merged)
        });
    }

    let read = |oid: &Oid| -> Result<Vec<u8>> { Ok(store.read(oid)?.content) };
    let base_content = match b {
        Some((oid, _)) => read(&oid)?,
        None => Vec::new(),
    };
    let ours_content = read(&ours.0)?;
    let theirs_content = read(&theirs.0)?;
    if is_binary(&base_content) || is_binary(&ours_content) || is_binary(&theirs_content) {
        return Ok(conflict(Some(ours)));
    }
    let merged = merge_lines(&base_content, &ours_content, &theirs_content, labels, style);
    let oid = store.write(ObjectType::Blob, &merged.content)?;
    Ok(if merged.conflicts > 0 || mode_conflict {
        conflict(Some((oid, mode)))
    } else {
        Resolution::Clean(Some((oid, mode)))
    })
}

/// Refuses results where a file and a directory would share a path (for
/// example `a` as a file on one side and `a/b` on the other).
fn check_directory_file_conflicts(result: &TreeMerge) -> Result<()> {
    let present: Vec<&String> = result
        .paths
        .iter()
        .filter(|(_, r)| match r {
            Resolution::Clean(entry) => entry.is_some(),
            Resolution::Conflict { stages, worktree } => {
                worktree.is_some() || stages.iter().any(Option::is_some)
            }
        })
        .map(|(p, _)| p)
        .collect();
    // In sorted order, "a" is followed by its "a/..." children (after any
    // "a" + byte smaller than '/', which cannot be a child).
    for (i, path) in present.iter().enumerate() {
        let prefix = format!("{}/", path);
        for other in &present[i + 1..] {
            if other.starts_with(&prefix) {
                return Err(Error::UnsupportedMerge(format!(
                    "{} is a file on one side and a directory on the other",
                    path
                )));
            }
            if other.as_bytes().first() != path.as_bytes().first()
                || !other.starts_with(path.as_str())
            {
                break;
            }
        }
    }
    Ok(())
}
