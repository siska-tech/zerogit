//! Git reference resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::packed;
use crate::error::{Error, Result};
use crate::objects::Oid;

/// The result of resolving a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefValue {
    /// A direct reference to an object ID.
    Direct(Oid),
    /// A symbolic reference to another ref (e.g., HEAD -> refs/heads/main).
    Symbolic(String),
}

/// A resolved reference with its name and target.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    /// The name of the reference (e.g., "refs/heads/main").
    pub name: String,
    /// The object ID this reference points to.
    pub oid: Oid,
}

/// A store for reading and resolving Git references.
///
/// References are stored in the `.git` directory as either:
/// - Loose refs: Individual files under `.git/refs/`
/// - Packed refs: A single file `.git/packed-refs`, shadowed by loose refs.
#[derive(Debug)]
pub struct RefStore {
    /// Path to the `.git` directory.
    git_dir: PathBuf,
}

impl RefStore {
    /// Creates a new RefStore for the given `.git` directory.
    ///
    /// # Arguments
    ///
    /// * `git_dir` - Path to the `.git` directory.
    pub fn new<P: AsRef<Path>>(git_dir: P) -> Self {
        RefStore {
            git_dir: git_dir.as_ref().to_path_buf(),
        }
    }

    /// Reads a loose reference, falling back to packed-refs only when absent.
    ///
    /// Malformed or unreadable loose references never fall back to packed refs.
    ///
    /// # Arguments
    ///
    /// * `name` - The reference name (e.g., "HEAD" or "refs/heads/main").
    ///
    /// # Returns
    ///
    /// The parsed reference value, or an error if the ref doesn't exist.
    pub fn read_ref_file(&self, name: &str) -> Result<RefValue> {
        match self.read_loose_ref(name) {
            Err(Error::RefNotFound(_)) if name.starts_with("refs/") => self
                .packed_refs()?
                .get(name)
                .copied()
                .map(RefValue::Direct)
                .ok_or_else(|| Error::RefNotFound(name.to_owned())),
            result => result,
        }
    }

