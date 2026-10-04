//! Storing received packs and building packs to send, compared with Git (#32).

mod common;

use common::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use zerogit::{Oid, Repository};

fn git_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// A repository with history where later versions delta well against
/// earlier ones.
fn history() -> tempfile::TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    let mut text = String::new();
    for i in 0..40 {
        text.push_str(&format!("line {} of a long enough document to delta\n", i));
        write(dir, "doc.txt", &text);
        write(dir, &format!("dir{}/f.txt", i % 3), &format!("v{}\n", i));
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Commit {}", i)]);
    }
    git(dir, &["tag", "-a", "-m", "tag", "v1", "HEAD~5"]);
    temp
}

#[test]
fn stored_git_pack_matches_git_index_pack() {
    let source = history();
    let pack = git_stdin(
        source.path(),
        &["pack-objects", "--stdout", "--revs", "--delta-base-offset"],
        b"main\nv1\n",
    );
    let target = empty_repository();
    let repo = Repository::open(target.path()).unwrap();
    let stored = repo.store_pack(&pack).unwrap();
    assert!(stored.objects().len() > 100);

    // Git agrees on the index, byte for byte.
    let ours = fs::read(stored.path().with_extension("idx")).unwrap();
    // Outside the repository: Git writes the index read-only, which
    // Windows does not let the test remove.
    let scratch = tempfile::TempDir::new().unwrap();
    let copy = scratch.path().join("copy.pack");
    fs::write(&copy, &pack).unwrap();
    git(target.path(), &["index-pack", copy.to_str().unwrap()]);
    assert_eq!(ours, fs::read(scratch.path().join("copy.idx")).unwrap());

    // The objects are usable.
    let head = git(source.path(), &["rev-parse", "main"]);
    git(
        target.path(),
        &["update-ref", "refs/heads/main", head.trim()],
    );
    git(
        target.path(),
        &[
            "update-ref",
            "refs/tags/v1",
            git(source.path(), &["rev-parse", "v1"]).trim(),
        ],
    );
    assert_fsck(target.path());
    let commit = repo.commit(head.trim()).unwrap();
    assert_eq!(commit.summary(), "Commit 39");
}

#[test]
fn thin_pack_is_completed_with_local_bases() {
    let source = history();
    let target_temp = empty_repository();
    let target = target_temp.path();
    // The target has history up to HEAD~10.
    let base = git(source.path(), &["rev-parse", "HEAD~10"])
        .trim()
        .to_owned();
    let early = git_stdin(
        source.path(),
        &["pack-objects", "--stdout", "--revs"],
        format!("{}\n", base).as_bytes(),
    );
    let repo = Repository::open(target).unwrap();
    repo.store_pack(&early).unwrap();

    let thin = git_stdin(
        source.path(),
        &["pack-objects", "--stdout", "--revs", "--thin"],
        format!("main\n^{}\n", base).as_bytes(),
    );
    // A thin pack cannot be indexed on its own.
    let alone = empty_repository();
    assert!(Repository::open(alone.path())
        .unwrap()
        .store_pack(&thin)
        .is_err());

    let stored = repo.store_pack(&thin).unwrap();
    git(
        target,
        &[
            "update-ref",
            "refs/heads/main",
            git(source.path(), &["rev-parse", "main"]).trim(),
        ],
    );
    assert_fsck(target);
    git(
        target,
        &[
            "verify-pack",
            // Relative: Git for Windows cannot open `\\?\` paths.
            &format!(
                ".git/objects/pack/{}",
                stored
                    .path()
                    .with_extension("idx")
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            ),
        ],
    );
}

#[test]
fn built_pack_has_what_git_rev_list_lists() {
    let source = history();
    let dir = source.path();
    let repo = Repository::open(dir).unwrap();
    let want = Oid::from_hex(git(dir, &["rev-parse", "main"]).trim()).unwrap();
    let tag = Oid::from_hex(git(dir, &["rev-parse", "v1"]).trim()).unwrap();
    let have = Oid::from_hex(git(dir, &["rev-parse", "HEAD~20"]).trim()).unwrap();

    let mut ours: Vec<String> = repo
        .objects_to_send(&[want, tag], &[have])
        .unwrap()
        .iter()
        .map(Oid::to_hex)
        .collect();
    ours.sort();
    let mut expected: Vec<String> = git(
        dir,
        &["rev-list", "--objects", "main", "v1", &format!("^{}", have)],
    )
    .lines()
    .map(|l| l.split(' ').next().unwrap().to_owned())
    .collect();
    expected.sort();
    assert_eq!(ours, expected);

    // Git can index the pack and the receiver ends up complete.
    let pack = repo.pack_objects(&[want], &[have]).unwrap();
    let target_temp = empty_repository();
    let target = target_temp.path();
    let early = git_stdin(
        dir,
        &["pack-objects", "--stdout", "--revs"],
        format!("{}\n", have).as_bytes(),
    );
    git_stdin(target, &["index-pack", "--stdin"], &early);
    git_stdin(target, &["index-pack", "--stdin"], &pack);
    git(target, &["update-ref", "refs/heads/main", &want.to_hex()]);
    assert_fsck(target);
}
