//! Resolving revisions the way `git rev-parse` does (see gitrevisions(7)).
//!
//! Supported syntax:
//!
//! - Object names: full or abbreviated (at least 4 hex digits) OIDs.
//! - Reference names, tried in Git's order: `<name>` (only `refs/...` or an
//!   all-caps name such as `HEAD` or `ORIG_HEAD`), `refs/<name>`,
//!   `refs/tags/<name>`, `refs/heads/<name>`, `refs/remotes/<name>` and
//!   `refs/remotes/<name>/HEAD`. The first one that exists wins, as in Git.
//! - `@` for `HEAD`.
//! - `<ref>@{<n>}`: the n-th prior value of a reference from its reflog;
//!   `@{<n>}` uses the current branch (or `HEAD` when detached).
//! - `@{-<n>}`: the n-th branch (or commit) checked out before the current
//!   one.
//! - `<branch>@{upstream}` / `@{u}`: the upstream of a branch (the current
//!   branch when the name is left out).
//! - `<rev>~<n>`, `<rev>^<n>`: ancestors and parents (`~` and `^` mean 1).
//! - `<rev>^{}`, `<rev>^{commit}`, `^{tree}`, `^{blob}`, `^{tag}`,
//!   `^{object}`: peeling.
//! - `<rev>:<path>`: a tree entry; `<rev>:` is the tree itself.
//! - `:<path>`, `:<n>:<path>`: an index entry (stage `n`, 0 by default).
//!
//! Not supported (reported as `Error::InvalidRevision`): dates in
//! `@{...}`, `@{push}`, `^{/<text>}` and `:/<text>` message searches,
//! ranges (`a..b`) and paths relative to the current directory.

use std::path::Path;

use crate::error::{Error, Result};
use crate::objects::{ObjectType, Oid, TagObject, Tree};
use crate::refs::reflog::zero_oid;
use crate::refs::RefValue;
use crate::repository::Repository;

impl Repository {
    /// Resolves a revision to an object ID, like `git rev-parse`.
    ///
    /// Besides full and abbreviated OIDs, this accepts reference names
    /// (`main`, `v1.0`, `origin/main`, `refs/heads/main`, `HEAD`),
    /// ancestry (`HEAD~2`, `main^2`), reflogs (`main@{1}`, `@{-1}`), the
    /// upstream branch (`@{u}`), peeling (`v1.0^{commit}`, `HEAD^{tree}`)
    /// and paths (`HEAD:src/lib.rs`, `:README.md` from the index). See the
    /// [module documentation](crate::revision) for the full list.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRevision` if the revision cannot be resolved; the
    ///   reason names the part concerned (an unknown name, a missing
    ///   parent, a path not in the tree, unsupported syntax, ...).
    /// - `Error::ObjectNotFound` or `Error::InvalidOid` (ambiguous) for an
    ///   abbreviated OID that matches no object or several, as
    ///   [`Repository::resolve_short_oid`] reports them.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let parent = repo.rev_parse("HEAD~1").unwrap();
    /// let readme = repo.rev_parse("main:README.md").unwrap();
    /// let upstream = repo.rev_parse("@{u}").unwrap();
    /// ```
    pub fn rev_parse(&self, revision: &str) -> Result<Oid> {
        Resolver {
            repo: self,
            revision,
        }
        .resolve()
    }

