//! Removing untracked files from the work tree: `git clean`.
//!
//! [`Repository::clean`] removes what `git clean -f` removes, with the
//! choices of [`CleanOptions`]: untracked directories (`-d`), ignored
//! files too (`-x`) or only ignored files (`-X`), pathspecs, and a dry run
//! (`-n`). Tracked files and nested repositories (directories with their
//! own `.git`) are never removed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ignore::IgnoreRules;
use crate::index::path_key;
use crate::infra::fs::remove_file;
use crate::pathspec::Pathspec;
use crate::repository::Repository;
use crate::worktree::native_path;

/// Which ignored files [`Repository::clean`] removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanIgnored {
    /// Keep them (Git's default).
    #[default]
    Keep,
    /// Remove them as well (`-x`).
    Include,
    /// Remove only them, keeping other untracked files (`-X`).
    Only,
}

/// Options for [`Repository::clean`].
#[derive(Debug, Clone, Default)]
pub struct CleanOptions {
    directories: bool,
    ignored: CleanIgnored,
    dry_run: bool,
    paths: Vec<PathBuf>,
}

impl CleanOptions {
    /// The default options: untracked files that are not ignored, outside
    /// untracked directories, everywhere in the work tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes untracked directories as well (`-d`): a directory whose
    /// files would all be removed goes as a whole (empty ones too), and
    /// the removable files of the others go one by one.
    pub fn directories(mut self, directories: bool) -> Self {
        self.directories = directories;
        self
    }

    /// Sets which ignored files are removed ([`CleanIgnored`]).
    pub fn ignored(mut self, ignored: CleanIgnored) -> Self {
        self.ignored = ignored;
        self
    }

    /// Only reports what would be removed (`-n`).
    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Limits the removal to pathspecs (`-- <paths>`): files, directories
    /// (everything under them) or globs, relative to the repository root.
    /// As in Git, a pathspec naming an untracked directory removes it (or
    /// its contents) even without [`CleanOptions::directories`].
    pub fn paths<P: AsRef<Path>>(mut self, paths: &[P]) -> Self {
        self.paths = paths.iter().map(|p| p.as_ref().to_path_buf()).collect();
        self
    }
}

/// What a walk found to remove.
enum Found {
    File(String),
    /// A directory whose content all goes (nested repositories aside).
    Dir(String),
}

/// What an untracked directory holds, nested repositories left out.
#[derive(Debug, Clone, Copy, Default)]
struct Summary {
    /// A file that stays.
    kept: bool,
    /// A file that goes.
    removable: bool,
    /// A nested repository somewhere inside.
    nested: bool,
}

struct Cleaner<'a> {
    work_dir: &'a Path,
    rules: &'a mut IgnoreRules,
    tracked: HashSet<String>,
    tracked_dirs: HashSet<String>,
    ignored: CleanIgnored,
    directories: bool,
    spec: Pathspec,
    summaries: HashMap<String, Summary>,
    found: Vec<Found>,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{}", dir, name)
    }
}

