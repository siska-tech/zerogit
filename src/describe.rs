//! Naming commits after the nearest tag (`git describe`).
//!
//! [`Repository::describe`] names HEAD, and [`Repository::describe_revision`]
//! any commit, after the most recent tag it can reach:
//! `v1.0-3-gabc1234` is three commits after the tag `v1.0`, at the commit
//! whose ID starts with `abc1234`; a tagged commit is just `v1.0`.

use std::collections::{BinaryHeap, HashMap};
use std::fmt;

use crate::error::{Error, Result};
use crate::objects::{ObjectType, Oid, TagObject};
use crate::repository::Repository;
use crate::status::FileStatus;

/// Options for [`Repository::describe`] and
/// [`Repository::describe_revision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeOptions {
    tags: bool,
    all: bool,
    long: bool,
    abbrev: Option<usize>,
    always: bool,
    first_parent: bool,
    candidates: usize,
    patterns: Vec<String>,
    exclude: Vec<String>,
    dirty: Option<String>,
}

impl Default for DescribeOptions {
    fn default() -> Self {
        DescribeOptions {
            tags: false,
            all: false,
            long: false,
            abbrev: None,
            always: false,
            first_parent: false,
            candidates: 10,
            patterns: Vec::new(),
            exclude: Vec::new(),
            dirty: None,
        }
    }
}

impl DescribeOptions {
    /// The default options: annotated tags only, the long form only when
    /// the commit is not tagged, and abbreviated IDs as `core.abbrev` says.
    pub fn new() -> Self {
        Self::default()
    }

    /// Uses lightweight tags too (`--tags`); annotated ones still win on
    /// the same commit.
    pub fn tags(mut self, tags: bool) -> Self {
        self.tags = tags;
        self
    }

    /// Uses any reference (`--all`), named as `tags/v1`, `heads/main` or
    /// `remotes/origin/main`.
    pub fn all(mut self, all: bool) -> Self {
        self.all = all;
        self
    }

    /// Always gives the long form (`--long`), `v1.0-0-gabc1234` for a
    /// tagged commit.
    pub fn long(mut self, long: bool) -> Self {
        self.long = long;
        self
    }

    /// Abbreviates the commit's ID to at least `digits` hexadecimal digits
    /// (`--abbrev`; at least 4), longer when that is not unique; 0 leaves
    /// out the distance and the ID, giving only the tag.
    pub fn abbrev(mut self, digits: usize) -> Self {
        self.abbrev = Some(digits);
        self
    }

    /// Falls back to the abbreviated ID when no tag can describe the commit
    /// (`--always`).
    pub fn always(mut self, always: bool) -> Self {
        self.always = always;
        self
    }

    /// Follows only the first parent of merges (`--first-parent`).
    pub fn first_parent(mut self, first_parent: bool) -> Self {
        self.first_parent = first_parent;
        self
    }

    /// Considers at most `count` tags while searching (`--candidates`,
    /// default 10); 0 accepts only a tag on the commit itself.
    pub fn candidates(mut self, count: usize) -> Self {
        self.candidates = count;
        self
    }

    /// Only considers tags matching one of the glob patterns (`--match`;
    /// `*` also matches `/`).
    pub fn patterns<S: AsRef<str>>(mut self, patterns: &[S]) -> Self {
        self.patterns = patterns.iter().map(|p| p.as_ref().to_owned()).collect();
        self
    }

    /// Does not consider tags matching one of the glob patterns
    /// (`--exclude`).
    pub fn exclude<S: AsRef<str>>(mut self, patterns: &[S]) -> Self {
        self.exclude = patterns.iter().map(|p| p.as_ref().to_owned()).collect();
        self
    }

    /// Appends `suffix` (Git's default is `-dirty`) when the work tree or
    /// the index has changes to tracked files (`--dirty`). Only for
    /// [`Repository::describe`], which describes HEAD.
    pub fn dirty<S: Into<String>>(mut self, suffix: Option<S>) -> Self {
        self.dirty = suffix.map(Into::into);
        self
    }
}

/// A commit named by [`Repository::describe`]; its [`Display`](fmt::Display)
/// form is what `git describe` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    tag: Option<String>,
    distance: usize,
    commit: Oid,
    abbreviated: String,
    suffix: bool,
    dirty: Option<String>,
}

