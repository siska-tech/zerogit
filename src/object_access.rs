//! Reading objects by ID: commits, trees, blobs and objects of any type.

use crate::error::{Error, Result};
use crate::objects::{Blob, Commit, Object, ObjectReader, ObjectType, Oid, Tree};
use crate::repository::Repository;

impl Repository {
    /// Resolves a short (abbreviated) OID to a full OID.
    ///
    /// # Arguments
    ///
    /// * `short_oid` - A hexadecimal string of at least 4 characters.
    ///
    /// # Returns
    ///
    /// The full OID if exactly one object matches the prefix.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidOid` if the prefix is too short or contains invalid characters.
    /// - `Error::ObjectNotFound` if no object matches the prefix.
    /// - `Error::InvalidOid` if multiple objects match the prefix (ambiguous).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let full_oid = repo.resolve_short_oid("abc1234").unwrap();
    /// ```
    pub fn resolve_short_oid(&self, short_oid: &str) -> Result<Oid> {
        self.object_store().resolve_oid(short_oid)
    }

    /// Retrieves a commit by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - A revision: a full or abbreviated OID, or anything
    ///   [`Repository::rev_parse`] accepts (`HEAD`, `main~2`, `v1.0^{}`, ...).
    ///
    /// # Returns
    ///
    /// The commit object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a commit.
    /// - `Error::InvalidRevision` if the revision cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let commit = repo.commit("abc1234").unwrap();
    /// println!("Author: {}", commit.author().name());
    /// ```
    pub fn commit(&self, oid_str: &str) -> Result<Commit> {
        let oid = self.rev_parse(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Commit {
            return Err(Error::TypeMismatch {
                expected: "commit",
                actual: raw.object_type.as_str(),
            });
        }

        Commit::parse(oid, raw)
    }

    /// Retrieves a tree by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - A revision: a full or abbreviated OID, or anything
    ///   [`Repository::rev_parse`] accepts (`HEAD`, `main~2`, `v1.0^{}`, ...).
    ///
    /// # Returns
    ///
    /// The tree object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a tree.
    /// - `Error::InvalidRevision` if the revision cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let tree = repo.tree("abc1234").unwrap();
    /// for entry in tree.iter() {
    ///     println!("{}: {}", entry.mode().as_octal(), entry.name());
    /// }
    /// ```
    pub fn tree(&self, oid_str: &str) -> Result<Tree> {
        let oid = self.rev_parse(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Tree {
            return Err(Error::TypeMismatch {
                expected: "tree",
                actual: raw.object_type.as_str(),
            });
        }

        Tree::parse(raw)
    }

    /// Retrieves a blob by its OID.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - A revision: a full or abbreviated OID, or anything
    ///   [`Repository::rev_parse`] accepts (`HEAD`, `main~2`, `v1.0^{}`, ...).
    ///
    /// # Returns
    ///
    /// The blob object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::TypeMismatch` if the object is not a blob.
    /// - `Error::InvalidRevision` if the revision cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let blob = repo.blob("abc1234").unwrap();
    /// println!("Size: {} bytes", blob.size());
    /// ```
    pub fn blob(&self, oid_str: &str) -> Result<Blob> {
        let oid = self.rev_parse(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        if raw.object_type != ObjectType::Blob {
            return Err(Error::TypeMismatch {
                expected: "blob",
                actual: raw.object_type.as_str(),
            });
        }

        Blob::parse(raw)
    }

    /// Opens a blob for reading as a stream, for files too large to hold in
    /// memory.
    ///
    /// `revision` is resolved as for [`Repository::blob`] (`HEAD:big.bin`
    /// names the file in HEAD). A loose blob, or one stored whole in a pack
    /// (as Git stores files larger than `core.bigFileThreshold`), is
    /// inflated as it is read, using a small, fixed amount of memory and
    /// no size limit; one stored as a delta is rebuilt in memory, within
    /// [`PackLimits`](crate::objects::pack::PackLimits). The content is
    /// checked against the object ID when the end is reached.
    ///
    /// # Errors
    ///
    /// The errors of [`Repository::blob`]; reading fails with an
    /// [`std::io::ErrorKind::InvalidData`] error if the content turns out
    /// not to match the object ID.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let mut reader = repo.blob_reader("HEAD:video.mp4").unwrap();
    /// let mut file = std::fs::File::create("video.mp4").unwrap();
    /// std::io::copy(&mut reader, &mut file).unwrap();
    /// ```
    pub fn blob_reader(&self, revision: &str) -> Result<ObjectReader> {
        let reader = self.object_reader(revision)?;
        if reader.object_type() != ObjectType::Blob {
            return Err(Error::TypeMismatch {
                expected: "blob",
                actual: reader.object_type().as_str(),
            });
        }
        Ok(reader)
    }

    /// Opens an object of any type for reading as a stream (see
    /// [`Repository::blob_reader`]).
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    /// - `Error::InvalidRevision` if the revision cannot be resolved.
    pub fn object_reader(&self, revision: &str) -> Result<ObjectReader> {
        let oid = self.rev_parse(revision)?;
        self.object_store().open(&oid)
    }

    /// Retrieves a Git object by its OID.
    ///
    /// This method returns the object as a unified `Object` enum,
    /// which can be any of blob, tree, or commit.
    ///
    /// # Arguments
    ///
    /// * `oid_str` - A revision: a full or abbreviated OID, or anything
    ///   [`Repository::rev_parse`] accepts (`HEAD`, `main~2`, `v1.0^{}`, ...).
    ///
    /// # Returns
    ///
    /// The object on success.
    ///
    /// # Errors
    ///
    /// - `Error::ObjectNotFound` if the object does not exist.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    /// use zerogit::objects::Object;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let obj = repo.object("abc1234").unwrap();
    /// match obj {
    ///     Object::Blob(blob) => println!("Blob: {} bytes", blob.size()),
    ///     Object::Tree(tree) => println!("Tree: {} entries", tree.len()),
    ///     Object::Commit(commit) => println!("Commit: {}", commit.summary()),
    /// }
    /// ```
    pub fn object(&self, oid_str: &str) -> Result<Object> {
        let oid = self.rev_parse(oid_str)?;
        let store = self.object_store();
        let raw = store.read(&oid)?;

        match raw.object_type {
            ObjectType::Blob => Ok(Object::Blob(Blob::parse(raw)?)),
            ObjectType::Tree => Ok(Object::Tree(Tree::parse(raw)?)),
            ObjectType::Commit => Ok(Object::Commit(Commit::parse(oid, raw)?)),
            ObjectType::Tag => Err(Error::InvalidObject {
                oid: oid.to_hex(),
                reason: "tag objects are not yet supported".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::*;
    use tempfile::TempDir;

    // =========================================================================
    // Object retrieval tests (RP-010 to RP-013, RP-030 to RP-034)
    // =========================================================================

    // RP-010: repository.commit() with full SHA returns Ok(Commit)
    #[test]
    fn test_commit_full_sha_returns_ok() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create a tree first (empty tree)
        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        // Create a commit
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Initial commit");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.commit(&commit_oid.to_hex());
        assert!(commit.is_ok());

        let commit = commit.unwrap();
        assert_eq!(commit.summary(), "Initial commit");
        assert_eq!(commit.tree().to_hex(), tree_oid.to_hex());
    }

    // RP-011: repository.commit() with short SHA returns Ok(Commit)
    #[test]
    fn test_commit_short_sha_returns_ok() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Test commit");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let short_sha = &commit_oid.to_hex()[..7];
        let commit = repo.commit(short_sha);
        assert!(commit.is_ok());
        assert_eq!(commit.unwrap().summary(), "Test commit");
    }

    // RP-012: repository.commit() with too short SHA returns InvalidOid
    #[test]
    fn test_commit_too_short_sha_returns_error() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.commit("abc");
        assert!(matches!(result, Err(Error::InvalidOid(_))));
    }

    // RP-013: repository.commit() with nonexistent SHA returns ObjectNotFound
    #[test]
    fn test_commit_nonexistent_sha_returns_object_not_found() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.commit("0000000000000000000000000000000000000000");
        assert!(matches!(result, Err(Error::ObjectNotFound(_))));
    }

    // RP-030: repository.object() with blob SHA returns Object::Blob
    #[test]
    fn test_object_blob_returns_blob() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"Hello, World!", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&blob_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Blob(_)));
        if let Object::Blob(blob) = obj {
            assert_eq!(blob.content(), b"Hello, World!");
        }
    }

