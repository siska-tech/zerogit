//! clone, fetch and push compared with Git, over each transport (#32).

mod common;

use common::*;
use std::path::Path;
use tempfile::TempDir;
use zerogit::{Oid, PackObjectsOptions, Repository};
use zerogit_remote::transport::{
    GitTransport, HttpConnector, LocalTransport, ProcessConnector, PushCommand, PushReply,
    RemoteRef, Transport,
};
use zerogit_remote::{
    clone_with, fetch_with, push_with, CloneOptions, PushOptions, PushStatus, UpdateKind,
};

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// zerogit reading the repository directly.
    Local,
    /// Git's upload-pack / receive-pack processes (protocol v2).
    Process,
    /// Smart HTTP through git http-backend.
    Http,
}

const KINDS: [Kind; 3] = [Kind::Local, Kind::Process, Kind::Http];

/// The URL Git uses for the same remote, and a transport to it.
fn connect(kind: Kind, root: &Path, bare: &Path) -> (String, Box<dyn Transport>) {
    match kind {
        Kind::Local => (
            bare.to_string_lossy().into_owned(),
            Box::new(LocalTransport::open(bare).unwrap()),
        ),
        Kind::Process => (
            bare.to_string_lossy().into_owned(),
            Box::new(GitTransport::new(ProcessConnector::git(
                bare.to_str().unwrap(),
            ))),
        ),
        Kind::Http => {
            let base = http_server(root);
            let url = format!("{}/origin.git", base);
            let transport = GitTransport::new(HttpConnector::new(&url).unwrap());
            (url, Box::new(transport))
        }
    }
}