impl Description {
    /// The tag (or with [`DescribeOptions::all`], the reference) the commit
    /// is named after; `None` when it fell back to the ID
    /// ([`DescribeOptions::always`]).
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// How many commits the described commit has that the tag does not
    /// reach (0 for the tagged commit).
    pub fn distance(&self) -> usize {
        self.distance
    }

    /// The described commit.
    pub fn commit(&self) -> &Oid {
        &self.commit
    }

    /// Whether the dirty suffix was added.
    pub fn is_dirty(&self) -> bool {
        self.dirty.is_some()
    }
}

impl fmt::Display for Description {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.tag {
            Some(tag) if self.suffix => {
                write!(f, "{}-{}-g{}", tag, self.distance, self.abbreviated)?
            }
            Some(tag) => f.write_str(tag)?,
            None => f.write_str(&self.abbreviated)?,
        }
        if let Some(dirty) = &self.dirty {
            f.write_str(dirty)?;
        }
        Ok(())
    }
}

/// A name for a commit: a tag (or reference) pointing to it.
struct Name {
    name: String,
    /// 2 for an annotated tag, 1 for a lightweight one, 0 for another
    /// reference.
    priority: u8,
    /// The tag object of an annotated tag, read when tags compete.
    tag: Option<Oid>,
}

/// A tag met while walking back from the described commit.
struct Candidate {
    name: usize,
    /// Commits the walk reached that the tag does not.
    depth: usize,
    /// The bit marking the commits the tag reaches.
    flag: u64,
    found_order: usize,
}

/// A commit waiting in the walk: the newest first, then in insertion order.
#[derive(PartialEq, Eq)]
struct Queued {
    time: i64,
    order: std::cmp::Reverse<u64>,
    oid: Oid,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.time, self.order).cmp(&(other.time, other.order))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The commits of a walk by commit date, with the flags of each commit:
/// bit 0 marks a commit as seen, the others which candidates reach it.
struct Walk<'a> {
    repo: &'a Repository,
    queue: BinaryHeap<Queued>,
    flags: HashMap<Oid, u64>,
    parents: HashMap<Oid, (i64, Vec<Oid>)>,
    counter: u64,
    first_parent: bool,
}

const SEEN: u64 = 1;
/// The most candidates Git tracks at once.
const MAX_CANDIDATES: usize = 27;

impl<'a> Walk<'a> {
    /// The commit time and parents of `oid`, read once.
    fn commit(&mut self, oid: &Oid) -> Result<&(i64, Vec<Oid>)> {
        if !self.parents.contains_key(oid) {
            let commit = self.repo.commit(&oid.to_hex())?;
            let mut parents = commit.parents().to_vec();
            if self.first_parent {
                parents.truncate(1);
            }
            self.parents
                .insert(*oid, (commit.committer().timestamp(), parents));
        }
        Ok(&self.parents[oid])
    }

    fn push(&mut self, oid: Oid) -> Result<()> {
        let time = self.commit(&oid)?.0;
        self.counter += 1;
        self.queue.push(Queued {
            time,
            order: std::cmp::Reverse(self.counter),
            oid,
        });
        Ok(())
    }

    fn flags(&self, oid: &Oid) -> u64 {
        self.flags.get(oid).copied().unwrap_or(0)
    }

    /// Queues the parents of `oid` not seen yet and passes its flags on.
    fn visit_parents(&mut self, oid: &Oid) -> Result<()> {
        let flags = self.flags(oid);
        let parents = self.commit(oid)?.1.clone();
        for parent in parents {
            if self.flags(&parent) & SEEN == 0 {
                self.push(parent)?;
            }
            *self.flags.entry(parent).or_insert(0) |= flags;
        }
        Ok(())
    }
}

impl Repository {
    /// Names HEAD after the nearest tag, like `git describe`.
    ///
    /// See [`Repository::describe_revision`]. With
    /// [`DescribeOptions::dirty`], the suffix is appended when tracked
    /// files differ from HEAD in the index or the work tree (untracked
    /// files do not count), as `git describe --dirty` does.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{DescribeOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let options = DescribeOptions::new().tags(true).dirty(Some("-dirty"));
    /// println!("{}", repo.describe(&options).unwrap()); // v1.0-3-gabc1234-dirty
    /// ```
    ///
    /// # Errors
    ///
    /// The errors of [`Repository::describe_revision`].
    pub fn describe(&self, options: &DescribeOptions) -> Result<Description> {
        let head = self
            .optional_head_oid()?
            .ok_or_else(|| Error::RefNotFound("HEAD".to_owned()))?;
        let mut description = self.describe_commit(head, options)?;
        if let Some(suffix) = &options.dirty {
            let dirty = self
                .status()?
                .iter()
                .any(|entry| entry.status() != FileStatus::Untracked);
            if dirty {
                description.dirty = Some(suffix.clone());
            }
        }
        Ok(description)
    }

