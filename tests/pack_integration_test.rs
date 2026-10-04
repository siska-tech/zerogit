//! Repository APIs over loose objects, packs and packed-refs.
//!
//! Git is used only to build fixtures and to produce expected values.

use std::path::Path;
use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::log::LogOptions;
use zerogit::objects::{LooseObjectStore, ObjectType};
use zerogit::{Error, Oid, Repository};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Background auto-maintenance would repack concurrently with the test.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// History with nested paths, edits that delta well, a rename, a deletion,
/// a merge, branches and lightweight and annotated tags.
fn history() -> TempDir {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    let mut lines: Vec<String> = (0..120).map(|i| format!("paragraph {}", i)).collect();
    for revision in 0..12 {
        lines[(revision * 11) % 120] = format!("revised in {}", revision);
        write(dir, "docs/guide.md", &lines.join("\n"));
        write(
            dir,
            "src/nested/lib.rs",
            &format!("// v{}\n{}", revision, "fn f() {}\n".repeat(revision + 1)),
        );
        if revision == 4 {
            write(dir, "old.txt", "to be renamed\n");
        }
        if revision == 6 {
            git(dir, &["mv", "old.txt", "renamed.txt"]);
        }
        if revision == 8 {
            write(dir, "temporary.txt", "short lived\n");
        }
        if revision == 9 {
            fs::remove_file(dir.join("temporary.txt")).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(
            dir,
            &["commit", "-q", "-m", &format!("Revision {}", revision)],
        );
    }
    git(dir, &["checkout", "-q", "-b", "feature"]);
    write(dir, "feature.txt", "feature work\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Feature"]);
    git(dir, &["checkout", "-q", "main"]);
    git(
        dir,
        &["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"],
    );
    git(dir, &["tag", "light"]);
    git(dir, &["tag", "-a", "annotated", "-m", "Annotated tag"]);
    temp
}

fn walk(repo: &Repository, tree: &Oid, prefix: &str, out: &mut Vec<String>) {
    for entry in repo.tree(&tree.to_hex()).unwrap().entries() {
        let path = format!("{}{}", prefix, entry.name());
        if entry.is_directory() {
            walk(repo, entry.oid(), &format!("{}/", path), out);
        } else {
            let blob = repo.blob(&entry.oid().to_hex()).unwrap();
            out.push(format!(
                "{} {} {:?}",
                path,
                entry.mode().as_octal(),
                String::from_utf8_lossy(blob.content())
            ));
        }
    }
}

/// Everything a reader of the repository can observe through the public API.
fn snapshot(repo: &Repository) -> Vec<String> {
    let mut out = Vec::new();
    let commits: Vec<_> = repo.log().unwrap().map(Result::unwrap).collect();
    for commit in &commits {
        out.push(format!(
            "commit {} tree {} parents {:?} {}",
            commit.oid(),
            commit.tree(),
            commit.parents(),
            commit.summary()
        ));
        let hex = commit.oid().to_hex();
        assert_eq!(repo.resolve_short_oid(&hex[..7]).unwrap(), *commit.oid());
        walk(repo, commit.tree(), "", &mut out);
        for delta in repo.commit_diff(commit).unwrap().deltas() {
            out.push(format!(
                "diff {} {:?} {:?}",
                delta.status_char(),
                delta.path(),
                delta.old_path()
            ));
        }
    }
    for options in [
        LogOptions::new().path("docs/"),
        LogOptions::new().path("renamed.txt"),
    ] {
        for commit in repo.log_with_options(options).unwrap() {
            out.push(format!("filtered {}", commit.unwrap().oid()));
        }
    }
    let first = repo.tree(&commits.last().unwrap().tree().to_hex()).unwrap();
    let last = repo.tree(&commits[0].tree().to_hex()).unwrap();
    out.push(format!(
        "{:?}",
        repo.diff_trees(Some(&first), &last).unwrap().deltas()
    ));
    for tag in repo.tags().unwrap() {
        out.push(format!(
            "tag {} {} {:?}",
            tag.name(),
            tag.target(),
            tag.message()
        ));
    }
    for branch in repo.branches().unwrap() {
        out.push(format!(
            "branch {} {} {}",
            branch.name(),
            branch.oid(),
            branch.is_current()
        ));
    }
    out.push(format!("head {}", repo.head().unwrap().oid()));
    out.push(format!("status {:?}", repo.status().unwrap()));
    out
}

fn loose_count(repo: &Repository) -> usize {
    fs::read_dir(repo.git_dir().join("objects"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().len() == 2)
        .map(|dir| fs::read_dir(dir).unwrap().count())
        .sum()
}

fn pack_count(repo: &Repository) -> usize {
    fs::read_dir(repo.git_dir().join("objects/pack"))
        .map(|entries| {
            entries
                .filter(|entry| {
                    entry
                        .as_ref()
                        .unwrap()
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == "pack")
                })
                .count()
        })
        .unwrap_or(0)
}

