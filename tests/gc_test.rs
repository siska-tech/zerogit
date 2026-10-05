//! Housekeeping compared with Git: `Repository::pack_refs` with
//! `git pack-refs --all`, `Repository::repack` with `git repack` (#63).

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

/// Deletes a loose object Git wrote. Git makes it read-only, which Rust
/// before 1.75 or so cannot delete on Windows.
fn remove_object(dir: &Path, oid: &str) {
    let path = dir.join(format!(".git/objects/{}/{}", &oid[..2], &oid[2..]));
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    fs::remove_file(&path).unwrap();
}

/// Loose object files (`objects/xx/...`) and pack files, by name.
fn object_files(dir: &Path) -> (Vec<String>, Vec<String>) {
    let objects = dir.join(".git/objects");
    let mut loose = Vec::new();
    for fanout in fs::read_dir(&objects).unwrap() {
        let fanout = fanout.unwrap();
        let name = fanout.file_name().to_string_lossy().into_owned();
        if name.len() == 2 && fanout.path().is_dir() {
            for file in fs::read_dir(fanout.path()).unwrap() {
                loose.push(format!(
                    "{}{}",
                    name,
                    file.unwrap().file_name().to_string_lossy()
                ));
            }
        }
    }
    let mut packs: Vec<String> = fs::read_dir(objects.join("pack"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    loose.sort();
    packs.sort();
    (loose, packs)
}

/// Every object Git considers reachable (references, HEAD, reflogs and
/// the index), sorted.
fn git_reachable(dir: &Path) -> Vec<String> {
    let mut oids: Vec<String> = git(
        dir,
        &[
            "rev-list",
            "--objects",
            "--all",
            "--reflog",
            "--indexed-objects",
        ],
    )
    .lines()
    .map(|line| line[..40].to_owned())
    .collect();
    oids.sort();
    oids.dedup();
    oids
}

/// The objects of the only pack, as `git verify-pack -v` lists them, and
/// how many are deltas.
fn verify_pack(dir: &Path, pack: &Path) -> (Vec<String>, usize) {
    // Git cannot open verbatim (`\\?\`) paths; name the pack relative to
    // the work tree.
    let relative = format!(
        ".git/objects/pack/{}",
        pack.file_name().unwrap().to_string_lossy()
    );
    let output = git(dir, &["verify-pack", "-v", &relative]);
    let mut oids = Vec::new();
    let mut deltas = 0;
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5 && fields[0].len() == 40 {
            oids.push(fields[0].to_owned());
            if fields.len() >= 7 {
                deltas += 1;
            }
        }
    }
    oids.sort();
    (oids, deltas)
}

/// A history Git packed with deltas (a file growing commit by commit),
/// then more commits, a stash, a staged file and a reflog-only commit
/// written loose.
fn packed_history() -> tempfile::TempDir {
    let temp = repository(&["base.txt"]);
    let dir = temp.path();
    git(dir, &["config", "gc.auto", "0"]);
    let mut content = String::new();
    for i in 0..30 {
        for j in 0..20 {
            content.push_str(&format!("line {} of commit {}\n", j, i));
        }
        write(dir, "big.txt", &content);
        git(dir, &["add", "big.txt"]);
        git(dir, &["commit", "-q", "-m", &format!("Commit {}", i)]);
    }
    git(dir, &["tag", "-a", "v1", "-m", "Version 1"]);
    git(dir, &["gc", "-q"]);
    // Loose objects after the pack.
    write(dir, "new.txt", "new\n");
    git(dir, &["add", "new.txt"]);
    git(dir, &["commit", "-q", "-m", "After gc"]);
    // Reachable only from the reflog.
    write(dir, "gone.txt", "gone\n");
    git(dir, &["add", "gone.txt"]);
    git(dir, &["commit", "-q", "-m", "Undone"]);
    git(dir, &["reset", "-q", "--hard", "HEAD~1"]);
    write(dir, "stashed.txt", "stashed\n");
    git(dir, &["add", "stashed.txt"]);
    git(dir, &["stash", "-q"]);
    // Only in the index.
    write(dir, "staged.txt", "staged\n");
    git(dir, &["add", "staged.txt"]);
    temp
}

#[test]
fn repack_packs_every_reachable_object_like_git() {
    let temp = packed_history();
    let dir = temp.path();
    let reachable = git_reachable(dir);
    let (loose_before, packs_before) = object_files(dir);
    assert!(!loose_before.is_empty());
    // Git's pack (with its `.rev` reverse index).
    assert_eq!(
        packs_before.iter().filter(|p| p.ends_with(".pack")).count(),
        1
    );
    assert!(packs_before.iter().any(|p| p.ends_with(".rev")));

    let repo = Repository::open(dir).unwrap();
    // A repository opened before keeps reading across the repack.
    let head = repo.head().unwrap().oid().to_hex();
    let summary = repo.repack().unwrap();
    let pack = summary.pack().unwrap().to_path_buf();
    assert_eq!(summary.objects(), reachable.len());
    assert_eq!(summary.removed_packs(), 1);
    assert_eq!(summary.loosened(), 0);
    assert!(summary.reused_deltas() > 0);

    let (loose, packs) = object_files(dir);
    assert_eq!(loose, Vec::<String>::new());
    let name = pack.file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(packs, [format!("{}.idx", name), format!("{}.pack", name)]);
    let (packed, deltas) = verify_pack(dir, &pack);
    assert_eq!(packed, reachable);
    assert_eq!(deltas, summary.reused_deltas());
    assert_fsck(dir);
    assert_eq!(git(dir, &["count-objects"]), "0 objects, 0 kilobytes\n");
    assert_eq!(repo.commit(&head).unwrap().summary(), "After gc");
    assert_eq!(repo.log().unwrap().count(), 32);

    // Deltas keep the pack small: about the size of Git's own pack plus
    // the loose objects, not of every object stored whole.
    let size = fs::metadata(&pack).unwrap().len();
    let whole: usize = git(
        dir,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectsize)",
        ],
    )
    .lines()
    .map(|l| l.parse::<usize>().unwrap())
    .sum();
    assert!((size as usize) < whole / 3, "{} vs {}", size, whole);

    // Repacking again finds everything in place.
    let again = repo.repack().unwrap();
    assert_eq!(again.objects(), reachable.len());
    let packs = object_files(dir).1;
    assert_eq!(packs.iter().filter(|p| p.ends_with(".pack")).count(), 1);
    assert_fsck(dir);
}

