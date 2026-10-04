//! Ignore rules (`.gitignore`, `.git/info/exclude`, `core.excludesFile`).
//!
//! Patterns follow [gitignore](https://git-scm.com/docs/gitignore): `!`
//! negation, a trailing `/` for directories only, a leading or inner `/` to
//! anchor a pattern to its `.gitignore` directory, `*`, `?`, `[...]`, `**`,
//! backslash escapes, comments and trailing spaces. Matching uses the same
//! algorithm as Git's `wildmatch`.
//!
//! Precedence is Git's: a `.gitignore` in a deeper directory overrides one
//! higher up, all `.gitignore` files override `.git/info/exclude`, which
//! overrides `core.excludesFile`. Within one file the last matching pattern
//! wins. A path inside an excluded directory is excluded too and cannot be
//! re-included by a negative pattern; the working tree walk enforces that by
//! not descending into excluded directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::Result;

/// One parsed pattern.
#[derive(Debug, Clone)]
struct Pattern {
    /// The pattern text, without `!`, a leading `/` or a trailing `/`.
    glob: Vec<u8>,
    negative: bool,
    /// Matches directories only (pattern ended with `/`).
    dir_only: bool,
    /// The pattern has no `/` and matches the last path component at any depth.
    basename_only: bool,
}

/// The patterns of one source, with the directory they are relative to.
#[derive(Debug, Clone, Default)]
struct PatternList {
    /// `/`-separated directory relative to the work tree, empty for the root
    /// and for the repository-wide sources.
    base: Vec<u8>,
    patterns: Vec<Pattern>,
}

impl PatternList {
    fn parse(base: Vec<u8>, content: &[u8]) -> Self {
        let content = content.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(content);
        let mut patterns = Vec::new();
        for line in content.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.first() == Some(&b'#') {
                continue;
            }
            if let Some(pattern) = parse_pattern(trim_trailing_spaces(line)) {
                patterns.push(pattern);
            }
        }
        PatternList { base, patterns }
    }

    /// The decision of the last matching pattern, if any.
    fn decide(&self, path: &[u8], is_dir: bool, icase: bool) -> Option<bool> {
        let relative = if self.base.is_empty() {
            path
        } else {
            let rest = path.strip_prefix(self.base.as_slice())?;
            rest.strip_prefix(b"/")?
        };
        let basename = match relative.iter().rposition(|&b| b == b'/') {
            Some(pos) => &relative[pos + 1..],
            None => relative,
        };
        for pattern in self.patterns.iter().rev() {
            if pattern.dir_only && !is_dir {
                continue;
            }
            let text = if pattern.basename_only {
                basename
            } else {
                relative
            };
            if wildmatch(&pattern.glob, text, icase) {
                return Some(!pattern.negative);
            }
        }
        None
    }
}

/// Removes trailing spaces that are not escaped with a backslash.
fn trim_trailing_spaces(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' {
        // Count the backslashes before this space.
        let backslashes = line[..end - 1]
            .iter()
            .rev()
            .take_while(|&&b| b == b'\\')
            .count();
        if backslashes % 2 == 1 {
            break;
        }
        end -= 1;
    }
    &line[..end]
}

fn parse_pattern(line: &[u8]) -> Option<Pattern> {
    let (negative, mut glob) = match line.strip_prefix(b"!") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let dir_only = glob.last() == Some(&b'/');
    if dir_only {
        glob = &glob[..glob.len() - 1];
    }
    if glob.is_empty() {
        return None;
    }
    let basename_only = !glob.contains(&b'/');
    if let Some(rest) = glob.strip_prefix(b"/") {
        glob = rest;
    }
    Some(Pattern {
        glob: glob.to_vec(),
        negative,
        dir_only,
        basename_only,
    })
}

/// Ignore rules for a working tree.
///
/// Per-directory `.gitignore` files are read lazily, the first time a path
/// below their directory is checked.
#[derive(Debug)]
pub(crate) struct IgnoreRules {
    work_dir: PathBuf,
    /// `core.excludesFile` then `.git/info/exclude` (lowest priority first).
    global: Vec<PatternList>,
    /// `.gitignore` of each directory read so far, keyed by its `/`-separated path.
    per_dir: HashMap<Vec<u8>, Option<PatternList>>,
    icase: bool,
}

