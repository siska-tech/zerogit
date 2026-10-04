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

    /// A packed-refs record is malformed.
    InvalidPackedRefs {
        /// The one-based line number of the malformed record.
        line: usize,
        /// The reason the record was rejected.
        reason: String,
    },

    /// Deleting a packed reference is not supported yet.
    PackedRefDeletionUnsupported(String),

    /// A pack index file is malformed or its checksum does not match.
    InvalidPackIndex {
        /// The reason the index was rejected.
        reason: String,
    },

    /// The pack index version is recognized but not supported.
    UnsupportedPackIndexVersion(u32),

    /// A pack file is malformed, inconsistent with its index, or a delta cannot be applied.
    InvalidPack {
        /// The reason the pack was rejected.
        reason: String,
    },

    /// The pack file version is not supported.
    UnsupportedPackVersion(u32),

    /// The index uses a feature this library cannot read or rewrite safely
    /// (for example split index or sparse index).
    UnsupportedIndex {
        /// The index version.
        version: u32,
        /// The unsupported feature.
        reason: String,
    },

    /// The repository uses a format this library does not support (for example SHA-256 objects).
    UnsupportedRepositoryFormat(String),

    /// Reading a packed object would exceed a configured size or delta depth limit.
    PackLimitExceeded {
        /// The limit that was exceeded.
        reason: String,
    },

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

    /// The index has unresolved merge conflicts (entries at stages 1-3), so
    /// the operation cannot proceed until the listed paths are resolved.
    UnmergedPaths(Vec<PathBuf>),

    /// The path is ignored by `.gitignore`, `.git/info/exclude` or
    /// `core.excludesFile`, and is not tracked.
    IgnoredPath(PathBuf),

    /// The path has an attribute whose conversion is not supported
    /// (`filter` with a configured driver, `ident`, `working-tree-encoding`),
    /// so its content cannot be converted the way Git would.
    UnsupportedAttribute {
        /// The path.
        path: PathBuf,
        /// The attribute name.
        attribute: String,
    },

    /// Converting the line endings of the file is irreversible and
    /// `core.safecrlf` is `true`.
    IrreversibleLineEndings(PathBuf),

    /// A merge is already in progress (`MERGE_HEAD` exists); conclude it
    /// with a commit or abort it first.
    MergeInProgress,

    /// No merge is in progress (there is no `MERGE_HEAD`).
    NoMergeInProgress,

    /// A fast-forward was required but the histories have diverged.
    NotFastForward,

    /// The operation would overwrite local changes (staged or unstaged) or
    /// untracked files at these paths; nothing was changed.
    LocalChangesWouldBeOverwritten(Vec<PathBuf>),

    /// The merge needs handling that is not supported (for example a path
    /// that is a file on one side and a directory on the other); nothing
    /// was changed.
    UnsupportedMerge(String),

    /// A rebase is already in progress (`.git/rebase-merge/` exists);
    /// continue, skip or abort it first.
    RebaseInProgress,

    /// No rebase is in progress.
    NoRebaseInProgress,

    /// The rebase state uses something not supported (for example a todo
    /// list with commands other than `pick`, from an interactive rebase).
    UnsupportedRebase(String),

    /// No remote with this name is configured.
    RemoteNotFound(String),

    /// A remote with this name is already configured.
    RemoteAlreadyExists(String),

    /// A reference did not have the value an update expected.
    StaleReference(String),

    /// The file is locked: its lock file (`<path>.lock`) exists because
    /// another process, such as Git, is changing it. Nothing was changed;
    /// retry once that process has finished. If no other process is
    /// running, the lock was left behind by one that crashed and can be
    /// removed.
    Locked(PathBuf),
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
            Error::InvalidPackedRefs { line, reason } => {
                write!(f, "invalid packed-refs at line {}: {}", line, reason)
            }
            Error::PackedRefDeletionUnsupported(name) => {
                write!(f, "deleting packed reference is not supported: {}", name)
            }
            Error::InvalidPackIndex { reason } => write!(f, "invalid pack index: {}", reason),
            Error::UnsupportedPackIndexVersion(version) => {
                write!(f, "unsupported pack index version: {}", version)
            }
            Error::InvalidPack { reason } => write!(f, "invalid pack: {}", reason),
            Error::UnsupportedPackVersion(version) => {
                write!(f, "unsupported pack version: {}", version)
            }
            Error::UnsupportedIndex { version, reason } => {
                write!(f, "unsupported index (version {}): {}", version, reason)
            }
            Error::UnsupportedRepositoryFormat(what) => {
                write!(f, "unsupported repository format: {}", what)
            }
            Error::PackLimitExceeded { reason } => write!(f, "pack limit exceeded: {}", reason),
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
            Error::UnmergedPaths(paths) => {
                write!(f, "index has unmerged paths:")?;
                for path in paths {
                    write!(f, " {}", path.display())?;
                }
                Ok(())
            }
            Error::IgnoredPath(path) => {
                write!(f, "path is ignored: {}", path.display())
            }
            Error::UnsupportedAttribute { path, attribute } => write!(
                f,
                "unsupported attribute '{}' for {}",
                attribute,
                path.display()
            ),
            Error::MergeInProgress => write!(f, "a merge is in progress"),
            Error::NoMergeInProgress => write!(f, "no merge is in progress"),
            Error::NotFastForward => write!(f, "not possible to fast-forward"),
            Error::LocalChangesWouldBeOverwritten(paths) => {
                write!(f, "local changes would be overwritten:")?;
                for path in paths {
                    write!(f, " {}", path.display())?;
                }
                Ok(())
            }
            Error::UnsupportedMerge(reason) => write!(f, "unsupported merge: {}", reason),
            Error::RebaseInProgress => write!(f, "a rebase is in progress"),
            Error::NoRebaseInProgress => write!(f, "no rebase is in progress"),
            Error::UnsupportedRebase(reason) => write!(f, "unsupported rebase: {}", reason),
            Error::RemoteNotFound(name) => write!(f, "remote not found: {}", name),
            Error::RemoteAlreadyExists(name) => write!(f, "remote already exists: {}", name),
            Error::StaleReference(name) => write!(f, "reference changed concurrently: {}", name),
            Error::Locked(path) => write!(
                f,
                "unable to create '{}': File exists; another process may be using the repository",
                path.display()
            ),
            Error::IrreversibleLineEndings(path) => write!(
                f,
                "line ending conversion would not round-trip: {}",
                path.display()
            ),
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
            Error::UnmergedPaths(vec![PathBuf::from("file.txt")]),
            Error::IgnoredPath(PathBuf::from("debug.log")),
            Error::UnsupportedAttribute {
                path: PathBuf::from("big.bin"),
                attribute: "filter".to_string(),
            },
            Error::IrreversibleLineEndings(PathBuf::from("mixed.txt")),
            Error::MergeInProgress,
            Error::NoMergeInProgress,
            Error::NotFastForward,
            Error::LocalChangesWouldBeOverwritten(vec![PathBuf::from("a.txt")]),
            Error::UnsupportedMerge("reason".to_string()),
            Error::RebaseInProgress,
            Error::NoRebaseInProgress,
            Error::UnsupportedRebase("reason".to_string()),
            Error::RemoteNotFound("origin".to_string()),
            Error::RemoteAlreadyExists("origin".to_string()),
            Error::StaleReference("refs/heads/main".to_string()),
            Error::Locked(PathBuf::from(".git/index.lock")),
        ];

        // All variants should implement Display without panicking
        for error in &errors {
            let _ = error.to_string();
            let _ = format!("{:?}", error);
        }
    }
}
