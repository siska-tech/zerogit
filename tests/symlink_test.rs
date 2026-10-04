//! Symbolic links staged, compared and checked out like Git (#22).

mod common;

use common::*;
use std::fs;
use zerogit::Repository;

/// Commits a symlink entry `link -> target.txt` on a new branch `with-link`
/// without creating a link on disk, then returns to `main`.
fn repository_with_link_branch() -> tempfile::TempDir {
    let temp = repository(&["target.txt"]);
    let dir = temp.path();
    git(dir, &["checkout", "-q", "-b", "with-link"]);
    let oid = git_with_stdin(dir, &["hash-object", "-w", "--stdin"], b"target.txt");
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{},link", oid.trim()),
        ],
    );
    git(dir, &["commit", "-q", "-m", "Add link"]);
    git(dir, &["checkout", "-q", "-f", "main"]);
    temp
}

fn git_with_stdin(dir: &std::path::Path, args: &[&str], input: &[u8]) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn checkout_with_symlinks_disabled_writes_target_as_file() {
    let temp = repository_with_link_branch();
    let dir = temp.path();
    git(dir, &["config", "core.symlinks", "false"]);
    let repo = Repository::open(dir).unwrap();
    repo.checkout("with-link").unwrap();

    let metadata = fs::symlink_metadata(dir.join("link")).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(fs::read(dir.join("link")).unwrap(), b"target.txt");
    // The entry keeps its symlink mode and the tree is clean for both tools.
    assert!(git_ls_files(dir).iter().any(|l| l.starts_with("120000")));
    assert!(repo.status().unwrap().is_empty());
    assert_eq!(git(dir, &["status", "--porcelain"]), "");

    // Staging it again keeps the symlink mode, as git add does.
    repo.add("link").unwrap();
    assert!(git_ls_files(dir).iter().any(|l| l.starts_with("120000")));
    assert_eq!(git(dir, &["status", "--porcelain"]), "");
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::symlink;

    /// `git ls-files -s` for `path` after staging it with zerogit and with Git.
    fn staged_by_both(dir: &std::path::Path, path: &str) -> (Vec<String>, Vec<String>) {
        let repo = Repository::open(dir).unwrap();
        repo.add(path).unwrap();
        let ours = git_ls_files(dir);
        git(dir, &["rm", "-q", "--cached", path]);
        git(dir, &["add", path]);
        (ours, git_ls_files(dir))
    }

    #[test]
    fn add_symlink_matches_git() {
        let temp = repository(&["target.txt"]);
        let dir = temp.path();
        symlink("target.txt", dir.join("link")).unwrap();
        let (ours, theirs) = staged_by_both(dir, "link");
        assert_eq!(ours, theirs);
        assert!(ours
            .iter()
            .any(|l| l.starts_with("120000") && l.ends_with("\tlink")));
    }

    #[test]
    fn dangling_symlink_can_be_added() {
        let temp = repository(&["target.txt"]);
        let dir = temp.path();
        symlink("missing/file", dir.join("dangling")).unwrap();
        let (ours, theirs) = staged_by_both(dir, "dangling");
        assert_eq!(ours, theirs);
    }

    #[test]
    fn directory_symlinks_are_not_followed() {
        let temp = repository(&["dir/file.txt"]);
        let dir = temp.path();
        symlink("dir", dir.join("dirlink")).unwrap();
        // A link back up the tree must not make the walk loop.
        symlink("..", dir.join("dir/up")).unwrap();
        symlink("/", dir.join("root")).unwrap();

        let repo = Repository::open(dir).unwrap();
        let mut untracked: Vec<String> = repo
            .status()
            .unwrap()
            .into_iter()
            .filter(|e| e.status() == FileStatus::Untracked)
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();
        untracked.sort();
        assert_eq!(untracked, ["dir/up", "dirlink", "root"]);

        repo.add_all().unwrap();
        let ours = git_ls_files(dir);
        git(dir, &["read-tree", "HEAD"]);
        git(dir, &["add", "-A"]);
        assert_eq!(ours, git_ls_files(dir));
    }

    #[test]
    fn tracked_symlink_is_clean_until_retargeted() {
        let temp = repository(&["a.txt", "b.txt"]);
        let dir = temp.path();
        symlink("a.txt", dir.join("link")).unwrap();
        git(dir, &["add", "link"]);
        git(dir, &["commit", "-q", "-m", "Link"]);

        let repo = Repository::open(dir).unwrap();
        assert!(repo.status().unwrap().is_empty());
        // Changing the target file's content does not change the link.
        write(dir, "a.txt", "changed\n");
        let status = repo.status().unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].path(), std::path::Path::new("a.txt"));

        fs::remove_file(dir.join("link")).unwrap();
        symlink("b.txt", dir.join("link")).unwrap();
        let status = repo.status().unwrap();
        assert!(status.iter().any(
            |e| e.path() == std::path::Path::new("link") && e.status() == FileStatus::Modified
        ));
        let diff = repo.diff_index_to_workdir().unwrap();
        assert!(diff
            .deltas()
            .iter()
            .any(|d| d.path() == std::path::Path::new("link")));
    }

    #[test]
    fn checkout_creates_symlinks() {
        let temp = repository_with_link_branch();
        let dir = temp.path();
        let repo = Repository::open(dir).unwrap();
        repo.checkout("with-link").unwrap();

        let metadata = fs::symlink_metadata(dir.join("link")).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_link(dir.join("link")).unwrap(),
            std::path::PathBuf::from("target.txt")
        );
        assert!(repo.status().unwrap().is_empty());
        assert_eq!(git(dir, &["status", "--porcelain"]), "");

        // Switching back removes the link.
        repo.checkout("main").unwrap();
        assert!(fs::symlink_metadata(dir.join("link")).is_err());
        assert_eq!(git(dir, &["status", "--porcelain"]), "");
    }

    #[test]
    fn checkout_replaces_file_with_symlink_and_back() {
        let temp = repository(&["target.txt"]);
        let dir = temp.path();
        write(dir, "entry", "a regular file\n");
        git(dir, &["add", "entry"]);
        git(dir, &["commit", "-q", "-m", "File"]);
        git(dir, &["checkout", "-q", "-b", "as-link"]);
        fs::remove_file(dir.join("entry")).unwrap();
        symlink("target.txt", dir.join("entry")).unwrap();
        git(dir, &["add", "entry"]);
        git(dir, &["commit", "-q", "-m", "Link"]);
        git(dir, &["checkout", "-q", "main"]);

        let repo = Repository::open(dir).unwrap();
        repo.checkout("as-link").unwrap();
        assert!(fs::symlink_metadata(dir.join("entry"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(git(dir, &["status", "--porcelain"]), "");
        repo.checkout("main").unwrap();
        assert!(fs::symlink_metadata(dir.join("entry"))
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(fs::read(dir.join("entry")).unwrap(), b"a regular file\n");
        assert_eq!(git(dir, &["status", "--porcelain"]), "");
    }

    #[test]
    fn executable_bit_changes_are_reported_and_staged() {
        use std::os::unix::fs::PermissionsExt;
        let temp = repository(&["run.sh"]);
        let dir = temp.path();
        fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        let repo = Repository::open(dir).unwrap();
        let expected = git(dir, &["status", "--porcelain"]);
        assert_eq!(expected, " M run.sh\n");
        assert_eq!(repo.status().unwrap().len(), 1);
        let (ours, theirs) = staged_by_both(dir, "run.sh");
        assert_eq!(ours, theirs);
    }
}