fn config_of(dir: &Path) -> String {
    let output = git_output(
        dir,
        &["config", "--local", "--get-regexp", "^(remote|branch)\\."],
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn clone_matches_git_clone() {
    for kind in KINDS {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let bare = bare_origin(root);
        let (url, mut transport) = connect(kind, root, &bare);
        let ours = root.join("ours");
        let theirs = root.join("theirs");
        let repo = clone_with(&url, &ours, &CloneOptions::new(), transport.as_mut()).unwrap();
        git(root, &["clone", "-q", &url, theirs.to_str().unwrap()]);

        assert_eq!(refs(&ours), refs(&theirs), "{:?} refs", kind);
        assert_eq!(config_of(&ours), config_of(&theirs), "{:?} config", kind);
        assert_eq!(git(&ours, &["symbolic-ref", "HEAD"]), "refs/heads/main\n");
        assert_eq!(
            git(&ours, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            git(&theirs, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        );
        assert_eq!(
            worktree_files(&ours),
            worktree_files(&theirs),
            "{:?} files",
            kind
        );
        assert_eq!(git(&ours, &["status", "--porcelain"]), "");
        assert_eq!(
            git(&ours, &["reflog", "-1", "--format=%gs", "HEAD"]),
            git(&theirs, &["reflog", "-1", "--format=%gs", "HEAD"])
        );
        git(&ours, &["fsck", "--strict"]);
        assert!(repo.status().unwrap().is_empty());
    }
}

#[test]
fn clone_options_match_git() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let url = bare.to_string_lossy().into_owned();

    let ours = root.join("ours-topic");
    let theirs = root.join("theirs-topic");
    clone_with(
        &url,
        &ours,
        &CloneOptions::new().branch("topic"),
        &mut LocalTransport::open(&bare).unwrap(),
    )
    .unwrap();
    git(
        root,
        &[
            "clone",
            "-q",
            "--branch",
            "topic",
            &url,
            theirs.to_str().unwrap(),
        ],
    );
    assert_eq!(refs(&ours), refs(&theirs));
    assert_eq!(config_of(&ours), config_of(&theirs));
    assert_eq!(worktree_files(&ours), worktree_files(&theirs));

    let ours = root.join("ours.git");
    let theirs = root.join("theirs.git");
    clone_with(
        &url,
        &ours,
        &CloneOptions::new().bare(true),
        &mut LocalTransport::open(&bare).unwrap(),
    )
    .unwrap();
    git(
        root,
        &["clone", "-q", "--bare", &url, theirs.to_str().unwrap()],
    );
    assert_eq!(refs(&ours), refs(&theirs));
    assert_eq!(git(&ours, &["symbolic-ref", "HEAD"]), "refs/heads/main\n");
    git(&ours, &["fsck", "--strict"]);
}

#[test]
fn clone_of_empty_repository() {
    for kind in KINDS {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let bare = root.join("origin.git");
        git(
            root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "trunk",
                bare.to_str().unwrap(),
            ],
        );
        let (url, mut transport) = connect(kind, root, &bare);
        let ours = root.join("ours");
        clone_with(&url, &ours, &CloneOptions::new(), transport.as_mut()).unwrap();
        assert_eq!(refs(&ours), "", "{:?}", kind);
        // The unborn default branch is known locally, and over protocol v2
        // (ls-refs "unborn") for the others.
        assert_eq!(
            git(&ours, &["symbolic-ref", "HEAD"]),
            "refs/heads/trunk\n",
            "{:?}",
            kind
        );
    }
}

/// Changes on the remote after cloning: new commits, a new branch, a
/// rewound branch and new tags.
fn change_origin(root: &Path) {
    let work = root.join("work");
    write(&work, "doc.txt", "changed\n");
    git(&work, &["commit", "-q", "-am", "Change"]);
    git(&work, &["branch", "-q", "new-branch"]);
    git(&work, &["branch", "-q", "-f", "topic", "HEAD~5"]);
    git(&work, &["tag", "-a", "-m", "Two", "v2"]);
    git(
        &work,
        &[
            "push",
            "-q",
            "--force",
            "--tags",
            root.join("origin.git").to_str().unwrap(),
            "main",
            "topic",
            "new-branch",
        ],
    );
}

#[test]
fn fetch_matches_git_fetch() {
    for kind in KINDS {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let bare = bare_origin(root);
        let (url, mut transport) = connect(kind, root, &bare);
        git(root, &["clone", "-q", &url, "ours"]);
        git(root, &["clone", "-q", &url, "theirs"]);
        change_origin(root);
        let (ours, theirs) = (root.join("ours"), root.join("theirs"));

        let repo = Repository::open(&ours).unwrap();
        let refspecs = repo.remote("origin").unwrap().fetch_refspecs().to_vec();
        let outcome = fetch_with(
            &repo,
            transport.as_mut(),
            &refspecs,
            &url,
            "fetch origin",
            Some("origin"),
        )
        .unwrap();
        git(&theirs, &["fetch", "origin"]);

        assert_eq!(refs(&ours), refs(&theirs), "{:?} refs", kind);
        assert_eq!(
            std::fs::read_to_string(ours.join(".git/FETCH_HEAD")).unwrap(),
            std::fs::read_to_string(theirs.join(".git/FETCH_HEAD")).unwrap(),
            "{:?} FETCH_HEAD",
            kind
        );
        for r in ["origin/main", "origin/topic", "origin/new-branch"] {
            assert_eq!(
                git(&ours, &["reflog", "-1", "--format=%gs", r]),
                git(&theirs, &["reflog", "-1", "--format=%gs", r]),
                "{:?} {}",
                kind,
                r
            );
        }
        let kind_of = |name: &str| {
            outcome
                .updates
                .iter()
                .find(|u| u.local == name)
                .map(|u| u.kind)
        };
        assert_eq!(
            kind_of("refs/remotes/origin/main"),
            Some(UpdateKind::FastForward)
        );
        assert_eq!(
            kind_of("refs/remotes/origin/topic"),
            Some(UpdateKind::Forced)
        );
        assert_eq!(
            kind_of("refs/remotes/origin/new-branch"),
            Some(UpdateKind::New)
        );
        assert_eq!(kind_of("refs/tags/v2"), Some(UpdateKind::New));
        git(&ours, &["fsck", "--strict"]);

        // A second fetch has nothing to do.
        let again = fetch_with(
            &repo,
            transport.as_mut(),
            &refspecs,
            &url,
            "fetch origin",
            Some("origin"),
        )
        .unwrap();
        assert!(again.updates.iter().all(|u| u.kind == UpdateKind::UpToDate));
    }
}

/// A clone of the bare origin with local changes to push.
fn prepare_push(root: &Path, url: &str) {
    git(root, &["clone", "-q", url, "local"]);
    let local = root.join("local");
    write(&local, "new.txt", "pushed\n");
    git(&local, &["add", "new.txt"]);
    git(&local, &["commit", "-q", "-m", "To push"]);
    git(&local, &["branch", "-q", "feature"]);
    git(&local, &["tag", "-a", "-m", "Pushed tag", "v3"]);
    // topic diverges from the remote's topic.
    git(
        &local,
        &["checkout", "-q", "-b", "topic-local", "origin/topic"],
    );
    git(&local, &["reset", "-q", "--hard", "HEAD~1"]);
    write(&local, "diverged.txt", "x\n");
    git(&local, &["add", "diverged.txt"]);
    git(&local, &["commit", "-q", "-m", "Diverged"]);
    git(&local, &["checkout", "-q", "main"]);
}

#[test]
fn push_matches_git_push() {
    for kind in KINDS {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let bare = bare_origin(root);
        let (url, _) = connect(kind, root, &bare);
        prepare_push(root, &url);
        // Git pushes from a copy of everything, to its own origin copy.
        let twin = TempDir::new().unwrap();
        copy_dir(root, twin.path());
        let (twin_url, _) = connect(kind, twin.path(), &twin.path().join("origin.git"));
        if !matches!(kind, Kind::Http) {
            let _ = twin_url;
        }
        let git_local = twin.path().join("local");
        if matches!(kind, Kind::Http) {
            git(&git_local, &["remote", "set-url", "origin", &twin_url]);
        } else {
            git(
                &git_local,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    twin.path().join("origin.git").to_str().unwrap(),
                ],
            );
        }

        let local = root.join("local");
        let repo = Repository::open(&local).unwrap();
        let (_, mut transport) = connect(kind, root, &bare);
        let updates = push_with(
            &repo,
            "origin",
            transport.as_mut(),
            &["main", "feature", "v3", "topic-local:topic"],
            &PushOptions::new().set_upstream(true),
        )
        .unwrap();
        let _ = git_output(
            &git_local,
            &[
                "push",
                "-q",
                "-u",
                "origin",
                "main",
                "feature",
                "v3",
                "topic-local:topic",
            ],
        );

        let status = |name: &str| {
            updates
                .iter()
                .find(|u| u.remote == name)
                .unwrap()
                .status
                .clone()
        };
        assert_eq!(
            status("refs/heads/main"),
            PushStatus::FastForward,
            "{:?}",
            kind
        );
        assert_eq!(status("refs/heads/feature"), PushStatus::Created);
        assert_eq!(status("refs/tags/v3"), PushStatus::Created);
        assert_eq!(
            status("refs/heads/topic"),
            PushStatus::Rejected("non-fast-forward".into())
        );
        assert_eq!(
            refs(&bare),
            refs(&twin.path().join("origin.git")),
            "{:?} remote refs",
            kind
        );
        assert_eq!(refs(&local), refs(&git_local), "{:?} local refs", kind);
        let without_url = |config: String| -> String {
            config
                .lines()
                .filter(|l| !l.starts_with("remote.origin.url"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            without_url(config_of(&local)),
            without_url(config_of(&git_local)),
            "{:?} upstream config",
            kind
        );
        git(&bare, &["fsck", "--strict"]);

        // Forcing the rejected update, then deleting a branch.
        let (_, mut transport) = connect(kind, root, &bare);
        let updates = push_with(
            &repo,
            "origin",
            transport.as_mut(),
            &["+topic-local:topic", ":feature"],
            &PushOptions::new(),
        )
        .unwrap();
        assert_eq!(updates[0].status, PushStatus::Forced);
        assert_eq!(updates[1].status, PushStatus::Deleted);
        assert_eq!(
            git(&bare, &["rev-parse", "topic"]),
            git(&local, &["rev-parse", "topic-local"])
        );
        assert!(!git_output(
            &bare,
            &["rev-parse", "--verify", "-q", "refs/heads/feature"]
        )
        .status
        .success());
        assert!(!git_output(
            &local,
            &["rev-parse", "--verify", "-q", "refs/remotes/origin/feature"]
        )
        .status
        .success());
        git(&bare, &["fsck", "--strict"]);

        // Up to date: nothing is sent.
        let (_, mut transport) = connect(kind, root, &bare);
        let updates = push_with(
            &repo,
            "origin",
            transport.as_mut(),
            &["main"],
            &PushOptions::new(),
        )
        .unwrap();
        assert_eq!(updates[0].status, PushStatus::UpToDate);
    }
}

#[test]
fn push_to_checked_out_branch_is_refused() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let work = root.join("work");
    origin(&work);
    git(root, &["clone", "-q", work.to_str().unwrap(), "local"]);
    let local = root.join("local");
    write(&local, "x.txt", "x\n");
    git(&local, &["add", "x.txt"]);
    git(&local, &["commit", "-q", "-m", "X"]);
    let repo = Repository::open(&local).unwrap();
    let updates = push_with(
        &repo,
        "origin",
        &mut LocalTransport::open(&work).unwrap(),
        &["main"],
        &PushOptions::new(),
    )
    .unwrap();
    assert!(matches!(updates[0].status, PushStatus::RemoteRejected(_)));
    let updates = push_with(
        &repo,
        "origin",
        &mut GitTransport::new(ProcessConnector::git(work.to_str().unwrap())),
        &["main"],
        &PushOptions::new(),
    )
    .unwrap();
    assert!(matches!(updates[0].status, PushStatus::RemoteRejected(_)));
}

#[test]
fn original_protocol_clone_and_fetch_match_git() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let bare = bare_origin(root);
    let url = bare.to_string_lossy().into_owned();
    let v0 = || GitTransport::new(ProcessConnector::git(&url).protocol_v2(false));
    let ours = root.join("ours");
    clone_with(&url, &ours, &CloneOptions::new(), &mut v0()).unwrap();
    git(root, &["clone", "-q", &url, "theirs"]);
    let theirs = root.join("theirs");
    assert_eq!(refs(&ours), refs(&theirs));
    assert_eq!(worktree_files(&ours), worktree_files(&theirs));

    change_origin(root);
    let repo = Repository::open(&ours).unwrap();
    let refspecs = repo.remote("origin").unwrap().fetch_refspecs().to_vec();
    fetch_with(
        &repo,
        &mut v0(),
        &refspecs,
        &url,
        "fetch origin",
        Some("origin"),
    )
    .unwrap();
    git(&theirs, &["fetch", "origin"]);
    assert_eq!(refs(&ours), refs(&theirs));
    git(&ours, &["fsck", "--strict"]);
}

/// Forwards to another transport and records the size of each pack
/// pushed.
struct Recording {
    inner: Box<dyn Transport>,
    sent: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
}

impl Transport for Recording {
    fn list_refs(&mut self, prefixes: &[String]) -> zerogit_remote::Result<Vec<RemoteRef>> {
        self.inner.list_refs(prefixes)
    }

    fn fetch_pack(&mut self, wants: &[Oid], haves: &[Oid]) -> zerogit_remote::Result<Vec<u8>> {
        self.inner.fetch_pack(wants, haves)
    }

    fn list_push_refs(&mut self) -> zerogit_remote::Result<Vec<RemoteRef>> {
        self.inner.list_push_refs()
    }

    fn push(
        &mut self,
        commands: &[PushCommand],
        pack: &[u8],
    ) -> zerogit_remote::Result<Vec<PushReply>> {
        self.sent.borrow_mut().push(pack.len());
        self.inner.push(commands, pack)
    }

    fn push_pack_options(&mut self) -> zerogit_remote::Result<PackObjectsOptions> {
        self.inner.push_pack_options()
    }
}

/// Text that compresses poorly, so that only deltas make it small.
fn noise(seed: u64, lines: usize) -> String {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    (0..lines)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            format!("{:016x} {:016x}\n", state, state.rotate_left(17))
        })
        .collect()
}

