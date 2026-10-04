//! Creating and deleting tags compared with Git (#27).

mod common;

use common::*;
use std::path::Path;
use zerogit::{Error, Oid, Repository};

fn oid(dir: &Path, rev: &str) -> Oid {
    Oid::from_hex(git(dir, &["rev-parse", rev]).trim()).unwrap()
}

/// The tag object's headers and message, without the tagger's timestamp.
fn tag_object_without_time(dir: &Path, tag: &str) -> String {
    git(dir, &["cat-file", "tag", tag])
        .lines()
        .map(|line| match line.strip_prefix("tagger ") {
            Some(rest) => format!("tagger {}", &rest[..rest.find('>').unwrap() + 1]),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lightweight_tag_matches_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let tag = repo.create_tag("v1.0", None).unwrap();
    git(dir, &["tag", "git-v1.0"]);

    assert!(!tag.is_annotated());
    assert_eq!(tag.target(), &oid(dir, "HEAD"));
    assert_eq!(
        git(dir, &["cat-file", "-t", "v1.0"]),
        git(dir, &["cat-file", "-t", "git-v1.0"])
    );
    assert_eq!(oid(dir, "v1.0"), oid(dir, "git-v1.0"));
    assert_eq!(git(dir, &["tag", "-l"]), "git-v1.0\nv1.0\n");
    assert_fsck(dir);
}

#[test]
fn annotated_tag_matches_git() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let message = "  Release 1.0  \n\n\n  Notes line   \n\n";
    let tag = repo
        .create_annotated_tag("v1.0", None, message, "Test", "test@example.com")
        .unwrap();
    git(dir, &["tag", "-a", "-m", message, "same"]);

    assert!(tag.is_annotated());
    assert_eq!(tag.target(), &oid(dir, "HEAD"));
    assert_eq!(
        tag_object_without_time(dir, "v1.0"),
        tag_object_without_time(dir, "same").replace("tag same", "tag v1.0")
    );
    // Git reads it like its own tags.
    assert_eq!(oid(dir, "v1.0^{commit}"), oid(dir, "HEAD"));
    git(dir, &["show", "v1.0"]);
    assert_fsck(dir);

    // tags() lists it with its message and tagger.
    let listed = repo
        .tags()
        .unwrap()
        .into_iter()
        .find(|t| t.name() == "v1.0")
        .unwrap();
    assert_eq!(listed.message(), tag.message());
    assert_eq!(listed.tagger().unwrap().name(), "Test");
    assert_eq!(listed.tagger().unwrap().email(), "test@example.com");
    assert!(listed.message().unwrap().starts_with("  Release 1.0"));
}

#[test]
fn tags_can_point_to_any_object_type() {
    let temp = repository(&["dir/a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let tree = oid(dir, "HEAD^{tree}");
    let blob = oid(dir, "HEAD:dir/a.txt");
    repo.create_annotated_tag("tree-tag", Some(tree), "A tree", "T", "t@example.com")
        .unwrap();
    repo.create_annotated_tag("blob-tag", Some(blob), "A blob", "T", "t@example.com")
        .unwrap();
    let inner = oid(dir, "tree-tag");
    repo.create_annotated_tag("tag-tag", Some(inner), "A tag", "T", "t@example.com")
        .unwrap();
    repo.create_tag("light-blob", Some(blob)).unwrap();

    assert!(git(dir, &["cat-file", "tag", "tree-tag"]).contains("\ntype tree\n"));
    assert!(git(dir, &["cat-file", "tag", "blob-tag"]).contains("\ntype blob\n"));
    assert!(git(dir, &["cat-file", "tag", "tag-tag"]).contains("\ntype tag\n"));
    assert_eq!(git(dir, &["cat-file", "-t", "light-blob"]), "blob\n");
    assert_eq!(oid(dir, "tag-tag^{tree}"), tree);
    assert_fsck(dir);
}

#[test]
fn invalid_existing_or_missing_targets_write_nothing() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    repo.create_tag("v1", None).unwrap();
    let objects = git(dir, &["count-objects", "-v"]);
    let refs = git(dir, &["for-each-ref"]);

    for name in [
        "",
        "-x",
        "a..b",
        "a b",
        "a~1",
        "a^",
        "a:b",
        "a?",
        "a*",
        "a[b",
        "a\\b",
        "/a",
        "a/",
        "a.",
        ".a",
        "a/.b",
        "a.lock",
        "a/b.lock/c",
        "a@{b",
        "a//b",
    ] {
        assert!(
            matches!(repo.create_tag(name, None), Err(Error::InvalidRefName(_))),
            "{:?}",
            name
        );
        // Git rejects the same names.
        let git_ok = git_output(dir, &["check-ref-format", &format!("refs/tags/{}", name)])
            .status
            .success();
        assert!(!git_ok || name.starts_with('-'), "git accepts {:?}", name);
    }
    for name in ["v2", "release/2.0", "日本語", "a@b", "x.y", "@"] {
        assert!(
            git_output(dir, &["check-ref-format", &format!("refs/tags/{}", name)])
                .status
                .success()
        );
    }

    assert!(matches!(
        repo.create_annotated_tag("v1", None, "m", "T", "t@example.com"),
        Err(Error::RefAlreadyExists(_))
    ));
    // "v1/x" would need refs/tags/v1 to be a directory.
    assert!(matches!(
        repo.create_tag("v1/x", None),
        Err(Error::RefAlreadyExists(_))
    ));
    let missing = Oid::from_hex("1234567890123456789012345678901234567890").unwrap();
    assert!(matches!(
        repo.create_annotated_tag("v2", Some(missing), "m", "T", "t@example.com"),
        Err(Error::ObjectNotFound(_))
    ));
    assert_eq!(git(dir, &["count-objects", "-v"]), objects);
    assert_eq!(git(dir, &["for-each-ref"]), refs);
}

#[test]
fn delete_tag_removes_ref_and_empty_directories() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    repo.create_annotated_tag("release/1.0", None, "m", "T", "t@example.com")
        .unwrap();
    repo.create_tag("v1", None).unwrap();
    repo.delete_tag("release/1.0").unwrap();
    repo.delete_tag("v1").unwrap();
    assert_eq!(git(dir, &["tag", "-l"]), "");
    assert!(!dir.join(".git/refs/tags/release").exists());
    assert!(matches!(repo.delete_tag("v1"), Err(Error::RefNotFound(_))));

    git(dir, &["tag", "packed"]);
    git(dir, &["pack-refs", "--all"]);
    assert!(matches!(
        repo.delete_tag("packed"),
        Err(Error::PackedRefDeletionUnsupported(_))
    ));
}

#[test]
fn branch_names_follow_check_ref_format() {
    let temp = repository(&["a.txt"]);
    let repo = Repository::open(temp.path()).unwrap();
    for name in [
        "a b",
        "a.",
        ".a",
        "a/.b",
        "a@{b",
        "@",
        "HEAD",
        "a//b",
        "a/b.lock/c",
    ] {
        assert!(
            matches!(
                repo.create_branch(name, None),
                Err(Error::InvalidRefName(_))
            ),
            "{:?}",
            name
        );
    }
    repo.create_branch("topic", None).unwrap();
    assert!(matches!(
        repo.create_branch("topic/x", None),
        Err(Error::RefAlreadyExists(_))
    ));
}
