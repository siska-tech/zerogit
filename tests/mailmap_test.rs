//! `Repository::mailmap` and `Mailmap::resolve` compared with
//! `git check-mailmap` (#77).

mod common;

use common::*;
use std::path::Path;
use zerogit::Repository;

const MAILMAP: &str = "\
# Comments start at the beginning of a line <ignored@example.com>
Proper Name <commit@example.com>
<proper@example.com> <Old@Example.com>
Both New <both@example.com> <both-old@example.com>
Only Joe <joe@example.com> Joe <shared@example.com>
Other <other@example.com> Someone Else <shared@example.com>
Shared <shared-default@example.com> <shared@example.com>
<email-only@example.com> Named <named@example.com>
NoSpace<nospace@example.com>
\tTabbed   Name\t <tabbed@example.com>   trailing text
First <dup@example.com>
Second <dup@example.com>
Anything <empty-old@example.com> <>
Comment <c@example.com> # a trailing comment
Weird <weird@example.com> # comment <weird-old@example.com>
Éric <eric@example.com> ÉRIC <accent@example.com>
not an entry
Empty <>
Windows <crlf@example.com>\r
";

/// Identities to look up: every mapping, case variations, and misses.
const QUERIES: &[&str] = &[
    "Anyone <commit@example.com>",
    "Anyone <COMMIT@example.com>",
    "Anyone <old@example.com>",
    "Anyone <both-old@example.com>",
    "Joe <shared@example.com>",
    "JOE <SHARED@example.com>",
    "Someone Else <shared@example.com>",
    "Nobody <shared@example.com>",
    "Named <named@example.com>",
    "Unnamed <named@example.com>",
    "x <nospace@example.com>",
    "x <tabbed@example.com>",
    "x <dup@example.com>",
    "x <>",
    "x <c@example.com>",
    "x <weird@example.com>",
    "# comment <weird-old@example.com>",
    "ÉRIC <accent@example.com>",
    "éric <accent@example.com>",
    "x <crlf@example.com>",
    "x <ignored@example.com>",
    "x <unknown@example.com>",
    "x <override@example.com>",
];

/// Splits `Name <email>`.
fn split(identity: &str) -> (&str, &str) {
    let (name, rest) = identity.split_once(" <").unwrap();
    (name, rest.trim_end_matches('>'))
}

/// Compares every query with `git check-mailmap`.
fn assert_same(dir: &Path, queries: &[&str]) {
    let mut args = vec!["check-mailmap"];
    args.extend_from_slice(queries);
    let theirs: Vec<String> = git(dir, &args).lines().map(str::to_owned).collect();
    let mailmap = Repository::open(dir).unwrap().mailmap().unwrap();
    for (query, expected) in queries.iter().zip(&theirs) {
        let (name, email) = split(query);
        let (name, email) = mailmap.resolve(name, email);
        assert_eq!(&format!("{} <{}>", name, email), expected, "{}", query);
    }
    assert_eq!(theirs.len(), queries.len());
}

#[test]
fn mailmap_matches_git_check_mailmap() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, ".mailmap", MAILMAP);
    assert_same(dir, QUERIES);
    assert!(!Repository::open(dir).unwrap().mailmap().unwrap().is_empty());

    // mailmap.file is read after .mailmap and overrides it.
    write(
        dir,
        "extra/mailmap",
        "Overridden <commit@example.com>\nNew <new@example.com> <override@example.com>\n",
    );
    git(dir, &["config", "mailmap.file", "extra/mailmap"]);
    assert_same(dir, QUERIES);

    // mailmap.blob is read between them.
    git(dir, &["config", "--unset", "mailmap.file"]);
    write(
        dir,
        "blob-mailmap",
        "From Blob <dup@example.com>\nBlob <b@example.com> <c@example.com>\n",
    );
    git(dir, &["add", "blob-mailmap"]);
    git(dir, &["commit", "-q", "-m", "Mailmap blob"]);
    git(dir, &["config", "mailmap.blob", "HEAD:blob-mailmap"]);
    assert_same(dir, QUERIES);
    // A missing blob is no mailmap.
    git(dir, &["config", "mailmap.blob", "HEAD:missing"]);
    assert_same(dir, QUERIES);
}

#[test]
fn bare_repositories_read_head_mailmap() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, ".mailmap", MAILMAP);
    git(dir, &["add", ".mailmap"]);
    git(dir, &["commit", "-q", "-m", "Mailmap"]);
    let bare = tempfile::TempDir::new().unwrap();
    git(
        bare.path(),
        &["clone", "-q", "--bare", dir.to_str().unwrap(), "."],
    );
    assert_same(bare.path(), QUERIES);
    let mailmap = Repository::open(bare.path()).unwrap().mailmap().unwrap();
    assert_eq!(mailmap.resolve("x", "commit@example.com").0, "Proper Name");
}

#[cfg(unix)]
#[test]
fn a_symlinked_mailmap_is_not_followed() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    write(dir, "elsewhere", "Linked <commit@example.com>\n");
    std::os::unix::fs::symlink("elsewhere", dir.join(".mailmap")).unwrap();
    assert_same(dir, &["x <commit@example.com>"]);
    let mailmap = Repository::open(dir).unwrap().mailmap().unwrap();
    assert!(mailmap.is_empty());
}