    fn read_loose_ref(&self, name: &str) -> Result<RefValue> {
        packed::validate_name(name)?;
        let ref_path = self.git_dir.join(name);

        let content = fs::read_to_string(&ref_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::RefNotFound(name.to_string())
            } else {
                Error::Io(e)
            }
        })?;

        let content = content.trim();

        // Check if it's a symbolic reference
        if let Some(target) = content.strip_prefix("ref: ") {
            packed::validate_name(target)?;
            Ok(RefValue::Symbolic(target.to_string()))
        } else {
            // It should be a direct SHA-1 reference
            let oid = Oid::from_hex(content)?;
            Ok(RefValue::Direct(oid))
        }
    }

    fn packed_refs(&self) -> Result<BTreeMap<String, Oid>> {
        match fs::read_to_string(self.git_dir.join("packed-refs")) {
            Ok(content) => packed::parse(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub(crate) fn is_packed(&self, name: &str) -> Result<bool> {
        Ok(self.packed_refs()?.contains_key(name))
    }

    fn list_refs(&self, prefix: &str, packed: &BTreeMap<String, Oid>) -> Result<Vec<String>> {
        let mut names: BTreeSet<String> = packed
            .keys()
            .filter_map(|name| name.strip_prefix(prefix).map(str::to_owned))
            .collect();
        let mut loose = Vec::new();
        Self::collect_refs_recursive(&self.git_dir.join(prefix), "", &mut loose)?;
        for name in loose {
            packed::validate_name(&format!("{}{}", prefix, name))?;
            names.insert(name);
        }
        Ok(names.into_iter().collect())
    }

    /// Resolves an enumeration using one packed-refs snapshot for the whole operation.
    pub(crate) fn resolved_refs(&self, prefix: &str) -> Result<Vec<ResolvedRef>> {
        let packed = self.packed_refs()?;
        self.list_refs(prefix, &packed)?
            .into_iter()
            .map(|name| {
                let full_name = format!("{}{}", prefix, name);
                let mut resolved =
                    self.resolve_with(&full_name, |name| match self.read_loose_ref(name) {
                        Err(Error::RefNotFound(_)) => packed
                            .get(name)
                            .copied()
                            .map(RefValue::Direct)
                            .ok_or_else(|| Error::RefNotFound(name.to_owned())),
                        result => result,
                    })?;
                resolved.name = full_name;
                Ok(resolved)
            })
            .collect()
    }

    /// Resolves a reference recursively until we get an OID.
    ///
    /// This follows symbolic references until a direct reference is found.
    ///
    /// # Arguments
    ///
    /// * `name` - The reference name to resolve.
    ///
    /// # Returns
    ///
    /// The resolved reference with its final name and OID.
    pub fn resolve_recursive(&self, name: &str) -> Result<ResolvedRef> {
        self.resolve_with(name, |name| self.read_ref_file(name))
    }

    fn resolve_with(
        &self,
        name: &str,
        read: impl Fn(&str) -> Result<RefValue>,
    ) -> Result<ResolvedRef> {
        let mut current_name = name.to_string();
        let mut depth = 0;
        const MAX_DEPTH: usize = 10;

        loop {
            if depth >= MAX_DEPTH {
                return Err(Error::InvalidRefName(format!(
                    "reference loop or too many levels: {}",
                    name
                )));
            }

            match read(&current_name)? {
                RefValue::Direct(oid) => {
                    return Ok(ResolvedRef {
                        name: current_name,
                        oid,
                    });
                }
                RefValue::Symbolic(target) => {
                    current_name = target;
                    depth += 1;
                }
            }
        }
    }

    /// Returns the current HEAD reference.
    ///
    /// This resolves HEAD to find the commit it points to.
    ///
    /// # Returns
    ///
    /// The resolved HEAD reference, or an error if HEAD doesn't exist
    /// or points to an unborn branch.
    pub fn head(&self) -> Result<ResolvedRef> {
        self.resolve_recursive("HEAD")
    }

    /// Returns the name of the current branch, if HEAD points to a branch.
    ///
    /// # Returns
    ///
    /// The branch name (e.g., "main") if HEAD is on a branch,
    /// or `None` if HEAD is detached.
    pub fn current_branch(&self) -> Result<Option<String>> {
        match self.read_ref_file("HEAD")? {
            RefValue::Symbolic(target) => {
                if let Some(branch) = target.strip_prefix("refs/heads/") {
                    Ok(Some(branch.to_string()))
                } else {
                    Ok(None)
                }
            }
            RefValue::Direct(_) => Ok(None), // Detached HEAD
        }
    }

    /// Lists all branches in the repository.
    ///
    /// # Returns
    ///
    /// A vector of branch names (without the `refs/heads/` prefix).
    pub fn branches(&self) -> Result<Vec<String>> {
        self.list_refs("refs/heads/", &self.packed_refs()?)
    }

    /// Recursively collects reference names from a directory.
    fn collect_refs_recursive(dir: &Path, prefix: &str, refs: &mut Vec<String>) -> Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(Error::Io(e)),
        };

        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            let full_name = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", prefix, name)
            };

            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_file() && !name.ends_with(".lock") {
                refs.push(full_name);
            } else if file_type.is_dir() {
                Self::collect_refs_recursive(&path, &full_name, refs)?;
            }
        }

        Ok(())
    }

    /// Lists all tags in the repository.
    ///
    /// # Returns
    ///
    /// A vector of tag names (without the `refs/tags/` prefix).
    pub fn tags(&self) -> Result<Vec<String>> {
        self.list_refs("refs/tags/", &self.packed_refs()?)
    }

    /// Lists all remote names in the repository.
    ///
    /// # Returns
    ///
    /// A vector of remote names (e.g., "origin", "upstream").
    pub fn remotes(&self) -> Result<Vec<String>> {
        let mut remotes: BTreeSet<String> = self
            .remote_branches()?
            .into_iter()
            .map(|(remote, _)| remote)
            .collect();
        match fs::read_dir(self.git_dir.join("refs/remotes")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        remotes.insert(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(e)),
        }
        Ok(remotes.into_iter().collect())
    }

    /// Lists all remote branches in the repository.
    ///
    /// # Returns
    ///
    /// A vector of tuples (remote_name, branch_name) without the `refs/remotes/` prefix.
    /// For example: `[("origin", "main"), ("origin", "develop"), ("upstream", "main")]`
    pub fn remote_branches(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .list_refs("refs/remotes/", &self.packed_refs()?)?
            .into_iter()
            .filter_map(|name| {
                name.split_once('/')
                    .map(|(remote, branch)| (remote.to_owned(), branch.to_owned()))
            })
            .collect())
    }

    /// Resolves a reference by name.
    ///
    /// This handles both full ref names (e.g., "refs/heads/main") and
    /// short names (e.g., "main", "HEAD").
    ///
    /// # Arguments
    ///
    /// * `name` - The reference name to resolve.
    ///
    /// # Returns
    ///
    /// The resolved reference, or an error if not found.
    pub fn resolve(&self, name: &str) -> Result<ResolvedRef> {
        for candidate in [
            name.to_owned(),
            format!("refs/heads/{}", name),
            format!("refs/tags/{}", name),
        ] {
            match self.resolve_recursive(&candidate) {
                Ok(resolved) => return Ok(resolved),
                Err(Error::RefNotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

        Err(Error::RefNotFound(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_dir() -> TempDir {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path();

        // Create basic structure
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("refs/tags")).unwrap();

        temp
    }

    const TEST_OID: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    const TEST_OID2: &str = "0123456789abcdef0123456789abcdef01234567";

    // R-001: read_ref_file reads direct reference
    #[test]
    fn test_read_ref_file_direct() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        // Create a direct reference
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let value = store.read_ref_file("refs/heads/main").unwrap();

        match value {
            RefValue::Direct(oid) => assert_eq!(oid.to_hex(), TEST_OID),
            _ => panic!("Expected direct reference"),
        }
    }

    // R-002: read_ref_file reads symbolic reference
    #[test]
    fn test_read_ref_file_symbolic() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        // Create HEAD as symbolic ref
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let store = RefStore::new(git_dir);
        let value = store.read_ref_file("HEAD").unwrap();

        match value {
            RefValue::Symbolic(target) => assert_eq!(target, "refs/heads/main"),
            _ => panic!("Expected symbolic reference"),
        }
    }

    // R-003: resolve_recursive resolves symbolic to direct
    #[test]
    fn test_resolve_recursive() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        // HEAD -> refs/heads/main -> OID
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let resolved = store.resolve_recursive("HEAD").unwrap();

        assert_eq!(resolved.name, "refs/heads/main");
        assert_eq!(resolved.oid.to_hex(), TEST_OID);
    }

    // R-004: head() returns resolved HEAD
    #[test]
    fn test_head() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let head = store.head().unwrap();

        assert_eq!(head.oid.to_hex(), TEST_OID);
    }

    // R-005: branches() lists all branches
    #[test]
    fn test_branches() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();
        fs::write(
            git_dir.join("refs/heads/feature"),
            format!("{}\n", TEST_OID2),
        )
        .unwrap();
        fs::create_dir_all(git_dir.join("refs/heads/feature")).ok(); // Nested branch
        fs::write(
            git_dir.join("refs/heads/develop"),
            format!("{}\n", TEST_OID),
        )
        .unwrap();

        let store = RefStore::new(git_dir);
        let branches = store.branches().unwrap();

        assert!(branches.contains(&"main".to_string()));
        assert!(branches.contains(&"feature".to_string()));
        assert!(branches.contains(&"develop".to_string()));
    }

    // Additional: read_ref_file returns RefNotFound for missing ref
    #[test]
    fn test_read_ref_not_found() {
        let temp = setup_git_dir();
        let store = RefStore::new(temp.path());

        let result = store.read_ref_file("refs/heads/nonexistent");
        assert!(matches!(result, Err(Error::RefNotFound(_))));
    }

    // Additional: current_branch returns branch name
    #[test]
    fn test_current_branch() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let branch = store.current_branch().unwrap();

        assert_eq!(branch, Some("main".to_string()));
    }

    // Additional: current_branch returns None for detached HEAD
    #[test]
    fn test_current_branch_detached() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        // Detached HEAD directly points to OID
        fs::write(git_dir.join("HEAD"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let branch = store.current_branch().unwrap();

        assert_eq!(branch, None);
    }

    // Additional: resolve with short name
    #[test]
    fn test_resolve_short_name() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let resolved = store.resolve("main").unwrap();

        assert_eq!(resolved.oid.to_hex(), TEST_OID);
    }

    // Additional: tags() lists all tags
    #[test]
    fn test_tags() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::write(git_dir.join("refs/tags/v1.0"), format!("{}\n", TEST_OID)).unwrap();
        fs::write(git_dir.join("refs/tags/v2.0"), format!("{}\n", TEST_OID2)).unwrap();

        let store = RefStore::new(git_dir);
        let tags = store.tags().unwrap();

        assert!(tags.contains(&"v1.0".to_string()));
        assert!(tags.contains(&"v2.0".to_string()));
    }

    // Additional: resolve_recursive handles multiple levels
    #[test]
    fn test_resolve_multiple_symbolic() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        // HEAD -> refs/heads/alias -> refs/heads/main (Git doesn't normally do this, but we support it)
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/alias\n").unwrap();
        fs::write(git_dir.join("refs/heads/alias"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{}\n", TEST_OID)).unwrap();

        let store = RefStore::new(git_dir);
        let resolved = store.resolve_recursive("HEAD").unwrap();

        assert_eq!(resolved.name, "refs/heads/main");
        assert_eq!(resolved.oid.to_hex(), TEST_OID);
    }

    // Additional: empty branches list
    #[test]
    fn test_branches_empty() {
        let temp = setup_git_dir();
        let store = RefStore::new(temp.path());

        let branches = store.branches().unwrap();
        assert!(branches.is_empty());
    }

    // Additional: nested branch names (e.g., feature/my-feature)
    #[test]
    fn test_nested_branch_names() {
        let temp = setup_git_dir();
        let git_dir = temp.path();

        fs::create_dir_all(git_dir.join("refs/heads/feature")).unwrap();
        fs::write(
            git_dir.join("refs/heads/feature/my-feature"),
            format!("{}\n", TEST_OID),
        )
        .unwrap();
        fs::write(
            git_dir.join("refs/heads/feature/other"),
            format!("{}\n", TEST_OID2),
        )
        .unwrap();

        let store = RefStore::new(git_dir);
        let branches = store.branches().unwrap();

        assert!(branches.contains(&"feature/my-feature".to_string()));
        assert!(branches.contains(&"feature/other".to_string()));
    }
}
