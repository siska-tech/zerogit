//! The cache tree (`TREE`) index extension: the tree object of each
//! directory of the index, kept while the directory's entries are
//! unchanged, so a commit does not have to rebuild every tree.
//!
//! Each node records how many index entries it covers and its tree OID, or
//! that it is invalid (`-1`) because an entry under it changed.

use crate::objects::Oid;

/// A node of the cache tree: the root (with an empty name) or a directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CacheTree {
    /// The number of index entries under this directory and its tree OID,
    /// or `None` when invalid.
    pub(crate) valid: Option<(usize, Oid)>,
    /// Subdirectories, by name, in the order Git keeps them.
    pub(crate) children: Vec<(Vec<u8>, CacheTree)>,
}

impl CacheTree {
    /// Parses the extension data. Returns `None` if it is malformed, in
    /// which case the cache is dropped (Git would rebuild it).
    pub(crate) fn parse(data: &[u8]) -> Option<CacheTree> {
        let mut pos = 0;
        let (name, tree) = parse_node(data, &mut pos)?;
        if !name.is_empty() || pos != data.len() {
            return None;
        }
        Some(tree)
    }

    /// Serializes the extension data.
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        write_node(b"", self, out);
    }

    /// Invalidates the directories leading to `path` (`/`-separated), as an
    /// entry at that path was added, changed or removed. A subdirectory with
    /// the same name as the path itself is dropped: a file replaced it.
    pub(crate) fn invalidate(&mut self, path: &[u8]) {
        self.valid = None;
        match path.iter().position(|&b| b == b'/') {
            Some(slash) => {
                let (dir, rest) = (&path[..slash], &path[slash + 1..]);
                if let Some(child) = self.child_mut(dir) {
                    child.invalidate(rest);
                }
            }
            None => self.children.retain(|(name, _)| name != path),
        }
    }

    /// The subdirectory named `name`.
    pub(crate) fn child(&self, name: &[u8]) -> Option<&CacheTree> {
        self.children
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, tree)| tree)
    }

    fn child_mut(&mut self, name: &[u8]) -> Option<&mut CacheTree> {
        self.children
            .iter_mut()
            .find(|(n, _)| n == name)
            .map(|(_, tree)| tree)
    }

    /// Whether every node is valid.
    #[cfg(test)]
    pub(crate) fn fully_valid(&self) -> bool {
        self.valid.is_some() && self.children.iter().all(|(_, c)| c.fully_valid())
    }
}

/// Git orders subtrees by name length, then by bytes.
pub(crate) fn subtree_order(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn parse_node(data: &[u8], pos: &mut usize) -> Option<(Vec<u8>, CacheTree)> {
    let nul = data[*pos..].iter().position(|&b| b == 0)?;
    let name = data[*pos..*pos + nul].to_vec();
    *pos += nul + 1;
    let newline = data[*pos..].iter().position(|&b| b == b'\n')?;
    let header = std::str::from_utf8(&data[*pos..*pos + newline]).ok()?;
    *pos += newline + 1;
    let (count, subtrees) = header.split_once(' ')?;
    let count: i64 = count.parse().ok()?;
    let subtrees: usize = subtrees.parse().ok()?;
    let valid = if count >= 0 {
        let oid = data.get(*pos..*pos + 20)?;
        *pos += 20;
        Some((count as usize, Oid::from_bytes(oid.try_into().ok()?)))
    } else {
        None
    };
    // Each subtree takes at least a NUL, "0 0\n"; reject absurd counts.
    if subtrees > data.len() {
        return None;
    }
    let mut children = Vec::with_capacity(subtrees);
    for _ in 0..subtrees {
        children.push(parse_node(data, pos)?);
    }
    Some((name, CacheTree { valid, children }))
}

fn write_node(name: &[u8], tree: &CacheTree, out: &mut Vec<u8>) {
    out.extend_from_slice(name);
    out.push(0);
    let count = match &tree.valid {
        Some((count, _)) => count.to_string(),
        None => "-1".to_owned(),
    };
    out.extend_from_slice(format!("{} {}\n", count, tree.children.len()).as_bytes());
    if let Some((_, oid)) = &tree.valid {
        out.extend_from_slice(oid.as_bytes());
    }
    for (child_name, child) in &tree.children {
        write_node(child_name, child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(byte: u8) -> Oid {
        Oid::from_bytes([byte; 20])
    }

    fn sample() -> CacheTree {
        CacheTree {
            valid: Some((3, oid(1))),
            children: vec![
                (
                    b"a".to_vec(),
                    CacheTree {
                        valid: Some((2, oid(2))),
                        children: vec![(
                            b"b".to_vec(),
                            CacheTree {
                                valid: Some((1, oid(3))),
                                children: vec![],
                            },
                        )],
                    },
                ),
                (
                    b"zz".to_vec(),
                    CacheTree {
                        valid: None,
                        children: vec![],
                    },
                ),
            ],
        }
    }

    #[test]
    fn round_trip() {
        let tree = sample();
        let mut data = Vec::new();
        tree.write(&mut data);
        assert_eq!(CacheTree::parse(&data), Some(tree));
        assert!(data.starts_with(b"\x003 2\n"));
    }

    #[test]
    fn malformed_data_is_dropped() {
        assert_eq!(CacheTree::parse(b""), None);
        assert_eq!(CacheTree::parse(b"\x001 0\nshort"), None);
        assert_eq!(CacheTree::parse(b"\x00-1 5\n"), None);
        assert_eq!(CacheTree::parse(b"x\x00-1 0\n"), None);
    }

    #[test]
    fn invalidation_follows_the_path() {
        let mut tree = sample();
        tree.invalidate(b"a/b/file.txt");
        assert_eq!(tree.valid, None);
        let a = tree.child(b"a").unwrap();
        assert_eq!(a.valid, None);
        assert_eq!(a.child(b"b").unwrap().valid, None);

        let mut tree = sample();
        tree.invalidate(b"top.txt");
        assert_eq!(tree.valid, None);
        assert!(tree.child(b"a").unwrap().fully_valid());

        // A file named like a directory replaces it.
        let mut tree = sample();
        tree.invalidate(b"a");
        assert!(tree.child(b"a").is_none());
    }

    #[test]
    fn subtrees_are_ordered_by_length_first() {
        let mut names: Vec<&[u8]> = vec![b"bb", b"c", b"a", b"aa"];
        names.sort_by(|a, b| subtree_order(a, b));
        assert_eq!(names, [&b"a"[..], b"c", b"aa", b"bb"]);
    }
}