    /// The full name of the reference a short name denotes, using Git's
    /// lookup order (see [`crate::revision`]), or `None` if no such
    /// reference exists.
    pub(crate) fn dwim_ref(&self, name: &str) -> Result<Option<String>> {
        if name.is_empty() {
            return Ok(None);
        }
        let store = self.ref_store();
        let root_syntax = name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_');
        let mut candidates = Vec::new();
        if root_syntax || name.starts_with("refs/") {
            candidates.push(name.to_owned());
        }
        for pattern in [
            "refs/{}",
            "refs/tags/{}",
            "refs/heads/{}",
            "refs/remotes/{}",
            "refs/remotes/{}/HEAD",
        ] {
            candidates.push(pattern.replace("{}", name));
        }
        for candidate in candidates {
            match store.resolve_recursive(&candidate) {
                Ok(_) => return Ok(Some(candidate)),
                Err(Error::RefNotFound(_) | Error::InvalidRefName(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// What was checked out before the n-th last checkout (`@{-<n>}`): a
    /// branch name or a commit, from the `checkout: moving from <a> to <b>`
    /// entries of HEAD's reflog, or `None` if there were fewer checkouts.
    pub(crate) fn previous_checkout(&self, n: usize) -> Result<Option<String>> {
        let Some(skip) = n.checked_sub(1) else {
            return Ok(None);
        };
        Ok(self
            .reflog("HEAD")?
            .into_iter()
            .filter_map(|entry| {
                let rest = entry.message().strip_prefix("checkout: moving from ")?;
                let (from, _) = rest.rsplit_once(" to ")?;
                Some(from.to_owned())
            })
            .nth(skip))
    }

    /// Follows annotated tags (and a commit to its tree) until an object of
    /// type `want` is reached.
    ///
    /// # Errors
    ///
    /// `Error::TypeMismatch` if the object cannot be peeled to `want`.
    pub(crate) fn peel_to(&self, mut oid: Oid, want: ObjectType) -> Result<Oid> {
        loop {
            let raw = self.object_store().read(&oid)?;
            if raw.object_type == want {
                return Ok(oid);
            }
            match raw.object_type {
                ObjectType::Tag => oid = *TagObject::parse(raw)?.object(),
                ObjectType::Commit if want == ObjectType::Tree => {
                    oid = *crate::objects::Commit::parse(oid, raw)?.tree();
                }
                other => {
                    return Err(Error::TypeMismatch {
                        expected: want.as_str(),
                        actual: other.as_str(),
                    })
                }
            }
        }
    }
}

struct Resolver<'a> {
    repo: &'a Repository,
    revision: &'a str,
}

impl Resolver<'_> {
    fn error(&self, reason: impl Into<String>) -> Error {
        Error::InvalidRevision {
            revision: self.revision.to_owned(),
            reason: reason.into(),
        }
    }

    fn resolve(&self) -> Result<Oid> {
        let revision = self.revision;
        if revision.is_empty() {
            return Err(self.error("empty revision"));
        }
        if let Some(rest) = revision.strip_prefix(':') {
            return self.index_path(rest);
        }
        match split_path(revision) {
            Some((rev, path)) => {
                let tree = self.peel(self.expression(rev)?, ObjectType::Tree, rev)?;
                self.tree_path(tree, rev, path)
            }
            None => self.expression(revision),
        }
    }

    /// `:<path>` or `:<n>:<path>`: an index entry.
    fn index_path(&self, rest: &str) -> Result<Oid> {
        let (stage, path) = match rest.as_bytes() {
            [digit @ b'0'..=b'3', b':', ..] => (digit - b'0', &rest[2..]),
            _ => (0, rest),
        };
        if path.starts_with('/') {
            return Err(self.error("searching commit messages (':/<text>') is not supported"));
        }
        let index = self.repo.read_index()?;
        match index.get_stage(Path::new(path), stage) {
            Some(entry) => Ok(*entry.oid()),
            None if stage == 0 && index.get(Path::new(path)).is_some() => Err(self.error(format!(
                "path '{}' is in the index, but not at stage 0",
                path
            ))),
            None => Err(self.error(format!(
                "path '{}' is not in the index at stage {}",
                path, stage
            ))),
        }
    }

    /// `<rev>:<path>`: an entry of the tree of `rev`.
    fn tree_path(&self, mut oid: Oid, rev: &str, path: &str) -> Result<Oid> {
        let store = self.repo.object_store();
        let mut walked = String::new();
        for component in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if component == ".." {
                return Err(self.error(format!(
                    "'..' in path '{}' is not supported (paths are relative to the root)",
                    path
                )));
            }
            let raw = store.read(&oid)?;
            if raw.object_type != ObjectType::Tree {
                return Err(self.error(format!("'{}' is not a directory in '{}'", walked, rev)));
            }
            let tree = Tree::parse(raw)?;
            match tree.get(component) {
                Some(entry) => oid = *entry.oid(),
                None => {
                    return Err(self.error(format!("path '{}' does not exist in '{}'", path, rev)))
                }
            }
            if !walked.is_empty() {
                walked.push('/');
            }
            walked.push_str(component);
        }
        Ok(oid)
    }

    /// A revision without `:<path>`: a base name followed by `@{...}`,
    /// `~<n>`, `^<n>` and `^{...}` suffixes.
    fn expression(&self, expr: &str) -> Result<Oid> {
        let bytes = expr.as_bytes();
        let base_end = (0..bytes.len())
            .find(|&i| {
                bytes[i] == b'~'
                    || bytes[i] == b'^'
                    || (bytes[i] == b'@' && bytes.get(i + 1) == Some(&b'{'))
            })
            .unwrap_or(bytes.len());
        let base = &expr[..base_end];
        let mut pos = base_end;

        let mut oid = if expr[pos..].starts_with("@{") {
            let close = self.closing_brace(expr, pos + 1)?;
            let selector = &expr[pos + 2..close];
            pos = close + 1;
            self.at_selector(base, selector, &expr[..pos])?
        } else {
            self.base(base)?
        };

        while pos < bytes.len() {
            let done = &expr[..pos];
            match bytes[pos] {
                b'~' => {
                    let (n, end) = number(expr, pos + 1, 1);
                    pos = end;
                    for i in 0..n {
                        let parents = self.parents(oid, done)?;
                        oid = match parents.first() {
                            Some(parent) => *parent,
                            None => {
                                return Err(self.error(format!("'{}~{}' has no parent", done, i)))
                            }
                        };
                    }
                }
                b'^' if bytes.get(pos + 1) == Some(&b'{') => {
                    let close = self.closing_brace(expr, pos + 1)?;
                    let kind = &expr[pos + 2..close];
                    pos = close + 1;
                    oid = self.peel_suffix(oid, kind, done)?;
                }
                b'^' => {
                    let (n, end) = number(expr, pos + 1, 1);
                    pos = end;
                    if n == 0 {
                        oid = self.peel(oid, ObjectType::Commit, done)?;
                    } else {
                        let parents = self.parents(oid, done)?;
                        oid = *parents.get(n - 1).ok_or_else(|| {
                            self.error(format!(
                                "'{}' has {} parent(s), so '^{}' does not exist",
                                done,
                                parents.len(),
                                n
                            ))
                        })?;
                    }
                }
                _ => {
                    return Err(self.error(format!(
                        "unexpected '{}' after '{}'",
                        &expr[pos..],
                        done
                    )))
                }
            }
        }
        Ok(oid)
    }

    /// The position of the `}` closing the `{` at `open`.
    fn closing_brace(&self, expr: &str, open: usize) -> Result<usize> {
        expr[open..]
            .find('}')
            .map(|i| open + i)
            .ok_or_else(|| self.error(format!("missing '}}' after '{}'", &expr[..open])))
    }

    /// A name without suffixes: `@`, a reference or an (abbreviated) OID.
    fn base(&self, name: &str) -> Result<Oid> {
        let name = if name == "@" { "HEAD" } else { name };
        if name.is_empty() {
            return Err(self.error("a revision must start with a name"));
        }
        // A full OID is taken as such, as in Git.
        if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Oid::from_hex(name);
        }
        if let Some(full) = self.repo.dwim_ref(name)? {
            return Ok(self.repo.ref_store().resolve_recursive(&full)?.oid);
        }
        if name.bytes().all(|b| b.is_ascii_hexdigit()) {
            // Keeps the errors of abbreviated OIDs (not found, ambiguous).
            return self.repo.resolve_short_oid(name);
        }
        Err(self.error(format!("unknown revision '{}'", name)))
    }

    /// `<base>@{<selector>}`.
    fn at_selector(&self, base: &str, selector: &str, part: &str) -> Result<Oid> {
        let lower = selector.to_ascii_lowercase();
        if lower == "u" || lower == "upstream" {
            return self.upstream(base, part);
        }
        if lower == "push" {
            return Err(self.error(format!("'{}': '@{{push}}' is not supported", part)));
        }
        if let Some(n) = selector.strip_prefix('-') {
            let n: usize = n
                .parse()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| self.error(format!("'{}': invalid '@{{-<n>}}'", part)))?;
            if !base.is_empty() {
                return Err(self.error(format!("'{}': '@{{-<n>}}' cannot follow a name", part)));
            }
            return self.previous_checkout(n, part);
        }
        match selector.parse::<usize>() {
            Ok(n) => self.reflog_entry(base, n, part),
            Err(_) => Err(self.error(format!(
                "'{}': only '@{{<n>}}', '@{{-<n>}}' and '@{{upstream}}' are supported \
                 (not dates)",
                part
            ))),
        }
    }

