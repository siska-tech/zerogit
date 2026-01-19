//! Git remote branch representation.

use crate::objects::Oid;

/// Represents a remote-tracking branch.
///
/// A remote branch is a reference in `refs/remotes/<remote>/<branch>` that
/// tracks a branch from a remote repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranch {
    /// The remote name (e.g., "origin").
    remote: String,
    /// The branch name (e.g., "main").
    name: String,
    /// The commit OID this branch points to.
    oid: Oid,
}

impl RemoteBranch {
    /// Creates a new RemoteBranch.
    ///
    /// # Arguments
    ///
    /// * `remote` - The remote name (e.g., "origin").
    /// * `name` - The branch name (e.g., "main").
    /// * `oid` - The commit OID the branch points to.
    pub fn new(remote: impl Into<String>, name: impl Into<String>, oid: Oid) -> Self {
        RemoteBranch {
            remote: remote.into(),
            name: name.into(),
            oid,
        }
    }

    /// Returns the remote name.
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// Returns the branch name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the full name in the format "remote/branch".
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.remote, self.name)
    }

    /// Returns the commit OID this branch points to.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Returns the full reference name (with `refs/remotes/` prefix).
    pub fn reference_name(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.name)
    }

    /// Returns a short representation of the commit OID (7 characters).
    pub fn short_oid(&self) -> String {
        self.oid.short()
    }
}

impl std::fmt::Display for RemoteBranch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.remote, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_OID: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    fn test_oid() -> Oid {
        Oid::from_hex(TEST_OID).unwrap()
    }

    #[test]
    fn test_remote_branch_new() {
        let rb = RemoteBranch::new("origin", "main", test_oid());

        assert_eq!(rb.remote(), "origin");
        assert_eq!(rb.name(), "main");
        assert_eq!(rb.oid().to_hex(), TEST_OID);
    }

    #[test]
    fn test_full_name() {
        let rb = RemoteBranch::new("origin", "main", test_oid());
        assert_eq!(rb.full_name(), "origin/main");

        let rb2 = RemoteBranch::new("upstream", "feature/xyz", test_oid());
        assert_eq!(rb2.full_name(), "upstream/feature/xyz");
    }

    #[test]
    fn test_reference_name() {
        let rb = RemoteBranch::new("origin", "main", test_oid());
        assert_eq!(rb.reference_name(), "refs/remotes/origin/main");

        let rb2 = RemoteBranch::new("origin", "feature/xyz", test_oid());
        assert_eq!(rb2.reference_name(), "refs/remotes/origin/feature/xyz");
    }

    #[test]
    fn test_short_oid() {
        let rb = RemoteBranch::new("origin", "main", test_oid());
        assert_eq!(rb.short_oid().len(), 7);
        assert_eq!(rb.short_oid(), "da39a3e");
    }

    #[test]
    fn test_display() {
        let rb = RemoteBranch::new("origin", "develop", test_oid());
        assert_eq!(format!("{}", rb), "origin/develop");
    }

    #[test]
    fn test_clone_and_eq() {
        let rb1 = RemoteBranch::new("origin", "main", test_oid());
        let rb2 = rb1.clone();
        assert_eq!(rb1, rb2);
    }
}
