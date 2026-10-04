//! Errors of remote operations.

use std::fmt;

/// An error from a remote operation.
#[derive(Debug)]
pub enum Error {
    /// An error from the local repository.
    Git(zerogit::Error),
    /// An I/O error (including a failed connection or process).
    Io(std::io::Error),
    /// The remote sent something that does not follow the protocol.
    Protocol(String),
    /// The remote reported an error (`ERR` packet or an error band).
    Remote(String),
    /// An HTTP request failed.
    Http(String),
    /// The URL is not one this crate can handle.
    UnsupportedUrl(String),
    /// The connection ended before the remote answered: for SSH, the
    /// client's exit status and messages (authentication failed, host key
    /// verification failed, repository not found, ...).
    Connection(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Git(e) => write!(f, "{}", e),
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Protocol(reason) => write!(f, "protocol error: {}", reason),
            Error::Remote(message) => write!(f, "remote error: {}", message),
            Error::Http(reason) => write!(f, "HTTP error: {}", reason),
            Error::UnsupportedUrl(url) => write!(f, "unsupported URL: {}", url),
            Error::Connection(details) => write!(f, "connection failed: {}", details),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Git(e) => Some(e),
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<zerogit::Error> for Error {
    fn from(e: zerogit::Error) -> Self {
        Error::Git(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;
