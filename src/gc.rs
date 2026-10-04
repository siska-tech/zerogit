//! Housekeeping: packing references (`git pack-refs`).
//!
//! [`Repository::pack_refs`] moves loose references into `packed-refs` as
//! `git pack-refs --all` does, so a repository with many branches and tags
//! keeps few files under `.git/refs/`.

use std::collections::BTreeMap;
use std::fs;

use crate::error::{Error, Result};
use crate::infra::LockFile;
use crate::objects::{ObjectType, Oid};
use crate::refs::RefValue;
use crate::repository::Repository;

/// References Git keeps per work tree and never packs.
const PER_WORKTREE: &[&str] = &["refs/bisect/", "refs/worktree/", "refs/rewritten/"];

/// The header Git writes: every record that points to an annotated tag is
/// followed by the object it peels to, and the records are sorted.
const PACKED_REFS_HEADER: &str = "# pack-refs with: peeled fully-peeled sorted \n";

impl Repository {
    /// Moves every loose reference into `packed-refs`, like
    /// `git pack-refs --all`, and removes the loose files.
    ///
    /// Branches, tags, remote-tracking branches and other references under
    /// `refs/` are written to `packed-refs` in Git's format (sorted, with
    /// the peeled object after each annotated tag), then each loose file is
    /// deleted, along with the directories it leaves empty below
    /// `refs/<kind>/`. Symbolic references (such as
    /// `refs/remotes/origin/HEAD`), references kept per work tree
    /// (`refs/bisect/`, `refs/worktree/`, `refs/rewritten/`) and references
    /// to missing objects stay loose. Reflogs are kept.
    ///
    /// A loose reference that changes while packing (or whose lock is held
    /// by another process) is left loose, where it takes precedence over
    /// the packed value, so no update is lost.
    ///
    /// # Errors
    ///
    /// `Error::Locked` if `packed-refs` is locked; nothing is changed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.pack_refs().unwrap();
    /// ```
    pub fn pack_refs(&self) -> Result<()> {
        let path = self.git_dir().join("packed-refs");
        let store = self.ref_store();
        // Read packed-refs under its lock so a concurrent rewrite is not
        // lost.
        let mut lock = LockFile::acquire(&path)?;
        let mut refs: BTreeMap<String, Oid> = store.packed_refs()?;

        let mut loose_names = Vec::new();
        crate::refs::RefStore::collect_refs_recursive(
            &self.git_dir().join("refs"),
            "refs",
            &mut loose_names,
        )?;
        let mut packed_loose = Vec::new();
        for name in loose_names {
            if PER_WORKTREE.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let oid = match store.read_loose_ref(&name) {
                Ok(RefValue::Direct(oid)) => oid,
                Ok(RefValue::Symbolic(_)) => continue,
                // Removed meanwhile.
                Err(Error::RefNotFound(_)) => continue,
                Err(e) => return Err(e),
            };
            if !self.object_store().exists(&oid)? {
                continue;
            }
            refs.insert(name.clone(), oid);
            packed_loose.push((name, oid));
        }

        let mut content = String::from(PACKED_REFS_HEADER);
        for (name, oid) in &refs {
            content.push_str(&format!("{} {}\n", oid.to_hex(), name));
            if let Some(peeled) = self.peeled_tag(oid)? {
                content.push_str(&format!("^{}\n", peeled.to_hex()));
            }
        }
        lock.write_all(content.as_bytes())?;
        lock.commit()?;

        // Now packed, the loose files can go, unless they changed meanwhile.
        for (name, oid) in packed_loose {
            let ref_path = self.git_dir().join(&name);
            let ref_lock = match LockFile::acquire(&ref_path) {
                Ok(lock) => lock,
                Err(Error::Locked(_)) => continue,
                Err(e) => return Err(e),
            };
            if matches!(store.read_loose_ref(&name), Ok(RefValue::Direct(current)) if current == oid)
            {
                fs::remove_file(&ref_path)?;
            }
            drop(ref_lock);
            // Git keeps `refs/<kind>/` itself.
            let mut parts = name.splitn(3, '/');
            let root = match (parts.next(), parts.next()) {
                (Some(first), Some(second)) => self.git_dir().join(first).join(second),
                _ => self.git_dir().join("refs"),
            };
            self.remove_empty_ref_dirs(&ref_path, &root)?;
        }
        Ok(())
    }

    /// The object an annotated tag finally points to, or `None` if `oid` is
    /// not a tag (or is missing).
    fn peeled_tag(&self, oid: &Oid) -> Result<Option<Oid>> {
        match self.object_store().read(oid) {
            Ok(raw) if raw.object_type == ObjectType::Tag => Ok(Some(self.peel(oid)?)),
            Ok(_) | Err(Error::ObjectNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
