//! Housekeeping: packing references (`git pack-refs`) and objects
//! (`git repack`), pruning unreachable objects (`git prune`) and all of
//! them together (`git gc`).
//!
//! [`Repository::pack_refs`] moves loose references into `packed-refs` as
//! `git pack-refs --all` does, so a repository with many branches and tags
//! keeps few files under `.git/refs/`. [`Repository::repack`] puts every
//! reachable object into one pack, as `git repack -a -d` does, so objects
//! written loose one by one do not pile up. [`Repository::prune`] removes
//! unreachable loose objects once they expire. [`Repository::gc`] runs the
//! three after expiring old reflog entries
//! ([`Repository::reflog_expire`]), and [`Repository::gc_auto`] runs it only when there are many
//! loose objects or packs, as `git gc --auto` does.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};
use crate::infra::fs::remove_file;
use crate::infra::hash::Sha1State;
use crate::infra::{compress, crc32, write_file_atomic, LockFile};
use crate::objects::pack::indexer::{entry_header, type_code, write_index, IndexedObject};
use crate::objects::pack::{EntryKind, PackFile, RawEntry};
use crate::objects::{Commit, FileMode, ObjectType, Oid, TagObject, Tree};
use crate::refs::RefValue;
use crate::repository::Repository;

/// What [`Repository::repack`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepackSummary {
    pack: Option<PathBuf>,
    objects: usize,
    reused_deltas: usize,
    removed_packs: usize,
    removed_loose: usize,
    loosened: usize,
}

impl RepackSummary {
    /// The `.pack` file holding the reachable objects, or `None` if there
    /// were none to pack (outside packs kept with a `.keep` file).
    pub fn pack(&self) -> Option<&Path> {
        self.pack.as_deref()
    }

    /// The number of objects in the new pack.
    pub fn objects(&self) -> usize {
        self.objects
    }

    /// How many of them were copied as deltas from the old packs.
    pub fn reused_deltas(&self) -> usize {
        self.reused_deltas
    }

    /// The number of old packs removed.
    pub fn removed_packs(&self) -> usize {
        self.removed_packs
    }

    /// The number of loose objects removed because a pack now has them.
    pub fn removed_loose(&self) -> usize {
        self.removed_loose
    }

    /// The number of unreachable objects of the removed packs that were
    /// written as loose objects, to be pruned once they expire.
    pub fn loosened(&self) -> usize {
        self.loosened
    }
}

/// Files of a pack that go with it when it is removed (the index first,
/// which makes the pack invisible to readers).
const PACK_FILES: &[&str] = &["idx", "pack", "rev", "bitmap", "mtimes"];

/// Files outside `refs/` whose object IDs Git or zerogit may still need:
/// the state of merges, rebases, cherry-picks and fetches.
const STATE_FILES: &[&str] = &[
    "ORIG_HEAD",
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "AUTO_MERGE",
    "FETCH_HEAD",
    "rebase-merge/onto",
    "rebase-merge/orig-head",
    "rebase-merge/amend",
    "rebase-merge/stopped-sha",
    "rebase-merge/rewritten-list",
    "rebase-apply/onto",
    "rebase-apply/orig-head",
    "sequencer/head",
    "sequencer/abort-safety",
];

/// A pack being written to a temporary file, hashed as it goes.
struct PackWriter {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    sha1: Sha1State,
    offset: u64,
}