    /// Names a commit after the nearest tag it can reach, like
    /// `git describe <revision>`.
    ///
    /// The tag reaching the commit through the fewest commits it does not
    /// itself reach wins (when that is a tie, the one met first walking
    /// back from the commit, newest commits first). By default only
    /// annotated tags count; [`DescribeOptions::tags`] adds lightweight
    /// ones and [`DescribeOptions::all`] any reference. A commit that a
    /// tag points to is named by that tag alone (when several do, an
    /// annotated tag over a lightweight one, the most recently tagged of
    /// annotated ones, otherwise the first by name). The abbreviated ID
    /// has `core.abbrev` digits (by default 7, more in large
    /// repositories, as in Git) or more to be unique.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRevision` / `Error::RefNotFound` if `revision`
    ///   cannot be resolved, `Error::TypeMismatch` if it is not a commit.
    /// - `Error::InvalidRevision` with [`DescribeOptions::dirty`], which
    ///   only [`Repository::describe`] takes.
    /// - `Error::NoDescription` if no tag reaches the commit (and
    ///   [`DescribeOptions::always`] is off).
    pub fn describe_revision(
        &self,
        revision: &str,
        options: &DescribeOptions,
    ) -> Result<Description> {
        if options.dirty.is_some() {
            return Err(Error::InvalidRevision {
                revision: revision.to_owned(),
                reason: "the dirty suffix applies to HEAD only".to_owned(),
            });
        }
        let oid = self.rev_parse(revision)?;
        let commit = self.peel_to(oid, ObjectType::Commit)?;
        self.describe_commit(commit, options)
    }

    fn describe_commit(&self, target: Oid, options: &DescribeOptions) -> Result<Description> {
        let (names, by_commit) = self.describe_names(options)?;
        let abbrev = match options.abbrev {
            Some(0) => 0,
            Some(digits) => digits.clamp(4, 40),
            None => self.default_abbrev()?,
        };
        let abbreviate = |oid: &Oid| -> Result<String> {
            if abbrev == 0 {
                Ok(oid.to_hex())
            } else {
                self.unique_abbrev(oid, abbrev)
            }
        };
        let any_name = options.tags || options.all;

        // A name for the commit itself.
        if let Some(&index) = by_commit.get(&target) {
            let name = &names[index];
            if any_name || name.priority == 2 {
                // As in Git, the long form names what an annotated tag
                // points to (another tag, for a tag of a tag).
                let named = match name.tag {
                    Some(tag) => *TagObject::parse(self.object_store().read(&tag)?)?.object(),
                    None => target,
                };
                return Ok(Description {
                    tag: Some(name.name.clone()),
                    distance: 0,
                    commit: target,
                    abbreviated: if options.long && abbrev > 0 {
                        abbreviate(&named)?
                    } else {
                        String::new()
                    },
                    suffix: options.long && abbrev > 0,
                    dirty: None,
                });
            }
        }
        // Each candidate marks the commits it reaches with its own bit.
        // Git keeps one flag bit per candidate: at most 27.
        let max_candidates = options.candidates.min(MAX_CANDIDATES);
        let no_description = |unannotated: bool| Error::NoDescription {
            commit: target.to_hex(),
            unannotated_tags: unannotated,
        };
        if max_candidates == 0 {
            if options.always {
                return Ok(self.id_description(target, abbreviate(&target)?));
            }
            return Err(no_description(false));
        }

        let mut walk = Walk {
            repo: self,
            queue: BinaryHeap::new(),
            flags: HashMap::new(),
            parents: HashMap::new(),
            counter: 0,
            first_parent: options.first_parent,
        };
        walk.flags.insert(target, SEEN);
        walk.push(target)?;
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut seen_commits = 0;
        let mut annotated = 0;
        let mut unannotated = 0;
        let mut gave_up_on: Option<Oid> = None;
        while let Some(Queued { oid, .. }) = walk.queue.pop() {
            seen_commits += 1;
            if let Some(&index) = by_commit.get(&oid) {
                if !any_name && names[index].priority < 2 {
                    unannotated += 1;
                } else if candidates.len() < max_candidates {
                    let flag = 1u64 << (candidates.len() + 1);
                    candidates.push(Candidate {
                        name: index,
                        depth: seen_commits - 1,
                        flag,
                        found_order: candidates.len() + 1,
                    });
                    *walk.flags.entry(oid).or_insert(0) |= flag;
                    if names[index].priority == 2 {
                        annotated += 1;
                    }
                } else {
                    gave_up_on = Some(oid);
                    break;
                }
            }
            let flags = walk.flags(&oid);
            for candidate in &mut candidates {
                if flags & candidate.flag == 0 {
                    candidate.depth += 1;
                }
            }
            // The best candidates already cover the last path.
            if annotated > 0 && walk.queue.is_empty() {
                break;
            }
            walk.visit_parents(&oid)?;
        }

        if candidates.is_empty() {
            if options.always {
                return Ok(self.id_description(target, abbreviate(&target)?));
            }
            return Err(no_description(unannotated > 0));
        }
        candidates.sort_by_key(|c| (c.depth, c.found_order));
        if let Some(oid) = gave_up_on {
            walk.push(oid)?;
        }
        // Count the commits the best one does not reach among those left.
        let best_flag = candidates[0].flag;
        let mut extra = 0;
        while let Some(Queued { oid, .. }) = walk.queue.pop() {
            if walk.flags(&oid) & best_flag != 0 {
                // Stop once everything left is reached by the best one.
                if walk
                    .queue
                    .iter()
                    .all(|queued| walk.flags(&queued.oid) & best_flag != 0)
                {
                    break;
                }
            } else {
                extra += 1;
            }
            walk.visit_parents(&oid)?;
        }
        let best = &candidates[0];
        let distance = best.depth + extra;
        Ok(Description {
            tag: Some(names[best.name].name.clone()),
            distance,
            commit: target,
            abbreviated: if abbrev > 0 {
                abbreviate(&target)?
            } else {
                String::new()
            },
            suffix: abbrev > 0,
            dirty: None,
        })
    }

