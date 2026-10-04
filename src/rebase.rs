//! Rebasing: replaying commits onto another base (`git rebase`).
//!
//! The rebase runs like Git's default (merge-based, non-interactive) rebase:
//! the commits of the branch that are not in the upstream are picked one by
//! one onto the new base with a three-way merge, keeping their author and
//! message. Merge commits are left out, commits whose change is already in
//! the upstream are skipped, and commits that become empty are dropped.
//!
//! Progress is stored in `.git/rebase-merge/` in Git's format, so a rebase
//! stopped on conflicts can be continued, skipped or aborted with either
//! zerogit or `git rebase --continue` / `--skip` / `--abort`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{write_locked, LockFile};
use crate::merge::file::Labels;
use crate::merge::tree::{merge_trees, Flat};
use crate::merge::{index_flat, two_way};
use crate::objects::{ObjectType, Oid, Signature};
use crate::refs::reflog::zero_oid;
use crate::repository::Repository;
use crate::status::{ChangeState, DetailedStatus};

/// The result of starting or continuing a rebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// The branch already contains the new base; nothing changed.
    UpToDate,
    /// All commits were replayed; the branch (or detached HEAD) now points
    /// to this commit.
    Completed(Oid),
    /// Replaying `commit` conflicted at `paths`. Resolve them (for example
    /// with [`Repository::add`]) and call [`Repository::rebase_continue`],
    /// or use [`Repository::rebase_skip`] / [`Repository::rebase_abort`].
    Conflicts {
        /// The original commit being replayed.
        commit: Oid,
        /// The conflicted paths.
        paths: Vec<PathBuf>,
    },
}

/// The progress of a rebase in `.git/rebase-merge/`.
struct State {
    dir: PathBuf,
    /// `refs/heads/<branch>`, or `None` for a detached HEAD.
    head_name: Option<String>,
    onto: Oid,
    orig_head: Oid,
    /// The commits still to pick.
    todo: VecDeque<Oid>,
    /// The commits already picked (or being picked).
    done: Vec<Oid>,
    /// Commits picked so far: original and rewritten.
    rewritten: Vec<(Oid, Oid)>,
    /// The commit a pick stopped at, if it stopped.
    stopped: Option<Oid>,
}

fn read_trimmed(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content.trim().to_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Parses todo lines, accepting only `pick` (and `p`).
fn parse_todo(content: &str) -> Result<Vec<Oid>> {
    let mut commits = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let command = words.next().unwrap_or("");
        if command != "pick" && command != "p" {
            return Err(Error::UnsupportedRebase(format!(
                "todo command '{}'",
                command
            )));
        }
        let oid = words
            .next()
            .ok_or_else(|| Error::UnsupportedRebase(format!("todo line '{}'", line)))?;
        commits.push(Oid::from_hex(oid).map_err(|_| {
            Error::UnsupportedRebase(format!("abbreviated commit in todo line '{}'", line))
        })?);
    }
    Ok(commits)
}

