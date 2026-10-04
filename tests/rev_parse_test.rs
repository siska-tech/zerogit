//! `Repository::rev_parse` resolves revisions exactly as `git rev-parse`.
//!
//! Git is used only to build fixtures and to produce expected values.

mod common;

use std::path::Path;

use common::{conflicted_repository, git, git_output, repository, write};
use zerogit::{Error, Repository};

/// A repository with history, a merge, tags, remote-tracking branches,
/// upstream configuration, packed references and checkout history.
fn history() -> tempfile::TempDir {
    let temp = repository(&["README.md", "src/lib.rs", "src/deep/mod.rs"]);
    let dir = temp.path();
    for (i, file) in ["a.txt", "b.txt"].iter().enumerate() {
        write(dir, file, &format!("{}\n", i));
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Commit {}", i)]);
    }
    git(dir, &["tag", "v1", "HEAD~2"]);
    git(dir, &["tag", "-a", "v2", "-m", "Annotated", "HEAD~1"]);
    git(dir, &["tag", "-a", "nested", "-m", "Tag of a tag", "v2"]);
    git(dir, &["tag", "tree-tag", "HEAD^{tree}"]);

    // A topic branch merged back with --no-ff (a second parent).
    git(dir, &["checkout", "-q", "-b", "topic", "HEAD~1"]);
    write(dir, "topic.txt", "topic\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Topic"]);
    git(dir, &["checkout", "-q", "main"]);
    git(
        dir,
        &["merge", "-q", "--no-ff", "-m", "Merge topic", "topic"],
    );

    // A branch and a tag with the same name: the tag wins, as in Git.
    git(dir, &["branch", "dup", "HEAD~1"]);
    git(dir, &["tag", "dup", "v1"]);

    // Remote-tracking branches, origin/HEAD and upstreams.
    git(
        dir,
        &["remote", "add", "origin", "https://example.com/repo.git"],
    );
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
    git(dir, &["update-ref", "refs/remotes/origin/feature", "v1"]);
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    git(dir, &["config", "branch.main.remote", "origin"]);
    git(dir, &["config", "branch.main.merge", "refs/heads/main"]);
    git(dir, &["config", "branch.topic.remote", "."]);
    git(dir, &["config", "branch.topic.merge", "refs/heads/main"]);

    // Some references only in packed-refs.
    git(dir, &["pack-refs", "--all"]);
    git(dir, &["branch", "loose", "HEAD~2"]);

    // Checkout history for @{-<n>}: main -> topic -> loose -> main.
    git(dir, &["checkout", "-q", "topic"]);
    git(dir, &["checkout", "-q", "loose"]);
    git(dir, &["checkout", "-q", "main"]);
    // Move main once more so main@{1} differs from main.
    write(dir, "c.txt", "c\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "After"]);
    temp
}

fn git_rev_parse(dir: &Path, revision: &str) -> Option<String> {
    let output = git_output(dir, &["rev-parse", "--verify", "-q", revision]);
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn assert_same(dir: &Path, revisions: &[&str]) {
    let repo = Repository::open(dir).unwrap();
    for revision in revisions {
        let expected = git_rev_parse(dir, revision)
            .unwrap_or_else(|| panic!("git cannot resolve {}", revision));
        let actual = repo
            .rev_parse(revision)
            .unwrap_or_else(|e| panic!("{}: {}", revision, e));
        assert_eq!(actual.to_hex(), expected, "{}", revision);
    }
}

#[test]
fn names_resolve_in_git_order() {
    let temp = history();
    let dir = temp.path();
    let head = git(dir, &["rev-parse", "HEAD"]);
    assert_same(
        dir,
        &[
            "HEAD",
            "@",
            "main",
            "refs/heads/main",
            "heads/main",
            "topic",
            "loose",
            "v1",
            "v2",
            "tags/v2",
            "nested",
            "tree-tag",
            "dup",
            "origin/main",
            "remotes/origin/feature",
            "origin",
            "ORIG_HEAD",
            &head.trim()[..7],
            head.trim(),
        ],
    );
}

#[test]
fn ancestry_and_peeling_match_git() {
    let temp = history();
    let dir = temp.path();
    assert_same(
        dir,
        &[
            "HEAD~",
            "HEAD~1",
            "HEAD~3",
            "HEAD^",
            "HEAD^^",
            "HEAD~1^2",
            "HEAD~1^2~1",
            "HEAD~1^1^",
            "main^0",
            "v2^0",
            "v2~1",
            "v2^{}",
            "nested^{}",
            "nested^{tag}",
            "nested^{commit}",
            "v2^{tree}",
            "HEAD^{tree}",
            "HEAD^{commit}",
            "HEAD^{object}",
            "tree-tag^{tree}",
            "@~2",
        ],
    );
}

#[test]
fn paths_in_trees_and_the_index_match_git() {
    let temp = history();
    let dir = temp.path();
    assert_same(
        dir,
        &[
            "HEAD:README.md",
            "HEAD:src",
            "HEAD:src/deep/mod.rs",
            "main~3:src/lib.rs",
            "v2:a.txt",
            "HEAD:",
            ":README.md",
            ":0:src/lib.rs",
            "tree-tag:src",
        ],
    );

    let temp = conflicted_repository();
    assert_same(temp.path(), &[":1:file.txt", ":2:file.txt", ":3:file.txt"]);
}

#[test]
fn reflogs_previous_checkouts_and_upstreams_match_git() {
    let temp = history();
    let dir = temp.path();
    assert_same(
        dir,
        &[
            "main@{0}",
            "main@{1}",
            "@{1}",
            "HEAD@{1}",
            "HEAD@{3}",
            "@{-1}",
            "@{-2}",
            "@{-3}",
            "@{u}",
            "@{upstream}",
            "main@{U}",
            "HEAD@{u}",
            "topic@{u}",
            "@{u}~1",
            "main@{1}~1",
        ],
    );
}

#[test]
fn unresolvable_revisions_name_the_problem() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let cases = [
        ("nosuch", "unknown revision 'nosuch'"),
        ("HEAD~100", "has no parent"),
        ("HEAD~1^3", "has 2 parent(s)"),
        (
            "HEAD:nosuch.txt",
            "path 'nosuch.txt' does not exist in 'HEAD'",
        ),
        ("HEAD:README.md/x", "'README.md' is not a directory"),
        (":nosuch.txt", "not in the index"),
        ("v1^{tag}", "cannot be peeled to a tag"),
        ("HEAD^{tree}~1", "cannot be peeled to a commit"),
        ("HEAD^{bogus}", "unknown object type 'bogus'"),
        ("@{-99}", "fewer than 99 checkouts"),
        ("main@{99}", "has only"),
        ("loose@{u}", "no upstream is configured for branch 'loose'"),
        ("v1@{u}", "no such branch 'v1'"),
        ("HEAD~1@{1}", "unexpected"),
        ("HEAD^{tree", "missing '}'"),
        ("", "empty revision"),
    ];
    for (revision, reason_part) in cases {
        assert!(
            git_rev_parse(dir, revision).is_none(),
            "git resolves {}",
            revision
        );
        match repo.rev_parse(revision) {
            Err(Error::InvalidRevision {
                revision: r,
                reason,
            }) => {
                assert_eq!(r, revision);
                assert!(
                    reason.contains(reason_part),
                    "{}: {:?} does not mention {:?}",
                    revision,
                    reason,
                    reason_part
                );
            }
            other => panic!("{}: expected InvalidRevision, got {:?}", revision, other),
        }
    }
}