    fn id_description(&self, commit: Oid, abbreviated: String) -> Description {
        Description {
            tag: None,
            distance: 0,
            commit,
            abbreviated,
            suffix: false,
            dirty: None,
        }
    }

    /// The names describe may use, and which name each commit has.
    fn describe_names(
        &self,
        options: &DescribeOptions,
    ) -> Result<(Vec<Name>, HashMap<Oid, usize>)> {
        let mut names: Vec<Name> = Vec::new();
        let mut by_commit: HashMap<Oid, usize> = HashMap::new();
        let matches = |text: &str, patterns: &[String]| {
            patterns
                .iter()
                .any(|p| crate::ignore::wildmatch_any(p.as_bytes(), text.as_bytes()))
        };
        let mut tag_dates: HashMap<Oid, i64> = HashMap::new();
        // References come sorted by name, as Git lists them.
        for (refname, oid) in self.references()? {
            let (is_tag, to_match) = match refname.strip_prefix("refs/tags/") {
                Some(tag) => (true, tag),
                None if !options.all => continue,
                None => {
                    let filtered = !options.patterns.is_empty() || !options.exclude.is_empty();
                    match refname
                        .strip_prefix("refs/heads/")
                        .or_else(|| refname.strip_prefix("refs/remotes/"))
                    {
                        Some(rest) => (false, rest),
                        // Only known kinds of references are matched.
                        None if filtered => continue,
                        None => (false, refname.as_str()),
                    }
                }
            };
            if matches(to_match, &options.exclude) {
                continue;
            }
            if !options.patterns.is_empty() && !matches(to_match, &options.patterns) {
                continue;
            }
            let peeled = match self.peel_to(oid, ObjectType::Commit) {
                Ok(commit) => commit,
                // Tags of trees and blobs name no commit.
                Err(Error::TypeMismatch { .. }) | Err(Error::ObjectNotFound(_)) => continue,
                Err(e) => return Err(e),
            };
            let annotated = self.object_store().read(&oid)?.object_type == ObjectType::Tag;
            let priority = if annotated {
                2
            } else if is_tag {
                1
            } else {
                0
            };
            let name = if options.all {
                refname["refs/".len()..].to_owned()
            } else {
                refname["refs/tags/".len()..].to_owned()
            };
            let replace = match by_commit.get(&peeled) {
                None => true,
                Some(&current) => {
                    let current = &names[current];
                    if current.priority != priority {
                        current.priority < priority
                    } else if priority == 2 {
                        // The most recently tagged annotated tag.
                        let mut date = |tag: Oid| -> Result<i64> {
                            if let Some(date) = tag_dates.get(&tag) {
                                return Ok(*date);
                            }
                            let raw = self.object_store().read(&tag)?;
                            let date = TagObject::parse(raw)?.tagger().timestamp();
                            tag_dates.insert(tag, date);
                            Ok(date)
                        };
                        let current_tag = current.tag.expect("annotated");
                        date(current_tag)? < date(oid)?
                    } else {
                        false
                    }
                }
            };
            if replace {
                names.push(Name {
                    name,
                    priority,
                    tag: annotated.then_some(oid),
                });
                by_commit.insert(peeled, names.len() - 1);
            }
        }
        Ok((names, by_commit))
    }

