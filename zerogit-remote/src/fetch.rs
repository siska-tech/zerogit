//! `git fetch`.

use std::collections::{HashSet, VecDeque};

use zerogit::{Oid, Refspec, Repository};

use crate::error::Result;
use crate::transport::{self, RemoteRef, Transport};

/// How a reference changed in a fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    /// It already had the fetched value.
    UpToDate,
    /// It was created.
    New,
    /// It moved forward.
    FastForward,
    /// It was rewritten (allowed by `+` in the refspec).
    Forced,
    /// The update was refused: not a fast-forward (or an existing tag
    /// would change) and the refspec does not force it.
    Rejected,
}

/// One reference touched by a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    /// The reference on the remote, such as `refs/heads/main`.
    pub remote: String,
    /// The local reference, such as `refs/remotes/origin/main`.
    pub local: String,
    /// The local value before the fetch.
    pub old: Option<Oid>,
    /// The fetched value.
    pub new: Oid,
    /// What happened.
    pub kind: UpdateKind,
}

/// The result of a fetch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchOutcome {
    /// The references updated (or refused), in the remote's order; tags
    /// fetched because they point into the fetched history are included.
    pub updates: Vec<RefUpdate>,
    /// `HEAD` on the remote: its target and value, if advertised.
    pub remote_head: Option<RemoteRef>,
}

/// Commits to offer as already present: the tips of every local
/// reference and their recent ancestors.
fn haves(repo: &Repository) -> Result<Vec<Oid>> {
    const LIMIT: usize = 256;
    let mut queue: VecDeque<Oid> = repo
        .references()?
        .into_iter()
        .map(|(_, oid)| oid)
        .chain(repo.find_reference("HEAD")?)
        .collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some(oid) = queue.pop_front() {
        if result.len() >= LIMIT || !seen.insert(oid) {
            continue;
        }
        if let Ok(commit) = repo.commit(&oid.to_hex()) {
            result.push(oid);
            queue.extend(commit.parents().iter().copied());
        }
    }
    Ok(result)
}

fn has_object(repo: &Repository, oid: &Oid) -> bool {
    repo.has_object(oid).unwrap_or(false)
}

/// Fetches from a configured remote, like `git fetch <remote>`: the
/// remote's references are mapped through the remote's fetch refspecs,
/// missing objects are downloaded, local references are updated
/// (fast-forwards, or any change where the refspec has `+`), tags pointing
/// into the fetched history are fetched too, and `FETCH_HEAD` is written.
pub fn fetch(repo: &Repository, remote: &str) -> Result<FetchOutcome> {
    let config = repo.remote(remote)?;
    let url = config
        .url()
        .ok_or_else(|| {
            crate::error::Error::UnsupportedUrl(format!("remote {} has no URL", remote))
        })?
        .to_owned();
    let mut transport = transport::open_for(repo, &url)?;
    fetch_with(
        repo,
        transport.as_mut(),
        config.fetch_refspecs(),
        &url,
        &format!("fetch {}", remote),
        Some(remote),
    )
}

