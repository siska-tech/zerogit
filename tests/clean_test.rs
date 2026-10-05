//! `Repository::clean` compared with `git clean` (#64).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{CleanIgnored, CleanOptions, Error, Repository};

/// Tracked files (one matching an ignore pattern), untracked files in
/// tracked and untracked directories, ignored files and directories,
/// empty directories, directories holding only ignored files, a mix of
/// both, and nested repositories (directly, and inside an untracked
/// directory).
fn messy() -> tempfile::TempDir {
    let temp = repository(&["a.txt", "src/main.rs", "src/sub/x.rs"]);
    let dir = temp.path();
    write(dir, ".gitignore", "*.log\nbuild/\ntarget/\n");
    write(dir, "src/tracked.log", "tracked though ignored\n");
    git(dir, &["add", "-f", ".gitignore", "src/tracked.log"]);
    git(dir, &["commit", "-q", "-m", "Ignore rules"]);
    for (path, content) in [
        ("u.txt", "u"),
        ("src/u.rs", "u"),
        ("src/sub/u2.rs", "u"),
        ("newdir/n.txt", "n"),
        ("newdir/deep/d.txt", "d"),
        ("ign.log", "l"),
        ("build/out.o", "b"),
        ("src/b.log", "l"),
        ("src/target/deep/t.o", "t"),
        ("mixed/m.log", "m"),
        ("d3/n.txt", "n"),
        ("d3/x.log", "l"),
        ("d3/sub/y.log", "l"),
        ("onlyign/x/a.log", "l"),
        ("nr/z.txt", "z"),
        ("nr/s/f", "f"),
        ("nested/f", "f"),
    ] {
        write(dir, path, content);
    }
    for empty in ["emptydir", "dirE/sub/subsub", "src/emptysub"] {
        fs::create_dir_all(dir.join(empty)).unwrap();
    }
    git(&dir.join("nested"), &["init", "-q"]);
    fs::create_dir_all(dir.join("nr/inner")).unwrap();
    git(&dir.join("nr/inner"), &["init", "-q"]);
    write(dir, "nr/inner/kept.txt", "inside a nested repository");
    temp
}

/// The paths `git clean -n` would remove, without trailing slashes.
fn git_would_remove(dir: &Path, args: &[&str]) -> Vec<String> {
    let mut all = vec!["clean", "-n"];
    all.extend_from_slice(args);
    git(dir, &all)
        .lines()
        .filter_map(|line| line.strip_prefix("Would remove "))
        .map(|path| path.trim_end_matches('/').to_owned())
        .collect()
}

fn options(args: &[&str]) -> CleanOptions {
    let mut options = CleanOptions::new();
    let mut paths = Vec::new();
    let mut after_dashes = false;
    for arg in args {
        match *arg {
            _ if after_dashes => paths.push(*arg),
            "--" => after_dashes = true,
            "-d" => options = options.directories(true),
            "-x" => options = options.ignored(CleanIgnored::Include),
            "-X" => options = options.ignored(CleanIgnored::Only),
            other => panic!("unknown option {}", other),
        }
    }
    options.paths(&paths)
}

fn names(paths: Vec<std::path::PathBuf>) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect()
}

/// Every directory and file of the work tree (`.git` and nested
/// repositories' contents included, to see they are untouched).
fn tree(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel == ".git" {
                continue;
            }
            if path.is_dir() {
                out.push(rel + "/");
                walk(root, &path, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

const CASES: &[&[&str]] = &[
    &[],
    &["-d"],
    &["-x"],
    &["-X"],
    &["-d", "-x"],
    &["-d", "-X"],
    &["--", "newdir"],
    &["--", "src"],
    &["--", "*.txt"],
    &["-d", "--", "*.txt"],
    &["--", "newdir/deep"],
    &["-d", "--", "newdir/deep"],
    &["--", "nr"],
    &["-d", "--", "nr"],
    &["--", "d3"],
    &["-X", "--", "d3"],
    &["--", "mixed"],
    &["-X", "--", "mixed"],
    &["-d", "-X", "--", "mixed"],
    &["-d", "--", "emptydir"],
    &["--", "dirE"],
    &["-x", "--", "build"],
    &["-d", "--", "src", "u.txt"],
    &["-d", "-x", "--", "*.log"],
    &["-X", "--", "*.log"],
    &["-x", "--", "*.log"],
    &["-d", "-X", "--", "d3"],
    &["-x", "--", "d3/sub"],
    &["--", "nested"],
    &["-d", "-x", "--", "nested"],
    &["-d", "--", "nr/inner"],
    &["--", "no/such/path"],
    &["-X", "--", "build/out.o"],
    &["-X", "--", "b*"],
    &["-X", "--", "src/*"],
    &["-X", "-d", "--", "*.o"],
    &["-x", "--", "build/out.o"],
];

#[test]
fn dry_run_lists_what_git_clean_would_remove() {
    let temp = messy();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let before = tree(dir);
    for args in CASES {
        let ours = names(repo.clean(&options(args).dry_run(true)).unwrap());
        assert_eq!(ours, git_would_remove(dir, args), "git clean -n {:?}", args);
    }
    assert_eq!(tree(dir), before, "a dry run changes nothing");
}

#[test]
fn removal_leaves_what_git_clean_leaves() {
    let temp = messy();
    for args in CASES {
        let ours = twin(temp.path());
        let theirs = twin(temp.path());
        let removed = Repository::open(ours.path())
            .unwrap()
            .clean(&options(args))
            .unwrap();
        let mut git_args = vec!["clean", "-q", "-f"];
        git_args.extend_from_slice(args);
        git(theirs.path(), &git_args);
        assert_eq!(
            tree(ours.path()),
            tree(theirs.path()),
            "git clean -f {:?}",
            args
        );
        for path in removed {
            assert!(!ours.path().join(path).exists());
        }
        // Tracked files and nested repositories are always there.
        for kept in ["a.txt", "src/tracked.log", "nested/f", "nr/inner/kept.txt"] {
            assert!(ours.path().join(kept).exists(), "{} after {:?}", kept, args);
        }
        assert_eq!(
            git(
                ours.path(),
                &["status", "--porcelain", "--untracked-files=no"]
            ),
            ""
        );
    }
}

#[test]
fn read_only_files_and_bare_repositories() {
    let temp = messy();
    let dir = temp.path();
    // Read-only files go as Git removes them.
    let path = dir.join("newdir/deep/d.txt");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    Repository::open(dir)
        .unwrap()
        .clean(&CleanOptions::new().directories(true))
        .unwrap();
    assert!(!dir.join("newdir").exists());

    // A bare repository has no work tree; nothing is removed.
    let bare = tempfile::TempDir::new().unwrap();
    git(bare.path(), &["init", "-q", "--bare"]);
    let before = tree(bare.path());
    let repo = Repository::open(bare.path()).unwrap();
    assert!(matches!(
        repo.clean(
            &CleanOptions::new()
                .directories(true)
                .ignored(CleanIgnored::Include)
        ),
        Err(Error::InvalidPathOperation { .. })
    ));
    assert_eq!(tree(bare.path()), before);
}
