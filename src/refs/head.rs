//! HEAD reference representation.

use crate::objects::Oid;

/// Represents the current HEAD state of a Git repository.
///
/// HEAD can either point to a branch (normal state) or directly
/// to a commit (detached HEAD state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// HEAD points to a branch (e.g., `refs/heads/main`).
    Branch {
        /// The branch name (without `refs/heads/` prefix).
        name: String,
        /// The commit OID the branch points to.
        oid: Oid,
    },
    /// HEAD points directly to a commit (detached state).
    Detached {
        /// The commit OID that HEAD points to.
        oid: Oid,
    },
}

impl Head {
    /// Creates a new Head pointing to a branch.
    ///
    /// # Arguments
    ///
    /// * `name` - The branch name (without `refs/heads/` prefix).
    /// * `oid` - The commit OID the branch points to.
    pub fn branch(name: impl Into<String>, oid: Oid) -> Self {
        Head::Branch {
            name: name.into(),
            oid,
        }
    }

    /// Creates a new detached Head.
    ///
    /// # Arguments
    ///
    /// * `oid` - The commit OID that HEAD points to.
    pub fn detached(oid: Oid) -> Self {
        Head::Detached { oid }
    }

    /// Returns the commit OID that HEAD points to.
    ///
    /// This works for both branch and detached states.
    pub fn oid(&self) -> &Oid {
        match self {
            Head::Branch { oid, .. } => oid,
            Head::Detached { oid } => oid,
        }
    }

    /// Returns the branch name if HEAD points to a branch.
    ///
    /// Returns `None` if HEAD is in detached state.
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Head::Branch { name, .. } => Some(name),
            Head::Detached { .. } => None,
        }
    }

    /// Returns `true` if HEAD is in detached state.
    pub fn is_detached(&self) -> bool {
        matches!(self, Head::Detached { .. })
    }

    /// Returns `true` if HEAD points to a branch.
    pub fn is_branch(&self) -> bool {
        matches!(self, Head::Branch { .. })
    }

    /// Returns the full reference name.
    ///
    /// For branches, returns `refs/heads/<name>`.
    /// For detached HEAD, returns the commit OID as hex.
    pub fn reference_name(&self) -> String {
        match self {
            Head::Branch { name, .. } => format!("refs/heads/{}", name),
            Head::Detached { oid } => oid.to_hex(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_OID: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    fn test_oid() -> Oid {
        Oid::from_hex(TEST_OID).unwrap()
    }

    // H-001: Head::branch creates branch state
    #[test]
    fn test_head_branch() {
        let head = Head::branch("main", test_oid());

        assert!(head.is_branch());
        assert!(!head.is_detached());
        assert_eq!(head.branch_name(), Some("main"));
        assert_eq!(head.oid().to_hex(), TEST_OID);
    }

    // H-002: Head::detached creates detached state
    #[test]
    fn test_head_detached() {
        let head = Head::detached(test_oid());

        assert!(head.is_detached());
        assert!(!head.is_branch());
        assert_eq!(head.branch_name(), None);
        assert_eq!(head.oid().to_hex(), TEST_OID);
    }

    // H-003: oid() returns correct OID for both states
    #[test]
    fn test_head_oid() {
        let branch_head = Head::branch("main", test_oid());
        let detached_head = Head::detached(test_oid());

        assert_eq!(branch_head.oid(), detached_head.oid());
    }

    // H-004: reference_name returns correct format
    #[test]
    fn test_reference_name() {
        let branch_head = Head::branch("main", test_oid());
        let detached_head = Head::detached(test_oid());

        assert_eq!(branch_head.reference_name(), "refs/heads/main");
        assert_eq!(detached_head.reference_name(), TEST_OID);
    }

    // H-005: Head with nested branch name
    #[test]
    fn test_nested_branch_name() {
        let head = Head::branch("feature/my-feature", test_oid());

        assert_eq!(head.branch_name(), Some("feature/my-feature"));
        assert_eq!(head.reference_name(), "refs/heads/feature/my-feature");
    }

    // Additional: Head implements Clone and PartialEq
    #[test]
    fn test_head_traits() {
        let head1 = Head::branch("main", test_oid());
        let head2 = head1.clone();

        assert_eq!(head1, head2);
    }
}
