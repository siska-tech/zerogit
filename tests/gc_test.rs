//! Housekeeping compared with Git: `Repository::pack_refs`, `repack`,
//! `prune` and `gc` with `git pack-refs --all`, `git repack`, `git prune`
//! and `git gc` (#63).

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
    assert_eq!(deltas, summary.reused_deltas() + summary.new_deltas());
    // The trees written loose after the pack are encoded against the packed ones.
    assert!(summary.new_deltas() > 0);
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

/// The longest delta chain in a pack, from `git verify-pack -v`.
fn longest_chain(dir: &Path, pack: &Path) -> usize {
    let relative = format!(
        ".git/objects/pack/{}",
        pack.file_name().unwrap().to_string_lossy()
    );
    git(dir, &["verify-pack", "-v", &relative])
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() >= 7 && fields[0].len() == 40).then(|| fields[5].parse().unwrap())
        })
        .max()
        .unwrap_or(0)
}

/// A history of 40 commits of a growing file and a file edited in place,
/// in two directories, every object loose (as zerogit writes them).
fn loose_history() -> tempfile::TempDir {
    let temp = repository(&["base.txt"]);
    let dir = temp.path();
    git(dir, &["config", "gc.auto", "0"]);
    let mut log = String::new();
    let mut table: Vec<String> = (0..200).map(|i| format!("row {} value 0\n", i)).collect();
    for i in 0..40 {
        log.push_str(&format!("entry {} with some text to make it longer\n", i));
        table[(i * 37) % 200] = format!("row {} value {}\n", (i * 37) % 200, i);
        write(dir, "docs/log.txt", &log);
        write(dir, "data/table.txt", &table.concat());
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Commit {}", i)]);
    }
    temp
}

