//! Path attributes (`.gitattributes`, `.git/info/attributes`,
//! `core.attributesFile`).
//!
//! Each line is a pattern followed by attribute assignments: `attr` (set),
//! `-attr` (unset), `attr=value`, or `!attr` (back to unspecified). Patterns
//! match like `.gitignore` patterns, except that negative patterns are not
//! allowed and a pattern ending in `/` matches no file. `[attr]name ...`
//! lines define macros; `binary` is predefined as `-diff -merge -text`.
//!
//! Precedence is Git's: `core.attributesFile` is lowest, then the
//! `.gitattributes` files from the root down to the file's directory, then
//! `.git/info/attributes`. Within that order, a later line overrides an
//! earlier one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::Result;
use crate::ignore::{read_optional, trim_trailing_spaces, Pattern};

/// The state of one attribute for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttrValue {
    /// `attr`
    Set,
    /// `-attr`
    Unset,
    /// `attr=value`
    Value(String),
}

#[derive(Debug, Clone)]
struct Assignment {
    name: String,
    /// `None` resets the attribute to unspecified (`!attr`).
    value: Option<AttrValue>,
}

#[derive(Debug, Clone)]
struct Line {
    pattern: Pattern,
    assignments: Vec<Assignment>,
}

#[derive(Debug, Clone, Default)]
struct AttrFile {
    /// `/`-separated directory the patterns are relative to.
    base: Vec<u8>,
    lines: Vec<Line>,
}

/// Attribute rules for a working tree.
#[derive(Debug)]
pub(crate) struct Attributes {
    work_dir: PathBuf,
    /// `core.attributesFile` (lowest priority).
    global: Option<AttrFile>,
    /// `.git/info/attributes` (highest priority).
    info: Option<AttrFile>,
    per_dir: HashMap<Vec<u8>, Option<AttrFile>>,
    macros: HashMap<String, Vec<Assignment>>,
    icase: bool,
}

impl Attributes {
    /// Loads the repository-wide attribute files.
    pub(crate) fn load(work_dir: &Path, git_dir: &Path, config: &Config) -> Result<Self> {
        let mut attributes = Attributes {
            work_dir: work_dir.to_path_buf(),
            global: None,
            info: None,
            per_dir: HashMap::new(),
            macros: HashMap::new(),
            icase: config.get_bool_or("core", "ignorecase", false),
        };
        attributes.macros.insert(
            "binary".to_owned(),
            ["diff", "merge", "text"]
                .iter()
                .map(|name| Assignment {
                    name: (*name).to_owned(),
                    value: Some(AttrValue::Unset),
                })
                .collect(),
        );
        let global_path = match config.get("core", "attributesfile") {
            Some(path) => crate::config::expand_home(path),
            None => crate::config::xdg_git_path("attributes"),
        };
        if let Some(path) = global_path {
            if let Some(content) = read_optional(&path)? {
                attributes.global = Some(attributes.parse(Vec::new(), &content, true));
            }
        }
        if let Some(content) = read_optional(&git_dir.join("info").join("attributes"))? {
            attributes.info = Some(attributes.parse(Vec::new(), &content, true));
        }
        // Macros may also be defined in the top-level .gitattributes.
        attributes.dir_file(&[])?;
        Ok(attributes)
    }

    fn parse(&mut self, base: Vec<u8>, content: &[u8], allow_macros: bool) -> AttrFile {
        let content = content.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(content);
        let mut lines = Vec::new();
        for raw in content.split(|&b| b == b'\n') {
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            let line = trim_trailing_spaces(raw);
            let line = trim_leading_whitespace(line);
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            let (pattern, rest) = split_pattern(line);
            let assignments = parse_assignments(rest);
            if let Some(name) = pattern.strip_prefix(b"[attr]") {
                if allow_macros {
                    let name = String::from_utf8_lossy(name).into_owned();
                    self.macros.insert(name, assignments);
                }
                continue;
            }
            match Pattern::parse(&pattern) {
                // Negative patterns are not allowed in attribute files.
                Some(pattern) if !pattern.is_negative() => lines.push(Line {
                    pattern,
                    assignments,
                }),
                _ => {}
            }
        }
        AttrFile { base, lines }
    }

    fn dir_file(&mut self, dir: &[u8]) -> Result<Option<&AttrFile>> {
        if !self.per_dir.contains_key(dir) {
            let mut path = self.work_dir.clone();
            if !dir.is_empty() {
                path.push(String::from_utf8_lossy(dir).as_ref());
            }
            path.push(".gitattributes");
            let file = read_optional(&path)?
                .map(|content| self.parse(dir.to_vec(), &content, dir.is_empty()));
            self.per_dir.insert(dir.to_vec(), file);
        }
        Ok(self.per_dir.get(dir).and_then(Option::as_ref))
    }

    /// Uses `content` as the `.gitattributes` of directory `dir` (`None`: no
    /// file) instead of reading the working tree, as Git does when checking
    /// out a tree whose attributes differ from the files on disk.
    pub(crate) fn set_dir_file(&mut self, dir: &[u8], content: Option<&[u8]>) {
        let file = content.map(|c| self.parse(dir.to_vec(), c, dir.is_empty()));
        self.per_dir.insert(dir.to_vec(), file);
    }

