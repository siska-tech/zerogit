//! `Repository::blob_reader` and `object_reader`: contents read as a stream
//! match Git's, wherever the object is stored, and corruption is caught
//! (#78).

mod common;

use common::*;
use std::fs;
use std::io::Read;
use std::path::Path;
use zerogit::objects::ObjectType;
use zerogit::{Error, Repository};

/// Deterministic, poorly compressible bytes.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn read_all(repo: &Repository, revision: &str) -> (Vec<u8>, bool) {
    let mut reader = repo.blob_reader(revision).unwrap();
    let streamed = reader.is_streamed();
    let mut content = Vec::new();
    reader.read_to_end(&mut content).unwrap();
    assert_eq!(content.len() as u64, reader.size());
    (content, streamed)
}

fn git_cat(dir: &Path, revision: &str) -> Vec<u8> {
    git_output(dir, &["cat-file", "blob", revision]).stdout
}

/// A history of text files (which Git packs as deltas), a 20 MB binary
/// file and an empty one.
fn history() -> tempfile::TempDir {
    let temp = empty_repository();
    let dir = temp.path();
    let mut text = String::new();
    for i in 0..5 {
        for line in 0..200 {
            text.push_str(&format!("version {} line {}\n", i, line));
        }
        write(dir, "text.txt", &text);
        if i == 0 {
            fs::write(dir.join("big.bin"), noise(1, 20 << 20)).unwrap();
            write(dir, "empty", "");
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Version {}", i)]);
    }
    temp
}

#[test]
fn streamed_contents_match_git_loose_and_packed() {
    let temp = history();
    let dir = temp.path();
    let revisions = [
        "HEAD:text.txt",
        "HEAD~4:text.txt",
        "HEAD:big.bin",
        "HEAD:empty",
    ];

    // Loose: every object is streamed.
    let repo = Repository::open(dir).unwrap();
    for revision in revisions {
        let (content, streamed) = read_all(&repo, revision);
        assert_eq!(content, git_cat(dir, revision), "{}", revision);
        assert!(streamed, "{}", revision);
    }

    // Packed: objects stored whole are streamed, deltas rebuilt in memory.
    git(dir, &["gc", "-q"]);
    let repo = Repository::open(dir).unwrap();
    let mut streamed_count = 0;
    let mut in_memory = 0;
    for revision in revisions {
        let (content, streamed) = read_all(&repo, revision);
        assert_eq!(content, git_cat(dir, revision), "{}", revision);
        if streamed {
            streamed_count += 1;
        } else {
            in_memory += 1;
        }
    }
    assert!(
        streamed_count >= 2 && in_memory >= 1,
        "{} {}",
        streamed_count,
        in_memory
    );
    // The large binary file is stored whole by Git, and so streamed.
    assert!(repo.blob_reader("HEAD:big.bin").unwrap().is_streamed());
}

#[test]
fn readers_report_type_and_reject_other_types() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    let reader = repo.object_reader("HEAD").unwrap();
    assert_eq!(reader.object_type(), ObjectType::Commit);
    assert!(matches!(
        repo.blob_reader("HEAD"),
        Err(Error::TypeMismatch { .. })
    ));
    assert!(matches!(
        repo.blob_reader("HEAD:missing"),
        Err(Error::InvalidRevision { .. })
            | Err(Error::PathNotFound(_))
            | Err(Error::RefNotFound(_))
    ));
}

#[test]
fn a_loose_object_with_other_content_fails_at_the_end() {
    let temp = history();
    let dir = temp.path();
    let oid_of = |revision: &str| git(dir, &["rev-parse", revision]).trim().to_owned();
    let path_of = |oid: &str| dir.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    // The latest text.txt's file holds the first version's object instead.
    let (latest, first) = (oid_of("HEAD:text.txt"), oid_of("HEAD~4:text.txt"));
    // Git writes objects read-only, which older Rust cannot remove on
    // Windows.
    let target = path_of(&latest);
    let mut permissions = fs::metadata(&target).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&target, permissions).unwrap();
    fs::remove_file(&target).unwrap();
    fs::copy(path_of(&first), path_of(&latest)).unwrap();
    let repo = Repository::open(dir).unwrap();
    let mut reader = repo.blob_reader(&latest).unwrap();
    let mut content = Vec::new();
    let error = reader.read_to_end(&mut content).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}
