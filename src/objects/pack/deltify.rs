//! Choosing delta bases for the objects of a pack being written.
//!
//! Objects are sorted so that likely bases come just before what they can
//! encode: by type, then by name (the file name read backwards, so files
//! of the same name and then the same extension are together), then by
//! size, largest first. Each object is tried as a delta against the few
//! objects before it of the same type (the window), and the smallest delta
//! is kept, provided the chain of deltas leading to it stays short enough
//! (the depth). This is the approach `git pack-objects` documents for its
//! `--window` and `--depth` options.

use std::collections::{HashMap, VecDeque};

use super::delta::{self, DeltaIndex};
use crate::error::Result;
use crate::objects::{ObjectType, Oid};

/// How hard to look for deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeltaSearch {
    /// How many objects before each object are tried as its base
    /// (`pack.window`); 0 turns the search off.
    pub(crate) window: usize,
    /// The longest chain of deltas (`pack.depth`).
    pub(crate) depth: usize,
    /// Objects larger than this are neither encoded nor used as a base
    /// (`core.bigFileThreshold`).
    pub(crate) max_object_size: u64,
}

impl Default for DeltaSearch {
    fn default() -> Self {
        DeltaSearch {
            window: 10,
            depth: 50,
            max_object_size: 512 << 20,
        }
    }
}

impl DeltaSearch {
    /// The search configured by `pack.window`, `pack.depth` and
    /// `core.bigFileThreshold`.
    pub(crate) fn from_config(config: &crate::config::Config) -> Result<Self> {
        let mut search = DeltaSearch::default();
        if config.get("pack", "window").is_some() {
            search.window = usize::try_from(config.get_int("pack", "window")?.max(0)).unwrap_or(0);
        }
        if config.get("pack", "depth").is_some() {
            // Git caps the depth at 4095.
            search.depth = config.get_int("pack", "depth")?.clamp(0, 4095) as usize;
        }
        if config.get("core", "bigfilethreshold").is_some() {
            search.max_object_size =
                u64::try_from(config.get_int("core", "bigfilethreshold")?.max(0)).unwrap_or(0);
        }
        Ok(search)
    }
}

/// An object to be written to the pack.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) oid: Oid,
    pub(crate) object_type: ObjectType,
    /// The path the object was found at (empty for commits and tags).
    pub(crate) name: Vec<u8>,
    pub(crate) size: u64,
    /// The base it is already stored against, when an existing delta is
    /// copied as it is; it is not searched then.
    pub(crate) reused_base: Option<Oid>,
}

/// A delta found for an object: its base and the delta instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewDelta {
    pub(crate) base: Oid,
    pub(crate) data: Vec<u8>,
}

/// An object in the window, read and indexed only when needed.
struct Slot {
    candidate: usize,
    content: Option<Vec<u8>>,
    index: Option<Option<DeltaIndex>>,
}

/// The sort key of a name: its last component, backwards.
fn name_key(name: &[u8]) -> Vec<u8> {
    let file = name.rsplit(|&b| b == b'/').next().unwrap_or(name);
    file.iter().rev().copied().collect()
}

fn type_rank(object_type: ObjectType) -> u8 {
    match object_type {
        ObjectType::Commit => 0,
        ObjectType::Tree => 1,
        ObjectType::Blob => 2,
        ObjectType::Tag => 3,
    }
}

/// The length of the chain of deltas from `oid` down to a whole object.
fn depth_of(oid: &Oid, bases: &HashMap<Oid, Oid>) -> usize {
    let mut depth = 0;
    let mut current = oid;
    while let Some(base) = bases.get(current) {
        depth += 1;
        current = base;
    }
    depth
}

/// Whether `target` is on the chain of deltas from `oid`.
fn leads_to(oid: &Oid, target: &Oid, bases: &HashMap<Oid, Oid>) -> bool {
    let mut current = oid;
    loop {
        if current == target {
            return true;
        }
        match bases.get(current) {
            Some(base) => current = base,
            None => return false,
        }
    }
}

