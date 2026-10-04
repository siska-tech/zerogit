//! Helpers for comparing remote operations with Git.

#![allow(dead_code)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub fn git_output(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap()
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A non-bare repository on `main` with some history, branches and tags.
pub fn origin(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    for i in 0..5 {
        write(dir, "doc.txt", &"line\n".repeat(i + 1));
        write(dir, &format!("dir/f{}.txt", i), &format!("{}\n", i));
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Commit {}", i)]);
    }
    git(dir, &["tag", "-a", "-m", "Release", "v1", "HEAD~1"]);
    git(dir, &["tag", "light", "HEAD~2"]);
    git(dir, &["branch", "topic", "HEAD~3"]);
}

/// A bare copy of `origin`.
pub fn bare_origin(root: &Path) -> PathBuf {
    let work = root.join("work");
    origin(&work);
    let bare = root.join("origin.git");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    bare
}

/// `refname oid` lines of every reference.
pub fn refs(dir: &Path) -> String {
    git(dir, &["for-each-ref", "--format=%(refname) %(objectname)"])
}

/// Files of a work tree (excluding .git) with contents.
pub fn worktree_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                out.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(&path).unwrap(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Serves repositories under `root` over HTTP by running `git http-backend`
/// as a CGI program, one request per connection. Returns the base URL.
pub fn http_server(root: &Path) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = serve(stream, &root);
            });
        }
    });
    format!("http://{}", address)
}

fn serve(stream: TcpStream, root: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));

    let mut command = Command::new("git");
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("REQUEST_METHOD", &method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("CONTENT_LENGTH", length.to_string())
        .env("REMOTE_USER", "tester")
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(content_type) = header("content-type") {
        command.env("CONTENT_TYPE", content_type);
    }
    if let Some(protocol) = header("git-protocol") {
        command.env("GIT_PROTOCOL", protocol);
    }
    let mut child = command.spawn()?;
    child.stdin.take().unwrap().write_all(&body)?;
    let output = child.wait_with_output()?;

    // CGI output: headers, a blank line, the body.
    let out = output.stdout;
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, p + 4))
        .or_else(|| {
            out.windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, p + 2))
        })
        .unwrap_or((out.len(), out.len()));
    let cgi_headers = String::from_utf8_lossy(&out[..split.0]).into_owned();
    let cgi_body = &out[split.1..];
    let mut status = "200 OK".to_owned();
    let mut response_headers = String::new();
    for line in cgi_headers.lines() {
        if let Some(value) = line.strip_prefix("Status:") {
            status = value.trim().to_owned();
        } else if !line.is_empty() {
            response_headers.push_str(line);
            response_headers.push_str("\r\n");
        }
    }
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        response_headers,
        cgi_body.len()
    )?;
    stream.write_all(cgi_body)?;
    stream.flush()
}

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}
