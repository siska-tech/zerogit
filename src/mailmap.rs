//! Canonical names and email addresses (`.mailmap`).
//!
//! A mailmap maps the names and addresses recorded in commits to the ones
//! a person wants to be known by, as `git check-mailmap`, `git shortlog`
//! and `git log --use-mailmap` apply them. [`Repository::mailmap`] reads
//! the repository's; [`Mailmap::resolve`] applies it.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::objects::{ObjectType, Signature};
use crate::repository::Repository;

/// What an identity maps to: a new name, a new address, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Replacement {
    name: Option<String>,
    email: Option<String>,
}

/// The mappings of one address: for any name, and for particular names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Entry {
    simple: Replacement,
    /// By lowercase name.
    by_name: HashMap<String, Replacement>,
}

/// A parsed mailmap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mailmap {
    /// By lowercase address.
    entries: HashMap<String, Entry>,
}

/// Splits `Name <email>` off the start of `text`: the name (trimmed, `None`
/// when empty), the address, and the rest after `>` (`None` when empty).
/// `None` if there is no `<...>` (or, unless `allow_empty`, it is empty).
fn name_and_email(text: &str, allow_empty: bool) -> Option<(Option<&str>, &str, Option<&str>)> {
    let left = text.find('<')?;
    let right = left + 1 + text[left + 1..].find('>')?;
    if !allow_empty && right == left + 1 {
        return None;
    }
    let name = text[..left].trim_matches(|c: char| c.is_ascii_whitespace());
    let rest = &text[right + 1..];
    Some((
        (!name.is_empty()).then_some(name),
        &text[left + 1..right],
        (!rest.is_empty()).then_some(rest),
    ))
}

impl Mailmap {
    /// An empty mailmap, which changes nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses mailmap lines and adds them, later lines overriding earlier
    /// ones as in Git. Each line is one of
    ///
    /// ```text
    /// Proper Name <commit@email>
    /// <proper@email> <commit@email>
    /// Proper Name <proper@email> <commit@email>
    /// Proper Name <proper@email> Commit Name <commit@email>
    /// ```
    ///
    /// Lines starting with `#` and lines without an address are ignored.
    pub fn add(&mut self, text: &str) {
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let Some((name1, email1, rest)) = name_and_email(line, false) else {
                continue;
            };
            let (name2, email2) = match rest.and_then(|rest| name_and_email(rest, true)) {
                Some((name2, email2, _)) => (name2, Some(email2)),
                None => (None, None),
            };
            // Without a second address, the first is the one to match.
            let (new_email, old_email) = match email2 {
                Some(old) => (Some(email1), old),
                None => (None, email1),
            };
            let entry = self
                .entries
                .entry(old_email.to_ascii_lowercase())
                .or_default();
            match name2 {
                None => {
                    if let Some(name) = name1 {
                        entry.simple.name = Some(name.to_owned());
                    }
                    if let Some(email) = new_email {
                        entry.simple.email = Some(email.to_owned());
                    }
                }
                Some(old_name) => {
                    entry.by_name.insert(
                        old_name.to_ascii_lowercase(),
                        Replacement {
                            name: name1.map(str::to_owned),
                            email: new_email.map(str::to_owned),
                        },
                    );
                }
            }
        }
    }

    /// Parses a mailmap (see [`Mailmap::add`]).
    pub fn parse(text: &str) -> Self {
        let mut mailmap = Mailmap::new();
        mailmap.add(text);
        mailmap
    }

    /// Whether the mailmap has no mappings.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The canonical name and address of `name <email>`, like
    /// `git check-mailmap`. Addresses and names match case-insensitively
    /// (ASCII); a mapping for the name and address together wins over one
    /// for the address alone. Unmapped identities come back unchanged.
    pub fn resolve(&self, name: &str, email: &str) -> (String, String) {
        let replacement = self.entries.get(&email.to_ascii_lowercase()).map(|entry| {
            entry
                .by_name
                .get(&name.to_ascii_lowercase())
                .unwrap_or(&entry.simple)
        });
        match replacement {
            Some(r) => (
                r.name.as_deref().unwrap_or(name).to_owned(),
                r.email.as_deref().unwrap_or(email).to_owned(),
            ),
            None => (name.to_owned(), email.to_owned()),
        }
    }

    /// `signature` with its canonical name and address (see
    /// [`Mailmap::resolve`]); the time is kept.
    pub fn resolve_signature(&self, signature: &Signature) -> Signature {
        let (name, email) = self.resolve(signature.name(), signature.email());
        Signature::new(name, email, signature.timestamp(), signature.tz_offset())
    }
}

