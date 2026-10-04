//! Remotes and refspecs (`remote.<name>.*`, `branch.<name>.remote/merge`).
//!
//! This covers the configuration side of remote operations: reading and
//! editing remotes, mapping references with refspecs, and the upstream of a
//! branch. Talking to a remote is left to a transport (see the separate
//! `zerogit-remote` crate).

use crate::config::edit::ConfigFile;
use crate::error::{Error, Result};
use crate::repository::Repository;

/// A refspec such as `+refs/heads/*:refs/remotes/origin/*`.
///
/// `src` and `dst` are reference names or patterns with a single `*`, which
/// matches any sequence of characters (including `/`). A leading `+` allows
/// non-fast-forward updates; a leading `^` (negative refspec) excludes the
/// matching references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refspec {
    force: bool,
    negative: bool,
    src: String,
    dst: Option<String>,
}

impl Refspec {
    /// Parses a refspec.
    ///
    /// # Errors
    ///
    /// `Error::InvalidRefName` if a side has more than one `*`, or only one
    /// side is a pattern.
    pub fn parse(spec: &str) -> Result<Self> {
        let invalid = || Error::InvalidRefName(format!("invalid refspec: {}", spec));
        let (negative, rest) = match spec.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => (false, spec),
        };
        let (force, rest) = match rest.strip_prefix('+') {
            Some(rest) if !negative => (true, rest),
            _ => (false, rest),
        };
        let (src, dst) = match rest.split_once(':') {
            Some((src, dst)) => (src.to_owned(), Some(dst.to_owned())),
            None => (rest.to_owned(), None),
        };
        if negative && dst.is_some() {
            return Err(invalid());
        }
        let stars = |s: &str| s.matches('*').count();
        if stars(&src) > 1 || dst.as_deref().map_or(0, stars) > 1 {
            return Err(invalid());
        }
        if let Some(dst) = &dst {
            if !dst.is_empty() && !src.is_empty() && (stars(&src) == 1) != (stars(dst) == 1) {
                return Err(invalid());
            }
        }
        Ok(Refspec {
            force,
            negative,
            src,
            dst,
        })
    }

    /// Whether non-fast-forward updates are allowed (`+`).
    pub fn is_force(&self) -> bool {
        self.force
    }

    /// Whether this is a negative refspec (`^`), excluding references.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// The source side.
    pub fn source(&self) -> &str {
        &self.src
    }

    /// The destination side, if any.
    pub fn destination(&self) -> Option<&str> {
        self.dst.as_deref()
    }

    /// Whether the sides are patterns (contain `*`).
    pub fn is_pattern(&self) -> bool {
        self.src.contains('*')
    }

    fn match_pattern(pattern: &str, name: &str) -> Option<String> {
        match pattern.split_once('*') {
            Some((prefix, suffix)) => {
                if name.len() >= prefix.len() + suffix.len()
                    && name.starts_with(prefix)
                    && name.ends_with(suffix)
                {
                    Some(name[prefix.len()..name.len() - suffix.len()].to_owned())
                } else {
                    None
                }
            }
            None => (pattern == name).then(String::new),
        }
    }

    /// Whether `name` matches the source side.
    pub fn matches_source(&self, name: &str) -> bool {
        Self::match_pattern(&self.src, name).is_some()
    }

    /// Maps a source name to its destination: for
    /// `refs/heads/*:refs/remotes/origin/*`, `refs/heads/main` maps to
    /// `refs/remotes/origin/main`. Returns `None` if the name does not match
    /// or there is no destination.
    pub fn map_to_destination(&self, name: &str) -> Option<String> {
        let dst = self.dst.as_deref().filter(|d| !d.is_empty())?;
        let matched = Self::match_pattern(&self.src, name)?;
        Some(match dst.split_once('*') {
            Some((prefix, suffix)) => format!("{}{}{}", prefix, matched, suffix),
            None => dst.to_owned(),
        })
    }

    /// Maps a destination name back to its source (the reverse of
    /// [`Refspec::map_to_destination`]).
    pub fn map_to_source(&self, name: &str) -> Option<String> {
        let dst = self.dst.as_deref().filter(|d| !d.is_empty())?;
        let matched = Self::match_pattern(dst, name)?;
        Some(match self.src.split_once('*') {
            Some((prefix, suffix)) => format!("{}{}{}", prefix, matched, suffix),
            None => self.src.clone(),
        })
    }
}

impl std::fmt::Display for Refspec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative {
            write!(f, "^")?;
        }
        if self.force {
            write!(f, "+")?;
        }
        write!(f, "{}", self.src)?;
        if let Some(dst) = &self.dst {
            write!(f, ":{}", dst)?;
        }
        Ok(())
    }
}