#[test]
fn repack_encodes_loose_objects_as_deltas() {
    let temp = loose_history();
    let source = temp.path();
    assert!(object_files(source).1.is_empty());

    let copy = twin(source);
    let dir = copy.path();
    let summary = Repository::open(dir).unwrap().repack().unwrap();
    let pack = summary.pack().unwrap().to_path_buf();
    let (packed, deltas) = verify_pack(dir, &pack);
    assert_eq!(packed, git_reachable(dir));
    assert_eq!(summary.reused_deltas(), 0);
    assert_eq!(deltas, summary.new_deltas());
    // Most versions of the files are deltas (Git makes 76 here; the
    // commits and trees are too small to gain).
    assert!(summary.new_deltas() >= 70, "{}", summary.new_deltas());
    assert!(longest_chain(dir, &pack) <= 50);
    assert_fsck(dir);
    // Git reads every object back.
    let read = git_output(dir, &["cat-file", "--batch-all-objects", "--batch"]);
    assert!(read.status.success());

    // About the size of Git's own pack.
    let theirs = twin(source);
    git(theirs.path(), &["repack", "-a", "-d", "-q"]);
    let size = |dir: &Path| -> u64 {
        let packs = object_files(dir).1;
        let name = packs.iter().find(|p| p.ends_with(".pack")).unwrap();
        fs::metadata(dir.join(".git/objects/pack").join(name))
            .unwrap()
            .len()
    };
    let (ours, git_size) = (size(dir), size(theirs.path()));
    assert!(ours < git_size * 3 / 2, "{} vs {}", ours, git_size);

    // pack.depth limits the chains; pack.window = 0 turns deltas off.
    let shallow = twin(source);
    git(shallow.path(), &["config", "pack.depth", "3"]);
    let summary = Repository::open(shallow.path()).unwrap().repack().unwrap();
    assert!(summary.new_deltas() > 0);
    assert!(longest_chain(shallow.path(), summary.pack().unwrap()) <= 3);
    assert_fsck(shallow.path());
    let off = twin(source);
    git(off.path(), &["config", "pack.window", "0"]);
    let summary = Repository::open(off.path()).unwrap().repack().unwrap();
    assert_eq!(summary.new_deltas(), 0);
    assert_eq!(verify_pack(off.path(), summary.pack().unwrap()).1, 0);
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

/// Writes `content` as a loose blob and returns its ID.
fn write_blob(dir: &Path, content: &str) -> String {
    write(dir, "blob.tmp", content);
    let oid = git(dir, &["hash-object", "-w", "blob.tmp"])
        .trim()
        .to_owned();
    fs::remove_file(dir.join("blob.tmp")).unwrap();
    oid
}

/// Writes a tree holding `blob` as `name`, without referring to it from
/// anywhere, and returns its ID.
fn write_tree(dir: &Path, name: &str, blob: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("mktree")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("100644 blob {}\t{}\n", blob, name).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Waits until the file system's clock has moved past now, and returns a
/// time between what was written before and what is written after.
fn time_boundary() -> std::time::SystemTime {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let boundary = std::time::SystemTime::now();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    boundary
}

/// Git's form of a time for `--expire`.
fn git_time(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("@{}", seconds)
}

#[test]
fn prune_removes_what_git_prune_removes() {
    let temp = packed_history();
    // Copying does not keep modification times everywhere (it does not on
    // Linux), so copy first, for zerogit and Git with and without an
    // expiry, and write the objects whose age matters into each copy.
    let copies: Vec<tempfile::TempDir> = (0..4).map(|_| twin(temp.path())).collect();
    let dirs: Vec<&Path> = copies.iter().map(|copy| copy.path()).collect();
    let (mut old_alone, mut old_referenced) = (String::new(), String::new());
    for dir in &dirs {
        // Old: one unreachable blob alone, another a recent tree refers to.
        old_alone = write_blob(dir, "old and alone\n");
        old_referenced = write_blob(dir, "old, but a recent tree has it\n");
        // A temporary file left by an interrupted write, old by then.
        write(dir, ".git/objects/pack/tmp_pack_leftover", "partial");
    }
    let boundary = time_boundary();
    let (mut recent_blob, mut recent_tree) = (String::new(), String::new());
    for dir in &dirs {
        recent_blob = write_blob(dir, "recent\n");
        recent_tree = write_tree(dir, "f", &old_referenced);
    }

    for (i, expire) in [Some(boundary), None].into_iter().enumerate() {
        let (o, t) = (dirs[2 * i], dirs[2 * i + 1]);
        let removed = Repository::open(o).unwrap().prune(expire).unwrap();
        match expire {
            Some(boundary) => git(t, &["prune", "--expire", &git_time(boundary)]),
            None => git(t, &["prune"]),
        };
        assert_eq!(object_files(o), object_files(t), "{:?}", expire);
        let (loose, _) = object_files(o);
        assert!(!loose.contains(&old_alone));
        if expire.is_some() {
            assert_eq!(removed, 1);
            for kept in [&old_referenced, &recent_blob, &recent_tree] {
                assert!(loose.contains(kept));
            }
        } else {
            for gone in [&old_referenced, &recent_blob, &recent_tree] {
                assert!(!loose.contains(gone));
            }
        }
        assert!(!o.join(".git/objects/pack/tmp_pack_leftover").exists());
        assert_fsck(o);
    }
}

#[test]
fn gc_matches_git_gc() {
    let temp = packed_history();
    let dir = temp.path();
    write_blob(dir, "unreachable\n");
    git(dir, &["config", "gc.pruneExpire", "now"]);
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    let summary = Repository::open(o).unwrap().gc().unwrap();
    git(t, &["gc", "-q"]);
    assert_eq!(summary.pruned(), 1);
    assert!(!o.join(".git/gc.pid.lock").exists());

    // The same references, packed the same way, and one pack with the
    // same objects; nothing loose.
    assert_eq!(ref_files(o), ref_files(t));
    let (loose, packs) = object_files(o);
    assert_eq!(loose, Vec::<String>::new());
    assert_eq!(object_files(t).0, Vec::<String>::new());
    let pack_of = |dir: &Path, packs: &[String]| {
        let name = packs.iter().find(|p| p.ends_with(".pack")).unwrap().clone();
        verify_pack(dir, &dir.join(".git/objects/pack").join(name)).0
    };
    assert_eq!(pack_of(o, &packs), pack_of(t, &object_files(t).1));
    assert_fsck(o);

    // Unreachable objects are kept while recent, by default.
    let kept = twin(dir);
    git(kept.path(), &["config", "--unset", "gc.pruneExpire"]);
    assert_eq!(
        Repository::open(kept.path())
            .unwrap()
            .gc()
            .unwrap()
            .pruned(),
        0
    );
    assert_eq!(object_files(kept.path()).0.len(), 1);

    // Another gc running: nothing happens.
    let locked = twin(dir);
    fs::write(locked.path().join(".git/gc.pid.lock"), "").unwrap();
    let before = object_files(locked.path());
    assert!(matches!(
        Repository::open(locked.path()).unwrap().gc(),
        Err(Error::Locked(_))
    ));
    assert_eq!(object_files(locked.path()), before);
}

#[test]
fn gc_auto_runs_only_when_needed() {
    let temp = packed_history();
    let dir = temp.path();
    // The fixture turns automatic gc off for Git; turn it back on.
    git(dir, &["config", "--unset", "gc.auto"]);
    let repo = Repository::open(dir).unwrap();
    // A few loose objects and one pack: not needed.
    assert_eq!(repo.gc_auto().unwrap(), None);
    // More packs than gc.autoPackLimit.
    git(dir, &["config", "gc.autoPackLimit", "1"]);
    pack_objects(dir, &format!("{}\n", write_blob(dir, "another pack\n")));
    let summary = repo.gc_auto().unwrap().expect("gc is needed");
    assert_eq!(summary.repack().removed_packs(), 2);
    assert_eq!(repo.gc_auto().unwrap(), None);
    // gc.auto = 0 turns it off.
    git(dir, &["config", "gc.auto", "0"]);
    pack_objects(dir, &format!("{}\n", write_blob(dir, "yet another\n")));
    pack_objects(dir, &format!("{}\n", write_blob(dir, "and one more\n")));
    assert_eq!(repo.gc_auto().unwrap(), None);
}
