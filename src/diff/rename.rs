//! Rename detection options and similarity-based rename detection.

use std::collections::HashMap;
use std::path::Path;

use super::{DiffDelta, DiffStatus};
use crate::error::Result;
use crate::objects::{FileMode, Oid};

/// Which renames [`Repository::diff_trees_with_options`](crate::Repository::diff_trees_with_options) detects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameDetection {
    /// Report every move as a deletion and an addition.
    Off,
    /// Pair deletions and additions with identical content (the default).
    Exact,
    /// Also pair edited moves whose similarity reaches the threshold.
    Similar,
}

/// Options for rename detection in tree diffs.
///
/// # Similarity
///
/// Each file is split after every `\n`. The similarity of two files is the
/// total byte length of the lines they have in common (counted as a multiset,
/// ignoring order) divided by the size of the larger file, as a whole
/// percentage rounded down. Identical files score 100.
///
/// Only regular and executable files take part. Empty files, files
/// containing a NUL byte (binary) and files larger than
/// [`RenameOptions::max_file_size`] are never paired by similarity; exact
/// renames still apply to them.
///
/// # Pairing
///
/// Candidate pairs scoring at least the threshold are taken greedily by
/// highest score, then same file name, then new path, then old path, so each
/// file is used at most once and the result does not depend on input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameOptions {
    detection: RenameDetection,
    threshold: u8,
    max_pairs: usize,
    max_file_size: usize,
}

impl Default for RenameOptions {
    /// Exact renames only; similarity settings default to 50%, 100,000 pairs and 1 MiB.
    fn default() -> Self {
        Self {
            detection: RenameDetection::Exact,
            threshold: 50,
            max_pairs: 100_000,
            max_file_size: 1 << 20,
        }
    }
}

impl RenameOptions {
    /// Creates the default options (exact renames only).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the detection mode.
    pub fn detection(mut self, detection: RenameDetection) -> Self {
        self.detection = detection;
        self
    }

    /// Sets the minimum similarity, in percent (clamped to 0..=100).
    pub fn threshold(mut self, percent: u8) -> Self {
        self.threshold = percent.min(100);
        self
    }

    /// Sets the largest number of (deleted, added) candidate pairs compared.
    ///
    /// When there are more, similarity detection is skipped entirely and
    /// reported by [`TreeDiff::rename_limits`](super::TreeDiff::rename_limits).
    pub fn max_pairs(mut self, pairs: usize) -> Self {
        self.max_pairs = pairs;
        self
    }

    /// Sets the largest file size, in bytes, considered for similarity.
    pub fn max_file_size(mut self, bytes: usize) -> Self {
        self.max_file_size = bytes;
        self
    }

    /// Returns the detection mode.
    pub fn get_detection(&self) -> RenameDetection {
        self.detection
    }

    /// Returns the similarity threshold in percent.
    pub fn get_threshold(&self) -> u8 {
        self.threshold
    }

    /// Returns the candidate pair limit.
    pub fn get_max_pairs(&self) -> usize {
        self.max_pairs
    }

    /// Returns the file size limit for similarity.
    pub fn get_max_file_size(&self) -> usize {
        self.max_file_size
    }
}

/// A limit that made similarity detection incomplete.
///
/// Files affected stay as additions and deletions, so no change is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameLimit {
    /// There were more candidate pairs than allowed; no similarity pairing was done.
    TooManyPairs {
        /// The number of candidate pairs.
        pairs: usize,
        /// The configured limit.
        limit: usize,
    },
    /// Some files were larger than allowed and were not compared.
    FileTooLarge {
        /// The number of files excluded.
        files: usize,
        /// The configured limit in bytes.
        limit: usize,
    },
}

/// Line multiset of one file: line -> (occurrences, byte length).
struct Profile<'a> {
    size: usize,
    lines: HashMap<&'a [u8], usize>,
}

impl<'a> Profile<'a> {
    fn new(content: &'a [u8]) -> Self {
        let mut lines = HashMap::new();
        for line in content.split_inclusive(|&b| b == b'\n') {
            *lines.entry(line).or_insert(0) += 1;
        }
        Self {
            size: content.len(),
            lines,
        }
    }
}

/// Returns the similarity of two non-empty contents in percent.
#[cfg(test)]
fn similarity(a: &[u8], b: &[u8]) -> u8 {
    score(&Profile::new(a), &Profile::new(b))
}