/// A configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    name: String,
    url: Option<String>,
    push_url: Option<String>,
    fetch: Vec<Refspec>,
    push: Vec<Refspec>,
}

impl Remote {
    /// The remote's name, such as `origin`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The URL fetched from (`remote.<name>.url`).
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// The URL pushed to: `remote.<name>.pushurl`, or the URL.
    pub fn push_url(&self) -> Option<&str> {
        self.push_url.as_deref().or(self.url.as_deref())
    }

    /// The fetch refspecs (`remote.<name>.fetch`).
    pub fn fetch_refspecs(&self) -> &[Refspec] {
        &self.fetch
    }

    /// The push refspecs (`remote.<name>.push`).
    pub fn push_refspecs(&self) -> &[Refspec] {
        &self.push
    }

    /// The remote-tracking reference for a reference on the remote, from the
    /// fetch refspecs: `refs/heads/main` maps to `refs/remotes/origin/main`
    /// with the default refspec.
    pub fn tracking_ref(&self, remote_ref: &str) -> Option<String> {
        let excluded = self
            .fetch
            .iter()
            .any(|r| r.is_negative() && r.matches_source(remote_ref));
        if excluded {
            return None;
        }
        self.fetch
            .iter()
            .filter(|r| !r.is_negative())
            .find_map(|r| r.map_to_destination(remote_ref))
    }
}

/// Validates a remote name the way Git does: it must be usable in
/// `refs/remotes/<name>/...`.
fn validate_remote_name(name: &str) -> Result<()> {
    let invalid = || Error::InvalidRefName(format!("invalid remote name: {}", name));
    if name.is_empty()
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name
            .chars()
            .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
        || name
            .split('/')
            .any(|c| c.starts_with('.') || c.ends_with(".lock"))
    {
        return Err(invalid());
    }
    Ok(())
}

impl Repository {
    fn edit_config(&self, f: impl FnOnce(&mut ConfigFile) -> Result<()>) -> Result<()> {
        let path = self.git_dir().join("config");
        let mut file = ConfigFile::open(&path)?;
        f(&mut file)?;
        file.save(&path)
    }

    /// Lists the configured remotes, sorted by name.
    pub fn remotes(&self) -> Result<Vec<Remote>> {
        let config = self.config()?;
        let mut names: Vec<String> = config
            .subsections("remote")
            .into_iter()
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        names.iter().map(|name| self.remote(name)).collect()
    }

    /// Returns a configured remote.
    ///
    /// # Errors
    ///
    /// `Error::RemoteNotFound` if no `remote.<name>` section exists.
    pub fn remote(&self, name: &str) -> Result<Remote> {
        let config = self.config()?;
        if !config.subsections("remote").contains(&name) {
            return Err(Error::RemoteNotFound(name.to_owned()));
        }
        let parse_all = |key: &str| -> Result<Vec<Refspec>> {
            config
                .get_all("remote", name, key)
                .into_iter()
                .map(Refspec::parse)
                .collect()
        };
        Ok(Remote {
            name: name.to_owned(),
            url: config
                .get_subsection("remote", name, "url")
                .map(str::to_owned),
            push_url: config
                .get_subsection("remote", name, "pushurl")
                .map(str::to_owned),
            fetch: parse_all("fetch")?,
            push: parse_all("push")?,
        })
    }

