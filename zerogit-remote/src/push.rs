//! `git push`.

use std::collections::HashMap;

use zerogit::{Oid, Refspec, Repository};

use crate::error::{Error, Result};
use crate::transport::{self, PushCommand, Transport};

/// Options for [`push`].
#[derive(Debug, Clone, Default)]
pub struct PushOptions {
    force: bool,
    set_upstream: bool,
}

impl PushOptions {
    /// The default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows updates that are not fast-forwards (`--force`).
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Makes each pushed local branch track the remote branch (`-u`).
    pub fn set_upstream(mut self, set: bool) -> Self {
        self.set_upstream = set;
        self
    }
}

/// What happened to one pushed reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    /// The remote already had this value; nothing was sent.
    UpToDate,
    /// The remote reference was created.
    Created,
    /// The remote reference moved forward.
    FastForward,
    /// The remote reference was rewritten (forced).
    Forced,
    /// The remote reference was deleted.
    Deleted,
    /// Refused before sending: the remote has changes we do not have
    /// (`fetch first`), the update is not a fast-forward, or an existing
    /// tag would change (`already exists`).
    Rejected(String),
    /// The remote refused the update, with its reason.
    RemoteRejected(String),
}

/// One reference handled by a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushUpdate {
    /// The local reference pushed (`None` for a deletion).
    pub local: Option<String>,
    /// The reference on the remote.
    pub remote: String,
    /// The remote's value before the push.
    pub old: Option<Oid>,
    /// The value pushed (`None` for a deletion).
    pub new: Option<Oid>,
    /// What happened.
    pub status: PushStatus,
}

/// A requested update: the local source (`None` to delete), the remote
/// destination and whether it may be forced.
type Wanted = (Option<(String, Oid)>, String, bool);

/// Resolves the source of a push refspec: a full name, a branch or a tag.
fn resolve_source(repo: &Repository, name: &str) -> Result<(String, Oid)> {
    let candidates = if name.starts_with("refs/") || name == "HEAD" {
        vec![name.to_owned()]
    } else {
        vec![
            format!("refs/heads/{}", name),
            format!("refs/tags/{}", name),
        ]
    };
    for candidate in candidates {
        if let Some(oid) = repo.find_reference(&candidate)? {
            let full = if candidate == "HEAD" {
                let head = std::fs::read_to_string(repo.git_dir().join("HEAD"))?;
                head.trim()
                    .strip_prefix("ref: ")
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        Error::Git(zerogit::Error::RefNotFound("HEAD (detached)".into()))
                    })?
            } else {
                candidate
            };
            return Ok((full, oid));
        }
    }
    Err(Error::Git(zerogit::Error::RefNotFound(name.to_owned())))
}

/// The destination for a source when the refspec gives none or a short
/// name: the same namespace as the source.
fn full_destination(source: &str, dst: Option<&str>) -> String {
    match dst {
        None | Some("") => source.to_owned(),
        Some(dst) if dst.starts_with("refs/") => dst.to_owned(),
        Some(dst) => {
            let namespace = if source.starts_with("refs/tags/") {
                "refs/tags/"
            } else {
                "refs/heads/"
            };
            format!("{}{}", namespace, dst)
        }
    }
}

/// Pushes to a configured remote, like `git push <remote> <refspec>...`.
///
/// Each refspec is `[+]<src>[:<dst>]` (a branch, tag or full name; `*`
/// patterns allowed) or `:<dst>` to delete. With no refspecs, the current
/// branch is pushed to the branch of the same name. Updates that are not
/// fast-forwards are refused unless forced (by `+` or
/// [`PushOptions::force`]); so is changing an existing tag. Accepted
/// updates also move the matching remote-tracking references.
pub fn push(
    repo: &Repository,
    remote: &str,
    refspecs: &[&str],
    options: &PushOptions,
) -> Result<Vec<PushUpdate>> {
    let config = repo.remote(remote)?;
    let url = config
        .push_url()
        .ok_or_else(|| Error::UnsupportedUrl(format!("remote {} has no URL", remote)))?
        .to_owned();
    let mut transport = transport::open_for(repo, &url)?;
    push_with(repo, remote, transport.as_mut(), refspecs, options)
}