#[test]
fn unsupported_syntax_is_reported_as_such() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    // Git resolves these; zerogit says clearly that it does not.
    for (revision, reason_part) in [
        ("main@{yesterday}", "not dates"),
        ("main@{push}", "not supported"),
        ("HEAD^{/Topic}", "not supported"),
        (":/Topic", "not supported"),
    ] {
        match repo.rev_parse(revision) {
            Err(Error::InvalidRevision { reason, .. }) => {
                assert!(reason.contains(reason_part), "{}: {}", revision, reason)
            }
            other => panic!("{}: expected InvalidRevision, got {:?}", revision, other),
        }
    }
}

#[test]
fn abbreviated_oids_keep_their_errors() {
    let temp = history();
    let repo = Repository::open(temp.path()).unwrap();
    assert!(matches!(
        repo.rev_parse("0000000"),
        Err(Error::ObjectNotFound(_))
    ));
    assert!(matches!(repo.rev_parse("abc"), Err(Error::InvalidOid(_))));
}

#[test]
fn object_accessors_accept_revisions() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    let head = git(dir, &["rev-parse", "HEAD~1"]);
    assert_eq!(repo.commit("HEAD~1").unwrap().oid().to_hex(), head.trim());
    assert_eq!(repo.commit("v2^{}").unwrap().summary(), "Commit 0");
    assert_eq!(
        repo.blob("HEAD:README.md").unwrap().content(),
        b"content of README.md\n"
    );
    assert!(repo.tree("HEAD^{tree}").is_ok());
    // Existing short OID calls work as before.
    assert_eq!(
        repo.commit(&head.trim()[..7]).unwrap().oid().to_hex(),
        head.trim()
    );
}

#[test]
fn checkout_accepts_revisions_and_previous_branches() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();

    repo.checkout("HEAD~1").unwrap();
    assert_eq!(
        git(dir, &["rev-parse", "HEAD"]),
        git(dir, &["rev-parse", "main~1"])
    );
    assert!(git_output(dir, &["symbolic-ref", "-q", "HEAD"])
        .stdout
        .is_empty());

    // `-` returns to the branch, attached, as `git checkout -` does.
    repo.checkout("-").unwrap();
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main\n");
    repo.checkout("topic").unwrap();
    repo.checkout("@{-1}").unwrap();
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main\n");
    assert_eq!(
        git(dir, &["reflog", "-1", "--format=%gs"]),
        "checkout: moving from topic to main\n"
    );

    assert!(matches!(
        repo.checkout("nosuch"),
        Err(Error::RefNotFound(name)) if name == "nosuch"
    ));
}

#[test]
fn merge_and_rebase_accept_revisions() {
    let temp = history();
    let dir = temp.path();
    let repo = Repository::open(dir).unwrap();
    git(dir, &["checkout", "-q", "-b", "side", "main~2"]);
    let repo_head = || git(dir, &["rev-parse", "HEAD"]);
    repo.merge(
        "main~1",
        "T",
        "t@example.com",
        &zerogit::MergeOptions::new(),
    )
    .unwrap();
    assert_eq!(repo_head(), git(dir, &["rev-parse", "main~1"]));
    // A remote-tracking branch found by Git's lookup order.
    git(dir, &["checkout", "-q", "-b", "other", "v1"]);
    let outcome = repo
        .merge(
            "origin/main",
            "T",
            "t@example.com",
            &zerogit::MergeOptions::new(),
        )
        .unwrap();
    assert!(matches!(outcome, zerogit::MergeOutcome::FastForward(_)));
}