    /// Adds a remote with the default fetch refspec
    /// (`+refs/heads/*:refs/remotes/<name>/*`), like `git remote add`.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRefName` for a name Git would reject.
    /// - `Error::RemoteAlreadyExists` if the remote exists.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<Remote> {
        validate_remote_name(name)?;
        if self.config()?.subsections("remote").contains(&name) {
            return Err(Error::RemoteAlreadyExists(name.to_owned()));
        }
        self.edit_config(|file| {
            file.set("remote", name, "url", url);
            file.add(
                "remote",
                name,
                "fetch",
                &format!("+refs/heads/*:refs/remotes/{}/*", name),
            );
            Ok(())
        })?;
        self.remote(name)
    }

    /// Changes a remote's URL (`git remote set-url`).
    pub fn set_remote_url(&self, name: &str, url: &str) -> Result<()> {
        self.remote(name)?;
        self.edit_config(|file| {
            file.set("remote", name, "url", url);
            Ok(())
        })
    }

    /// Removes a remote like `git remote remove`: its configuration, its
    /// remote-tracking references (and their reflogs), and the upstream
    /// settings of branches that track it.
    pub fn remove_remote(&self, name: &str) -> Result<()> {
        let remote = self.remote(name)?;
        let config = self.config()?;
        let tracking_branches: Vec<String> = config
            .subsections("branch")
            .into_iter()
            .filter(|b| config.get_subsection("branch", b, "remote") == Some(name))
            .map(str::to_owned)
            .collect();
        // Remote-tracking references that the fetch refspecs map to.
        let reflog = self.reflog_writer()?;
        for resolved in self.ref_store().resolved_refs("refs/")? {
            let tracked = remote
                .fetch_refspecs()
                .iter()
                .filter(|r| !r.is_negative())
                .any(|r| r.map_to_source(&resolved.name).is_some());
            if tracked && resolved.name.starts_with("refs/remotes/") {
                if self.ref_store().is_packed(&resolved.name)? {
                    self.remove_packed_ref(&resolved.name)?;
                }
                let path = self.git_dir().join(&resolved.name);
                if path.is_file() {
                    std::fs::remove_file(&path)?;
                }
                reflog.delete(&resolved.name)?;
            }
        }
        let remotes_dir = self.git_dir().join("refs").join("remotes").join(name);
        if remotes_dir.is_dir() {
            remove_empty_dirs(&remotes_dir)?;
        }
        self.edit_config(|file| {
            file.remove_section("remote", name);
            for branch in &tracking_branches {
                file.unset_all("branch", branch, "remote");
                file.unset_all("branch", branch, "merge");
            }
            Ok(())
        })
    }

    /// Returns the upstream of a local branch: the remote (or `.` for a
    /// local upstream) and the reference on it, from `branch.<name>.remote`
    /// and `branch.<name>.merge`.
    pub fn branch_upstream(&self, branch: &str) -> Result<Option<(String, String)>> {
        let config = self.config()?;
        Ok(
            match (
                config.get_subsection("branch", branch, "remote"),
                config.get_subsection("branch", branch, "merge"),
            ) {
                (Some(remote), Some(merge)) => Some((remote.to_owned(), merge.to_owned())),
                _ => None,
            },
        )
    }

    /// Sets (or with `None`, removes) the upstream of a local branch, like
    /// `git branch --set-upstream-to` / `--unset-upstream`. `merge` is the
    /// reference on the remote, such as `refs/heads/main`.
    pub fn set_branch_upstream(&self, branch: &str, upstream: Option<(&str, &str)>) -> Result<()> {
        self.edit_config(|file| {
            match upstream {
                Some((remote, merge)) => {
                    file.set("branch", branch, "remote", remote);
                    file.set("branch", branch, "merge", merge);
                }
                None => {
                    file.unset_all("branch", branch, "remote");
                    file.unset_all("branch", branch, "merge");
                }
            }
            Ok(())
        })
    }
}

/// Removes empty directories below and including `dir`.
fn remove_empty_dirs(dir: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_dirs(&entry.path())?;
        }
    }
    if std::fs::read_dir(dir)?.next().is_none() {
        std::fs::remove_dir(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refspec_parse_and_map() {
        let spec = Refspec::parse("+refs/heads/*:refs/remotes/origin/*").unwrap();
        assert!(spec.is_force() && spec.is_pattern());
        assert_eq!(
            spec.map_to_destination("refs/heads/feature/x").as_deref(),
            Some("refs/remotes/origin/feature/x")
        );
        assert_eq!(spec.map_to_destination("refs/tags/v1"), None);
        assert_eq!(
            spec.map_to_source("refs/remotes/origin/main").as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(spec.to_string(), "+refs/heads/*:refs/remotes/origin/*");

        let exact = Refspec::parse("refs/heads/main:refs/heads/main").unwrap();
        assert!(!exact.is_force() && !exact.is_pattern());
        assert_eq!(
            exact.map_to_destination("refs/heads/main").as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(exact.map_to_destination("refs/heads/other"), None);

        let delete = Refspec::parse(":refs/heads/gone").unwrap();
        assert_eq!(delete.source(), "");
        assert_eq!(delete.destination(), Some("refs/heads/gone"));
        assert!(Refspec::parse("^refs/heads/skip").unwrap().is_negative());

        assert!(Refspec::parse("refs/heads/*:refs/remotes/origin/x").is_err());
        assert!(Refspec::parse("refs/*/*:refs/x/*").is_err());
        assert!(Refspec::parse("^a:b").is_err());

        let mid = Refspec::parse("refs/heads/*/done:refs/done/*").unwrap();
        assert_eq!(
            mid.map_to_destination("refs/heads/a/b/done").as_deref(),
            Some("refs/done/a/b")
        );
    }
}
