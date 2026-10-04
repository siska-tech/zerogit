//! Applying and undoing commits: `git cherry-pick` and `git revert`.
//!
//! [`Repository::cherry_pick`] applies the changes of commits on top of
//! HEAD, keeping their authors and messages; [`Repository::revert`] makes
//! commits that undo them. Each commit is merged with a three-way merge, as
//! a rebase picks it (renames are followed), and committed with a new
//! committer.
//!
//! A commit that conflicts, or whose change is already in HEAD, stops the
//! operation the way Git stops it: `CHERRY_PICK_HEAD` or `REVERT_HEAD` and
//! `MERGE_MSG`, and for several commits the rest of the list in
//! `.git/sequencer/`. Either zerogit or Git (`git cherry-pick --continue`,
//! `--skip`, `--abort`, and the same for `git revert`) can then carry on.
//!
//! Merge commits (`-m <parent>`) are not supported.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use crate::commit::EmptyCheck;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::infra::write_locked;
use crate::merge::file::Labels;
use crate::merge::staged_changes;
use crate::objects::{ObjectType, Oid, Signature};
use crate::rebase::{raw_message, strip_message};
use crate::repository::Repository;

/// Options for [`Repository::cherry_pick`].
#[derive(Debug, Clone, Default)]
pub struct CherryPickOptions {
    record_origin: bool,
}

impl CherryPickOptions {
    /// The default options: the messages are kept as they are.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `(cherry picked from commit <oid>)` to each message, as
    /// `git cherry-pick -x` does (after a blank line, unless the message
    /// already ends with trailers such as `Signed-off-by:`).
    pub fn record_origin(mut self, record_origin: bool) -> Self {
        self.record_origin = record_origin;
        self
    }
}

/// The result of a cherry-pick or revert, or of continuing or skipping one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickOutcome {
    /// Every commit was applied; HEAD (and its branch) now points to this
    /// commit.
    Completed(Oid),
    /// Applying `commit` conflicted at `paths`, recorded as in a merge
    /// (index stages, conflict markers). Resolve them (for example with
    /// [`Repository::add`]) and continue, or skip or abort.
    Conflicts {
        /// The commit being picked or reverted.
        commit: Oid,
        /// The conflicted paths.
        paths: Vec<PathBuf>,
    },
    /// Applying `commit` changed nothing, because HEAD already has its
    /// change (or, for a revert, already lacks it), or it changes nothing
    /// itself. As Git does, the operation stops: skip the commit, or commit
    /// it anyway with [`Repository::create_commit_with`] and
    /// [`crate::CommitOptions::allow_empty`], then continue.
    Empty {
        /// The commit being picked or reverted.
        commit: Oid,
    },
}

/// Which operation is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    CherryPick,
    Revert,
}

impl Action {
    /// The command in `.git/sequencer/todo`.
    fn command(self) -> &'static str {
        match self {
            Action::CherryPick => "pick",
            Action::Revert => "revert",
        }
    }

    /// The file naming the commit a stopped operation was applying.
    fn head_file(self) -> &'static str {
        match self {
            Action::CherryPick => "CHERRY_PICK_HEAD",
            Action::Revert => "REVERT_HEAD",
        }
    }

    fn in_progress(self) -> Error {
        match self {
            Action::CherryPick => Error::CherryPickInProgress,
            Action::Revert => Error::RevertInProgress,
        }
    }

    fn not_in_progress(self) -> Error {
        match self {
            Action::CherryPick => Error::NoCherryPickInProgress,
            Action::Revert => Error::NoRevertInProgress,
        }
    }
}

/// The commits of an operation still to apply.
struct Sequence {
    action: Action,
    record_origin: bool,
    /// The commits still to apply; while stopped, the first is the one it
    /// stopped at (as in Git's `sequencer/todo`).
    todo: VecDeque<Oid>,
    /// Whether the progress is kept in `.git/sequencer/`: Git keeps it
    /// only for more than one commit.
    saved: bool,
}

