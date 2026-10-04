//! Reflogs (`.git/logs/`).
//!
//! Each line records one update of a reference:
//! `<old-oid> <new-oid> <name> <<email>> <time> <tz>\t<message>`.
//! Which references are logged follows `core.logAllRefUpdates`: by default
//! (true in a repository with a work tree) `HEAD`, `refs/heads/`,
//! `refs/remotes/` and `refs/notes/`; `always` logs every reference; with
//! false only references whose log already exists are logged.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::objects::{Oid, Signature};

/// One entry of a reflog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    old_oid: Oid,
    new_oid: Oid,
    committer: Signature,
    message: String,
}

impl ReflogEntry {
    /// The value before the update (all zeros when the reference was created).
    pub fn old_oid(&self) -> &Oid {
        &self.old_oid
    }

    pub(crate) fn set_old_oid(&mut self, oid: Oid) {
        self.old_oid = oid;
    }

    /// The value after the update (all zeros when the reference was deleted).
    pub fn new_oid(&self) -> &Oid {
        &self.new_oid
    }

    /// Who made the update, and when.
    pub fn committer(&self) -> &Signature {
        &self.committer
    }

    /// The message, such as `commit: Fix typo` or
    /// `checkout: moving from main to feature`.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// The all-zero OID used for "no value".
pub(crate) fn zero_oid() -> Oid {
    Oid::from_bytes([0; 20])
}

/// When to write reflogs (`core.logAllRefUpdates`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogAll {
    False,
    True,
    Always,
}

/// Writes and reads the reflogs of one repository.
#[derive(Debug)]
pub(crate) struct Reflog {
    logs_dir: PathBuf,
    mode: LogAll,
}

impl Reflog {
    pub(crate) fn new(git_dir: &Path, config: &Config, bare: bool) -> Self {
        let mode = match config.get("core", "logallrefupdates") {
            Some(v) if v.eq_ignore_ascii_case("always") => LogAll::Always,
            Some(_) if config.get_bool_or("core", "logallrefupdates", !bare) => LogAll::True,
            Some(_) => LogAll::False,
            None if bare => LogAll::False,
            None => LogAll::True,
        };
        Reflog {
            logs_dir: git_dir.join("logs"),
            mode,
        }
    }

    fn path(&self, refname: &str) -> PathBuf {
        let mut path = self.logs_dir.clone();
        for part in refname.split('/') {
            path.push(part);
        }
        path
    }

    fn should_log(&self, refname: &str) -> bool {
        // The stash list is its reflog, so it is always written.
        if refname == "refs/stash" {
            return true;
        }
        match self.mode {
            LogAll::Always => true,
            LogAll::True => {
                refname == "HEAD"
                    || ["refs/heads/", "refs/remotes/", "refs/notes/"]
                        .iter()
                        .any(|prefix| refname.starts_with(prefix))
            }
            LogAll::False => false,
        }
    }

    /// Appends an entry for an update of `refname`, if it is to be logged.
    pub(crate) fn append(
        &self,
        refname: &str,
        old: &Oid,
        new: &Oid,
        committer: &Signature,
        message: &str,
    ) -> Result<()> {
        let path = self.path(refname);
        if !self.should_log(refname) && !path.is_file() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let line = format!(
            "{} {} {}\t{}\n",
            old.to_hex(),
            new.to_hex(),
            committer.to_git_string(),
            normalize_message(message)
        );
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Deletes the reflog of a reference and the directories it leaves empty.
    pub(crate) fn delete(&self, refname: &str) -> Result<()> {
        let path = self.path(refname);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if dir == self.logs_dir.join("refs") || dir == self.logs_dir {
                break;
            }
            let empty = match dir.read_dir() {
                Ok(mut entries) => entries.next().is_none(),
                Err(_) => false,
            };
            if !empty {
                break;
            }
            fs::remove_dir(dir)?;
            parent = dir.parent();
        }
        Ok(())
    }

