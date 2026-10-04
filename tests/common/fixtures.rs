//! Named fixture repositories shared by read-only tests.
//!
//! Each fixture is built with Git on first use under `tests/fixtures/<name>`
//! (ignored by Git), so `cargo test` works on a clean clone without running a
//! script first. Authors, dates and configuration are fixed, so a fixture has
//! the same object IDs on every machine and OS.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::Mutex;

/// Serializes fixture creation between the tests of one test binary.
static LOCK: Mutex<()> = Mutex::new(());

/// Returns the path of the fixture `name`, creating it if it does not exist.
///
/// | Name       | Contents                                                  |
/// |------------|-----------------------------------------------------------|
/// | `simple`   | Two commits                                               |
/// | `empty`    | No commits                                                |
/// | `branches` | `main` and `feature`                                      |
/// | `remotes`  | Remote-tracking refs (`origin`, `upstream`, nested names) |
/// | `tags`     | A lightweight tag and an annotated tag                    |
/// | `diff`     | Additions, deletions and changes, including nested paths  |
/// | `rename`   | An exact rename                                           |
/// | `merge`    | A `--no-ff` merge commit                                  |
pub fn fixture(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let path = root.join(name);
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if path.join(".git").is_dir() {
        return path;
    }

    // Build in a private directory and move it into place, so a concurrently
    // running test binary never sees a half-built fixture.
    let building = root.join(format!(".{}-{}", name, process::id()));
    let _ = fs::remove_dir_all(&building);
    fs::create_dir_all(&building).unwrap();
    build(name, &building);
    if fs::rename(&building, &path).is_err() {
        // Another process finished first.
        let _ = fs::remove_dir_all(&building);
        assert!(
            path.join(".git").is_dir(),
            "fixture {} was not created",
            name
        );
    }
    path
}

fn build(name: &str, dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test User"]);

    match name {
        "simple" => {
            write(dir, "README.md", "Hello\n");
            commit(dir, "Initial commit");
            write(dir, "README.md", "Hello\nWorld\n");
            commit(dir, "Second commit");
        }
        "empty" => {}
        "branches" => {
            write(dir, "file.txt", "main\n");
            commit(dir, "Main commit");
            git(dir, &["checkout", "-q", "-b", "feature"]);
            write(dir, "feature.txt", "feature\n");
            commit(dir, "Feature commit");
            git(dir, &["checkout", "-q", "main"]);
        }
        "remotes" => {
            write(dir, "file.txt", "main\n");
            commit(dir, "Initial commit");
            let oid = git(dir, &["rev-parse", "HEAD"]);
            for name in [
                "refs/remotes/origin/main",
                "refs/remotes/origin/develop",
                "refs/remotes/origin/feature/xyz",
                "refs/remotes/upstream/main",
            ] {
                git(dir, &["update-ref", name, oid.trim()]);
            }
        }
        "tags" => {
            write(dir, "file.txt", "v1\n");
            commit(dir, "Version 1");
            git(dir, &["tag", "v1.0.0"]);
            git(dir, &["tag", "-a", "v1.0.1", "-m", "Annotated tag"]);
        }
        "diff" => {
            write(dir, "file1.txt", "initial\n");
            write(dir, "file2.txt", "to-delete\n");
            write(dir, "src/main.rs", "fn main() {}\n");
            commit(dir, "Initial commit");
            write(dir, "file1.txt", "modified\n");
            fs::remove_file(dir.join("file2.txt")).unwrap();
            write(dir, "file3.txt", "new file\n");
            write(dir, "src/main.rs", "fn main() { println!(\"hello\"); }\n");
            commit(dir, "Various changes");
        }
        "rename" => {
            write(dir, "old_name.txt", "content\n");
            write(dir, "keep.txt", "unchanged\n");
            commit(dir, "Initial commit");
            git(dir, &["mv", "old_name.txt", "new_name.txt"]);
            commit(dir, "Rename file");
        }
        "merge" => {
            write(dir, "main.txt", "main\n");
            commit(dir, "Initial commit");
            git(dir, &["checkout", "-q", "-b", "feature"]);
            write(dir, "feature.txt", "feature\n");
            commit(dir, "Add feature");
            git(dir, &["checkout", "-q", "main"]);
            write(dir, "main2.txt", "main2\n");
            commit(dir, "Add main2");
            git(
                dir,
                &["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"],
            );
        }
        _ => panic!("unknown fixture: {}", name),
    }
}

fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// Runs Git with user and system configuration ignored and a fixed identity
/// and date, and returns its standard output.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Background auto-maintenance would repack fixtures nondeterministically.
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .env("GIT_AUTHOR_NAME", "Test User")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test User")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
        .output()
        .expect("git must be installed to create test fixtures");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
