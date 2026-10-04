//! Document diff flow: pin a commit, list changed files, then diff one file.
//!
//! ```text
//! cargo run --example document_diff -- <repo> [--commit <oid>] [--parent <n>] [<path>...]
//! ```
//!
//! 1. The commit (default `HEAD`) is resolved to an OID once. Every later
//!    read uses OIDs, so branches moving or `git gc` running meanwhile do not
//!    change what is compared.
//! 2. The base is the first parent by default, the empty tree for a root
//!    commit, or the parent chosen with `--parent` (e.g. `--parent 2` for the
//!    merged branch of a merge commit).
//! 3. The change list comes from tree comparison only; no blob is read.
//! 4. Blobs are read only for the files shown in detail. Errors, non-text
//!    content and skipped diffs are reported explicitly, never as an empty diff.

use std::process::ExitCode;

use zerogit::{
    BlobDiffContent, DiffDelta, DiffOptions, DiffStatus, FileMode, LineKind, Oid, Repository,
    Result,
};

struct Args {
    repo: String,
    commit: String,
    parent: usize,
    paths: Vec<String>,
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let repo = args.next()?;
    let mut commit = "HEAD".to_owned();
    let mut parent = 1;
    let mut paths = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--commit" => commit = args.next()?,
            "--parent" => parent = args.next()?.parse().ok().filter(|&n| n >= 1)?,
            _ => paths.push(arg),
        }
    }
    Some(Args {
        repo,
        commit,
        parent,
        paths,
    })
}

/// Resolves `HEAD` or a (short) OID to a fixed commit OID.
fn pin_commit(repo: &Repository, spec: &str) -> Result<Oid> {
    if spec == "HEAD" {
        Ok(*repo.head()?.oid())
    } else {
        repo.resolve_short_oid(spec)
    }
}

fn mode(mode: Option<FileMode>) -> &'static str {
    mode.map_or("------", |m| m.as_octal())
}

fn describe(delta: &DiffDelta) -> String {
    let path = delta.path().display();
    let modes = format!("{} -> {}", mode(delta.old_mode()), mode(delta.new_mode()));
    match (delta.status(), delta.old_path()) {
        (DiffStatus::Renamed, Some(old)) => format!("R {} -> {} ({})", old.display(), path, modes),
        (status, _) => format!("{} {} ({})", status.as_char(), path, modes),
    }
}

/// Shows the detail of one change, reading its blobs only now.
fn show_detail(repo: &Repository, delta: &DiffDelta, options: &DiffOptions) -> Result<()> {
    println!("\n=== {}", describe(delta));
    // A gitlink records a submodule commit, which is not a blob of this repository.
    if delta.old_mode() == Some(FileMode::Submodule)
        || delta.new_mode() == Some(FileMode::Submodule)
    {
        let oid = |o: Option<&Oid>| o.map_or("(none)".to_owned(), Oid::to_hex);
        println!(
            "submodule commit {} -> {}",
            oid(delta.old_oid()),
            oid(delta.new_oid())
        );
        return Ok(());
    }
    if delta.new_mode() == Some(FileMode::Symlink) || delta.old_mode() == Some(FileMode::Symlink) {
        println!("(symlink: the content below is the link target)");
    }
    let diff = repo.diff_blobs(delta.old_oid(), delta.new_oid(), options)?;
    if !diff.old_exists() {
        println!("(file added)");
    }
    if !diff.new_exists() {
        println!("(file deleted)");
    }
    match diff.content() {
        BlobDiffContent::Text(hunks) if hunks.is_empty() => {
            // Renames and mode-only changes have identical content.
            println!("(content unchanged)");
        }
        BlobDiffContent::Text(hunks) => {
            for hunk in hunks {
                println!("{}", hunk.header());
                for line in hunk.lines() {
                    let (mark, old, new) = match line.kind() {
                        LineKind::Context => (' ', line.old_lineno(), line.new_lineno()),
                        LineKind::Removed => ('-', line.old_lineno(), None),
                        LineKind::Added => ('+', None, line.new_lineno()),
                    };
                    let number = |n: Option<usize>| n.map_or(String::new(), |n| n.to_string());
                    println!(
                        "{:>5} {:>5} {}{}",
                        number(old),
                        number(new),
                        mark,
                        line.text()
                    );
                }
            }
        }
        BlobDiffContent::NonText(reason) => {
            println!(
                "(not compared as text: {:?}; {} -> {} bytes)",
                reason,
                diff.old_size(),
                diff.new_size()
            );
        }
        BlobDiffContent::Skipped(reason) => println!("(diff skipped: {})", reason),
    }
    Ok(())
}

fn run(args: &Args) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::open(&args.repo)?;
    let commit_oid = pin_commit(&repo, &args.commit)?;
    let commit = repo.commit(&commit_oid.to_hex())?;
    let base_tree = match commit.parents().get(args.parent - 1) {
        Some(parent) => Some(repo.tree(&repo.commit(&parent.to_hex())?.tree().to_hex())?),
        None if commit.is_root() => None, // compare with the empty tree
        None => return Err(format!("commit {} has no parent {}", commit_oid, args.parent).into()),
    };
    let tree = repo.tree(&commit.tree().to_hex())?;

    println!("commit {} ({})", commit_oid, commit.summary());
    let changes = repo.diff_trees(base_tree.as_ref(), &tree)?;
    for delta in changes.deltas() {
        println!("{}", describe(delta));
    }

    let options = DiffOptions::new();
    for delta in changes.deltas() {
        let selected = args.paths.is_empty()
            || args
                .paths
                .iter()
                .any(|p| delta.path().to_str() == Some(p.as_str()));
        if selected {
            show_detail(&repo, delta, &options)?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let Some(args) = parse_args() else {
        eprintln!("usage: document_diff <repo> [--commit <oid>] [--parent <n>] [<path>...]");
        return ExitCode::from(2);
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // Corrupt or unsupported repositories are errors, not empty histories.
        Err(error) => {
            eprintln!("error: {}", error);
            ExitCode::FAILURE
        }
    }
}