/// Quotes a value for `author-script` (shell single quotes).
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl State {
    fn load(repo: &Repository) -> Result<Option<State>> {
        let dir = repo.git_dir().join("rebase-merge");
        if !dir.is_dir() {
            return Ok(None);
        }
        let read = |name: &str| -> Result<String> {
            read_trimmed(&dir.join(name))?.ok_or_else(|| {
                Error::UnsupportedRebase(format!("rebase-merge/{} is missing", name))
            })
        };
        let head_name = read("head-name")?;
        let onto = Oid::from_hex(&read("onto")?)?;
        let orig_head = Oid::from_hex(&read("orig-head")?)?;
        let todo = parse_todo(&read_trimmed(&dir.join("git-rebase-todo"))?.unwrap_or_default())?;
        let done = parse_todo(&read_trimmed(&dir.join("done"))?.unwrap_or_default())?;
        let mut rewritten = Vec::new();
        for line in read_trimmed(&dir.join("rewritten-list"))?
            .unwrap_or_default()
            .lines()
        {
            let mut parts = line.split_whitespace();
            if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                rewritten.push((Oid::from_hex(a)?, Oid::from_hex(b)?));
            }
        }
        let stopped = match read_trimmed(&dir.join("stopped-sha"))? {
            Some(hex) if !hex.is_empty() => Some(repo.resolve_short_oid(&hex)?),
            _ => None,
        };
        Ok(Some(State {
            dir,
            head_name: if head_name == "detached HEAD" {
                None
            } else {
                Some(head_name)
            },
            onto,
            orig_head,
            todo: todo.into(),
            done,
            rewritten,
            stopped,
        }))
    }

    /// A todo line in the format of current Git (the subject after `#`;
    /// older Git, which writes it without, reads it too).
    fn todo_line(repo: &Repository, oid: &Oid) -> Result<String> {
        Ok(format!(
            "pick {} # {}\n",
            oid.to_hex(),
            repo.commit(&oid.to_hex())?.subject()
        ))
    }

    /// Writes the progress files Git reads.
    fn save(&self, repo: &Repository) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let write =
            |name: &str, content: &str| write_locked(self.dir.join(name), content.as_bytes());
        write(
            "head-name",
            &format!("{}\n", self.head_name.as_deref().unwrap_or("detached HEAD")),
        )?;
        write("onto", &format!("{}\n", self.onto.to_hex()))?;
        write("orig-head", &format!("{}\n", self.orig_head.to_hex()))?;
        write("interactive", "")?;
        write("drop_redundant_commits", "")?;
        let mut todo = String::new();
        for oid in &self.todo {
            todo.push_str(&Self::todo_line(repo, oid)?);
        }
        write("git-rebase-todo", &todo)?;
        let mut done = String::new();
        for oid in &self.done {
            done.push_str(&Self::todo_line(repo, oid)?);
        }
        write("done", &done)?;
        write("msgnum", &format!("{}\n", self.done.len()))?;
        write("end", &format!("{}\n", self.done.len() + self.todo.len()))?;
        let mut rewritten = String::new();
        for (old, new) in &self.rewritten {
            rewritten.push_str(&format!("{} {}\n", old.to_hex(), new.to_hex()));
        }
        write("rewritten-list", &rewritten)?;
        Ok(())
    }

    /// Removes the files describing a stopped pick.
    fn clear_stop(&mut self, repo: &Repository) -> Result<()> {
        self.stopped = None;
        for path in [
            self.dir.join("stopped-sha"),
            self.dir.join("author-script"),
            self.dir.join("message"),
            self.dir.join("patch"),
            repo.git_dir().join("REBASE_HEAD"),
            repo.git_dir().join("MERGE_MSG"),
            repo.git_dir().join("AUTO_MERGE"),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

/// The raw message of a commit (everything after the headers).
fn raw_message(repo: &Repository, oid: &Oid) -> Result<String> {
    let raw = repo.object_store().read(oid)?;
    let content = String::from_utf8_lossy(&raw.content).into_owned();
    Ok(match content.find("\n\n") {
        Some(pos) => content[pos + 2..].to_owned(),
        None => String::new(),
    })
}

impl Repository {
    /// Returns whether a rebase is in progress (`.git/rebase-merge/` or
    /// `.git/rebase-apply/` exists).
    pub fn is_rebasing(&self) -> bool {
        self.git_dir().join("rebase-merge").is_dir() || self.git_dir().join("rebase-apply").is_dir()
    }

    /// A short identifier for a commit, as Git shows it (7 hex digits).
    fn short(oid: &Oid) -> String {
        oid.to_hex()[..7].to_owned()
    }

    /// Resolves a commit given as a branch, tag, remote-tracking branch,
    /// full reference name or (abbreviated) commit ID.
    fn resolve_commit(&self, name: &str) -> Result<Oid> {
        let store = self.ref_store();
        for candidate in [
            format!("refs/heads/{}", name),
            format!("refs/tags/{}", name),
            format!("refs/remotes/{}", name),
            name.to_owned(),
        ] {
            if !candidate.starts_with("refs/") && candidate != "HEAD" {
                continue;
            }
            match store.resolve_recursive(&candidate) {
                Ok(resolved) => return self.peel_to_commit(resolved.oid),
                Err(Error::RefNotFound(_)) | Err(Error::InvalidRefName(_)) => {}
                Err(e) => return Err(e),
            }
        }
        match self.resolve_short_oid(name) {
            Ok(oid) => self.peel_to_commit(oid),
            Err(Error::ObjectNotFound(_) | Error::InvalidOid(_)) => {
                Err(Error::RefNotFound(name.to_owned()))
            }
            Err(e) => Err(e),
        }
    }

    fn peel_to_commit(&self, mut oid: Oid) -> Result<Oid> {
        loop {
            let raw = self.object_store().read(&oid)?;
            match raw.object_type {
                ObjectType::Commit => return Ok(oid),
                ObjectType::Tag => oid = *crate::objects::TagObject::parse(raw)?.object(),
                other => {
                    return Err(Error::TypeMismatch {
                        expected: "commit",
                        actual: other.as_str(),
                    })
                }
            }
        }
    }

    /// A patch ID for a non-merge commit: the same for commits making the
    /// same change, wherever they are applied.
    fn patch_id(&self, oid: &Oid) -> Result<Vec<u8>> {
        let commit = self.commit(&oid.to_hex())?;
        let new = self.flat_tree(Some(oid))?;
        let old = self.flat_tree(commit.parents().first())?;
        let mut text = Vec::new();
        let store = self.object_store();
        for path in old
            .keys()
            .chain(new.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let (a, b) = (old.get(path), new.get(path));
            if a == b {
                continue;
            }
            text.extend_from_slice(format!("\0{}\0", path).as_bytes());
            let read = |entry: Option<&(Oid, crate::FileMode)>| -> Result<Vec<u8>> {
                Ok(match entry {
                    Some((oid, _)) => store.read(oid)?.content,
                    None => Vec::new(),
                })
            };
            let (a_content, b_content) = (read(a)?, read(b)?);
            let a_lines = crate::diff::histogram::split_lines(&a_content);
            let b_lines = crate::diff::histogram::split_lines(&b_content);
            for hunk in crate::diff::histogram::diff_lines(&a_lines, &b_lines) {
                for line in &a_lines[hunk.old_start..hunk.old_end()] {
                    text.push(b'-');
                    text.extend_from_slice(line);
                }
                for line in &b_lines[hunk.new_start..hunk.new_end()] {
                    text.push(b'+');
                    text.extend_from_slice(line);
                }
            }
            text.extend_from_slice(
                format!("\0{:?}{:?}", a.map(|e| e.1), b.map(|e| e.1)).as_bytes(),
            );
        }
        Ok(crate::infra::hash::sha1(&text).to_vec())
    }

    /// The non-merge commits reachable from `head` but not from `upstream`,
    /// parents before children, without those whose change the upstream
    /// side already has.
    fn commits_to_replay(&self, upstream: &Oid, head: &Oid) -> Result<Vec<Oid>> {
        let mut cache: HashMap<Oid, Vec<Oid>> = HashMap::new();
        let mut parents_of = |oid: &Oid| -> Result<Vec<Oid>> {
            if let Some(p) = cache.get(oid) {
                return Ok(p.clone());
            }
            let p = self.commit(&oid.to_hex())?.parents().to_vec();
            cache.insert(*oid, p.clone());
            Ok(p)
        };
        let reachable = |start: &Oid,
                         parents_of: &mut dyn FnMut(&Oid) -> Result<Vec<Oid>>|
         -> Result<HashSet<Oid>> {
            let mut seen = HashSet::new();
            let mut queue = VecDeque::from([*start]);
            while let Some(oid) = queue.pop_front() {
                if seen.insert(oid) {
                    queue.extend(parents_of(&oid)?);
                }
            }
            Ok(seen)
        };
        let from_upstream = reachable(upstream, &mut parents_of)?;
        let from_head = reachable(head, &mut parents_of)?;

        // Ours: in head only. Order parents before children (a reverse
        // depth-first post-order from head, following first parents first).
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![(*head, false)];
        while let Some((oid, expanded)) = stack.pop() {
            if from_upstream.contains(&oid) {
                continue;
            }
            if expanded {
                order.push(oid);
                continue;
            }
            if !visited.insert(oid) {
                continue;
            }
            stack.push((oid, true));
            for parent in parents_of(&oid)?.iter().rev() {
                if !visited.contains(parent) {
                    stack.push((*parent, false));
                }
            }
        }
        let mut ours = Vec::new();
        for oid in order {
            if parents_of(&oid)?.len() <= 1 {
                ours.push(oid);
            }
        }

        // Changes already upstream: patch IDs of upstream-only commits.
        let mut theirs = HashSet::new();
        for oid in from_upstream.difference(&from_head) {
            if parents_of(oid)?.len() <= 1 {
                theirs.insert(self.patch_id(oid)?);
            }
        }
        let mut result = Vec::new();
        for oid in ours {
            if theirs.is_empty() || !theirs.contains(&self.patch_id(&oid)?) {
                result.push(oid);
            }
        }
        Ok(result)
    }

    /// Fails unless the index and the tracked files match HEAD.
    fn require_clean(&self) -> Result<()> {
        let dirty = self
            .detailed_status()?
            .into_iter()
            .any(|e| match e.status() {
                DetailedStatus::Changed { index, worktree } => {
                    index != ChangeState::Unmodified || worktree != ChangeState::Unmodified
                }
                DetailedStatus::Unmerged(_) => true,
                DetailedStatus::Untracked => false,
            });
        if dirty {
            Err(Error::DirtyWorkingTree)
        } else {
            Ok(())
        }
    }

    /// Moves the index and work tree from HEAD's tree to `target`'s and
    /// detaches HEAD there.
    fn detach_to(&self, target: &Oid, who: &Signature, message: &str) -> Result<()> {
        let head = self.optional_head_oid()?;
        let from = self.flat_tree(head.as_ref())?;
        let to = self.flat_tree(Some(target))?;
        let (index_lock, mut idx) = self.lock_index()?;
        let mut head_lock = LockFile::acquire(self.git_dir().join("HEAD"))?;
        let mut worktree = self.worktree()?;
        let result = two_way(&from, &to);
        self.check_overwrites(&idx, &from, &result, &mut worktree)?;
        self.apply_merge(&mut idx, &mut worktree, &from, &result)?;
        index_lock.write(&idx)?;
        head_lock.write_all(format!("{}\n", target.to_hex()).as_bytes())?;
        self.reflog_writer()?.append(
            "HEAD",
            &head.unwrap_or_else(zero_oid),
            target,
            who,
            message,
        )?;
        head_lock.commit()
    }

    /// Replays the branch onto a new base, like `git rebase <upstream>` or
    /// `git rebase --onto <onto> <upstream>`.
    ///
    /// The commits reachable from HEAD but not from `upstream` are picked,
    /// oldest first, onto `onto` (by default `upstream`). Each keeps its
    /// author and message; the committer is `committer_name` /
    /// `committer_email` with the current time. Merge commits are left out,
    /// commits whose change `upstream` already has are skipped, and commits
    /// that become empty are dropped. When done, the branch points to the
    /// last picked commit and `ORIG_HEAD` to where it was.
    ///
    /// Picks stop on conflicts, recorded as in a merge (`HEAD` and
    /// `<commit> (<subject>)` markers, index stages); continue with
    /// [`Repository::rebase_continue`] (or `git rebase --continue`).
    ///
    /// # Errors
    ///
    /// Nothing is changed when any of these is returned:
    /// - `Error::RebaseInProgress` / `Error::MergeInProgress` while a rebase
    ///   or merge is in progress.
    /// - `Error::DirtyWorkingTree` if the index or tracked files have
    ///   changes.
    /// - `Error::RefNotFound` if `upstream` or `onto` cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{RebaseOutcome, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// match repo.rebase("main", None, "John Doe", "john@example.com").unwrap() {
    ///     RebaseOutcome::Conflicts { paths, .. } => println!("resolve {:?}", paths),
    ///     outcome => println!("{:?}", outcome),
    /// }
    /// ```
    pub fn rebase(
        &self,
        upstream: &str,
        onto: Option<&str>,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<RebaseOutcome> {
        if self.is_rebasing() {
            return Err(Error::RebaseInProgress);
        }
        if !self.merge_heads()?.is_empty() {
            return Err(Error::MergeInProgress);
        }
        self.require_clean()?;
        let upstream_oid = self.resolve_commit(upstream)?;
        let onto_oid = match onto {
            Some(name) => self.resolve_commit(name)?,
            None => upstream_oid,
        };
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let head_name = self
            .ref_store()
            .current_branch()?
            .map(|b| format!("refs/heads/{}", b));

        // Already based on onto with nothing to drop: up to date.
        if self.merge_base(&onto_oid, &head)? == Some(onto_oid)
            && self.merge_base(&upstream_oid, &head)? == Some(onto_oid)
        {
            return Ok(RebaseOutcome::UpToDate);
        }

        let commits = self.commits_to_replay(&upstream_oid, &head)?;
        let who = Signature::new(committer_name, committer_email, crate::repository::now(), 0);
        self.write_orig_head(&head)?;
        let mut state = State {
            dir: self.git_dir().join("rebase-merge"),
            head_name,
            onto: onto_oid,
            orig_head: head,
            todo: commits.into(),
            done: Vec::new(),
            rewritten: Vec::new(),
            stopped: None,
        };
        self.detach_to(
            &onto_oid,
            &who,
            &format!("rebase (start): checkout {}", onto.unwrap_or(upstream)),
        )?;
        state.save(self)?;
        self.run_rebase(&mut state, &who)
    }

    /// Picks the remaining commits; stops at a conflict or finishes.
    fn run_rebase(&self, state: &mut State, who: &Signature) -> Result<RebaseOutcome> {
        let style = self.conflict_style()?;
        while let Some(commit_oid) = state.todo.pop_front() {
            state.done.push(commit_oid);
            state.save(self)?;
            let commit = self.commit(&commit_oid.to_hex())?;
            let subject = commit.subject();
            let head = self
                .optional_head_oid()?
                .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;

            let base = self.flat_tree(commit.parents().first())?;
            let theirs = self.flat_tree(Some(&commit_oid))?;
            let (index_lock, mut idx) = self.lock_index()?;
            let ours = index_flat(&idx);
            let theirs_label = format!("{} ({})", Self::short(&commit_oid), subject);
            let base_label = format!("parent of {}", theirs_label);
            let labels = Labels {
                ours: "HEAD",
                base: &base_label,
                theirs: &theirs_label,
            };
            let result = merge_trees(&self.object_store(), &base, &ours, &theirs, labels, style)?;
            let mut worktree = self.worktree()?;
            self.check_overwrites(&idx, &ours, &result, &mut worktree)?;
            self.apply_merge(&mut idx, &mut worktree, &ours, &result)?;
            index_lock.write(&idx)?;

            let conflicts = result.conflicted_paths();
            if !conflicts.is_empty() {
                self.record_stop(state, &commit_oid, &conflicts)?;
                return Ok(RebaseOutcome::Conflicts {
                    commit: commit_oid,
                    paths: conflicts.into_iter().map(PathBuf::from).collect(),
                });
            }
            self.commit_pick(state, &commit_oid, &head, who, "pick")?;
        }
        self.finish_rebase(state, who)
    }

    /// Records a pick stopped on conflicts the way Git does.
    fn record_stop(&self, state: &mut State, commit_oid: &Oid, conflicts: &[String]) -> Result<()> {
        let commit = self.commit(&commit_oid.to_hex())?;
        let author = commit.author();
        let sign = if author.tz_offset() < 0 { '-' } else { '+' };
        let tz = author.tz_offset().abs();
        let script = format!(
            "GIT_AUTHOR_NAME={}\nGIT_AUTHOR_EMAIL={}\nGIT_AUTHOR_DATE={}\n",
            shell_quote(author.name()),
            shell_quote(author.email()),
            shell_quote(&format!(
                "@{} {}{:02}{:02}",
                author.timestamp(),
                sign,
                tz / 60,
                tz % 60
            ))
        );
        // The message, an empty line and the conflicted paths as comments,
        // which committing strips.
        let mut message = raw_message(self, commit_oid)?;
        message.push_str("\n# Conflicts:\n");
        for path in conflicts {
            message.push_str(&format!("#\t{}\n", path));
        }
        write_locked(state.dir.join("author-script"), script.as_bytes())?;
        write_locked(state.dir.join("message"), message.as_bytes())?;
        write_locked(
            state.dir.join("stopped-sha"),
            format!("{}\n", commit_oid.to_hex()).as_bytes(),
        )?;
        write_locked(
            self.git_dir().join("REBASE_HEAD"),
            format!("{}\n", commit_oid.to_hex()).as_bytes(),
        )?;
        write_locked(self.git_dir().join("MERGE_MSG"), message.as_bytes())?;
        state.stopped = Some(*commit_oid);
        Ok(())
    }

    /// Commits the index as the replayed `commit_oid`, or drops it if it
    /// became empty.
    fn commit_pick(
        &self,
        state: &mut State,
        commit_oid: &Oid,
        head: &Oid,
        who: &Signature,
        action: &str,
    ) -> Result<()> {
        let commit = self.commit(&commit_oid.to_hex())?;
        let idx = self.read_index()?;
        let tree = self.build_tree_from_index(&idx)?;
        let head_tree = *self.commit(&head.to_hex())?.tree();
        let parent_tree = match commit.parents().first() {
            Some(parent) => Some(*self.commit(&parent.to_hex())?.tree()),
            None => None,
        };
        let originally_empty = parent_tree == Some(*commit.tree());
        if tree == head_tree && !originally_empty {
            // The change is already there: drop the commit.
            return Ok(());
        }
        let author = commit.author().to_git_string();
        let content = Self::format_commit(
            &tree,
            &[*head],
            &author,
            &who.to_git_string(),
            &raw_message(self, commit_oid)?,
        );
        let new = self.object_store().write(ObjectType::Commit, &content)?;
        self.update_head(
            &new,
            Some(*head),
            who,
            &format!("rebase ({}): {}", action, commit.subject()),
        )?;
        state.rewritten.push((*commit_oid, new));
        state.save(self)
    }

    /// Points the branch at the result, reattaches HEAD and removes the
    /// progress files.
    fn finish_rebase(&self, state: &mut State, who: &Signature) -> Result<RebaseOutcome> {
        let new = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let reflog = self.reflog_writer()?;
        if let Some(branch) = &state.head_name {
            write_locked(
                self.git_dir().join(branch),
                format!("{}\n", new.to_hex()).as_bytes(),
            )?;
            reflog.append(
                branch,
                &state.orig_head,
                &new,
                who,
                &format!("rebase (finish): {} onto {}", branch, state.onto.to_hex()),
            )?;
            write_locked(
                self.git_dir().join("HEAD"),
                format!("ref: {}\n", branch).as_bytes(),
            )?;
            reflog.append(
                "HEAD",
                &new,
                &new,
                who,
                &format!("rebase (finish): returning to {}", branch),
            )?;
        }
        state.clear_stop(self)?;
        fs::remove_dir_all(&state.dir)?;
        Ok(RebaseOutcome::Completed(new))
    }

    fn load_rebase(&self) -> Result<State> {
        State::load(self)?.ok_or(Error::NoRebaseInProgress)
    }

    /// Continues a rebase stopped on conflicts, like
    /// `git rebase --continue`: the resolved index is committed as the
    /// stopped commit (with its author and message; nothing is committed if
    /// the resolution left no change), then the remaining commits are
    /// picked.
    ///
    /// # Errors
    ///
    /// - `Error::NoRebaseInProgress` if no rebase is in progress.
    /// - `Error::UnmergedPaths` if conflicts are still unresolved.
    /// - `Error::UnsupportedRebase` for a rebase Git started with commands
    ///   other than `pick` (for example an interactive rebase).
    pub fn rebase_continue(
        &self,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<RebaseOutcome> {
        let mut state = self.load_rebase()?;
        let idx = self.read_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let who = Signature::new(committer_name, committer_email, crate::repository::now(), 0);
        if let Some(stopped) = state.stopped {
            let head = self
                .optional_head_oid()?
                .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
            self.commit_pick(&mut state, &stopped, &head, &who, "continue")?;
            state.clear_stop(self)?;
        }
        self.run_rebase(&mut state, &who)
    }

    /// Skips the commit a rebase stopped at, like `git rebase --skip`: its
    /// changes are discarded from the index and work tree, then the
    /// remaining commits are picked.
    ///
    /// # Errors
    ///
    /// `Error::NoRebaseInProgress` if no rebase is in progress.
    pub fn rebase_skip(
        &self,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<RebaseOutcome> {
        let mut state = self.load_rebase()?;
        self.reset_to_head()?;
        state.clear_stop(self)?;
        let who = Signature::new(committer_name, committer_email, crate::repository::now(), 0);
        self.run_rebase(&mut state, &who)
    }

    /// Resets the index and the tracked files to HEAD, like
    /// `git reset --hard`: staged and unstaged changes to tracked files are
    /// discarded, files HEAD does not have are removed from the index and the
    /// work tree, and untracked files are kept.
    pub fn reset_hard(&self) -> Result<()> {
        self.reset_to_head()
    }

    fn reset_to_head(&self) -> Result<()> {
        let head = self.optional_head_oid()?;
        self.reset_hard_to(&self.flat_tree(head.as_ref())?)
    }

    fn reset_hard_to(&self, target: &Flat) -> Result<()> {
        let (index_lock, mut idx) = self.lock_index()?;
        let mut worktree = self.worktree()?;
        let mut paths: Vec<String> = Vec::new();
        for entry in idx.entries() {
            let key = String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned();
            if paths.last() != Some(&key) {
                paths.push(key);
            }
        }
        for path in target.keys() {
            if idx.get(Path::new(path)).is_none() {
                paths.push(path.clone());
            }
        }
        // Only touch paths that differ from the target in the index or the
        // work tree.
        let mut changed = Vec::new();
        for path in paths {
            let native = crate::worktree::native_path(Path::new(&path));
            let entry = idx.get_stage(Path::new(&path), 0).cloned();
            let wanted = target.get(&path).copied();
            let staged = entry.as_ref().map(|e| (*e.oid(), e.mode()));
            let conflicted = idx.get(Path::new(&path)).is_some_and(|e| e.is_conflicted());
            let on_disk = worktree.hash(&native, entry.as_ref())?;
            if conflicted || staged != wanted || on_disk != wanted {
                changed.push(path);
            }
        }
        self.restore_paths(&mut idx, &mut worktree, target, &changed)?;
        index_lock.write(&idx)
    }

    /// Abandons a rebase, like `git rebase --abort`: the index, the work
    /// tree and HEAD (and its branch) return to where they were before the
    /// rebase started.
    ///
    /// # Errors
    ///
    /// `Error::NoRebaseInProgress` if no rebase is in progress.
    pub fn rebase_abort(&self) -> Result<()> {
        let mut state = self.load_rebase()?;
        let orig = self.flat_tree(Some(&state.orig_head))?;
        self.reset_hard_to(&orig)?;
        let current = self.optional_head_oid()?.unwrap_or_else(zero_oid);
        let who = self.reflog_identity()?;
        let reflog = self.reflog_writer()?;
        match &state.head_name {
            Some(branch) => {
                // The branch itself never moved; make sure it is unchanged.
                write_locked(
                    self.git_dir().join(branch),
                    format!("{}\n", state.orig_head.to_hex()).as_bytes(),
                )?;
                write_locked(
                    self.git_dir().join("HEAD"),
                    format!("ref: {}\n", branch).as_bytes(),
                )?;
                reflog.append(
                    "HEAD",
                    &current,
                    &state.orig_head,
                    &who,
                    &format!("rebase (abort): returning to {}", branch),
                )?;
            }
            None => {
                write_locked(
                    self.git_dir().join("HEAD"),
                    format!("{}\n", state.orig_head.to_hex()).as_bytes(),
                )?;
                reflog.append(
                    "HEAD",
                    &current,
                    &state.orig_head,
                    &who,
                    &format!("rebase (abort): returning to {}", state.orig_head.to_hex()),
                )?;
            }
        }
        state.clear_stop(self)?;
        fs::remove_dir_all(&state.dir)?;
        Ok(())
    }
}
