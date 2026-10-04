//! Pathspecs as Git matches them by default: a path, a directory (for
//! everything under it), or a glob in which `*`, `?` and `[...]` also match
//! `/` (Git's pathspec globs are not limited to one directory level).

use std::path::Path;

/// A list of pathspecs; a path matches if any of them matches.
#[derive(Debug, Clone)]
pub(crate) struct Pathspec {
    patterns: Vec<Vec<u8>>,
}

impl Pathspec {
    /// Builds pathspecs from paths relative to the repository root. `.`
    /// and an empty path match everything; `./` prefixes and trailing `/`
    /// are ignored.
    pub(crate) fn new<P: AsRef<Path>>(paths: &[P]) -> Self {
        let patterns = paths
            .iter()
            .map(|p| {
                let mut s = p.as_ref().to_string_lossy().replace('\\', "/");
                while let Some(rest) = s.strip_prefix("./") {
                    s = rest.to_owned();
                }
                let s = s.trim_end_matches('/');
                (if s == "." { "" } else { s }).as_bytes().to_vec()
            })
            .collect();
        Pathspec { patterns }
    }

    /// The pathspecs as given (normalized), for error messages.
    pub(crate) fn patterns(&self) -> impl Iterator<Item = String> + '_ {
        self.patterns
            .iter()
            .map(|p| String::from_utf8_lossy(p).into_owned())
    }

    /// Whether `path` (`/`-separated) matches any pathspec.
    pub(crate) fn matches(&self, path: &[u8]) -> bool {
        self.patterns.iter().any(|p| matches_one(p, path))
    }

    /// Whether `path` matches the pathspec `pattern` (as returned by
    /// [`Pathspec::patterns`]).
    pub(crate) fn matches_pattern(pattern: &str, path: &[u8]) -> bool {
        matches_one(pattern.as_bytes(), path)
    }

    /// Whether `pattern` names `path` only as a directory (not as the
    /// path itself or through a glob), which `git rm` refuses without
    /// `-r`.
    pub(crate) fn matches_as_directory(pattern: &str, path: &[u8]) -> bool {
        let pattern = pattern.as_bytes();
        !has_glob(pattern) && path != pattern && matches_one(pattern, path)
    }
}

fn has_glob(pattern: &[u8]) -> bool {
    pattern.iter().any(|b| matches!(b, b'*' | b'?' | b'['))
}

fn matches_one(pattern: &[u8], path: &[u8]) -> bool {
    if pattern.is_empty() {
        return true;
    }
    // A literal path, or a directory and everything under it.
    if path == pattern || (path.starts_with(pattern) && path.get(pattern.len()) == Some(&b'/')) {
        return true;
    }
    has_glob(pattern) && fnmatch(pattern, path)
}

/// `fnmatch` without `FNM_PATHNAME`: `*` matches any string, `/` included.
fn fnmatch(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // The last `*` and the text position it is retried from.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(b'?') => Some(p + 1),
            Some(b'[') => match crate::ignore::match_bracket(pattern, p, text[t], false) {
                Some((true, end)) => Some(end + 1),
                Some((false, _)) => None,
                // An unterminated bracket is a literal `[`.
                None => (text[t] == b'[').then_some(p + 1),
            },
            Some(b'\\') if pattern.get(p + 1) == Some(&text[t]) => Some(p + 2),
            Some(&c) if c == text[t] && c != b'\\' => Some(p + 1),
            _ => None,
        };
        match step {
            Some(next) => {
                p = next;
                t += 1;
            }
            None => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(paths: &[&str]) -> Pathspec {
        Pathspec::new(paths)
    }

    #[test]
    fn literal_paths_and_directories() {
        let s = spec(&["src", "README.md"]);
        assert!(s.matches(b"src/lib.rs"));
        assert!(s.matches(b"src/deep/mod.rs"));
        assert!(s.matches(b"README.md"));
        assert!(!s.matches(b"srcx/lib.rs"));
        assert!(!s.matches(b"README.md.bak"));
        assert!(spec(&["."]).matches(b"any/thing"));
        assert!(spec(&["./src/"]).matches(b"src/lib.rs"));
    }

    #[test]
    fn globs_cross_directories() {
        let s = spec(&["*.txt"]);
        assert!(s.matches(b"a.txt"));
        assert!(s.matches(b"dir/b.txt"));
        assert!(!s.matches(b"a.rs"));
        assert!(spec(&["dir/?.t[x]t"]).matches(b"dir/a.txt"));
        assert!(!spec(&["dir/?.t[!x]t"]).matches(b"dir/a.txt"));
        assert!(spec(&["d*"]).matches(b"dir/sub/c.rs"));
    }

    #[test]
    fn directory_matches_are_told_apart() {
        assert!(Pathspec::matches_as_directory("src", b"src/lib.rs"));
        assert!(!Pathspec::matches_as_directory("src/lib.rs", b"src/lib.rs"));
        assert!(!Pathspec::matches_as_directory("src/*", b"src/lib.rs"));
    }
}
