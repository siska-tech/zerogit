//! Reflogs (`.git/logs/`).
//!
//! Each line records one update of a reference:
//! `<old-oid> <new-oid> <name> <<email>> <time> <tz>\t<message>`.
//! Which references are logged follows `core.logAllRefUpdates`: by default
//! (true in a repository with a work tree) `HEAD`, `refs/heads/`,
//! `refs/remotes/` and `refs/notes/`; `always` logs every reference; with
//! false only references whose log already exists are logged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::infra::LockFile;
use crate::objects::{Commit, ObjectType, Oid, Signature, TagObject};
use crate::repository::Repository;

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
            if !line.is_empty() {
                entries.push(parse_line(refname, number, line)?);
            }
        }
        entries.reverse();
        Ok(entries)
    }
}

/// Parses line `number` (from 0) of the reflog of `refname`.
fn parse_line(refname: &str, number: usize, line: &[u8]) -> Result<ReflogEntry> {
    let line = String::from_utf8_lossy(line);
    let invalid = || Error::InvalidObject {
        oid: format!("logs/{}", refname),
        reason: format!("malformed reflog line {}", number + 1),
    };
    let (head, message) = line.split_once('\t').unwrap_or((&line, ""));
    if head.len() < 82 || !head.is_char_boundary(82) {
        return Err(invalid());
    }
    let old_oid = Oid::from_hex(&head[..40]).map_err(|_| invalid())?;
    let new_oid = Oid::from_hex(&head[41..81]).map_err(|_| invalid())?;
    let committer = Signature::parse(&head[82..]).map_err(|_| invalid())?;
    Ok(ReflogEntry {
        old_oid,
        new_oid,
        committer,
        message: message.to_owned(),
    })
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
        crate::infra::write_locked(self.path(refname), content.as_bytes())
    }
}

/// When reflog entries expire, for [`Repository::reflog_expire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflogExpiry {
    /// No entry expires (`never`).
    Never,
    /// Entries recorded before this time expire.
    Before(SystemTime),
    /// Every entry expires, even one dated in the future (`now`, `all`).
    All,
}

impl ReflogExpiry {
    /// Parses an expiry as `git reflog expire --expire` and
    /// `gc.reflogExpire` take it: `never` (or `false`), `now` (or `all`),
    /// a relative date such as `90.days.ago` or `3 weeks ago`, or a date.
    ///
    /// # Errors
    ///
    /// `Error::InvalidDate` if `value` is none of these.
    pub fn parse(value: &str) -> Result<Self> {
        Self::parse_at(value, whole_seconds(SystemTime::now()))
    }

    fn parse_at(value: &str, now: SystemTime) -> Result<Self> {
        // As in Git, `now` means every entry rather than the current time:
        // a reflog records only the past.
        if matches!(value.trim().to_ascii_lowercase().as_str(), "now" | "all") {
            return Ok(ReflogExpiry::All);
        }
        Ok(match crate::gc::parse_expire(value, now)? {
            crate::gc::Expire::Never => ReflogExpiry::Never,
            crate::gc::Expire::Before(time) => ReflogExpiry::Before(time),
        })
    }

    /// The time (in nanoseconds since the epoch) before which entries
    /// expire, so that `Never` <= any time <= `All`.
    fn cutoff(self) -> i128 {
        match self {
            ReflogExpiry::Never => i128::MIN,
            ReflogExpiry::All => i128::MAX,
            ReflogExpiry::Before(time) => match time.duration_since(UNIX_EPOCH) {
                Ok(after) => after.as_nanos() as i128,
                Err(e) => -(e.duration().as_nanos() as i128),
            },
        }
    }
}

/// `time` without its fraction of a second, as Git keeps times.
fn whole_seconds(time: SystemTime) -> SystemTime {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    UNIX_EPOCH + Duration::from_secs(seconds)
}

const DAY: u64 = 24 * 3600;

/// The expiry settings of `gc.reflogExpire`, `gc.reflogExpireUnreachable`
/// and their `gc.<pattern>.*` forms.
struct ExpirePolicy {
    total: ReflogExpiry,
    unreachable: ReflogExpiry,
    /// Patterns in the order they first appear; a key not set for a
    /// pattern never expires, as in Git.
    patterns: Vec<(String, ReflogExpiry, ReflogExpiry)>,
}

impl ExpirePolicy {
    fn from_config(config: &Config, now: SystemTime) -> Result<Self> {
        let mut policy = ExpirePolicy {
            total: ReflogExpiry::Before(now - Duration::from_secs(90 * DAY)),
            unreachable: ReflogExpiry::Before(now - Duration::from_secs(30 * DAY)),
            patterns: Vec::new(),
        };
        for (pattern, key, value) in config.section_in_order("gc") {
            let unreachable = match key {
                "reflogexpire" => false,
                "reflogexpireunreachable" => true,
                _ => continue,
            };
            let expiry = ReflogExpiry::parse_at(value, now)?;
            let (total, unreach) = if pattern.is_empty() {
                (&mut policy.total, &mut policy.unreachable)
            } else {
                let index = match policy.patterns.iter().position(|(p, _, _)| p == pattern) {
                    Some(index) => index,
                    None => {
                        policy.patterns.push((
                            pattern.to_owned(),
                            ReflogExpiry::Never,
                            ReflogExpiry::Never,
                        ));
                        policy.patterns.len() - 1
                    }
                };
                let (_, total, unreach) = &mut policy.patterns[index];
                (total, unreach)
            };
            *if unreachable { unreach } else { total } = expiry;
        }
        Ok(policy)
    }

