//! Connections through a process: `git upload-pack` / `git receive-pack`
//! for a local repository, or `ssh host git-upload-pack 'path'` (key
//! authentication through the system's SSH client and agent).
//!
//! The SSH client is chosen as Git chooses it: `GIT_SSH_COMMAND` (run by
//! the shell), then `GIT_SSH` (a program), then `core.sshCommand` (run by
//! the shell; see [`ProcessConnector::ssh_command`]), then `ssh`. PuTTY's
//! `plink` and `tortoiseplink` get their own port option, as in Git.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread::JoinHandle;

use super::protocol::{Connector, Service, Session};
use crate::error::{Error, Result};

/// The most standard error output kept for an error message.
const STDERR_KEPT: usize = 4096;

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
        /// `core.sshCommand`, used when the environment names no client.
        command: Option<String>,
    },
}

/// Quotes an argument for the remote shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// How the SSH client is run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SshClient {
    /// A program, run directly (`GIT_SSH`, or the default `ssh`).
    Program(String),
    /// A command line, run by the shell (`GIT_SSH_COMMAND`,
    /// `core.sshCommand`).
    Shell(String),
}

/// Picks the SSH client the way Git does.
fn resolve_client(env: &dyn Fn(&str) -> Option<String>, configured: Option<&str>) -> SshClient {
    if let Some(command) = env("GIT_SSH_COMMAND").filter(|c| !c.trim().is_empty()) {
        return SshClient::Shell(command);
    }
    if let Some(program) = env("GIT_SSH").filter(|p| !p.is_empty()) {
        return SshClient::Program(program);
    }
    if let Some(command) = configured.filter(|c| !c.trim().is_empty()) {
        return SshClient::Shell(command.to_owned());
    }
    SshClient::Program("ssh".to_owned())
}

/// The command-line conventions of an SSH client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Variant {
    /// OpenSSH (and compatible clients): `-p <port>`, `-o SendEnv=...`.
    OpenSsh,
    /// PuTTY's `plink`: `-P <port>`.
    Plink,
    /// TortoiseGit's `tortoiseplink`: `-batch -P <port>`.
    TortoisePlink,
}

fn variant(client: &SshClient) -> Variant {
    let program = match client {
        SshClient::Program(program) => program.as_str(),
        SshClient::Shell(command) => command.split_whitespace().next().unwrap_or(""),
    };
    // The base name, with either separator (a Windows path may be configured
    // on any platform) and without ".exe", as Git compares it.
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower = base.to_ascii_lowercase();
    match lower.strip_suffix(".exe").unwrap_or(&lower) {
        "plink" => Variant::Plink,
        "tortoiseplink" => Variant::TortoisePlink,
        _ => Variant::OpenSsh,
    }
}

/// The arguments given to the SSH client after its own command line.
fn ssh_args(
    variant: Variant,
    host: &str,
    port: Option<u16>,
    service: Service,
    path: &str,
) -> Result<Vec<String>> {
    // A host or path starting with '-' would be taken as an option of the
    // client (CVE-2017-1000117); Git refuses them too.
    if host.starts_with('-') {
        return Err(Error::UnsupportedUrl(format!(
            "strange host name '{}' blocked",
            host
        )));
    }
    if path.starts_with('-') {
        return Err(Error::UnsupportedUrl(format!(
            "strange path name '{}' blocked",
            path
        )));
    }
    let mut args = Vec::new();
    match variant {
        Variant::OpenSsh => {
            args.extend(["-o".to_owned(), "SendEnv=GIT_PROTOCOL".to_owned()]);
            if let Some(port) = port {
                args.extend(["-p".to_owned(), port.to_string()]);
            }
        }
        Variant::Plink | Variant::TortoisePlink => {
            if variant == Variant::TortoisePlink {
                args.push("-batch".to_owned());
            }
            if let Some(port) = port {
                args.extend(["-P".to_owned(), port.to_string()]);
            }
        }
    }
    args.push(host.to_owned());
    args.push(format!("{} {}", service.name(), shell_quote(path)));
    Ok(args)
}

/// Whether a command needs the shell (Git's `prepare_shell_cmd` test).
fn needs_shell(command: &str) -> bool {
    command
        .chars()
        .any(|c| "|&;<>()$`\\\"' \t\n*?[#~=%".contains(c))
}

