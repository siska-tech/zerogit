//! Connections through a process: `git upload-pack` / `git receive-pack`
//! for a local repository, or `ssh host git-upload-pack 'path'` (key
//! authentication through the system's SSH client and agent).

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use super::protocol::{Connector, Service, Session};
use crate::error::{Error, Result};

/// Runs the service as a process and talks to it over its standard input
/// and output.
#[derive(Debug, Clone)]
pub struct ProcessConnector {
    kind: Kind,
    v2: bool,
}

#[derive(Debug, Clone)]
enum Kind {
    /// `git upload-pack <path>` on this machine.
    Git { path: String },
    /// The service on an SSH server.
    Ssh {
        host: String,
        port: Option<u16>,
        path: String,
    },
}

/// Quotes an argument for the remote shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl ProcessConnector {
    /// Runs Git's own `upload-pack` / `receive-pack` on a local repository
    /// (needs `git` installed; [`super::LocalTransport`] does not).
    pub fn git(path: &str) -> Self {
        ProcessConnector {
            kind: Kind::Git {
                path: path.to_owned(),
            },
            v2: true,
        }
    }

    /// Runs the service on an SSH server. The client is `ssh`, or the
    /// command in `GIT_SSH_COMMAND` (split on whitespace).
    pub fn ssh(host: &str, port: Option<u16>, path: &str) -> Self {
        ProcessConnector {
            kind: Kind::Ssh {
                host: host.to_owned(),
                port,
                path: path.to_owned(),
            },
            v2: true,
        }
    }

    /// Whether to ask for protocol version 2 when fetching (the default).
    /// Without it, the server speaks the original protocol.
    pub fn protocol_v2(mut self, v2: bool) -> Self {
        self.v2 = v2;
        self
    }

    fn command(&self, service: Service) -> Command {
        match &self.kind {
            Kind::Git { path } => {
                let mut command = Command::new("git");
                command
                    .arg(match service {
                        Service::UploadPack => "upload-pack",
                        Service::ReceivePack => "receive-pack",
                    })
                    .arg(path);
                command
            }
            Kind::Ssh { host, port, path } => {
                let ssh = std::env::var("GIT_SSH_COMMAND").unwrap_or_else(|_| "ssh".to_owned());
                let mut words = ssh.split_whitespace();
                let mut command = Command::new(words.next().unwrap_or("ssh"));
                command.args(words);
                if let Some(port) = port {
                    command.arg("-p").arg(port.to_string());
                }
                command
                    .arg("-o")
                    .arg("SendEnv=GIT_PROTOCOL")
                    .arg(host)
                    .arg(format!("{} {}", service.name(), shell_quote(path)));
                command
            }
        }
    }
}

impl Connector for ProcessConnector {
    fn connect(&mut self, service: Service) -> Result<Box<dyn Session>> {
        let mut command = self.command(service);
        if service == Service::UploadPack && self.v2 {
            command.env("GIT_PROTOCOL", "version=2");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("cannot run {:?}: {}", command, e),
                ))
            })?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Protocol("no output from the process".into()))?;
        Ok(Box::new(ProcessSession {
            child,
            stdin,
            stdout,
        }))
    }
}

struct ProcessSession {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
}

impl Session for ProcessSession {
    fn reader(&mut self) -> &mut dyn Read {
        &mut self.stdout
    }

    fn send(&mut self, request: &[u8]) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| Error::Protocol("connection closed".into()))?;
        stdin.write_all(request)?;
        stdin.flush()?;
        Ok(())
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        // Closing standard input ends the conversation.
        self.stdin.take();
        let mut rest = Vec::new();
        let _ = self.stdout.read_to_end(&mut rest);
        let _ = self.child.wait();
    }
}
