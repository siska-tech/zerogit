//! Line ending conversion (core.autocrlf, core.eol, .gitattributes) compared
//! with Git (#21).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use zerogit::{Error, Repository};

const ATTRIBUTES: &str = "\
*.txt text
*.crlf text eol=crlf
*.lf text eol=lf
*.auto text=auto
*.raw -text
*.bin binary
legacy.* crlf=input
";

/// Contents covering LF, CRLF, mixed, lone CR, binary and empty files.
const CONTENTS: &[(&str, &[u8])] = &[
    ("lf", b"one\ntwo\n"),
    ("crlf", b"one\r\ntwo\r\n"),
    ("mixed", b"one\r\ntwo\n"),
    ("lonecr", b"one\rtwo\n"),
    ("binary", b"one\r\n\0two\n"),
    ("empty", b""),
];

const EXTENSIONS: &[&str] = &["plain", "txt", "crlf", "lf", "auto", "raw", "bin"];

fn file_names() -> Vec<String> {
    let mut names = Vec::new();
    for (kind, _) in CONTENTS {
        for ext in EXTENSIONS {
            names.push(format!("{}.{}", kind, ext));
        }
        names.push(format!("legacy.{}", kind));
    }
    names
}

fn write_contents(dir: &Path) {
    for (kind, content) in CONTENTS {
        for ext in EXTENSIONS {
            fs::write(dir.join(format!("{}.{}", kind, ext)), content).unwrap();
        }
        fs::write(dir.join(format!("legacy.{}", kind)), content).unwrap();
    }
}

fn configure(dir: &Path, settings: &[(&str, &str)]) {
    for (key, value) in settings {
        git(dir, &["config", key, value]);
    }
}

const SETTINGS: &[&[(&str, &str)]] = &[
    &[("core.autocrlf", "false")],
    &[("core.autocrlf", "true")],
    &[("core.autocrlf", "input")],
    &[("core.autocrlf", "false"), ("core.eol", "crlf")],
    &[("core.autocrlf", "false"), ("core.eol", "lf")],
];

#[test]
fn added_blobs_match_git_hash_object() {
    for settings in SETTINGS {
        let temp = empty_repository();
        let dir = temp.path();
        configure(dir, settings);
        write(dir, ".gitattributes", ATTRIBUTES);
        write_contents(dir);
        let repo = Repository::open(dir).unwrap();
        repo.add_all().unwrap();
        for name in file_names() {
            let expected = git(dir, &["hash-object", "--path", &name, &name]);
            let staged = git(dir, &["ls-files", "-s", &name]);
            assert!(
                staged.contains(expected.trim()),
                "{:?} {}: git {} but staged {}",
                settings,
                name,
                expected.trim(),
                staged
            );
        }
    }
}

#[test]
fn checkout_writes_what_git_writes() {
    for settings in SETTINGS {
        let temp = empty_repository();
        let dir = temp.path();
        write(dir, ".gitattributes", ATTRIBUTES);
        write_contents(dir);
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "Contents"]);
        git(dir, &["checkout", "-q", "-b", "other"]);
        git(dir, &["rm", "-q", "-r", "--cached", "."]);
        fs::write(dir.join("placeholder"), "x").unwrap();
        git(dir, &["add", "placeholder"]);
        git(dir, &["commit", "-q", "-m", "Empty"]);
        git(dir, &["clean", "-q", "-f", "-x"]);
        configure(dir, settings);

        let repo = Repository::open(dir).unwrap();
        repo.checkout("main").unwrap();
        let ours: Vec<Vec<u8>> = file_names()
            .iter()
            .map(|name| fs::read(dir.join(name)).unwrap())
            .collect();
        assert!(repo.status().unwrap().is_empty(), "{:?}", settings);
        assert_eq!(git(dir, &["status", "--porcelain"]), "", "{:?}", settings);

        for name in file_names() {
            fs::remove_file(dir.join(name)).unwrap();
        }
        git(dir, &["checkout", "--", "."]);
        for (name, content) in file_names().iter().zip(ours) {
            assert_eq!(
                fs::read(dir.join(name)).unwrap(),
                content,
                "{:?} {}",
                settings,
                name
            );
        }
    }
}

