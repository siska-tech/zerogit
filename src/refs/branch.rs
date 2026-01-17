//! Git branch representation.

use crate::objects::Oid;

/// Represents a Git branch.
///
/// A branch is a named reference that points to a commit.
/// The branch name does not include the `refs/heads/` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// The branch name (without `refs/heads/` prefix).
    name: String,
    /// The commit OID the branch points to.
    oid: Oid,
    /// Whether this is the current branch (HEAD points to it).
    is_current: bool,
}

impl Branch {
    /// Creates a new Branch.
    ///
    /// # Arguments
    ///
    /// * `name` - The branch name (without `refs/heads/` prefix).
    /// * `oid` - The commit OID the branch points to.
    pub fn new(name: impl Into<String>, oid: Oid) -> Self {
        Branch {
            name: name.into(),
            oid,
            is_current: false,
        }
    }

    /// Creates a new Branch marked as current.
    ///
    /// # Arguments
    ///
    /// * `name` - The branch name (without `refs/heads/` prefix).
    /// * `oid` - The commit OID the branch points to.
    pub fn current(name: impl Into<String>, oid: Oid) -> Self {
        Branch {
            name: name.into(),
            oid,
            is_current: true,
        }
    }

    /// Returns the branch name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the commit OID the branch points to.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Returns `true` if this is the current branch.
    pub fn is_current(&self) -> bool {
        self.is_current
    }

    /// Returns the full reference name (with `refs/heads/` prefix).
    pub fn reference_name(&self) -> String {
        format!("refs/heads/{}", self.name)
    }

    /// Returns a short representation of the commit OID (7 characters).
    pub fn short_oid(&self) -> String {
        self.oid.short()
    }

    /// Sets whether this branch is the current branch.
    pub fn set_current(&mut self, is_current: bool) {
        self.is_current = is_current;
    }
}

impl std::fmt::Display for Branch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_current {
            write!(f, "* {}", self.name)
        } else {
            write!(f, "  {}", self.name)
        }
    }
}

/// A collection of branches with optional current branch tracking.
#[derive(Debug, Clone, Default)]
pub struct BranchList {
    /// The branches in this list.
    branches: Vec<Branch>,
}

impl BranchList {
    /// Creates a new empty BranchList.
    pub fn new() -> Self {
        BranchList {
            branches: Vec::new(),
        }
    }

    /// Creates a BranchList from a vector of branches.
    pub fn from_branches(branches: Vec<Branch>) -> Self {
        BranchList { branches }
    }

    /// Adds a branch to the list.
    pub fn push(&mut self, branch: Branch) {
        self.branches.push(branch);
    }

    /// Returns the number of branches.
    pub fn len(&self) -> usize {
        self.branches.len()
    }

