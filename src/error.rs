//! Error types for zerogit.

use std::fmt;
use std::path::PathBuf;

/// The main error type for zerogit operations.
#[derive(Debug)]
pub enum Error {
    /// An I/O error occurred.
    Io(std::io::Error),

    /// The specified path is not a valid Git repository.
    NotARepository(PathBuf),

    /// The requested object was not found.
    ObjectNotFound(String),

    /// The requested reference was not found.
    RefNotFound(String),

    /// The specified path was not found.
    PathNotFound(PathBuf),

    /// The provided string is not a valid object ID.
    InvalidOid(String),

    /// The provided string is not a valid reference name.
    InvalidRefName(String),

    /// The object is invalid or corrupted.
    InvalidObject {
        /// The object ID.
        oid: String,
        /// The reason for invalidity.
        reason: String,
    },

    /// The index file is invalid.
    InvalidIndex {
        /// The index version.
        version: u32,
        /// The reason for invalidity.
        reason: String,
    },

    /// Type mismatch when expecting a specific object type.
    TypeMismatch {
        /// The expected type.
        expected: &'static str,
        /// The actual type.
        actual: &'static str,
    },

    /// Invalid UTF-8 sequence encountered.
    InvalidUtf8,

    /// Zlib decompression failed.
    DecompressionFailed,

    // Phase 2: Write operations
    /// The reference already exists.
    RefAlreadyExists(String),

    /// Cannot delete the currently checked out branch.
    CannotDeleteCurrentBranch,

    /// Attempted to create an empty commit.
    EmptyCommit,

    /// The working tree has uncommitted changes.
    DirtyWorkingTree,

    /// The requested configuration key was not found.
    ConfigNotFound(String),

    /// A repository already exists at the specified path.
    AlreadyARepository(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::NotARepository(path) => {
                write!(f, "not a git repository: {}", path.display())
            }
            Error::ObjectNotFound(oid) => write!(f, "object not found: {}", oid),
            Error::RefNotFound(name) => write!(f, "reference not found: {}", name),
            Error::PathNotFound(path) => write!(f, "path not found: {}", path.display()),
            Error::InvalidOid(s) => write!(f, "invalid object id: {}", s),
            Error::InvalidRefName(name) => write!(f, "invalid reference name: {}", name),
            Error::InvalidObject { oid, reason } => {
                write!(f, "invalid object {}: {}", oid, reason)
            }
            Error::InvalidIndex { version, reason } => {
                write!(f, "invalid index (version {}): {}", version, reason)
            }
            Error::TypeMismatch { expected, actual } => {
                write!(f, "type mismatch: expected {}, got {}", expected, actual)
            }
            Error::InvalidUtf8 => write!(f, "invalid UTF-8 sequence"),
            Error::DecompressionFailed => write!(f, "zlib decompression failed"),
            Error::RefAlreadyExists(name) => write!(f, "reference already exists: {}", name),
            Error::CannotDeleteCurrentBranch => write!(f, "cannot delete the current branch"),
            Error::EmptyCommit => write!(f, "nothing to commit"),
            Error::DirtyWorkingTree => write!(f, "working tree has uncommitted changes"),
            Error::ConfigNotFound(key) => write!(f, "configuration not found: {}", key),
            Error::AlreadyARepository(path) => {
                write!(f, "repository already exists: {}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Result type alias for zerogit operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as StdError;

    // E-001: Error::Io can be created from std::io::Error
    #[test]
    fn test_error_from_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let error: Error = io_error.into();
        assert!(matches!(error, Error::Io(_)));
        assert!(error.to_string().contains("I/O error"));
    }

    // E-002: Error implements Display with human-readable messages
    #[test]
    fn test_error_display() {
        let error = Error::NotARepository(PathBuf::from("/tmp/not-a-repo"));
        assert_eq!(error.to_string(), "not a git repository: /tmp/not-a-repo");

        let error = Error::ObjectNotFound("abc123".to_string());
        assert_eq!(error.to_string(), "object not found: abc123");

        let error = Error::InvalidOid("not-a-sha".to_string());
        assert_eq!(error.to_string(), "invalid object id: not-a-sha");
    }

    // E-003: Error implements std::error::Error
    #[test]
    fn test_error_trait() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let error: Error = io_error.into();

        // source() returns the underlying io::Error
        let source = StdError::source(&error);
        assert!(source.is_some());

        // Non-Io errors return None
        let error = Error::InvalidUtf8;
        assert!(StdError::source(&error).is_none());
    }

    // E-004: All error variants can be created and displayed
    #[test]
    fn test_all_error_variants() {
        let errors: Vec<Error> = vec![
            Error::Io(std::io::Error::new(std::io::ErrorKind::Other, "test")),
            Error::NotARepository(PathBuf::from("/test")),
            Error::ObjectNotFound("abc".to_string()),
            Error::RefNotFound("refs/heads/main".to_string()),
            Error::PathNotFound(PathBuf::from("/test/path")),
            Error::InvalidOid("xyz".to_string()),
            Error::InvalidRefName("bad ref".to_string()),
            Error::InvalidObject {
                oid: "abc".to_string(),
                reason: "corrupted".to_string(),
            },
            Error::InvalidIndex {
                version: 2,
                reason: "bad header".to_string(),
            },
            Error::TypeMismatch {
                expected: "commit",
                actual: "blob",
            },
            Error::InvalidUtf8,
            Error::DecompressionFailed,
            Error::RefAlreadyExists("refs/heads/main".to_string()),
            Error::CannotDeleteCurrentBranch,
            Error::EmptyCommit,
            Error::DirtyWorkingTree,
            Error::ConfigNotFound("user.name".to_string()),
            Error::AlreadyARepository(PathBuf::from("/test/repo")),
        ];

        // All variants should implement Display without panicking
        for error in &errors {
            let _ = error.to_string();
            let _ = format!("{:?}", error);
        }
    }
}
