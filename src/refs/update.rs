//! Reading and updating arbitrary references (used by fetch, push and
//! clone).

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::infra::LockFile;
use crate::objects::Oid;
use crate::refs::reflog::zero_oid;
use crate::repository::Repository;

impl Repository {
    /// Lists every reference under `refs/` with the commit or object it
    /// points to (symbolic references resolved), sorted by name. Loose and
    /// packed references are both included.
    pub fn references(&self) -> Result<Vec<(String, Oid)>> {
        Ok(self
            .ref_store()
            .resolved_refs("refs/")?
            .into_iter()
            .map(|r| (r.name, r.oid))
            .collect())
    }

    /// Returns the object a reference points to, following symbolic
    /// references, or `None` if it does not exist. `name` is a full name
    /// such as `refs/heads/main`, or `HEAD`.
    pub fn find_reference(&self, name: &str) -> Result<Option<Oid>> {
        match self.ref_store().resolve_recursive(name) {
            Ok(resolved) => Ok(Some(resolved.oid)),
            Err(Error::RefNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Creates, moves or (with `new` = `None`) deletes a reference, like
    /// `git update-ref`, recording `message` in its reflog.
    ///
    /// With `expected`, the update only happens if the reference currently
    /// has that value (`Some(None)`: it must not exist); this guards
    /// against concurrent changes.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRefName` if `name` is not a valid `refs/...` name.
    /// - `Error::StaleReference` if `expected` does not match.
    pub fn update_reference(
        &self,
        name: &str,
        new: Option<&Oid>,
        expected: Option<Option<Oid>>,
        message: &str,
    ) -> Result<()> {
        let rest = name
            .strip_prefix("refs/")
            .ok_or_else(|| Error::InvalidRefName(name.to_owned()))?;
        crate::repository::validate_ref_name("reference", rest)?;
        // Lock the reference before reading it, so that the comparison and
        // the update are one step (compare-and-swap), as in Git.
        let path = self.git_dir().join(name);
        let mut lock = LockFile::acquire(&path)?;
        let current = self.find_reference(name)?;
        if let Some(expected) = expected {
            if current != expected {
                return Err(Error::StaleReference(name.to_owned()));
            }
        }
        let reflog = self.reflog_writer()?;
        let who = self.reflog_identity()?;
        match new {
            Some(oid) => {
                lock.write_all(format!("{}\n", oid.to_hex()).as_bytes())?;
                reflog.append(name, &current.unwrap_or_else(zero_oid), oid, &who, message)?;
                lock.commit()?;
            }
            None => {
                if current.is_none() {
                    return Ok(());
                }
                if self.ref_store().is_packed(name)? {
                    self.remove_packed_ref(name)?;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                reflog.delete(name)?;
            }
        }
        Ok(())
    }

    /// Makes `name` (`HEAD` or a `refs/...` name) a symbolic reference to
    /// `target`, like `git symbolic-ref`, whether or not `target` exists
    /// yet. With `message`, the change is recorded in the reflog of `name`
    /// when `target` points to a commit. The work tree is not changed.
    pub fn set_symbolic_reference(
        &self,
        name: &str,
        target: &str,
        message: Option<&str>,
    ) -> Result<()> {
        for refname in [name, target] {
            if refname != "HEAD" {
                let rest = refname
                    .strip_prefix("refs/")
                    .ok_or_else(|| Error::InvalidRefName(refname.to_owned()))?;
                crate::repository::validate_ref_name("reference", rest)?;
            }
        }
        let mut lock = LockFile::acquire(self.git_dir().join(name))?;
        let old = self.find_reference(name)?;
        lock.write_all(format!("ref: {}\n", target).as_bytes())?;
        if let (Some(message), Some(new)) = (message, self.find_reference(target)?) {
            self.reflog_writer()?.append(
                name,
                &old.unwrap_or_else(zero_oid),
                &new,
                &self.reflog_identity()?,
                message,
            )?;
        }
        lock.commit()
    }

    /// Returns whether an object exists (loose or packed).
    pub fn has_object(&self, oid: &Oid) -> Result<bool> {
        self.object_store().exists(oid)
    }

    /// Returns the type of an object.
    pub fn object_type(&self, oid: &Oid) -> Result<crate::objects::ObjectType> {
        Ok(self.object_store().read(oid)?.object_type)
    }

    /// Follows annotated tags from `oid` to the object they finally point
    /// to (`<oid>^{}`); other objects are returned as they are.
    pub fn peel(&self, oid: &Oid) -> Result<Oid> {
        let mut oid = *oid;
        loop {
            let raw = self.object_store().read(&oid)?;
            if raw.object_type != crate::objects::ObjectType::Tag {
                return Ok(oid);
            }
            oid = *crate::objects::TagObject::parse(raw)?.object();
        }
    }

    /// Returns whether `ancestor` is reachable from `descendant` (a commit
    /// is its own ancestor), as `git merge-base --is-ancestor` does.
    pub fn is_ancestor(&self, ancestor: &Oid, descendant: &Oid) -> Result<bool> {
        Ok(self
            .merge_bases(ancestor, descendant)?
            .iter()
            .any(|base| base == ancestor))
    }
}

impl Repository {
    /// Checks that `refs/<namespace>/<name>` can be created: neither it nor a
    /// reference that would conflict with it as a directory or file exists
    /// (`a` and `a/b` cannot both exist).
    pub(crate) fn check_new_ref(&self, namespace: &str, name: &str) -> Result<String> {
        let ref_name = format!("refs/{}/{}", namespace, name);
        let prefix = format!("refs/{}/", namespace);
        for existing in self.ref_store().resolved_refs(&prefix)? {
            let other = &existing.name;
            if *other == ref_name
                || other.starts_with(&format!("{}/", ref_name))
                || ref_name.starts_with(&format!("{}/", other))
            {
                return Err(Error::RefAlreadyExists(other.clone()));
            }
        }
        match self.ref_store().read_ref_file(&ref_name) {
            Ok(_) => Err(Error::RefAlreadyExists(ref_name)),
            Err(Error::RefNotFound(_)) => Ok(ref_name),
            Err(e) => Err(e),
        }
    }

    /// Writes a loose reference file.
    pub(crate) fn write_ref(&self, ref_name: &str, oid: &Oid) -> Result<()> {
        let mut lock = self.lock_new_ref(ref_name)?;
        lock.write_all(format!("{}\n", oid.to_hex()).as_bytes())?;
        lock.commit()
    }

    /// Locks a reference that is about to be created, and checks under the
    /// lock that it still does not exist, so that a reference created
    /// concurrently (after [`Repository::check_new_ref`]) is not overwritten.
    pub(crate) fn lock_new_ref(&self, ref_name: &str) -> Result<LockFile> {
        let lock = LockFile::acquire(self.git_dir.join(ref_name))?;
        match self.ref_store().read_ref_file(ref_name) {
            Ok(_) => Err(Error::RefAlreadyExists(ref_name.to_owned())),
            Err(Error::RefNotFound(_)) => Ok(lock),
            Err(e) => Err(e),
        }
    }

    /// Removes a reference (and its peeled line) from `packed-refs`.
    pub(crate) fn remove_packed_ref(&self, ref_name: &str) -> Result<()> {
        let path = self.git_dir.join("packed-refs");
        // Read under the lock so a concurrent rewrite is not lost.
        let mut lock = LockFile::acquire(&path)?;
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut out = String::with_capacity(content.len());
        let mut skipping = false;
        for line in content.lines() {
            if line.starts_with('^') {
                if !skipping {
                    out.push_str(line);
                    out.push('\n');
                }
                continue;
            }
            skipping = line.split_once(' ').map(|(_, name)| name) == Some(ref_name);
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }
        lock.write_all(out.as_bytes())?;
        lock.commit()
    }

    /// Deletes a reference, loose or packed (or both), its reflog and the
    /// directories under `root` it leaves empty.
    pub(crate) fn delete_ref(&self, ref_name: &str, root: &str) -> Result<()> {
        let store = self.ref_store();
        let path = self.git_dir.join(ref_name);
        let lock = LockFile::acquire(&path)?;
        store.read_ref_file(ref_name)?;
        // The packed record first: removing only the loose file would
        // uncover an older packed value.
        if store.is_packed(ref_name)? {
            self.remove_packed_ref(ref_name)?;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        self.reflog_writer()?.delete(ref_name)?;
        // Release the lock first: its file would keep the directory non-empty.
        drop(lock);
        self.remove_empty_ref_dirs(&path, &self.git_dir.join(root))
    }

    /// Removes the directories above a deleted reference file that are now
    /// empty, up to (not including) `root`.
    pub(crate) fn remove_empty_ref_dirs(&self, path: &Path, root: &Path) -> Result<()> {
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if dir == root || !dir.starts_with(root) {
                break;
            }
            match dir.read_dir() {
                Ok(mut entries) => {
                    if entries.next().is_some() {
                        break;
                    }
                    fs::remove_dir(dir)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            parent = dir.parent();
        }
        Ok(())
    }
}
