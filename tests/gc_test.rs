//! Housekeeping compared with Git: `Repository::pack_refs` and
//! `git pack-refs --all` (#63).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Error, Repository};

/// A repository with branches (some nested), lightweight, annotated and
/// nested annotated tags, a remote-tracking branch with a symbolic HEAD, a
/// stash, a bisect reference and a reference to a missing object; some
/// references are packed already, one of them with a newer loose value.
fn many_refs() -> tempfile::TempDir {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["branch", "feat/x/y"]);
    git(dir, &["branch", "old"]);
    git(dir, &["tag", "v1"]);
    git(dir, &["tag", "-a", "v2", "-m", "v2"]);
    git(dir, &["tag", "-a", "release/v3", "-m", "v3", "v2"]);
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    // Packed now; `old` then moves on as a loose reference.
    git(dir, &["pack-refs", "--all"]);
    write(dir, "a.txt", "second\n");
    git(dir, &["commit", "-q", "-am", "Second"]);
    git(dir, &["branch", "-f", "old"]);
    git(dir, &["branch", "new"]);
    write(dir, "a.txt", "stashed\n");
    git(dir, &["stash", "-q"]);
    git(dir, &["update-ref", "refs/bisect/bad", "HEAD"]);
    write(
        dir,
        ".git/refs/heads/broken",
        "1111111111111111111111111111111111111111\n",
    );
    temp
}

/// Every file under `.git/refs/`, `.git/logs/` and `packed-refs`, with
/// its content.
fn ref_files(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.push((rel(root, &path) + "/", String::new()));
                walk(root, &path, out);
            } else {
                out.push((rel(root, &path), fs::read_to_string(&path).unwrap()));
            }
        }
    }
    fn rel(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }
    let git_dir = dir.join(".git");
    let mut out = Vec::new();
    walk(&git_dir, &git_dir.join("refs"), &mut out);
    walk(&git_dir, &git_dir.join("logs"), &mut out);
    if let Ok(content) = fs::read_to_string(git_dir.join("packed-refs")) {
        out.push(("packed-refs".to_owned(), content));
    }
    out.sort();
    out
}

#[test]
fn pack_refs_matches_git() {
    let temp = many_refs();
    let ours = twin(temp.path());
    let theirs = twin(temp.path());
    let (o, t) = (ours.path(), theirs.path());
    // Git reports the broken reference but succeeds.
    assert!(git_output(t, &["pack-refs", "--all"]).status.success());
    Repository::open(o).unwrap().pack_refs().unwrap();
    assert_eq!(ref_files(o), ref_files(t));
    assert!(fs::read_to_string(o.join(".git/packed-refs"))
        .unwrap()
        .starts_with("# pack-refs with: peeled fully-peeled sorted \n"));
    assert!(o.join(".git/refs/heads/broken").exists());
    assert!(!o.join(".git/refs/heads/feat").exists());
    assert_eq!(
        git(o, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        git(
            temp.path(),
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        )
    );

    // Packing again changes nothing.
    let before = ref_files(o);
    Repository::open(o).unwrap().pack_refs().unwrap();
    assert_eq!(ref_files(o), before);
}

#[test]
fn packed_references_keep_working() {
    let temp = many_refs();
    let dir = temp.path();
    fs::remove_file(dir.join(".git/refs/heads/broken")).unwrap();
    let repo = Repository::open(dir).unwrap();
    repo.pack_refs().unwrap();
    let theirs = twin(dir);
    let t = theirs.path();

    // Reading.
    let mut branches: Vec<String> = repo
        .branches()
        .unwrap()
        .iter()
        .map(|b| b.name().to_owned())
        .collect();
    branches.sort();
    assert_eq!(branches, ["feat/x/y", "main", "new", "old"]);
    assert_eq!(repo.stash_list().unwrap().len(), 1);
    assert_eq!(
        repo.rev_parse("release/v3^{}").unwrap().to_hex(),
        git(dir, &["rev-parse", "release/v3^{}"]).trim()
    );

    // Deleting packed branches and tags, and dropping the last stash.
    repo.delete_branch("feat/x/y").unwrap();
    repo.delete_tag("release/v3").unwrap();
    repo.stash_drop(0).unwrap();
    git(t, &["branch", "-q", "-D", "feat/x/y"]);
    git(t, &["tag", "-d", "release/v3"]);
    git(t, &["stash", "drop", "-q"]);
    assert_eq!(ref_files(dir), ref_files(t));

    // Moving a packed branch writes a loose one over it.
    write(dir, "a.txt", "third\n");
    git(dir, &["add", "a.txt"]);
    let commit = repo.create_commit("Third", "T", "t@example.com").unwrap();
    assert_eq!(
        git(dir, &["rev-parse", "main"]).trim(),
        commit.to_hex().as_str()
    );
    assert!(matches!(
        repo.create_branch("new", None),
        Err(Error::RefAlreadyExists(_))
    ));
    assert_fsck(dir);
}

#[test]
fn locked_files_are_respected() {
    let temp = many_refs();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    // packed-refs locked: nothing changes.
    let before = ref_files(dir);
    fs::write(dir.join(".git/packed-refs.lock"), "").unwrap();
    assert!(matches!(repo.pack_refs(), Err(Error::Locked(_))));
    assert!(matches!(repo.delete_tag("v1"), Err(Error::Locked(_))));
    fs::remove_file(dir.join(".git/packed-refs.lock")).unwrap();
    assert_eq!(ref_files(dir), before);

    // A locked loose reference stays loose and keeps its value.
    fs::write(dir.join(".git/refs/heads/new.lock"), "").unwrap();
    repo.pack_refs().unwrap();
    fs::remove_file(dir.join(".git/refs/heads/new.lock")).unwrap();
    assert!(dir.join(".git/refs/heads/new").exists());
    assert!(!dir.join(".git/refs/heads/old").exists());
    assert_eq!(
        git(dir, &["rev-parse", "new"]),
        git(dir, &["rev-parse", "HEAD"])
    );
}