    /// The full name of the reference whose reflog `<base>@{<n>}` reads.
    fn reflog_ref(&self, base: &str, part: &str) -> Result<String> {
        if base.is_empty() || base == "@" {
            // `@{n}` is the current branch's reflog, or HEAD's when detached.
            return Ok(match self.repo.ref_store().read_ref_file("HEAD")? {
                RefValue::Symbolic(target) if base.is_empty() => target,
                _ => "HEAD".to_owned(),
            });
        }
        self.repo
            .dwim_ref(base)?
            .ok_or_else(|| self.error(format!("'{}': unknown reference '{}'", part, base)))
    }

    fn reflog_entry(&self, base: &str, n: usize, part: &str) -> Result<Oid> {
        let refname = self.reflog_ref(base, part)?;
        let entries = self.repo.reflog(&refname)?;
        if let Some(entry) = entries.get(n) {
            return Ok(*entry.new_oid());
        }
        // One past the oldest entry is the value before it, if any.
        if n == entries.len() && n > 0 {
            let old = *entries[n - 1].old_oid();
            if old != zero_oid() {
                return Ok(old);
            }
        }
        Err(self.error(format!(
            "'{}': the reflog of '{}' has only {} entries",
            part,
            refname,
            entries.len()
        )))
    }

    /// `@{-<n>}`: the n-th previous checkout, from HEAD's reflog.
    fn previous_checkout(&self, n: usize, part: &str) -> Result<Oid> {
        let previous = self.repo.previous_checkout(n)?.ok_or_else(|| {
            self.error(format!("'{}': there are fewer than {} checkouts", part, n))
        })?;
        let branch = format!("refs/heads/{}", previous);
        match self.repo.ref_store().resolve_recursive(&branch) {
            Ok(resolved) => Ok(resolved.oid),
            Err(Error::RefNotFound(_) | Error::InvalidRefName(_)) => self.base(&previous),
            Err(e) => Err(e),
        }
    }

