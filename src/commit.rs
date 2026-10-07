//! Creating commits: author and committer identities, dates and the
//! message clean-up `git commit -m` performs.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::index::cache_tree::{subtree_order, CacheTree};
use crate::index::{self, Index, IndexEntry};
use crate::infra::time;
use crate::objects::tree::FileMode;
use crate::objects::ObjectStore;
use crate::objects::{ObjectType, Oid, Signature};
use crate::repository::Repository;

/// Options for [`Repository::create_commit_with`].
///
/// By default the author and the committer are resolved as Git resolves
/// them (see [`Repository::default_author`] and
/// [`Repository::default_committer`]), a commit that changes nothing is
/// refused and the message is cleaned up like `git commit -m` does.
///
/// # Examples
///
/// ```no_run
/// use zerogit::{CommitOptions, Repository, Signature};
///
/// let repo = Repository::open("path/to/repo").unwrap();
/// let options = CommitOptions::new()
///     .author(Signature::new("Alice", "alice@example.com", 1_700_000_000, 540))
///     .committer(Signature::now("Bob", "bob@example.com"));
/// let oid = repo.create_commit_with("Fix the parser", &options).unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct CommitOptions {
    author: Option<Signature>,
    committer: Option<Signature>,
    allow_empty: bool,
    allow_empty_message: bool,
}

impl CommitOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the author, including the date and time zone. Without it, the
    /// author is [`Repository::default_author`].
    pub fn author(mut self, author: Signature) -> Self {
        self.author = Some(author);
        self
    }

    /// Sets the committer, including the date and time zone. Without it,
    /// the committer is [`Repository::default_committer`].
    pub fn committer(mut self, committer: Signature) -> Self {
        self.committer = Some(committer);
        self
    }

    /// Allows a commit with the same tree as its parent (or an empty root
    /// commit), like `git commit --allow-empty`.
    pub fn allow_empty(mut self, allow: bool) -> Self {
        self.allow_empty = allow;
        self
    }

    /// Allows a message that is empty after clean-up, like
    /// `git commit --allow-empty-message`.
    pub fn allow_empty_message(mut self, allow: bool) -> Self {
        self.allow_empty_message = allow;
        self
    }
}

/// Which identity is resolved.
#[derive(Clone, Copy)]
enum Role {
    Author,
    Committer,
}

impl Role {
    fn env(self) -> &'static str {
        match self {
            Role::Author => "AUTHOR",
            Role::Committer => "COMMITTER",
        }
    }

    fn section(self) -> &'static str {
        match self {
            Role::Author => "author",
            Role::Committer => "committer",
        }
    }
}

