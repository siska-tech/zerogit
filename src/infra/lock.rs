//! Lock files compatible with Git's (`<path>.lock`).
//!
//! Git serializes writers of the index, references, `packed-refs`, the
//! configuration and other shared files by creating `<path>.lock`
//! exclusively. New content is written to the lock file, which is then
//! renamed over `<path>`, releasing the lock. Taking the same lock lets
//! zerogit and Git run side by side without losing each other's updates.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use crate::error::{Error, Result};

/// An exclusive lock on a file, held while `<path>.lock` exists.
///
/// The lock is released by [`LockFile::commit`], which replaces the file
/// with what was written, or by dropping it, which leaves the file as it
/// was.
#[derive(Debug)]
pub(crate) struct LockFile {
    path: PathBuf,
    lock_path: PathBuf,
    file: Option<File>,
}

impl LockFile {
    /// Locks `path` by creating `<path>.lock`, creating missing parent
    /// directories.
    ///
    /// # Errors
    ///
    /// `Error::Locked` with the lock file's path if it already exists, that
    /// is, another process (Git or zerogit) is changing the file.
    pub(crate) fn acquire(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut lock_path = path.clone().into_os_string();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => Ok(Self {
                path,
                lock_path,
                file: Some(file),
            }),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(Error::Locked(lock_path)),
            Err(e) => Err(e.into()),
        }
    }

    /// Appends `data` to the new content of the file.
    pub(crate) fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let file = self.file.as_mut().expect("lock file is open until commit");
        file.write_all(data)?;
        Ok(())
    }

    /// Flushes what was written and closes the lock file, keeping the lock,
    /// so that many locks can be held at once. Nothing more can be written.
    pub(crate) fn close(&mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        Ok(())
    }

    /// Replaces the file with what was written and releases the lock.
    pub(crate) fn commit(mut self) -> Result<()> {
        self.close()?;
        fs::rename(&self.lock_path, &self.path)?;
        // Renamed away: nothing for drop to clean up.
        self.lock_path = PathBuf::new();
        Ok(())
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        self.file = None;
        if !self.lock_path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

/// Replaces the content of `path` under its lock, as Git does for a single
/// update of a shared file.
///
/// # Errors
///
/// `Error::Locked` if `<path>.lock` already exists; the file is not changed.
pub(crate) fn write_locked(path: impl Into<PathBuf>, data: &[u8]) -> Result<()> {
    let mut lock = LockFile::acquire(path)?;
    lock.write_all(data)?;
    lock.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn commit_replaces_the_file_and_removes_the_lock() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("index");
        fs::write(&path, b"old").unwrap();

        let mut lock = LockFile::acquire(&path).unwrap();
        assert!(temp.path().join("index.lock").is_file());
        lock.write_all(b"new").unwrap();
        lock.commit().unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!temp.path().join("index.lock").exists());
    }

    #[test]
    fn existing_lock_is_reported_and_kept() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("refs/heads/main");
        let held = LockFile::acquire(&path).unwrap();

        match LockFile::acquire(&path) {
            Err(Error::Locked(lock_path)) => {
                assert_eq!(lock_path, temp.path().join("refs/heads/main.lock"))
            }
            other => panic!("expected Locked, got {:?}", other),
        }
        assert!(matches!(write_locked(&path, b"x"), Err(Error::Locked(_))));
        // The other holder's lock is untouched by the failed attempts.
        assert!(temp.path().join("refs/heads/main.lock").is_file());
        drop(held);
        assert!(!temp.path().join("refs/heads/main.lock").exists());
        assert!(!path.exists());
    }

    #[test]
    fn dropping_without_commit_leaves_the_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config");
        fs::write(&path, b"old").unwrap();
        {
            let mut lock = LockFile::acquire(&path).unwrap();
            lock.write_all(b"partial").unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert!(!temp.path().join("config.lock").exists());
    }
}
