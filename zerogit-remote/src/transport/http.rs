//! Smart HTTP(S) (`gitprotocol-http`), with TLS by rustls.
//!
//! The advertisement comes from `GET <url>/info/refs?service=<service>`
//! (asking for protocol v2 for fetches) and each request is a `POST` to
//! `<url>/<service>`.

use std::io::{Cursor, Read};

use super::protocol::{Connector, Service, Session};
use crate::error::{Error, Result};

/// Credentials for HTTP requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpAuth {
    /// Basic authentication.
    Basic {
        /// The user name.
        user: String,
        /// The password or access token.
        password: String,
    },
    /// A bearer token.
    Bearer(String),
}

/// Connects to a smart HTTP(S) server.
#[derive(Clone)]
pub struct HttpConnector {
    url: String,
    auth: Option<HttpAuth>,
    agent: ureq::Agent,
}

impl std::fmt::Debug for HttpConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConnector")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes `%XX` escapes in a URL user name or password.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl HttpConnector {
    /// Creates a connector for a repository URL. A `user:password@` in the
    /// URL becomes Basic authentication.
    pub fn new(url: &str) -> Result<Self> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| Error::UnsupportedUrl(url.to_owned()))?;
        let (authority, path) = match rest.find('/') {
            Some(pos) => (&rest[..pos], &rest[pos..]),
            None => (rest, ""),
        };
        let (auth, host) = match authority.rsplit_once('@') {
            Some((userinfo, host)) => {
                let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
                (
                    Some(HttpAuth::Basic {
                        user: percent_decode(user),
                        password: percent_decode(password),
                    }),
                    host,
                )
            }
            None => (None, authority),
        };
        Ok(HttpConnector {
            url: format!("{}://{}{}", scheme, host, path.trim_end_matches('/')),
            auth,
            agent: ureq::AgentBuilder::new().build(),
        })
    }

    /// Sets the credentials.
    pub fn with_auth(mut self, auth: HttpAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    fn authorize(&self, request: ureq::Request) -> ureq::Request {
        match &self.auth {
            Some(HttpAuth::Basic { user, password }) => request.set(
                "Authorization",
                &format!(
                    "Basic {}",
                    base64(format!("{}:{}", user, password).as_bytes())
                ),
            ),
            Some(HttpAuth::Bearer(token)) => {
                request.set("Authorization", &format!("Bearer {}", token))
            }
            None => request,
        }
    }

    fn read(response: std::result::Result<ureq::Response, ureq::Error>) -> Result<Vec<u8>> {
        match response {
            Ok(response) => {
                let mut body = Vec::new();
                response.into_reader().read_to_end(&mut body)?;
                Ok(body)
            }
            Err(ureq::Error::Status(code, response)) => Err(Error::Http(format!(
                "{} {} from {}",
                code,
                response.status_text(),
                response.get_url()
            ))),
            Err(e) => Err(Error::Http(e.to_string())),
        }
    }
}

impl Connector for HttpConnector {
    fn connect(&mut self, service: Service) -> Result<Box<dyn Session>> {
        let mut request = self.authorize(
            self.agent
                .get(&format!("{}/info/refs", self.url))
                .query("service", service.name()),
        );
        if service == Service::UploadPack {
            request = request.set("Git-Protocol", "version=2");
        }
        let body = Self::read(request.call())?;
        Ok(Box::new(HttpSession {
            connector: self.clone(),
            service,
            response: Cursor::new(body),
        }))
    }
}

struct HttpSession {
    connector: HttpConnector,
    service: Service,
    response: Cursor<Vec<u8>>,
}

impl Session for HttpSession {
    fn reader(&mut self) -> &mut dyn Read {
        &mut self.response
    }

    fn send(&mut self, body: &[u8]) -> Result<()> {
        let name = self.service.name();
        let mut request = self
            .connector
            .authorize(
                self.connector
                    .agent
                    .post(&format!("{}/{}", self.connector.url, name)),
            )
            .set("Content-Type", &format!("application/x-{}-request", name))
            .set("Accept", &format!("application/x-{}-result", name));
        if self.service == Service::UploadPack {
            request = request.set("Git-Protocol", "version=2");
        }
        let response = HttpConnector::read(request.send_bytes(body))?;
        self.response = Cursor::new(response);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_and_credentials_in_url() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        let connector = HttpConnector::new("https://me:p%40ss@example.com/repo.git/").unwrap();
        assert_eq!(connector.url, "https://example.com/repo.git");
        assert_eq!(
            connector.auth,
            Some(HttpAuth::Basic {
                user: "me".into(),
                password: "p@ss".into()
            })
        );
    }
}
