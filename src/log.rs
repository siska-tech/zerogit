//! Git commit log iteration.
//!
//! This module provides an iterator for traversing commit history
//! in reverse chronological order (newest first).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::PathBuf;

use crate::error::Result;
use crate::objects::{Commit, LooseObjectStore, ObjectType, Oid};

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
}

impl LogIterator {
    /// Creates a new LogIterator starting from the given OID.
    ///
    /// # Arguments
    ///
    /// * `objects_dir` - Path to the `.git/objects` directory.
    /// * `start_oid` - The OID of the commit to start from.
    pub fn new(objects_dir: PathBuf, start_oid: Oid) -> Result<Self> {
        let store = LooseObjectStore::new(&objects_dir);
        let mut pending = BinaryHeap::new();
        let visited = HashSet::new();

        // Read the initial commit to get its timestamp
        let raw = store.read(&start_oid)?;
        let commit = Commit::parse(raw)?;

        pending.push(PendingCommit {
            oid: start_oid,
            timestamp: commit.author().timestamp(),
        });

        Ok(LogIterator {
            store,
            pending,
            visited,
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

        Commit::parse(raw)
    }
}

impl Iterator for LogIterator {
    type Item = Result<Commit>;

    fn next(&mut self) -> Option<Self::Item> {
        // Find the next unvisited commit
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
            for parent_oid in commit.parents() {
                if !self.visited.contains(parent_oid) {
                    // Read parent to get its timestamp
                    match self.read_commit(parent_oid) {
                        Ok(parent_commit) => {
                            self.pending.push(PendingCommit {
                                oid: *parent_oid,
                                timestamp: parent_commit.author().timestamp(),
                            });
                        }
                        Err(e) => {
                            // If we can't read a parent, return the error on next iteration
                            // For now, we continue
                            return Some(Err(e));
                        }
                    }
                }
            }

            return Some(Ok(commit));
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
}
