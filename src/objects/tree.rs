//! Git tree object implementation.

use super::oid::{Oid, OID_BYTES};
use super::store::{ObjectType, RawObject};
use crate::error::{Error, Result};

/// File mode for tree entries.
///
/// Git uses specific mode values to represent different types of entries
/// in a tree object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// Regular file (non-executable): 100644
    Regular,
    /// Executable file: 100755
    Executable,
    /// Symbolic link: 120000
    Symlink,
    /// Subdirectory (tree): 40000
    Directory,
    /// Git submodule (commit): 160000
    Submodule,
}

impl FileMode {
    /// Parses a file mode from its octal string representation.
    pub fn from_octal(s: &str) -> Option<Self> {
        match s {
            "100644" | "644" => Some(FileMode::Regular),
            "100755" | "755" => Some(FileMode::Executable),
            "120000" => Some(FileMode::Symlink),
            "40000" => Some(FileMode::Directory),
            "160000" => Some(FileMode::Submodule),
            _ => None,
        }
    }

    /// Returns the octal string representation of the mode.
    pub fn as_octal(&self) -> &'static str {
        match self {
            FileMode::Regular => "100644",
            FileMode::Executable => "100755",
            FileMode::Symlink => "120000",
            FileMode::Directory => "40000",
            FileMode::Submodule => "160000",
        }
    }

    /// Returns true if this mode represents a file (blob).
    pub fn is_file(&self) -> bool {
        matches!(
            self,
            FileMode::Regular | FileMode::Executable | FileMode::Symlink
        )
    }

    /// Returns true if this mode represents a directory (tree).
    pub fn is_directory(&self) -> bool {
        matches!(self, FileMode::Directory)
    }

    /// Returns true if this mode represents an executable file.
    pub fn is_executable(&self) -> bool {
        matches!(self, FileMode::Executable)
    }
}

/// An entry in a Git tree object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The file mode of the entry.
    mode: FileMode,
    /// The name of the entry (file or directory name).
    name: String,
    /// The object ID that this entry points to.
    oid: Oid,
}

impl TreeEntry {
    /// Returns the file mode of the entry.
    pub fn mode(&self) -> FileMode {
        self.mode
    }

    /// Returns the name of the entry.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the object ID of the entry.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Returns true if this entry represents a file (blob).
    pub fn is_file(&self) -> bool {
        self.mode.is_file()
    }

    /// Returns true if this entry represents a directory (tree).
    pub fn is_directory(&self) -> bool {
        self.mode.is_directory()
    }
}

/// A Git tree object representing a directory listing.
///
/// Trees contain entries that map names to either blobs (files) or
/// other trees (subdirectories).
#[derive(Debug, Clone)]
pub struct Tree {
    /// The entries in this tree.
    entries: Vec<TreeEntry>,
}

impl Tree {
    /// Parses a Tree from a RawObject.
    ///
    /// Tree objects have a binary format where each entry is:
    /// `<mode> <name>\0<20-byte-sha1>`
    ///
    /// # Arguments
    ///
    /// * `raw` - The raw object to parse.
    ///
    /// # Returns
    ///
    /// The parsed Tree on success, or an error if parsing fails.
    pub fn parse(raw: RawObject) -> Result<Self> {
        if raw.object_type != ObjectType::Tree {
            return Err(Error::TypeMismatch {
                expected: "tree",
                actual: raw.object_type.as_str(),
            });
        }

        let mut entries = Vec::new();
        let content = &raw.content;
        let mut pos = 0;

        while pos < content.len() {
            // Find the space that separates mode from name
            let space_pos = content[pos..]
                .iter()
                .position(|&b| b == b' ')
                .ok_or_else(|| Error::InvalidObject {
                    oid: String::new(),
                    reason: "missing space in tree entry".to_string(),
                })?;

            let mode_bytes = &content[pos..pos + space_pos];
            let mode_str = std::str::from_utf8(mode_bytes).map_err(|_| Error::InvalidObject {
                oid: String::new(),
                reason: "invalid UTF-8 in mode".to_string(),
            })?;

            let mode = FileMode::from_octal(mode_str).ok_or_else(|| Error::InvalidObject {
                oid: String::new(),
                reason: format!("unknown file mode: {}", mode_str),
            })?;

            pos += space_pos + 1; // Skip mode and space

            // Find the null byte that separates name from SHA-1
            let null_pos = content[pos..].iter().position(|&b| b == 0).ok_or_else(|| {
                Error::InvalidObject {
                    oid: String::new(),
                    reason: "missing null byte in tree entry".to_string(),
                }
            })?;

            let name_bytes = &content[pos..pos + null_pos];
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| Error::InvalidObject {
                    oid: String::new(),
                    reason: "invalid UTF-8 in entry name".to_string(),
                })?
                .to_string();

            pos += null_pos + 1; // Skip name and null byte

            // Read the 20-byte SHA-1
            if pos + OID_BYTES > content.len() {
                return Err(Error::InvalidObject {
                    oid: String::new(),
                    reason: "truncated SHA-1 in tree entry".to_string(),
                });
            }

            let mut oid_bytes = [0u8; OID_BYTES];
            oid_bytes.copy_from_slice(&content[pos..pos + OID_BYTES]);
            let oid = Oid::from_bytes(oid_bytes);

            pos += OID_BYTES;

            entries.push(TreeEntry { mode, name, oid });
        }