/// Fetches through a given transport with explicit refspecs. `url` is used
/// in `FETCH_HEAD` and `reflog_prefix` starts each reflog message (for
/// example `fetch origin`). `remote` names the remote for choosing what
/// `FETCH_HEAD` marks for merging.
pub fn fetch_with(
    repo: &Repository,
    transport: &mut dyn Transport,
    refspecs: &[Refspec],
    url: &str,
    reflog_prefix: &str,
    remote: Option<&str>,
) -> Result<FetchOutcome> {
    let positive: Vec<&Refspec> = refspecs.iter().filter(|r| !r.is_negative()).collect();
    let mut prefixes: Vec<String> = positive
        .iter()
        .map(|r| r.source().split('*').next().unwrap_or("").to_owned())
        .collect();
    prefixes.push("refs/tags/".to_owned());
    prefixes.sort();
    prefixes.dedup();
    let refs = transport.list_refs(&prefixes)?;
    let zero = Oid::from_bytes([0; 20]);

    // Map the remote's references to local ones.
    let mut mapped: Vec<(RemoteRef, String, bool)> = Vec::new();
    for r in refs.iter().filter(|r| r.name != "HEAD" && r.oid != zero) {
        if refspecs
            .iter()
            .any(|spec| spec.is_negative() && spec.matches_source(&r.name))
        {
            continue;
        }
        if let Some((spec, local)) = positive
            .iter()
            .find_map(|spec| spec.map_to_destination(&r.name).map(|l| (spec, l)))
        {
            if !mapped.iter().any(|(_, l, _)| *l == local) {
                mapped.push((r.clone(), local, spec.is_force()));
            }
        }
    }

    // Download what is missing.
    let mut wants: Vec<Oid> = Vec::new();
    for (r, _, _) in &mapped {
        if !wants.contains(&r.oid) && !has_object(repo, &r.oid) {
            wants.push(r.oid);
        }
    }
    let haves = haves(repo)?;
    if !wants.is_empty() {
        let pack = transport.fetch_pack(&wants, &haves)?;
        if !pack.is_empty() {
            repo.store_pack(&pack)?;
        }
    }

    // Tags pointing into what we now have (git's automatic tag following).
    let mut tags: Vec<(RemoteRef, String, bool)> = Vec::new();
    for r in refs.iter().filter(|r| r.name.starts_with("refs/tags/")) {
        if mapped.iter().any(|(m, _, _)| m.name == r.name)
            || repo.find_reference(&r.name)?.is_some()
        {
            continue;
        }
        let target = r.peeled.unwrap_or(r.oid);
        if has_object(repo, &target) {
            tags.push((r.clone(), r.name.clone(), false));
        }
    }
    let missing_tags: Vec<Oid> = tags
        .iter()
        .map(|(r, _, _)| r.oid)
        .filter(|oid| !has_object(repo, oid))
        .collect();
    if !missing_tags.is_empty() {
        let pack = transport.fetch_pack(&missing_tags, &haves)?;
        if !pack.is_empty() {
            repo.store_pack(&pack)?;
        }
    }
    mapped.extend(tags);

    // Update local references.
    let mut outcome = FetchOutcome {
        updates: Vec::new(),
        remote_head: refs.iter().find(|r| r.name == "HEAD").cloned(),
    };
    for (r, local, force) in &mapped {
        let old = repo.find_reference(local)?;
        let is_tag = local.starts_with("refs/tags/");
        let kind = match old {
            Some(old) if old == r.oid => UpdateKind::UpToDate,
            None => UpdateKind::New,
            Some(_) if *force => UpdateKind::Forced,
            Some(_) if is_tag => UpdateKind::Rejected,
            Some(old) if repo.is_ancestor(&old, &r.oid).unwrap_or(false) => UpdateKind::FastForward,
            Some(_) => UpdateKind::Rejected,
        };
        let kind = match (kind, old) {
            // A forced update that is in fact a fast-forward is reported so.
            (UpdateKind::Forced, Some(old)) if repo.is_ancestor(&old, &r.oid).unwrap_or(false) => {
                UpdateKind::FastForward
            }
            (kind, _) => kind,
        };
        let message = match kind {
            UpdateKind::New if is_tag => Some("storing tag"),
            UpdateKind::New => Some("storing head"),
            UpdateKind::FastForward => Some("fast-forward"),
            UpdateKind::Forced => Some("forced-update"),
            _ => None,
        };
        if let Some(message) = message {
            repo.update_reference(
                local,
                Some(&r.oid),
                Some(old),
                &format!("{}: {}", reflog_prefix, message),
            )?;
        }
        outcome.updates.push(RefUpdate {
            remote: r.name.clone(),
            local: local.clone(),
            old,
            new: r.oid,
            kind,
        });
    }
    write_fetch_head(repo, &mapped, url, remote)?;
    Ok(outcome)
}

/// Writes `FETCH_HEAD`: the fetched references, the current branch's
/// upstream first and the rest marked `not-for-merge`.
fn write_fetch_head(
    repo: &Repository,
    mapped: &[(RemoteRef, String, bool)],
    url: &str,
    remote: Option<&str>,
) -> Result<()> {
    let head = std::fs::read_to_string(repo.git_dir().join("HEAD")).unwrap_or_default();
    let merge_ref = match (head.trim().strip_prefix("ref: refs/heads/"), remote) {
        (Some(branch), Some(remote)) => match repo.branch_upstream(branch)? {
            Some((r, merge)) if r == remote => Some(merge),
            _ => None,
        },
        _ => None,
    };
    // Git shows the URL without credentials, a trailing slash or ".git".
    let mut shown = url.trim_end_matches('/').to_owned();
    if let Some((scheme, rest)) = shown.split_once("://") {
        if let Some((_, host)) = rest.split_once('@').filter(|(user, _)| !user.contains('/')) {
            shown = format!("{}://{}", scheme, host);
        }
    }
    let shown = shown
        .strip_suffix(".git")
        .unwrap_or(&shown)
        .trim_end_matches('/')
        .to_owned();
    let url = shown.as_str();
    let describe = |name: &str| {
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            format!("branch '{}' of {}", branch, url)
        } else if let Some(tag) = name.strip_prefix("refs/tags/") {
            format!("tag '{}' of {}", tag, url)
        } else {
            format!("'{}' of {}", name, url)
        }
    };
    let mut for_merge = String::new();
    let mut others = String::new();
    for (r, _, _) in mapped {
        if merge_ref.as_deref() == Some(r.name.as_str()) {
            for_merge.push_str(&format!("{}\t\t{}\n", r.oid, describe(&r.name)));
        } else {
            others.push_str(&format!(
                "{}\tnot-for-merge\t{}\n",
                r.oid,
                describe(&r.name)
            ));
        }
    }
    std::fs::write(repo.git_dir().join("FETCH_HEAD"), for_merge + &others)?;
    Ok(())
}