fn score(a: &Profile<'_>, b: &Profile<'_>) -> u8 {
    let (small, large) = if a.lines.len() <= b.lines.len() {
        (a, b)
    } else {
        (b, a)
    };
    let common: usize = small
        .lines
        .iter()
        .filter_map(|(line, &count)| {
            large
                .lines
                .get(line)
                .map(|&other| count.min(other) * line.len())
        })
        .sum();
    let larger = a.size.max(b.size);
    if larger == 0 {
        return 100;
    }
    (common as u128 * 100 / larger as u128) as u8
}

fn eligible(mode: Option<FileMode>) -> bool {
    matches!(mode, Some(FileMode::Regular | FileMode::Executable))
}

/// Pairs remaining deletions and additions by similarity.
///
/// Must run after exact rename detection. `read` returns blob contents.
pub(crate) fn detect_similar_renames(
    deltas: &mut Vec<DiffDelta>,
    options: &RenameOptions,
    read: &mut dyn FnMut(&Oid) -> Result<Vec<u8>>,
) -> Result<Vec<RenameLimit>> {
    let candidates = |status: DiffStatus| -> Vec<usize> {
        deltas
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.status == status
                    && eligible(match status {
                        DiffStatus::Deleted => d.old_mode,
                        _ => d.new_mode,
                    })
            })
            .map(|(i, _)| i)
            .collect()
    };
    let sources = candidates(DiffStatus::Deleted);
    let targets = candidates(DiffStatus::Added);
    if sources.is_empty() || targets.is_empty() {
        return Ok(Vec::new());
    }
    let pairs = sources.len().saturating_mul(targets.len());
    if pairs > options.max_pairs {
        return Ok(vec![RenameLimit::TooManyPairs {
            pairs,
            limit: options.max_pairs,
        }]);
    }

    // Load contents; exclude empty, binary and oversized files.
    let mut oversized = 0;
    let mut load = |indices: &[usize], old_side: bool| -> Result<Vec<(usize, Vec<u8>)>> {
        let mut loaded = Vec::new();
        for &i in indices {
            let delta = &deltas[i];
            let oid = if old_side {
                delta.old_oid
            } else {
                delta.new_oid
            };
            let Some(oid) = oid else { continue };
            let content = read(&oid)?;
            if content.len() > options.max_file_size {
                oversized += 1;
            } else if !content.is_empty() && !content.contains(&0) {
                loaded.push((i, content));
            }
        }
        Ok(loaded)
    };
    let source_contents = load(&sources, true)?;
    let target_contents = load(&targets, false)?;
    let source_profiles: Vec<_> = source_contents
        .iter()
        .map(|(i, c)| (*i, Profile::new(c)))
        .collect();
    let target_profiles: Vec<_> = target_contents
        .iter()
        .map(|(i, c)| (*i, Profile::new(c)))
        .collect();

    let threshold = u128::from(options.threshold);
    let mut matches = Vec::new();
    for (s, source) in &source_profiles {
        for (t, target) in &target_profiles {
            // The common bytes cannot exceed the smaller file.
            let (small, large) = (source.size.min(target.size), source.size.max(target.size));
            if (small as u128) * 100 < threshold * large as u128 {
                continue;
            }
            let score = score(source, target);
            if score >= options.threshold {
                matches.push((score, *s, *t));
            }
        }
    }
    let file_name = |i: usize| deltas[i].path.file_name().map(|n| n.to_owned());
    let path = |i: usize| -> &Path { &deltas[i].path };
    matches.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                (file_name(a.1) != file_name(a.2)).cmp(&(file_name(b.1) != file_name(b.2)))
            })
            .then_with(|| path(a.2).cmp(path(b.2)))
            .then_with(|| path(a.1).cmp(path(b.1)))
    });

    let mut used = vec![false; deltas.len()];
    let mut renames = Vec::new();
    for (score, s, t) in matches {
        if used[s] || used[t] {
            continue;
        }
        used[s] = true;
        used[t] = true;
        let (source, target) = (&deltas[s], &deltas[t]);
        renames.push(DiffDelta {
            status: DiffStatus::Renamed,
            path: target.path.clone(),
            old_path: Some(source.path.clone()),
            old_oid: source.old_oid,
            new_oid: target.new_oid,
            old_mode: source.old_mode,
            new_mode: target.new_mode,
            similarity: Some(score),
        });
    }
    let mut index = 0;
    deltas.retain(|_| {
        index += 1;
        !used[index - 1]
    });
    deltas.extend(renames);
    deltas.sort_by(|a, b| a.path.cmp(&b.path));

    let mut limits = Vec::new();
    if oversized > 0 {
        limits.push(RenameLimit::FileTooLarge {
            files: oversized,
            limit: options.max_file_size,
        });
    }
    Ok(limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::hash_object;
    use std::path::PathBuf;

    struct Fixture {
        blobs: HashMap<Oid, Vec<u8>>,
        deltas: Vec<DiffDelta>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                blobs: HashMap::new(),
                deltas: Vec::new(),
            }
        }

        fn oid(&mut self, content: &[u8]) -> Oid {
            let oid = Oid::from_bytes(hash_object("blob", content));
            self.blobs.insert(oid, content.to_vec());
            oid
        }

        fn deleted(&mut self, path: &str, content: &[u8], mode: FileMode) -> &mut Self {
            let oid = self.oid(content);
            self.deltas
                .push(DiffDelta::deleted(PathBuf::from(path), oid, mode));
            self
        }

        fn added(&mut self, path: &str, content: &[u8], mode: FileMode) -> &mut Self {
            let oid = self.oid(content);
            self.deltas
                .push(DiffDelta::added(PathBuf::from(path), oid, mode));
            self
        }

        /// Runs detection on the deltas in the given order.
        fn run(&self, order: &[usize], options: &RenameOptions) -> (Vec<String>, Vec<RenameLimit>) {
            let mut deltas: Vec<_> = order.iter().map(|&i| self.deltas[i].clone()).collect();
            super::super::detect_renames(&mut deltas);
            let mut reads = 0;
            let limits = detect_similar_renames(&mut deltas, options, &mut |oid| {
                reads += 1;
                Ok(self.blobs[oid].clone())
            })
            .unwrap();
            let summary = deltas
                .iter()
                .map(|d| {
                    format!(
                        "{} {} -> {} {:?} {:?}->{:?}",
                        d.status_char(),
                        d.old_path()
                            .map_or(String::new(), |p| p.display().to_string()),
                        d.path().display(),
                        d.similarity(),
                        d.old_mode(),
                        d.new_mode()
                    )
                })
                .collect();
            (summary, limits)
        }

        fn run_all(&self, options: &RenameOptions) -> (Vec<String>, Vec<RenameLimit>) {
            let order: Vec<usize> = (0..self.deltas.len()).collect();
            let result = self.run(&order, options);
            let reversed: Vec<usize> = order.iter().rev().copied().collect();
            assert_eq!(self.run(&reversed, options), result, "order dependent");
            result
        }
    }

    fn similar() -> RenameOptions {
        RenameOptions::new().detection(RenameDetection::Similar)
    }

    fn lines(range: std::ops::Range<usize>) -> Vec<u8> {
        range
            .map(|i| format!("line {:03}\n", i))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn similarity_counts_common_lines_against_the_larger_file() {
        assert_eq!(similarity(b"a\nb\n", b"a\nb\n"), 100);
        assert_eq!(similarity(b"a\nb\n", b"a\nc\n"), 50);
        // Order is ignored; repeated lines count as a multiset.
        assert_eq!(similarity(b"a\nb\n", b"b\na\n"), 100);
        assert_eq!(similarity(b"x\nx\n", b"x\n"), 50);
        assert_eq!(similarity(&lines(0..10), &lines(0..20)), 50);
        assert_eq!(similarity(b"abc", b"xyz"), 0);
    }

    #[test]
    fn edited_moves_become_renames_with_both_oids_and_modes() {
        let mut f = Fixture::new();
        let old = lines(0..10);
        let mut new = lines(0..9);
        new.extend_from_slice(b"edited\n");
        f.deleted("docs/a.md", &old, FileMode::Regular)
            .added("guide/a.md", &new, FileMode::Executable)
            .added("other.md", b"unrelated\n", FileMode::Regular);
        let (summary, limits) = f.run_all(&similar());
        assert!(limits.is_empty());
        assert_eq!(
            summary,
            [
                "R docs/a.md -> guide/a.md Some(90) Some(Regular)->Some(Executable)",
                "A  -> other.md None None->Some(Regular)",
            ]
        );
        let renamed = {
            let mut deltas = f.deltas.clone();
            detect_similar_renames(&mut deltas, &similar(), &mut |oid| Ok(f.blobs[oid].clone()))
                .unwrap();
            deltas
                .into_iter()
                .find(|d| d.status() == DiffStatus::Renamed)
                .unwrap()
        };
        assert_eq!(
            renamed.old_oid(),
            Some(&Oid::from_bytes(hash_object("blob", &old)))
        );
        assert_eq!(
            renamed.new_oid(),
            Some(&Oid::from_bytes(hash_object("blob", &new)))
        );

        // Off and the default (exact) modes keep the addition and deletion.
        for options in [RenameOptions::new(), similar().threshold(95)] {
            let mut deltas = f.deltas.clone();
            if options.get_detection() == RenameDetection::Similar {
                detect_similar_renames(&mut deltas, &options, &mut |oid| Ok(f.blobs[oid].clone()))
                    .unwrap();
            }
            assert!(deltas.iter().all(|d| d.status() != DiffStatus::Renamed));
            assert_eq!(deltas.len(), 3);
        }
    }

    #[test]
    fn ties_prefer_same_file_name_then_path_order() {
        let mut f = Fixture::new();
        let base = lines(0..10);
        let mut edited = lines(0..8);
        edited.extend_from_slice(b"x\ny\n");
        f.deleted("a/one.md", &base, FileMode::Regular)
            .deleted("b/two.md", &base, FileMode::Regular)
            .added("c/two.md", &edited, FileMode::Regular)
            .added("d/zzz.md", &edited, FileMode::Regular);
        let (summary, _) = f.run_all(&similar());
        assert_eq!(
            summary,
            [
                "R b/two.md -> c/two.md Some(80) Some(Regular)->Some(Regular)",
                "R a/one.md -> d/zzz.md Some(80) Some(Regular)->Some(Regular)",
            ]
        );

        // Higher similarity wins over the same file name.
        let mut f = Fixture::new();
        let mut close = lines(0..9);
        close.extend_from_slice(b"z\n"); // 81 common bytes of 90: 90%
        f.deleted("a/same.md", &base, FileMode::Regular)
            .added("b/same.md", &lines(0..6), FileMode::Regular) // 60%
            .added("c/new.md", &close, FileMode::Regular);
        let (summary, _) = f.run_all(&similar());
        assert_eq!(
            summary,
            [
                "A  -> b/same.md None None->Some(Regular)",
                "R a/same.md -> c/new.md Some(90) Some(Regular)->Some(Regular)",
            ]
        );
    }

    #[test]
    fn empty_binary_and_non_file_entries_are_not_paired_by_similarity() {
        let mut f = Fixture::new();
        f.deleted("empty_old", b"", FileMode::Regular)
            .added("empty_new_dir/x", b"", FileMode::Regular)
            .deleted("bin_old", b"\0\x01\x02\n", FileMode::Regular)
            .added("bin_new", b"\0\x01\x03\n", FileMode::Regular)
            .deleted("link_old", b"target\n", FileMode::Symlink)
            .added("link_new", b"target\nx\n", FileMode::Symlink);
        let (summary, limits) = f.run_all(&similar().threshold(0));
        assert!(limits.is_empty());
        // The two empty files share an OID, so the exact pass pairs them.
        assert_eq!(summary.iter().filter(|s| s.starts_with('R')).count(), 1);
        assert!(summary
            .iter()
            .any(|s| s.starts_with("R empty_old -> empty_new_dir/x Some(100)")));
        assert!(summary.iter().any(|s| s.starts_with("D  -> bin_old")));
        assert!(summary.iter().any(|s| s.starts_with("A  -> link_new")));
    }

    #[test]
    fn limits_keep_additions_and_deletions_and_are_reported() {
        let mut f = Fixture::new();
        for i in 0..4 {
            let mut edited = lines(0..10);
            edited.extend(format!("edit {}\n", i).bytes());
            f.deleted(&format!("old{}", i), &lines(0..10), FileMode::Regular);
            f.added(&format!("new{}", i), &edited, FileMode::Regular);
        }
        // The deleted files share an OID, but none is identical to an added file.
        let (summary, limits) = f.run_all(&similar().max_pairs(15));
        assert_eq!(
            limits,
            [RenameLimit::TooManyPairs {
                pairs: 16,
                limit: 15
            }]
        );
        assert_eq!(summary.len(), 8);
        assert!(summary.iter().all(|s| !s.starts_with('R')));

        let (summary, limits) = f.run_all(&similar().max_pairs(16));
        assert!(limits.is_empty());
        assert_eq!(summary.iter().filter(|s| s.starts_with('R')).count(), 4);

        let (summary, limits) = f.run_all(&similar().max_file_size(80));
        assert_eq!(
            limits,
            [RenameLimit::FileTooLarge {
                files: 8,
                limit: 80
            }]
        );
        assert!(summary.iter().all(|s| !s.starts_with('R')));
    }
}
