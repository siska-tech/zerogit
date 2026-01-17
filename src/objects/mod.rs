//! Git object types (blob, tree, commit, tag).

pub mod blob;
pub mod commit;
pub mod oid;
pub mod store;
pub mod tree;

pub use blob::Blob;
pub use commit::{Commit, Signature};
pub use oid::Oid;
pub use store::{LooseObjectStore, ObjectType, RawObject};
pub use tree::{FileMode, Tree, TreeEntry};

/// A unified enum representing any Git object type.
///
/// This enum allows handling different Git object types uniformly
/// while still providing type-safe access to specific object data.
#[derive(Debug, Clone)]
pub enum Object {
    /// A blob object containing file content.
    Blob(Blob),
    /// A tree object containing directory entries.
    Tree(Tree),
    /// A commit object containing commit metadata.
    Commit(Commit),
}

impl Object {
    /// Returns the type of this object.
    pub fn kind(&self) -> ObjectType {
        match self {
            Object::Blob(_) => ObjectType::Blob,
            Object::Tree(_) => ObjectType::Tree,
            Object::Commit(_) => ObjectType::Commit,
        }
    }

    /// Returns a reference to the inner Blob if this is a Blob object.
    pub fn as_blob(&self) -> Option<&Blob> {
        match self {
            Object::Blob(blob) => Some(blob),
            _ => None,
        }
    }

    /// Returns a reference to the inner Tree if this is a Tree object.
    pub fn as_tree(&self) -> Option<&Tree> {
        match self {
            Object::Tree(tree) => Some(tree),
            _ => None,
        }
    }

    /// Returns a reference to the inner Commit if this is a Commit object.
    pub fn as_commit(&self) -> Option<&Commit> {
        match self {
            Object::Commit(commit) => Some(commit),
            _ => None,
        }
    }

    /// Consumes this Object and returns the inner Blob if this is a Blob object.
    pub fn into_blob(self) -> Option<Blob> {
        match self {
            Object::Blob(blob) => Some(blob),
            _ => None,
        }
    }

    /// Consumes this Object and returns the inner Tree if this is a Tree object.
    pub fn into_tree(self) -> Option<Tree> {
        match self {
            Object::Tree(tree) => Some(tree),
            _ => None,
        }
    }

    /// Consumes this Object and returns the inner Commit if this is a Commit object.
    pub fn into_commit(self) -> Option<Commit> {
        match self {
            Object::Commit(commit) => Some(commit),
            _ => None,
        }
    }
}

impl From<Blob> for Object {
    fn from(blob: Blob) -> Self {
        Object::Blob(blob)
    }
}

impl From<Tree> for Object {
    fn from(tree: Tree) -> Self {
        Object::Tree(tree)
    }
}