/// The ways to start `client` with `args`, to try in order: a command line
/// runs through `sh`, and (on Windows, where `sh` may not be on the path)
/// split on whitespace if `sh` cannot be found.
fn client_commands(client: &SshClient, args: &[String]) -> Vec<Command> {
    match client {
        SshClient::Program(program) => {
            let mut command = Command::new(program);
            command.args(args);
            vec![command]
        }
        SshClient::Shell(line) if !needs_shell(line) => {
            let mut command = Command::new(line);
            command.args(args);
            vec![command]
        }
        SshClient::Shell(line) => {
            let mut shell = Command::new("sh");
            shell
                .arg("-c")
                .arg(format!("{} \"$@\"", line))
                .arg(line)
                .args(args);
            let mut commands = vec![shell];
            if cfg!(windows) {
                let mut words = line.split_whitespace();
                let mut split = Command::new(words.next().unwrap_or("ssh"));
                split.args(words).args(args);
                commands.push(split);
            }
            commands
        }
    }
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

    /// Runs the service on an SSH server. The client is chosen as Git
    /// chooses it: `GIT_SSH_COMMAND` (run by the shell), `GIT_SSH` (a
    /// program), the command set with [`ProcessConnector::ssh_command`],
    /// or `ssh`.
    pub fn ssh(host: &str, port: Option<u16>, path: &str) -> Self {
        ProcessConnector {
            kind: Kind::Ssh {
                host: host.to_owned(),
                port,
                path: path.to_owned(),
                command: None,
            },
            v2: true,
        }
    }

    /// Sets the SSH command used when neither `GIT_SSH_COMMAND` nor
    /// `GIT_SSH` is set, like `core.sshCommand`: a command line run by the
    /// shell, such as `ssh -i ~/.ssh/deploy_key`. Has no effect on a
    /// connector made with [`ProcessConnector::git`].
    pub fn ssh_command(mut self, command: impl Into<String>) -> Self {
        if let Kind::Ssh { command: slot, .. } = &mut self.kind {
            *slot = Some(command.into());
        }
        self
    }

    /// Whether to ask for protocol version 2 when fetching (the default).
    /// Without it, the server speaks the original protocol.
    pub fn protocol_v2(mut self, v2: bool) -> Self {
        self.v2 = v2;
        self
    }

    /// The ways to start the process, to try in order.
    fn commands(&self, service: Service) -> Result<Vec<Command>> {
        match &self.kind {
            Kind::Git { path } => {
                let mut command = Command::new("git");
                command
                    .arg(match service {
                        Service::UploadPack => "upload-pack",
                        Service::ReceivePack => "receive-pack",
                    })
                    .arg(path);
                Ok(vec![command])
            }
            Kind::Ssh {
                host,
                port,
                path,
                command,
            } => {
                let client = resolve_client(&|key| std::env::var(key).ok(), command.as_deref());
                let args = ssh_args(variant(&client), host, *port, service, path)?;
                Ok(client_commands(&client, &args))
            }
        }
    }
}

impl Connector for ProcessConnector {
    fn connect(&mut self, service: Service) -> Result<Box<dyn Session>> {
        let mut commands = self.commands(service)?;
        let count = commands.len();
        for (i, command) in commands.iter_mut().enumerate() {
            if service == Service::UploadPack && self.v2 {
                command.env("GIT_PROTOCOL", "version=2");
            }
            let mut child = match command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                // Try the next way of starting the client.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && i + 1 < count => continue,
                Err(e) => {
                    return Err(Error::Io(std::io::Error::new(
                        e.kind(),
                        format!("cannot run {:?}: {}", command, e),
                    )))
                }
            };
            let stdin = child.stdin.take();
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::Protocol("no output from the process".into()))?;
            let stderr = child.stderr.take().map(|mut stderr| {
                std::thread::spawn(move || {
                    // Show the client's messages as Git does, and keep the
                    // last of them for an error message.
                    let mut kept = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while let Ok(n) = stderr.read(&mut buffer) {
                        if n == 0 {
                            break;
                        }
                        let _ = std::io::stderr().write_all(&buffer[..n]);
                        kept.extend_from_slice(&buffer[..n]);
                        if kept.len() > STDERR_KEPT {
                            kept.drain(..kept.len() - STDERR_KEPT);
                        }
                    }
                    kept
                })
            });
            let program = command.get_program().to_string_lossy().into_owned();
            return Ok(Box::new(ProcessSession {
                child,
                stdin,
                stdout,
                stderr,
                program,
            }));
        }
        Err(Error::Protocol("no way to start the process".into()))
    }
}

struct ProcessSession {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    stderr: Option<JoinHandle<Vec<u8>>>,
    program: String,
}

