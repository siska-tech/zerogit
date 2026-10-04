//! Working tree access: traversal, reading files as blobs and writing blobs
//! back.
//!
//! The walk skips only `.git`; files and directories whose names start with
//! `.` are treated like any other. Ignore rules are applied the way Git
//! applies them: an untracked path that is excluded is left out, an excluded
//! directory is not entered (so nothing inside it can be re-included), and a
//! tracked file is always kept, even when it matches an ignore pattern.
//!
//! Symbolic links are recorded as Git does: mode `120000`, with the link
//! target (using `/` separators) as the blob content. They are never
//! followed, so a link to a directory is one entry and a dangling link can
//! still be added. With `core.symlinks=false`, or where a link cannot be
//! created, checkout writes a plain file containing the target instead, and
//! such a file keeps its symlink mode in the index.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::attributes::{AttrValue, Attributes};
use crate::config::Config;
use crate::eol::{CrlfAction, EolSettings};
use crate::error::{Error, Result};
use crate::ignore::IgnoreRules;
use crate::index::{Index, IndexEntry};
use crate::infra::{hash_object, write_file_atomic};
use crate::objects::{tree::FileMode, ObjectStore, Oid};

/// The result of walking the working tree.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// Tracked files that exist, and untracked files that are not ignored.
    pub(crate) files: BTreeSet<PathBuf>,
    /// Untracked files that are ignored (only collected on request).
    pub(crate) ignored: BTreeSet<PathBuf>,
}

/// A file of the working tree, read the way Git stores it.
#[derive(Debug)]
pub(crate) struct WorkFile {
    /// The blob content: the file content, or the link target of a symlink.
    pub(crate) content: Vec<u8>,
    /// The mode Git records for the file.
    pub(crate) mode: FileMode,
    /// The metadata of the file itself (not of a symlink's target).
    pub(crate) metadata: fs::Metadata,
}

impl WorkFile {
    /// The blob OID of the content.
    pub(crate) fn oid(&self) -> Oid {
        Oid::from_bytes(hash_object("blob", &self.content))
    }

    /// Builds a stage 0 index entry for this file, with its stat data.
    pub(crate) fn index_entry(&self, path: PathBuf, oid: Oid) -> IndexEntry {
        let (mtime, mtime_nsec) = timestamp(self.metadata.modified().ok());
        let (ctime, ctime_nsec) = match self.metadata.created().ok() {
            Some(created) => timestamp(Some(created)),
            None => (mtime, mtime_nsec),
        };
        IndexEntry::new(
            ctime,
            mtime,
            0, // dev (not portable, use 0)
            0, // ino (not portable, use 0)
            self.mode,
            0, // uid (not portable, use 0)
            0, // gid (not portable, use 0)
            // The size of the file on disk, as Git records it.
            self.metadata.len() as u32,
            oid,
            path,
            0,
        )
        .with_nanos(ctime_nsec, mtime_nsec)
    }
}

fn timestamp(time: Option<std::time::SystemTime>) -> (u64, u32) {
    time.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0))
}

/// The working tree of a repository and the settings that control how its
/// files map to blobs.
#[derive(Debug)]
pub(crate) struct Worktree {
    root: PathBuf,
    rules: IgnoreRules,
    attributes: Attributes,
    eol: EolSettings,
    /// Filter drivers configured with `filter.<name>.clean`, `smudge` or
    /// `process`, by name. Others are ignored, as Git ignores them.
    filters: HashSet<String>,
    /// Reads index blobs for the CRLF check of automatic conversion.
    store: ObjectStore,
    /// `core.symlinks`: create symbolic links on checkout.
    symlinks: bool,
    /// `core.fileMode`: trust the executable bit of files.
    filemode: bool,
}

/// Attributes whose conversions are not implemented.
const UNSUPPORTED: &[&str] = &["filter", "ident", "working-tree-encoding"];

impl Worktree {
    /// Loads the ignore rules, attributes and checkout settings of a
    /// working tree.
    pub(crate) fn load(
        work_dir: &Path,
        git_dir: &Path,
        config: &Config,
        store: ObjectStore,
    ) -> Result<Self> {
        let filters = config
            .subsections("filter")
            .into_iter()
            .filter(|name| {
                ["clean", "smudge", "process"]
                    .iter()
                    .any(|key| config.get_subsection("filter", name, key).is_some())
            })
            .map(str::to_owned)
            .collect();
        Ok(Worktree {
            root: work_dir.to_path_buf(),
            rules: IgnoreRules::load(work_dir, git_dir, config)?,
            attributes: Attributes::load(work_dir, git_dir, config)?,
            eol: EolSettings::from_config(config),
            filters,
            store,
            symlinks: config.get_bool_or("core", "symlinks", true),
            // Without an executable bit, the index keeps the recorded mode.
            filemode: cfg!(unix) && config.get_bool_or("core", "filemode", true),
        })
    }

