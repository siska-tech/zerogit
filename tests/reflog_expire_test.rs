//! Expiring reflog entries compared with Git: `Repository::reflog_expire`
//! with `git reflog expire --all`, and `Repository::gc` with `git gc` (#74).

mod common;

use common::*;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zerogit::{Error, ReflogExpiry, Repository};

const DAY: i64 = 24 * 3600;
const ZERO: &str = "0000000000000000000000000000000000000000";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn rev(dir: &Path, name: &str) -> String {
    git(dir, &["rev-parse", name]).trim().to_owned()
}

/// Replaces the reflog of `refname` with entries for every pair of values
/// at every age (in days; negative is in the future).
fn write_log(dir: &Path, refname: &str, pairs: &[(&str, &str)], ages: &[i64]) {
    let mut content = String::new();
    let now = now();
    for (i, age) in ages.iter().enumerate() {
        for (j, (old, new)) in pairs.iter().enumerate() {
            content.push_str(&format!(
                "{} {} Test <test@example.com> {} +0900\tentry {}.{}\n",
                old,
                new,
                now - age * DAY,
                i,
                j
            ));
        }
    }
    write(dir, &format!(".git/logs/{}", refname), &content);
}

/// Every reflog with its content.
fn logs(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path.strip_prefix(root).unwrap().to_string_lossy();
                out.push((name.replace('\\', "/"), fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let root = dir.join(".git/logs");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    out
}

fn line_count(logs: &[(String, String)]) -> usize {
    logs.iter()
        .map(|(_, content)| content.lines().count())
        .sum()
}

/// A repository whose reflogs mix entries old and new, of commits reachable
/// and not, of other objects and of missing ones, for branches, HEAD, the
/// stash, a tag, a reference to a blob and a deleted branch.
fn mixed_reflogs() -> tempfile::TempDir {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    let mut commits = vec![rev(dir, "HEAD")];
    for i in 2..=4 {
        write(dir, "a.txt", &format!("{}\n", i));
        git(dir, &["commit", "-q", "-am", &format!("C{}", i)]);
        commits.push(rev(dir, "HEAD"));
    }
    // Rewritten away: reachable from no reference.
    write(dir, "a.txt", "x\n");
    git(dir, &["commit", "-q", "-am", "X"]);
    let x = rev(dir, "HEAD");
    git(dir, &["reset", "-q", "--hard", "HEAD~1"]);
    fs::remove_file(dir.join(".git/ORIG_HEAD")).unwrap();
    git(dir, &["branch", "topic", &commits[1]]);
    git(dir, &["tag", "-a", "v1", "-m", "v1", &commits[1]]);
    git(dir, &["tag", "-a", "vx", "-m", "vx", &x]);
    let tag_x = rev(dir, "vx");
    git(dir, &["tag", "-d", "vx"]);
    write(dir, "blob.txt", "a blob\n");
    let blob = git(dir, &["hash-object", "-w", "blob.txt"]);
    let blob = blob.trim();
    fs::remove_file(dir.join("blob.txt")).unwrap();
    git(dir, &["update-ref", "refs/blobref", blob]);
    write(dir, "a.txt", "stashed\n");
    git(dir, &["stash", "-q"]);

    let c = &commits;
    let missing = "1234567890123456789012345678901234567890";
    let pairs = [
        (ZERO, c[0].as_str()),
        (c[0].as_str(), c[1].as_str()),
        (c[1].as_str(), x.as_str()),
        (x.as_str(), c[2].as_str()),
        (c[2].as_str(), blob),
        (c[2].as_str(), missing),
        (c[3].as_str(), tag_x.as_str()),
        (c[3].as_str(), ZERO),
    ];
    let ages = [100, 60, 10, -1];
    for refname in [
        "HEAD",
        "refs/heads/main",
        "refs/heads/topic",
        "refs/heads/gone/x",
        "refs/stash",
        "refs/tags/v1",
        "refs/blobref",
    ] {
        write_log(dir, refname, &pairs, &ages);
    }
    temp
}

/// Git 2.55 swaps the documented defaults of `gc.reflogExpire` (90 days)
/// and `gc.reflogExpireUnreachable` (30 days); zerogit keeps the documented
/// ones, so Git is given them explicitly.
const DOCUMENTED_DEFAULTS: &[&str] = &[
    "gc.reflogExpire=90.days.ago",
    "gc.reflogExpireUnreachable=30.days.ago",
];

/// Expires with zerogit and with `git -c <config>... reflog expire --all
/// <args>` in two copies of `source` and compares the reflogs.
fn compare(
    source: &Path,
    expire: Option<ReflogExpiry>,
    expire_unreachable: Option<ReflogExpiry>,
    config: &[&str],
    args: &[&str],
) -> Vec<(String, String)> {
    let ours = twin(source);
    let theirs = twin(source);
    let before = line_count(&logs(source));
    let removed = Repository::open(ours.path())
        .unwrap()
        .reflog_expire(expire, expire_unreachable)
        .unwrap();
    let mut command = Vec::new();
    for setting in config {
        command.extend(["-c", setting]);
    }
    command.extend(["reflog", "expire", "--all"]);
    command.extend_from_slice(args);
    git(theirs.path(), &command);
    let result = logs(ours.path());
    let expected = logs(theirs.path());
    // Compared by message first, which names each entry.
    for ((name, ours), (_, theirs)) in result.iter().zip(&expected) {
        let entries = |content: &str| -> Vec<String> {
            content
                .lines()
                .map(|line| line.rsplit('\t').next().unwrap().to_owned())
                .collect()
        };
        assert_eq!(entries(ours), entries(theirs), "{} {:?}", name, args);
    }
    assert_eq!(result, expected, "{:?}", args);
    assert_eq!(removed, before - line_count(&result));
    result
}

fn at(seconds: i64) -> (ReflogExpiry, String) {
    (
        ReflogExpiry::Before(UNIX_EPOCH + Duration::from_secs(seconds as u64)),
        format!("@{}", seconds),
    )
}

#[test]
fn expire_matches_git_reflog_expire() {
    let temp = mixed_reflogs();
    let dir = temp.path();

    // The defaults: 90 days, 30 for unreachable entries; never for the stash.
    let result = compare(dir, None, None, DOCUMENTED_DEFAULTS, &[]);
    let stash = &result
        .iter()
        .find(|(name, _)| name == "refs/stash")
        .unwrap()
        .1;
    assert_eq!(stash.lines().count(), 32);
    let main = &result
        .iter()
        .find(|(name, _)| name == "refs/heads/main")
        .unwrap()
        .1;
    assert!(main.lines().count() < 32 && main.lines().count() > 8);

    // Explicit times (the stash expires too).
    let (expire, expire_arg) = at(now() - 80 * DAY);
    let (unreachable, unreachable_arg) = at(now() - 5 * DAY);
    compare(
        dir,
        Some(expire),
        Some(unreachable),
        &[],
        &[
            &format!("--expire={}", expire_arg),
            &format!("--expire-unreachable={}", unreachable_arg),
        ],
    );

    // `now` takes every unreachable entry, the future ones too.
    compare(
        dir,
        Some(ReflogExpiry::Never),
        Some(ReflogExpiry::All),
        &[],
        &["--expire=never", "--expire-unreachable=now"],
    );
    // And everything.
    let result = compare(
        dir,
        Some(ReflogExpiry::All),
        Some(ReflogExpiry::All),
        &[],
        &["--expire=all", "--expire-unreachable=all"],
    );
    // Emptied reflogs are kept as empty files.
    assert_eq!(result.len(), 7);
    assert!(result.iter().all(|(_, content)| content.is_empty()));
}

#[test]
fn expire_follows_the_configuration_like_git() {
    let temp = mixed_reflogs();
    let dir = temp.path();
    git(dir, &["config", "gc.reflogExpire", "50.days.ago"]);
    git(
        dir,
        &["config", "gc.reflogExpireUnreachable", "20 days ago"],
    );
    // `*` matches `/`; the first matching pattern wins, and a key it does
    // not set never expires.
    git(
        dir,
        &["config", "gc.refs/heads/*.reflogExpireUnreachable", "now"],
    );
    git(dir, &["config", "gc.refs/heads/t*.reflogExpire", "now"]);
    git(
        dir,
        &["config", "gc.refs/st?sh.reflogExpire", "70.days.ago"],
    );
    git(dir, &["config", "gc.HEAD.reflogExpire", "never"]);
    compare(dir, None, None, &[], &[]);

    // An explicit expiry replaces only its own setting.
    compare(
        dir,
        Some(ReflogExpiry::Never),
        None,
        &[],
        &["--expire=never"],
    );
    let (unreachable, arg) = at(now() - 45 * DAY);
    compare(
        dir,
        None,
        Some(unreachable),
        &[],
        &[&format!("--expire-unreachable={}", arg)],
    );

    git(dir, &["config", "gc.reflogExpire", "someday"]);
    assert!(matches!(
        Repository::open(dir).unwrap().reflog_expire(None, None),
        Err(Error::InvalidDate(_))
    ));
}

#[test]
fn locked_references_change_nothing() {
    let temp = mixed_reflogs();
    let dir = temp.path();
    let before = logs(dir);
    let repo = Repository::open(dir).unwrap();
    for lock in [
        ".git/refs/heads/topic.lock",
        ".git/logs/refs/heads/main.lock",
    ] {
        write(dir, lock, "");
        assert!(matches!(
            repo.reflog_expire(Some(ReflogExpiry::All), Some(ReflogExpiry::All)),
            Err(Error::Locked(_))
        ));
        fs::remove_file(dir.join(lock)).unwrap();
        assert_eq!(logs(dir), before);
        assert!(!dir.join(".git/logs/HEAD.lock").exists());
        assert!(!dir.join(".git/HEAD.lock").exists());
    }
    // gc stops before touching anything else.
    write(dir, ".git/refs/heads/topic.lock", "");
    assert!(matches!(repo.gc(), Err(Error::Locked(_))));
    assert_eq!(logs(dir), before);
}

/// Every loose object and packed object.
fn objects(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = git(dir, &["cat-file", "--batch-all-objects", "--batch-check"])
        .lines()
        .map(str::to_owned)
        .collect();
    out.sort();
    out
}

#[test]
fn gc_prunes_what_only_expired_entries_reached() {
    let temp = repository(&["a.txt"]);
    let dir = temp.path();
    git(dir, &["config", "gc.pruneExpire", "now"]);
    write(dir, "a.txt", "rewritten\n");
    git(dir, &["commit", "-q", "-am", "Rewritten"]);
    let gone = rev(dir, "HEAD");
    git(dir, &["reset", "-q", "--hard", "HEAD~1"]);
    fs::remove_file(dir.join(".git/ORIG_HEAD")).unwrap();
    // The entries about the rewritten commit are 40 days old (unreachable
    // ones expire after 30); the others are recent.
    for refname in ["HEAD", "refs/heads/main"] {
        let path = dir.join(".git/logs").join(refname);
        let content: String = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                if line.contains(&gone) {
                    let old = (now() - 40 * DAY).to_string();
                    let (head, message) = line.split_once('\t').unwrap();
                    let mut words: Vec<&str> = head.split(' ').collect();
                    let n = words.len();
                    words[n - 2] = &old;
                    format!("{}\t{}\n", words.join(" "), message)
                } else {
                    format!("{}\n", line)
                }
            })
            .collect();
        fs::write(&path, content).unwrap();
    }
    let ours = twin(dir);
    let theirs = twin(dir);
    let (o, t) = (ours.path(), theirs.path());
    let summary = Repository::open(o).unwrap().gc().unwrap();
    git(t, &["gc", "-q"]);

    assert_eq!(logs(o), logs(t));
    assert_eq!(objects(o), objects(t));
    assert!(!objects(o).iter().any(|line| line.starts_with(&gone)));
    // The commit, its tree and the new blob.
    assert_eq!(summary.pruned(), 3);
    assert_fsck(o);
}
