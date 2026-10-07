//! Merging: common ancestors, fast-forwards and three-way merges.
//!
//! [`Repository::merge`] works like `git merge <commit>`: it fast-forwards
//! when possible, and otherwise merges the trees against the merge base,
//! creating a merge commit or stopping with the conflicts recorded the way
//! Git records them (index stages 1-3, conflict markers in the work tree,
//! `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE` and `ORIG_HEAD`). A stopped merge
//! is concluded with [`Repository::create_commit`] or abandoned with
//! [`Repository::abort_merge`], and Git can do either as well.

pub(crate) mod file;
pub(crate) mod tree;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::infra::write_locked;
use crate::objects::{ObjectType, Oid, Signature};
use crate::repository::Repository;
use crate::worktree::{native_path, Worktree};

use file::{ConflictStyle, Labels};
use tree::{Flat, Resolution, TreeMerge};

/// Whether a merge may or must fast-forward (`--ff`, `--ff-only`,
/// `--no-ff`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FastForward {
    /// Fast-forward when possible, otherwise create a merge commit.
    #[default]
    Allow,
    /// Only fast-forward; fail with `Error::NotFastForward` otherwise.
    Only,
    /// Always create a merge commit.
    Never,
}

/// Options for [`Repository::merge`].
#[derive(Debug, Clone, Default)]
pub struct MergeOptions {
    fast_forward: FastForward,
    message: Option<String>,
}

impl MergeOptions {
    /// The default options: fast-forward when possible, Git's message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the fast-forward behaviour.
    pub fn fast_forward(mut self, fast_forward: FastForward) -> Self {
        self.fast_forward = fast_forward;
        self
    }

    /// Sets the merge commit message instead of Git's default
    /// (`Merge branch 'topic'`, plus ` into <branch>` unless the current
    /// branch is `main` or `master`).
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

/// The result of [`Repository::merge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The commit is already part of HEAD's history; nothing changed.
    UpToDate,
    /// HEAD (and its branch) moved forward to the commit.
    FastForward(Oid),
    /// A merge commit was created.
    Merged(Oid),
    /// The merge stopped with conflicts at these paths. Resolve them (for
    /// example by editing the files and calling [`Repository::add`]) and
    /// conclude with [`Repository::create_commit`], or call
    /// [`Repository::abort_merge`].
    Conflicts(Vec<PathBuf>),
}

/// How the merged commit was named, for the default message.
enum TargetKind {
    Branch,
    Tag,
    RemoteBranch,
    Commit,
}

