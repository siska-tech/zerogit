//! Shared object access for repository operations.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use super::pack::{PackFile, PackLimits};
use super::{LooseObjectStore, ObjectType, Oid, RawObject};
use crate::error::{Error, Result};
use crate::infra::hash_object;

/// REF_DELTA bases in other packs are resolved recursively; bound the number
/// of hops far below anything that could exhaust the stack.
const MAX_BASE_HOPS: usize = 64;

/// Internal access point for loose objects and packs. Writes remain loose objects.
///
/// Clones share one set of opened packs. The pack directory is scanned and
/// indexes are parsed on first use, then kept for the lifetime of the store,
/// which a `Repository` shares with the iterators it creates. Each pack holds
/// its own delta cache of [`PackLimits::delta_cache_size`] bytes.
///
/// When an object or delta base is not found, the pack directory is rescanned
/// once so that packs written or replaced by an external `git repack`/`git gc`
/// are picked up; there is no further retry. Prefix searches always rescan.
/// Packs removed from disk are dropped from the set, and a pack that cannot be
/// opened is an error rather than being skipped.
#[derive(Debug, Clone)]
pub(crate) struct ObjectStore {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    loose: LooseObjectStore,
    pack_dir: PathBuf,
    /// `None` until the pack directory is first scanned.
    packs: RwLock<Option<Vec<Arc<PackFile>>>>,
}

fn lock_poisoned() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        "object store lock poisoned",
    ))
}

/// Lists `*.pack` files that have a matching `.idx`, in name order.
///
/// Git writes the index last, so a pack without one is still being written.
fn list_packs(pack_dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(pack_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(e)),
    };
    let mut packs = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "idx") {
            let pack = path.with_extension("pack");
            if pack.is_file() {
                packs.push(pack);
            }
        }
    }
    packs.sort();
    Ok(packs)
}

impl ObjectStore {
    pub(crate) fn new(objects_dir: impl AsRef<Path>) -> Self {
        let objects_dir = objects_dir.as_ref();
        Self {
            inner: Arc::new(Inner {
                loose: LooseObjectStore::new(objects_dir),
                pack_dir: objects_dir.join("pack"),
                packs: RwLock::new(None),
            }),
        }
    }

    pub(crate) fn from_loose(loose: &LooseObjectStore) -> Self {
        Self::new(loose.objects_dir())
    }

    /// Returns the opened packs, scanning the directory on first use or when
    /// `rescan` is set. The flag reports whether the set of packs changed.
    fn packs(&self, rescan: bool) -> Result<(Vec<Arc<PackFile>>, bool)> {
        if !rescan {
            if let Some(packs) = self
                .inner
                .packs
                .read()
                .map_err(|_| lock_poisoned())?
                .as_ref()
            {
                return Ok((packs.clone(), false));
            }
        }
        let mut guard = self.inner.packs.write().map_err(|_| lock_poisoned())?;
        if let (false, Some(packs)) = (rescan, guard.as_ref()) {
            return Ok((packs.clone(), false));
        }
        let current = guard.as_deref().unwrap_or(&[]);
        let mut packs = Vec::new();
        for path in list_packs(&self.inner.pack_dir)? {
            match current.iter().find(|pack| pack.path() == path) {
                Some(pack) => packs.push(Arc::clone(pack)),
                None => packs.push(Arc::new(PackFile::open_with_limits(
                    &path,
                    PackLimits::default(),
                )?)),
            }
        }
        let changed = guard.is_none()
            || packs.len() != current.len()
            || packs.iter().zip(current).any(|(a, b)| !Arc::ptr_eq(a, b));
        *guard = Some(packs.clone());
        Ok((packs, changed))
    }

    /// The packs in the pack directory now (rescanned).
    pub(crate) fn pack_files(&self) -> Result<Vec<Arc<PackFile>>> {
        Ok(self.packs(true)?.0)
    }

    /// The loose objects.
    pub(crate) fn loose(&self) -> &LooseObjectStore {
        &self.inner.loose
    }

    pub(crate) fn read(&self, oid: &Oid) -> Result<RawObject> {
        self.find(oid, PackLimits::default().max_delta_depth, 0)?
            .ok_or_else(|| Error::ObjectNotFound(oid.to_hex()))
    }

    /// Looks in loose objects, then packs, rescanning packs once on a miss.
    fn find(&self, oid: &Oid, max_depth: usize, hops: usize) -> Result<Option<RawObject>> {
        let (packs, _) = self.packs(false)?;
        if let Some(object) = self.find_in(&packs, oid, max_depth, hops)? {
            return Ok(Some(object));
        }
        let (packs, changed) = self.packs(true)?;
        if !changed {
            return Ok(None);
        }
        self.find_in(&packs, oid, max_depth, hops)
    }