impl IgnoreRules {
    /// Loads the repository-wide sources. `config` supplies
    /// `core.excludesFile` and `core.ignoreCase`.
    pub(crate) fn load(work_dir: &Path, git_dir: &Path, config: &Config) -> Result<Self> {
        let mut global = Vec::new();
        let excludes_file = match config.get("core", "excludesfile") {
            Some(path) => crate::config::expand_home(path),
            None => crate::config::xdg_git_path("ignore"),
        };
        if let Some(path) = excludes_file {
            if let Some(content) = read_optional(&path)? {
                global.push(PatternList::parse(Vec::new(), &content));
            }
        }
        if let Some(content) = read_optional(&git_dir.join("info").join("exclude"))? {
            global.push(PatternList::parse(Vec::new(), &content));
        }
        let icase = config.get_bool_or("core", "ignorecase", false);
        Ok(IgnoreRules {
            work_dir: work_dir.to_path_buf(),
            global,
            per_dir: HashMap::new(),
            icase,
        })
    }

    /// Returns whether `path` (relative to the work tree, `/`-separated) is
    /// excluded by the rules themselves.
    ///
    /// This does not consider whether a parent directory is excluded; see
    /// [`IgnoreRules::is_ignored`] for that.
    pub(crate) fn is_excluded(&mut self, path: &Path, is_dir: bool) -> Result<bool> {
        let key = path_bytes(path);
        // Directories from the path's parent up to the root, deepest first.
        let mut dirs = Vec::new();
        let mut end = key.len();
        while let Some(pos) = key[..end].iter().rposition(|&b| b == b'/') {
            dirs.push(key[..pos].to_vec());
            end = pos;
        }
        dirs.push(Vec::new());
        let icase = self.icase;
        for dir in dirs {
            if let Some(list) = self.dir_patterns(&dir)? {
                if let Some(decision) = list.decide(&key, is_dir, icase) {
                    return Ok(decision);
                }
            }
        }
        for list in self.global.iter().rev() {
            if let Some(decision) = list.decide(&key, is_dir, self.icase) {
                return Ok(decision);
            }
        }
        Ok(false)
    }

