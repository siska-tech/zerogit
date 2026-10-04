//! Creating commits: author and committer identities, dates and the
//! message clean-up `git commit -m` performs.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::infra::time;
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
        let author = match &options.author {
            Some(author) => author.clone(),
            None => self.default_author()?,
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
        self.commit_index(&message, &author, &committer, check)
    }

    /// Writes a commit of the index with an already cleaned-up message and
    /// moves HEAD to it.
    pub(crate) fn commit_index(
        &self,
        message: &str,
        author: &Signature,
        committer: &Signature,
        check: EmptyCheck,
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

        let (tree_oid, cache_tree) = self.build_cache_tree(&idx)?;

        // Parents: the current HEAD (if any), then MERGE_HEAD when
        // concluding a merge.
        let parent_oid = self.optional_head_oid()?;
        let merge_heads = self.merge_heads()?;
        if check == EmptyCheck::UnchangedTree && merge_heads.is_empty() {
            let parent_tree = match &parent_oid {
                Some(parent) => Some(*self.commit(&parent.to_hex())?.tree()),
                None => None,
            };
            let unchanged = match parent_tree {
                Some(tree) => tree == tree_oid,
                None => idx.is_empty(),
            };
            if unchanged {
                return Err(Error::EmptyCommit);
            }
        }
        let mut parents: Vec<Oid> = parent_oid.into_iter().collect();
        parents.extend(merge_heads.iter().copied());

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
        let reflog_message = if !merge_heads.is_empty() {
            format!("commit (merge): {}", subject)
        } else if parent_oid.is_some() {
            format!("commit: {}", subject)
        } else {
            format!("commit (initial): {}", subject)
        };
        self.update_head(&commit_oid, parent_oid, committer, &reflog_message)?;
        if !merge_heads.is_empty() {
            self.clear_merge_state()?;
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
