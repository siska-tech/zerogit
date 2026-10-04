//! # zerogit
//!
//! A lightweight, pure Rust Git client library.
//!
//! This crate provides Git repository operations without external dependencies
//! like libgit2 or the git command-line tool.
//!
//! ## Features
//!
//! - Read Git repositories (loose objects only, no pack files yet)
//! - Navigate commits, trees, and blobs
//! - Read branches and HEAD
//! - Query working tree status
//! - Read index (staging area)
//!
//! ## Quick Start
//!
//! ```no_run
//! use zerogit::{Repository, Result};
//!
//! fn main() -> Result<()> {
//!     // Open a repository
//!     let repo = Repository::open("path/to/repo")?;
//!
//!     // Get HEAD
//!     let head = repo.head()?;
//!     println!("On branch: {:?}", head.branch_name());
//!
//!     // Read a commit
//!     let commit = repo.commit(&head.oid().to_hex())?;
//!     println!("Latest commit: {}", commit.summary());
//!
//!     // Check status
//!     for entry in repo.status()? {
//!         println!("{:?}: {}", entry.status(), entry.path().display());
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Module Overview
//!
//! - [`error`] - Error types and Result alias
//! - [`repository`] - Main `Repository` type for accessing Git data
//! - [`objects`] - Git object types (blob, tree, commit)
//! - [`refs`] - References (HEAD, branches)
//! - [`index`] - Index (staging area) operations
//! - [`status`] - Working tree status

pub(crate) mod attributes;
pub mod commit;
pub mod config;
pub mod diff;
pub(crate) mod eol;
pub mod error;
pub(crate) mod ignore;
pub mod index;
pub mod log;
pub mod merge;
pub mod objects;
pub mod path_ops;
pub(crate) mod pathspec;
pub mod rebase;
pub mod refs;
pub mod remote;
pub mod repository;
pub mod reset;
pub mod revision;
pub mod stash;
pub mod status;
pub mod transfer;
pub(crate) mod worktree;

// Internal modules (not part of public API)
pub(crate) mod infra;

// Re-export primary types for convenient access
pub use commit::CommitOptions;
pub use config::{Config, ConfigLevel};
pub use error::{Error, Result};
pub use path_ops::{RemoveOptions, RestoreOptions};
pub use repository::{CheckoutOptions, Repository};
pub use reset::ResetMode;

// Re-export object types
pub use objects::{Blob, Commit, FileMode, Object, Oid, Signature, Tree, TreeEntry};

// Re-export reference types
pub use refs::{Branch, Head, ReflogEntry, RemoteBranch, Tag};

// Re-export status types
pub use status::{
    ChangeState, ConflictKind, DetailedStatus, DetailedStatusEntry, FileStatus, StatusEntry,
};

// Re-export index types
pub use index::{Index, IndexEntry};

// Re-export log types
pub use log::LogOptions;

// Re-export merge types
pub use merge::{FastForward, MergeOptions, MergeOutcome};

// Re-export remote types
pub use remote::{Refspec, Remote};

// Re-export transfer types
pub use transfer::StoredPack;

// Re-export rebase types
pub use rebase::RebaseOutcome;

// Re-export stash types
pub use stash::{StashApplyOutcome, StashEntry, StashOptions};

// Re-export diff types
pub use diff::{
    BlobDiff, BlobDiffContent, DiffDelta, DiffHunk, DiffLine, DiffOptions, DiffStats, DiffStatus,
    LineEnding, LineKind, NonTextReason, RenameDetection, RenameLimit, RenameOptions, SkipReason,
    TreeDiff,
};