impl PackWriter {
    fn create(dir: &Path, count: u32) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = dir.join(format!("tmp_pack_zerogit_{}_{}", std::process::id(), nanos));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = PackWriter {
            path,
            file: Some(BufWriter::new(file)),
            sha1: Sha1State::new(),
            offset: 0,
        };
        let mut header = b"PACK".to_vec();
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&count.to_be_bytes());
        writer.write(&header)?;
        Ok(writer)
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .expect("open until finished")
            .write_all(data)?;
        self.sha1.update(data);
        self.offset += data.len() as u64;
        Ok(())
    }

    /// Writes the trailer and closes the file; returns the checksum.
    fn finish(mut self) -> Result<(PathBuf, [u8; 20])> {
        let checksum = std::mem::replace(&mut self.sha1, Sha1State::new()).finalize();
        let mut file = self.file.take().expect("open until finished");
        file.write_all(&checksum)?;
        let file = file.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.sync_all()?;
        Ok((std::mem::take(&mut self.path), checksum))
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        // Not finished: remove the partial file.
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Encodes the distance back to an OFS_DELTA base, as Git does.
fn ofs_distance(mut distance: u64) -> Vec<u8> {
    let mut bytes = vec![(distance & 0x7f) as u8];
    distance >>= 7;
    while distance > 0 {
        distance -= 1;
        bytes.push(0x80 | (distance & 0x7f) as u8);
        distance >>= 7;
    }
    bytes.reverse();
    bytes
}

/// Reads the object IDs at the start of each line of a state file (a
/// `rewritten-list` line has two).
fn oids_in(content: &str) -> Vec<Oid> {
    content
        .lines()
        .flat_map(|line| line.split_whitespace().take(2))
        .filter(|word| word.len() == 40)
        .filter_map(|word| Oid::from_hex(word).ok())
        .collect()
}

/// References Git keeps per work tree and never packs.
const PER_WORKTREE: &[&str] = &["refs/bisect/", "refs/worktree/", "refs/rewritten/"];

/// The header Git writes: every record that points to an annotated tag is
/// followed by the object it peels to, and the records are sorted.
const PACKED_REFS_HEADER: &str = "# pack-refs with: peeled fully-peeled sorted \n";

impl Repository {
    /// Moves every loose reference into `packed-refs`, like
    /// `git pack-refs --all`, and removes the loose files.
    ///
    /// Branches, tags, remote-tracking branches and other references under
    /// `refs/` are written to `packed-refs` in Git's format (sorted, with
    /// the peeled object after each annotated tag), then each loose file is
    /// deleted, along with the directories it leaves empty below
    /// `refs/<kind>/`. Symbolic references (such as
    /// `refs/remotes/origin/HEAD`), references kept per work tree
    /// (`refs/bisect/`, `refs/worktree/`, `refs/rewritten/`) and references
    /// to missing objects stay loose. Reflogs are kept.
    ///
    /// A loose reference that changes while packing (or whose lock is held
    /// by another process) is left loose, where it takes precedence over
    /// the packed value, so no update is lost.
    ///
    /// # Errors
    ///
    /// `Error::Locked` if `packed-refs` is locked; nothing is changed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.pack_refs().unwrap();
    /// ```
    pub fn pack_refs(&self) -> Result<()> {
        let path = self.git_dir().join("packed-refs");
        let store = self.ref_store();
        // Read packed-refs under its lock so a concurrent rewrite is not
        // lost.
        let mut lock = LockFile::acquire(&path)?;
        let mut refs: BTreeMap<String, Oid> = store.packed_refs()?;

        let mut loose_names = Vec::new();
        crate::refs::RefStore::collect_refs_recursive(
            &self.git_dir().join("refs"),
            "refs",
            &mut loose_names,
        )?;
        let mut packed_loose = Vec::new();
        for name in loose_names {
            if PER_WORKTREE.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let oid = match store.read_loose_ref(&name) {
                Ok(RefValue::Direct(oid)) => oid,
                Ok(RefValue::Symbolic(_)) => continue,
                // Removed meanwhile.
                Err(Error::RefNotFound(_)) => continue,
                Err(e) => return Err(e),
            };
            if !self.object_store().exists(&oid)? {
                continue;
            }
            refs.insert(name.clone(), oid);
            packed_loose.push((name, oid));
        }

        let mut content = String::from(PACKED_REFS_HEADER);
        for (name, oid) in &refs {
            content.push_str(&format!("{} {}\n", oid.to_hex(), name));
            if let Some(peeled) = self.peeled_tag(oid)? {
                content.push_str(&format!("^{}\n", peeled.to_hex()));
            }
        }
        lock.write_all(content.as_bytes())?;
        lock.commit()?;

        // Now packed, the loose files can go, unless they changed meanwhile.
        for (name, oid) in packed_loose {
            let ref_path = self.git_dir().join(&name);
            let ref_lock = match LockFile::acquire(&ref_path) {
                Ok(lock) => lock,
                Err(Error::Locked(_)) => continue,
                Err(e) => return Err(e),
            };
            if matches!(store.read_loose_ref(&name), Ok(RefValue::Direct(current)) if current == oid)
            {
                fs::remove_file(&ref_path)?;
            }
            drop(ref_lock);
            // Git keeps `refs/<kind>/` itself.
            let mut parts = name.splitn(3, '/');
            let root = match (parts.next(), parts.next()) {
                (Some(first), Some(second)) => self.git_dir().join(first).join(second),
                _ => self.git_dir().join("refs"),
            };
            self.remove_empty_ref_dirs(&ref_path, &root)?;
        }
        Ok(())
    }

    /// The object an annotated tag finally points to, or `None` if `oid` is
    /// not a tag (or is missing).
    fn peeled_tag(&self, oid: &Oid) -> Result<Option<Oid>> {
        match self.object_store().read(oid) {
            Ok(raw) if raw.object_type == ObjectType::Tag => Ok(Some(self.peel(oid)?)),
            Ok(_) | Err(Error::ObjectNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl Repository {
    /// Puts every reachable object into one new pack and removes what it
    /// replaces, like `git repack -a -d`, and returns what was done.
    ///
    /// Reachable means reachable from the references (loose and packed),
    /// HEAD, the reflogs, the index, and the state of a stopped merge,
    /// rebase, cherry-pick or fetch (`ORIG_HEAD`, `MERGE_HEAD`,
    /// `FETCH_HEAD`, ...). The new pack (version 2, with a version 2
    /// index) is written to a temporary file and renamed into
    /// `objects/pack/` as `pack-<checksum>.pack` / `.idx`; only then are
    /// the old packs and the loose objects the new pack holds removed, so a
    /// reader always finds every object.
    ///
    /// Objects that are deltas in the old packs stay deltas (their stored
    /// data is copied) when their base goes into the new pack too; loose
    /// objects are stored whole, as zerogit does not compute deltas. Packs
    /// with a `.keep` file are left as they are and their objects are not
    /// copied. Unreachable objects of the removed packs are written as loose
    /// objects, as `git repack -A` does, so nothing is lost before it is
    /// pruned.
    ///
    /// # Errors
    ///
    /// Nothing is removed when an error is returned (a partly written pack
    /// is deleted):
    /// - `Error::ObjectNotFound` if a reachable object is missing (the
    ///   repository is corrupt). An object a reference, reflog entry or
    ///   index entry names directly is skipped if missing.
    /// - `Error::InvalidPack` if an old pack is corrupt.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let summary = repo.repack().unwrap();
    /// println!("{} objects in {:?}", summary.objects(), summary.pack());
    /// ```
    pub fn repack(&self) -> Result<RepackSummary> {
        let store = self.object_store();
        let pack_dir = self.git_dir().join("objects").join("pack");
        let (kept, old): (Vec<Arc<PackFile>>, Vec<Arc<PackFile>>) = store
            .pack_files()?
            .into_iter()
            .partition(|pack| pack.path().with_extension("keep").exists());

        let reachable = self.reachable_for_repack()?;
        let reachable_set: HashSet<Oid> = reachable.iter().copied().collect();
        let wanted: Vec<Oid> = reachable
            .into_iter()
            .filter(|oid| !kept.iter().any(|pack| pack.contains(oid)))
            .collect();

        let mut summary = RepackSummary {
            pack: None,
            objects: wanted.len(),
            reused_deltas: 0,
            removed_packs: 0,
            removed_loose: 0,
            loosened: 0,
        };
        let mut new_pack: Option<Arc<PackFile>> = None;
        if !wanted.is_empty() {
            let (path, reused) = self.write_repack(&pack_dir, &wanted, &old)?;
            summary.reused_deltas = reused;
            new_pack = Some(Arc::new(PackFile::open(&path)?));
            summary.pack = Some(path);
        }

        // Unreachable objects of the packs about to go become loose.
        let new_path = summary.pack.clone();
        let is_new = |pack: &PackFile| new_path.as_deref() == Some(pack.path());
        for pack in old.iter().filter(|pack| !is_new(pack)) {
            for entry in pack.index().entries() {
                if reachable_set.contains(&entry.oid)
                    || kept.iter().any(|k| k.contains(&entry.oid))
                    || store.loose().exists(&entry.oid)
                {
                    continue;
                }
                let object = store.read(&entry.oid)?;
                store.loose().write(object.object_type, &object.content)?;
                summary.loosened += 1;
            }
        }
        for pack in old.iter().filter(|pack| !is_new(pack)) {
            for extension in PACK_FILES {
                remove_file(&pack.path().with_extension(extension))?;
            }
            summary.removed_packs += 1;
        }

        // Loose objects that a pack now holds (`git prune-packed`).
        let mut packed: Vec<Arc<PackFile>> = kept;
        packed.extend(new_pack);
        summary.removed_loose = self.prune_packed(&packed)?;
        Ok(summary)
    }

    /// The objects named directly by references, HEAD, reflogs, the index
    /// and state files (missing ones left out).
    fn repack_roots(&self) -> Result<Vec<Oid>> {
        let store = self.object_store();
        let mut roots: Vec<Oid> = self.references()?.into_iter().map(|(_, oid)| oid).collect();
        roots.extend(self.optional_head_oid()?);
        for name in STATE_FILES {
            if let Ok(content) = fs::read_to_string(self.git_dir().join(name)) {
                roots.extend(oids_in(&content));
            }
        }
        // Every reflog entry, old and new.
        let logs_dir = self.git_dir().join("logs");
        let mut logs = Vec::new();
        if logs_dir.join("HEAD").is_file() {
            logs.push(logs_dir.join("HEAD"));
        }
        let mut names = Vec::new();
        crate::refs::RefStore::collect_refs_recursive(&logs_dir.join("refs"), "", &mut names)?;
        logs.extend(names.iter().map(|name| logs_dir.join("refs").join(name)));
        for log in logs {
            let content = fs::read_to_string(&log)?;
            for line in content.lines() {
                roots.extend(
                    line.split(' ')
                        .take(2)
                        .filter(|word| word.len() == 40)
                        .filter_map(|word| Oid::from_hex(word).ok()),
                );
            }
        }
        for entry in self.read_index()?.entries() {
            if entry.mode() != FileMode::Submodule {
                roots.push(*entry.oid());
            }
        }
        let zero = crate::refs::reflog::zero_oid();
        let mut seen = HashSet::new();
        let mut existing = Vec::new();
        for oid in roots {
            if oid != zero && seen.insert(oid) && store.exists(&oid)? {
                existing.push(oid);
            }
        }
        Ok(existing)
    }

    /// Every object reachable from the roots, commits and tags before the
    /// trees and blobs they lead to. Blobs are checked to exist, not read.
    fn reachable_for_repack(&self) -> Result<Vec<Oid>> {
        let mut seen = HashSet::new();
        self.walk_objects(self.repack_roots()?, &mut seen, false)
    }

    /// The objects reachable from `starts` that are not in `seen` yet
    /// (which collects them). A missing object is an error, or with
    /// `tolerant` (for objects nothing refers to) skipped.
    fn walk_objects(
        &self,
        starts: Vec<Oid>,
        seen: &mut HashSet<Oid>,
        tolerant: bool,
    ) -> Result<Vec<Oid>> {
        let store = self.object_store();
        let mut order = Vec::new();
        let mut queue: VecDeque<(Oid, Option<ObjectType>)> =
            starts.into_iter().map(|oid| (oid, None)).collect();
        while let Some((oid, known)) = queue.pop_front() {
            if !seen.insert(oid) {
                continue;
            }
            if known == Some(ObjectType::Blob) {
                if !store.exists(&oid)? {
                    if tolerant {
                        continue;
                    }
                    return Err(Error::ObjectNotFound(oid.to_hex()));
                }
                order.push(oid);
                continue;
            }
            let raw = match store.read(&oid) {
                Ok(raw) => raw,
                Err(Error::ObjectNotFound(_)) if tolerant => continue,
                Err(e) => return Err(e),
            };
            order.push(oid);
            match raw.object_type {
                ObjectType::Commit => {
                    let commit = Commit::parse(oid, raw)?;
                    queue.push_back((*commit.tree(), Some(ObjectType::Tree)));
                    queue.extend(commit.parents().iter().map(|p| (*p, None)));
                }
                ObjectType::Tree => {
                    for entry in Tree::parse(raw)?.iter() {
                        let kind = match entry.mode() {
                            // Submodule commits live in another repository.
                            FileMode::Submodule => continue,
                            FileMode::Directory => ObjectType::Tree,
                            _ => ObjectType::Blob,
                        };
                        queue.push_back((*entry.oid(), Some(kind)));
                    }
                }
                ObjectType::Tag => queue.push_back((*TagObject::parse(raw)?.object(), None)),
                ObjectType::Blob => {}
            }
        }
        Ok(order)
    }

    /// Writes `objects` to a new pack, copying deltas from `old` packs
    /// whose base is written too; returns the pack's path and the number
    /// of copied deltas.
    fn write_repack(
        &self,
        pack_dir: &Path,
        objects: &[Oid],
        old: &[Arc<PackFile>],
    ) -> Result<(PathBuf, usize)> {
        let store = self.object_store();
        let wanted: HashSet<Oid> = objects.iter().copied().collect();
        let too_large = |oid: &Oid| Error::PackLimitExceeded {
            reason: format!("object {} too large for this platform", oid),
        };
        let count = u32::try_from(objects.len()).map_err(|_| Error::PackLimitExceeded {
            reason: "too many objects for one pack".to_owned(),
        })?;
        let mut writer = PackWriter::create(pack_dir, count)?;
        let mut written: HashMap<Oid, u64> = HashMap::new();
        let mut indexed: Vec<IndexedObject> = Vec::with_capacity(objects.len());
        let mut reused = 0;
        // Offsets of each old pack's entries, to name OFS_DELTA bases.
        let mut offsets: HashMap<usize, HashMap<u64, Oid>> = HashMap::new();

        for &start in objects {
            // Follow the delta chain down to an object that is written
            // already or will be stored whole, then write bottom up.
            let mut chain: Vec<(Oid, Option<(RawEntry, Oid)>)> = Vec::new();
            let mut current = start;
            while !written.contains_key(&current) {
                let found = old.iter().enumerate().find(|(_, p)| p.contains(&current));
                let Some((pack_no, pack)) = found else {
                    chain.push((current, None));
                    break;
                };
                let entry = pack
                    .raw_entry(&current)?
                    .ok_or_else(|| Error::ObjectNotFound(current.to_hex()))?;
                let base = match entry.kind {
                    EntryKind::Base(_) => None,
                    EntryKind::RefDelta(base) => Some(base),
                    EntryKind::OfsDelta(offset) => offsets
                        .entry(pack_no)
                        .or_insert_with(|| {
                            pack.index()
                                .entries()
                                .iter()
                                .map(|e| (e.offset, e.oid))
                                .collect()
                        })
                        .get(&offset)
                        .copied(),
                };
                match base {
                    None => {
                        // Stored whole: copied as it is.
                        chain.push((current, Some((entry, current))));
                        break;
                    }
                    // A delta against an object that goes in the new pack.
                    Some(base)
                        if wanted.contains(&base)
                            && base != current
                            && !chain.iter().any(|(oid, _)| *oid == base) =>
                    {
                        chain.push((current, Some((entry, base))));
                        current = base;
                    }
                    Some(_) => {
                        // The base stays behind: store the object whole.
                        chain.push((current, None));
                        break;
                    }
                }
            }
            for (oid, entry) in chain.into_iter().rev() {
                if written.contains_key(&oid) {
                    continue;
                }
                let mut bytes = Vec::new();
                match entry {
                    Some((entry, base)) if base != oid => {
                        // OFS_DELTA against the base written before it.
                        let size = usize::try_from(entry.size).map_err(|_| too_large(&oid))?;
                        bytes.extend(entry_header(6, size));
                        bytes.extend(ofs_distance(writer.offset - written[&base]));
                        bytes.extend_from_slice(&entry.compressed);
                        reused += 1;
                    }
                    Some((entry, _)) => {
                        let EntryKind::Base(object_type) = entry.kind else {
                            unreachable!("only whole entries are copied as they are")
                        };
                        let size = usize::try_from(entry.size).map_err(|_| too_large(&oid))?;
                        bytes.extend(entry_header(type_code(object_type), size));
                        bytes.extend_from_slice(&entry.compressed);
                    }
                    None => {
                        let object = store.read(&oid)?;
                        bytes.extend(entry_header(
                            type_code(object.object_type),
                            object.content.len(),
                        ));
                        bytes.extend(compress(&object.content));
                    }
                }
                written.insert(oid, writer.offset);
                indexed.push(IndexedObject {
                    oid,
                    offset: writer.offset,
                    crc32: crc32(&bytes),
                });
                writer.write(&bytes)?;
            }
        }

        let (tmp, checksum) = writer.finish()?;
        let name: String = checksum.iter().map(|b| format!("{:02x}", b)).collect();
        let pack_path = pack_dir.join(format!("pack-{}.pack", name));
        let index_path = pack_dir.join(format!("pack-{}.idx", name));
        if index_path.exists() && pack_path.exists() {
            // The same pack is there already.
            fs::remove_file(&tmp)?;
        } else {
            // The pack first, then the index that makes it visible.
            fs::rename(&tmp, &pack_path)?;
            write_file_atomic(&index_path, &write_index(&indexed, &checksum))?;
        }
        Ok((pack_path, reused))
    }

    /// Removes loose objects that `packs` hold, and the fan-out
    /// directories left empty; returns how many were removed.
    fn prune_packed(&self, packs: &[Arc<PackFile>]) -> Result<usize> {
        let objects = self.git_dir().join("objects");
        let mut removed = 0;
        for fanout in 0..=255u8 {
            let dir = objects.join(format!("{:02x}", fanout));
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(oid) = Oid::from_hex(&format!("{:02x}{}", fanout, name)) else {
                    continue;
                };
                if packs.iter().any(|pack| pack.contains(&oid)) {
                    remove_file(&entry.path())?;
                    removed += 1;
                }
            }
            if fs::read_dir(&dir)?.next().is_none() {
                fs::remove_dir(&dir)?;
            }
        }
        Ok(removed)
    }
}

/// What [`Repository::gc`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcSummary {
    repack: RepackSummary,
    pruned: usize,
}

impl GcSummary {
    /// What the repack did.
    pub fn repack(&self) -> &RepackSummary {
        &self.repack
    }

    /// The number of unreachable loose objects removed.
    pub fn pruned(&self) -> usize {
        self.pruned
    }
}

/// How long unreachable objects are kept (`gc.pruneExpire`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expire {
    /// Never prune.
    Never,
    /// Prune objects last written before this time.
    Before(SystemTime),
}

/// Parses an expiry date as Git's `gc.pruneExpire` takes it: `now`,
/// `never`, a relative date such as `2.weeks.ago` or `3 days ago`, or an
/// absolute date (see `infra::time::parse_git_date`).
pub(crate) fn parse_expire(value: &str, now: SystemTime) -> Result<Expire> {
    let text = value.trim().to_ascii_lowercase();
    match text.as_str() {
        "never" | "false" => return Ok(Expire::Never),
        "now" | "all" => return Ok(Expire::Before(now)),
        _ => {}
    }
    let words: Vec<&str> = text
        .split(|c: char| c == '.' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect();
    if let [count, unit, "ago"] = words[..] {
        if let Ok(count) = count.parse::<u64>() {
            let seconds = match unit.trim_end_matches('s') {
                "second" | "sec" => Some(1),
                "minute" | "min" => Some(60),
                "hour" => Some(3600),
                "day" => Some(86_400),
                "week" => Some(7 * 86_400),
                "month" => Some(30 * 86_400),
                "year" => Some(365 * 86_400),
                _ => None,
            };
            if let Some(seconds) = seconds {
                let ago = Duration::from_secs(count.saturating_mul(seconds));
                return Ok(Expire::Before(
                    now.checked_sub(ago).unwrap_or(SystemTime::UNIX_EPOCH),
                ));
            }
        }
    }
    match crate::infra::time::parse_git_date(value) {
        Some((seconds, _)) => {
            let at = Duration::from_secs(u64::try_from(seconds).unwrap_or(0));
            Ok(Expire::Before(SystemTime::UNIX_EPOCH + at))
        }
        None => Err(Error::InvalidDate(value.to_owned())),
    }
}

/// Whether a file in `objects/` is a temporary one left by an
/// interrupted writer (Git's `tmp_*` / `.tmp-*`, zerogit's `.<name>.tmp-*`).
fn is_temporary(name: &str) -> bool {
    name.starts_with("tmp_") || name.starts_with(".tmp-") || name.contains(".tmp-")
}

impl Repository {
    /// Removes unreachable loose objects, like `git prune`, and returns how
    /// many were removed.
    ///
    /// With `older_than`, only objects last written before it are removed
    /// (`git prune --expire <time>`), and objects that newer unreachable
    /// objects refer to are kept as well, so an object being written by a
    /// concurrent operation (not yet referenced) survives. Without it,
    /// every unreachable loose object goes. Reachable means what
    /// [`Repository::repack`] packs. Loose objects that a pack holds are
    /// removed too (`git prune-packed`), and so are temporary files of
    /// interrupted writes older than `older_than`. Packed objects are not
    /// pruned; [`Repository::repack`] writes the unreachable ones out as
    /// loose objects first.
    ///
    /// # Errors
    ///
    /// `Error::ObjectNotFound` if a reachable object is missing (the
    /// repository is corrupt); nothing is removed then.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::{Duration, SystemTime};
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Unreachable objects older than two weeks, as `git gc` prunes.
    /// let two_weeks = Duration::from_secs(14 * 24 * 3600);
    /// repo.prune(Some(SystemTime::now() - two_weeks)).unwrap();
    /// ```
    pub fn prune(&self, older_than: Option<SystemTime>) -> Result<usize> {
        let mut keep: HashSet<Oid> = HashSet::new();
        self.walk_objects(self.repack_roots()?, &mut keep, false)?;
        let expired = |modified: SystemTime| older_than.map_or(true, |limit| modified < limit);

        let objects = self.git_dir().join("objects");
        let mut loose: Vec<(Oid, PathBuf, SystemTime)> = Vec::new();
        let mut temporary: Vec<PathBuf> = Vec::new();
        for fanout in 0..=255u8 {
            let dir = objects.join(format!("{:02x}", fanout));
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let modified = entry.metadata()?.modified()?;
                match Oid::from_hex(&format!("{:02x}{}", fanout, name)) {
                    Ok(oid) if name.len() == 38 => loose.push((oid, entry.path(), modified)),
                    _ if is_temporary(&name) && expired(modified) => temporary.push(entry.path()),
                    _ => {}
                }
            }
        }
        if let Ok(entries) = fs::read_dir(objects.join("pack")) {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_temporary(&name) && expired(entry.metadata()?.modified()?) {
                    temporary.push(entry.path());
                }
            }
        }

        // Recent unreachable objects keep what they refer to.
        let recent: Vec<Oid> = loose
            .iter()
            .filter(|(oid, _, modified)| !keep.contains(oid) && !expired(*modified))
            .map(|(oid, _, _)| *oid)
            .collect();
        self.walk_objects(recent, &mut keep, true)?;

        let mut removed = 0;
        for (oid, path, modified) in &loose {
            if !keep.contains(oid) && expired(*modified) && remove_file(path)? {
                removed += 1;
            }
        }
        for path in temporary {
            remove_file(&path)?;
        }
        let packs = self.object_store().pack_files()?;
        self.prune_packed(&packs)?;
        Ok(removed)
    }

    /// Cleans up the repository like `git gc`: packs the references
    /// ([`Repository::pack_refs`]), expires old reflog entries
    /// ([`Repository::reflog_expire`] with `gc.reflogExpire` and
    /// `gc.reflogExpireUnreachable`), repacks the objects
    /// ([`Repository::repack`]) and prunes unreachable loose objects older
    /// than `gc.pruneExpire` ([`Repository::prune`]; by default two weeks,
    /// `now` for all, `never` to keep them). Objects that only expired
    /// reflog entries reached are thus pruned too, once old enough.
    ///
    /// Unlike `git gc`, unreachable objects are kept as loose objects
    /// rather than in a cruft pack. A `gc.pid.lock` file is held meanwhile,
    /// so `git gc` does not start at the same time.
    ///
    /// # Errors
    ///
    /// - `Error::Locked` if `gc.pid` or `packed-refs` is locked (another gc
    ///   is starting), or a reference or reflog is (no reflog is changed).
    /// - `Error::InvalidDate` if `gc.pruneExpire` or a `gc.*reflogExpire*`
    ///   setting cannot be parsed; nothing is changed.
    /// - The errors of [`Repository::reflog_expire`], [`Repository::repack`]
    ///   and [`Repository::prune`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let summary = repo.gc().unwrap();
    /// println!("{} objects packed, {} pruned", summary.repack().objects(), summary.pruned());
    /// ```
    pub fn gc(&self) -> Result<GcSummary> {
        let expire = match self.config()?.get("gc", "pruneexpire") {
            Some(value) => parse_expire(value, SystemTime::now())?,
            None => parse_expire("2.weeks.ago", SystemTime::now())?,
        };
        // Check the reflog settings before changing anything.
        crate::refs::reflog::check_expiry_config(&self.config()?)?;
        let _lock = LockFile::acquire(self.git_dir().join("gc.pid"))?;
        // In Git's order: references, reflogs, then objects.
        self.pack_refs()?;
        self.reflog_expire(None, None)?;
        let repack = self.repack()?;
        let pruned = match expire {
            Expire::Never => 0,
            Expire::Before(limit) => self.prune(Some(limit))?,
        };
        Ok(GcSummary { repack, pruned })
    }

    /// Runs [`Repository::gc`] only when the repository needs it, like
    /// `git gc --auto`, and returns its summary, or `None` if it was not
    /// needed. Calling it after operations that write objects keeps a
    /// repository tidy without Git.
    ///
    /// As in Git, it is needed when there are more than about `gc.auto`
    /// loose objects (default 6700; estimated from `objects/17/`) or more
    /// than `gc.autoPackLimit` packs without a `.keep` file (default 50).
    /// `gc.auto = 0` turns it off.
    ///
    /// # Errors
    ///
    /// The errors of [`Repository::gc`].
    pub fn gc_auto(&self) -> Result<Option<GcSummary>> {
        let config = self.config()?;
        let setting = |key: &str, default: i64| -> Result<i64> {
            match config.get("gc", key) {
                Some(_) => config.get_int("gc", key),
                None => Ok(default),
            }
        };
        let auto = setting("auto", 6700)?;
        if auto <= 0 {
            return Ok(None);
        }
        // Loose objects are spread evenly over the 256 directories.
        let threshold = (auto + 255) / 256;
        let sample = match fs::read_dir(self.git_dir().join("objects").join("17")) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.len() == 38 && name.bytes().all(|b| b.is_ascii_hexdigit())
                })
                .count() as i64,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        let pack_limit = setting("autopacklimit", 50)?;
        let packs = self
            .object_store()
            .pack_files()?
            .iter()
            .filter(|pack| !pack.path().with_extension("keep").exists())
            .count() as i64;
        if sample > threshold || (pack_limit > 0 && packs > pack_limit) {
            self.gc().map(Some)
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_dates_like_git() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let ago = |s: u64| Expire::Before(now - Duration::from_secs(s));
        assert_eq!(parse_expire("2.weeks.ago", now).unwrap(), ago(14 * 86_400));
        assert_eq!(parse_expire("3 days ago", now).unwrap(), ago(3 * 86_400));
        assert_eq!(parse_expire("1.hour.ago", now).unwrap(), ago(3600));
        assert_eq!(parse_expire("now", now).unwrap(), Expire::Before(now));
        assert_eq!(parse_expire("never", now).unwrap(), Expire::Never);
        assert_eq!(
            parse_expire("2005-04-07T22:13:13Z", now).unwrap(),
            Expire::Before(SystemTime::UNIX_EPOCH + Duration::from_secs(1_112_911_993))
        );
        assert!(matches!(
            parse_expire("someday", now),
            Err(Error::InvalidDate(_))
        ));
    }

    #[test]
    fn temporary_files() {
        assert!(is_temporary("tmp_obj_abc"));
        assert!(is_temporary(".tmp-1234-pack"));
        assert!(is_temporary(".0123456789.tmp-77-0"));
        assert!(!is_temporary("0123456789abcdef0123456789abcdef012345"));
    }
}