    fn find_in(
        &self,
        packs: &[Arc<PackFile>],
        oid: &Oid,
        max_depth: usize,
        hops: usize,
    ) -> Result<Option<RawObject>> {
        match self.inner.loose.read(oid) {
            Ok(object) => return Ok(Some(object)),
            Err(Error::ObjectNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        for pack in packs {
            if pack.contains(oid) {
                return pack.read_with_resolver(oid, max_depth, &mut |base, remaining| {
                    if hops >= MAX_BASE_HOPS {
                        return Err(Error::PackLimitExceeded {
                            reason: format!(
                                "delta base {} is more than {} packs away",
                                base, MAX_BASE_HOPS
                            ),
                        });
                    }
                    self.find(base, remaining, hops + 1)
                });
            }
        }
        Ok(None)
    }

    /// Unlike the public compatibility helper, does not hide I/O failures.
    pub(crate) fn exists(&self, oid: &Oid) -> Result<bool> {
        let (packs, _) = self.packs(false)?;
        if self.exists_in(&packs, oid)? {
            return Ok(true);
        }
        let (packs, changed) = self.packs(true)?;
        Ok(changed && self.exists_in(&packs, oid)?)
    }

    fn exists_in(&self, packs: &[Arc<PackFile>], oid: &Oid) -> Result<bool> {
        match fs::metadata(self.inner.loose.oid_to_path(oid)) {
            Ok(metadata) if metadata.is_file() => return Ok(true),
            Ok(_) => {
                return Err(Error::InvalidObject {
                    oid: oid.to_hex(),
                    reason: "object path is not a file".into(),
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(e)),
        }
        Ok(packs.iter().any(|pack| pack.contains(oid)))
    }

    /// Returns every distinct object, loose or packed, whose ID starts with `prefix`.
    ///
    /// Always rescans the pack directory: with a stale pack list, a prefix
    /// shared with a newly packed object would wrongly look unambiguous.
    /// Unchanged packs are reused, so this costs one directory listing.
    pub(crate) fn find_objects_by_prefix(&self, prefix: &str) -> Result<Vec<Oid>> {
        let (packs, _) = self.packs(true)?;
        self.prefix_in(&packs, prefix)
    }

    fn prefix_in(&self, packs: &[Arc<PackFile>], prefix: &str) -> Result<Vec<Oid>> {
        let mut matches: BTreeSet<Oid> = self
            .inner
            .loose
            .find_objects_by_prefix(prefix)?
            .into_iter()
            .collect();
        for pack in packs {
            matches.extend(pack.index().find_objects_by_prefix(prefix)?);
        }
        Ok(matches.into_iter().collect())
    }

    pub(crate) fn resolve_oid(&self, value: &str) -> Result<Oid> {
        // Preserve the existing full-OID parsing behavior; reads check existence.
        if value.len() == 40 {
            return Oid::from_hex(value);
        }
        let matches = self.find_objects_by_prefix(value)?;
        match matches.as_slice() {
            [] => Err(Error::ObjectNotFound(value.to_owned())),
            [oid] => Ok(*oid),
            _ => Err(Error::InvalidOid(format!(
                "ambiguous short OID: {} ({} matches)",
                value,
                matches.len()
            ))),
        }
    }

    pub(crate) fn write(&self, kind: ObjectType, content: &[u8]) -> Result<Oid> {
        let oid = Oid::from_bytes(hash_object(kind.as_str(), content));
        if self.exists(&oid)? {
            // An existing but unreadable object must not look like a successful write.
            let existing = self.read(&oid)?;
            if existing.object_type != kind || existing.content != content {
                return Err(Error::InvalidObject {
                    oid: oid.to_hex(),
                    reason: "existing object does not match its object ID".into(),
                });
            }
            return Ok(oid);
        }
        self.inner.loose.write(kind, content)
    }
}

#[cfg(test)]
mod tests {
    use super::super::pack::test_support::{append_delta, oid_of, Builder};
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_corrupt_and_io_errors_remain_distinct() {
        let temp = TempDir::new().unwrap();
        let store = ObjectStore::new(temp.path());
        let oid = Oid::from_hex("abcd000000000000000000000000000000000000").unwrap();
        assert!(!store.exists(&oid).unwrap());
        assert!(matches!(store.read(&oid), Err(Error::ObjectNotFound(_))));
        let path = store.inner.loose.oid_to_path(&oid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not zlib").unwrap();
        assert!(matches!(store.read(&oid), Err(Error::DecompressionFailed)));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(store.read(&oid), Err(Error::Io(_))));
        assert!(matches!(
            store.exists(&oid),
            Err(Error::InvalidObject { .. })
        ));
    }

    #[test]
    fn prefix_resolution_rejects_ambiguity_and_non_objects() {
        let temp = TempDir::new().unwrap();
        let store = ObjectStore::new(temp.path());
        let a = Oid::from_hex("abcd000000000000000000000000000000000000").unwrap();
        let b = Oid::from_hex("abcd100000000000000000000000000000000000").unwrap();
        let loose = &store.inner.loose;
        let path = loose.oid_to_path(&a);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"placeholder").unwrap();
        fs::create_dir(loose.oid_to_path(&b)).unwrap();
        assert_eq!(store.resolve_oid("ABCD").unwrap(), a);
        fs::remove_dir(loose.oid_to_path(&b)).unwrap();
        fs::write(loose.oid_to_path(&b), b"placeholder").unwrap();
        assert!(matches!(
            store.resolve_oid("abcd"),
            Err(Error::InvalidOid(_))
        ));
        assert_eq!(store.resolve_oid("abcd0").unwrap(), a);
        assert!(matches!(
            store.resolve_oid("eeee"),
            Err(Error::ObjectNotFound(_))
        ));
        assert!(matches!(
            store.resolve_oid("xyz!"),
            Err(Error::InvalidOid(_))
        ));
    }

    #[test]
    fn write_does_not_accept_corrupt_existing_object() {
        let temp = TempDir::new().unwrap();
        let store = ObjectStore::new(temp.path());
        let oid = store.write(ObjectType::Blob, b"original").unwrap();
        fs::write(
            store.inner.loose.oid_to_path(&oid),
            crate::infra::compress(b"blob 8\0modified"),
        )
        .unwrap();
        assert!(matches!(store.read(&oid), Err(Error::InvalidObject { .. })));
        assert!(matches!(
            store.write(ObjectType::Blob, b"original"),
            Err(Error::InvalidObject { .. })
        ));
    }

    fn store_with_pack_dir() -> (TempDir, ObjectStore, PathBuf) {
        let temp = TempDir::new().unwrap();
        let store = ObjectStore::new(temp.path());
        let pack_dir = temp.path().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        (temp, store, pack_dir)
    }

    #[test]
    fn ref_delta_bases_resolve_across_packs_and_loose_objects() {
        let (_temp, store, pack_dir) = store_with_pack_dir();
        let base = b"shared base text\n".repeat(10);
        let loose_base = b"loose base text\n".repeat(10);
        let mut a = Builder::default();
        let (base_oid, _) = a.object(ObjectType::Blob, &base);
        a.write_named(&pack_dir, "pack-a", 2);
        let loose_oid = store.write(ObjectType::Blob, &loose_base).unwrap();

        let mut b = Builder::default();
        let mut expected = Vec::new();
        for (base_oid, base) in [(base_oid, &base), (loose_oid, &loose_base)] {
            let mut content = base.clone();
            content.extend_from_slice(b"delta\n");
            let oid = oid_of(ObjectType::Blob, &content);
            b.ref_delta(oid, base_oid, &append_delta(base, b"delta\n"));
            expected.push((oid, content));
        }
        b.write_named(&pack_dir, "pack-b", 2);

        for (oid, content) in expected {
            assert!(store.exists(&oid).unwrap());
            assert_eq!(store.read(&oid).unwrap().content, content);
        }
    }

    #[test]
    fn cross_pack_delta_cycles_hit_the_hop_limit() {
        let (_temp, store, pack_dir) = store_with_pack_dir();
        let x = Oid::from_hex("1111111111111111111111111111111111111111").unwrap();
        let y = Oid::from_hex("2222222222222222222222222222222222222222").unwrap();
        let delta = append_delta(b"", b"x");
        let mut a = Builder::default();
        a.ref_delta(x, y, &delta);
        a.write_named(&pack_dir, "pack-a", 2);
        let mut b = Builder::default();
        b.ref_delta(y, x, &delta);
        b.write_named(&pack_dir, "pack-b", 2);
        assert!(matches!(
            store.read(&x),
            Err(Error::PackLimitExceeded { .. })
        ));
    }

    #[test]
    fn duplicate_objects_are_reported_once_by_prefix() {
        let (_temp, store, pack_dir) = store_with_pack_dir();
        let content = b"duplicated";
        let oid = store.write(ObjectType::Blob, content).unwrap();
        for name in ["pack-a", "pack-b"] {
            let mut builder = Builder::default();
            builder.object(ObjectType::Blob, content);
            builder.write_named(&pack_dir, name, 2);
        }
        let hex = oid.to_hex();
        assert_eq!(store.find_objects_by_prefix(&hex[..4]).unwrap(), vec![oid]);
        assert_eq!(store.resolve_oid(&hex[..7]).unwrap(), oid);
        // Packed copies stay readable once the loose copy is gone.
        fs::remove_file(store.inner.loose.oid_to_path(&oid)).unwrap();
        assert_eq!(store.read(&oid).unwrap().content, content);
    }

    #[test]
    fn pack_directory_is_scanned_lazily_and_rescanned_on_miss() {
        let temp = TempDir::new().unwrap();
        let store = ObjectStore::new(temp.path());
        assert!(store.packs(false).unwrap().0.is_empty());
        let (_, changed) = store.packs(true).unwrap();
        assert!(!changed);

        // A pack without its index is still being written and is ignored.
        let pack_dir = temp.path().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(pack_dir.join("pack-a.pack"), b"partial").unwrap();
        assert!(!store.packs(true).unwrap().1);

        // An unreadable pack is reported, never silently skipped.
        fs::write(pack_dir.join("pack-a.idx"), b"broken").unwrap();
        let missing = Oid::from_hex("abcd000000000000000000000000000000000000").unwrap();
        assert!(matches!(
            store.read(&missing),
            Err(Error::InvalidPackIndex { .. })
        ));
    }
}
