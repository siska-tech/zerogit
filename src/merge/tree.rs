//! Three-way merge of flattened trees.
//!
//! Paths are compared between the base and each side. A path changed on one
//! side only takes that side's version; a path changed identically on both
//! sides is clean. When both sides changed a regular file, the contents are
//! merged line by line and the executable bit is merged separately. Other
//! cases where both sides changed a path conflict: modified on one side and
//! deleted on the other, both changed but one is binary, changed between
//! file, symlink and submodule, or both changed a symlink or submodule
//! differently.
//!
//! Renames are followed as Git's `ort` strategy follows them (unless
//! `merge.renames` is false): a file renamed on one side, exactly or with
//! changes (at least 50% similar), gets the other side's changes at its new
//! path, and conflict markers name both paths. Files renamed differently on
//! the two sides, renamed on one side and deleted on the other, and renames
//! onto a path the other side added are still treated as deletions and
//! additions. Similarity is measured on lines, close to but not exactly as
//! Git measures it, so files near the threshold may be paired differently.

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
///
/// With `renames`, files renamed on one side are followed, as Git's `ort`
/// strategy follows them (see [`follow_renames`]).
pub(crate) fn merge_trees(
    store: &ObjectStore,
    base: &Flat,
    ours: &Flat,
    theirs: &Flat,
    labels: Labels<'_>,
    style: ConflictStyle,
    renames: bool,
) -> Result<TreeMerge> {
    let (mut base, mut ours, mut theirs) = (base.clone(), ours.clone(), theirs.clone());
    let moved = if renames {
        follow_renames(store, &mut base, &mut ours, &mut theirs)?
    } else {
        BTreeMap::new()
    };

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
            match moved.get(path) {
                // Conflict markers name each side's path, as Git does.
                Some(paths) => {
                    let ours_label = format!("{}:{}", labels.ours, paths.ours);
                    let base_label = format!("{}:{}", labels.base, paths.base);
                    let theirs_label = format!("{}:{}", labels.theirs, paths.theirs);
                    let labels = Labels {
                        ours: &ours_label,
                        base: &base_label,
                        theirs: &theirs_label,
                    };
                    merge_path(store, b, o, t, labels, style)?
                }
                None => merge_path(store, b, o, t, labels, style)?,
            }
        };
        result.paths.insert(path.clone(), resolution);
    }
    // A path moved away by a rename is deleted (on the side that still had
    // it, the work tree and index entry go).
    for path in moved.values().flat_map(|p| [&p.base, &p.ours, &p.theirs]) {
        result
            .paths
            .entry(path.clone())
            .or_insert(Resolution::Clean(None));
    }
    check_directory_file_conflicts(&result)?;
    Ok(result)
}

/// The paths a renamed file had in the base and on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenamedPaths {
    base: String,
    ours: String,
    theirs: String,
}