/// Reads every object Git knows about and compares commits with `git log`.
fn assert_matches_git(repo: &Repository) {
    let listing = git(
        repo.path(),
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ],
    );
    for line in listing.lines() {
        let (hex, kind) = line.split_once(' ').unwrap();
        match kind {
            "blob" => assert_eq!(
                String::from_utf8_lossy(repo.blob(hex).unwrap().content()),
                git(repo.path(), &["cat-file", "blob", hex]),
                "{}",
                hex
            ),
            "tree" => {
                let entries: Vec<String> = repo
                    .tree(hex)
                    .unwrap()
                    .entries()
                    .iter()
                    .map(|e| format!("{} {}", e.oid(), e.name()))
                    .collect();
                let expected: Vec<String> = git(repo.path(), &["ls-tree", hex])
                    .lines()
                    .map(|line| {
                        let (meta, name) = line.split_once('\t').unwrap();
                        format!("{} {}", meta.rsplit(' ').next().unwrap(), name)
                    })
                    .collect();
                assert_eq!(entries, expected, "{}", hex);
            }
            "commit" => {
                let commit = repo.commit(hex).unwrap();
                let raw = git(repo.path(), &["cat-file", "commit", hex]);
                assert!(raw.trim_end().ends_with(commit.message().trim_end()));
                let mut expected = git(repo.path(), &["rev-list", "--parents", "-n1", hex]);
                expected.push_str(&git(
                    repo.path(),
                    &["rev-parse", &format!("{}^{{tree}}", hex)],
                ));
                let mut actual = hex.to_owned();
                for parent in commit.parents() {
                    actual.push_str(&format!(" {}", parent));
                }
                actual.push_str(&format!("\n{}\n", commit.tree()));
                assert_eq!(actual, expected);
            }
            "tag" => {}
            other => panic!("unexpected type {}", other),
        }
    }
    let mut log: Vec<String> = repo
        .log()
        .unwrap()
        .map(|commit| commit.unwrap().oid().to_hex())
        .collect();
    let mut expected: Vec<String> = git(repo.path(), &["rev-list", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect();
    log.sort();
    expected.sort();
    assert_eq!(log, expected);
}

#[test]
fn repack_and_pack_refs_preserve_every_api_result() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    let before = snapshot(&repo);
    assert_eq!(pack_count(&repo), 0);

    git(temp.path(), &["repack", "-a", "-d", "-f", "--depth=50"]);
    git(temp.path(), &["pack-refs", "--all", "--prune"]);
    assert_eq!(loose_count(&repo), 0);
    assert_eq!(pack_count(&repo), 1);
    assert!(!repo.git_dir().join("refs/heads/main").exists());

    // The instance opened before the repack rescans; a fresh one starts packed.
    assert_eq!(snapshot(&repo), before);
    let reopened = Repository::open(temp.path()).unwrap();
    assert_eq!(snapshot(&reopened), before);
    assert_matches_git(&reopened);
}

#[test]
fn mixed_loose_and_multiple_packs_support_reads_and_writes() {
    let temp = history();
    git(temp.path(), &["repack", "-a", "-d"]);
    let repo = Repository::open(temp.path()).unwrap();

    // New loose objects on top of packed history, through zerogit's write path.
    write(temp.path(), "docs/guide.md", "rewritten after packing\n");
    repo.add("docs/guide.md").unwrap();
    let packed_head = *repo.head().unwrap().oid();
    let loose_commit = repo
        .create_commit("Loose commit", "Test", "test@example.com")
        .unwrap();
    assert_eq!(
        repo.commit(&loose_commit.to_hex()).unwrap().parent(),
        Some(&packed_head)
    );
    repo.create_branch("topic", None).unwrap();
    repo.checkout("feature").unwrap();
    repo.checkout("main").unwrap();
    assert!(repo.status().unwrap().is_empty());

    // An incremental pack, then more loose objects: two packs plus loose.
    git(temp.path(), &["repack", "-d"]);
    write(temp.path(), "after.txt", "after incremental repack\n");
    repo.add("after.txt").unwrap();
    repo.create_commit("After repack", "Test", "test@example.com")
        .unwrap();
    assert_eq!(pack_count(&repo), 2);
    assert!(loose_count(&repo) > 0);
    assert_matches_git(&repo);

    // Duplicate every object: keep the old packs and loose files, add a full pack.
    let before = snapshot(&repo);
    git(temp.path(), &["repack", "-a"]);
    assert_eq!(pack_count(&repo), 3);
    assert_eq!(snapshot(&repo), before);
    assert_eq!(snapshot(&Repository::open(temp.path()).unwrap()), before);
    git(temp.path(), &["fsck", "--strict", "--no-dangling"]);
}