    /// Returns `true` if there are no branches.
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty()
    }

    /// Returns the current branch, if any.
    pub fn current(&self) -> Option<&Branch> {
        self.branches.iter().find(|b| b.is_current)
    }

    /// Returns an iterator over all branches.
    pub fn iter(&self) -> impl Iterator<Item = &Branch> {
        self.branches.iter()
    }

    /// Returns a slice of all branches.
    pub fn as_slice(&self) -> &[Branch] {
        &self.branches
    }

    /// Finds a branch by name.
    pub fn find(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|b| b.name == name)
    }

    /// Sorts branches by name.
    pub fn sort_by_name(&mut self) {
        self.branches.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

impl IntoIterator for BranchList {
    type Item = Branch;
    type IntoIter = std::vec::IntoIter<Branch>;

    fn into_iter(self) -> Self::IntoIter {
        self.branches.into_iter()
    }
}

impl<'a> IntoIterator for &'a BranchList {
    type Item = &'a Branch;
    type IntoIter = std::slice::Iter<'a, Branch>;

    fn into_iter(self) -> Self::IntoIter {
        self.branches.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_OID: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    const TEST_OID2: &str = "0123456789abcdef0123456789abcdef01234567";

    fn test_oid() -> Oid {
        Oid::from_hex(TEST_OID).unwrap()
    }

    fn test_oid2() -> Oid {
        Oid::from_hex(TEST_OID2).unwrap()
    }

    // BR-001: Branch::new creates non-current branch
    #[test]
    fn test_branch_new() {
        let branch = Branch::new("main", test_oid());

        assert_eq!(branch.name(), "main");
        assert_eq!(branch.oid().to_hex(), TEST_OID);
        assert!(!branch.is_current());
    }

    // BR-002: Branch::current creates current branch
    #[test]
    fn test_branch_current() {
        let branch = Branch::current("main", test_oid());

        assert_eq!(branch.name(), "main");
        assert!(branch.is_current());
    }

    // BR-003: reference_name returns full ref path
    #[test]
    fn test_reference_name() {
        let branch = Branch::new("main", test_oid());
        assert_eq!(branch.reference_name(), "refs/heads/main");

        let nested = Branch::new("feature/my-feature", test_oid());
        assert_eq!(nested.reference_name(), "refs/heads/feature/my-feature");
    }

    // BR-004: short_oid returns 7 character OID
    #[test]
    fn test_short_oid() {
        let branch = Branch::new("main", test_oid());
        assert_eq!(branch.short_oid().len(), 7);
        assert_eq!(branch.short_oid(), "da39a3e");
    }

    // BR-005: Display shows branch with marker
    #[test]
    fn test_branch_display() {
        let current = Branch::current("main", test_oid());
        let other = Branch::new("develop", test_oid());

        assert_eq!(format!("{}", current), "* main");
        assert_eq!(format!("{}", other), "  develop");
    }

    // BR-006: BranchList operations
    #[test]
    fn test_branch_list() {
        let mut list = BranchList::new();

        list.push(Branch::new("develop", test_oid()));
        list.push(Branch::current("main", test_oid2()));
        list.push(Branch::new("feature", test_oid()));

        assert_eq!(list.len(), 3);
        assert!(!list.is_empty());

        let current = list.current().unwrap();
        assert_eq!(current.name(), "main");

        let found = list.find("develop").unwrap();
        assert_eq!(found.name(), "develop");

        assert!(list.find("nonexistent").is_none());
    }

    // BR-007: BranchList sorting
    #[test]
    fn test_branch_list_sort() {
        let mut list = BranchList::new();

        list.push(Branch::new("zebra", test_oid()));
        list.push(Branch::new("alpha", test_oid()));
        list.push(Branch::new("main", test_oid()));

        list.sort_by_name();

        let names: Vec<_> = list.iter().map(|b| b.name()).collect();
        assert_eq!(names, vec!["alpha", "main", "zebra"]);
    }

    // BR-008: BranchList iteration
    #[test]
    fn test_branch_list_iteration() {
        let mut list = BranchList::new();
        list.push(Branch::new("a", test_oid()));
        list.push(Branch::new("b", test_oid()));

        let names: Vec<_> = list.iter().map(|b| b.name()).collect();
        assert_eq!(names, vec!["a", "b"]);

        // IntoIterator for &BranchList
        let names: Vec<_> = (&list).into_iter().map(|b| b.name()).collect();
        assert_eq!(names, vec!["a", "b"]);

        // IntoIterator for BranchList (consuming)
        let names: Vec<_> = list.into_iter().map(|b| b.name().to_string()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    // BR-009: set_current modifies branch
    #[test]
    fn test_set_current() {
        let mut branch = Branch::new("main", test_oid());
        assert!(!branch.is_current());

        branch.set_current(true);
        assert!(branch.is_current());

        branch.set_current(false);
        assert!(!branch.is_current());
    }

    // Additional: Branch implements Clone and PartialEq
    #[test]
    fn test_branch_traits() {
        let branch1 = Branch::new("main", test_oid());
        let branch2 = branch1.clone();

        assert_eq!(branch1, branch2);
    }
}
