//! Git references (HEAD, branches, tags).

pub mod branch;
pub mod head;
pub mod resolver;

pub use branch::{Branch, BranchList};
pub use head::Head;
pub use resolver::{RefStore, RefValue, ResolvedRef};