    /// `<branch>@{upstream}`: the remote-tracking branch (or local branch
    /// for remote `.`) that `branch.<name>.merge` maps to.
    fn upstream(&self, base: &str, part: &str) -> Result<Oid> {
        let branch = if base.is_empty() || base == "@" || base == "HEAD" {
            self.repo
                .ref_store()
                .current_branch()?
                .ok_or_else(|| self.error(format!("'{}': HEAD does not point to a branch", part)))?
        } else {
            let name = base.strip_prefix("refs/heads/").unwrap_or(base);
            match self
                .repo
                .ref_store()
                .resolve_recursive(&format!("refs/heads/{}", name))
            {
                Ok(_) => name.to_owned(),
                Err(Error::RefNotFound(_) | Error::InvalidRefName(_)) => {
                    return Err(self.error(format!("'{}': no such branch '{}'", part, base)))
                }
                Err(e) => return Err(e),
            }
        };
        let (remote, merge) = self.repo.branch_upstream(&branch)?.ok_or_else(|| {
            self.error(format!(
                "'{}': no upstream is configured for branch '{}'",
                part, branch
            ))
        })?;
        let tracking = if remote == "." {
            Some(merge.clone())
        } else {
            self.repo.remote(&remote)?.tracking_ref(&merge)
        };
        let tracking = tracking.ok_or_else(|| {
            self.error(format!(
                "'{}': '{}' of remote '{}' has no remote-tracking branch",
                part, merge, remote
            ))
        })?;
        match self.repo.ref_store().resolve_recursive(&tracking) {
            Ok(resolved) => Ok(resolved.oid),
            Err(Error::RefNotFound(_)) => Err(self.error(format!(
                "'{}': the upstream branch '{}' does not exist",
                part, tracking
            ))),
            Err(e) => Err(e),
        }
    }

