//! packed-refs resolution, enumeration and write protection.

use std::{fs, process::Command};
use tempfile::TempDir;
use zerogit::objects::{LooseObjectStore, ObjectType};
use zerogit::refs::RefStore;
use zerogit::{Error, Oid, Repository};

const OTHER: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

fn repository() -> (TempDir, Repository, Oid) {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    fs::write(temp.path().join("document.txt"), "initial\n").unwrap();
    repo.add("document.txt").unwrap();
    let oid = repo
        .create_commit("Initial", "Test", "test@example.com")
        .unwrap();
    (temp, repo, oid)
}

fn write_ref(repo: &Repository, name: &str, content: &str) {
    let path = repo.git_dir().join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn git_pack_refs_preserves_head_branches_remotes_and_tags() {
    let (_temp, repo, oid) = repository();
    repo.create_branch("feature/nested", Some(oid)).unwrap();
    write_ref(&repo, "refs/remotes/origin/feature/nested", &oid.to_hex());
    write_ref(
        &repo,
        "refs/remotes/origin/HEAD",
        "ref: refs/remotes/origin/feature/nested\n",
    );
    write_ref(&repo, "refs/tags/light", &oid.to_hex());
    let content = format!("object {oid}\ntype commit\ntag annotated\ntagger Test <test@example.com> 1234567890 +0000\n\nAnnotated message\n");
    let tag_oid = LooseObjectStore::new(repo.git_dir().join("objects"))
        .write(ObjectType::Tag, content.as_bytes())
        .unwrap();
    write_ref(&repo, "refs/tags/annotated", &tag_oid.to_hex());

    let snapshot = || {
        (
            repo.head().unwrap().oid().to_hex(),
            repo.branches()
                .unwrap()
                .into_iter()
                .map(|b| (b.name().to_owned(), b.oid().to_hex(), b.is_current()))
                .collect::<Vec<_>>(),
            repo.remote_branches()
                .unwrap()
                .into_iter()
                .map(|b| (b.remote().to_owned(), b.name().to_owned(), b.oid().to_hex()))
                .collect::<Vec<_>>(),
            repo.tags()
                .unwrap()
                .into_iter()
                .map(|t| {
                    (
                        t.name().to_owned(),
                        t.target().to_hex(),
                        t.message().map(str::to_owned),
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    let before = snapshot();
    let output = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["pack-refs", "--all", "--prune"])
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
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!repo.git_dir().join("refs/heads/main").exists());
    assert_eq!(snapshot(), before);
    let store = RefStore::new(repo.git_dir());
    assert_eq!(store.resolve("annotated").unwrap().oid, tag_oid);
    assert_eq!(store.remotes().unwrap(), vec!["origin"]);
    repo.checkout("feature/nested").unwrap();
    assert_eq!(repo.head().unwrap().branch_name(), Some("feature/nested"));
    assert!(repo.status().unwrap().is_empty());
}

#[test]
fn loose_refs_override_packed_and_lists_are_deduplicated_sorted() {
    let (_temp, repo, original) = repository();
    write_ref(&repo, "packed-refs", &format!("{OTHER} refs/heads/main\n{original} refs/heads/a\n{original} refs/remotes/upstream/topic/nested\n"));
    let store = RefStore::new(repo.git_dir());
    assert_eq!(store.resolve("main").unwrap().oid, original);
    assert_eq!(store.branches().unwrap(), vec!["a", "main"]);
    assert_eq!(
        store.remote_branches().unwrap(),
        vec![("upstream".into(), "topic/nested".into())]
    );
    assert_eq!(store.remotes().unwrap(), vec!["upstream"]);
    assert_eq!(repo.branches().unwrap().len(), 2);
}

#[test]
fn corrupt_loose_ref_never_falls_back_to_valid_packed_ref() {
    let (_temp, repo, oid) = repository();
    write_ref(
        &repo,
        "packed-refs",
        &format!("{oid} refs/heads/main\n{oid} refs/tags/main\n"),
    );
    write_ref(&repo, "refs/heads/main", "corrupt\n");
    assert!(matches!(repo.head(), Err(Error::InvalidOid(_))));
    assert!(matches!(repo.branches(), Err(Error::InvalidOid(_))));
    assert!(matches!(
        RefStore::new(repo.git_dir()).resolve("main"),
        Err(Error::InvalidOid(_))
    ));
}

#[test]
fn malformed_packed_refs_and_io_failures_are_not_empty_lists() {
    let (_temp, repo, _) = repository();
    write_ref(&repo, "packed-refs", "broken refs/heads/feature\n");
    assert!(matches!(
        repo.branches(),
        Err(Error::InvalidPackedRefs { line: 1, .. })
    ));
    assert!(matches!(
        repo.remote_branches(),
        Err(Error::InvalidPackedRefs { .. })
    ));
    assert!(matches!(repo.tags(), Err(Error::InvalidPackedRefs { .. })));
    fs::remove_file(repo.git_dir().join("packed-refs")).unwrap();
    fs::create_dir(repo.git_dir().join("packed-refs")).unwrap();
    assert!(matches!(repo.branches(), Err(Error::Io(_))));
    assert!(matches!(
        RefStore::new(repo.git_dir()).resolve("feature"),
        Err(Error::Io(_))
    ));
}

#[test]
fn create_refuses_and_delete_removes_packed_branches() {
    let (_temp, repo, oid) = repository();
    let packed = format!("{oid} refs/heads/feature\n{oid} refs/heads/other\n");
    write_ref(&repo, "packed-refs", &packed);
    assert!(matches!(
        repo.create_branch("feature", Some(oid)),
        Err(Error::RefAlreadyExists(_))
    ));
    // Packed only.
    repo.delete_branch("feature").unwrap();
    assert!(!repo.git_dir().join("refs/heads/feature").exists());
    assert_eq!(
        fs::read_to_string(repo.git_dir().join("packed-refs")).unwrap(),
        format!("{oid} refs/heads/other\n")
    );
    assert!(matches!(
        repo.delete_branch("feature"),
        Err(Error::RefNotFound(_))
    ));
    // Loose and packed: both go, so no older value reappears.
    write_ref(&repo, "refs/heads/other", &oid.to_hex());
    repo.delete_branch("other").unwrap();
    assert!(!repo.git_dir().join("refs/heads/other").exists());
    assert_eq!(
        fs::read_to_string(repo.git_dir().join("packed-refs")).unwrap(),
        ""
    );
    assert!(repo.branches().unwrap().iter().all(|b| b.name() == "main"));
    assert!(matches!(
        repo.delete_branch("main"),
        Err(Error::CannotDeleteCurrentBranch)
    ));
}

#[test]
fn packed_unborn_missing_and_symbolic_cycles_remain_distinct() {
    let temp = TempDir::new().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    write_ref(&repo, "packed-refs", "# pack-refs with: peeled\n");
    assert!(repo.status().unwrap().is_empty());
    assert!(matches!(repo.head(), Err(Error::RefNotFound(_))));
    write_ref(&repo, "refs/heads/main", "ref: refs/heads/cycle\n");
    write_ref(&repo, "refs/heads/cycle", "ref: refs/heads/main\n");
    assert!(matches!(repo.head(), Err(Error::InvalidRefName(_))));
    assert!(matches!(repo.branches(), Err(Error::InvalidRefName(_))));
}

#[test]
fn missing_or_corrupt_tag_object_is_not_reported_as_lightweight() {
    let (_temp, repo, oid) = repository();
    write_ref(&repo, "refs/tags/broken", OTHER);
    assert!(matches!(repo.tags(), Err(Error::ObjectNotFound(_))));
    write_ref(&repo, "refs/tags/broken", &oid.to_hex());
    assert!(!repo.tags().unwrap()[0].is_annotated());
    let hex = oid.to_hex();
    fs::write(
        repo.git_dir()
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..]),
        "corrupt",
    )
    .unwrap();
    assert!(matches!(repo.tags(), Err(Error::DecompressionFailed)));
}

#[test]
fn locked_refs_are_not_enumerated_and_invalid_paths_are_rejected() {
    let (_temp, repo, oid) = repository();
    write_ref(&repo, "refs/heads/feature.lock", &oid.to_hex());
    assert_eq!(
        RefStore::new(repo.git_dir()).branches().unwrap(),
        vec!["main"]
    );
    assert!(matches!(
        RefStore::new(repo.git_dir()).resolve("../HEAD"),
        Err(Error::InvalidRefName(_))
    ));
    assert!(matches!(
        repo.delete_branch("../HEAD"),
        Err(Error::InvalidRefName(_))
    ));
}
