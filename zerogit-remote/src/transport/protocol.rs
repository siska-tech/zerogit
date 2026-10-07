//! The Git protocol over a connection: protocol v2 (with a fallback to the
//! original protocol) for fetching, and the receive-pack protocol for
//! pushing.

use std::io::Read;

use zerogit::{Oid, PackObjectsOptions};

use super::{PushCommand, PushReply, RemoteRef, Transport};
use crate::error::{Error, Result};
use crate::pktline::{self, Packet, PacketReader};

const AGENT: &str = concat!("agent=zerogit-remote/", env!("CARGO_PKG_VERSION"));

/// The service a connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Fetching (`git-upload-pack`).
    UploadPack,
    /// Pushing (`git-receive-pack`).
    ReceivePack,
}

impl Service {
    /// The program / endpoint name.
    pub fn name(self) -> &'static str {
        match self {
            Service::UploadPack => "git-upload-pack",
            Service::ReceivePack => "git-receive-pack",
        }
    }
}

/// A connection to one service of a server.
///
/// [`Session::reader`] first yields the server's advertisement, then the
/// response to each request given to [`Session::send`]. A stateful
/// connection (a process) keeps one stream; a stateless one (HTTP) makes a
/// request per [`Session::send`].
pub trait Session {
    /// The stream from the server.
    fn reader(&mut self) -> &mut dyn Read;
    /// Sends a request.
    fn send(&mut self, request: &[u8]) -> Result<()>;
    /// Ends the connection and describes why it failed, if the other side
    /// reported anything (for a process: its exit status and error output).
    /// Used to explain a connection that ended unexpectedly.
    fn diagnostics(&mut self) -> Option<String> {
        None
    }
}

/// Opens connections to a server.
pub trait Connector {
    /// Connects to a service.
    fn connect(&mut self, service: Service) -> Result<Box<dyn Session>>;
}

/// What the server advertised when the connection opened.
enum Advertisement {
    /// Protocol v2: the capabilities.
    V2(Vec<String>),
    /// The original protocol: the references and capabilities.
    V0 {
        refs: Vec<RemoteRef>,
        capabilities: Vec<String>,
    },
}

fn parse_oid(text: &str) -> Result<Oid> {
    Oid::from_hex(text).map_err(|_| Error::Protocol(format!("invalid object ID {:?}", text)))
}

fn zero() -> Oid {
    Oid::from_bytes([0; 20])
}