impl ProcessSession {
    /// Ends the conversation and waits for the process.
    fn finish(&mut self) -> (Option<std::process::ExitStatus>, Vec<u8>) {
        // Closing standard input ends the conversation.
        self.stdin.take();
        let mut rest = Vec::new();
        let _ = self.stdout.read_to_end(&mut rest);
        let status = self.child.wait().ok();
        // The error output ends when the process exits, unless something it
        // started (an SSH connection master, say) keeps it open: wait only
        // briefly for it then.
        let stderr = match self.stderr.take() {
            Some(thread) => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
                while !thread.is_finished() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if thread.is_finished() {
                    thread.join().unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        };
        (status, stderr)
    }
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

    fn diagnostics(&mut self) -> Option<String> {
        let (status, stderr) = self.finish();
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        let failed = status.is_some_and(|s| !s.success());
        if !failed && stderr.is_empty() {
            return None;
        }
        let mut message = match status {
            Some(status) if failed => format!("{} failed ({})", self.program, status),
            _ => self.program.clone(),
        };
        if !stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&stderr);
        }
        Some(message)
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn client_is_chosen_in_git_order() {
        let both = env(&[("GIT_SSH_COMMAND", "ssh -i key"), ("GIT_SSH", "/bin/myssh")]);
        assert_eq!(
            resolve_client(&both, Some("ssh -v")),
            SshClient::Shell("ssh -i key".into())
        );
        let program = env(&[("GIT_SSH", "/bin/myssh")]);
        assert_eq!(
            resolve_client(&program, Some("ssh -v")),
            SshClient::Program("/bin/myssh".into())
        );
        assert_eq!(
            resolve_client(&env(&[]), Some("ssh -v")),
            SshClient::Shell("ssh -v".into())
        );
        assert_eq!(
            resolve_client(&env(&[("GIT_SSH_COMMAND", " ")]), None),
            SshClient::Program("ssh".into())
        );
    }

    #[test]
    fn arguments_follow_the_client_variant() {
        let openssh = ssh_args(
            Variant::OpenSsh,
            "git@example.com",
            Some(2222),
            Service::UploadPack,
            "/srv/repo.git",
        )
        .unwrap();
        assert_eq!(
            openssh,
            [
                "-o",
                "SendEnv=GIT_PROTOCOL",
                "-p",
                "2222",
                "git@example.com",
                "git-upload-pack '/srv/repo.git'"
            ]
        );
        let tortoise = ssh_args(
            Variant::TortoisePlink,
            "host",
            Some(22),
            Service::ReceivePack,
            "repo",
        )
        .unwrap();
        assert_eq!(
            tortoise,
            ["-batch", "-P", "22", "host", "git-receive-pack 'repo'"]
        );
        assert_eq!(
            variant(&SshClient::Program(r"C:\PuTTY\plink.exe".into())),
            Variant::Plink
        );
        assert_eq!(
            variant(&SshClient::Shell("tortoiseplink -v".into())),
            Variant::TortoisePlink
        );
    }

    #[test]
    fn remote_paths_are_quoted_for_the_shell() {
        let args = ssh_args(
            Variant::OpenSsh,
            "host",
            None,
            Service::UploadPack,
            "/srv/it's a repo",
        )
        .unwrap();
        assert_eq!(
            args.last().unwrap(),
            r"git-upload-pack '/srv/it'\''s a repo'"
        );
    }

    #[test]
    fn option_injection_is_blocked() {
        for (host, path) in [("-oProxyCommand=evil", "repo"), ("host", "-repo")] {
            assert!(matches!(
                ssh_args(Variant::OpenSsh, host, None, Service::UploadPack, path),
                Err(Error::UnsupportedUrl(reason)) if reason.contains("blocked")
            ));
        }
    }

    #[test]
    fn command_lines_run_through_the_shell() {
        assert!(!needs_shell("ssh"));
        assert!(!needs_shell("/usr/bin/ssh"));
        assert!(needs_shell("ssh -i key"));
        let args = vec!["host".to_owned(), "git-upload-pack 'x'".to_owned()];
        let commands = client_commands(&SshClient::Shell("ssh -i 'my key'".into()), &args);
        assert_eq!(commands[0].get_program(), "sh");
        let shell_args: Vec<_> = commands[0].get_args().collect();
        assert_eq!(shell_args[0], "-c");
        assert_eq!(shell_args[1], "ssh -i 'my key' \"$@\"");
        assert_eq!(shell_args[3], "host");
    }

    #[cfg(unix)]
    #[test]
    fn client_failures_are_reported_with_its_messages() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::TempDir::new().unwrap();
        let fake = temp.path().join("fake-ssh");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'Permission denied (publickey).' >&2\nexit 255\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let connector = ProcessConnector::ssh("host", None, "/srv/repo.git")
            .ssh_command(fake.to_str().unwrap());
        let mut transport = super::super::GitTransport::new(connector);
        let error = super::super::Transport::list_refs(&mut transport, &[]).unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, Error::Connection(_)), "{:?}", error);
        assert!(
            message.contains("Permission denied (publickey)."),
            "{}",
            message
        );
        assert!(message.contains("255"), "{}", message);
    }
}