    /// The ignore rules of this working tree.
    pub(crate) fn rules(&mut self) -> &mut IgnoreRules {
        &mut self.rules
    }

    /// The attributes of this working tree.
    pub(crate) fn attributes(&mut self) -> &mut Attributes {
        &mut self.attributes
    }

    /// Walks the working tree, sorting files into kept and ignored.
    ///
    /// `index` supplies the tracked paths. Ignored files are collected only
    /// when `collect_ignored` is set, because that means entering every
    /// excluded directory (`target/`, `node_modules/`, ...).
    pub(crate) fn scan(&mut self, index: Option<&Index>, collect_ignored: bool) -> Result<Scan> {
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
            work_dir: &self.root,
            rules: &mut self.rules,
            tracked: &tracked,
            tracked_dirs: &tracked_dirs,
            collect_ignored,
            scan: Scan::default(),
        };
        walker.walk(Path::new(""), false)?;
        Ok(walker.scan)
    }

    /// Returns the metadata of a path without following a final symlink, or
    /// `None` if nothing (not even a dangling link) is there.
    pub(crate) fn metadata(&self, path: &Path) -> Result<Option<fs::Metadata>> {
        match fs::symlink_metadata(self.root.join(path)) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            // A parent that is now a file reads as "not a directory".
            Err(_) if !self.root.join(path).parent().map_or(true, Path::is_dir) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The line ending conversion for a path, or an error if the path has an
    /// attribute whose conversion is not implemented.
    fn conversion(&mut self, path: &Path) -> Result<CrlfAction> {
        let mut names: Vec<&str> = crate::eol::ATTRIBUTES.to_vec();
        names.extend_from_slice(UNSUPPORTED);
        let attrs = self.attributes.lookup(path, &names)?;
        for name in UNSUPPORTED {
            let unsupported = match (attrs.get(*name), *name) {
                // A filter without a configured driver is ignored, as in Git.
                (Some(AttrValue::Value(driver)), "filter") => self.filters.contains(driver),
                (Some(AttrValue::Set), "ident") => true,
                (Some(AttrValue::Value(_)), "working-tree-encoding") => true,
                _ => false,
            };
            if unsupported {
                return Err(Error::UnsupportedAttribute {
                    path: path.to_path_buf(),
                    attribute: (*name).to_owned(),
                });
            }
        }
        Ok(self.eol.action(&attrs))
    }

    /// Checks that a path can be converted in both directions.
    pub(crate) fn check_supported(&mut self, path: &Path) -> Result<()> {
        self.conversion(path).map(|_| ())
    }

    /// Reads a file of the working tree as Git would store it: line endings
    /// converted per `core.autocrlf`, `core.eol` and `.gitattributes`.
    ///
    /// `tracked` is the path's index entry. Its mode is used where the file
    /// system cannot tell (the executable bit with `core.fileMode=false`, a
    /// symlink checked out as a plain file), and its blob decides whether
    /// automatic conversion leaves CRLF alone. Returns `None` if the path does
    /// not exist or is a directory.
    ///
    /// # Errors
    ///
    /// `Error::UnsupportedAttribute` if the path has a `filter` (with a
    /// configured driver), `ident` or `working-tree-encoding` attribute.
    pub(crate) fn read(
        &mut self,
        path: &Path,
        tracked: Option<&IndexEntry>,
    ) -> Result<Option<WorkFile>> {
        let Some(file) = self.read_raw(path, tracked.map(IndexEntry::mode))? else {
            return Ok(None);
        };
        // A link target (or a link checked out as a file) is not text to convert.
        if file.mode == FileMode::Symlink {
            return Ok(Some(file));
        }
        let action = self.conversion(path)?;
        let store = &self.store;
        let cleaned = crate::eol::to_git(&self.eol, action, &file.content, || {
            tracked
                .filter(|e| e.mode() != FileMode::Symlink)
                .and_then(|e| store.read(e.oid()).ok())
                .is_some_and(|raw| crate::eol::has_crlf_text(&raw.content))
        });
        if cleaned.irreversible && self.eol.safecrlf {
            return Err(Error::IrreversibleLineEndings(path.to_path_buf()));
        }
        let content = cleaned.content.into_owned();
        Ok(Some(WorkFile { content, ..file }))
    }

    /// The mode Git would record for the file at `path`, from its metadata
    /// only, or `None` if it does not exist or is a directory.
    pub(crate) fn mode(
        &self,
        path: &Path,
        tracked_mode: Option<FileMode>,
    ) -> Result<Option<FileMode>> {
        let Some(metadata) = self.metadata(path)? else {
            return Ok(None);
        };
        let file_type = metadata.file_type();
        Ok(if file_type.is_symlink() {
            Some(FileMode::Symlink)
        } else if file_type.is_file() {
            Some(self.file_mode(&metadata, tracked_mode))
        } else {
            None
        })
    }

    fn file_mode(&self, metadata: &fs::Metadata, tracked_mode: Option<FileMode>) -> FileMode {
        match tracked_mode {
            // A link checked out as a file keeps its mode.
            Some(FileMode::Symlink) if !self.symlinks => FileMode::Symlink,
            _ if self.filemode => executable_mode(metadata),
            Some(FileMode::Executable) => FileMode::Executable,
            _ => FileMode::Regular,
        }
    }

    /// Reads a file without converting its content.
    fn read_raw(&self, path: &Path, tracked_mode: Option<FileMode>) -> Result<Option<WorkFile>> {
        let Some(metadata) = self.metadata(path)? else {
            return Ok(None);
        };
        let full = self.root.join(path);
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(&full)?;
            return Ok(Some(WorkFile {
                content: link_target_bytes(&target),
                mode: FileMode::Symlink,
                metadata,
            }));
        }
        if !file_type.is_file() {
            return Ok(None);
        }
        let content = fs::read(&full)?;
        let mode = self.file_mode(&metadata, tracked_mode);
        Ok(Some(WorkFile {
            content,
            mode,
            metadata,
        }))
    }

    /// Returns the blob OID and mode of a work tree file, or `None` if it
    /// does not exist.
    ///
    /// A file whose conversion is not supported (see [`Worktree::read`]) is
    /// compared by its stat data: if size and modification time still match
    /// the index entry it is taken as unchanged, otherwise its unconverted
    /// content is hashed (and so differs from the index).
    pub(crate) fn hash(
        &mut self,
        path: &Path,
        tracked: Option<&IndexEntry>,
    ) -> Result<Option<(Oid, FileMode)>> {
        match self.read(path, tracked) {
            Ok(file) => Ok(file.map(|file| (file.oid(), file.mode))),
            Err(Error::UnsupportedAttribute { .. }) | Err(Error::IrreversibleLineEndings(_)) => {
                let Some(file) = self.read_raw(path, tracked.map(IndexEntry::mode))? else {
                    return Ok(None);
                };
                if let Some(entry) = tracked {
                    if stat_matches(entry, &file.metadata) {
                        return Ok(Some((*entry.oid(), entry.mode())));
                    }
                }
                Ok(Some((file.oid(), file.mode)))
            }
            Err(e) => Err(e),
        }
    }

    /// Builds a stage 0 index entry with the stat data of the file at `path`
    /// (zeros if it is missing), for a blob just checked out.
    pub(crate) fn stat_entry(
        &self,
        path: &Path,
        index_path: PathBuf,
        oid: Oid,
        mode: FileMode,
    ) -> Result<IndexEntry> {
        Ok(match self.metadata(path)? {
            Some(metadata) => WorkFile {
                content: Vec::new(),
                mode,
                metadata,
            }
            .index_entry(index_path, oid),
            None => IndexEntry::new(0, 0, 0, 0, mode, 0, 0, 0, oid, index_path, 0),
        })
    }

    /// Writes a blob to the working tree, replacing whatever is at `path`.
    ///
    /// Line endings are converted for the working tree. A symlink is created
    /// as a link when `core.symlinks` allows and the platform supports it,
    /// and otherwise written as a file containing the target, as Git does.
    pub(crate) fn write(&mut self, path: &Path, content: &[u8], mode: FileMode) -> Result<()> {
        let full = self.root.join(path);
        let converted = if mode == FileMode::Symlink {
            std::borrow::Cow::Borrowed(content)
        } else {
            let action = self.conversion(path)?;
            crate::eol::to_worktree(&self.eol, action, content)
        };
        if let Some(parent) = full.parent() {
            self.make_dirs(parent)?;
        }
        if let Some(existing) = self.metadata(path)? {
            if existing.is_dir() {
                fs::remove_dir_all(&full)?;
            } else if existing.file_type().is_symlink() || mode == FileMode::Symlink {
                fs::remove_file(&full)?;
            }
        }
        if mode == FileMode::Symlink && self.symlinks && create_symlink(content, &full).is_ok() {
            return Ok(());
        }
        write_file_atomic(&full, &converted)?;
        set_executable(&full, mode == FileMode::Executable)?;
        Ok(())
    }

    /// Creates the directories leading to a file, replacing a file or link
    /// that is in the way.
    fn make_dirs(&self, dir: &Path) -> Result<()> {
        if dir.is_dir() && !fs::symlink_metadata(dir)?.file_type().is_symlink() {
            return Ok(());
        }
        if let Some(parent) = dir.parent() {
            if dir != self.root {
                self.make_dirs(parent)?;
            }
        }
        match fs::symlink_metadata(dir) {
            Ok(metadata) if !metadata.is_dir() => fs::remove_file(dir)?,
            _ => {}
        }
        if !dir.is_dir() {
            fs::create_dir(dir)?;
        }
        Ok(())
    }

    /// Removes a file or link from the working tree, then any parent
    /// directories that became empty.
    pub(crate) fn remove(&self, path: &Path) -> Result<()> {
        let full = self.root.join(path);
        if let Some(metadata) = self.metadata(path)? {
            if !metadata.is_dir() {
                fs::remove_file(&full)?;
            }
        }
        let mut parent = full.parent();
        while let Some(dir) = parent {
            if dir == self.root {
                break;
            }
            let empty = match dir.read_dir() {
                Ok(mut entries) => entries.next().is_none(),
                Err(_) => false,
            };
            if !empty {
                break;
            }
            fs::remove_dir(dir)?;
            parent = dir.parent();
        }
        Ok(())
    }
}