#[test]
fn autocrlf_checkout_by_git_has_clean_status() {
    let temp = repository(&["a.txt", "dir/b.md"]);
    let dir = temp.path();
    write(dir, "c.txt", "one\ntwo\n");
    git(dir, &["add", "c.txt"]);
    git(dir, &["commit", "-q", "-m", "More"]);
    git(dir, &["config", "core.autocrlf", "true"]);
    for path in ["a.txt", "dir/b.md", "c.txt"] {
        fs::remove_file(dir.join(path)).unwrap();
    }
    git(dir, &["checkout", "--", "."]);
    assert_eq!(fs::read(dir.join("c.txt")).unwrap(), b"one\r\ntwo\r\n");

    let repo = Repository::open(dir).unwrap();
    assert!(repo.status().unwrap().is_empty());
    assert!(repo.diff_index_to_workdir().unwrap().deltas().is_empty());

    // Adding the CRLF file stores the same LF blob.
    let before = git_ls_files(dir);
    repo.add("c.txt").unwrap();
    repo.add_all().unwrap();
    assert_eq!(git_ls_files(dir), before);
}

#[test]
fn crlf_already_in_index_is_kept_by_auto_conversion() {
    let temp = empty_repository();
    let dir = temp.path();
    fs::write(dir.join("dos.txt"), b"one\r\ntwo\r\n").unwrap();
    git(dir, &["add", "dos.txt"]);
    git(dir, &["commit", "-q", "-m", "CRLF blob"]);
    git(dir, &["config", "core.autocrlf", "true"]);
    fs::write(dir.join("dos.txt"), b"one\r\ntwo\r\nthree\r\n").unwrap();

    // git hash-object does not consult the index, so compare with git add.
    let repo = Repository::open(dir).unwrap();
    repo.add("dos.txt").unwrap();
    let ours = git_ls_files(dir);
    git(dir, &["read-tree", "HEAD"]);
    git(dir, &["add", "dos.txt"]);
    assert_eq!(ours, git_ls_files(dir));
    assert_eq!(
        git(dir, &["cat-file", "-p", ":dos.txt"]),
        "one\r\ntwo\r\nthree\r\n"
    );
}

#[test]
fn safecrlf_true_rejects_irreversible_add() {
    let temp = empty_repository();
    let dir = temp.path();
    configure(dir, &[("core.autocrlf", "true"), ("core.safecrlf", "true")]);
    fs::write(dir.join("mixed.txt"), b"one\r\ntwo\n").unwrap();
    let repo = Repository::open(dir).unwrap();
    assert!(matches!(
        repo.add("mixed.txt"),
        Err(Error::IrreversibleLineEndings(_))
    ));
    assert!(!git_output(dir, &["add", "mixed.txt"]).status.success());
}

#[test]
fn configured_filter_is_reported_as_unsupported() {
    let temp = repository(&["README.md"]);
    let dir = temp.path();
    write(
        dir,
        ".gitattributes",
        "*.big filter=custom\n*.ignored filter=unset\n",
    );
    git(dir, &["config", "filter.custom.clean", "cat"]);
    git(dir, &["config", "filter.custom.smudge", "cat"]);
    write(dir, "data.big", "payload\n");
    write(dir, "data.ignored", "payload\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Filtered"]);

    let repo = Repository::open(dir).unwrap();
    // Unchanged files are recognised from their stat data.
    assert!(repo.status().unwrap().is_empty());
    match repo.add("data.big") {
        Err(Error::UnsupportedAttribute { attribute, .. }) => assert_eq!(attribute, "filter"),
        other => panic!("expected UnsupportedAttribute, got {:?}", other),
    }
    // A filter without a configured driver is ignored, as Git ignores it.
    repo.add("data.ignored").unwrap();
}
