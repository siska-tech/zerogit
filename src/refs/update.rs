//! Reading and updating arbitrary references (used by fetch, push and
//! clone).

use crate::error::{Error, Result};
use crate::infra::write_file_atomic;
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
                let path = self.git_dir().join(name);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_file_atomic(&path, format!("{}\n", oid.to_hex()).as_bytes())?;
                reflog.append(name, &current.unwrap_or_else(zero_oid), oid, &who, message)?;
            }
            None => {
                if current.is_none() {
                    return Ok(());
                }
                if self.ref_store().is_packed(name)? {
                    self.remove_packed_ref(name)?;
                }
                let path = self.git_dir().join(name);
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
        let old = self.find_reference(name)?;
        let path = self.git_dir().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_file_atomic(&path, format!("ref: {}\n", target).as_bytes())?;
        if let (Some(message), Some(new)) = (message, self.find_reference(target)?) {
            self.reflog_writer()?.append(
                name,
                &old.unwrap_or_else(zero_oid),
                &new,
                &self.reflog_identity()?,
                message,
            )?;
        }
        Ok(())
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