/// Runs `git pack-objects` on the objects listed in `input` and returns
/// the new pack's path.
fn pack_objects(dir: &Path, input: &str) -> std::path::PathBuf {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["pack-objects", "-q", ".git/objects/pack/pack"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let name = String::from_utf8(output.stdout).unwrap();
    dir.join(format!(".git/objects/pack/pack-{}.pack", name.trim()))
}

#[test]
fn unreachable_objects_and_kept_packs_survive() {
    let temp = packed_history();
    let dir = temp.path();
    // An unreachable blob in a pack, another loose.
    write(dir, "x.txt", "packed but unreferenced\n");
    let packed_blob = git(dir, &["hash-object", "-w", "x.txt"]).trim().to_owned();
    pack_objects(dir, &format!("{}\n", packed_blob));
    let loose_blob_path = {
        write(dir, "x.txt", "loose and unreferenced\n");
        let oid = git(dir, &["hash-object", "-w", "x.txt"]).trim().to_owned();
        format!(".git/objects/{}/{}", &oid[..2], &oid[2..])
    };
    fs::remove_file(dir.join("x.txt")).unwrap();
    remove_object(dir, &packed_blob);
    // A kept pack with the last commit's new objects.
    let kept = pack_objects(dir, &git(dir, &["rev-list", "--objects", "HEAD~1..HEAD"]));
    fs::write(kept.with_extension("keep"), "").unwrap();
    let kept_files: Vec<_> = ["pack", "idx", "keep"]
        .iter()
        .map(|e| fs::read(kept.with_extension(e)).unwrap())
        .collect();

    let summary = Repository::open(dir).unwrap().repack().unwrap();
    assert_eq!(summary.loosened(), 1);
    // The kept pack and the new one remain; the other packs were replaced.
    let (loose, packs) = object_files(dir);
    let pack_count = packs.iter().filter(|p| p.ends_with(".pack")).count();
    assert_eq!(pack_count, 2, "{:?}", packs);
    let kept_after: Vec<_> = ["pack", "idx", "keep"]
        .iter()
        .map(|e| fs::read(kept.with_extension(e)).unwrap())
        .collect();
    assert_eq!(kept_after, kept_files);
    // The new pack does not repeat what the kept pack has.
    let (new_objects, _) = verify_pack(dir, summary.pack().unwrap());
    let (kept_objects, _) = verify_pack(dir, &kept);
    assert!(new_objects.iter().all(|o| !kept_objects.contains(o)));
    assert_eq!(
        new_objects.len() + kept_objects.len(),
        git_reachable(dir).len()
    );
    // Unreachable objects are loose: the old loose one untouched, the
    // packed one written out.
    assert!(dir.join(&loose_blob_path).exists());
    assert!(loose.contains(&packed_blob));
    assert_eq!(loose.len(), 2, "{:?}", loose);
    git(dir, &["cat-file", "-e", &packed_blob]);
    assert_fsck(dir);
}

#[test]
fn repack_of_a_corrupt_repository_removes_nothing() {
    let temp = packed_history();
    let dir = temp.path();
    // The blob of the commit after gc is lost.
    let blob = git(dir, &["rev-parse", "HEAD:new.txt"]).trim().to_owned();
    remove_object(dir, &blob);
    let before = object_files(dir);
    assert!(matches!(
        Repository::open(dir).unwrap().repack(),
        Err(Error::ObjectNotFound(_))
    ));
    assert_eq!(object_files(dir), before);

    // An empty repository has nothing to pack.
    let empty = empty_repository();
    let summary = Repository::open(empty.path()).unwrap().repack().unwrap();
    assert_eq!(summary.pack(), None);
    assert_eq!(summary.objects(), 0);
}