#[test]
fn prefix_collisions_between_packed_and_loose_objects_are_ambiguous() {
    // Find two blobs whose IDs share their first four hex digits.
    let scratch = TempDir::new().unwrap();
    let hasher = LooseObjectStore::new(scratch.path());
    let mut seen = std::collections::HashMap::new();
    let (first, second) = (0..)
        .find_map(|i| {
            let content = format!("collision candidate {}\n", i);
            let oid = hasher.write(ObjectType::Blob, content.as_bytes()).unwrap();
            seen.insert(oid.to_hex()[..4].to_owned(), content.clone())
                .map(|other| (other, content))
        })
        .unwrap();

    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    write(temp.path(), "first.txt", &first);
    repo.add("first.txt").unwrap();
    repo.create_commit("First", "Test", "test@example.com")
        .unwrap();
    git(temp.path(), &["repack", "-a", "-d"]);
    let store = LooseObjectStore::new(repo.git_dir().join("objects"));
    let packed = hasher.write(ObjectType::Blob, first.as_bytes()).unwrap();
    let loose = store.write(ObjectType::Blob, second.as_bytes()).unwrap();
    assert!(!store.exists(&packed));

    let packed_hex = packed.to_hex();
    let loose_hex = loose.to_hex();
    assert_eq!(packed_hex[..4], loose_hex[..4]);
    assert!(matches!(
        repo.resolve_short_oid(&packed_hex[..4]),
        Err(Error::InvalidOid(_))
    ));
    let unique = (4..40)
        .find(|&len| packed_hex[..len] != loose_hex[..len])
        .unwrap()
        + 1;
    assert_eq!(
        repo.resolve_short_oid(&packed_hex[..unique]).unwrap(),
        packed
    );
    assert_eq!(repo.resolve_short_oid(&loose_hex[..unique]).unwrap(), loose);
    assert_eq!(repo.blob(&packed_hex).unwrap().content(), first.as_bytes());
    assert_eq!(repo.blob(&loose_hex).unwrap().content(), second.as_bytes());
}

#[test]
fn packs_replaced_by_external_gc_are_picked_up() {
    let temp = history();
    git(temp.path(), &["repack", "-a", "-d"]);
    let repo = Repository::open(temp.path()).unwrap();
    snapshot(&repo); // opens the first pack

    write(temp.path(), "external.txt", "committed by git\n");
    git(temp.path(), &["add", "-A"]);
    git(temp.path(), &["commit", "-q", "-m", "External"]);
    git(temp.path(), &["gc", "-q", "--prune=now"]);
    assert_eq!(pack_count(&repo), 1);
    assert_eq!(loose_count(&repo), 0);

    let head = repo.head().unwrap();
    assert_eq!(
        repo.commit(&head.oid().to_hex()).unwrap().summary(),
        "External"
    );
    assert_eq!(
        snapshot(&repo),
        snapshot(&Repository::open(temp.path()).unwrap())
    );

    // An object that exists nowhere stays an explicit, ordinary miss.
    let missing = "0123456789012345678901234567890123456789";
    assert!(matches!(repo.blob(missing), Err(Error::ObjectNotFound(_))));
}

#[test]
fn unsupported_repository_formats_are_rejected_at_open() {
    let temp = TempDir::new().unwrap();
    git(
        temp.path(),
        &["init", "-q", "--object-format=sha256", "sha256"],
    );
    let sha256 = temp.path().join("sha256");
    assert!(matches!(
        Repository::open(&sha256),
        Err(Error::UnsupportedRepositoryFormat(_))
    ));
    assert!(matches!(
        Repository::discover(sha256.join(".git")),
        Err(Error::UnsupportedRepositoryFormat(_))
    ));

    let repo = Repository::init(temp.path().join("reftable")).unwrap();
    let config = repo.git_dir().join("config");
    let mut text = fs::read_to_string(&config).unwrap();
    text = text.replace("repositoryformatversion = 0", "repositoryformatversion = 1");
    text.push_str("[extensions]\n\trefStorage = reftable\n");
    fs::write(&config, text).unwrap();
    assert!(matches!(
        Repository::open(repo.path()),
        Err(Error::UnsupportedRepositoryFormat(_))
    ));

    let repo = Repository::init(temp.path().join("sha1")).unwrap();
    let config = repo.git_dir().join("config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[extensions]\n\tobjectFormat = sha1\n");
    fs::write(&config, text).unwrap();
    assert!(Repository::open(repo.path()).is_ok());
}