/// Reads the advertisement of a new connection. If the connection ends
/// before it (an SSH client that cannot log in, a server that refuses the
/// repository), the error says what the other side reported.
fn open_session(session: &mut dyn Session) -> Result<Advertisement> {
    let result = {
        let mut reader = PacketReader::new(session.reader());
        read_advertisement(&mut reader)
    };
    match result {
        Ok(advertisement) => Ok(advertisement),
        Err(e @ (Error::Protocol(_) | Error::Io(_))) => match session.diagnostics() {
            Some(details) => Err(Error::Connection(format!("{} ({})", details, e))),
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

/// Reads the advertisement, skipping the `# service=...` preamble of smart
/// HTTP.
fn read_advertisement(reader: &mut PacketReader<&mut dyn Read>) -> Result<Advertisement> {
    let mut first = reader.expect()?;
    if first.text().is_some_and(|t| t.starts_with("# service=")) {
        // The preamble ends with a flush.
        loop {
            if reader.expect()? == Packet::Flush {
                break;
            }
        }
        first = reader.expect()?;
    }
    if first.text().as_deref() == Some("version 2") {
        let mut capabilities = Vec::new();
        loop {
            match reader.expect()? {
                Packet::Flush => return Ok(Advertisement::V2(capabilities)),
                packet => capabilities.extend(packet.text()),
            }
        }
    }
    // The original protocol (also when a server answers "version 1").
    let mut packet = first;
    if packet.text().as_deref() == Some("version 1") {
        packet = reader.expect()?;
    }
    let mut refs: Vec<RemoteRef> = Vec::new();
    let mut capabilities = Vec::new();
    loop {
        let data = match packet {
            Packet::Flush => break,
            Packet::Data(data) => data,
            _ => return Err(Error::Protocol("unexpected packet in advertisement".into())),
        };
        let (line, caps) = match data.iter().position(|&b| b == 0) {
            Some(pos) => (&data[..pos], Some(&data[pos + 1..])),
            None => (&data[..], None),
        };
        if let Some(caps) = caps {
            capabilities = String::from_utf8_lossy(caps)
                .trim_end()
                .split(' ')
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect();
        }
        let line = String::from_utf8_lossy(line).trim_end().to_owned();
        let (oid, name) = line
            .split_once(' ')
            .ok_or_else(|| Error::Protocol(format!("invalid ref line {:?}", line)))?;
        let oid = parse_oid(oid)?;
        if name == "capabilities^{}" {
            // An empty repository advertises only capabilities.
        } else if let Some(base) = name.strip_suffix("^{}") {
            if let Some(previous) = refs.last_mut().filter(|r| r.name == base) {
                previous.peeled = Some(oid);
            }
        } else {
            refs.push(RemoteRef {
                name: name.to_owned(),
                oid,
                peeled: None,
                symref_target: None,
            });
        }
        packet = match reader.read()? {
            Some(packet) => packet,
            None => break,
        };
    }
    // Symbolic references are announced as capabilities.
    for cap in &capabilities {
        if let Some((name, target)) = cap.strip_prefix("symref=").and_then(|s| s.split_once(':')) {
            if let Some(r) = refs.iter_mut().find(|r| r.name == name) {
                r.symref_target = Some(target.to_owned());
            }
        }
    }
    Ok(Advertisement::V0 { refs, capabilities })
}

/// A receive-pack connection with its advertised references and
/// capabilities.
type ReceiveSession = (Box<dyn Session>, Vec<RemoteRef>, Vec<String>);

/// A transport speaking the Git protocol through a [`Connector`].
pub struct GitTransport<C: Connector> {
    connector: C,
    upload: Option<(Box<dyn Session>, Advertisement)>,
    receive: Option<ReceiveSession>,
}

impl<C: Connector> GitTransport<C> {
    /// Creates a transport; connections open when first needed.
    pub fn new(connector: C) -> Self {
        GitTransport {
            connector,
            upload: None,
            receive: None,
        }
    }

    fn upload(&mut self) -> Result<&mut (Box<dyn Session>, Advertisement)> {
        if self.upload.is_none() {
            let mut session = self.connector.connect(Service::UploadPack)?;
            let advertisement = open_session(session.as_mut())?;
            self.upload = Some((session, advertisement));
        }
        Ok(self.upload.as_mut().unwrap())
    }

    fn receive(&mut self) -> Result<&mut ReceiveSession> {
        if self.receive.is_none() {
            let mut session = self.connector.connect(Service::ReceivePack)?;
            let advertisement = open_session(session.as_mut())?;
            let (refs, capabilities) = match advertisement {
                Advertisement::V0 { refs, capabilities } => (refs, capabilities),
                Advertisement::V2(_) => {
                    return Err(Error::Protocol(
                        "receive-pack answered with protocol v2".into(),
                    ))
                }
            };
            self.receive = Some((session, refs, capabilities));
        }
        Ok(self.receive.as_mut().unwrap())
    }
}

/// Builds the v2 `ls-refs` request.
fn ls_refs_request(capabilities: &[String], prefixes: &[String]) -> Vec<u8> {
    let mut request = Vec::new();
    pktline::write_line(&mut request, "command=ls-refs");
    pktline::write_line(&mut request, AGENT);
    if capabilities.iter().any(|c| c == "object-format=sha1") {
        pktline::write_line(&mut request, "object-format=sha1");
    }
    pktline::write_delimiter(&mut request);
    pktline::write_line(&mut request, "symrefs");
    pktline::write_line(&mut request, "peel");
    let unborn = capabilities.iter().any(|c| {
        c.starts_with("ls-refs=") && c["ls-refs=".len()..].split(' ').any(|f| f == "unborn")
    });
    if unborn {
        pktline::write_line(&mut request, "unborn");
    }
    if !prefixes.is_empty() {
        pktline::write_line(&mut request, "ref-prefix HEAD");
        for prefix in prefixes {
            pktline::write_line(&mut request, &format!("ref-prefix {}", prefix));
        }
    }
    pktline::write_flush(&mut request);
    request
}

fn parse_ls_refs(reader: &mut PacketReader<&mut dyn Read>) -> Result<Vec<RemoteRef>> {
    let mut refs = Vec::new();
    loop {
        let line = match reader.expect()? {
            Packet::Flush | Packet::ResponseEnd => return Ok(refs),
            packet => packet
                .text()
                .ok_or_else(|| Error::Protocol("unexpected packet in ls-refs".into()))?,
        };
        let mut words = line.split(' ');
        let oid = words.next().unwrap_or_default();
        let name = words
            .next()
            .ok_or_else(|| Error::Protocol(format!("invalid ls-refs line {:?}", line)))?;
        let mut r = RemoteRef {
            name: name.to_owned(),
            oid: if oid == "unborn" {
                zero()
            } else {
                parse_oid(oid)?
            },
            peeled: None,
            symref_target: None,
        };
        for attribute in words {
            if let Some(target) = attribute.strip_prefix("symref-target:") {
                r.symref_target = Some(target.to_owned());
            } else if let Some(peeled) = attribute.strip_prefix("peeled:") {
                r.peeled = Some(parse_oid(peeled)?);
            }
        }
        refs.push(r);
    }
}

fn matches_prefixes(name: &str, prefixes: &[String]) -> bool {
    prefixes.is_empty() || name == "HEAD" || prefixes.iter().any(|p| name.starts_with(p.as_str()))
}

impl<C: Connector> Transport for GitTransport<C> {
    fn list_refs(&mut self, prefixes: &[String]) -> Result<Vec<RemoteRef>> {
        let (session, advertisement) = self.upload()?;
        match advertisement {
            Advertisement::V2(capabilities) => {
                let request = ls_refs_request(capabilities, prefixes);
                session.send(&request)?;
                let mut reader = PacketReader::new(session.reader());
                parse_ls_refs(&mut reader)
            }
            Advertisement::V0 { refs, .. } => Ok(refs
                .iter()
                .filter(|r| matches_prefixes(&r.name, prefixes))
                .cloned()
                .collect()),
        }
    }

    fn fetch_pack(&mut self, wants: &[Oid], haves: &[Oid]) -> Result<Vec<u8>> {
        if wants.is_empty() {
            return Ok(Vec::new());
        }
        let (session, advertisement) = self.upload()?;
        match advertisement {
            Advertisement::V2(capabilities) => {
                let mut request = Vec::new();
                pktline::write_line(&mut request, "command=fetch");
                pktline::write_line(&mut request, AGENT);
                if capabilities.iter().any(|c| c == "object-format=sha1") {
                    pktline::write_line(&mut request, "object-format=sha1");
                }
                pktline::write_delimiter(&mut request);
                for feature in ["thin-pack", "ofs-delta", "no-progress", "include-tag"] {
                    pktline::write_line(&mut request, feature);
                }
                for want in wants {
                    pktline::write_line(&mut request, &format!("want {}", want));
                }
                for have in haves {
                    pktline::write_line(&mut request, &format!("have {}", have));
                }
                pktline::write_line(&mut request, "done");
                pktline::write_flush(&mut request);
                session.send(&request)?;
                let mut reader = PacketReader::new(session.reader());
                // Sections before the pack (shallow-info, wanted-refs) are
                // skipped.
                loop {
                    match reader.expect()? {
                        Packet::Data(data)
                            if data.strip_suffix(b"\n").unwrap_or(&data) == b"packfile" =>
                        {
                            return reader.read_sideband();
                        }
                        Packet::Flush | Packet::ResponseEnd => {
                            return Err(Error::Protocol("response without a packfile".into()))
                        }
                        _ => {}
                    }
                }
            }
            Advertisement::V0 { capabilities, .. } => {
                let has = |c: &str| capabilities.iter().any(|x| x == c);
                let sideband = if has("side-band-64k") {
                    Some("side-band-64k")
                } else if has("side-band") {
                    Some("side-band")
                } else {
                    None
                };
                let mut caps: Vec<&str> = Vec::new();
                caps.extend(sideband);
                for c in ["thin-pack", "ofs-delta", "no-progress", "include-tag"] {
                    if has(c) {
                        caps.push(c);
                    }
                }
                caps.push(AGENT);
                let mut request = Vec::new();
                for (i, want) in wants.iter().enumerate() {
                    if i == 0 {
                        pktline::write_line(
                            &mut request,
                            &format!("want {} {}", want, caps.join(" ")),
                        );
                    } else {
                        pktline::write_line(&mut request, &format!("want {}", want));
                    }
                }
                pktline::write_flush(&mut request);
                for have in haves {
                    pktline::write_line(&mut request, &format!("have {}", have));
                }
                pktline::write_line(&mut request, "done");
                session.send(&request)?;
                let mut reader = PacketReader::new(session.reader());
                // NAK, or ACK of a common commit, precedes the pack.
                loop {
                    let packet = reader.expect()?;
                    let text = packet.text().unwrap_or_default();
                    if text == "NAK"
                        || (text.starts_with("ACK ")
                            && !text.ends_with(" continue")
                            && !text.ends_with(" common"))
                    {
                        break;
                    }
                }
                if sideband.is_some() {
                    reader.read_sideband()
                } else {
                    let mut pack = Vec::new();
                    reader.into_inner().read_to_end(&mut pack)?;
                    Ok(pack)
                }
            }
        }
    }

    fn list_push_refs(&mut self) -> Result<Vec<RemoteRef>> {
        Ok(self.receive()?.1.clone())
    }

    fn push_pack_options(&mut self) -> Result<PackObjectsOptions> {
        let (_, _, capabilities) = self.receive()?;
        let has = |name: &str| capabilities.iter().any(|c| c == name);
        // Like `git push`: thin unless the server refuses it.
        Ok(PackObjectsOptions::new()
            .thin(!has("no-thin"))
            .ofs_delta(has("ofs-delta")))
    }

    fn push(&mut self, commands: &[PushCommand], pack: &[u8]) -> Result<Vec<PushReply>> {
        let (session, _, capabilities) = self.receive()?;
        if commands.is_empty() {
            let mut request = Vec::new();
            pktline::write_flush(&mut request);
            session.send(&request)?;
            return Ok(Vec::new());
        }
        let report = capabilities.iter().any(|c| c == "report-status");
        let mut caps = Vec::new();
        if report {
            caps.push("report-status");
        }
        if capabilities.iter().any(|c| c == "ofs-delta") {
            caps.push("ofs-delta");
        }
        caps.push(AGENT);
        let hex = |oid: &Option<Oid>| oid.unwrap_or_else(zero).to_hex();
        let mut request = Vec::new();
        for (i, command) in commands.iter().enumerate() {
            let line = format!(
                "{} {} {}",
                hex(&command.old),
                hex(&command.new),
                command.name
            );
            if i == 0 {
                let mut data = line.into_bytes();
                data.push(0);
                data.extend_from_slice(caps.join(" ").as_bytes());
                data.push(b'\n');
                pktline::write_data(&mut request, &data);
            } else {
                pktline::write_line(&mut request, &line);
            }
        }
        pktline::write_flush(&mut request);
        if commands.iter().any(|c| c.new.is_some()) {
            request.extend_from_slice(pack);
        }
        session.send(&request)?;
        if !report {
            return Ok(commands
                .iter()
                .map(|c| PushReply {
                    name: c.name.clone(),
                    error: None,
                })
                .collect());
        }
        let mut reader = PacketReader::new(session.reader());
        let unpack = reader
            .expect()?
            .text()
            .ok_or_else(|| Error::Protocol("missing unpack status".into()))?;
        if unpack != "unpack ok" {
            return Err(Error::Remote(unpack));
        }
        let mut replies = Vec::new();
        loop {
            match reader.read()? {
                None | Some(Packet::Flush) => break,
                Some(packet) => {
                    let line = packet.text().unwrap_or_default();
                    if let Some(name) = line.strip_prefix("ok ") {
                        replies.push(PushReply {
                            name: name.to_owned(),
                            error: None,
                        });
                    } else if let Some(rest) = line.strip_prefix("ng ") {
                        let (name, reason) = rest.split_once(' ').unwrap_or((rest, "rejected"));
                        replies.push(PushReply {
                            name: name.to_owned(),
                            error: Some(reason.to_owned()),
                        });
                    }
                }
            }
        }
        Ok(replies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v0_advertisement() {
        let mut data = Vec::new();
        pktline::write_line(&mut data, "# service=git-upload-pack");
        pktline::write_flush(&mut data);
        let a = "1".repeat(40);
        let b = "2".repeat(40);
        let mut first = format!("{} HEAD", a).into_bytes();
        first.push(0);
        first.extend_from_slice(b"multi_ack side-band-64k symref=HEAD:refs/heads/main\n");
        pktline::write_data(&mut data, &first);
        pktline::write_line(&mut data, &format!("{} refs/heads/main", a));
        pktline::write_line(&mut data, &format!("{} refs/tags/v1", b));
        pktline::write_line(&mut data, &format!("{} refs/tags/v1^{{}}", a));
        pktline::write_flush(&mut data);
        let mut stream: &[u8] = &data;
        let mut reader = PacketReader::new(&mut stream as &mut dyn Read);
        let Advertisement::V0 { refs, capabilities } = read_advertisement(&mut reader).unwrap()
        else {
            panic!("expected v0");
        };
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].symref_target.as_deref(), Some("refs/heads/main"));
        assert_eq!(refs[2].peeled, Some(parse_oid(&a).unwrap()));
        assert!(capabilities.iter().any(|c| c == "side-band-64k"));
    }
}
