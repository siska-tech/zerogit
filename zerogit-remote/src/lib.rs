//! # zerogit-remote
//!
//! Remote operations for [`zerogit`]: [`clone`](clone()), [`fetch`](fetch())
//! and [`push`](push()), over these transports:
//!
//! - Local repositories (paths and `file://`), read and written directly
//!   without running Git.
//! - SSH (`ssh://` and `user@host:path`) through the system's `ssh` client
//!   (key authentication by its agent and configuration).
//! - Smart HTTP(S) (`http://`, `https://`) with TLS by rustls, with Basic
//!   or bearer token authentication (the `https` feature, on by default).
//!
//! Fetching uses Git protocol version 2 (falling back to the original
//! protocol when a server does not offer it); pushing uses the
//! receive-pack protocol. Objects are stored with [`zerogit`]'s pack
//! handling, so the core crate keeps its single dependency.
//!
//! ```no_run
//! use std::path::Path;
//! use zerogit_remote::{clone, fetch, push, CloneOptions, PushOptions};
//!
//! let repo = clone("https://example.com/repo.git", Path::new("repo"), &CloneOptions::new())?;
//! fetch(&repo, "origin")?;
//! push(&repo, "origin", &["main"], &PushOptions::new())?;
//! # Ok::<(), zerogit_remote::Error>(())
//! ```
//!
//! Shallow and partial clones are not supported.

mod clone;
mod error;
mod fetch;
pub mod pktline;
mod push;
pub mod transport;
pub mod url;

pub use clone::{clone, clone_with, CloneOptions};
pub use error::{Error, Result};
pub use fetch::{fetch, fetch_with, FetchOutcome, RefUpdate, UpdateKind};
pub use push::{push, push_with, PushOptions, PushStatus, PushUpdate};