impl Cleaner<'_> {
    fn full(&self, rel: &str) -> PathBuf {
        self.work_dir.join(native_path(Path::new(rel)))
    }

    /// The entries of a directory: name and whether it is a directory
    /// (symbolic links are never followed, so they count as files).
    fn entries(&self, dir: &str) -> Result<Vec<(String, bool)>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.full(dir))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            entries.push((name, entry.file_type()?.is_dir()));
        }
        entries.sort();
        Ok(entries)
    }

    fn is_nested_repository(&self, rel: &str) -> bool {
        self.full(rel).join(".git").exists()
    }

    fn is_ignored(&mut self, rel: &str, is_dir: bool) -> Result<bool> {
        self.rules.is_ignored(&native_path(Path::new(rel)), is_dir)
    }

    fn removable(&mut self, rel: &str) -> Result<bool> {
        Ok(match self.ignored {
            CleanIgnored::Keep => !self.is_ignored(rel, false)?,
            CleanIgnored::Include => true,
            CleanIgnored::Only => self.is_ignored(rel, false)?,
        })
    }

    /// Walks a directory that holds tracked files (or the root).
    fn walk_tracked(&mut self, dir: &str) -> Result<()> {
        for (name, is_dir) in self.entries(dir)? {
            let rel = join(dir, &name);
            if self.tracked.contains(&rel) {
                // A tracked file, or a submodule.
                continue;
            }
            if !is_dir {
                self.file(rel)?;
            } else if self.tracked_dirs.contains(&rel) {
                self.walk_tracked(&rel)?;
            } else if !self.is_nested_repository(&rel) {
                self.untracked_dir(rel)?;
            }
        }
        Ok(())
    }

    /// Walks the contents of an untracked directory.
    fn walk_untracked(&mut self, dir: &str) -> Result<()> {
        for (name, is_dir) in self.entries(dir)? {
            let rel = join(dir, &name);
            if !is_dir {
                self.file(rel)?;
            } else if !self.is_nested_repository(&rel) {
                self.untracked_dir(rel)?;
            }
        }
        Ok(())
    }

    fn file(&mut self, rel: String) -> Result<()> {
        if self.removable(&rel)? && (self.spec.is_empty() || self.spec.matches(rel.as_bytes())) {
            self.found.push(Found::File(rel));
        }
        Ok(())
    }

    /// Decides about an untracked directory as Git does: removed as a
    /// whole, entered, or left alone.
    fn untracked_dir(&mut self, rel: String) -> Result<()> {
        let any_spec = !self.spec.is_empty();
        let matched = any_spec && self.spec.matches(rel.as_bytes());
        // Within reach of a pathspec: named, or holding what one may name.
        let reached = matched || (any_spec && self.spec.reaches_into(rel.as_bytes()));
        let summary = self.summary(&rel)?;
        match self.ignored {
            CleanIgnored::Only => {
                if any_spec && !reached {
                    return Ok(());
                }
                // Git does not look inside an ignored directory: once in
                // reach of a pathspec (or of -d), it goes as a whole.
                if self.is_ignored(&rel, true)? {
                    if any_spec || self.directories {
                        self.found.push(Found::Dir(rel));
                    }
                    return Ok(());
                }
                // Only ignored files.
                let whole = !summary.kept && summary.removable;
                if !whole {
                    return self.walk_untracked(&rel);
                }
                if matched || (!any_spec && self.directories) {
                    self.found.push(Found::Dir(rel));
                } else if any_spec {
                    self.walk_untracked(&rel)?;
                }
            }
            CleanIgnored::Keep | CleanIgnored::Include => {
                let enter = if any_spec { reached } else { self.directories };
                if !enter {
                    return Ok(());
                }
                if !summary.kept && (!any_spec || matched) {
                    self.found.push(Found::Dir(rel));
                } else {
                    self.walk_untracked(&rel)?;
                }
            }
        }
        Ok(())
    }

    /// What an untracked directory holds.
    fn summary(&mut self, rel: &str) -> Result<Summary> {
        if let Some(summary) = self.summaries.get(rel) {
            return Ok(*summary);
        }
        let mut summary = Summary::default();
        for (name, is_dir) in self.entries(rel)? {
            let child = join(rel, &name);
            if is_dir {
                if self.is_nested_repository(&child) {
                    summary.nested = true;
                    continue;
                }
                let inner = self.summary(&child)?;
                summary.kept |= inner.kept;
                summary.removable |= inner.removable;
                summary.nested |= inner.nested;
            } else if self.removable(&child)? {
                summary.removable = true;
            } else {
                summary.kept = true;
            }
        }
        self.summaries.insert(rel.to_owned(), summary);
        Ok(summary)
    }

    /// Removes a directory found whole (or, with `dry_run`, only reports
    /// it): all of it, or everything but its nested repositories, in which
    /// case its removed contents are reported one by one, as Git does.
    fn remove_dir(&mut self, rel: &str, dry_run: bool, removed: &mut Vec<String>) -> Result<()> {
        if !self.summary(rel)?.nested {
            if !dry_run {
                remove_tree(&self.full(rel))?;
            }
            removed.push(rel.to_owned());
            return Ok(());
        }
        for (name, is_dir) in self.entries(rel)? {
            let child = join(rel, &name);
            if !is_dir {
                if !dry_run {
                    remove_entry(&self.full(&child))?;
                }
                removed.push(child);
            } else if !self.is_nested_repository(&child) {
                self.remove_dir(&child, dry_run, removed)?;
            }
        }
        Ok(())
    }
}