    /// Returns whether `path` is ignored: it or one of its parent directories
    /// is excluded.
    pub(crate) fn is_ignored(&mut self, path: &Path, is_dir: bool) -> Result<bool> {
        let mut prefix = PathBuf::new();
        let components: Vec<_> = path.components().collect();
        for (i, component) in components.iter().enumerate() {
            prefix.push(component);
            let last = i + 1 == components.len();
            if self.is_excluded(&prefix, if last { is_dir } else { true })? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn dir_patterns(&mut self, dir: &[u8]) -> Result<Option<&PatternList>> {
        if !self.per_dir.contains_key(dir) {
            let mut path = self.work_dir.clone();
            if !dir.is_empty() {
                path.push(String::from_utf8_lossy(dir).as_ref());
            }
            path.push(".gitignore");
            let list = read_optional(&path)?.map(|c| PatternList::parse(dir.to_vec(), &c));
            self.per_dir.insert(dir.to_vec(), list);
        }
        Ok(self.per_dir.get(dir).and_then(Option::as_ref))
    }
}

/// Reads a file, treating a missing file (or a directory in its place) as absent.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::PermissionDenied
                || path.is_dir() =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// The `/`-separated bytes of a relative path.
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().replace('\\', "/").into_bytes()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wild {
    Match,
    NoMatch,
    AbortAll,
    AbortToStarStar,
}

/// Matches `text` against a glob the way Git's `wildmatch` does with
/// `WM_PATHNAME`: `*` and `?` do not match `/`, `**` between slashes matches
/// any number of directories.
pub(crate) fn wildmatch(pattern: &[u8], text: &[u8], icase: bool) -> bool {
    dowild(pattern, text, icase) == Wild::Match
}

fn fold(c: u8, icase: bool) -> u8 {
    if icase {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

fn dowild(pattern: &[u8], text: &[u8], icase: bool) -> Wild {
    let mut p = 0;
    let mut t = 0;
    while p < pattern.len() {
        let mut p_ch = pattern[p];
        if t >= text.len() && p_ch != b'*' {
            return Wild::AbortAll;
        }
        match p_ch {
            b'\\' => {
                // A literal character; a trailing backslash matches nothing.
                p += 1;
                if p >= pattern.len() {
                    return Wild::NoMatch;
                }
                p_ch = pattern[p];
                if fold(text[t], icase) != fold(p_ch, icase) {
                    return Wild::NoMatch;
                }
            }
            b'?' => {
                if text[t] == b'/' {
                    return Wild::NoMatch;
                }
            }
            b'*' => {
                let match_slash;
                p += 1;
                if pattern.get(p) == Some(&b'*') {
                    let prev_is_boundary = p < 2 || pattern[p - 2] == b'/';
                    while pattern.get(p) == Some(&b'*') {
                        p += 1;
                    }
                    let next = pattern.get(p).copied();
                    let next_is_boundary = next.is_none()
                        || next == Some(b'/')
                        || (next == Some(b'\\') && pattern.get(p + 1) == Some(&b'/'));
                    if prev_is_boundary && next_is_boundary {
                        // "**/" may match no directory at all.
                        if next == Some(b'/')
                            && dowild(&pattern[p + 1..], &text[t..], icase) == Wild::Match
                        {
                            return Wild::Match;
                        }
                        match_slash = true;
                    } else {
                        match_slash = false;
                    }
                } else {
                    match_slash = false;
                }
                if p >= pattern.len() {
                    // A trailing "**" matches everything; a trailing "*" only
                    // the rest of this path component.
                    if !match_slash && text[t..].contains(&b'/') {
                        return Wild::NoMatch;
                    }
                    return Wild::Match;
                }
                if !match_slash && pattern[p] == b'/' {
                    // A single "*" followed by "/" matches up to the next slash.
                    match text[t..].iter().position(|&b| b == b'/') {
                        Some(pos) => {
                            t += pos + 1;
                            p += 1;
                            continue;
                        }
                        None => return Wild::NoMatch,
                    }
                }
                while t < text.len() {
                    match dowild(&pattern[p..], &text[t..], icase) {
                        Wild::NoMatch => {
                            if !match_slash && text[t] == b'/' {
                                return Wild::AbortToStarStar;
                            }
                        }
                        Wild::AbortToStarStar if match_slash => {}
                        other => return other,
                    }
                    t += 1;
                }
                return Wild::AbortAll;
            }
            b'[' => {
                let t_ch = text[t];
                if t_ch == b'/' {
                    return Wild::NoMatch;
                }
                match match_bracket(pattern, p, t_ch, icase) {
                    Some((matched, end)) => {
                        if !matched {
                            return Wild::NoMatch;
                        }
                        p = end;
                    }
                    // An unterminated bracket matches nothing, as in Git.
                    None => return Wild::AbortAll,
                }
            }
            _ => {
                if fold(text[t], icase) != fold(p_ch, icase) {
                    return Wild::NoMatch;
                }
            }
        }
        p += 1;
        t += 1;
    }
    if t >= text.len() {
        Wild::Match
    } else {
        Wild::NoMatch
    }
}

/// Matches one character against the bracket expression starting at
/// `pattern[start] == b'['`. Returns whether it matched and the index of the
/// closing `]`, or `None` if the bracket is not terminated.
fn match_bracket(pattern: &[u8], start: usize, t_ch: u8, icase: bool) -> Option<(bool, usize)> {
    let mut p = start + 1;
    let negated = matches!(pattern.get(p), Some(b'!') | Some(b'^'));
    if negated {
        p += 1;
    }
    let t_fold = fold(t_ch, icase);
    let mut matched = false;
    let mut prev: Option<u8> = None;
    let mut first = true;
    loop {
        let mut c = *pattern.get(p)?;
        if c == b']' && !first {
            break;
        }
        first = false;
        if c == b'\\' {
            p += 1;
            c = *pattern.get(p)?;
            if t_ch == c || t_fold == fold(c, icase) {
                matched = true;
            }
            prev = Some(c);
        } else if c == b'-'
            && prev.is_some()
            && pattern.get(p + 1).is_some()
            && pattern[p + 1] != b']'
        {
            p += 1;
            let mut hi = pattern[p];
            if hi == b'\\' {
                p += 1;
                hi = *pattern.get(p)?;
            }
            let lo = prev.unwrap();
            if lo <= t_ch && t_ch <= hi {
                matched = true;
            }
            if icase && t_ch.is_ascii_alphabetic() {
                let other = if t_ch.is_ascii_lowercase() {
                    t_ch.to_ascii_uppercase()
                } else {
                    t_ch.to_ascii_lowercase()
                };
                if lo <= other && other <= hi {
                    matched = true;
                }
            }
            prev = None;
        } else if c == b'[' && pattern.get(p + 1) == Some(&b':') {
            let class_start = p + 2;
            let len = pattern[class_start..].windows(2).position(|w| w == b":]")?;
            let class = &pattern[class_start..class_start + len];
            p = class_start + len + 1;
            let in_class = match class {
                b"alnum" => t_ch.is_ascii_alphanumeric(),
                b"alpha" => t_ch.is_ascii_alphabetic(),
                b"blank" => t_ch == b' ' || t_ch == b'\t',
                b"cntrl" => t_ch.is_ascii_control(),
                b"digit" => t_ch.is_ascii_digit(),
                b"graph" => t_ch.is_ascii_graphic(),
                b"lower" => t_ch.is_ascii_lowercase() || (icase && t_ch.is_ascii_uppercase()),
                b"print" => t_ch.is_ascii_graphic() || t_ch == b' ',
                b"punct" => t_ch.is_ascii_punctuation(),
                b"space" => t_ch.is_ascii_whitespace() || t_ch == 0x0b,
                b"upper" => t_ch.is_ascii_uppercase() || (icase && t_ch.is_ascii_lowercase()),
                b"xdigit" => t_ch.is_ascii_hexdigit(),
                // An unknown class makes the whole pattern fail.
                _ => return Some((false, p)),
            };
            if in_class {
                matched = true;
            }
            prev = None;
        } else {
            if t_ch == c || t_fold == fold(c, icase) {
                matched = true;
            }
            prev = Some(c);
        }
        p += 1;
    }
    Some((matched != negated, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm(pattern: &str, text: &str) -> bool {
        wildmatch(pattern.as_bytes(), text.as_bytes(), false)
    }

    // Cases from Git's t/t3070-wildmatch.sh (pathname mode).
    #[test]
    fn test_wildmatch_basics() {
        assert!(wm("foo", "foo"));
        assert!(!wm("bar", "foo"));
        assert!(wm("???", "foo"));
        assert!(!wm("??", "foo"));
        assert!(wm("*", "foo"));
        assert!(wm("f*", "foo"));
        assert!(!wm("*f", "foo"));
        assert!(wm("*foo*", "foo"));
        assert!(wm("*ob*a*r*", "foobar"));
        assert!(wm("*ab", "aaaaaaabababab"));
        assert!(wm("foo\\*", "foo*"));
        assert!(!wm("foo\\*bar", "foobar"));
        assert!(wm("f\\\\oo", "f\\oo"));
        assert!(wm("*[al]?", "ball"));
        assert!(!wm("[ten]", "ten"));
        assert!(wm("t[a-g]n", "ten"));
        assert!(!wm("t[!a-g]n", "ten"));
        assert!(wm("t[!a-g]n", "ton"));
        assert!(wm("t[^a-g]n", "ton"));
        assert!(wm("a[]]b", "a]b"));
        assert!(wm("a[]-]b", "a-b"));
        assert!(wm("a[]-]b", "a]b"));
        assert!(!wm("a[]-]b", "aab"));
        assert!(wm("a[]a-]b", "aab"));
        assert!(wm("]", "]"));
    }

    #[test]
    fn test_wildmatch_slashes() {
        assert!(!wm("foo*bar", "foo/baz/bar"));
        assert!(!wm("foo**bar", "foo/baz/bar"));
        assert!(wm("foo**bar", "foobazbar"));
        assert!(wm("foo/**/bar", "foo/baz/bar"));
        assert!(wm("foo/**/**/bar", "foo/baz/bar"));
        assert!(wm("foo/**/bar", "foo/b/a/z/bar"));
        assert!(wm("foo/**/**/bar", "foo/b/a/z/bar"));
        assert!(wm("foo/**/bar", "foo/bar"));
        assert!(wm("foo/**/**/bar", "foo/bar"));
        assert!(!wm("foo?bar", "foo/bar"));
        assert!(!wm("foo[/]bar", "foo/bar"));
        assert!(!wm("f[^eiu][^eiu][^eiu][^eiu][^eiu]r", "foo/bar"));
        assert!(wm("f[^eiu][^eiu][^eiu][^eiu][^eiu]r", "foo-bar"));
        assert!(wm("**/foo", "foo"));
        assert!(wm("**/foo", "XXX/foo"));
        assert!(wm("**/foo", "bar/baz/foo"));
        assert!(!wm("*/foo", "bar/baz/foo"));
        assert!(!wm("**/bar*", "foo/bar/baz"));
        assert!(wm("**/bar/*", "deep/foo/bar/baz"));
        assert!(!wm("**/bar/*", "deep/foo/bar/baz/"));
        assert!(wm("**/bar/**", "deep/foo/bar/baz/"));
        assert!(!wm("**/bar/*", "deep/foo/bar"));
        assert!(wm("**/bar/**", "deep/foo/bar/"));
        assert!(wm("*/bar/**", "foo/bar/baz/x"));
        assert!(!wm("*/bar/**", "deep/foo/bar/baz/x"));
        assert!(wm("**/bar/*/*", "deep/foo/bar/baz/x"));
        assert!(wm("abc/**", "abc/x/y"));
        assert!(!wm("abc/**", "abc"));
        assert!(wm("a/**/b", "a/b"));
    }

    #[test]
    fn test_wildmatch_classes_and_case() {
        assert!(wm("[[:alpha:]][[:digit:]][[:upper:]]", "a1B"));
        assert!(!wm("[[:digit:][:upper:][:space:]]", "a"));
        assert!(wm("[[:digit:][:upper:][:space:]]", "A"));
        assert!(wm("[[:xdigit:]]", "f"));
        assert!(!wm("[[:xdigit:]]", "g"));
        assert!(!wm("[abc", "abc"));
        assert!(!wm("FOO", "foo"));
        assert!(wildmatch(b"FOO", b"foo", true));
        assert!(wildmatch(b"[A-Z]oo", b"foo", true));
    }

    fn rules(content: &str) -> PatternList {
        PatternList::parse(Vec::new(), content.as_bytes())
    }

    #[test]
    fn test_pattern_parsing() {
        let list = rules("# comment\n\n\\#hash\n\\!bang\nfoo \nbar\\ \n!keep\nbuild/\r\n");
        let globs: Vec<_> = list
            .patterns
            .iter()
            .map(|p| String::from_utf8(p.glob.clone()).unwrap())
            .collect();
        assert_eq!(
            globs,
            ["\\#hash", "\\!bang", "foo", "bar\\ ", "keep", "build"]
        );
        assert!(list.patterns[4].negative);
        assert!(list.patterns[5].dir_only);
    }

    #[test]
    fn test_decide_rules() {
        let list = rules("*.log\n!important.log\n/root.txt\ndoc/*.txt\nbuild/\n");
        assert_eq!(list.decide(b"a.log", false, false), Some(true));
        assert_eq!(list.decide(b"sub/a.log", false, false), Some(true));
        assert_eq!(list.decide(b"important.log", false, false), Some(false));
        assert_eq!(list.decide(b"root.txt", false, false), Some(true));
        assert_eq!(list.decide(b"sub/root.txt", false, false), None);
        assert_eq!(list.decide(b"doc/a.txt", false, false), Some(true));
        assert_eq!(list.decide(b"doc/sub/a.txt", false, false), None);
        assert_eq!(list.decide(b"build", true, false), Some(true));
        assert_eq!(list.decide(b"build", false, false), None);
        assert_eq!(list.decide(b"x/build", true, false), Some(true));

        let nested = PatternList::parse(b"sub".to_vec(), b"/only.txt\n");
        assert_eq!(nested.decide(b"sub/only.txt", false, false), Some(true));
        assert_eq!(nested.decide(b"only.txt", false, false), None);
        assert_eq!(nested.decide(b"sub/deeper/only.txt", false, false), None);
    }
}
