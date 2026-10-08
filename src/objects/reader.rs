//! Reading an object's content as a stream ([`ObjectReader`]).
//!
//! Loose objects and objects stored whole in a pack are inflated as they
//! are read, so that reading a large blob uses a small, fixed amount of
//! memory. The object ID (and for a pack entry, its CRC-32) is checked
//! when the end is reached; a mismatch is an error then. Objects stored as
//! deltas are rebuilt in memory, within the pack limits, as before.

use std::fs::File;
use std::io::{self, Cursor, Read};

use super::{ObjectType, Oid};
use crate::infra::hash::{Crc32State, Sha1State};

fn corrupt(oid: &Oid, reason: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("object {}: {}", oid.to_hex(), reason.into()),
    )
}

/// Inflates a zlib stream read from `source` as it is read.
pub(crate) struct Inflater<R> {
    source: R,
    state: Box<miniz_oxide::inflate::stream::InflateState>,
    buffer: Vec<u8>,
    pos: usize,
    len: usize,
    source_done: bool,
    finished: bool,
}

impl<R: Read> Inflater<R> {
    pub(crate) fn new(source: R) -> Self {
        Inflater {
            source,
            state: miniz_oxide::inflate::stream::InflateState::new_boxed(
                miniz_oxide::DataFormat::Zlib,
            ),
            buffer: vec![0; 64 << 10],
            pos: 0,
            len: 0,
            source_done: false,
            finished: false,
        }
    }

    /// Whether the zlib stream has ended.
    pub(crate) fn finished(&self) -> bool {
        self.finished
    }

    /// The source, once the stream is read (what is left of it unread).
    pub(crate) fn into_source(self) -> R {
        self.source
    }
}

impl<R: Read> Read for Inflater<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        use miniz_oxide::inflate::stream::inflate;
        use miniz_oxide::{MZFlush, MZStatus};
        if self.finished || out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pos == self.len && !self.source_done {
                self.len = self.source.read(&mut self.buffer)?;
                self.pos = 0;
                self.source_done = self.len == 0;
            }
            let result = inflate(
                &mut self.state,
                &self.buffer[self.pos..self.len],
                out,
                MZFlush::None,
            );
            self.pos += result.bytes_consumed;
            match result.status {
                Ok(MZStatus::StreamEnd) => {
                    self.finished = true;
                    return Ok(result.bytes_written);
                }
                Ok(_) if result.bytes_written > 0 => return Ok(result.bytes_written),
                Ok(_) if result.bytes_consumed > 0 => {}
                Ok(_) | Err(_) if self.source_done => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated zlib stream",
                    ))
                }
                Ok(_) => {}
                Err(miniz_oxide::MZError::Buf) => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "corrupt zlib stream",
                    ))
                }
            }
        }
    }
}

/// Passes reads through, keeping a CRC-32 of what was read.
pub(crate) struct CrcReader<R> {
    inner: R,
    crc: Crc32State,
}

impl<R: Read> CrcReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        CrcReader {
            inner,
            crc: Crc32State::new(),
        }
    }

    /// The CRC-32 of what was read so far.
    pub(crate) fn crc(&self) -> u32 {
        self.crc.finish()
    }
}

impl<R: Read> Read for CrcReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        self.crc.update(&out[..n]);
        Ok(n)
    }
}

/// The source of a streamed object.
pub(crate) enum Source {
    /// A loose object file (its header already read).
    Loose(Inflater<File>),
    /// A pack entry stored whole: the rest of the entry, read through a
    /// CRC check against `crc`.
    Packed {
        inflater: Inflater<CrcReader<io::Take<File>>>,
        crc: u32,
    },
}

/// A streamed object's content, checked against its ID at the end.
struct Stream {
    oid: Oid,
    source: Option<Source>,
    remaining: u64,
    sha1: Sha1State,
}

impl Stream {
    /// Checks what was read once the content is complete.
    fn finish(&mut self) -> io::Result<()> {
        let oid = self.oid;
        let sha1 = std::mem::replace(&mut self.sha1, Sha1State::new()).finalize();
        let mut extra = [0u8; 1];
        match self.source.take() {
            Some(Source::Loose(mut inflater)) => {
                if inflater.read(&mut extra)? != 0 {
                    return Err(corrupt(&oid, "longer than its declared size"));
                }
            }
            Some(Source::Packed { mut inflater, crc }) => {
                if inflater.read(&mut extra)? != 0 {
                    return Err(corrupt(&oid, "longer than its declared size"));
                }
                if !inflater.finished() {
                    return Err(corrupt(&oid, "truncated zlib stream"));
                }
                // The CRC covers the whole entry, the zlib trailer too.
                let mut rest = inflater.into_source();
                io::copy(&mut rest, &mut io::sink())?;
                if rest.crc.finish() != crc {
                    return Err(corrupt(&oid, "CRC32 mismatch"));
                }
            }
            None => return Ok(()),
        }
        if Oid::from_bytes(sha1) != oid {
            return Err(corrupt(&oid, "content does not match the object ID"));
        }
        Ok(())
    }
}

impl Read for Stream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            self.finish()?;
            return Ok(0);
        }
        let want = out
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = match self.source.as_mut() {
            Some(Source::Loose(inflater)) => inflater.read(&mut out[..want])?,
            Some(Source::Packed { inflater, .. }) => inflater.read(&mut out[..want])?,
            None => 0,
        };
        if n == 0 {
            return Err(corrupt(&self.oid, "shorter than its declared size"));
        }
        self.sha1.update(&out[..n]);
        self.remaining -= n as u64;
        Ok(n)
    }
}

enum Inner {
    Memory(Cursor<Vec<u8>>),
    Stream(Box<Stream>),
}

/// An object's content, read as a stream: see [`Repository::object_reader`].
///
/// Reading fails with an [`io::ErrorKind::InvalidData`] error when the
/// content turns out not to match the object ID (or is corrupt), which can
/// only be known once all of it is read.
///
/// [`Repository::object_reader`]: crate::Repository::object_reader
pub struct ObjectReader {
    object_type: ObjectType,
    size: u64,
    inner: Inner,
}

impl ObjectReader {
    /// A reader over content already in memory.
    pub(crate) fn from_memory(object_type: ObjectType, content: Vec<u8>) -> Self {
        ObjectReader {
            object_type,
            size: content.len() as u64,
            inner: Inner::Memory(Cursor::new(content)),
        }
    }

    /// A reader inflating `source`, whose content of `size` bytes must hash
    /// to `oid`.
    pub(crate) fn streamed(oid: Oid, object_type: ObjectType, size: u64, source: Source) -> Self {
        let mut sha1 = Sha1State::new();
        sha1.update(format!("{} {}\0", object_type.as_str(), size).as_bytes());
        ObjectReader {
            object_type,
            size,
            inner: Inner::Stream(Box::new(Stream {
                oid,
                source: Some(source),
                remaining: size,
                sha1,
            })),
        }
    }

    /// The object's type.
    pub fn object_type(&self) -> ObjectType {
        self.object_type
    }

    /// The size of the content in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Whether the content is inflated as it is read (rather than held in
    /// memory).
    pub fn is_streamed(&self) -> bool {
        matches!(self.inner, Inner::Stream(_))
    }
}

impl Read for ObjectReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            Inner::Memory(cursor) => cursor.read(out),
            Inner::Stream(stream) => stream.read(out),
        }
    }
}

impl std::fmt::Debug for ObjectReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectReader")
            .field("object_type", &self.object_type)
            .field("size", &self.size)
            .field("streamed", &self.is_streamed())
            .finish()
    }
}
