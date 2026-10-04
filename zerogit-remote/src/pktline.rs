//! pkt-line framing (`gitprotocol-common`).
//!
//! Each packet is a 4-digit hexadecimal length (including the 4 digits)
//! followed by the data. `0000` is a flush packet, `0001` a delimiter and
//! `0002` a response-end packet (protocol v2).

use std::io::Read;

use crate::error::{Error, Result};

/// The largest data in one packet.
pub const MAX_DATA: usize = 65516;

/// One packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Data (for text lines, usually ending in `\n`).
    Data(Vec<u8>),
    /// `0000`
    Flush,
    /// `0001`
    Delimiter,
    /// `0002`
    ResponseEnd,
}

impl Packet {
    /// The data as text without a trailing newline, if it is a data packet.
    pub fn text(&self) -> Option<String> {
        match self {
            Packet::Data(data) => {
                let data = data.strip_suffix(b"\n").unwrap_or(data);
                Some(String::from_utf8_lossy(data).into_owned())
            }
            _ => None,
        }
    }
}

/// Appends a data packet.
pub fn write_data(out: &mut Vec<u8>, data: &[u8]) {
    for chunk in data.chunks(MAX_DATA) {
        out.extend_from_slice(format!("{:04x}", chunk.len() + 4).as_bytes());
        out.extend_from_slice(chunk);
    }
}

/// Appends a text line packet (a newline is added).
pub fn write_line(out: &mut Vec<u8>, line: &str) {
    let mut data = line.as_bytes().to_vec();
    data.push(b'\n');
    write_data(out, &data);
}

/// Appends a flush packet.
pub fn write_flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

/// Appends a delimiter packet.
pub fn write_delimiter(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0001");
}

/// Reads packets from a stream.
pub struct PacketReader<R> {
    inner: R,
}

impl<R: Read> PacketReader<R> {
    /// Wraps a stream.
    pub fn new(inner: R) -> Self {
        PacketReader { inner }
    }

    /// Reads the next packet, or `None` at the end of the stream.
    pub fn read(&mut self) -> Result<Option<Packet>> {
        let mut length = [0u8; 4];
        let mut filled = 0;
        while filled < 4 {
            let n = self.inner.read(&mut length[filled..])?;
            if n == 0 {
                if filled == 0 {
                    return Ok(None);
                }
                return Err(Error::Protocol("truncated packet length".into()));
            }
            filled += n;
        }
        let text = std::str::from_utf8(&length)
            .map_err(|_| Error::Protocol("invalid packet length".into()))?;
        let len = usize::from_str_radix(text, 16)
            .map_err(|_| Error::Protocol(format!("invalid packet length {:?}", text)))?;
        match len {
            0 => Ok(Some(Packet::Flush)),
            1 => Ok(Some(Packet::Delimiter)),
            2 => Ok(Some(Packet::ResponseEnd)),
            3 => Err(Error::Protocol("invalid packet length 3".into())),
            len => {
                let mut data = vec![0u8; len - 4];
                self.inner
                    .read_exact(&mut data)
                    .map_err(|_| Error::Protocol("truncated packet".into()))?;
                if data.starts_with(b"ERR ") {
                    let message = String::from_utf8_lossy(&data[4..]).trim_end().to_owned();
                    return Err(Error::Remote(message));
                }
                Ok(Some(Packet::Data(data)))
            }
        }
    }

    /// Reads a packet, failing at the end of the stream.
    pub fn expect(&mut self) -> Result<Packet> {
        self.read()?
            .ok_or_else(|| Error::Protocol("unexpected end of stream".into()))
    }

    /// Reads side-band packets until a flush, returning band 1 (data).
    /// Band 2 (progress) is dropped; band 3 is an error.
    pub fn read_sideband(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        loop {
            match self.read()? {
                None | Some(Packet::Flush) | Some(Packet::ResponseEnd) => return Ok(data),
                Some(Packet::Delimiter) => {
                    return Err(Error::Protocol("delimiter in side-band data".into()))
                }
                Some(Packet::Data(packet)) => match packet.first() {
                    Some(1) => data.extend_from_slice(&packet[1..]),
                    Some(2) => {}
                    Some(3) => {
                        return Err(Error::Remote(
                            String::from_utf8_lossy(&packet[1..]).trim_end().to_owned(),
                        ))
                    }
                    _ => return Err(Error::Protocol("invalid side-band packet".into())),
                },
            }
        }
    }

    /// The underlying stream.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let mut out = Vec::new();
        write_line(&mut out, "version 2");
        write_delimiter(&mut out);
        write_data(&mut out, b"\x01pack");
        write_flush(&mut out);
        assert_eq!(&out[..14], b"000eversion 2\n");
        let mut reader = PacketReader::new(&out[..]);
        assert_eq!(reader.expect().unwrap().text().unwrap(), "version 2");
        assert_eq!(reader.expect().unwrap(), Packet::Delimiter);
        assert_eq!(reader.expect().unwrap(), Packet::Data(b"\x01pack".to_vec()));
        assert_eq!(reader.expect().unwrap(), Packet::Flush);
        assert_eq!(reader.read().unwrap(), None);
    }

    #[test]
    fn test_sideband_and_errors() {
        let mut out = Vec::new();
        write_data(&mut out, b"\x02progress\n");
        write_data(&mut out, b"\x01ab");
        write_data(&mut out, b"\x01cd");
        write_flush(&mut out);
        assert_eq!(
            PacketReader::new(&out[..]).read_sideband().unwrap(),
            b"abcd"
        );
        let mut err = Vec::new();
        write_line(&mut err, "ERR access denied");
        assert!(matches!(
            PacketReader::new(&err[..]).read(),
            Err(Error::Remote(m)) if m == "access denied"
        ));
    }
}