/// Removes a file or a symbolic link (to a file or a directory).
fn remove_entry(path: &Path) -> Result<()> {
    match remove_file(path) {
        Ok(_) => Ok(()),
        // A symbolic link to a directory, on Windows.
        Err(e) => match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(_) => Err(e),
        },
    }
}

/// Removes a directory and everything in it, read-only files included,
/// without following symbolic links.
fn remove_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_tree(&entry.path())?;
        } else {
            remove_entry(&entry.path())?;
        }
    }
    fs::remove_dir(path)?;
    Ok(())
}

impl Repository {
    /// Removes untracked files from the work tree, like `git clean -f`, and
    /// returns what was removed (or, with [`CleanOptions::dry_run`], what
    /// would be), sorted, as `git clean -n` lists it.
    ///
    /// By default only untracked files that are not ignored go, and
    /// untracked directories are left alone. [`CleanOptions::directories`]
    /// removes them too (`-d`): a directory whose files would all be
    /// removed goes as a whole and is listed once; in one that holds files
    /// to keep, the others go one by one. [`CleanOptions::ignored`] adds
    /// ignored files (`-x`) or removes only them (`-X`), and
    /// [`CleanOptions::paths`] limits the removal to pathspecs.
    ///
    /// Tracked files are never removed, even if they match an ignore
    /// pattern, and neither are nested repositories (directories with their
    /// own `.git`) or anything in them; the directory around one keeps it
    /// and loses the rest. `.git` itself is never touched.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidPathOperation` for a bare repository, which has no
    ///   work tree; nothing is removed.
    /// - `Error::Io` if a file cannot be removed; what was removed before
    ///   stays removed, as with Git.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::{CleanIgnored, CleanOptions, Repository};
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// // Like `git clean -n -d -x`: what would go, build output included.
    /// let options = CleanOptions::new()
    ///     .directories(true)
    ///     .ignored(CleanIgnored::Include)
    ///     .dry_run(true);
    /// for path in repo.clean(&options).unwrap() {
    ///     println!("would remove {}", path.display());
    /// }
    /// ```
    pub fn clean(&self, options: &CleanOptions) -> Result<Vec<PathBuf>> {
        // A bare repository's "work tree" is the repository itself.
        if self.is_bare() {
            return Err(Error::InvalidPathOperation {
                path: self.path().to_path_buf(),
                reason: "a bare repository has no work tree to clean".to_owned(),
            });
        }
        let index = self.read_index()?;
        let mut tracked = HashSet::new();
        let mut tracked_dirs = HashSet::new();
        for entry in index.entries() {
            let key = String::from_utf8_lossy(&path_key(entry.path())).into_owned();
            let mut end = key.len();
            while let Some(pos) = key[..end].rfind('/') {
                if !tracked_dirs.insert(key[..pos].to_owned()) {
                    break;
                }
                end = pos;
            }
            tracked.insert(key);
        }
        let mut worktree = self.worktree()?;
        let mut cleaner = Cleaner {
            work_dir: self.path(),
            rules: worktree.rules(),
            tracked,
            tracked_dirs,
            ignored: options.ignored,
            directories: options.directories,
            spec: Pathspec::new(&options.paths),
            summaries: HashMap::new(),
            found: Vec::new(),
        };
        cleaner.walk_tracked("")?;

        let mut removed = Vec::new();
        for found in std::mem::take(&mut cleaner.found) {
            match found {
                Found::File(rel) => {
                    if !options.dry_run {
                        remove_entry(&cleaner.full(&rel))?;
                    }
                    removed.push(rel);
                }
                Found::Dir(rel) => cleaner.remove_dir(&rel, options.dry_run, &mut removed)?,
            }
        }
        removed.sort();
        Ok(removed
            .into_iter()
            .map(|rel| native_path(Path::new(&rel)))
            .collect())
    }
}
