//! A repository on this machine, read and written directly.

use std::path::Path;

use zerogit::{Oid, Repository};

use super::{PushCommand, PushReply, RemoteRef, Transport};
use crate::error::Result;

/// Fetches from and pushes to a local repository without running Git.
///
/// Pushes behave like `git receive-pack` with default settings: an update
/// is refused if the reference changed since it was listed, and pushing to
/// the branch checked out in a non-bare repository is refused
/// (`receive.denyCurrentBranch`).
pub struct LocalTransport {
    repo: Repository,
}

impl LocalTransport {
    /// Opens the repository at `path` (bare or not).
    pub fn open(path: &Path) -> Result<Self> {
        Ok(LocalTransport {
            repo: Repository::open(path)?,
        })
    }

    fn head(&self) -> Result<Option<RemoteRef>> {
        let content = std::fs::read_to_string(self.repo.git_dir().join("HEAD"))?;
        let target = content.trim().strip_prefix("ref: ").map(str::to_owned);
        let oid = self.repo.find_reference("HEAD")?;
        Ok(match (oid, target) {
            (Some(oid), target) => Some(RemoteRef {
                name: "HEAD".into(),
                oid,
                peeled: None,
                symref_target: target,
            }),
            // An unborn HEAD still names the default branch.
            (None, Some(target)) => Some(RemoteRef {
                name: "HEAD".into(),
                oid: Oid::from_bytes([0; 20]),
                peeled: None,
                symref_target: Some(target),
            }),
            (None, None) => None,
        })
    }

    fn refs(&self) -> Result<Vec<RemoteRef>> {
        let mut refs = Vec::new();
        for (name, oid) in self.repo.references()? {
            let peeled = if name.starts_with("refs/tags/") {
                let target = self.repo.peel(&oid)?;
                (target != oid).then_some(target)
            } else {
                None
            };
            refs.push(RemoteRef {
                name,
                oid,
                peeled,
                symref_target: None,
            });
        }
        Ok(refs)
    }
}

impl Transport for LocalTransport {
    fn list_refs(&mut self, prefixes: &[String]) -> Result<Vec<RemoteRef>> {
        let mut refs: Vec<RemoteRef> = self.head()?.into_iter().collect();
        refs.extend(self.refs()?.into_iter().filter(|r| {
            prefixes.is_empty() || prefixes.iter().any(|p| r.name.starts_with(p.as_str()))
        }));
        Ok(refs)
    }

    fn fetch_pack(&mut self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<u8>> {
        if wants.is_empty() {
            return Ok(Vec::new());
        }
        // Tags pointing into what is sent come along (include-tag).
        let mut wants = wants.to_vec();
        let sent: std::collections::HashSet<Oid> = self
            .repo
            .objects_to_send(&wants, haves)?
            .into_iter()
            .collect();
        for r in self.refs()? {
            if r.name.starts_with("refs/tags/") {
                if let Some(peeled) = r.peeled {
                    if sent.contains(&peeled) && !wants.contains(&r.oid) {
                        wants.push(r.oid);
                    }
                }
            }
        }
        Ok(self.repo.pack_objects(&wants, haves)?)
    }

    fn list_push_refs(&mut self) -> Result<Vec<RemoteRef>> {
        self.refs()
    }

    fn push(&mut self, commands: &[PushCommand], pack: &[u8]) -> Result<Vec<PushReply>> {
        if commands.iter().any(|c| c.new.is_some()) && !pack.is_empty() {
            self.repo.store_pack(pack)?;
        }
        let current_branch = if self.repo.is_bare() {
            None
        } else {
            self.head()?.and_then(|h| h.symref_target)
        };
        let mut replies = Vec::new();
        for command in commands {
            let error = if current_branch.as_deref() == Some(command.name.as_str()) {
                Some("branch is currently checked out".to_owned())
            } else {
                match self.repo.update_reference(
                    &command.name,
                    command.new.as_ref(),
                    Some(command.old),
                    "push",
                ) {
                    Ok(()) => None,
                    Err(zerogit::Error::StaleReference(_)) => Some("failed to lock".to_owned()),
                    Err(e) => Some(e.to_string()),
                }
            };
            replies.push(PushReply {
                name: command.name.clone(),
                error,
            });
        }
        Ok(replies)
    }
}