impl Repository {
    /// The commits in `MERGE_HEAD` (empty when no merge is in progress).
    pub(crate) fn merge_heads(&self) -> Result<Vec<Oid>> {
        let content = match std::fs::read_to_string(self.git_dir().join("MERGE_HEAD")) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| Oid::from_hex(l.trim()))
            .collect()
    }

    /// Returns the commit being merged while a merge is stopped on
    /// conflicts (`MERGE_HEAD`), or `None` when no merge is in progress.
    pub fn merge_head(&self) -> Result<Option<Oid>> {
        Ok(self.merge_heads()?.first().copied())
    }

    /// Whether merges follow renames: `merge.renames`, which defaults to
    /// `diff.renames` (true unless set to false), as in Git.
    pub(crate) fn merge_renames(&self) -> Result<bool> {
        let config = self.config()?;
        let enabled = |value: &str| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "false" | "no" | "off" | "0"
            )
        };
        Ok(match config.get("merge", "renames") {
            Some(value) => enabled(value),
            None => config.get("diff", "renames").map_or(true, enabled),
        })
    }

    /// Removes `MERGE_HEAD`, `MERGE_MSG` and `MERGE_MODE`.
    pub(crate) fn clear_merge_state(&self) -> Result<()> {
        for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE"] {
            match std::fs::remove_file(self.git_dir().join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn parents_of(&self, oid: &Oid, cache: &mut HashMap<Oid, Vec<Oid>>) -> Result<Vec<Oid>> {
        if let Some(parents) = cache.get(oid) {
            return Ok(parents.clone());
        }
        let parents = self.commit(&oid.to_hex())?.parents().to_vec();
        cache.insert(*oid, parents.clone());
        Ok(parents)
    }

    fn ancestors(&self, start: &Oid, cache: &mut HashMap<Oid, Vec<Oid>>) -> Result<HashSet<Oid>> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([*start]);
        while let Some(oid) = queue.pop_front() {
            if seen.insert(oid) {
                queue.extend(self.parents_of(&oid, cache)?);
            }
        }
        Ok(seen)
    }

    /// Returns all best common ancestors of two commits, like
    /// `git merge-base --all`: common ancestors that are not ancestors of
    /// another common ancestor. Usually there is one; there are several after
    /// criss-cross merges, and none for unrelated histories.
    pub fn merge_bases(&self, a: &Oid, b: &Oid) -> Result<Vec<Oid>> {
        let mut cache = HashMap::new();
        let of_a = self.ancestors(a, &mut cache)?;
        let of_b = self.ancestors(b, &mut cache)?;
        let common: HashSet<Oid> = of_a.intersection(&of_b).copied().collect();
        // Every ancestor of a common ancestor is common; drop those.
        let mut below = HashSet::new();
        let mut queue: VecDeque<Oid> = VecDeque::new();
        for oid in &common {
            queue.extend(self.parents_of(oid, &mut cache)?);
        }
        while let Some(oid) = queue.pop_front() {
            if below.insert(oid) {
                queue.extend(self.parents_of(&oid, &mut cache)?);
            }
        }
        let mut bases: Vec<Oid> = common.difference(&below).copied().collect();
        // Oldest first, by committer time then OID, for a stable order.
        let mut keyed = Vec::new();
        for oid in bases.drain(..) {
            let time = self.commit(&oid.to_hex())?.committer().timestamp();
            keyed.push((time, oid));
        }
        keyed.sort_by(|x, y| {
            x.0.cmp(&y.0)
                .then_with(|| x.1.as_bytes().cmp(y.1.as_bytes()))
        });
        Ok(keyed.into_iter().map(|(_, oid)| oid).collect())
    }

    /// Returns a best common ancestor of two commits (`git merge-base`), or
    /// `None` if the histories are unrelated. See
    /// [`Repository::merge_bases`] for all of them.
    pub fn merge_base(&self, a: &Oid, b: &Oid) -> Result<Option<Oid>> {
        Ok(self.merge_bases(a, b)?.pop())
    }

    /// Resolves what to merge: a branch, tag, remote-tracking branch, full
    /// reference name or commit, peeling annotated tags.
    fn resolve_merge_target(&self, target: &str) -> Result<(Oid, TargetKind)> {
        // The kind of reference names the merge in the default message.
        let kind = match self.dwim_ref(target)? {
            Some(full) if full.starts_with("refs/heads/") => TargetKind::Branch,
            Some(full) if full.starts_with("refs/tags/") => TargetKind::Tag,
            Some(full) if full.starts_with("refs/remotes/") => TargetKind::RemoteBranch,
            _ => TargetKind::Commit,
        };
        let oid = match self.rev_parse(target) {
            Ok(oid) => oid,
            Err(
                Error::InvalidRevision { .. } | Error::ObjectNotFound(_) | Error::InvalidOid(_),
            ) => return Err(Error::RefNotFound(target.to_owned())),
            Err(e) => return Err(e),
        };
        // Peel annotated tags to the commit.
        Ok((self.peel_to(oid, ObjectType::Commit)?, kind))
    }

    fn default_merge_message(&self, target: &str, kind: &TargetKind) -> Result<String> {
        let what = match kind {
            TargetKind::Branch => "branch",
            TargetKind::Tag => "tag",
            TargetKind::RemoteBranch => "remote-tracking branch",
            TargetKind::Commit => "commit",
        };
        let mut message = format!("Merge {} '{}'", what, target);
        if let Some(branch) = self.ref_store().current_branch()? {
            if branch != "main" && branch != "master" {
                message.push_str(&format!(" into {}", branch));
            }
        }
        Ok(message)
    }

    /// The flattened tree of a commit (empty for `None`).
    pub(crate) fn flat_tree(&self, commit: Option<&Oid>) -> Result<Flat> {
        let mut flat = BTreeMap::new();
        if let Some(oid) = commit {
            let tree = *self.commit(&oid.to_hex())?.tree();
            let mut by_path = BTreeMap::new();
            crate::status::flatten_with_modes(&self.object_store(), &tree, "", &mut by_path)?;
            for (path, entry) in by_path {
                flat.insert(path.to_string_lossy().into_owned(), entry);
            }
        }
        Ok(flat)
    }

    pub(crate) fn conflict_style(&self) -> Result<ConflictStyle> {
        Ok(match self.config()?.get("merge", "conflictstyle") {
            Some(style)
                if style.eq_ignore_ascii_case("diff3") || style.eq_ignore_ascii_case("zdiff3") =>
            {
                ConflictStyle::Diff3
            }
            _ => ConflictStyle::Merge,
        })
    }

    /// Merges several merge bases into one tree, as Git's recursive and ort
    /// strategies do after criss-cross merges.
    fn virtual_base(&self, bases: &[Oid], style: ConflictStyle) -> Result<Flat> {
        let mut merged = self.flat_tree(Some(&bases[0]))?;
        let mut merged_commits = vec![bases[0]];
        for next in &bases[1..] {
            // The common ancestors of what is merged so far and the next base.
            let mut inner_bases = Vec::new();
            for done in &merged_commits {
                for base in self.merge_bases(done, next)? {
                    if !inner_bases.contains(&base) {
                        inner_bases.push(base);
                    }
                }
            }
            let inner = match inner_bases.len() {
                0 => Flat::new(),
                1 => self.flat_tree(Some(&inner_bases[0]))?,
                _ => self.virtual_base(&inner_bases, style)?,
            };
            let labels = Labels {
                ours: "Temporary merge branch 1",
                base: "merged common ancestors",
                theirs: "Temporary merge branch 2",
            };
            merged = tree::merge_trees(
                &self.object_store(),
                &inner,
                &merged,
                &self.flat_tree(Some(next))?,
                labels,
                style,
                self.merge_renames()?,
            )?
            .as_flat();
            merged_commits.push(*next);
        }
        Ok(merged)
    }

    /// Merges a branch, tag, remote-tracking branch or commit into HEAD, like
    /// `git merge <target>`.
    ///
    /// - If `target` is already in HEAD's history: [`MergeOutcome::UpToDate`].
    /// - If HEAD is in `target`'s history: fast-forward (unless
    ///   [`FastForward::Never`]); the index and work tree are updated and
    ///   local changes on paths the update does not touch are kept.
    /// - Otherwise the trees are merged against the merge base (several
    ///   bases are first merged into one). Without conflicts a merge commit
    ///   with parents HEAD and `target` is created, authored like
    ///   [`Repository::create_commit`]; with conflicts the merge stops and
    ///   is recorded as Git records it.
    ///
    /// Conflicts are written in the style of `merge.conflictStyle` (`merge`
    /// or `diff3`; `zdiff3` is treated as `diff3`). Contents are merged with
    /// the histogram diff, as `git merge` does. Renames are not detected, so
    /// a file renamed on one side and changed on the other conflicts as a
    /// deletion and a modification.
    ///
    /// # Errors
    ///
    /// Nothing is changed when any of these is returned:
    /// - `Error::MergeInProgress` if a merge is already in progress, or
    ///   `Error::CherryPickInProgress` while a cherry-pick is stopped.
    /// - `Error::UnmergedPaths` if the index has unresolved conflicts.
    /// - `Error::NotFastForward` with [`FastForward::Only`] when the
    ///   histories have diverged.
    /// - `Error::LocalChangesWouldBeOverwritten` if the merge would change
    ///   paths with local changes or untracked files, or (for a merge that
    ///   is not a fast-forward) the index has staged changes.
    /// - `Error::UnsupportedMerge` for unrelated histories or a path that is
    ///   a file on one side and a directory on the other.
    /// - `Error::RefNotFound` if `target` cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{MergeOptions, MergeOutcome, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// match repo
    ///     .merge("topic", "John Doe", "john@example.com", &MergeOptions::new())
    ///     .unwrap()
    /// {
    ///     MergeOutcome::Conflicts(paths) => println!("conflicts in {:?}", paths),
    ///     outcome => println!("{:?}", outcome),
    /// }
    /// ```
    pub fn merge(
        &self,
        target: &str,
        author_name: &str,
        author_email: &str,
        options: &MergeOptions,
    ) -> Result<MergeOutcome> {
        if !self.merge_heads()?.is_empty() {
            return Err(Error::MergeInProgress);
        }
        // Git refuses to merge before a stopped cherry-pick is concluded.
        if self.git_dir().join("CHERRY_PICK_HEAD").exists() {
            return Err(Error::CherryPickInProgress);
        }
        let (index_lock, mut idx) = self.lock_index()?;
        if idx.has_conflicts() {
            return Err(Error::UnmergedPaths(idx.conflicted_paths()));
        }
        let (theirs, kind) = self.resolve_merge_target(target)?;
        let head = self.optional_head_oid()?;
        let committer = Signature::now(author_name, author_email);
        let reflog_prefix = format!("merge {}", target);

        let bases = match head {
            Some(head) => self.merge_bases(&head, &theirs)?,
            None => Vec::new(),
        };
        if bases.contains(&theirs) {
            return Ok(MergeOutcome::UpToDate);
        }
        let can_fast_forward = head.map_or(true, |h| bases.contains(&h));
        if can_fast_forward && (options.fast_forward != FastForward::Never || head.is_none()) {
            let ours = self.flat_tree(head.as_ref())?;
            let target_tree = self.flat_tree(Some(&theirs))?;
            let mut worktree = self.worktree()?;
            let result = two_way(&ours, &target_tree);
            self.check_overwrites(&idx, &ours, &result, &mut worktree)?;
            if let Some(head) = head {
                self.write_orig_head(&head)?;
            }
            self.apply_merge(&mut idx, &mut worktree, &ours, &result)?;
            index_lock.write(&idx)?;
            self.update_head(
                &theirs,
                head,
                &committer,
                &format!("{}: Fast-forward", reflog_prefix),
            )?;
            return Ok(MergeOutcome::FastForward(theirs));
        }
        if options.fast_forward == FastForward::Only {
            return Err(Error::NotFastForward);
        }
        let head = head.expect("an unborn HEAD always fast-forwards");
        if bases.is_empty() {
            return Err(Error::UnsupportedMerge(
                "refusing to merge unrelated histories".to_owned(),
            ));
        }

        // A merge that is not a fast-forward starts from a clean index.
        let ours = self.flat_tree(Some(&head))?;
        let staged = staged_changes(&idx, &ours);
        if !staged.is_empty() {
            return Err(Error::LocalChangesWouldBeOverwritten(staged));
        }

        let style = self.conflict_style()?;
        let base_tree = if bases.len() == 1 {
            self.flat_tree(Some(&bases[0]))?
        } else {
            self.virtual_base(&bases, style)?
        };
        let base_label = if bases.len() == 1 {
            bases[0].to_hex()[..7].to_owned()
        } else {
            "merged common ancestors".to_owned()
        };
        let labels = Labels {
            ours: "HEAD",
            base: &base_label,
            theirs: target,
        };
        let theirs_tree = self.flat_tree(Some(&theirs))?;
        let result = tree::merge_trees(
            &self.object_store(),
            &base_tree,
            &ours,
            &theirs_tree,
            labels,
            style,
            self.merge_renames()?,
        )?;

        let mut worktree = self.worktree()?;
        self.check_overwrites(&idx, &ours, &result, &mut worktree)?;
        let message = match &options.message {
            Some(message) => message.clone(),
            None => self.default_merge_message(target, &kind)?,
        };
        self.write_orig_head(&head)?;
        self.apply_merge(&mut idx, &mut worktree, &ours, &result)?;
        index_lock.write(&idx)?;

        let conflicts = result.conflicted_paths();
        if !conflicts.is_empty() {
            let git_dir = self.git_dir();
            write_locked(
                git_dir.join("MERGE_HEAD"),
                format!("{}\n", theirs.to_hex()).as_bytes(),
            )?;
            let mode = if options.fast_forward == FastForward::Never {
                "no-ff"
            } else {
                ""
            };
            write_locked(git_dir.join("MERGE_MODE"), mode.as_bytes())?;
            let mut merge_msg = format!("{}\n\n# Conflicts:\n", message);
            for path in &conflicts {
                merge_msg.push_str(&format!("#\t{}\n", path));
            }
            write_locked(git_dir.join("MERGE_MSG"), merge_msg.as_bytes())?;
            return Ok(MergeOutcome::Conflicts(
                conflicts.iter().map(PathBuf::from).collect(),
            ));
        }

        let tree_oid = self.build_tree_from_index(&idx)?;
        let signature = committer.to_git_string();
        let content = Self::format_commit(
            &tree_oid,
            &[head, theirs],
            &signature,
            &signature,
            &crate::commit::cleanup_message(&message),
        );
        let commit = self.object_store().write(ObjectType::Commit, &content)?;
        self.update_head(
            &commit,
            Some(head),
            &committer,
            &format!("{}: Merge made by the 'zerogit' strategy.", reflog_prefix),
        )?;
        Ok(MergeOutcome::Merged(commit))
    }

    pub(crate) fn write_orig_head(&self, head: &Oid) -> Result<()> {
        write_locked(
            self.git_dir().join("ORIG_HEAD"),
            format!("{}\n", head.to_hex()).as_bytes(),
        )
    }

    /// Refuses a merge result that would lose local changes: a changed path
    /// whose work tree file differs from the index, an untracked file (not
    /// ignored) where a file is written, or an untracked file where a
    /// directory is needed.
    pub(crate) fn check_overwrites(
        &self,
        idx: &Index,
        ours: &Flat,
        result: &TreeMerge,
        worktree: &mut Worktree,
    ) -> Result<()> {
        let mut blocked = Vec::new();
        for (path, resolution) in &result.paths {
            let ours_entry = ours.get(path).copied();
            let written = match resolution {
                Resolution::Clean(entry) => *entry,
                Resolution::Conflict { worktree, .. } => *worktree,
            };
            let index_entry = idx.get_stage(Path::new(path), 0);
            let changed = match resolution {
                Resolution::Clean(entry) => *entry != ours_entry,
                Resolution::Conflict { .. } => true,
            };
            if !changed {
                continue;
            }
            let native = native_path(Path::new(path));
            // The index must still have our version of the path.
            if index_entry.map(|e| (*e.oid(), e.mode())) != ours_entry {
                blocked.push(PathBuf::from(path));
                continue;
            }
            match index_entry {
                Some(entry) if !entry.skip_worktree() => {
                    let current = worktree.hash(&native, Some(entry))?;
                    if current.is_some() && current != Some((*entry.oid(), entry.mode())) {
                        blocked.push(PathBuf::from(path));
                    }
                }
                Some(_) => {}
                None => {
                    if written.is_some()
                        && worktree.metadata(&native)?.is_some()
                        && !worktree.rules().is_ignored(&native, false)?
                    {
                        blocked.push(PathBuf::from(path));
                    }
                }
            }
            // A file written below a path that is an untracked file.
            if written.is_some() {
                let mut prefix = PathBuf::new();
                let components: Vec<_> = native.components().collect();
                for component in &components[..components.len().saturating_sub(1)] {
                    prefix.push(component);
                    let tracked = idx.get(&prefix).is_some();
                    if let Some(metadata) = worktree.metadata(&prefix)? {
                        if !metadata.is_dir() && !tracked {
                            blocked.push(PathBuf::from(path));
                            break;
                        }
                    }
                }
            }
        }
        if blocked.is_empty() {
            Ok(())
        } else {
            Err(Error::LocalChangesWouldBeOverwritten(blocked))
        }
    }

    /// Writes a merge result to the work tree and the index. Paths whose
    /// result equals ours are left alone.
    pub(crate) fn apply_merge(
        &self,
        idx: &mut Index,
        worktree: &mut Worktree,
        ours: &Flat,
        result: &TreeMerge,
    ) -> Result<()> {
        // Refuse unsupported conversions before changing anything.
        for (path, resolution) in &result.paths {
            let written = match resolution {
                Resolution::Clean(entry) => *entry,
                Resolution::Conflict { worktree, .. } => *worktree,
            };
            if let Some((_, mode)) = written {
                if written != ours.get(path).copied()
                    && matches!(mode, crate::FileMode::Regular | crate::FileMode::Executable)
                {
                    worktree.check_supported(&native_path(Path::new(path)))?;
                }
            }
        }
        // Removals first, so files can replace directories that empty out.
        for (path, resolution) in &result.paths {
            let written = match resolution {
                Resolution::Clean(entry) => *entry,
                Resolution::Conflict { worktree, .. } => *worktree,
            };
            if written.is_none() && ours.contains_key(path) {
                worktree.remove(&native_path(Path::new(path)))?;
            }
        }
        for (path, resolution) in &result.paths {
            let index_path = PathBuf::from(path);
            let native = native_path(Path::new(path));
            let ours_entry = ours.get(path).copied();
            match resolution {
                Resolution::Clean(entry) => {
                    if *entry == ours_entry {
                        continue;
                    }
                    match entry {
                        Some((oid, mode)) => {
                            worktree.write_blob(&native, oid, *mode)?;
                            idx.add(worktree.stat_entry(&native, index_path, *oid, *mode)?);
                        }
                        None => {
                            idx.remove(&index_path);
                        }
                    }
                }
                Resolution::Conflict {
                    stages,
                    worktree: written,
                } => {
                    if let Some((oid, mode)) = written {
                        if *written != ours_entry {
                            worktree.write_blob(&native, oid, *mode)?;
                        }
                    }
                    idx.remove(&index_path);
                    for (i, stage) in stages.iter().enumerate() {
                        if let Some((oid, mode)) = stage {
                            idx.add(IndexEntry::new(
                                0,
                                0,
                                0,
                                0,
                                *mode,
                                0,
                                0,
                                0,
                                *oid,
                                index_path.clone(),
                                i as u8 + 1,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Abandons a merge stopped on conflicts, like `git merge --abort`.
    ///
    /// Paths the merge changed in the index (including conflicts) are reset
    /// to HEAD in the index and the work tree; local changes to other paths
    /// are kept. `MERGE_HEAD`, `MERGE_MSG` and `MERGE_MODE` are removed.
    ///
    /// # Errors
    ///
    /// `Error::NoMergeInProgress` if there is no `MERGE_HEAD`.
    pub fn abort_merge(&self) -> Result<()> {
        if self.merge_heads()?.is_empty() {
            return Err(Error::NoMergeInProgress);
        }
        let head = self.optional_head_oid()?;
        let ours = self.flat_tree(head.as_ref())?;
        self.reset_merge_to(&ours, &ours)?;
        self.clear_merge_state()
    }

    /// Moves the index and work tree from HEAD's tree `ours` to `target`
    /// like `git reset --merge` (HEAD itself is not moved): paths whose
    /// index entry differs from HEAD (including conflicts) and paths that
    /// differ between the trees get `target`'s version; local changes to
    /// other paths are kept.
    ///
    /// # Errors
    ///
    /// `Error::LocalChangesWouldBeOverwritten` if a path that differs
    /// between the trees has local changes; nothing is changed.
    pub(crate) fn reset_merge_to(&self, ours: &Flat, target: &Flat) -> Result<()> {
        let (index_lock, mut idx) = self.lock_index()?;
        let mut worktree = self.worktree()?;

        // Paths whose index state differs from HEAD.
        let mut paths: Vec<String> = Vec::new();
        for entry in idx.entries() {
            let key = String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned();
            let differs =
                entry.stage() != 0 || ours.get(&key).copied() != Some((*entry.oid(), entry.mode()));
            if differs && paths.last() != Some(&key) {
                paths.push(key);
            }
        }
        for path in ours.keys() {
            if idx.get(Path::new(path)).is_none() {
                paths.push(path.clone());
            }
        }
        // Paths the move between the trees changes must not have local
        // changes.
        let mut moving = two_way(ours, target);
        moving.paths.retain(|path, _| !paths.contains(path));
        self.check_overwrites(&idx, ours, &moving, &mut worktree)?;
        paths.extend(moving.paths.into_keys());

        self.restore_paths(&mut idx, &mut worktree, target, &paths)?;
        // Back at a commit, there is no resolved conflict left to recreate.
        idx.clear_resolve_undo();
        index_lock.write(&idx)
    }
}

impl Repository {
    /// Resets `paths` to their version in `target` in both the index and the
    /// work tree (removing them where `target` has none).
    pub(crate) fn restore_paths(
        &self,
        idx: &mut Index,
        worktree: &mut Worktree,
        target: &Flat,
        paths: &[String],
    ) -> Result<()> {
        for path in paths {
            let native = native_path(Path::new(path));
            let index_path = PathBuf::from(path);
            match target.get(path) {
                Some((oid, mode)) => {
                    worktree.write_blob(&native, oid, *mode)?;
                    idx.remove(&index_path);
                    idx.add(worktree.stat_entry(&native, index_path, *oid, *mode)?);
                }
                None => {
                    worktree.remove(&native)?;
                    idx.remove(&index_path);
                }
            }
        }
        Ok(())
    }
}

/// The result that moves from one tree to another (a two-way merge).
pub(crate) fn two_way(from: &Flat, to: &Flat) -> TreeMerge {
    let mut result = TreeMerge::default();
    for path in from.keys().chain(to.keys()) {
        let entry = to.get(path).copied();
        if from.get(path).copied() != entry {
            result.paths.insert(path.clone(), Resolution::Clean(entry));
        }
    }
    result
}

/// The stage 0 entries of an index as a flattened tree (intent-to-add
/// entries have no content and are left out).
pub(crate) fn index_flat(idx: &Index) -> Flat {
    idx.entries()
        .iter()
        .filter(|e| e.stage() == 0 && !e.intent_to_add())
        .map(|e| {
            (
                String::from_utf8_lossy(&crate::index::path_key(e.path())).into_owned(),
                (*e.oid(), e.mode()),
            )
        })
        .collect()
}

/// Paths where the index (stage 0) differs from the tree.
pub(crate) fn staged_changes(idx: &Index, tree: &Flat) -> Vec<PathBuf> {
    let mut changed = Vec::new();
    let mut seen = HashSet::new();
    for entry in idx.entries() {
        let key = String::from_utf8_lossy(&crate::index::path_key(entry.path())).into_owned();
        seen.insert(key.clone());
        if tree.get(&key).copied() != Some((*entry.oid(), entry.mode())) || entry.intent_to_add() {
            changed.push(PathBuf::from(key));
        }
    }
    for path in tree.keys() {
        if !seen.contains(path) {
            changed.push(PathBuf::from(path));
        }
    }
    changed
}
