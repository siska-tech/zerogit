//! Git loose object store implementation.

use std::fs;
use std::path::{Path, PathBuf};

use super::oid::{Oid, OID_HEX_LEN};
use crate::error::{Error, Result};
use crate::infra::{compress, decompress, hash_object, read_file, write_file_atomic};

/// The type of a Git object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    /// A blob (file content).
    Blob,
    /// A tree (directory listing).
    Tree,
    /// A commit.
    Commit,
    /// A tag.
    Tag,
}

impl ObjectType {
    /// Returns the type name as used in Git object headers.
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectType::Blob => "blob",
            ObjectType::Tree => "tree",
            ObjectType::Commit => "commit",
            ObjectType::Tag => "tag",
        }
    }

    /// Parses a type name from a Git object header.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "blob" => Some(ObjectType::Blob),
            "tree" => Some(ObjectType::Tree),
            "commit" => Some(ObjectType::Commit),
            "tag" => Some(ObjectType::Tag),
            _ => None,
        }
    }
}

/// A raw Git object with its type and content.
#[derive(Debug, Clone)]
pub struct RawObject {
    /// The type of the object.
    pub object_type: ObjectType,
    /// The raw content of the object (without the header).
    pub content: Vec<u8>,
}

/// A store for reading loose Git objects.
///
/// Loose objects are stored in `.git/objects/` as individual zlib-compressed
/// files, with the path determined by the object's SHA-1 hash.
#[derive(Debug)]
pub struct LooseObjectStore {
    /// Path to the objects directory (e.g., `.git/objects`).
    objects_dir: PathBuf,
}

impl LooseObjectStore {
    /// Creates a new LooseObjectStore for the given objects directory.
    ///
    /// # Arguments
    ///
    /// * `objects_dir` - Path to the `.git/objects` directory.
    pub fn new<P: AsRef<Path>>(objects_dir: P) -> Self {
        LooseObjectStore {
            objects_dir: objects_dir.as_ref().to_path_buf(),
        }
    }

    /// Converts an Oid to the path of its loose object file.
    ///
    /// For example, `da39a3ee5e6b4b0d3255bfef95601890afd80709` becomes
    /// `objects/da/39a3ee5e6b4b0d3255bfef95601890afd80709`.
    pub fn oid_to_path(&self, oid: &Oid) -> PathBuf {
        let hex = oid.to_hex();
        self.objects_dir.join(&hex[..2]).join(&hex[2..])
    }

    /// Reads the raw compressed data for an object.
    fn read_raw(&self, oid: &Oid) -> Result<Vec<u8>> {
        let path = self.oid_to_path(oid);
        read_file(&path).map_err(|e| {
            if matches!(e, Error::PathNotFound(_)) {
                Error::ObjectNotFound(oid.to_hex())
            } else {
                e
            }
        })
    }

    /// Parses a raw decompressed object into its type and content.
    ///
    /// Git objects have the format: `<type> <size>\0<content>`
    fn parse_raw_object(data: &[u8], oid: &Oid) -> Result<RawObject> {
        // Find the null byte that separates header from content
        let null_pos = data
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::InvalidObject {
                oid: oid.to_hex(),
                reason: "missing null byte in header".to_string(),
            })?;

        // Parse the header
        let header = std::str::from_utf8(&data[..null_pos]).map_err(|_| Error::InvalidObject {
            oid: oid.to_hex(),
            reason: "invalid UTF-8 in header".to_string(),
        })?;

        // Split into type and size
        let mut parts = header.split(' ');
        let type_str = parts.next().ok_or_else(|| Error::InvalidObject {
            oid: oid.to_hex(),
            reason: "missing object type".to_string(),
        })?;

        let size_str = parts.next().ok_or_else(|| Error::InvalidObject {
            oid: oid.to_hex(),
            reason: "missing object size".to_string(),
        })?;

        // Parse the object type
        let object_type = ObjectType::parse(type_str).ok_or_else(|| Error::InvalidObject {
            oid: oid.to_hex(),
            reason: format!("unknown object type: {}", type_str),
        })?;

        // Parse and validate the size
        let size: usize = size_str.parse().map_err(|_| Error::InvalidObject {
            oid: oid.to_hex(),
            reason: format!("invalid size: {}", size_str),
        })?;

