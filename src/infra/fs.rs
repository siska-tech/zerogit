//! Filesystem utilities for file reading, writing, and directory traversal.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Reads the entire contents of a file as bytes.
///
/// # Arguments
///
/// * `path` - The path to the file to read.
///
/// # Returns
///
/// The file contents as a byte vector, or an error if the file cannot be read.
pub fn read_file<P: AsRef<Path>>(path: P) -> Result<Vec<u8>> {
    fs::read(path.as_ref()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::PathNotFound(path.as_ref().to_path_buf())
        } else {
            Error::Io(e)
        }
    })
}

/// Writes data to a file atomically.
///
/// This function writes to a temporary file first, then renames it to the
/// target path. This ensures that the file is either fully written or not
/// modified at all, preventing partial writes.
///
/// # Arguments
///
/// * `path` - The path to write to.
/// * `data` - The data to write.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the write fails.
#[allow(dead_code)]
pub fn write_file_atomic<P: AsRef<Path>>(path: P, data: &[u8]) -> Result<()> {
    let path = path.as_ref();

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    // Create a temporary file in the same directory
    let temp_path = {
        let mut temp = path.to_path_buf();
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "temp".to_string());
        temp.set_file_name(format!(".{}.tmp", file_name));
        temp
    };

    // Write to temporary file
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }

    // Rename temporary file to target (atomic on most filesystems)
    fs::rename(&temp_path, path)?;

    Ok(())
}

/// Lists all files in the working tree, excluding `.git` directory.
///
/// Returns paths relative to the given root directory.
///
/// # Arguments
///
/// * `root` - The root directory to traverse.
///
/// # Returns
///
/// A vector of relative paths to all files in the working tree.
pub fn list_working_tree<P: AsRef<Path>>(root: P) -> Result<Vec<PathBuf>> {
    let root = root.as_ref();
    let mut files = Vec::new();

    list_working_tree_recursive(root, root, &mut files)?;

    // Sort for consistent ordering
    files.sort();

    Ok(files)
}

fn list_working_tree_recursive(
    root: &Path,
    current: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::PathNotFound(current.to_path_buf())
        } else {
            Error::Io(e)
        }
    })?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();

        // Skip .git directory
        if file_name == ".git" {
            continue;
        }

        // Skip hidden files/directories (starting with .)
        // except for specific files like .gitignore
        let name_str = file_name.to_string_lossy();
        if name_str.starts_with('.') && name_str != ".gitignore" && name_str != ".gitattributes" {
            continue;
        }

        let file_type = entry.file_type()?;

        if file_type.is_file() {
            // Get relative path
            let relative = path
                .strip_prefix(root)
                .map_err(|_| Error::PathNotFound(path.clone()))?;
            files.push(relative.to_path_buf());
        } else if file_type.is_dir() {
            list_working_tree_recursive(root, &path, files)?;
        }
        // Skip symlinks and other special files
    }

    Ok(())
}