/// Resolves an identity as Git does: the name from `GIT_<ROLE>_NAME`,
/// `<role>.name` or `user.name`; the email from `GIT_<ROLE>_EMAIL`,
/// `<role>.email`, `user.email` or `EMAIL`; the date from `GIT_<ROLE>_DATE`
/// or the current time in the local time zone.
fn resolve_identity(
    config: &Config,
    role: Role,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Signature> {
    let var = |key: &str| env(&format!("GIT_{}_{}", role.env(), key));
    let setting = |key: &str| {
        config
            .get(role.section(), key)
            .or_else(|| config.get("user", key))
            .map(str::to_owned)
    };
    let name = var("NAME")
        .or_else(|| setting("name"))
        .ok_or_else(|| Error::ConfigNotFound("user.name".to_owned()))?;
    let email = var("EMAIL")
        .or_else(|| setting("email"))
        .or_else(|| env("EMAIL"))
        .ok_or_else(|| Error::ConfigNotFound("user.email".to_owned()))?;
    match var("DATE") {
        Some(date) => {
            let (timestamp, offset) =
                time::parse_git_date(&date).ok_or(Error::InvalidDate(date))?;
            Ok(Signature::new(name, email, timestamp, offset))
        }
        None => Ok(Signature::now(name, email)),
    }
}

/// Cleans up a message the way `git commit -m` and `git tag -m` do
/// (`--cleanup=whitespace`): trailing whitespace is removed from each line,
/// runs of blank lines are collapsed, leading and trailing blank lines are
/// dropped, and a non-empty message ends with a newline.
pub(crate) fn cleanup_message(message: &str) -> String {
    let mut out = String::new();
    let mut pending_blank = false;
    for line in message.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            pending_blank = !out.is_empty();
            continue;
        }
        if pending_blank {
            out.push('\n');
            pending_blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// When a commit counts as empty.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyCheck {
    /// Refuse only an empty index ([`Repository::create_commit`]).
    EmptyIndex,
    /// Refuse a tree equal to the parent's, as `git commit` does.
    UnchangedTree,
    /// Allow any commit (`--allow-empty`).
    Allow,
}

impl Repository {
    /// The author `git commit` would record: `GIT_AUTHOR_NAME`,
    /// `author.name` or `user.name`; `GIT_AUTHOR_EMAIL`, `author.email`,
    /// `user.email` or `EMAIL`; and `GIT_AUTHOR_DATE` or the current time
    /// in the local time zone.
    ///
    /// # Errors
    ///
    /// - `Error::ConfigNotFound` (`user.name` or `user.email`) if no name or
    ///   email is set.
    /// - `Error::InvalidDate` if `GIT_AUTHOR_DATE` is not a date Git accepts.
    pub fn default_author(&self) -> Result<Signature> {
        resolve_identity(&self.config()?, Role::Author, &|key| {
            std::env::var(key).ok()
        })
    }

    /// The committer `git commit` would record, resolved like
    /// [`Repository::default_author`] from `GIT_COMMITTER_*` and
    /// `committer.*` instead.
    ///
    /// # Errors
    ///
    /// The same as [`Repository::default_author`].
    pub fn default_committer(&self) -> Result<Signature> {
        resolve_identity(&self.config()?, Role::Committer, &|key| {
            std::env::var(key).ok()
        })
    }

    /// Creates a commit from the index, like `git commit -m`, with the
    /// author, committer and checks set in `options`.
    ///
    /// The message is cleaned up as `git commit -m` does: trailing
    /// whitespace and surrounding blank lines are removed, runs of blank
    /// lines are collapsed and the message ends with a newline. With the
    /// same tree, parents, signatures and message, the commit has the same
    /// OID as one made by Git.
    ///
    /// While a merge is in progress, `MERGE_HEAD` becomes the second parent
    /// and the merge state is removed, as in [`Repository::create_commit`].
    /// A commit made while a cherry-pick or revert is stopped concludes the
    /// stopped commit, as `git commit` does: a cherry-picked commit keeps
    /// its author unless [`CommitOptions::author`] is set, and the
    /// operation goes on with [`Repository::cherry_pick_continue`] or
    /// [`Repository::revert_continue`].
    ///
    /// # Errors
    ///
    /// - `Error::EmptyCommit` if the tree is the same as HEAD's (and no
    ///   merge is being concluded), unless [`CommitOptions::allow_empty`].
    /// - `Error::EmptyCommitMessage` if the message is empty after clean-up,
    ///   unless [`CommitOptions::allow_empty_message`].
    /// - `Error::UnmergedPaths` if the index has unresolved conflicts.
    /// - `Error::ConfigNotFound` or `Error::InvalidDate` if an identity is
    ///   not given and cannot be resolved.
    pub fn create_commit_with(&self, message: &str, options: &CommitOptions) -> Result<Oid> {
        let author = match (&options.author, self.cherry_pick_head()?) {
            (Some(author), _) => author.clone(),
            // Concluding a stopped cherry-pick keeps the picked author.
            (None, Some(picked)) => self.commit(&picked.to_hex())?.author().clone(),
            (None, None) => self.default_author()?,
        };
        let committer = match &options.committer {
            Some(committer) => committer.clone(),
            None => self.default_committer()?,
        };
        let message = cleanup_message(message);
        if message.is_empty() && !options.allow_empty_message {
            return Err(Error::EmptyCommitMessage);
        }
        let check = if options.allow_empty {
            EmptyCheck::Allow
        } else {
            EmptyCheck::UnchangedTree
        };
        self.commit_index(&message, &author, &committer, check, false)
    }

    /// Replaces the commit HEAD points to with a new one made from the
    /// index, like `git commit --amend`, and returns the new commit.
    ///
    /// The new commit has the parents of the replaced one. Its author is
    /// the replaced commit's (unless [`CommitOptions::author`] is set) and
    /// its committer is [`CommitOptions::committer`] or
    /// [`Repository::default_committer`], with the current time. With
    /// `message`, the message is cleaned up as in
    /// [`Repository::create_commit_with`]; without it, the replaced
    /// commit's message is kept as it is (`--no-edit`). The reflogs record
    /// `commit (amend): <subject>`.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned.
    ///
    /// - `Error::RefNotFound` if HEAD has no commit to amend.
    /// - `Error::MergeInProgress` / `Error::CherryPickInProgress` while a
    ///   merge is in progress or a cherry-pick is stopped, which Git
    ///   refuses to amend.
    /// - `Error::EmptyCommit` if the new commit would have the same tree as
    ///   its parent, unless [`CommitOptions::allow_empty`] (a merge commit
    ///   can always be amended).
    /// - `Error::EmptyCommitMessage`, `Error::UnmergedPaths`,
    ///   `Error::ConfigNotFound`, `Error::InvalidDate`: as in
    ///   [`Repository::create_commit_with`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{CommitOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.add("forgotten.txt").unwrap();
    /// // Add the file to the last commit, keeping its message.
    /// repo.amend_commit(None, &CommitOptions::new()).unwrap();
    /// ```
    pub fn amend_commit(&self, message: Option<&str>, options: &CommitOptions) -> Result<Oid> {
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let author = match &options.author {
            Some(author) => author.clone(),
            None => self.commit(&head.to_hex())?.author().clone(),
        };
        let committer = match &options.committer {
            Some(committer) => committer.clone(),
            None => self.default_committer()?,
        };
        let message = match message {
            Some(message) => {
                let message = cleanup_message(message);
                if message.is_empty() && !options.allow_empty_message {
                    return Err(Error::EmptyCommitMessage);
                }
                message
            }
            None => crate::rebase::raw_message(self, &head)?,
        };
        let check = if options.allow_empty {
            EmptyCheck::Allow
        } else {
            EmptyCheck::UnchangedTree
        };
        self.commit_index(&message, &author, &committer, check, true)
    }

    /// Writes a commit of the index with an already cleaned-up message and
    /// moves HEAD to it. With `amend`, the commit replaces the one HEAD
    /// points to, taking its parents.
    pub(crate) fn commit_index(
        &self,
        message: &str,
        author: &Signature,
        committer: &Signature,
        check: EmptyCheck,
        amend: bool,
    ) -> Result<Oid> {
        // Hold the index lock until HEAD is updated, as `git commit` does.
        let (index_lock, mut idx) = self.lock_index()?;

        // A tree built from conflict stages would contain duplicate names.
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        if check == EmptyCheck::EmptyIndex && idx.is_empty() {
            return Err(Error::EmptyCommit);
        }

        let head = self.optional_head_oid()?;
        let merge_heads = self.merge_heads()?;
        // Parents: the current HEAD (if any), then MERGE_HEAD when
        // concluding a merge; or, when amending, those of HEAD's commit.
        let cherry_picking = self.cherry_pick_head()?.is_some();
        let (parents, kind) = if amend {
            let head = head.ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
            if !merge_heads.is_empty() {
                return Err(Error::MergeInProgress);
            }
            if cherry_picking {
                return Err(Error::CherryPickInProgress);
            }
            let parents = self.commit(&head.to_hex())?.parents().to_vec();
            (parents, "commit (amend)")
        } else {
            let mut parents: Vec<Oid> = head.into_iter().collect();
            parents.extend(merge_heads.iter().copied());
            let kind = if !merge_heads.is_empty() {
                "commit (merge)"
            } else if cherry_picking {
                "commit (cherry-pick)"
            } else if head.is_some() {
                "commit"
            } else {
                "commit (initial)"
            };
            (parents, kind)
        };

        let (tree_oid, cache_tree) = self.build_cache_tree(&idx)?;
        // A commit that changes nothing is refused, except a merge.
        if check == EmptyCheck::UnchangedTree && parents.len() <= 1 {
            let unchanged = match parents.first() {
                Some(parent) => *self.commit(&parent.to_hex())?.tree() == tree_oid,
                None => idx.is_empty(),
            };
            if unchanged {
                return Err(Error::EmptyCommit);
            }
        }

        let content = Self::format_commit(
            &tree_oid,
            &parents,
            &author.to_git_string(),
            &committer.to_git_string(),
            message,
        );
        let commit_oid = self.object_store().write(ObjectType::Commit, &content)?;

        // Update HEAD, logging "commit: <subject>" as Git does.
        let subject = message.lines().next().unwrap_or("");
        let reflog_message = format!("{}: {}", kind, subject);
        self.update_head(&commit_oid, head, committer, &reflog_message)?;
        if !amend && !merge_heads.is_empty() {
            self.clear_merge_state()?;
        }
        // The commit concludes a stopped cherry-pick or revert.
        if !amend && self.conclude_pick()? {
            match std::fs::remove_file(self.git_dir().join("MERGE_MSG")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }

        // Save the trees just built as the cache tree, so the next commit
        // (by Git or zerogit) reuses them, and forget the resolved
        // conflicts, now committed, as `git commit` does.
        idx.set_cache_tree(Some(cache_tree));
        idx.clear_resolve_undo();
        index_lock.write(&idx)?;

        Ok(commit_oid)
    }
}

impl Repository {
    /// Builds a tree object from the current index.
    ///
    /// # Returns
    ///
    /// The OID of the root tree object.
    pub(crate) fn build_tree_from_index(&self, idx: &Index) -> Result<Oid> {
        Ok(self.build_cache_tree(idx)?.0)
    }

    /// Builds the tree objects of the index, reusing the trees of
    /// directories the cache tree (`TREE` extension) still has as valid, and
    /// returns the root tree with a cache tree covering every directory.
    pub(crate) fn build_cache_tree(&self, idx: &Index) -> Result<(Oid, CacheTree)> {
        let mut entries: Vec<(Vec<u8>, &IndexEntry)> = idx
            .entries()
            .iter()
            .filter(|e| e.stage() == 0)
            .map(|e| (index::path_key(e.path()), e))
            .collect();
        // Directories must be contiguous runs, even in an index read unsorted.
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let store = self.object_store();
        let (oid, tree) = build_tree_level(&store, &entries, 0, idx.cache_tree())?;
        Ok((oid, tree))
    }

    /// Builds the binary content of a tree object.
    ///
    /// Tree format: `<mode> <name>\0<20-byte-sha1>` for each entry
    fn build_tree_content(entries: &[(String, FileMode, Oid)]) -> Vec<u8> {
        let mut content = Vec::new();

        for (name, mode, oid) in entries {
            // Mode (without leading zeros for directories)
            let mode_str = match mode {
                FileMode::Directory => "40000",
                _ => mode.as_octal(),
            };
            content.extend_from_slice(mode_str.as_bytes());
            content.push(b' ');
            content.extend_from_slice(name.as_bytes());
            content.push(0);
            content.extend_from_slice(oid.as_bytes());
        }

        content
    }

    /// Formats a commit object.
    ///
    /// # Arguments
    ///
    /// * `tree_oid` - The OID of the tree object.
    /// * `parents` - The parent commits (empty for a root commit).
    /// * `author` - The author signature string.
    /// * `committer` - The committer signature string.
    /// * `message` - The commit message.
    ///
    /// # Returns
    ///
    /// The formatted commit content as bytes.
    pub(crate) fn format_commit(
        tree_oid: &Oid,
        parents: &[Oid],
        author: &str,
        committer: &str,
        message: &str,
    ) -> Vec<u8> {
        let mut content = String::new();

        // Tree line
        content.push_str(&format!("tree {}\n", tree_oid.to_hex()));

        // Parent lines (none for a root commit, two or more for a merge)
        for parent in parents {
            content.push_str(&format!("parent {}\n", parent.to_hex()));
        }

        // Author and committer
        content.push_str(&format!("author {}\n", author));
        content.push_str(&format!("committer {}\n", committer));

        // Blank line and message
        content.push('\n');
        content.push_str(message);

        content.into_bytes()
    }

    /// Creates a new commit from the staged changes.
    ///
    /// This function:
    /// 1. Builds a tree from the current index
    /// 2. Creates a commit object with the tree and parent
    /// 3. Updates HEAD to point to the new commit
    ///
    /// # Arguments
    ///
    /// * `message` - The commit message.
    /// * `author_name` - The author's name.
    /// * `author_email` - The author's email.
    ///
    /// # Returns
    ///
    /// The OID of the new commit.
    ///
    /// # Errors
    ///
    /// - `Error::EmptyCommit` if there are no staged changes.
    /// - `Error::UnmergedPaths` if the index has unresolved merge conflicts.
    ///   Nothing is written; resolve each path with [`Repository::add`] (or
    ///   [`Repository::reset`]) first.
    ///
    /// While a merge is in progress (`MERGE_HEAD` exists, for example after
    /// [`Repository::merge`] stopped on conflicts), the commit concludes it:
    /// `MERGE_HEAD` becomes the second parent and the merge state files are
    /// removed, as `git commit` does.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.add("file.txt").unwrap();
    /// let commit_oid = repo.create_commit(
    ///     "Add file.txt",
    ///     "John Doe",
    ///     "john@example.com"
    /// ).unwrap();
    /// ```
    pub fn create_commit(
        &self,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<Oid> {
        // The same person is author and committer, at the current time in
        // the local time zone, as `git commit` records it.
        let signature = Signature::now(author_name, author_email);
        self.commit_index(
            &cleanup_message(message),
            &signature,
            &signature,
            EmptyCheck::EmptyIndex,
            false,
        )
    }
}

/// Builds the tree of one directory of the index.
///
/// `entries` are the stage 0 entries under the directory, sorted by their
/// `/`-separated paths, and `prefix` is the length of the directory's path
/// with its trailing `/`. The tree recorded in `cached` is reused when it
/// is valid for exactly these entries. Intent-to-add entries are left out
/// of the tree, and a directory holding one is not cached, as in Git.
fn build_tree_level(
    store: &ObjectStore,
    entries: &[(Vec<u8>, &IndexEntry)],
    prefix: usize,
    cached: Option<&CacheTree>,
) -> Result<(Oid, CacheTree)> {
    let has_intent_to_add = entries.iter().any(|(_, e)| e.intent_to_add());
    if let Some((count, oid)) = cached.and_then(|tree| tree.valid) {
        if count == entries.len() && !has_intent_to_add && store.exists(&oid)? {
            return Ok((oid, cached.cloned().unwrap_or_default()));
        }
    }

    let mut items: Vec<(String, FileMode, Oid)> = Vec::new();
    let mut children: Vec<(Vec<u8>, CacheTree)> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let (key, entry) = &entries[i];
        let rest = &key[prefix..];
        match rest.iter().position(|&b| b == b'/') {
            Some(slash) => {
                let name = &rest[..slash];
                let dir = &key[..prefix + slash + 1];
                let end = i + entries[i..]
                    .iter()
                    .take_while(|(k, _)| k.starts_with(dir))
                    .count();
                let sub = &entries[i..end];
                let (oid, child) = build_tree_level(
                    store,
                    sub,
                    dir.len(),
                    cached.and_then(|tree| tree.child(name)),
                )?;
                // A directory of intent-to-add entries has nothing to commit.
                if sub.iter().any(|(_, e)| !e.intent_to_add()) {
                    let name = String::from_utf8_lossy(name).into_owned();
                    items.push((name, FileMode::Directory, oid));
                }
                children.push((name.to_vec(), child));
                i = end;
            }
            None => {
                if !entry.intent_to_add() {
                    let name = String::from_utf8_lossy(rest).into_owned();
                    items.push((name, entry.mode(), *entry.oid()));
                }
                i += 1;
            }
        }
    }

    // Git orders entries by name, comparing a directory as if its name
    // ended with '/'.
    items.sort_by_key(|e| tree_sort_key(&e.0, e.1));
    children.sort_by(|a, b| subtree_order(&a.0, &b.0));
    let oid = store.write(ObjectType::Tree, &Repository::build_tree_content(&items))?;
    let valid = if has_intent_to_add {
        None
    } else {
        Some((entries.len(), oid))
    };
    Ok((oid, CacheTree { valid, children }))
}

/// The key Git sorts tree entries by: the name, with `/` appended for a
/// subtree, so `foo.txt` sorts before the directory `foo`.
fn tree_sort_key(name: &str, mode: FileMode) -> Vec<u8> {
    let mut key = name.as_bytes().to_vec();
    if mode == FileMode::Directory {
        key.push(b'/');
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::from_str(text).unwrap()
    }

    fn env_of(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn identity_prefers_environment_then_role_then_user() {
        let config = config(
            "[user]\n\tname = User\n\temail = user@example.com\n[committer]\n\tname = Committer\n",
        );
        let none = env_of(&[]);
        let author = resolve_identity(&config, Role::Author, &none).unwrap();
        assert_eq!(
            (author.name(), author.email()),
            ("User", "user@example.com")
        );
        let committer = resolve_identity(&config, Role::Committer, &none).unwrap();
        assert_eq!(
            (committer.name(), committer.email()),
            ("Committer", "user@example.com")
        );

        let env = env_of(&[
            ("GIT_AUTHOR_NAME", "Env Author"),
            ("GIT_AUTHOR_EMAIL", "env@example.com"),
            ("GIT_AUTHOR_DATE", "1700000000 +0900"),
        ]);
        let author = resolve_identity(&config, Role::Author, &env).unwrap();
        assert_eq!(
            author.to_git_string(),
            "Env Author <env@example.com> 1700000000 +0900"
        );
        // The committer is not affected by the author's variables.
        let committer = resolve_identity(&config, Role::Committer, &env).unwrap();
        assert_eq!(committer.name(), "Committer");
    }

    #[test]
    fn identity_falls_back_to_email_variable_and_reports_missing_values() {
        let empty = config("");
        assert!(matches!(
            resolve_identity(&empty, Role::Author, &env_of(&[])),
            Err(Error::ConfigNotFound(key)) if key == "user.name"
        ));
        let named = config("[user]\n\tname = User\n");
        assert!(matches!(
            resolve_identity(&named, Role::Author, &env_of(&[])),
            Err(Error::ConfigNotFound(key)) if key == "user.email"
        ));
        let sig =
            resolve_identity(&named, Role::Author, &env_of(&[("EMAIL", "e@example.com")])).unwrap();
        assert_eq!(sig.email(), "e@example.com");
        assert!(matches!(
            resolve_identity(
                &named,
                Role::Author,
                &env_of(&[("EMAIL", "e@example.com"), ("GIT_AUTHOR_DATE", "soon")])
            ),
            Err(Error::InvalidDate(date)) if date == "soon"
        ));
    }

    #[test]
    fn cleanup_matches_git_commit_m() {
        assert_eq!(cleanup_message("msg"), "msg\n");
        assert_eq!(
            cleanup_message("\n\n  Subject  \n\n\n\nBody\t \n# kept\n\n"),
            "  Subject\n\nBody\n# kept\n"
        );
        assert_eq!(cleanup_message(" \n\t\n"), "");
    }
}

#[cfg(test)]
mod repository_tests {
    use super::*;
    use crate::repository::test_support::*;
    use std::fs;
    use tempfile::TempDir;

    // =========================================================================
    // create_commit tests (W-004 to W-005)
    // =========================================================================

    // W-004: create_commit with staged files creates commit and returns Oid
    #[test]
    fn test_create_commit_with_staged_files() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create refs/heads directory
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create a file and add it
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "Hello, World!").unwrap();
        repo.add("test.txt").unwrap();

        // Create commit
        let commit_oid = repo
            .create_commit("Initial commit", "Test User", "test@example.com")
            .unwrap();

        // Verify commit exists and is valid
        let commit = repo.commit(&commit_oid.to_hex()).unwrap();
        assert_eq!(commit.summary(), "Initial commit");
        assert_eq!(commit.author().name(), "Test User");
        assert_eq!(commit.author().email(), "test@example.com");
        assert!(commit.is_root()); // No parent

        // Verify HEAD is updated
        let head = repo.head().unwrap();
        assert_eq!(head.oid(), &commit_oid);

        // Verify tree contains the file
        let tree = repo.tree(&commit.tree().to_hex()).unwrap();
        assert_eq!(tree.len(), 1);
        let entry = tree.get("test.txt").unwrap();
        assert_eq!(entry.name(), "test.txt");
    }

    // W-005: create_commit with empty index returns EmptyCommit error
    #[test]
    fn test_create_commit_empty_index() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());

        let repo = Repository::open(temp.path()).unwrap();

        // Try to create commit without staging anything
        let result = repo.create_commit("Empty commit", "Test User", "test@example.com");

        assert!(matches!(result, Err(Error::EmptyCommit)));
    }

    // Additional: create_commit with subdirectories
    #[test]
    fn test_create_commit_with_subdirectories() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        // Create refs/heads directory
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create files in subdirectories
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("README.md"), "# Test").unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();

        repo.add("README.md").unwrap();
        repo.add("src/main.rs").unwrap();

        // Create commit
        let commit_oid = repo
            .create_commit("Add files", "Test User", "test@example.com")
            .unwrap();

        // Verify tree structure
        let commit = repo.commit(&commit_oid.to_hex()).unwrap();
        let tree = repo.tree(&commit.tree().to_hex()).unwrap();

        // Root tree should have README.md and src directory
        assert_eq!(tree.len(), 2);
        assert!(tree.get("README.md").is_some());

        let src_entry = tree.get("src").unwrap();
        assert!(src_entry.is_directory());

        // Verify src subtree
        let src_tree = repo.tree(&src_entry.oid().to_hex()).unwrap();
        assert_eq!(src_tree.len(), 1);
        assert!(src_tree.get("main.rs").is_some());
    }

    // Additional: create_commit chain (parent linking)
    #[test]
    fn test_create_commit_chain() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // First commit
        fs::write(temp.path().join("file1.txt"), "Content 1").unwrap();
        repo.add("file1.txt").unwrap();
        let first_oid = repo
            .create_commit("First commit", "Test User", "test@example.com")
            .unwrap();

        // Second commit
        fs::write(temp.path().join("file2.txt"), "Content 2").unwrap();
        repo.add("file2.txt").unwrap();
        let second_oid = repo
            .create_commit("Second commit", "Test User", "test@example.com")
            .unwrap();

        // Verify parent linking
        let second_commit = repo.commit(&second_oid.to_hex()).unwrap();
        assert_eq!(second_commit.parent().unwrap(), &first_oid);
        assert!(!second_commit.is_root());

        // Verify HEAD points to second commit
        let head = repo.head().unwrap();
        assert_eq!(head.oid(), &second_oid);
    }

    // Additional: create_commit updates branch ref (not just HEAD)
    #[test]
    fn test_create_commit_updates_branch() {
        let temp = TempDir::new().unwrap();
        create_git_dir(temp.path());
        let git_dir = temp.path().join(".git");

        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        let repo = Repository::open(temp.path()).unwrap();

        // Create a file and commit
        fs::write(temp.path().join("test.txt"), "Test content").unwrap();
        repo.add("test.txt").unwrap();
        let commit_oid = repo
            .create_commit("Test commit", "Test User", "test@example.com")
            .unwrap();

        // Verify branch ref was created/updated
        let branch_ref = fs::read_to_string(git_dir.join("refs/heads/main")).unwrap();
        assert_eq!(branch_ref.trim(), commit_oid.to_hex());
    }

    // Additional: build_tree_content produces valid tree
    #[test]
    fn test_build_tree_content() {
        let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let entries = vec![("file.txt".to_string(), FileMode::Regular, oid)];

        let content = Repository::build_tree_content(&entries);

        // Verify format: "100644 file.txt\0<20-byte-sha>"
        assert_eq!(&content[..7], b"100644 ");
        assert_eq!(&content[7..15], b"file.txt");
        assert_eq!(content[15], 0);
        assert_eq!(&content[16..], oid.as_bytes());
    }

    // Additional: format_commit produces valid commit
    #[test]
    fn test_format_commit() {
        let tree_oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        let parent_oid = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();

        let content = Repository::format_commit(
            &tree_oid,
            &[parent_oid],
            "Test User <test@example.com> 1234567890 +0000",
            "Test User <test@example.com> 1234567890 +0000",
            "Test message",
        );

        let content_str = String::from_utf8(content).unwrap();
        assert!(content_str.contains(&format!("tree {}", tree_oid.to_hex())));
        assert!(content_str.contains(&format!("parent {}", parent_oid.to_hex())));
        assert!(content_str.contains("author Test User <test@example.com>"));
        assert!(content_str.contains("committer Test User <test@example.com>"));
        assert!(content_str.contains("\n\nTest message"));
    }

    // Additional: format_commit without parent (root commit)
    #[test]
    fn test_format_commit_no_parent() {
        let tree_oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();

        let content = Repository::format_commit(
            &tree_oid,
            &[],
            "Test User <test@example.com> 1234567890 +0000",
            "Test User <test@example.com> 1234567890 +0000",
            "Initial commit",
        );

        let content_str = String::from_utf8(content).unwrap();
        assert!(content_str.contains(&format!("tree {}", tree_oid.to_hex())));
        assert!(!content_str.contains("parent")); // No parent line
        assert!(content_str.contains("Initial commit"));
    }
}
