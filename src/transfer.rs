//! Moving objects between repositories: storing received packs and building
//! packs to send.
//!
//! These are the repository-side halves of fetch and push. The network side
//! (protocols and transports) lives in the separate `zerogit-remote` crate.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::write_file_atomic;
use crate::objects::pack::indexer::{index_pack, write_index, write_pack};
use crate::objects::{ObjectType, Oid, TagObject, Tree};
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

    /// Builds a pack (version 2, without deltas) of the objects reachable
    /// from `wants` but not from `haves`, as sent by fetch or push.
    pub fn pack_objects(&self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<u8>> {
        let store = self.object_store();
        let mut objects = Vec::new();
        for oid in self.objects_to_send(wants, haves)? {
            let raw = store.read(&oid)?;
            objects.push((raw.object_type, raw.content));
        }
        Ok(write_pack(&objects))
    }
}