/// Detects the files renamed on each side and moves the other versions of
/// a renamed file (the base's, and the other side's when it kept the old
/// path) to the new path, so that the path-wise merge combines a rename on
/// one side with changes on the other, as Git's `ort` strategy does.
/// Returns the merged paths that came from a rename, with each side's path.
///
/// Followed: a rename on one side while the other side kept the file (with
/// or without changes) and has nothing at the new path; and the same rename
/// on both sides. Other cases (renamed differently on each side, renamed
/// on one side and deleted on the other, or a new file at the target) are
/// left as deletions and additions.
fn follow_renames(
    store: &ObjectStore,
    base: &mut Flat,
    ours: &mut Flat,
    theirs: &mut Flat,
) -> Result<BTreeMap<String, RenamedPaths>> {
    let ours_renames = detect_renames(store, base, ours)?;
    let theirs_renames = detect_renames(store, base, theirs)?;
    let mut moved = BTreeMap::new();

    for (old, new) in &ours_renames {
        match theirs_renames.get(old) {
            // Renamed the same way on both sides.
            Some(theirs_new) if theirs_new == new => {
                if let Some(entry) = base.remove(old) {
                    base.insert(new.clone(), entry);
                }
            }
            Some(_) => {}
            None => {
                if theirs.contains_key(old) && !theirs.contains_key(new) && !base.contains_key(new)
                {
                    let entry = base.remove(old).expect("a rename source is in the base");
                    base.insert(new.clone(), entry);
                    let entry = theirs.remove(old).expect("checked above");
                    theirs.insert(new.clone(), entry);
                    moved.insert(
                        new.clone(),
                        RenamedPaths {
                            base: old.clone(),
                            ours: new.clone(),
                            theirs: old.clone(),
                        },
                    );
                }
            }
        }
    }
    for (old, new) in &theirs_renames {
        if ours_renames.contains_key(old) {
            continue;
        }
        if ours.contains_key(old)
            && !ours.contains_key(new)
            && base.contains_key(old)
            && !base.contains_key(new)
        {
            let entry = base.remove(old).expect("checked above");
            base.insert(new.clone(), entry);
            let entry = ours.remove(old).expect("checked above");
            ours.insert(new.clone(), entry);
            moved.insert(
                new.clone(),
                RenamedPaths {
                    base: old.clone(),
                    ours: old.clone(),
                    theirs: new.clone(),
                },
            );
        }
    }
    Ok(moved)
}

/// The files renamed from `base` to `side`: base path to new path. Exact
/// renames (same content) are paired first, preferring the same file name;
/// the remaining files are paired by similarity (at least 50%, Git's
/// default for merges).
fn detect_renames(
    store: &ObjectStore,
    base: &Flat,
    side: &Flat,
) -> Result<BTreeMap<String, String>> {
    let deleted: Vec<(&String, Entry)> = base
        .iter()
        .filter(|(path, _)| !side.contains_key(*path))
        .map(|(path, entry)| (path, *entry))
        .collect();
    let added: Vec<(&String, Entry)> = side
        .iter()
        .filter(|(path, _)| !base.contains_key(*path))
        .map(|(path, entry)| (path, *entry))
        .collect();
    let mut renames = BTreeMap::new();
    if deleted.is_empty() || added.is_empty() {
        return Ok(renames);
    }

    let file_name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    let mut used: BTreeSet<&String> = BTreeSet::new();
    for (old, (oid, mode)) in &deleted {
        let candidates: Vec<&(&String, Entry)> = added
            .iter()
            .filter(|(new, (new_oid, new_mode))| {
                new_oid == oid && type_class(*new_mode) == type_class(*mode) && !used.contains(new)
            })
            .collect();
        let chosen = candidates
            .iter()
            .find(|(new, _)| file_name(new) == file_name(old))
            .or_else(|| candidates.first());
        if let Some((new, _)) = chosen {
            used.insert(new);
            renames.insert((*old).clone(), (*new).clone());
        }
    }

    let sources: Vec<(String, Oid, FileMode)> = deleted
        .iter()
        .filter(|(old, _)| !renames.contains_key(*old))
        .map(|(path, (oid, mode))| ((*path).clone(), *oid, *mode))
        .collect();
    let targets: Vec<(String, Oid, FileMode)> = added
        .iter()
        .filter(|(new, _)| !used.contains(new))
        .map(|(path, (oid, mode))| ((*path).clone(), *oid, *mode))
        .collect();
    if !sources.is_empty() && !targets.is_empty() {
        let options = crate::diff::RenameOptions::new()
            .detection(crate::diff::RenameDetection::Similar)
            .threshold(50);
        let mut read = |oid: &Oid| -> Result<Vec<u8>> { Ok(store.read(oid)?.content) };
        for (old, new) in
            crate::diff::rename::pair_similar(&sources, &targets, &options, &mut read)?
        {
            renames.insert(old, new);
        }
    }
    Ok(renames)
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