    /// `^{<kind>}`.
    fn peel_suffix(&self, oid: Oid, kind: &str, done: &str) -> Result<Oid> {
        match kind {
            "" => self.repo.peel(&oid),
            "commit" => self.peel(oid, ObjectType::Commit, done),
            "tree" => self.peel(oid, ObjectType::Tree, done),
            "blob" => self.peel(oid, ObjectType::Blob, done),
            "tag" => self.peel(oid, ObjectType::Tag, done),
            "object" => {
                self.repo.object_store().read(&oid)?;
                Ok(oid)
            }
            _ if kind.starts_with('/') => Err(self.error(format!(
                "'{}^{{{}}}': searching commit messages is not supported",
                done, kind
            ))),
            _ => Err(self.error(format!(
                "'{}^{{{}}}': unknown object type '{}'",
                done, kind, kind
            ))),
        }
    }

    fn peel(&self, oid: Oid, want: ObjectType, done: &str) -> Result<Oid> {
        self.repo.peel_to(oid, want).map_err(|e| match e {
            Error::TypeMismatch { actual, .. } => self.error(format!(
                "'{}' is a {}, which cannot be peeled to a {}",
                done,
                actual,
                want.as_str()
            )),
            e => e,
        })
    }

    fn parents(&self, oid: Oid, done: &str) -> Result<Vec<Oid>> {
        let commit = self.peel(oid, ObjectType::Commit, done)?;
        Ok(self.repo.commit(&commit.to_hex())?.parents().to_vec())
    }
}

/// Splits `<rev>:<path>` at the first `:` outside `{...}`.
fn split_path(revision: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, c) in revision.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return Some((&revision[..i], &revision[i + 1..])),
            _ => {}
        }
    }
    None
}

/// The decimal number starting at `start` (or `default` when there is
/// none) and the position after it.
fn number(expr: &str, start: usize, default: usize) -> (usize, usize) {
    let digits = expr[start..].bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return (default, start);
    }
    let n = expr[start..start + digits].parse().unwrap_or(usize::MAX);
    (n, start + digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_split_outside_braces() {
        assert_eq!(split_path("HEAD:src/lib.rs"), Some(("HEAD", "src/lib.rs")));
        assert_eq!(split_path("HEAD:"), Some(("HEAD", "")));
        assert_eq!(split_path("main@{1}:a:b"), Some(("main@{1}", "a:b")));
        assert_eq!(split_path("main~2"), None);
    }

    #[test]
    fn numbers_default_when_absent() {
        assert_eq!(number("~", 1, 1), (1, 1));
        assert_eq!(number("~12^", 1, 1), (12, 3));
        assert_eq!(number("^0", 1, 1), (0, 2));
    }
}