/// Validates that a path does not escape its root directory (path traversal prevention).
///
/// # Arguments
///
/// * `root` - The root directory that the path should be contained within.
/// * `path` - The path to validate.
///
/// # Returns
///
/// The canonicalized path if valid, or an error if the path escapes the root.
#[allow(dead_code)]
pub fn safe_join<P: AsRef<Path>, Q: AsRef<Path>>(root: P, path: Q) -> Result<PathBuf> {
    let root = root.as_ref();
    let path = path.as_ref();

    // Check for obvious traversal attempts in the path components
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(Error::PathNotFound(path.to_path_buf()));
            }
            std::path::Component::Normal(s) => {
                let s_str = s.to_string_lossy();
                // Block paths containing null bytes or other dangerous characters
                if s_str.contains('\0') {
                    return Err(Error::PathNotFound(path.to_path_buf()));
                }
            }
            _ => {}
        }
    }

    let joined = root.join(path);

    // For existing paths, verify canonicalization
    if joined.exists() {
        let canonical_root = root
            .canonicalize()
            .map_err(|_| Error::PathNotFound(root.to_path_buf()))?;
        let canonical_joined = joined
            .canonicalize()
            .map_err(|_| Error::PathNotFound(joined.clone()))?;

        if !canonical_joined.starts_with(&canonical_root) {
            return Err(Error::PathNotFound(path.to_path_buf()));
        }

        Ok(canonical_joined)
    } else {
        // For non-existing paths, just do component-level validation (already done above)
        Ok(joined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // FS-001: Read file successfully
    #[test]
    fn test_read_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        fs::write(&file_path, b"Hello, World!").unwrap();

        let contents = read_file(&file_path).unwrap();
        assert_eq!(contents, b"Hello, World!");
    }

    // FS-002: Read file not found
    #[test]
    fn test_read_file_not_found() {
        let result = read_file("/nonexistent/path/file.txt");
        assert!(matches!(result, Err(Error::PathNotFound(_))));
    }

    // FS-003: Write file atomic success
    #[test]
    fn test_write_file_atomic_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("output.txt");

        write_file_atomic(&file_path, b"Test data").unwrap();

        let contents = fs::read(&file_path).unwrap();
        assert_eq!(contents, b"Test data");
    }

    // FS-004: Write file atomic creates parent directories
    #[test]
    fn test_write_file_atomic_creates_parents() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("nested/dir/file.txt");

        write_file_atomic(&file_path, b"Nested data").unwrap();

        let contents = fs::read(&file_path).unwrap();
        assert_eq!(contents, b"Nested data");
    }

    // FS-005: Write file atomic overwrites existing file
    #[test]
    fn test_write_file_atomic_overwrite() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("existing.txt");

        fs::write(&file_path, b"Old content").unwrap();
        write_file_atomic(&file_path, b"New content").unwrap();

        let contents = fs::read(&file_path).unwrap();
        assert_eq!(contents, b"New content");
    }

    // FS-006: List working tree excludes .git
    #[test]
    fn test_list_working_tree_excludes_git() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create files and directories
        fs::write(root.join("file1.txt"), b"content").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), b"git config").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();

        let files = list_working_tree(root).unwrap();

        // Should contain file1.txt and src/main.rs but not .git/config
        assert!(files.contains(&PathBuf::from("file1.txt")));
        assert!(
            files.contains(&PathBuf::from("src/main.rs"))
                || files.contains(&PathBuf::from("src\\main.rs"))
        );
        assert!(!files.iter().any(|p| p.to_string_lossy().contains(".git")));
    }

    // FS-007: List working tree includes .gitignore
    #[test]
    fn test_list_working_tree_includes_gitignore() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join(".gitignore"), b"*.log").unwrap();
        fs::write(root.join("file.txt"), b"content").unwrap();

        let files = list_working_tree(root).unwrap();

        assert!(files.contains(&PathBuf::from(".gitignore")));
        assert!(files.contains(&PathBuf::from("file.txt")));
    }

    // FS-008: List working tree empty directory
    #[test]
    fn test_list_working_tree_empty() {
        let temp_dir = TempDir::new().unwrap();

        let files = list_working_tree(temp_dir.path()).unwrap();
        assert!(files.is_empty());
    }

    // FS-009: Safe join prevents path traversal
    #[test]
    fn test_safe_join_prevents_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Attempting to traverse up should fail
        let result = safe_join(root, "../etc/passwd");
        assert!(matches!(result, Err(Error::PathNotFound(_))));

        // Attempting to traverse up in the middle should fail
        let result = safe_join(root, "subdir/../../../etc/passwd");
        assert!(matches!(result, Err(Error::PathNotFound(_))));
    }

    // FS-010: Safe join allows valid paths
    #[test]
    fn test_safe_join_allows_valid_paths() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file
        fs::write(root.join("test.txt"), b"content").unwrap();

        // Valid path should succeed
        let result = safe_join(root, "test.txt");
        assert!(result.is_ok());
    }

    // FS-011: Safe join with nested paths
    #[test]
    fn test_safe_join_nested_paths() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create nested structure
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::write(root.join("a/b/c/file.txt"), b"content").unwrap();

        let result = safe_join(root, "a/b/c/file.txt");
        assert!(result.is_ok());
    }

    // FS-012: List working tree returns sorted results
    #[test]
    fn test_list_working_tree_sorted() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        fs::write(root.join("z.txt"), b"").unwrap();
        fs::write(root.join("a.txt"), b"").unwrap();
        fs::write(root.join("m.txt"), b"").unwrap();

        let files = list_working_tree(root).unwrap();

        assert_eq!(files.len(), 3);
        assert_eq!(files[0], PathBuf::from("a.txt"));
        assert_eq!(files[1], PathBuf::from("m.txt"));
        assert_eq!(files[2], PathBuf::from("z.txt"));
    }
}
