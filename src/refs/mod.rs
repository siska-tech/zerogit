//! Git references (HEAD, branches, tags).

pub mod branch;
mod branch_ops;
pub mod head;
mod packed;
pub(crate) mod reflog;
pub mod remote_branch;
pub mod resolver;
pub mod tag;
mod tag_ops;
mod update;

pub use branch::{Branch, BranchList};
pub use head::Head;
pub use reflog::{ReflogEntry, ReflogExpiry};
pub use remote_branch::RemoteBranch;
pub use resolver::{RefStore, RefValue, ResolvedRef};
pub use tag::Tag;