/// Finds deltas for `candidates` (which must not reuse deltas in a cycle),
/// reading object contents with `read` as they are needed. Every delta
/// found is checked to reproduce its object.
pub(crate) fn find_deltas(
    candidates: &[Candidate],
    search: &DeltaSearch,
    read: &mut dyn FnMut(&Oid) -> Result<Vec<u8>>,
) -> Result<HashMap<Oid, NewDelta>> {
    let mut found = HashMap::new();
    if search.window == 0 || search.depth == 0 {
        return Ok(found);
    }
    let mut bases: HashMap<Oid, Oid> = candidates
        .iter()
        .filter_map(|c| c.reused_base.map(|base| (c.oid, base)))
        .collect();
    // How long the reused chains built on each object are: encoding the
    // object as a delta makes them that much deeper.
    let mut height: HashMap<Oid, usize> = HashMap::new();
    for candidate in candidates {
        let mut current = candidate.oid;
        let mut distance = 0;
        while let Some(base) = bases.get(&current) {
            distance += 1;
            let entry = height.entry(*base).or_insert(0);
            *entry = (*entry).max(distance);
            current = *base;
        }
    }

    let keys: Vec<Vec<u8>> = candidates.iter().map(|c| name_key(&c.name)).collect();
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (&candidates[a], &candidates[b]);
        type_rank(x.object_type)
            .cmp(&type_rank(y.object_type))
            .then_with(|| keys[a].cmp(&keys[b]))
            .then_with(|| y.size.cmp(&x.size))
            .then_with(|| a.cmp(&b))
    });

    let mut window: VecDeque<Slot> = VecDeque::new();
    for position in order {
        let target = &candidates[position];
        if window
            .back()
            .is_some_and(|slot| candidates[slot.candidate].object_type != target.object_type)
        {
            window.clear();
        }
        let mut slot = Slot {
            candidate: position,
            content: None,
            index: None,
        };
        let searched = target.reused_base.is_none()
            && target.size <= search.max_object_size
            && !window.is_empty();
        // A delta must save at least half the object, less 20 bytes, as in
        // Git; deeper bases must save more.
        let room = (target.size / 2).saturating_sub(20) as usize;
        if searched && room > 0 {
            let content = read(&target.oid)?;
            let target_height = height.get(&target.oid).copied().unwrap_or(0);
            let mut best: Option<NewDelta> = None;
            for base_slot in window.iter_mut().rev() {
                let base = &candidates[base_slot.candidate];
                if base.size > search.max_object_size {
                    continue;
                }
                let base_depth = depth_of(&base.oid, &bases);
                if base_depth + 1 + target_height > search.depth
                    || leads_to(&base.oid, &target.oid, &bases)
                {
                    continue;
                }
                let mut max_size = room * (search.depth - base_depth) / search.depth.max(1);
                if let Some(best) = &best {
                    max_size = max_size.min(best.data.len().saturating_sub(1));
                }
                // Too different in size for a small delta.
                let grows = target.size.saturating_sub(base.size);
                if max_size == 0 || grows >= max_size as u64 || base.size < target.size / 32 {
                    continue;
                }
                if base_slot.content.is_none() {
                    base_slot.content = Some(read(&base.oid)?);
                }
                let base_content = base_slot.content.as_deref().expect("read above");
                let index = base_slot
                    .index
                    .get_or_insert_with(|| DeltaIndex::new(base_content));
                let Some(index) = index else {
                    continue;
                };
                if let Some(data) = delta::create(base_content, index, &content, max_size) {
                    // A delta that does not reproduce the object would lose it.
                    if delta::apply(base_content, &data, content.len() as u64)
                        .ok()
                        .as_deref()
                        == Some(&content[..])
                    {
                        best = Some(NewDelta {
                            base: base.oid,
                            data,
                        });
                    }
                }
            }
            if let Some(delta) = best {
                bases.insert(target.oid, delta.base);
                found.insert(target.oid, delta);
            }
            slot.content = Some(content);
        }
        window.push_back(slot);
        if window.len() > search.window {
            window.pop_front();
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::hash_object;

    fn blob(content: &[u8]) -> Oid {
        Oid::from_bytes(hash_object("blob", content))
    }

    fn versions(count: usize) -> Vec<Vec<u8>> {
        let mut content: Vec<u8> = Vec::new();
        (0..count)
            .map(|i| {
                for line in 0..30 {
                    content.extend(format!("version {} line {}\n", i, line).into_bytes());
                }
                content.clone()
            })
            .collect()
    }

    fn candidates(contents: &[Vec<u8>], name: &str) -> Vec<Candidate> {
        contents
            .iter()
            .map(|content| Candidate {
                oid: blob(content),
                object_type: ObjectType::Blob,
                name: name.as_bytes().to_vec(),
                size: content.len() as u64,
                reused_base: None,
            })
            .collect()
    }

    fn search(
        candidates: &[Candidate],
        contents: &HashMap<Oid, Vec<u8>>,
        options: &DeltaSearch,
    ) -> HashMap<Oid, NewDelta> {
        find_deltas(candidates, options, &mut |oid| Ok(contents[oid].clone())).unwrap()
    }

    #[test]
    fn versions_of_a_file_become_a_chain() {
        let contents = versions(8);
        let cands = candidates(&contents, "dir/file.txt");
        let by_oid: HashMap<Oid, Vec<u8>> = cands
            .iter()
            .zip(&contents)
            .map(|(c, v)| (c.oid, v.clone()))
            .collect();
        let found = search(&cands, &by_oid, &DeltaSearch::default());
        // The largest (newest) is whole; each other one is a delta against
        // the next larger.
        assert_eq!(found.len(), 7);
        assert!(!found.contains_key(&cands[7].oid));
        for i in 0..7 {
            let delta = &found[&cands[i].oid];
            assert_eq!(delta.base, cands[i + 1].oid);
            let base = &contents[i + 1];
            assert_eq!(
                delta::apply(base, &delta.data, u64::MAX).unwrap(),
                contents[i]
            );
            assert!(delta.data.len() < 40);
        }

        // The depth limits the chain.
        let shallow = DeltaSearch {
            depth: 2,
            ..DeltaSearch::default()
        };
        let found = search(&cands, &by_oid, &shallow);
        let bases: HashMap<Oid, Oid> = found.iter().map(|(o, d)| (*o, d.base)).collect();
        assert!(cands.iter().all(|c| depth_of(&c.oid, &bases) <= 2));
        assert!(found.len() >= 4);
        // No window, no deltas.
        let off = DeltaSearch {
            window: 0,
            ..DeltaSearch::default()
        };
        assert!(search(&cands, &by_oid, &off).is_empty());
    }

    #[test]
    fn reused_deltas_and_other_types_are_respected() {
        let contents = versions(4);
        let mut cands = candidates(&contents, "a.txt");
        let by_oid: HashMap<Oid, Vec<u8>> = cands
            .iter()
            .zip(&contents)
            .map(|(c, v)| (c.oid, v.clone()))
            .collect();
        // The largest is already a delta against the smallest: it is not
        // searched, and the smallest may not use a base leading back to it.
        cands[3].reused_base = Some(cands[0].oid);
        // A tree of the same content is never a base for a blob.
        let mut tree = cands[2].clone();
        tree.object_type = ObjectType::Tree;
        tree.oid = Oid::from_bytes([9; 20]);
        cands.push(tree);
        let mut by_oid = by_oid;
        by_oid.insert(Oid::from_bytes([9; 20]), contents[2].clone());

        let found = search(&cands, &by_oid, &DeltaSearch::default());
        assert!(!found.contains_key(&cands[3].oid));
        assert!(!found.contains_key(&Oid::from_bytes([9; 20])));
        let bases: HashMap<Oid, Oid> = found
            .iter()
            .map(|(o, d)| (*o, d.base))
            .chain([(cands[3].oid, cands[0].oid)])
            .collect();
        for c in &cands {
            assert!(depth_of(&c.oid, &bases) < cands.len(), "no cycle");
        }
        assert!(found.values().all(|d| d.base != Oid::from_bytes([9; 20])));
    }

    #[test]
    fn unrelated_and_small_objects_stay_whole() {
        let contents: Vec<Vec<u8>> = (0..5u8)
            .map(|i| {
                (0..2000u32)
                    .map(|j| (j * 7919 + u32::from(i) * 104_729) as u8 ^ i)
                    .collect()
            })
            .chain([b"tiny".to_vec(), b"tiny!".to_vec()])
            .collect();
        let cands = candidates(&contents, "x");
        let by_oid: HashMap<Oid, Vec<u8>> = cands
            .iter()
            .zip(&contents)
            .map(|(c, v)| (c.oid, v.clone()))
            .collect();
        let found = search(&cands, &by_oid, &DeltaSearch::default());
        assert!(!found.contains_key(&cands[5].oid));
        assert!(!found.contains_key(&cands[6].oid));
    }

    #[test]
    fn names_sort_by_file_name_backwards() {
        assert_eq!(name_key(b"src/main.rs"), b"sr.niam".to_vec());
        assert!(name_key(b"a/x.rs") < name_key(b"b/y.txt"));
        assert_eq!(name_key(b""), Vec::<u8>::new());
    }
}
