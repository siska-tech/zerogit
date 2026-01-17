//! Git blob object implementation.

use super::store::{ObjectType, RawObject};
use crate::error::{Error, Result};

/// A Git blob object representing file content.
///
/// Blobs store the raw content of files in a Git repository.
/// They do not contain any metadata like filename or permissions;
/// that information is stored in tree objects.
#[derive(Debug, Clone)]
pub struct Blob {
    /// The raw content of the blob.
    content: Vec<u8>,
}

impl Blob {
    /// Parses a Blob from a RawObject.
    ///
    /// # Arguments
    ///
    /// * `raw` - The raw object to parse.
    ///
    /// # Returns
    ///
    /// The parsed Blob on success, or `Error::TypeMismatch` if the object
    /// is not a blob.
    pub fn parse(raw: RawObject) -> Result<Self> {
        if raw.object_type != ObjectType::Blob {
            return Err(Error::TypeMismatch {
                expected: "blob",
                actual: raw.object_type.as_str(),
            });
        }

        Ok(Blob {
            content: raw.content,
        })
    }

    /// Returns the raw content of the blob.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Returns the content as a UTF-8 string, if valid.
    ///
    /// # Returns
    ///
    /// `Ok(&str)` if the content is valid UTF-8, or `Err(Error::InvalidUtf8)`
    /// if it contains invalid UTF-8 sequences.
    pub fn content_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.content).map_err(|_| Error::InvalidUtf8)
    }

    /// Returns the size of the blob content in bytes.
    pub fn size(&self) -> usize {
        self.content.len()
    }

    /// Returns true if the content appears to be binary.
    ///
    /// A file is considered binary if it contains a null byte (0x00)
    /// within the first 8000 bytes, which is the same heuristic
    /// used by Git.
    pub fn is_binary(&self) -> bool {
        let check_len = self.content.len().min(8000);
        self.content[..check_len].contains(&0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob(content: &[u8]) -> RawObject {
        RawObject {
            object_type: ObjectType::Blob,
            content: content.to_vec(),
        }
    }

    fn make_tree(content: &[u8]) -> RawObject {
        RawObject {
            object_type: ObjectType::Tree,
            content: content.to_vec(),
        }
    }

    // B-001: Parse blob from RawObject
    #[test]
    fn test_parse_blob() {
        let raw = make_blob(b"Hello, World!");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.content(), b"Hello, World!");
    }

    // B-002: Parse returns TypeMismatch for non-blob
    #[test]
    fn test_parse_type_mismatch() {
        let raw = make_tree(b"tree content");
        let result = Blob::parse(raw);
        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "blob",
                actual: "tree"
            })
        ));
    }

    // B-003: content() returns raw bytes
    #[test]
    fn test_content() {
        let data = b"fn main() { println!(\"Hello\"); }";
        let blob = Blob::parse(make_blob(data)).unwrap();
        assert_eq!(blob.content(), data);
    }

    // B-004: content_str() returns UTF-8 string
    #[test]
    fn test_content_str_valid() {
        let raw = make_blob(b"Hello, World!");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.content_str().unwrap(), "Hello, World!");
    }

    // B-005: content_str() returns error for invalid UTF-8
    #[test]
    fn test_content_str_invalid() {
        // Invalid UTF-8 sequence
        let raw = make_blob(&[0xFF, 0xFE, 0x00, 0x01]);
        let blob = Blob::parse(raw).unwrap();
        assert!(matches!(blob.content_str(), Err(Error::InvalidUtf8)));
    }

    // B-006: size() returns content length
    #[test]
    fn test_size() {
        let raw = make_blob(b"Hello");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.size(), 5);

        let raw = make_blob(b"");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.size(), 0);
    }

    // B-007: is_binary() returns true for binary content
    #[test]
    fn test_is_binary_true() {
        // Contains null byte
        let raw = make_blob(&[0x89, 0x50, 0x4E, 0x47, 0x00, 0x00]);
        let blob = Blob::parse(raw).unwrap();
        assert!(blob.is_binary());
    }

    // B-008: is_binary() returns false for text content
    #[test]
    fn test_is_binary_false() {
        let raw = make_blob(b"Hello, World!\nThis is text.");
        let blob = Blob::parse(raw).unwrap();
        assert!(!blob.is_binary());
    }

    // B-009: is_binary() only checks first 8000 bytes
    #[test]
    fn test_is_binary_8000_limit() {
        // Null byte at position 8001 should not be detected
        let mut content = vec![b'a'; 8001];
        content[8000] = 0x00;
        let raw = make_blob(&content);
        let blob = Blob::parse(raw).unwrap();
        assert!(!blob.is_binary());

        // Null byte at position 7999 should be detected
        let mut content = vec![b'a'; 8000];
        content[7999] = 0x00;
        let raw = make_blob(&content);
        let blob = Blob::parse(raw).unwrap();
        assert!(blob.is_binary());
    }

    // Additional: Empty blob
    #[test]
    fn test_empty_blob() {
        let raw = make_blob(b"");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.content(), b"");
        assert_eq!(blob.content_str().unwrap(), "");
        assert_eq!(blob.size(), 0);
        assert!(!blob.is_binary());
    }

    // Additional: Blob with newlines
    #[test]
    fn test_blob_with_newlines() {
        let raw = make_blob(b"line1\nline2\nline3\n");
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.content_str().unwrap(), "line1\nline2\nline3\n");
    }

    // Additional: Large blob
    #[test]
    fn test_large_blob() {
        let content: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        // This will contain 0x00 at positions 0, 256, 512, etc.
        let raw = make_blob(&content);
        let blob = Blob::parse(raw).unwrap();
        assert_eq!(blob.size(), 10000);
        assert!(blob.is_binary()); // Contains null bytes
    }
}