        Ok(Tree { entries })
    }

    /// Returns a slice of all entries in the tree.
    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    /// Returns the number of entries in the tree.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the tree has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finds an entry by name.
    ///
    /// # Arguments
    ///
    /// * `name` - The name to search for.
    ///
    /// # Returns
    ///
    /// The entry if found, or `None` if not found.
    pub fn get(&self, name: &str) -> Option<&TreeEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Returns an iterator over the entries.
    pub fn iter(&self) -> impl Iterator<Item = &TreeEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree_content(entries: &[(&str, &str, &[u8; 20])]) -> Vec<u8> {
        let mut content = Vec::new();
        for (mode, name, sha1) in entries {
            content.extend_from_slice(mode.as_bytes());
            content.push(b' ');
            content.extend_from_slice(name.as_bytes());
            content.push(0);
            content.extend_from_slice(*sha1);
        }
        content
    }

    fn make_tree(entries: &[(&str, &str, &[u8; 20])]) -> RawObject {
        RawObject {
            object_type: ObjectType::Tree,
            content: make_tree_content(entries),
        }
    }

    fn make_blob_raw() -> RawObject {
        RawObject {
            object_type: ObjectType::Blob,
            content: vec![],
        }
    }

    const SHA1_A: [u8; 20] = [
        0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60, 0x18,
        0x90, 0xaf, 0xd8, 0x07, 0x09,
    ];

    const SHA1_B: [u8; 20] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ];

    // T-001: Parse tree from RawObject
    #[test]
    fn test_parse_tree() {
        let raw = make_tree(&[("100644", "file.txt", &SHA1_A)]);
        let tree = Tree::parse(raw).unwrap();
        assert_eq!(tree.len(), 1);
    }

    // T-002: Parse returns TypeMismatch for non-tree
    #[test]
    fn test_parse_type_mismatch() {
        let raw = make_blob_raw();
        let result = Tree::parse(raw);
        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "tree",
                actual: "blob"
            })
        ));
    }

    // T-003: Parse multiple entries
    #[test]
    fn test_parse_multiple_entries() {
        let raw = make_tree(&[
            ("100644", "file1.txt", &SHA1_A),
            ("100755", "script.sh", &SHA1_B),
            ("40000", "subdir", &SHA1_A),
        ]);
        let tree = Tree::parse(raw).unwrap();

        assert_eq!(tree.len(), 3);
        assert_eq!(tree.entries()[0].name(), "file1.txt");
        assert_eq!(tree.entries()[1].name(), "script.sh");
        assert_eq!(tree.entries()[2].name(), "subdir");
    }

    // T-004: entries() returns all entries
    #[test]
    fn test_entries() {
        let raw = make_tree(&[("100644", "a.txt", &SHA1_A), ("100644", "b.txt", &SHA1_B)]);
        let tree = Tree::parse(raw).unwrap();

        let entries = tree.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "a.txt");
        assert_eq!(entries[1].name(), "b.txt");
    }

    // T-005: get() finds entry by name
    #[test]
    fn test_get() {
        let raw = make_tree(&[
            ("100644", "file1.txt", &SHA1_A),
            ("40000", "subdir", &SHA1_B),
        ]);
        let tree = Tree::parse(raw).unwrap();

        let entry = tree.get("file1.txt").unwrap();
        assert_eq!(entry.name(), "file1.txt");
        assert_eq!(entry.mode(), FileMode::Regular);

        let entry = tree.get("subdir").unwrap();
        assert_eq!(entry.name(), "subdir");
        assert!(entry.is_directory());

        assert!(tree.get("nonexistent").is_none());
    }

    // T-006: iter() iterates over entries
    #[test]
    fn test_iter() {
        let raw = make_tree(&[("100644", "a.txt", &SHA1_A), ("100755", "b.sh", &SHA1_B)]);
        let tree = Tree::parse(raw).unwrap();

        let names: Vec<_> = tree.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["a.txt", "b.sh"]);
    }

    // T-007: FileMode parsing
    #[test]
    fn test_file_mode() {
        assert_eq!(FileMode::from_octal("100644"), Some(FileMode::Regular));
        assert_eq!(FileMode::from_octal("644"), Some(FileMode::Regular));
        assert_eq!(FileMode::from_octal("100755"), Some(FileMode::Executable));
        assert_eq!(FileMode::from_octal("755"), Some(FileMode::Executable));
        assert_eq!(FileMode::from_octal("120000"), Some(FileMode::Symlink));
        assert_eq!(FileMode::from_octal("40000"), Some(FileMode::Directory));
        assert_eq!(FileMode::from_octal("160000"), Some(FileMode::Submodule));
        assert_eq!(FileMode::from_octal("invalid"), None);
    }

    // T-008: FileMode methods
    #[test]
    fn test_file_mode_methods() {
        assert!(FileMode::Regular.is_file());
        assert!(FileMode::Executable.is_file());
        assert!(FileMode::Symlink.is_file());
        assert!(!FileMode::Directory.is_file());
        assert!(!FileMode::Submodule.is_file());

        assert!(!FileMode::Regular.is_directory());
        assert!(FileMode::Directory.is_directory());

        assert!(!FileMode::Regular.is_executable());
        assert!(FileMode::Executable.is_executable());

        assert_eq!(FileMode::Regular.as_octal(), "100644");
        assert_eq!(FileMode::Executable.as_octal(), "100755");
        assert_eq!(FileMode::Directory.as_octal(), "40000");
    }

    // T-009: TreeEntry methods
    #[test]
    fn test_tree_entry_methods() {
        let raw = make_tree(&[("100644", "file.txt", &SHA1_A), ("40000", "dir", &SHA1_B)]);
        let tree = Tree::parse(raw).unwrap();

        let file = tree.get("file.txt").unwrap();
        assert_eq!(file.mode(), FileMode::Regular);
        assert_eq!(file.name(), "file.txt");
        assert_eq!(file.oid(), &Oid::from_bytes(SHA1_A));
        assert!(file.is_file());
        assert!(!file.is_directory());

        let dir = tree.get("dir").unwrap();
        assert!(dir.is_directory());
        assert!(!dir.is_file());
    }

    // Additional: Empty tree
    #[test]
    fn test_empty_tree() {
        let raw = RawObject {
            object_type: ObjectType::Tree,
            content: vec![],
        };
        let tree = Tree::parse(raw).unwrap();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
    }

    // Additional: Parse errors
    #[test]
    fn test_parse_errors() {
        // Missing space
        let raw = RawObject {
            object_type: ObjectType::Tree,
            content: b"100644filename".to_vec(),
        };
        assert!(Tree::parse(raw).is_err());

        // Missing null byte
        let raw = RawObject {
            object_type: ObjectType::Tree,
            content: b"100644 filename".to_vec(),
        };
        assert!(Tree::parse(raw).is_err());

        // Truncated SHA-1
        let mut content = Vec::new();
        content.extend_from_slice(b"100644 file\0");
        content.extend_from_slice(&[0u8; 10]); // Only 10 bytes instead of 20
        let raw = RawObject {
            object_type: ObjectType::Tree,
            content,
        };
        assert!(Tree::parse(raw).is_err());
    }
}
