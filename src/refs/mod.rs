//! Git references (HEAD, branches, tags).

pub mod branch;
pub mod head;
pub mod remote_branch;
pub mod resolver;
pub mod tag;

pub use branch::{Branch, BranchList};
pub use head::Head;
pub use remote_branch::RemoteBranch;
pub use resolver::{RefStore, RefValue, ResolvedRef};
pub use tag::Tag;