/// Whether a file's size and modification time match its index entry.
fn stat_matches(entry: &IndexEntry, metadata: &fs::Metadata) -> bool {
    let (mtime, mtime_nsec) = timestamp(metadata.modified().ok());
    entry.size() == metadata.len() as u32
        && entry.mtime() == mtime
        && entry.mtime_nsec() == mtime_nsec
}

/// The mode of a regular file from its executable bit.
#[allow(unused_variables)]
fn executable_mode(metadata: &fs::Metadata) -> FileMode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return FileMode::Executable;
        }
    }
    FileMode::Regular
}

#[allow(unused_variables)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        let mode = permissions.mode();
        let new_mode = if executable {
            // Grant execute where read is granted, as Git does (0644 -> 0755).
            mode | ((mode & 0o444) >> 2)
        } else {
            mode & !0o111
        };
        if new_mode != mode {
            permissions.set_mode(new_mode);
            fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

/// The blob content of a link target: its bytes with `/` separators.
fn link_target_bytes(target: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        target.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        target.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

#[cfg(unix)]
fn create_symlink(target: &[u8], link: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(target), link)
}

#[cfg(windows)]
fn create_symlink(target: &[u8], link: &Path) -> std::io::Result<()> {
    let target = String::from_utf8_lossy(target).replace('/', "\\");
    let is_dir = link
        .parent()
        .map(|parent| parent.join(&target).is_dir())
        .unwrap_or(false);
    if is_dir {
        std::os::windows::fs::symlink_dir(&target, link)
    } else {
        std::os::windows::fs::symlink_file(&target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &[u8], _link: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
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
            // The type of the entry itself: a symlink is never followed, so a
            // link to a directory is a single file and cannot loop.
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let child_excluded = excluded || self.rules.is_excluded(&path, true)?;
                if !child_excluded || self.collect_ignored || self.tracked_dirs.contains(&path) {
                    self.walk(&path, child_excluded)?;
                }
            } else if file_type.is_file() || file_type.is_symlink() {
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
            // Sockets, FIFOs and other special files are skipped.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut worktree = Worktree::load(
            temp.path(),
            &temp.path().join(".git"),
            &Config::new(),
            ObjectStore::new(temp.path().join(".git/objects")),
        )
        .unwrap();
        worktree.scan(index, collect).unwrap()
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