impl From<Commit> for Object {
    fn from(commit: Commit) -> Self {
        Object::Commit(commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::RawObject;

    fn make_blob_raw(content: &[u8]) -> RawObject {
        RawObject {
            object_type: ObjectType::Blob,
            content: content.to_vec(),
        }
    }

    fn make_tree_raw() -> RawObject {
        RawObject {
            object_type: ObjectType::Tree,
            content: vec![],
        }
    }

    fn make_commit_raw() -> RawObject {
        let content = "tree da39a3ee5e6b4b0d3255bfef95601890afd80709\n\
                       author John Doe <john@example.com> 1234567890 +0000\n\
                       committer John Doe <john@example.com> 1234567890 +0000\n\
                       \n\
                       Test commit";
        RawObject {
            object_type: ObjectType::Commit,
            content: content.as_bytes().to_vec(),
        }
    }

    // O-001: Object::Blob can be created from Blob
    #[test]
    fn test_object_from_blob() {
        let blob = Blob::parse(make_blob_raw(b"Hello")).unwrap();
        let obj = Object::from(blob);
        assert!(matches!(obj, Object::Blob(_)));
    }

    // O-002: Object::Tree can be created from Tree
    #[test]
    fn test_object_from_tree() {
        let tree = Tree::parse(make_tree_raw()).unwrap();
        let obj = Object::from(tree);
        assert!(matches!(obj, Object::Tree(_)));
    }

    // O-003: Object::Commit can be created from Commit
    #[test]
    fn test_object_from_commit() {
        let commit = Commit::parse(make_commit_raw()).unwrap();
        let obj = Object::from(commit);
        assert!(matches!(obj, Object::Commit(_)));
    }

    // O-004: kind() returns correct ObjectType
    #[test]
    fn test_kind() {
        let blob = Object::from(Blob::parse(make_blob_raw(b"test")).unwrap());
        assert_eq!(blob.kind(), ObjectType::Blob);

        let tree = Object::from(Tree::parse(make_tree_raw()).unwrap());
        assert_eq!(tree.kind(), ObjectType::Tree);

        let commit = Object::from(Commit::parse(make_commit_raw()).unwrap());
        assert_eq!(commit.kind(), ObjectType::Commit);
    }

    // O-005: as_blob() returns Some for Blob, None for others
    #[test]
    fn test_as_blob() {
        let blob_obj = Object::from(Blob::parse(make_blob_raw(b"test")).unwrap());
        assert!(blob_obj.as_blob().is_some());
        assert!(blob_obj.as_tree().is_none());
        assert!(blob_obj.as_commit().is_none());
    }

    // O-006: as_tree() returns Some for Tree, None for others
    #[test]
    fn test_as_tree() {
        let tree_obj = Object::from(Tree::parse(make_tree_raw()).unwrap());
        assert!(tree_obj.as_tree().is_some());
        assert!(tree_obj.as_blob().is_none());
        assert!(tree_obj.as_commit().is_none());
    }

    // O-007: as_commit() returns Some for Commit, None for others
    #[test]
    fn test_as_commit() {
        let commit_obj = Object::from(Commit::parse(make_commit_raw()).unwrap());
        assert!(commit_obj.as_commit().is_some());
        assert!(commit_obj.as_blob().is_none());
        assert!(commit_obj.as_tree().is_none());
    }

    // O-008: into_blob() returns Some for Blob, None for others
    #[test]
    fn test_into_blob() {
        let blob_obj = Object::from(Blob::parse(make_blob_raw(b"test")).unwrap());
        let blob = blob_obj.into_blob();
        assert!(blob.is_some());
        assert_eq!(blob.unwrap().content(), b"test");

        let tree_obj = Object::from(Tree::parse(make_tree_raw()).unwrap());
        assert!(tree_obj.into_blob().is_none());
    }

    // O-009: into_tree() returns Some for Tree, None for others
    #[test]
    fn test_into_tree() {
        let tree_obj = Object::from(Tree::parse(make_tree_raw()).unwrap());
        let tree = tree_obj.into_tree();
        assert!(tree.is_some());
        assert!(tree.unwrap().is_empty());

        let blob_obj = Object::from(Blob::parse(make_blob_raw(b"test")).unwrap());
        assert!(blob_obj.into_tree().is_none());
    }

    // O-010: into_commit() returns Some for Commit, None for others
    #[test]
    fn test_into_commit() {
        let commit_obj = Object::from(Commit::parse(make_commit_raw()).unwrap());
        let commit = commit_obj.into_commit();
        assert!(commit.is_some());
        assert_eq!(commit.unwrap().summary(), "Test commit");

        let blob_obj = Object::from(Blob::parse(make_blob_raw(b"test")).unwrap());
        assert!(blob_obj.into_commit().is_none());
    }

    // O-011: as_* methods return references to inner data
    #[test]
    fn test_as_methods_return_references() {
        let blob_obj = Object::from(Blob::parse(make_blob_raw(b"content")).unwrap());
        let blob_ref = blob_obj.as_blob().unwrap();
        assert_eq!(blob_ref.content(), b"content");

        // Object is still usable after as_* call
        assert_eq!(blob_obj.kind(), ObjectType::Blob);
    }
}