fn read_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn remove_file(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn remove_dir(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The subject Git shows for a message: its first line that is not blank.
fn subject_line(message: &str) -> &str {
    message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Where a trailer's separator is: after a token of letters, digits and
/// `-` (optionally followed by spaces), as Git's `find_separator` finds it.
fn separator_pos(line: &str) -> Option<usize> {
    let mut whitespace_found = false;
    for (i, c) in line.char_indices() {
        if c == ':' {
            return Some(i);
        }
        if !whitespace_found && (c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        if i != 0 && (c == ' ' || c == '\t') {
            whitespace_found = true;
            continue;
        }
        break;
    }
    None
}

/// Whether the message ends with a block of trailers (`Signed-off-by:`,
/// `(cherry picked from commit ...)`, `Token: value`), as Git's trailer
/// parsing decides it for `cherry-pick -x`.
fn has_trailers(message: &str) -> bool {
    let lines: Vec<&str> = message.lines().collect();
    let is_comment = |line: &str| line.starts_with('#');
    let is_blank = |line: &str| line.trim().is_empty();
    // The first paragraph is the title and cannot hold trailers.
    let Some(title_end) = lines
        .iter()
        .position(|line| !is_comment(line) && is_blank(line))
    else {
        return false;
    };
    let (mut only_spaces, mut recognized) = (true, false);
    let (mut trailers, mut others, mut continuations) = (0usize, 0usize, 0usize);
    for line in lines[title_end..].iter().rev() {
        if is_comment(line) {
            others += continuations;
            continuations = 0;
            continue;
        }
        if is_blank(line) {
            if only_spaces {
                continue;
            }
            others += continuations;
            return (recognized && trailers * 3 >= others) || (trailers > 0 && others == 0);
        }
        only_spaces = false;
        if line.starts_with("Signed-off-by: ") || line.starts_with("(cherry picked from commit ") {
            trailers += 1;
            continuations = 0;
            recognized = true;
        } else if separator_pos(line).is_some_and(|pos| pos >= 1)
            && !line.starts_with(char::is_whitespace)
        {
            trailers += 1;
            continuations = 0;
        } else if line.starts_with(char::is_whitespace) {
            continuations += 1;
        } else {
            others += 1 + continuations;
            continuations = 0;
        }
    }
    false
}

/// The message of a picked commit: the original, verbatim, with
/// `(cherry picked from commit <oid>)` appended for `-x`.
fn cherry_pick_message(original: &str, oid: &Oid, record_origin: bool) -> String {
    let mut message = original.to_owned();
    if record_origin {
        if !message.is_empty() && !message.ends_with('\n') {
            message.push('\n');
        }
        if !has_trailers(&message) {
            message.push('\n');
        }
        message.push_str(&format!("(cherry picked from commit {})\n", oid.to_hex()));
    }
    message
}

/// Git's message for a revert: `Revert "<subject>"` (or `Reapply "..."`
/// for the revert of a revert) and `This reverts commit <oid>.`.
fn revert_message(subject: &str, oid: &Oid) -> String {
    let title = match subject.strip_prefix("Revert \"") {
        Some(original) if !original.starts_with("Revert \"") => format!("Reapply \"{}", original),
        _ => format!("Revert \"{}\"", subject),
    };
    format!("{}\n\nThis reverts commit {}.\n", title, oid.to_hex())
}

impl Sequence {
    fn dir(repo: &Repository) -> PathBuf {
        repo.git_dir().join("sequencer")
    }

    /// Reads `.git/sequencer/`, written by zerogit or Git.
    fn load(repo: &Repository) -> Result<Option<Sequence>> {
        let dir = Self::dir(repo);
        if !dir.is_dir() {
            return Ok(None);
        }
        let mut action = None;
        let mut todo = VecDeque::new();
        for line in read_file(&dir.join("todo"))?.unwrap_or_default().lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let unsupported = || Error::UnsupportedCherryPick(format!("todo line '{}'", line));
            let line_action = match words.next() {
                Some("pick" | "p") => Action::CherryPick,
                Some("revert") => Action::Revert,
                _ => return Err(unsupported()),
            };
            if action.is_some_and(|a| a != line_action) {
                return Err(unsupported());
            }
            action = Some(line_action);
            let oid = words.next().ok_or_else(unsupported)?;
            todo.push_back(repo.resolve_short_oid(oid)?);
        }
        let action = match action {
            Some(action) => action,
            // Nothing left to do: the stopped commit tells which it is.
            None if repo.git_dir().join("REVERT_HEAD").exists() => Action::Revert,
            None => Action::CherryPick,
        };
        let mut record_origin = false;
        if let Some(content) = read_file(&dir.join("opts"))? {
            let opts = Config::from_str(&content)?;
            for key in opts.keys("options") {
                if key.eq_ignore_ascii_case("record-origin") {
                    record_origin = opts.get_bool("options", key)?;
                } else if !key.eq_ignore_ascii_case("edit") {
                    // zerogit never opens an editor; other options change
                    // what is committed.
                    return Err(Error::UnsupportedCherryPick(format!(
                        "sequencer option '{}'",
                        key
                    )));
                }
            }
        }
        Ok(Some(Sequence {
            action,
            record_origin,
            todo,
            saved: true,
        }))
    }

    /// Starts `.git/sequencer/` for the commits, from `head`.
    fn create(&self, repo: &Repository, head: &Oid) -> Result<()> {
        let dir = Self::dir(repo);
        fs::create_dir_all(&dir)?;
        let hex = format!("{}\n", head.to_hex());
        write_locked(dir.join("head"), hex.as_bytes())?;
        write_locked(dir.join("abort-safety"), hex.as_bytes())?;
        // The options Git would write: `-x`, and `--no-edit` for a revert.
        let opts = match self.action {
            Action::CherryPick if self.record_origin => Some("record-origin = true"),
            Action::CherryPick => None,
            Action::Revert => Some("edit = false"),
        };
        if let Some(option) = opts {
            write_locked(
                dir.join("opts"),
                format!("[options]\n\t{}\n", option).as_bytes(),
            )?;
        }
        self.save_todo(repo)
    }

    /// Writes the commits still to apply, in Git's format.
    fn save_todo(&self, repo: &Repository) -> Result<()> {
        let mut todo = String::new();
        for oid in &self.todo {
            todo.push_str(&format!(
                "{} {} {}\n",
                self.action.command(),
                Repository::short(oid),
                subject_line(&raw_message(repo, oid)?)
            ));
        }
        write_locked(Self::dir(repo).join("todo"), todo.as_bytes())
    }
}

impl Repository {
    /// Applies the changes of commits on top of HEAD, in the given order,
    /// like `git cherry-pick <commit>...`, and returns how it ended.
    ///
    /// Each commit is merged into HEAD (its parent as the base) and
    /// committed with its author and message (verbatim, plus
    /// `(cherry picked from commit <oid>)` with
    /// [`CherryPickOptions::record_origin`]); the committer is
    /// `committer_name` / `committer_email` with the current time. The
    /// reflogs record `cherry-pick: <subject>`. Local changes to paths the
    /// commits do not touch are kept.
    ///
    /// A commit that conflicts stops with [`PickOutcome::Conflicts`]
    /// (`HEAD` and `<commit> (<subject>)` markers); one that changes
    /// nothing stops with [`PickOutcome::Empty`]. `CHERRY_PICK_HEAD`,
    /// `MERGE_MSG` and, for several commits, `.git/sequencer/` are written
    /// as Git writes them, so [`Repository::cherry_pick_continue`],
    /// [`Repository::cherry_pick_skip`] and
    /// [`Repository::cherry_pick_abort`] (or the same `git cherry-pick`
    /// options) carry on. Committing the resolution yourself
    /// ([`Repository::create_commit_with`], or `git commit`) concludes the
    /// stopped commit as well.
    ///
    /// # Errors
    ///
    /// Nothing is changed when any of these is returned:
    /// - `Error::CherryPickInProgress` / `Error::RevertInProgress` /
    ///   `Error::MergeInProgress` / `Error::RebaseInProgress` while one is
    ///   in progress.
    /// - `Error::UnmergedPaths` if the index has conflicts.
    /// - `Error::LocalChangesWouldBeOverwritten` if the index has staged
    ///   changes, or the first commit changes paths with local changes or
    ///   untracked files.
    /// - `Error::UnsupportedCherryPick` for a merge commit or an empty list.
    /// - `Error::RefNotFound` if a commit cannot be resolved or HEAD has no
    ///   commit.
    /// - `Error::Locked` if HEAD, its branch or the index is locked.
    ///
    /// When a later commit fails this way, the commits before it stay
    /// applied and the operation stays in progress, as in Git: fix the
    /// cause and continue, or abort.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{CherryPickOptions, PickOutcome, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let options = CherryPickOptions::new().record_origin(true);
    /// match repo
    ///     .cherry_pick(&["topic~1", "topic"], &options, "John Doe", "john@example.com")
    ///     .unwrap()
    /// {
    ///     PickOutcome::Conflicts { paths, .. } => println!("resolve {:?}", paths),
    ///     outcome => println!("{:?}", outcome),
    /// }
    /// ```
    pub fn cherry_pick(
        &self,
        commits: &[&str],
        options: &CherryPickOptions,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.start_picks(Action::CherryPick, commits, options.record_origin, &who)
    }

    /// Makes commits that undo the changes of commits, in the given order,
    /// like `git revert --no-edit <commit>...`, and returns how it ended.
    ///
    /// Each commit's change is merged into HEAD in reverse (the commit as
    /// the base, its parent as theirs) and committed with Git's message,
    /// `Revert "<subject>"` (`Reapply "<subject>"` when reverting a
    /// revert) and `This reverts commit <oid>.`.
    /// `committer_name` / `committer_email` are the author and committer.
    /// The reflogs record `revert: <subject>`.
    ///
    /// Stops like [`Repository::cherry_pick`], with `REVERT_HEAD`; carry on
    /// with [`Repository::revert_continue`], [`Repository::revert_skip`]
    /// and [`Repository::revert_abort`] (or `git revert`).
    ///
    /// # Errors
    ///
    /// The errors of [`Repository::cherry_pick`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.revert(&["HEAD"], "John Doe", "john@example.com").unwrap();
    /// ```
    pub fn revert(
        &self,
        commits: &[&str],
        committer_name: &str,
        committer_email: &str,
    ) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.start_picks(Action::Revert, commits, false, &who)
    }

    /// Continues a stopped cherry-pick, like `git cherry-pick --continue`.
    ///
    /// The resolved index is committed with the stopped commit's author and
    /// the message in `MERGE_MSG` (its comment lines removed), and the
    /// reflogs record `commit (cherry-pick): <subject>`. Then the remaining
    /// commits are picked.
    ///
    /// # Errors
    ///
    /// - `Error::NoCherryPickInProgress` if no cherry-pick is in progress,
    ///   or `Error::RevertInProgress` during a revert.
    /// - `Error::UnmergedPaths` if conflicts are still unresolved.
    /// - `Error::UnsupportedCherryPick` for a sequence Git started with
    ///   options zerogit does not apply.
    /// - The errors of [`Repository::cherry_pick`] for the remaining
    ///   commits.
    ///
    /// If the resolution leaves nothing to commit, [`PickOutcome::Empty`]
    /// is returned again and nothing changes.
    pub fn cherry_pick_continue(
        &self,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.continue_picks(Action::CherryPick, &who)
    }

    /// Continues a stopped revert, like `git revert --continue`: as
    /// [`Repository::cherry_pick_continue`], with `committer_name` /
    /// `committer_email` as the author too. The reflogs record
    /// `commit: <subject>`.
    ///
    /// # Errors
    ///
    /// As [`Repository::cherry_pick_continue`], with
    /// `Error::NoRevertInProgress` and `Error::CherryPickInProgress`.
    pub fn revert_continue(
        &self,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.continue_picks(Action::Revert, &who)
    }

    /// Skips the commit a cherry-pick stopped at, like
    /// `git cherry-pick --skip`: the changes it made to the index and work
    /// tree are discarded (as by `git reset --merge`), then the remaining
    /// commits are picked.
    ///
    /// # Errors
    ///
    /// - `Error::NoCherryPickInProgress` if no cherry-pick is in progress,
    ///   or `Error::RevertInProgress` during a revert.
    /// - `Error::UnsupportedCherryPick` if the stopped commit was already
    ///   committed (HEAD moved since): continue instead.
    /// - The errors of [`Repository::cherry_pick`] for the remaining
    ///   commits.
    pub fn cherry_pick_skip(
        &self,
        committer_name: &str,
        committer_email: &str,
    ) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.skip_pick(Action::CherryPick, &who)
    }

    /// Skips the commit a revert stopped at, like `git revert --skip`; see
    /// [`Repository::cherry_pick_skip`].
    ///
    /// # Errors
    ///
    /// As [`Repository::cherry_pick_skip`], with `Error::NoRevertInProgress`
    /// and `Error::CherryPickInProgress`.
    pub fn revert_skip(&self, committer_name: &str, committer_email: &str) -> Result<PickOutcome> {
        let who = Signature::now(committer_name, committer_email);
        self.skip_pick(Action::Revert, &who)
    }

    /// Abandons a cherry-pick, like `git cherry-pick --abort`: HEAD (and
    /// its branch), the index and the work tree return to where they were
    /// before it started (as by `git reset --merge`, so local changes to
    /// other paths are kept). If HEAD was moved by something else since the
    /// last pick, it is left where it is, as Git does.
    ///
    /// # Errors
    ///
    /// - `Error::NoCherryPickInProgress` if no cherry-pick is in progress,
    ///   or `Error::RevertInProgress` during a revert.
    /// - `Error::LocalChangesWouldBeOverwritten` if going back would
    ///   overwrite local changes; nothing is changed.
    pub fn cherry_pick_abort(&self) -> Result<()> {
        self.abort_picks(Action::CherryPick)
    }

    /// Abandons a revert, like `git revert --abort`; see
    /// [`Repository::cherry_pick_abort`].
    ///
    /// # Errors
    ///
    /// As [`Repository::cherry_pick_abort`], with `Error::NoRevertInProgress`
    /// and `Error::CherryPickInProgress`.
    pub fn revert_abort(&self) -> Result<()> {
        self.abort_picks(Action::Revert)
    }

    /// The commit a stopped cherry-pick or revert was applying.
    fn stopped_pick(&self) -> Result<Option<(Action, Oid)>> {
        for action in [Action::CherryPick, Action::Revert] {
            if let Some(content) = read_file(&self.git_dir().join(action.head_file()))? {
                return Ok(Some((action, Oid::from_hex(content.trim())?)));
            }
        }
        Ok(None)
    }

    /// The operation in progress, checked against the one asked for.
    fn check_action(
        &self,
        action: Action,
        sequence: Option<&Sequence>,
        stopped: Option<(Action, Oid)>,
    ) -> Result<()> {
        match sequence.map(|s| s.action).or(stopped.map(|s| s.0)) {
            None => Err(action.not_in_progress()),
            Some(running) if running != action => Err(running.in_progress()),
            Some(_) => Ok(()),
        }
    }

    fn start_picks(
        &self,
        action: Action,
        commits: &[&str],
        record_origin: bool,
        who: &Signature,
    ) -> Result<PickOutcome> {
        if self.is_rebasing() {
            return Err(Error::RebaseInProgress);
        }
        if !self.merge_heads()?.is_empty() {
            return Err(Error::MergeInProgress);
        }
        if let Some(sequence) = Sequence::load(self)? {
            return Err(sequence.action.in_progress());
        }
        if let Some((running, _)) = self.stopped_pick()? {
            return Err(running.in_progress());
        }
        if commits.is_empty() {
            return Err(Error::UnsupportedCherryPick("no commits given".to_owned()));
        }
        let mut todo = VecDeque::new();
        for name in commits {
            let oid = self.resolve_commit(name)?;
            if self.commit(&oid.to_hex())?.parents().len() > 1 {
                return Err(Self::merge_commit_error(&oid));
            }
            // As in Git, which walks them as revisions, each commit is
            // applied once.
            if !todo.contains(&oid) {
                todo.push_back(oid);
            }
        }
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let idx = self.read_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let mut sequence = Sequence {
            action,
            record_origin,
            todo,
            saved: commits.len() > 1,
        };
        if sequence.saved {
            sequence.create(self, &head)?;
        }
        self.run_picks(&mut sequence, true, who)
    }

    fn merge_commit_error(oid: &Oid) -> Error {
        Error::UnsupportedCherryPick(format!(
            "commit {} is a merge; picking a mainline parent is not supported",
            oid.to_hex()
        ))
    }

    /// Applies the commits still to apply; stops at a conflict or an empty
    /// commit, or finishes. With `fresh`, a failure of the first commit
    /// removes the sequencer just started, so nothing is changed.
    fn run_picks(
        &self,
        sequence: &mut Sequence,
        fresh: bool,
        who: &Signature,
    ) -> Result<PickOutcome> {
        let mut first = fresh;
        while let Some(oid) = sequence.todo.front().copied() {
            if sequence.saved {
                sequence.save_todo(self)?;
            }
            let stop = match self.pick_one(sequence.action, &oid, sequence.record_origin, who) {
                Ok(stop) => stop,
                Err(e) => {
                    if first && sequence.saved {
                        remove_dir(&Sequence::dir(self))?;
                    }
                    return Err(e);
                }
            };
            first = false;
            if let Some(stop) = stop {
                return Ok(stop);
            }
            sequence.todo.pop_front();
            if sequence.saved {
                self.update_abort_safety()?;
            }
        }
        if sequence.saved {
            remove_dir(&Sequence::dir(self))?;
        }
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        Ok(PickOutcome::Completed(head))
    }

    /// Records HEAD as the commit the sequence last made, which `--abort`
    /// checks before going back.
    fn update_abort_safety(&self) -> Result<()> {
        let dir = Sequence::dir(self);
        if !dir.is_dir() {
            return Ok(());
        }
        let head = self.optional_head_oid()?;
        let content = head.map(|h| format!("{}\n", h.to_hex()));
        write_locked(
            dir.join("abort-safety"),
            content.unwrap_or_default().as_bytes(),
        )
    }

    /// Picks or reverts one commit onto HEAD and commits it; returns the
    /// stop when it conflicts or changes nothing.
    fn pick_one(
        &self,
        action: Action,
        oid: &Oid,
        record_origin: bool,
        who: &Signature,
    ) -> Result<Option<PickOutcome>> {
        let commit = self.commit(&oid.to_hex())?;
        if commit.parents().len() > 1 {
            return Err(Self::merge_commit_error(oid));
        }
        // Lock HEAD first, so a locked HEAD changes nothing.
        let head_lock = self.lock_head()?;
        let head = head_lock
            .old
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let original = raw_message(self, oid)?;
        let subject = subject_line(&original);
        let label = format!("{} ({})", Self::short(oid), subject);
        let parent_label = format!("parent of {}", label);
        let parent_tree = self.flat_tree(commit.parents().first())?;
        let commit_tree = self.flat_tree(Some(oid))?;
        let (base, theirs, labels, message) = match action {
            Action::CherryPick => (
                &parent_tree,
                &commit_tree,
                Labels {
                    ours: "HEAD",
                    base: &parent_label,
                    theirs: &label,
                },
                cherry_pick_message(&original, oid, record_origin),
            ),
            Action::Revert => (
                &commit_tree,
                &parent_tree,
                Labels {
                    ours: "HEAD",
                    base: &label,
                    theirs: &parent_label,
                },
                revert_message(subject, oid),
            ),
        };
        let conflicts = self.pick_merge(&head, base, theirs, labels, self.conflict_style()?)?;
        if !conflicts.is_empty() {
            // The message, an empty line and the conflicted paths as
            // comments, which committing strips.
            let mut merge_msg = message;
            merge_msg.push_str("\n# Conflicts:\n");
            for path in &conflicts {
                merge_msg.push_str(&format!("#\t{}\n", path));
            }
            self.record_pick_stop(action, oid, &merge_msg)?;
            return Ok(Some(PickOutcome::Conflicts {
                commit: *oid,
                paths: conflicts.into_iter().map(PathBuf::from).collect(),
            }));
        }
        let tree = self.build_tree_from_index(&self.read_index()?)?;
        if tree == *self.commit(&head.to_hex())?.tree() {
            self.record_pick_stop(action, oid, &message)?;
            return Ok(Some(PickOutcome::Empty { commit: *oid }));
        }
        let author = match action {
            Action::CherryPick => commit.author().to_git_string(),
            Action::Revert => who.to_git_string(),
        };
        let content = Self::format_commit(&tree, &[head], &author, &who.to_git_string(), &message);
        let new = self.object_store().write(ObjectType::Commit, &content)?;
        let reflog = match action {
            Action::CherryPick => "cherry-pick",
            Action::Revert => "revert",
        };
        head_lock.update(
            self,
            &new,
            who,
            &format!("{}: {}", reflog, message.lines().next().unwrap_or("")),
        )?;
        Ok(None)
    }

    /// Records a stopped commit the way Git does.
    fn record_pick_stop(&self, action: Action, oid: &Oid, message: &str) -> Result<()> {
        write_locked(
            self.git_dir().join(action.head_file()),
            format!("{}\n", oid.to_hex()).as_bytes(),
        )?;
        write_locked(self.git_dir().join("MERGE_MSG"), message.as_bytes())
    }

    fn continue_picks(&self, action: Action, who: &Signature) -> Result<PickOutcome> {
        let sequence = Sequence::load(self)?;
        let stopped = self.stopped_pick()?;
        self.check_action(action, sequence.as_ref(), stopped)?;
        let idx = self.read_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        if let Some((_, oid)) = stopped {
            let tree = self.build_tree_from_index(&idx)?;
            if tree == *self.commit(&head.to_hex())?.tree() {
                return Ok(PickOutcome::Empty { commit: oid });
            }
            // Committing as `git commit` does, which also removes the stop.
            let message = match read_file(&self.git_dir().join("MERGE_MSG"))? {
                Some(message) => strip_message(&message),
                None => match action {
                    Action::CherryPick => raw_message(self, &oid)?,
                    Action::Revert => revert_message(subject_line(&raw_message(self, &oid)?), &oid),
                },
            };
            if message.is_empty() {
                return Err(Error::EmptyCommitMessage);
            }
            let author = match action {
                Action::CherryPick => self.commit(&oid.to_hex())?.author().clone(),
                Action::Revert => who.clone(),
            };
            self.commit_index(&message, &author, who, EmptyCheck::UnchangedTree, false)?;
        } else {
            // Concluded already (for example by `git commit`): the index
            // must be clean to go on.
            let staged = staged_changes(&idx, &self.flat_tree(Some(&head))?);
            if !staged.is_empty() {
                return Err(Error::LocalChangesWouldBeOverwritten(staged));
            }
        }
        match sequence {
            Some(mut sequence) => {
                sequence.todo.pop_front();
                self.update_abort_safety()?;
                self.run_picks(&mut sequence, false, who)
            }
            None => Ok(PickOutcome::Completed(
                self.optional_head_oid()?
                    .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?,
            )),
        }
    }

    fn skip_pick(&self, action: Action, who: &Signature) -> Result<PickOutcome> {
        let sequence = Sequence::load(self)?;
        let stopped = self.stopped_pick()?;
        self.check_action(action, sequence.as_ref(), stopped)?;
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        if stopped.is_none() && !self.rollback_is_safe(&head)? {
            return Err(Error::UnsupportedCherryPick(
                "nothing to skip: the stopped commit was committed already; continue instead"
                    .to_owned(),
            ));
        }
        self.reset_merge_moving(&head)?;
        self.clear_pick_stop()?;
        match sequence {
            Some(mut sequence) => {
                sequence.todo.pop_front();
                self.run_picks(&mut sequence, false, who)
            }
            None => Ok(PickOutcome::Completed(head)),
        }
    }

    fn abort_picks(&self, action: Action) -> Result<()> {
        let sequence = Sequence::load(self)?;
        let stopped = self.stopped_pick()?;
        self.check_action(action, sequence.as_ref(), stopped)?;
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        match sequence {
            Some(_) => {
                // Go back to where the sequence started, unless HEAD moved
                // since the last pick.
                if self.rollback_is_safe(&head)? {
                    let start = read_file(&Sequence::dir(self).join("head"))?.ok_or_else(|| {
                        Error::UnsupportedCherryPick("sequencer/head is missing".to_owned())
                    })?;
                    self.reset_merge_moving(&Oid::from_hex(start.trim())?)?;
                }
                self.clear_pick_stop()?;
                remove_dir(&Sequence::dir(self))
            }
            None => {
                self.reset_merge_moving(&head)?;
                self.clear_pick_stop()
            }
        }
    }

    /// Whether HEAD is still the commit the sequence last made.
    fn rollback_is_safe(&self, head: &Oid) -> Result<bool> {
        let safety = read_file(&Sequence::dir(self).join("abort-safety"))?;
        Ok(safety.is_some_and(|hex| hex.trim() == head.to_hex()))
    }

    /// Moves HEAD to `target` like `git reset --merge <target>`, recording
    /// `ORIG_HEAD` and `reset: moving to <target>`.
    fn reset_merge_moving(&self, target: &Oid) -> Result<()> {
        let head_lock = self.lock_head()?;
        let ours = self.flat_tree(head_lock.old.as_ref())?;
        self.reset_merge_to(&ours, &self.flat_tree(Some(target))?)?;
        if let Some(old) = head_lock.old {
            self.write_orig_head(&old)?;
        }
        let who = self.reflog_identity()?;
        head_lock.update(
            self,
            target,
            &who,
            &format!("reset: moving to {}", target.to_hex()),
        )
    }

    /// Removes the files of a stopped commit.
    fn clear_pick_stop(&self) -> Result<()> {
        for name in ["CHERRY_PICK_HEAD", "REVERT_HEAD", "MERGE_MSG"] {
            remove_file(&self.git_dir().join(name))?;
        }
        Ok(())
    }

    /// Ends a stopped cherry-pick or revert after its commit was made (or
    /// the state was reset), as Git's commit and reset do: removes
    /// `CHERRY_PICK_HEAD` / `REVERT_HEAD`, and `.git/sequencer/` when that
    /// commit was the last. Returns whether one was stopped.
    pub(crate) fn conclude_pick(&self) -> Result<bool> {
        let mut stopped = false;
        for name in ["CHERRY_PICK_HEAD", "REVERT_HEAD"] {
            stopped |= remove_file(&self.git_dir().join(name))?;
        }
        if stopped {
            let dir = Sequence::dir(self);
            if let Some(todo) = read_file(&dir.join("todo"))? {
                if todo.trim_end_matches('\n').lines().count() <= 1 {
                    remove_dir(&dir)?;
                }
            }
        }
        Ok(stopped)
    }

    /// Whether a cherry-pick has stopped (`CHERRY_PICK_HEAD` exists).
    pub(crate) fn cherry_pick_head(&self) -> Result<Option<Oid>> {
        Ok(match self.stopped_pick()? {
            Some((Action::CherryPick, oid)) => Some(oid),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid() -> Oid {
        Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    #[test]
    fn record_origin_follows_trailers_like_git() {
        let line = "(cherry picked from commit 0123456789abcdef0123456789abcdef01234567)\n";
        assert_eq!(
            cherry_pick_message("Subject\n", &oid(), true),
            format!("Subject\n\n{}", line)
        );
        assert_eq!(
            cherry_pick_message("Subject\n\nSigned-off-by: A <a@b>\n", &oid(), true),
            format!("Subject\n\nSigned-off-by: A <a@b>\n{}", line)
        );
        assert_eq!(
            cherry_pick_message("Subject\n\nFixes: 12\n cont\nSome prose", &oid(), true),
            format!("Subject\n\nFixes: 12\n cont\nSome prose\n\n{}", line)
        );
        // A title alone is never trailers.
        assert_eq!(
            cherry_pick_message("Fixes: 12\n", &oid(), true),
            format!("Fixes: 12\n\n{}", line)
        );
        // Mostly trailers with a Git-generated one counts.
        assert!(has_trailers(
            "S\n\nSigned-off-by: A <a@b>\nReviewed-by: B\nnot a trailer\n"
        ));
        assert!(!has_trailers(
            "S\n\nnot a trailer\nnor this\nReviewed-by: B\n"
        ));
        assert_eq!(cherry_pick_message("Body\n", &oid(), false), "Body\n");
    }

    #[test]
    fn revert_messages_like_git() {
        let hex = oid().to_hex();
        assert_eq!(
            revert_message("Add a", &oid()),
            format!("Revert \"Add a\"\n\nThis reverts commit {}.\n", hex)
        );
        assert_eq!(
            revert_message("Revert \"Add a\"", &oid()),
            format!("Reapply \"Add a\"\n\nThis reverts commit {}.\n", hex)
        );
        assert_eq!(
            revert_message("Revert \"Revert \"Add a\"\"", &oid()),
            format!(
                "Revert \"Revert \"Revert \"Add a\"\"\"\n\nThis reverts commit {}.\n",
                hex
            )
        );
        assert_eq!(subject_line("\n  \nTitle  \nmore\n"), "Title  ");
    }
}