    // RP-031: repository.object() with tree SHA returns Object::Tree
    #[test]
    fn test_object_tree_returns_tree() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create an empty tree
        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&tree_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Tree(_)));
        if let Object::Tree(tree) = obj {
            assert!(tree.is_empty());
        }
    }

    // RP-032: repository.object() with commit SHA returns Object::Commit
    #[test]
    fn test_object_commit_returns_commit() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let commit_content = make_commit_content(&tree_oid.to_hex(), None, "Test message");
        let commit_oid = create_loose_object(&git_dir, commit_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let obj = repo.object(&commit_oid.to_hex()).unwrap();

        assert!(matches!(obj, Object::Commit(_)));
        if let Object::Commit(commit) = obj {
            assert_eq!(commit.summary(), "Test message");
        }
    }

    // RP-033: repository.tree() with blob SHA returns TypeMismatch
    #[test]
    fn test_tree_with_blob_sha_returns_type_mismatch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"blob content", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.tree(&blob_oid.to_hex());

        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "tree",
                actual: "blob"
            })
        ));
    }

    // RP-034: repository.blob() with tree SHA returns TypeMismatch
    #[test]
    fn test_blob_with_tree_sha_returns_type_mismatch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.blob(&tree_oid.to_hex());

        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "blob",
                actual: "tree"
            })
        ));
    }

    // Additional: resolve_short_oid with full OID
    #[test]
    fn test_resolve_short_oid_full_oid() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let blob_oid = create_loose_object(&git_dir, b"test", "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let resolved = repo.resolve_short_oid(&blob_oid.to_hex()).unwrap();
        assert_eq!(resolved, blob_oid);
    }

    // Additional: resolve_short_oid with nonexistent prefix
    #[test]
    fn test_resolve_short_oid_not_found() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();
        let result = repo.resolve_short_oid("0000000");
        assert!(matches!(result, Err(Error::ObjectNotFound(_))));
    }

    // Additional: blob() returns correct content
    #[test]
    fn test_blob_returns_content() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let content = b"fn main() { println!(\"Hello\"); }";
        let blob_oid = create_loose_object(&git_dir, content, "blob");

        let repo = Repository::open(temp.path()).unwrap();
        let blob = repo.blob(&blob_oid.to_hex()).unwrap();

        assert_eq!(blob.content(), content);
        assert_eq!(blob.size(), content.len());
    }

    // Additional: tree() parses entries correctly
    #[test]
    fn test_tree_parses_entries() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create a blob first
        let blob_oid = create_loose_object(&git_dir, b"file content", "blob");

        // Create tree content: "100644 file.txt\0<20-byte-sha>"
        let mut tree_content = Vec::new();
        tree_content.extend_from_slice(b"100644 file.txt\0");
        tree_content.extend_from_slice(blob_oid.as_bytes());

        let tree_oid = create_loose_object(&git_dir, &tree_content, "tree");

        let repo = Repository::open(temp.path()).unwrap();
        let tree = repo.tree(&tree_oid.to_hex()).unwrap();

        assert_eq!(tree.len(), 1);
        let entry = tree.get("file.txt").unwrap();
        assert_eq!(entry.name(), "file.txt");
        assert_eq!(entry.oid(), &blob_oid);
    }

    // Additional: commit() with parent
    #[test]
    fn test_commit_with_parent() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        let tree_oid = create_loose_object(&git_dir, b"", "tree");
        let parent_content = make_commit_content(&tree_oid.to_hex(), None, "First commit");
        let parent_oid = create_loose_object(&git_dir, parent_content.as_bytes(), "commit");

        let child_content = make_commit_content(
            &tree_oid.to_hex(),
            Some(&parent_oid.to_hex()),
            "Second commit",
        );
        let child_oid = create_loose_object(&git_dir, child_content.as_bytes(), "commit");

        let repo = Repository::open(temp.path()).unwrap();
        let commit = repo.commit(&child_oid.to_hex()).unwrap();

        assert_eq!(commit.summary(), "Second commit");
        assert_eq!(commit.parents().len(), 1);
        assert_eq!(commit.parent().unwrap(), &parent_oid);
    }
}