        let content = &data[null_pos + 1..];
        if content.len() != size {
            return Err(Error::InvalidObject {
                oid: oid.to_hex(),
                reason: format!(
                    "size mismatch: header says {} but content is {} bytes",
                    size,
                    content.len()
                ),
            });
        }

        Ok(RawObject {
            object_type,
            content: content.to_vec(),
        })
    }

    /// Reads and parses a Git object by its Oid.
    ///
    /// # Arguments
    ///
    /// * `oid` - The object ID to read.
    ///
    /// # Returns
    ///
    /// The parsed object on success, or an error if the object cannot be read
    /// or is invalid.
    pub fn read(&self, oid: &Oid) -> Result<RawObject> {
        let compressed = self.read_raw(oid)?;
        let decompressed = decompress(&compressed)?;
        Self::parse_raw_object(&decompressed, oid)
    }

    /// Checks if an object exists in the store.
    ///
    /// # Arguments
    ///
    /// * `oid` - The object ID to check.
    ///
    /// # Returns
    ///
    /// `true` if the object exists, `false` otherwise.
    pub fn exists(&self, oid: &Oid) -> bool {
        self.oid_to_path(oid).exists()
    }

    /// Finds objects whose Oid starts with the given prefix.
    ///
    /// This is used to resolve abbreviated SHA-1 hashes.
    ///
    /// # Arguments
    ///
    /// * `prefix` - A hexadecimal prefix (at least 4 characters).
    ///
    /// # Returns
    ///
    /// A vector of matching Oids, or an error if the prefix is invalid.
    pub fn find_objects_by_prefix(&self, prefix: &str) -> Result<Vec<Oid>> {
        // Validate prefix
        if prefix.len() < 4 {
            return Err(Error::InvalidOid(prefix.to_string()));
        }

        if prefix.len() > OID_HEX_LEN {
            return Err(Error::InvalidOid(prefix.to_string()));
        }

        // Validate that prefix contains only hex characters
        if !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidOid(prefix.to_string()));
        }

        let prefix_lower = prefix.to_lowercase();
        let dir_prefix = &prefix_lower[..2];
        let file_prefix = if prefix_lower.len() > 2 {
            &prefix_lower[2..]
        } else {
            ""
        };

        let subdir = self.objects_dir.join(dir_prefix);
        if !subdir.exists() {
            return Ok(Vec::new());
        }

        let mut matches = Vec::new();

        let entries = fs::read_dir(&subdir)?;
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if name.starts_with(file_prefix) {
                let full_hex = format!("{}{}", dir_prefix, name);
                if full_hex.len() == OID_HEX_LEN {
                    if let Ok(oid) = Oid::from_hex(&full_hex) {
                        matches.push(oid);
                    }
                }
            }
        }

        Ok(matches)
    }

    /// Writes a Git object to the store.
    ///
    /// This function:
    /// 1. Creates the raw object data with Git header: `<type> <size>\0<content>`
    /// 2. Computes the SHA-1 hash of the raw data
    /// 3. Compresses the data using zlib
    /// 4. Writes to the correct path based on the hash
    ///
    /// # Arguments
    ///
    /// * `object_type` - The type of object (blob, tree, commit, tag).
    /// * `content` - The content of the object.
    ///
    /// # Returns
    ///
    /// The Oid of the written object.
    pub fn write(&self, object_type: ObjectType, content: &[u8]) -> Result<Oid> {
        // Create the raw object data with header
        let header = format!("{} {}\0", object_type.as_str(), content.len());
        let mut raw = header.into_bytes();
        raw.extend_from_slice(content);

        // Compute the hash
        let hash = hash_object(object_type.as_str(), content);
        let oid = Oid::from_bytes(hash);

        // Check if object already exists (idempotent)
        let path = self.oid_to_path(&oid);
        if path.exists() {
            return Ok(oid);
        }

        // Compress the data
        let compressed = compress(&raw);

        // Write to the object store
        write_file_atomic(&path, &compressed)?;

        Ok(oid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::hash_object;
    use miniz_oxide::deflate::compress_to_vec_zlib;
    use tempfile::TempDir;

    /// Helper to create a loose object file
    fn create_loose_object(objects_dir: &Path, content: &[u8], object_type: &str) -> Oid {
        // Create the raw object data
        let header = format!("{} {}\0", object_type, content.len());
        let mut raw = header.into_bytes();
        raw.extend_from_slice(content);

        // Hash it
        let oid = Oid::from_bytes(hash_object(object_type, content));

        // Compress it
        let compressed = compress_to_vec_zlib(&raw, 6);

        // Write to the correct path
        let hex = oid.to_hex();
        let object_path = objects_dir.join(&hex[..2]).join(&hex[2..]);
        fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        fs::write(&object_path, &compressed).unwrap();

        oid
    }

    // S-001: oid_to_path generates correct path
    #[test]
    fn test_oid_to_path() {
        let store = LooseObjectStore::new("/repo/.git/objects");
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let path = store.oid_to_path(&oid);

        assert!(path.to_string_lossy().contains("da"));
        assert!(path
            .to_string_lossy()
            .contains("39a3ee5e6b4b0d3255bfef95601890afd80709"));
    }

    // S-002: read() returns object content
    #[test]
    fn test_read_blob() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let content = b"Hello, World!";
        let oid = create_loose_object(&objects_dir, content, "blob");

        let store = LooseObjectStore::new(&objects_dir);
        let obj = store.read(&oid).unwrap();

        assert_eq!(obj.object_type, ObjectType::Blob);
        assert_eq!(obj.content, content);
    }

    // S-003: read() handles different object types
    #[test]
    fn test_read_different_types() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        // Test blob
        let blob_oid = create_loose_object(&objects_dir, b"blob content", "blob");
        let store = LooseObjectStore::new(&objects_dir);
        assert_eq!(store.read(&blob_oid).unwrap().object_type, ObjectType::Blob);

        // Test tree (simplified - real trees have binary content)
        let tree_oid = create_loose_object(&objects_dir, b"tree content", "tree");
        assert_eq!(store.read(&tree_oid).unwrap().object_type, ObjectType::Tree);

        // Test commit
        let commit_oid = create_loose_object(&objects_dir, b"commit content", "commit");
        assert_eq!(
            store.read(&commit_oid).unwrap().object_type,
            ObjectType::Commit
        );
    }

    // S-004: read() returns ObjectNotFound for missing objects
    #[test]
    fn test_read_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let oid = Oid::from_hex("0000000000000000000000000000000000000000").unwrap();

        let result = store.read(&oid);
        assert!(matches!(result, Err(Error::ObjectNotFound(_))));
    }

    // S-005: exists() returns correct values
    #[test]
    fn test_exists() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let oid = create_loose_object(&objects_dir, b"test", "blob");
        let store = LooseObjectStore::new(&objects_dir);

        assert!(store.exists(&oid));

        let missing = Oid::from_hex("0000000000000000000000000000000000000000").unwrap();
        assert!(!store.exists(&missing));
    }

    // S-006: find_objects_by_prefix finds matching objects
    #[test]
    fn test_find_by_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let oid = create_loose_object(&objects_dir, b"test content", "blob");
        let store = LooseObjectStore::new(&objects_dir);

        // Search with various prefix lengths
        let hex = oid.to_hex();
        let results = store.find_objects_by_prefix(&hex[..4]).unwrap();
        assert!(results.contains(&oid));

        let results = store.find_objects_by_prefix(&hex[..7]).unwrap();
        assert!(results.contains(&oid));

        let results = store.find_objects_by_prefix(&hex).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], oid);
    }

    // S-007: find_objects_by_prefix returns empty for no matches
    #[test]
    fn test_find_by_prefix_no_match() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let results = store.find_objects_by_prefix("0000").unwrap();
        assert!(results.is_empty());
    }

    // S-008: find_objects_by_prefix validates prefix
    #[test]
    fn test_find_by_prefix_invalid() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);

        // Too short
        let result = store.find_objects_by_prefix("abc");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Invalid characters
        let result = store.find_objects_by_prefix("ghij");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Too long
        let result = store.find_objects_by_prefix("da39a3ee5e6b4b0d3255bfef95601890afd807091");
        assert!(matches!(result, Err(Error::InvalidOid(_))));
    }

    // S-009: ObjectType conversion
    #[test]
    fn test_object_type() {
        assert_eq!(ObjectType::Blob.as_str(), "blob");
        assert_eq!(ObjectType::Tree.as_str(), "tree");
        assert_eq!(ObjectType::Commit.as_str(), "commit");
        assert_eq!(ObjectType::Tag.as_str(), "tag");

        assert_eq!(ObjectType::parse("blob"), Some(ObjectType::Blob));
        assert_eq!(ObjectType::parse("tree"), Some(ObjectType::Tree));
        assert_eq!(ObjectType::parse("commit"), Some(ObjectType::Commit));
        assert_eq!(ObjectType::parse("tag"), Some(ObjectType::Tag));
        assert_eq!(ObjectType::parse("unknown"), None);
    }

    // S-010: parse_raw_object handles malformed data
    #[test]
    fn test_parse_malformed() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();

        // Missing null byte
        let result = LooseObjectStore::parse_raw_object(b"blob 5", &oid);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));

        // Invalid type
        let result = LooseObjectStore::parse_raw_object(b"invalid 5\0hello", &oid);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));

        // Size mismatch
        let result = LooseObjectStore::parse_raw_object(b"blob 10\0hello", &oid);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));
    }

    // S-011: write() creates object file
    #[test]
    fn test_write_blob() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let content = b"Hello, World!";
        let oid = store.write(ObjectType::Blob, content).unwrap();

        // Verify object exists
        assert!(store.exists(&oid));

        // Verify we can read it back
        let obj = store.read(&oid).unwrap();
        assert_eq!(obj.object_type, ObjectType::Blob);
        assert_eq!(obj.content, content);
    }

    // S-012: write() is idempotent
    #[test]
    fn test_write_idempotent() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let content = b"Test content";

        // Write the same object twice
        let oid1 = store.write(ObjectType::Blob, content).unwrap();
        let oid2 = store.write(ObjectType::Blob, content).unwrap();

        // Should get the same OID
        assert_eq!(oid1, oid2);

        // Object should still be readable
        let obj = store.read(&oid1).unwrap();
        assert_eq!(obj.content, content);
    }

    // S-013: write() handles different object types
    #[test]
    fn test_write_different_types() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);

        // Write blob
        let blob_oid = store.write(ObjectType::Blob, b"blob content").unwrap();
        assert_eq!(store.read(&blob_oid).unwrap().object_type, ObjectType::Blob);

        // Write tree (simplified content)
        let tree_oid = store.write(ObjectType::Tree, b"tree content").unwrap();
        assert_eq!(store.read(&tree_oid).unwrap().object_type, ObjectType::Tree);

        // Write commit (simplified content)
        let commit_oid = store.write(ObjectType::Commit, b"commit content").unwrap();
        assert_eq!(
            store.read(&commit_oid).unwrap().object_type,
            ObjectType::Commit
        );
    }

    // S-014: write() creates correct directory structure
    #[test]
    fn test_write_creates_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);
        let content = b"Test";
        let oid = store.write(ObjectType::Blob, content).unwrap();

        // Verify directory structure
        let hex = oid.to_hex();
        let subdir = objects_dir.join(&hex[..2]);
        assert!(subdir.exists());

        let object_path = subdir.join(&hex[2..]);
        assert!(object_path.exists());
    }

    // S-015: write() produces correct hash
    #[test]
    fn test_write_correct_hash() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);

        // Empty blob has a known hash
        let oid = store.write(ObjectType::Blob, b"").unwrap();
        assert_eq!(oid.to_hex(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");

        // "hello\n" blob has a known hash
        let oid = store.write(ObjectType::Blob, b"hello\n").unwrap();
        assert_eq!(oid.to_hex(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    // S-016: write() large content
    #[test]
    fn test_write_large_content() {
        let temp_dir = TempDir::new().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        fs::create_dir(&objects_dir).unwrap();

        let store = LooseObjectStore::new(&objects_dir);

        // Create large content (1MB)
        let content: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
        let oid = store.write(ObjectType::Blob, &content).unwrap();

        // Verify we can read it back
        let obj = store.read(&oid).unwrap();
        assert_eq!(obj.content, content);
    }
}