    /// Reads the reflog of a reference, newest entry first (as
    /// `git reflog` lists it). A reference without a log has no entries.
    pub(crate) fn read(&self, refname: &str) -> Result<Vec<ReflogEntry>> {
        let content = match fs::read(self.path(refname)) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut entries = Vec::new();
        for (number, line) in content.split(|&b| b == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let line = String::from_utf8_lossy(line);
            let invalid = || Error::InvalidObject {
                oid: format!("logs/{}", refname),
                reason: format!("malformed reflog line {}", number + 1),
            };
            let (head, message) = line.split_once('\t').unwrap_or((&line, ""));
            if head.len() < 82 {
                return Err(invalid());
            }
            let old_oid = Oid::from_hex(&head[..40]).map_err(|_| invalid())?;
            let new_oid = Oid::from_hex(&head[41..81]).map_err(|_| invalid())?;
            let committer = Signature::parse(&head[82..]).map_err(|_| invalid())?;
            entries.push(ReflogEntry {
                old_oid,
                new_oid,
                committer,
                message: message.to_owned(),
            });
        }
        entries.reverse();
        Ok(entries)
    }
}

impl Reflog {
    /// Replaces the reflog with `entries` (newest first), or deletes it when
    /// empty.
    pub(crate) fn rewrite(&self, refname: &str, entries: &[ReflogEntry]) -> Result<()> {
        if entries.is_empty() {
            return self.delete(refname);
        }
        let mut content = String::new();
        for entry in entries.iter().rev() {
            content.push_str(&format!(
                "{} {} {}\t{}\n",
                entry.old_oid.to_hex(),
                entry.new_oid.to_hex(),
                entry.committer.to_git_string(),
                entry.message
            ));
        }
        crate::infra::write_file_atomic(self.path(refname), content.as_bytes())
    }
}

/// Collapses whitespace runs (including newlines) to single spaces and trims
/// the message, as Git does for reflog messages.
fn normalize_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    #[test]
    fn test_append_read_delete() {
        let temp = TempDir::new().unwrap();
        let reflog = Reflog::new(temp.path(), &Config::new(), false);
        let who = Signature::new("A U Thor", "a@example.com", 1700000000, 540);
        reflog
            .append(
                "refs/heads/topic/x",
                &zero_oid(),
                &oid(1),
                &who,
                "branch: Created from HEAD",
            )
            .unwrap();
        reflog
            .append(
                "refs/heads/topic/x",
                &oid(1),
                &oid(2),
                &who,
                "commit: Two\n\nbody",
            )
            .unwrap();
        let content = fs::read_to_string(temp.path().join("logs/refs/heads/topic/x")).unwrap();
        assert!(content.ends_with(&format!(
            "{} {} A U Thor <a@example.com> 1700000000 +0900\tcommit: Two body\n",
            oid(1).to_hex(),
            oid(2).to_hex()
        )));

        let entries = reflog.read("refs/heads/topic/x").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message(), "commit: Two body");
        assert_eq!(entries[1].old_oid(), &zero_oid());
        assert_eq!(entries[0].committer(), &who);

        reflog.delete("refs/heads/topic/x").unwrap();
        assert!(!temp.path().join("logs/refs/heads/topic").exists());
        assert!(reflog.read("refs/heads/topic/x").unwrap().is_empty());
    }

    #[test]
    fn test_log_all_ref_updates() {
        let temp = TempDir::new().unwrap();
        let who = Signature::new("A", "a@example.com", 0, 0);
        // Tags are not logged by default; bare repositories log nothing.
        let reflog = Reflog::new(temp.path(), &Config::new(), false);
        reflog
            .append("refs/tags/v1", &zero_oid(), &oid(1), &who, "x")
            .unwrap();
        assert!(!temp.path().join("logs/refs/tags/v1").exists());
        let bare = Reflog::new(temp.path(), &Config::new(), true);
        bare.append("HEAD", &zero_oid(), &oid(1), &who, "x")
            .unwrap();
        assert!(!temp.path().join("logs/HEAD").exists());

        let always = Config::from_str("[core]\n\tlogAllRefUpdates = always\n").unwrap();
        let reflog = Reflog::new(temp.path(), &always, false);
        reflog
            .append("refs/tags/v1", &zero_oid(), &oid(1), &who, "x")
            .unwrap();
        assert!(temp.path().join("logs/refs/tags/v1").exists());

        // With false, an existing log is still appended to.
        let off = Config::from_str("[core]\n\tlogAllRefUpdates = false\n").unwrap();
        let reflog = Reflog::new(temp.path(), &off, false);
        reflog
            .append("refs/tags/v1", &oid(1), &oid(2), &who, "y")
            .unwrap();
        reflog
            .append("refs/heads/main", &zero_oid(), &oid(2), &who, "y")
            .unwrap();
        assert_eq!(reflog.read("refs/tags/v1").unwrap().len(), 2);
        assert!(!temp.path().join("logs/refs/heads/main").exists());
    }
}