    /// Returns the values of the requested attributes for a file (relative
    /// to the work tree). An attribute that is unspecified is absent.
    pub(crate) fn lookup(
        &mut self,
        path: &Path,
        names: &[&str],
    ) -> Result<HashMap<String, AttrValue>> {
        let key = path.to_string_lossy().replace('\\', "/").into_bytes();
        // Directories from the root down to the file's directory.
        let mut dirs = vec![Vec::new()];
        for (i, &b) in key.iter().enumerate() {
            if b == b'/' {
                dirs.push(key[..i].to_vec());
            }
        }
        for dir in &dirs {
            self.dir_file(dir)?;
        }

        let mut files: Vec<&AttrFile> = Vec::new();
        files.extend(self.global.as_ref());
        for dir in &dirs {
            if let Some(Some(file)) = self.per_dir.get(dir) {
                files.push(file);
            }
        }
        files.extend(self.info.as_ref());

        let mut state: HashMap<String, Option<AttrValue>> = HashMap::new();
        for file in files {
            for line in &file.lines {
                if line.pattern.is_dir_only()
                    || !line.pattern.matches(&file.base, &key, false, self.icase)
                {
                    continue;
                }
                for assignment in &line.assignments {
                    self.apply(assignment, &mut state, 0);
                }
            }
        }
        Ok(names
            .iter()
            .filter_map(|name| {
                state
                    .get(*name)
                    .cloned()
                    .flatten()
                    .map(|value| ((*name).to_owned(), value))
            })
            .collect())
    }

    /// Applies an assignment, expanding a macro that is set.
    fn apply(
        &self,
        assignment: &Assignment,
        state: &mut HashMap<String, Option<AttrValue>>,
        depth: usize,
    ) {
        if depth < 8 && assignment.value == Some(AttrValue::Set) {
            if let Some(expansion) = self.macros.get(&assignment.name) {
                for inner in expansion {
                    self.apply(inner, state, depth + 1);
                }
            }
        }
        state.insert(assignment.name.clone(), assignment.value.clone());
    }
}

fn trim_leading_whitespace(line: &[u8]) -> &[u8] {
    let start = line
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(line.len());
    &line[start..]
}

/// Splits off the pattern, which may be a C-style quoted string.
fn split_pattern(line: &[u8]) -> (Vec<u8>, &[u8]) {
    if line.first() == Some(&b'"') {
        let mut pattern = Vec::new();
        let mut i = 1;
        while i < line.len() {
            match line[i] {
                b'"' => return (pattern, &line[i + 1..]),
                b'\\' if i + 1 < line.len() => {
                    i += 1;
                    pattern.push(match line[i] {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                other => pattern.push(other),
            }
            i += 1;
        }
        return (pattern, &[]);
    }
    let end = line
        .iter()
        .position(|&b| b == b' ' || b == b'\t')
        .unwrap_or(line.len());
    (line[..end].to_vec(), &line[end..])
}

fn parse_assignments(rest: &[u8]) -> Vec<Assignment> {
    rest.split(|&b| b == b' ' || b == b'\t')
        .filter(|token| !token.is_empty())
        .map(|token| {
            let token = String::from_utf8_lossy(token);
            if let Some(name) = token.strip_prefix('-') {
                Assignment {
                    name: name.to_owned(),
                    value: Some(AttrValue::Unset),
                }
            } else if let Some(name) = token.strip_prefix('!') {
                Assignment {
                    name: name.to_owned(),
                    value: None,
                }
            } else if let Some((name, value)) = token.split_once('=') {
                Assignment {
                    name: name.to_owned(),
                    value: Some(AttrValue::Value(value.to_owned())),
                }
            } else {
                Assignment {
                    name: token.into_owned(),
                    value: Some(AttrValue::Set),
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn attrs(temp: &TempDir, path: &str) -> HashMap<String, AttrValue> {
        let mut attributes =
            Attributes::load(temp.path(), &temp.path().join(".git"), &Config::new()).unwrap();
        attributes
            .lookup(Path::new(path), &["text", "eol", "diff", "custom"])
            .unwrap()
    }

    #[test]
    fn test_lookup_precedence_and_macros() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".git/info")).unwrap();
        fs::create_dir_all(temp.path().join("sub")).unwrap();
        fs::write(
            temp.path().join(".gitattributes"),
            "[attr]mine text eol=crlf\n*.txt text\n*.bin binary\n*.mine mine\n*.sh eol=lf\nnone.txt !text\n",
        )
        .unwrap();
        fs::write(temp.path().join("sub/.gitattributes"), "*.txt -text\n").unwrap();
        fs::write(
            temp.path().join(".git/info/attributes"),
            "forced.txt custom=yes\n",
        )
        .unwrap();

        let a = attrs(&temp, "a.txt");
        assert_eq!(a.get("text"), Some(&AttrValue::Set));
        let a = attrs(&temp, "sub/a.txt");
        assert_eq!(a.get("text"), Some(&AttrValue::Unset));
        let a = attrs(&temp, "data.bin");
        assert_eq!(a.get("text"), Some(&AttrValue::Unset));
        assert_eq!(a.get("diff"), Some(&AttrValue::Unset));
        let a = attrs(&temp, "x.mine");
        assert_eq!(a.get("text"), Some(&AttrValue::Set));
        assert_eq!(a.get("eol"), Some(&AttrValue::Value("crlf".into())));
        let a = attrs(&temp, "dir/run.sh");
        assert_eq!(a.get("eol"), Some(&AttrValue::Value("lf".into())));
        assert_eq!(a.get("text"), None);
        assert_eq!(attrs(&temp, "none.txt").get("text"), None);
        assert_eq!(
            attrs(&temp, "forced.txt").get("custom"),
            Some(&AttrValue::Value("yes".into()))
        );
    }
}
