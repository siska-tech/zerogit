//! Remote URLs.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Where a remote is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A repository on this machine (`/path`, `./path`, `file:///path`).
    Local(PathBuf),
    /// SSH: `ssh://[user@]host[:port]/path` or `[user@]host:path`.
    Ssh {
        /// `user@host`, or `host`.
        host: String,
        /// The port, if given.
        port: Option<u16>,
        /// The path on the server.
        path: String,
    },
    /// Smart HTTP(S): `http://...` or `https://...`.
    Http(String),
}

/// Parses a remote URL the way Git interprets it.
pub fn parse(url: &str) -> Result<Location> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Ok(Location::Local(PathBuf::from(rest)));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(Location::Http(url.trim_end_matches('/').to_owned()));
    }
    if let Some(rest) = url
        .strip_prefix("ssh://")
        .or_else(|| url.strip_prefix("git+ssh://"))
    {
        let (authority, path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => return Err(Error::UnsupportedUrl(url.to_owned())),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
                let port = port
                    .parse()
                    .map_err(|_| Error::UnsupportedUrl(url.to_owned()))?;
                (host.to_owned(), Some(port))
            }
            _ => (authority.to_owned(), None),
        };
        // ssh://host/~user/path keeps the home-relative form.
        let path = path
            .strip_prefix("/~")
            .map_or(path.to_owned(), |p| format!("~{}", p));
        return Ok(Location::Ssh { host, port, path });
    }
    if url.contains("://") {
        return Err(Error::UnsupportedUrl(url.to_owned()));
    }
    // scp-like syntax: [user@]host:path, where the part before ':' has no '/'.
    if let Some((host, path)) = url.split_once(':') {
        let looks_like_drive = host.len() == 1 && host.chars().all(|c| c.is_ascii_alphabetic());
        if !host.contains('/') && !host.is_empty() && !looks_like_drive {
            return Ok(Location::Ssh {
                host: host.to_owned(),
                port: None,
                path: path.to_owned(),
            });
        }
    }
    Ok(Location::Local(PathBuf::from(url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        assert_eq!(
            parse("/srv/repo.git").unwrap(),
            Location::Local("/srv/repo.git".into())
        );
        assert_eq!(
            parse("file:///srv/repo.git").unwrap(),
            Location::Local("/srv/repo.git".into())
        );
        assert_eq!(
            parse("C:/repos/x").unwrap(),
            Location::Local("C:/repos/x".into())
        );
        assert_eq!(
            parse("https://example.com/org/repo.git/").unwrap(),
            Location::Http("https://example.com/org/repo.git".into())
        );
        assert_eq!(
            parse("git@github.com:org/repo.git").unwrap(),
            Location::Ssh {
                host: "git@github.com".into(),
                port: None,
                path: "org/repo.git".into()
            }
        );
        assert_eq!(
            parse("ssh://git@example.com:2222/srv/repo.git").unwrap(),
            Location::Ssh {
                host: "git@example.com".into(),
                port: Some(2222),
                path: "/srv/repo.git".into()
            }
        );
        assert_eq!(
            parse("ssh://example.com/~me/repo").unwrap(),
            Location::Ssh {
                host: "example.com".into(),
                port: None,
                path: "~me/repo".into()
            }
        );
        assert!(parse("ftp://example.com/repo").is_err());
    }
}
