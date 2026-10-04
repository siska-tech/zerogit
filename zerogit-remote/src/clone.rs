//! `git clone`.

use std::path::Path;

use zerogit::{Refspec, Repository};

use crate::error::{Error, Result};
use crate::fetch::fetch_with;
use crate::transport::{self, Transport};

/// Options for [`clone`].
#[derive(Debug, Clone)]
pub struct CloneOptions {
    bare: bool,
    branch: Option<String>,
    remote_name: String,
}

impl Default for CloneOptions {
    fn default() -> Self {
        CloneOptions {
            bare: false,
            branch: None,
            remote_name: "origin".to_owned(),
        }
    }
}

impl CloneOptions {
    /// The default options: a work tree, the remote's default branch,
    /// remote `origin`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a bare repository (`--bare`): the remote's branches become
    /// local branches and nothing is checked out.
    pub fn bare(mut self, bare: bool) -> Self {
        self.bare = bare;
        self
    }

    /// Checks out this branch instead of the remote's default (`--branch`).
    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    /// Names the remote (`--origin`).
    pub fn remote_name(mut self, name: impl Into<String>) -> Self {
        self.remote_name = name.into();
        self
    }
}

/// The URL to record for the remote: local paths are made absolute, as Git
/// does, without resolving symbolic links or short names.
fn recorded_url(url: &str) -> String {
    match crate::url::parse(url) {
        Ok(crate::url::Location::Local(path)) if !url.starts_with("file://") => {
            if path.is_absolute() {
                url.to_owned()
            } else {
                std::env::current_dir()
                    .map(|dir| dir.join(&path).to_string_lossy().into_owned())
                    .unwrap_or_else(|_| url.to_owned())
            }
        }
        _ => url.to_owned(),
    }
}

/// Clones a repository into `path`, like `git clone <url> <path>`.
///
/// The new repository gets the remote (`origin`) with the default fetch
/// refspec, all its branches as remote-tracking branches, tags, the default
/// branch (or [`CloneOptions::branch`]) as a local branch tracking the
/// remote one, and that branch checked out.
pub fn clone(url: &str, path: &Path, options: &CloneOptions) -> Result<Repository> {
    let mut transport = transport::open(url)?;
    clone_with(url, path, options, transport.as_mut())
}

/// Clones through a given transport. See [`clone`].
pub fn clone_with(
    url: &str,
    path: &Path,
    options: &CloneOptions,
    transport: &mut dyn Transport,
) -> Result<Repository> {
    let repo = if options.bare {
        Repository::init_bare(path)?
    } else {
        Repository::init(path)?
    };
    let url = recorded_url(url);
    let message = format!("clone: from {}", url);
    let name = &options.remote_name;
    let refspecs = if options.bare {
        // A bare clone mirrors the branches as local branches.
        vec![Refspec::parse("+refs/heads/*:refs/heads/*")?]
    } else {
        repo.add_remote(name, &url)?;
        repo.remote(name)?.fetch_refspecs().to_vec()
    };
    if options.bare {
        repo.add_remote(name, &url)?;
    }
    let outcome = fetch_with(&repo, transport, &refspecs, &url, &message, None)?;

    // The branch to check out: the requested one, or the remote's HEAD.
    let remote_head = outcome.remote_head.clone();
    let branch_ref = match &options.branch {
        Some(branch) => format!("refs/heads/{}", branch),
        None => remote_head
            .as_ref()
            .and_then(|h| h.symref_target.clone())
            .unwrap_or_else(|| "refs/heads/main".to_owned()),
    };
    let branch = branch_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&branch_ref)
        .to_owned();
    let tracking = format!("refs/remotes/{}/{}", name, branch);
    let source = if options.bare {
        branch_ref.clone()
    } else {
        tracking.clone()
    };
    let oid = repo.find_reference(&source)?;
    if oid.is_none() && options.branch.is_some() {
        return Err(Error::Remote(format!("remote branch {} not found", branch)));
    }

    if let Some(oid) = oid {
        if !options.bare {
            repo.update_reference(&branch_ref, Some(&oid), Some(None), &message)?;
            repo.set_branch_upstream(&branch, Some((name, &branch_ref)))?;
            // origin/HEAD names the remote's default branch.
            if let Some(default) = remote_head
                .as_ref()
                .and_then(|h| h.symref_target.as_deref())
                .and_then(|t| t.strip_prefix("refs/heads/"))
            {
                repo.set_symbolic_reference(
                    &format!("refs/remotes/{}/HEAD", name),
                    &format!("refs/remotes/{}/{}", name, default),
                    None,
                )?;
            }
        }
        repo.set_symbolic_reference("HEAD", &branch_ref, Some(&message))?;
        if !options.bare {
            repo.reset_hard()?;
        }
    } else {
        // An empty remote: point HEAD at its default branch name.
        repo.set_symbolic_reference("HEAD", &branch_ref, None)?;
    }
    Ok(repo)
}