    /// The default length of abbreviated IDs: `core.abbrev`, or as Git
    /// computes it for `auto` from the number of packed objects (at least
    /// 7).
    pub(crate) fn default_abbrev(&self) -> Result<usize> {
        let config = self.config()?;
        match config.get("core", "abbrev") {
            Some(value) if value.eq_ignore_ascii_case("no") || value == "false" => return Ok(40),
            Some(value) if !value.eq_ignore_ascii_case("auto") => {
                return Ok(config.get_int("core", "abbrev")?.clamp(4, 40) as usize);
            }
            _ => {}
        }
        let count: u64 = self
            .object_store()
            .pack_files()?
            .iter()
            .map(|pack| pack.index().len() as u64)
            .sum();
        // About 2^bits objects make a collision likely at 2^(bits/2): half
        // the bits, in hexadecimal digits, rounded up.
        let bits = 64 - count.leading_zeros() as usize;
        Ok(((bits + 1) / 2).max(7))
    }

    /// The shortest prefix of `oid`, of at least `min` digits, that no
    /// other object shares.
    pub(crate) fn unique_abbrev(&self, oid: &Oid, min: usize) -> Result<String> {
        let hex = oid.to_hex();
        let mut len = min.clamp(4, 40);
        while len < 40 {
            let others = self
                .object_store()
                .find_objects_by_prefix(&hex[..len])?
                .into_iter()
                .any(|other| other != *oid);
            if !others {
                break;
            }
            len += 1;
        }
        Ok(hex[..len].to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_display_like_git() {
        let oid = Oid::from_bytes([0xab; 20]);
        let mut d = Description {
            tag: Some("v1.0".into()),
            distance: 3,
            commit: oid,
            abbreviated: "abababa".into(),
            suffix: true,
            dirty: None,
        };
        assert_eq!(d.to_string(), "v1.0-3-gabababa");
        d.dirty = Some("-dirty".into());
        assert_eq!(d.to_string(), "v1.0-3-gabababa-dirty");
        d.suffix = false;
        d.dirty = None;
        assert_eq!(d.to_string(), "v1.0");
        d.tag = None;
        assert_eq!(d.to_string(), "abababa");
    }

    #[test]
    fn abbreviations_grow_until_unique() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let store = repo.object_store();
        let first = store.write(ObjectType::Blob, b"first").unwrap();
        assert_eq!(repo.unique_abbrev(&first, 4).unwrap(), first.to_hex()[..4]);
        // Another blob sharing the first four digits.
        let prefix = &first.to_hex()[..4];
        let other = (0u32..)
            .map(|i| i.to_string().into_bytes())
            .find(|content| {
                Oid::from_bytes(crate::infra::hash_object("blob", content)).to_hex()[..4] == *prefix
            })
            .unwrap();
        let other = store.write(ObjectType::Blob, &other).unwrap();
        let abbrev = repo.unique_abbrev(&first, 4).unwrap();
        assert!(abbrev.len() > 4 && first.to_hex().starts_with(&abbrev));
        assert!(!other.to_hex().starts_with(&abbrev));
        assert_eq!(repo.unique_abbrev(&first, 7).unwrap(), first.to_hex()[..7]);
    }
}