/// Pushes through a given transport. See [`push`].
pub fn push_with(
    repo: &Repository,
    remote: &str,
    transport: &mut dyn Transport,
    refspecs: &[&str],
    options: &PushOptions,
) -> Result<Vec<PushUpdate>> {
    let remote_config = repo.remote(remote).ok();
    let remote_refs: HashMap<String, Oid> = transport
        .list_push_refs()?
        .into_iter()
        .map(|r| (r.name, r.oid))
        .collect();

    // Expand the refspecs into (source, destination, force).
    let mut wanted: Vec<Wanted> = Vec::new();
    let specs: Vec<String> = if refspecs.is_empty() {
        vec!["HEAD".to_owned()]
    } else {
        refspecs.iter().map(|s| (*s).to_owned()).collect()
    };
    for spec in &specs {
        let parsed = Refspec::parse(spec)?;
        let force = parsed.is_force() || options.force;
        if parsed.source().is_empty() {
            let dst = full_destination("refs/heads/", parsed.destination());
            wanted.push((None, dst, force));
        } else if parsed.is_pattern() {
            for (name, oid) in repo.references()? {
                if let Some(dst) = parsed.map_to_destination(&name) {
                    wanted.push((Some((name, oid)), dst, force));
                }
            }
        } else {
            let (source, oid) = resolve_source(repo, parsed.source())?;
            let dst = full_destination(&source, parsed.destination());
            wanted.push((Some((source, oid)), dst, force));
        }
    }

    let mut updates = Vec::new();
    let mut commands = Vec::new();
    for (source, dst, force) in wanted {
        let old = remote_refs.get(&dst).copied();
        let new = source.as_ref().map(|(_, oid)| *oid);
        let local = source.map(|(name, _)| name);
        let rejection = match (old, new) {
            (old, new) if old == new => Some(PushStatus::UpToDate),
            (None, None) => Some(PushStatus::Rejected("remote ref does not exist".into())),
            (Some(_), Some(_)) if dst.starts_with("refs/tags/") && !force => {
                Some(PushStatus::Rejected("already exists".into()))
            }
            (Some(old), Some(new)) if !force => {
                if !repo.has_object(&old).unwrap_or(false) {
                    Some(PushStatus::Rejected("fetch first".into()))
                } else if !repo.is_ancestor(&old, &new).unwrap_or(false) {
                    Some(PushStatus::Rejected("non-fast-forward".into()))
                } else {
                    None
                }
            }
            _ => None,
        };
        let update = PushUpdate {
            local,
            remote: dst.clone(),
            old,
            new,
            status: rejection.clone().unwrap_or(PushStatus::UpToDate),
        };
        if rejection.is_none() {
            commands.push(PushCommand {
                old,
                new,
                name: dst,
            });
        }
        updates.push((update, rejection.is_none()));
    }

    if commands.is_empty() {
        transport.push(&[], &[])?;
        return Ok(updates.into_iter().map(|(u, _)| u).collect());
    }
    let wants: Vec<Oid> = commands.iter().filter_map(|c| c.new).collect();
    let haves: Vec<Oid> = remote_refs
        .values()
        .copied()
        .filter(|oid| repo.has_object(oid).unwrap_or(false))
        .collect();
    let pack_options = transport.push_pack_options()?;
    let pack = repo.pack_objects_with(&wants, &haves, &pack_options)?;
    let replies = transport.push(&commands, &pack)?;

    let mut result = Vec::new();
    for (mut update, sent) in updates {
        if sent {
            let reply = replies.iter().find(|r| r.name == update.remote);
            update.status = match reply.and_then(|r| r.error.clone()) {
                Some(reason) => PushStatus::RemoteRejected(reason),
                None => match (update.old, update.new) {
                    (_, None) => PushStatus::Deleted,
                    (None, Some(_)) => PushStatus::Created,
                    (Some(old), Some(new)) if repo.is_ancestor(&old, &new).unwrap_or(false) => {
                        PushStatus::FastForward
                    }
                    _ => PushStatus::Forced,
                },
            };
            let accepted = !matches!(update.status, PushStatus::RemoteRejected(_));
            if accepted {
                // Keep the remote-tracking reference in step.
                if let Some(tracking) = remote_config
                    .as_ref()
                    .and_then(|r| r.tracking_ref(&update.remote))
                {
                    repo.update_reference(&tracking, update.new.as_ref(), None, "update by push")?;
                }
                if options.set_upstream {
                    if let (Some(local), Some(_)) = (&update.local, update.new) {
                        if let Some(branch) = local.strip_prefix("refs/heads/") {
                            repo.set_branch_upstream(branch, Some((remote, &update.remote)))?;
                        }
                    }
                }
            }
        }
        result.push(update);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_destination() {
        assert_eq!(full_destination("refs/heads/main", None), "refs/heads/main");
        assert_eq!(
            full_destination("refs/heads/main", Some("other")),
            "refs/heads/other"
        );
        assert_eq!(full_destination("refs/tags/v1", Some("v2")), "refs/tags/v2");
        assert_eq!(
            full_destination("refs/heads/a", Some("refs/x/y")),
            "refs/x/y"
        );
    }
}