#[test]
fn push_sends_deltas_against_what_the_remote_has() {
    for kind in KINDS {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let bare = bare_origin(root);
        let (url, _) = connect(kind, root, &bare);
        git(root, &["clone", "-q", &url, "local"]);
        let local = root.join("local");
        // A large file the remote gets first, then a small change to it.
        let mut content = noise(1, 6000);
        write(&local, "large.txt", &content);
        git(&local, &["add", "large.txt"]);
        git(&local, &["commit", "-q", "-m", "Large"]);
        let repo = Repository::open(&local).unwrap();
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let push = |message: &str| {
            let (_, inner) = connect(kind, root, &bare);
            let mut transport = Recording {
                inner,
                sent: sent.clone(),
            };
            let updates = push_with(
                &repo,
                "origin",
                &mut transport,
                &["main"],
                &PushOptions::new(),
            )
            .unwrap();
            assert_eq!(
                updates[0].status,
                PushStatus::FastForward,
                "{} {:?}",
                message,
                kind
            );
            git(&bare, &["fsck", "--strict"]);
            assert_eq!(
                git(&bare, &["rev-parse", "main"]),
                git(&local, &["rev-parse", "main"])
            );
        };
        push("large");
        content.push_str(&noise(2, 10));
        write(&local, "large.txt", &content);
        git(&local, &["commit", "-q", "-am", "Change"]);
        push("change");

        let sent = sent.borrow();
        // The first push carries the file (about 200 KB, compressing
        // poorly); the second only a delta against it.
        assert!(sent[0] > 100_000, "{:?} {:?}", kind, sent);
        assert!(sent[1] < 3_000, "{:?} {:?}", kind, sent);
    }
}