impl Repository {
    /// Reads the repository's mailmap as Git does: `.mailmap` at the top of
    /// the work tree (not followed if it is a symbolic link, as in Git),
    /// then the blob `mailmap.blob` names (by default `HEAD:.mailmap` in a
    /// bare repository), then the file `mailmap.file`; later mappings
    /// override earlier ones. Missing sources are skipped.
    ///
    /// # Errors
    ///
    /// `Error::Io` if a mailmap file exists but cannot be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let mailmap = repo.mailmap().unwrap();
    /// for commit in repo.log().unwrap() {
    ///     let commit = commit.unwrap();
    ///     let author = mailmap.resolve_signature(commit.author());
    ///     println!("{} <{}>", author.name(), author.email());
    /// }
    /// ```
    pub fn mailmap(&self) -> Result<Mailmap> {
        let config = self.config()?;
        let mut mailmap = Mailmap::new();
        if !self.is_bare() {
            let path = self.path().join(".mailmap");
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {}
                Ok(_) => mailmap.add(&read_text(&path)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let blob = match config.get("mailmap", "blob") {
            Some(blob) => Some(blob.to_owned()),
            None if self.is_bare() => Some("HEAD:.mailmap".to_owned()),
            None => None,
        };
        if let Some(blob) = blob.filter(|b| !b.is_empty()) {
            match self.rev_parse(&blob) {
                Ok(oid) => {
                    let raw = self.object_store().read(&oid)?;
                    if raw.object_type == ObjectType::Blob {
                        mailmap.add(&String::from_utf8_lossy(&raw.content));
                    }
                }
                // As in Git, a missing blob is no mailmap.
                Err(Error::RefNotFound(_))
                | Err(Error::ObjectNotFound(_))
                | Err(Error::PathNotFound(_))
                | Err(Error::InvalidRevision { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(file) = config.get("mailmap", "file") {
            if let Some(path) = crate::config::expand_home(file) {
                let path = if path.is_relative() {
                    self.path().join(path)
                } else {
                    path
                };
                match read_text(&path) {
                    Ok(text) => mailmap.add(&text),
                    Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(mailmap)
    }
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_form() {
        let mailmap = Mailmap::parse(
            "# a comment <not@an.entry>\n\
             Proper Name <commit@example.com>\n\
             <proper@example.com> <Old@Example.com>\n\
             Both New <both@example.com> <both-old@example.com>\n\
             Only Joe <joe@example.com> Joe <shared@example.com>\n\
             Other <other@example.com> Someone Else <shared@example.com>\n\
             Shared <shared-default@example.com> <shared@example.com>\n\
             not an entry\n\
             Empty <>\n",
        );
        let r = |name: &str, email: &str| mailmap.resolve(name, email);
        let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            r("x", "commit@example.com"),
            s("Proper Name", "commit@example.com")
        );
        assert_eq!(r("x", "OLD@example.COM"), s("x", "proper@example.com"));
        assert_eq!(
            r("x", "both-old@example.com"),
            s("Both New", "both@example.com")
        );
        assert_eq!(
            r("joe", "shared@example.com"),
            s("Only Joe", "joe@example.com")
        );
        assert_eq!(
            r("Someone Else", "Shared@example.com"),
            s("Other", "other@example.com")
        );
        assert_eq!(
            r("Anyone", "shared@example.com"),
            s("Shared", "shared-default@example.com")
        );
        assert_eq!(r("x", "unknown@example.com"), s("x", "unknown@example.com"));
        assert_eq!(r("x", "not@an.entry"), s("x", "not@an.entry"));
    }

    #[test]
    fn later_lines_override_and_names_are_trimmed() {
        let mailmap = Mailmap::parse(
            "First <a@example.com>\n  Second\t <a@example.com>  \r\n<new@example.com> <a@example.com>\n",
        );
        assert_eq!(
            mailmap.resolve("x", "a@example.com"),
            ("Second".to_owned(), "new@example.com".to_owned())
        );
        assert!(Mailmap::new().is_empty());
        let signature = Signature::new("x", "a@example.com", 1_700_000_000, 540);
        let resolved = mailmap.resolve_signature(&signature);
        assert_eq!(resolved.name(), "Second");
        assert_eq!(resolved.timestamp(), 1_700_000_000);
        assert_eq!(resolved.tz_offset(), 540);
    }
}
