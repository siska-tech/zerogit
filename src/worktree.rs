//! Working tree traversal.
//!
//! The walk skips only `.git`; files and directories whose names start with
//! `.` are treated like any other. Ignore rules are applied the way Git
//! applies them: an untracked path that is excluded is left out, an excluded
//! directory is not entered (so nothing inside it can be re-included), and a
//! tracked file is always kept, even when it matches an ignore pattern.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ignore::IgnoreRules;
use crate::index::Index;

/// The result of walking the working tree.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// Tracked files that exist, and untracked files that are not ignored.
    pub(crate) files: BTreeSet<PathBuf>,
    /// Untracked files that are ignored (only collected on request).
    pub(crate) ignored: BTreeSet<PathBuf>,
}

/// Walks the working tree, sorting files into kept and ignored.
///
/// `index` supplies the tracked paths. Ignored files are collected only when
/// `collect_ignored` is set, because that means entering every excluded
/// directory (`target/`, `node_modules/`, ...).
pub(crate) fn scan(
    work_dir: &Path,
    rules: &mut IgnoreRules,
    index: Option<&Index>,
    collect_ignored: bool,
) -> Result<Scan> {
    let mut tracked: HashSet<PathBuf> = HashSet::new();
    let mut tracked_dirs: HashSet<PathBuf> = HashSet::new();
    if let Some(index) = index {
        for entry in index.entries() {
            let path = native_path(entry.path());
            let mut parent = path.parent();
            while let Some(dir) = parent.filter(|d| !d.as_os_str().is_empty()) {
                if !tracked_dirs.insert(dir.to_path_buf()) {
                    break;
                }
                parent = dir.parent();
            }
            tracked.insert(path);
        }
    }
    let mut walker = Walker {
        work_dir,
        rules,
        tracked: &tracked,
        tracked_dirs: &tracked_dirs,
        collect_ignored,
        scan: Scan::default(),
    };
    walker.walk(Path::new(""), false)?;
    Ok(walker.scan)
}

/// Converts an index path (`/`-separated) to the platform's form, matching
/// the paths produced by the walk.
pub(crate) fn native_path(path: &Path) -> PathBuf {
    path.components().collect()
}

struct Walker<'a> {
    work_dir: &'a Path,
    rules: &'a mut IgnoreRules,
    tracked: &'a HashSet<PathBuf>,
    tracked_dirs: &'a HashSet<PathBuf>,
    collect_ignored: bool,
    scan: Scan,
}

impl Walker<'_> {
    /// Walks `dir` (relative to the work tree). `excluded` is set inside an
    /// excluded directory, where only tracked files are kept.
    fn walk(&mut self, dir: &Path, excluded: bool) -> Result<()> {
        let full = self.work_dir.join(dir);
        let entries = fs::read_dir(&full).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::PathNotFound(full.clone())
            } else {
                Error::Io(e)
            }
        })?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let path = dir.join(&name);
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let child_excluded = excluded || self.rules.is_excluded(&path, true)?;
                if !child_excluded || self.collect_ignored || self.tracked_dirs.contains(&path) {
                    self.walk(&path, child_excluded)?;
                }
            } else if file_type.is_file() {
                if self.tracked.contains(&path) {
                    self.scan.files.insert(path);
                } else if excluded || self.rules.is_excluded(&path, false)? {
                    if self.collect_ignored {
                        self.scan.ignored.insert(path);
                    }
                } else {
                    self.scan.files.insert(path);
                }
            }
            // Symlinks and other special files are skipped.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::index::IndexEntry;
    use crate::objects::{tree::FileMode, Oid};
    use tempfile::TempDir;

    fn setup(files: &[&str]) -> TempDir {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join(".git/config"), b"").unwrap();
        for file in files {
            let path = temp.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, file.as_bytes()).unwrap();
        }
        temp
    }

    fn files_of(temp: &TempDir, index: Option<&Index>, collect: bool) -> Scan {
        let mut rules =
            IgnoreRules::load(temp.path(), &temp.path().join(".git"), &Config::new()).unwrap();
        scan(temp.path(), &mut rules, index, collect).unwrap()
    }

    fn names(set: &BTreeSet<PathBuf>) -> Vec<String> {
        set.iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    fn tracked(paths: &[&str]) -> Index {
        let mut index = Index::empty(2);
        for path in paths {
            index.add(IndexEntry::new(
                0,
                0,
                0,
                0,
                FileMode::Regular,
                0,
                0,
                0,
                Oid::from_bytes([0; 20]),
                PathBuf::from(path),
                0,
            ));
        }
        index
    }

    #[test]
    fn test_scan_excludes_git_and_sorts() {
        let temp = setup(&["z.txt", "a.txt", "src/main.rs"]);
        fs::write(temp.path().join(".git/HEAD"), b"x").unwrap();
        let scan = files_of(&temp, None, false);
        assert_eq!(names(&scan.files), ["a.txt", "src/main.rs", "z.txt"]);
    }

    #[test]
    fn test_scan_empty() {
        let temp = setup(&[]);
        assert!(files_of(&temp, None, false).files.is_empty());
    }

    #[test]
    fn test_scan_includes_dotfiles() {
        let temp = setup(&[".github/workflows/ci.yml", ".env.example", ".gitignore"]);
        fs::write(temp.path().join(".gitignore"), "# nothing\n").unwrap();
        let scan = files_of(&temp, None, false);
        assert_eq!(
            names(&scan.files),
            [".env.example", ".github/workflows/ci.yml", ".gitignore"]
        );
    }

    #[test]
    fn test_scan_applies_ignore_rules() {
        let temp = setup(&[
            "keep.txt",
            "debug.log",
            "target/out.bin",
            "docs/a.md",
            "docs/.gitignore",
            "docs/draft.md",
        ]);
        fs::write(temp.path().join(".gitignore"), "*.log\ntarget/\n").unwrap();
        fs::write(temp.path().join("docs/.gitignore"), "draft.md\n").unwrap();
        let scan = files_of(&temp, None, true);
        assert_eq!(
            names(&scan.files),
            [".gitignore", "docs/.gitignore", "docs/a.md", "keep.txt"]
        );
        assert_eq!(
            names(&scan.ignored),
            ["debug.log", "docs/draft.md", "target/out.bin"]
        );
        // Without collecting, ignored directories are not entered.
        assert!(files_of(&temp, None, false).ignored.is_empty());
    }

    #[test]
    fn test_scan_keeps_tracked_ignored_files() {
        let temp = setup(&["target/tracked.txt", "target/other.txt", "app.log"]);
        fs::write(temp.path().join(".gitignore"), "*.log\ntarget/\n").unwrap();
        let index = tracked(&["target/tracked.txt", "app.log"]);
        let scan = files_of(&temp, Some(&index), false);
        assert_eq!(
            names(&scan.files),
            [".gitignore", "app.log", "target/tracked.txt"]
        );
    }

    #[test]
    fn test_negation_cannot_reinclude_inside_excluded_directory() {
        let temp = setup(&["build/keep.txt", "logs/a.log", "logs/keep.log"]);
        fs::write(
            temp.path().join(".gitignore"),
            "build/\n!build/keep.txt\nlogs/*\n!logs/keep.log\n",
        )
        .unwrap();
        let scan = files_of(&temp, None, false);
        assert_eq!(names(&scan.files), [".gitignore", "logs/keep.log"]);
    }
}
