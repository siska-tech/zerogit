//! Filesystem utilities for file reading, writing, and directory traversal.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Removes a file, read-only or not, and returns whether it existed.
///
/// Git writes objects and packs read-only. On Windows, older Rust (before
/// 1.75 or so) cannot remove a read-only file, so the attribute is cleared
/// and the removal retried.
pub(crate) fn remove_file(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let mut permissions = match fs::metadata(path) {
                Ok(metadata) => metadata.permissions(),
                Err(_) => return Err(e.into()),
            };
            if !permissions.readonly() {
                return Err(e.into());
            }
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
            fs::remove_file(path)?;
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Writes data to a file atomically.
///
/// This function writes to a temporary file first, then renames it to the
/// target path. This ensures that the file is either fully written or not
/// modified at all, preventing partial writes.
///
/// No lock is taken, so this suits files that only ever get one content
/// (objects, packs) or that Git does not lock (work tree files). Files Git
/// locks, such as the index and references, are written through a lock
/// file instead.
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

    // Create a temporary file in the same directory. The name is unique to
    // this process and call, so concurrent writers of the same path (two
    // processes storing the same object, say) never share a temporary file.
    let temp_path = {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let mut temp = path.to_path_buf();
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "temp".to_string());
        temp.set_file_name(format!(
            ".{}.tmp-{}-{}",
            file_name,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        temp
    };

    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        // Rename temporary file to target (atomic on most filesystems)
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
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
}
