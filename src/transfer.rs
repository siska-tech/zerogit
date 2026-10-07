//! Moving objects between repositories: storing received packs and building
//! packs to send.
//!
//! These are the repository-side halves of fetch and push. The network side
//! (protocols and transports) lives in the separate `zerogit-remote` crate.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::write_file_atomic;
use crate::objects::pack::deltify::DeltaSearch;
use crate::objects::pack::indexer::{index_pack, write_index};
use crate::objects::{ObjectType, Oid, TagObject, Tree};
use crate::pack_builder::{MemoryPack, PackPlan};
use crate::repository::Repository;

/// A pack stored by [`Repository::store_pack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPack {
    path: PathBuf,
    objects: Vec<Oid>,
}

impl StoredPack {
    /// The path of the `.pack` file.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The objects in the pack (including bases added to complete a thin
    /// pack).
    pub fn objects(&self) -> &[Oid] {
        &self.objects
    }
}

impl Repository {
    /// Stores a pack received from another repository, like
    /// `git index-pack --fix-thin`: the pack is checked, its deltas are
    /// resolved, missing delta bases are taken from this repository and
    /// appended, and the pack is written to `objects/pack/` with a version 2
    /// index as `pack-<checksum>.pack` / `.idx`.
    ///
    /// # Errors
    ///
    /// `Error::InvalidPack` if the pack is malformed, its checksum does not
    /// match, or a delta base is missing; nothing is written then.
    pub fn store_pack(&self, data: &[u8]) -> Result<StoredPack> {
        let store = self.object_store();
        let indexed = index_pack(data, &mut |oid| match store.read(oid) {
            Ok(object) => Ok(Some(object)),
            Err(Error::ObjectNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        })?;
        let name: String = indexed
            .checksum
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let dir = self.git_dir().join("objects").join("pack");
        std::fs::create_dir_all(&dir)?;
        let pack_path = dir.join(format!("pack-{}.pack", name));
        let index_path = dir.join(format!("pack-{}.idx", name));
        if !index_path.exists() {
            // The pack first, then the index that makes it visible.
            write_file_atomic(&pack_path, &indexed.data)?;
            write_file_atomic(
                &index_path,
                &write_index(&indexed.objects, &indexed.checksum),
            )?;
        }
        Ok(StoredPack {
            path: pack_path,
            objects: indexed.objects.iter().map(|o| o.oid).collect(),
        })
    }

    /// Every object reachable from `starts` (commits, their trees and
    /// blobs, and tags), skipping those in `exclude` and below them.
    fn reachable_objects(&self, starts: &[Oid], exclude: &HashSet<Oid>) -> Result<Vec<Oid>> {
        let store = self.object_store();
        let mut seen: HashSet<Oid> = HashSet::new();
        let mut order = Vec::new();
        let mut queue: VecDeque<Oid> = starts.iter().copied().collect();
        while let Some(oid) = queue.pop_front() {
            if exclude.contains(&oid) || !seen.insert(oid) {
                continue;
            }
            let raw = store.read(&oid)?;
            order.push(oid);
            match raw.object_type {
                ObjectType::Commit => {
                    let commit = self.commit(&oid.to_hex())?;
                    queue.push_back(*commit.tree());
                    queue.extend(commit.parents().iter().copied());
                }
                ObjectType::Tree => {
                    for entry in Tree::parse(raw)?.iter() {
                        // Submodule commits live in another repository.
                        if entry.mode() != crate::FileMode::Submodule {
                            queue.push_back(*entry.oid());
                        }
                    }
                }
                ObjectType::Tag => queue.push_back(*TagObject::parse(raw)?.object()),
                ObjectType::Blob => {}
            }
        }
        Ok(order)
    }

    /// The objects a receiver that has `haves` needs to get `wants`: those
    /// reachable from `wants` but not from `haves`. Haves this repository
    /// does not have are ignored.
    pub fn objects_to_send(&self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<Oid>> {
        let store = self.object_store();
        let mut known: Vec<Oid> = Vec::new();
        for oid in haves {
            if store.exists(oid)? {
                known.push(*oid);
            }
        }
        let excluded: HashSet<Oid> = self
            .reachable_objects(&known, &HashSet::new())?
            .into_iter()
            .collect();
        self.reachable_objects(wants, &excluded)
    }

    /// Builds a pack (version 2) of the objects reachable from `wants` but
    /// not from `haves`, as sent by fetch or push, with the default
    /// [`PackObjectsOptions`]: deltas against objects of the pack, named
    /// by object ID, which every Git reads.
    pub fn pack_objects(&self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<u8>> {
        self.pack_objects_with(wants, haves, &PackObjectsOptions::new())
    }

    /// Builds a pack (version 2) of the objects reachable from `wants` but
    /// not from `haves`, like `git pack-objects --revs`.
    ///
    /// Objects are encoded as deltas against similar objects where that
    /// saves space, as [`Repository::repack`] does (`pack.window`,
    /// `pack.depth`, `core.bigFileThreshold`), and deltas already in this
    /// repository's packs are copied when their base is sent too (or, for
    /// a thin pack, is one the receiver has). With
    /// [`PackObjectsOptions::thin`], the files and trees of the commits
    /// in `haves` that the sent commits build on are tried as delta bases
    /// too; the receiver completes such a pack from its own objects (as
    /// `git index-pack --fix-thin` and [`Repository::store_pack`] do).
    ///
    /// # Errors
    ///
    /// `Error::ObjectNotFound` if an object to send is missing.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{Oid, PackObjectsOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// # let (new, old) = (Oid::from_bytes([1; 20]), Oid::from_bytes([2; 20]));
    /// // A push to a server that takes thin packs and OFS_DELTA.
    /// let options = PackObjectsOptions::new().thin(true).ofs_delta(true);
    /// let pack = repo.pack_objects_with(&[new], &[old], &options).unwrap();
    /// ```
    pub fn pack_objects_with(
        &self,
        wants: &[Oid],
        haves: &[Oid],
        options: &PackObjectsOptions,
    ) -> Result<Vec<u8>> {
        let store = self.object_store();
        let mut known: Vec<Oid> = Vec::new();
        for oid in haves {
            if store.exists(oid)? {
                known.push(*oid);
            }
        }
        let excluded: HashSet<Oid> = self
            .reachable_objects(&known, &HashSet::new())?
            .into_iter()
            .collect();
        // Seeded with what the receiver has, the walk stops there.
        let mut seen = excluded.clone();
        let mut info = HashMap::new();
        let objects = self.walk_objects(wants.to_vec(), &mut seen, false, Some(&mut info))?;
        let count = u32::try_from(objects.len()).map_err(|_| Error::PackLimitExceeded {
            reason: "too many objects for one pack".to_owned(),
        })?;

        let mut external_bases = Vec::new();
        if options.thin {
            // The trees of the commits the sent ones build on.
            let mut edges = Vec::new();
            for oid in &objects {
                if info.get(oid).map(|c| c.object_type) == Some(ObjectType::Commit) {
                    for parent in self.commit(&oid.to_hex())?.parents() {
                        if excluded.contains(parent) {
                            edges.push(*self.commit(&parent.to_hex())?.tree());
                        }
                    }
                }
            }
            let mut base_info = HashMap::new();
            self.walk_objects(edges, &mut HashSet::new(), false, Some(&mut base_info))?;
            external_bases = base_info.into_values().collect();
        }

        let packs = store.pack_files()?;
        let plan = PackPlan {
            objects: &objects,
            info,
            external_bases,
            reader_has: options.thin.then_some(&excluded),
            packs: &packs,
            ofs_delta: options.ofs_delta,
            search: DeltaSearch::from_config(&self.config()?)?,
        };
        let mut pack = MemoryPack::new(count);
        self.write_pack_objects(plan, &mut pack)?;
        Ok(pack.finish())
    }
}

/// Options for [`Repository::pack_objects_with`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackObjectsOptions {
    thin: bool,
    ofs_delta: bool,
}

impl PackObjectsOptions {
    /// The default options: a complete pack whose deltas name their base
    /// by object ID (REF_DELTA), which every receiver reads.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a thin pack (`--thin`): deltas may be against objects the
    /// receiver has (named in `haves`) rather than in the pack. Send one
    /// only to a receiver that completes it: a Git server that does not
    /// advertise `no-thin`, or a client that asked for `thin-pack`.
    pub fn thin(mut self, thin: bool) -> Self {
        self.thin = thin;
        self
    }

    /// Lets deltas name a base in the pack by its offset (OFS_DELTA,
    /// `--delta-base-offset`), which is smaller. Use it when the receiver
    /// advertises `ofs-delta`.
    pub fn ofs_delta(mut self, ofs_delta: bool) -> Self {
        self.ofs_delta = ofs_delta;
        self
    }
}
