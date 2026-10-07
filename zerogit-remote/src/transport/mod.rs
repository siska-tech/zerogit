//! Transports: how references are listed and objects exchanged with a
//! remote.
//!
//! - [`LocalTransport`] reads and writes a repository on this machine
//!   directly, without running Git.
//! - [`GitTransport`] speaks the Git protocol to a server through a
//!   [`Connector`]: a process (`git-upload-pack`, or `ssh host ...`), or
//!   smart HTTP(S) with the `https` feature.

mod local;
mod process;
mod protocol;

#[cfg(feature = "https")]
mod http;

pub use local::LocalTransport;
pub use process::ProcessConnector;
pub use protocol::{Connector, GitTransport, Session};

#[cfg(feature = "https")]
pub use http::{HttpAuth, HttpConnector};

use zerogit::{Oid, PackObjectsOptions};

use crate::error::Result;

/// A reference advertised by a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    /// The full name, such as `refs/heads/main`, or `HEAD`.
    pub name: String,
    /// The object it points to.
    pub oid: Oid,
    /// For an annotated tag, the object the tag points to.
    pub peeled: Option<Oid>,
    /// For a symbolic reference (such as `HEAD`), its target.
    pub symref_target: Option<String>,
}

/// One reference update sent by a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushCommand {
    /// The value the remote is expected to have (`None`: create).
    pub old: Option<Oid>,
    /// The new value (`None`: delete).
    pub new: Option<Oid>,
    /// The full reference name on the remote.
    pub name: String,
}

/// How the remote answered one pushed update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReply {
    /// The full reference name.
    pub name: String,
    /// `None` if the update was accepted, or the reason it was rejected.
    pub error: Option<String>,
}

/// Lists references and exchanges objects with one remote.
pub trait Transport {
    /// Lists the remote's references (with `HEAD` and its target) whose
    /// names start with one of `prefixes` (all if empty).
    fn list_refs(&mut self, prefixes: &[String]) -> Result<Vec<RemoteRef>>;

    /// Returns a pack with the objects reachable from `wants` that are not
    /// reachable from `haves` (tags pointing into it included). The pack
    /// may be thin (deltas against objects in `haves`).
    fn fetch_pack(&mut self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<u8>>;

    /// Lists the references a push can update.
    fn list_push_refs(&mut self) -> Result<Vec<RemoteRef>>;

    /// Sends updates and the pack with the objects they need, and returns
    /// the remote's answer for each update.
    fn push(&mut self, commands: &[PushCommand], pack: &[u8]) -> Result<Vec<PushReply>>;

    /// How to build the pack for [`Transport::push`]: thin and with
    /// OFS_DELTA when the remote takes them. By default, a complete pack
    /// whose deltas name their base by ID, which every receiver reads.
    fn push_pack_options(&mut self) -> Result<PackObjectsOptions> {
        Ok(PackObjectsOptions::new())
    }
}

/// Opens the transport for a URL: local paths and `file://` with
/// [`LocalTransport`], `ssh://` and `user@host:path` by running `ssh`, and
/// `http(s)://` with the HTTP connector (with the `https` feature).
pub fn open(url: &str) -> Result<Box<dyn Transport>> {
    open_with_ssh_command(url, None)
}

/// Opens the transport for a URL like [`open`], using the repository's
/// `core.sshCommand` for SSH when the environment sets neither
/// `GIT_SSH_COMMAND` nor `GIT_SSH`, as Git does.
pub fn open_for(repo: &zerogit::Repository, url: &str) -> Result<Box<dyn Transport>> {
    let config = repo.config()?;
    open_with_ssh_command(url, config.get("core", "sshCommand"))
}

fn open_with_ssh_command(url: &str, ssh_command: Option<&str>) -> Result<Box<dyn Transport>> {
    match crate::url::parse(url)? {
        crate::url::Location::Local(path) => Ok(Box::new(LocalTransport::open(&path)?)),
        crate::url::Location::Ssh { host, port, path } => {
            let mut connector = ProcessConnector::ssh(&host, port, &path);
            if let Some(command) = ssh_command {
                connector = connector.ssh_command(command);
            }
            Ok(Box::new(GitTransport::new(connector)))
        }
        #[cfg(feature = "https")]
        crate::url::Location::Http(url) => {
            Ok(Box::new(GitTransport::new(HttpConnector::new(&url)?)))
        }
        #[cfg(not(feature = "https"))]
        crate::url::Location::Http(url) => Err(crate::error::Error::UnsupportedUrl(format!(
            "{} (built without the https feature)",
            url
        ))),
    }
}