    /// The expiries of `refname`: the given ones, else those of the first
    /// matching pattern, else (for the stash, never) the defaults.
    fn for_ref(
        &self,
        refname: &str,
        total: Option<ReflogExpiry>,
        unreachable: Option<ReflogExpiry>,
    ) -> (ReflogExpiry, ReflogExpiry) {
        let (default_total, default_unreachable) = self
            .patterns
            .iter()
            .find(|(pattern, _, _)| {
                crate::ignore::wildmatch_any(pattern.as_bytes(), refname.as_bytes())
            })
            .map(|(_, total, unreach)| (*total, *unreach))
            .unwrap_or(if refname == "refs/stash" {
                (ReflogExpiry::Never, ReflogExpiry::Never)
            } else {
                (self.total, self.unreachable)
            });
        (
            total.unwrap_or(default_total),
            unreachable.unwrap_or(default_unreachable),
        )
    }
}

/// Checks that the `gc.*reflogExpire*` settings can be parsed.
pub(crate) fn check_expiry_config(config: &Config) -> Result<()> {
    ExpirePolicy::from_config(config, SystemTime::now()).map(|_| ())
}

impl Repository {
    /// Removes old reflog entries from every reflog, like
    /// `git reflog expire --all`, and returns how many were removed.
    ///
    /// An entry expires when it is older than `expire`, or older than
    /// `expire_unreachable` and its old or new value is a commit that the
    /// reference's current value cannot reach (for HEAD, that no reference
    /// under `refs/` can reach). `None` takes the expiry from the
    /// configuration, as Git does without `--expire` / `--expire-unreachable`:
    /// `gc.<pattern>.reflogExpire` / `gc.<pattern>.reflogExpireUnreachable`
    /// for the first pattern (a glob in which `*` also matches `/`) that
    /// matches the reference, then `gc.reflogExpire` /
    /// `gc.reflogExpireUnreachable` (by default 90 and 30 days). Unless a
    /// pattern matches, the stash (`refs/stash`) never expires.
    ///
    /// As in Git (without `--rewrite`), the remaining entries are kept as
    /// they are, and a reflog left without entries is kept as an empty file.
    /// Each reference and its reflog are locked as Git locks them, and every
    /// lock is taken before any reflog is written. Only the reflogs under
    /// `.git/logs/` are expired.
    ///
    /// # Errors
    ///
    /// Nothing is changed when an error is returned:
    /// - `Error::Locked` if a reference or reflog is locked.
    /// - `Error::InvalidDate` if a `gc.*reflogExpire*` setting cannot be
    ///   parsed.
    /// - `Error::InvalidObject` if a reflog line is malformed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{ReflogExpiry, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // As `git gc` does it.
    /// repo.reflog_expire(None, None).unwrap();
    /// // `git reflog expire --all --expire=now --expire-unreachable=now`.
    /// repo.reflog_expire(Some(ReflogExpiry::All), Some(ReflogExpiry::All))
    ///     .unwrap();
    /// ```
    pub fn reflog_expire(
        &self,
        expire: Option<ReflogExpiry>,
        expire_unreachable: Option<ReflogExpiry>,
    ) -> Result<usize> {
        let now = whole_seconds(SystemTime::now());
        let policy = ExpirePolicy::from_config(&self.config()?, now)?;
        let reflog = self.reflog_writer()?;
        let mut names = Vec::new();
        if reflog.path("HEAD").is_file() {
            names.push("HEAD".to_owned());
        }
        crate::refs::RefStore::collect_refs_recursive(
            &reflog.logs_dir.join("refs"),
            "refs",
            &mut names,
        )?;

        let mut parents: HashMap<Oid, Vec<Oid>> = HashMap::new();
        let mut removed = 0;
        let mut pending = Vec::new();
        for name in names {
            let (total, unreachable) = policy.for_ref(&name, expire, expire_unreachable);
            let (total, unreachable) = (total.cutoff(), unreachable.cutoff());
            // The reference's lock, then its reflog's, as Git takes them.
            let mut ref_lock = LockFile::acquire(self.git_dir().join(&name))?;
            ref_lock.close()?;
            let path = reflog.path(&name);
            let mut log_lock = LockFile::acquire(&path)?;
            let content = match fs::read(&path) {
                Ok(content) => content,
                // Deleted meanwhile.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };

            // What the entries' commits must be reachable from, or `None`
            // when every entry older than `unreachable` expires (Git's
            // unreachable_expire_kind).
            let tips: Option<Vec<Oid>> = if unreachable <= total {
                None
            } else if name == "HEAD" {
                let mut tips = Vec::new();
                for (_, oid) in self.references()? {
                    tips.extend(self.commit_of(&oid)?);
                }
                Some(tips)
            } else {
                match self.find_reference(&name)? {
                    Some(oid) => self.commit_of(&oid)?.map(|commit| vec![commit]),
                    None => None,
                }
            };
            let mut reachable: Option<HashSet<Oid>> = None;

            let mut kept = Vec::new();
            let mut expired = 0;
            for (number, line) in content.split(|&b| b == b'\n').enumerate() {
                if line.is_empty() {
                    continue;
                }
                let entry = parse_line(&name, number, line)?;
                let time = i128::from(entry.committer.timestamp()) * 1_000_000_000;
                let expires = if time < total {
                    true
                } else if time < unreachable {
                    match &tips {
                        None => true,
                        Some(tips) => {
                            if reachable.is_none() {
                                reachable = Some(self.commit_ancestry(tips, &mut parents)?);
                            }
                            let reachable = reachable.as_ref().expect("computed above");
                            let mut unreached = false;
                            for oid in [&entry.old_oid, &entry.new_oid] {
                                if let Some(commit) = self.commit_of(oid)? {
                                    unreached |= !reachable.contains(&commit);
                                }
                            }
                            unreached
                        }
                    }
                } else {
                    false
                };
                if expires {
                    expired += 1;
                } else {
                    kept.extend_from_slice(line);
                    kept.push(b'\n');
                }
            }
            if expired == 0 {
                continue;
            }
            log_lock.write_all(&kept)?;
            log_lock.close()?;
            removed += expired;
            pending.push((ref_lock, log_lock));
        }

        for (ref_lock, log_lock) in pending {
            log_lock.commit()?;
            drop(ref_lock);
        }
        Ok(removed)
    }

    /// The commit `oid` is or (as an annotated tag) points to, or `None`
    /// for another kind of object or a missing one.
    fn commit_of(&self, oid: &Oid) -> Result<Option<Oid>> {
        let mut oid = *oid;
        if oid == zero_oid() {
            return Ok(None);
        }
        loop {
            let raw = match self.object_store().read(&oid) {
                Ok(raw) => raw,
                Err(Error::ObjectNotFound(_)) => return Ok(None),
                Err(e) => return Err(e),
            };
            match raw.object_type {
                ObjectType::Commit => return Ok(Some(oid)),
                ObjectType::Tag => oid = *TagObject::parse(raw)?.object(),
                _ => return Ok(None),
            }
        }
    }

    /// The commits reachable from `tips` (a missing commit ends its line),
    /// with each commit's parents cached in `parents`.
    fn commit_ancestry(
        &self,
        tips: &[Oid],
        parents: &mut HashMap<Oid, Vec<Oid>>,
    ) -> Result<HashSet<Oid>> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<Oid> = tips.iter().copied().collect();
        while let Some(oid) = queue.pop_front() {
            if !seen.insert(oid) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = parents.entry(oid) {
                slot.insert(match self.object_store().read(&oid) {
                    Ok(raw) => Commit::parse(oid, raw)?.parents().to_vec(),
                    Err(Error::ObjectNotFound(_)) => Vec::new(),
                    Err(e) => return Err(e),
                });
            }
            queue.extend(parents[&oid].iter().copied());
        }
        Ok(seen)
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

    #[test]
    fn test_expiry_policy() {
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let ago = |days: u64| ReflogExpiry::Before(now - Duration::from_secs(days * DAY));
        assert_eq!(
            ReflogExpiry::parse_at("now", now).unwrap(),
            ReflogExpiry::All
        );
        assert_eq!(
            ReflogExpiry::parse_at("never", now).unwrap(),
            ReflogExpiry::Never
        );
        assert_eq!(ReflogExpiry::parse_at("3.days.ago", now).unwrap(), ago(3));
        assert!(ReflogExpiry::Never.cutoff() < ago(3).cutoff());
        assert!(ago(3).cutoff() < ReflogExpiry::All.cutoff());

        let policy = ExpirePolicy::from_config(&Config::new(), now).unwrap();
        assert_eq!(policy.for_ref("HEAD", None, None), (ago(90), ago(30)));
        let never = (ReflogExpiry::Never, ReflogExpiry::Never);
        assert_eq!(policy.for_ref("refs/stash", None, None), never);

        let config = Config::from_str(
            "[gc]\n\treflogExpire = 10.days.ago\n\
             [gc \"refs/*\"]\n\treflogExpireUnreachable = now\n\
             [gc \"refs/heads/*\"]\n\treflogExpire = never\n",
        )
        .unwrap();
        let policy = ExpirePolicy::from_config(&config, now).unwrap();
        assert_eq!(policy.for_ref("HEAD", None, None), (ago(10), ago(30)));
        // The first pattern matches, across slashes; its unset key is never.
        assert_eq!(
            policy.for_ref("refs/heads/a/b", None, None),
            (ReflogExpiry::Never, ReflogExpiry::All)
        );
        assert_eq!(
            policy.for_ref("refs/stash", Some(ago(1)), None),
            (ago(1), ReflogExpiry::All)
        );
    }
}
