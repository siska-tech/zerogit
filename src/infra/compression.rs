//! Zlib compression and decompression utilities.

use crate::error::{Error, Result};

/// Compresses data using zlib.
///
/// This function compresses the input data using the DEFLATE algorithm
/// with zlib wrapper (header and checksum).
///
/// # Arguments
///
/// * `data` - The data to compress.
///
/// # Returns
///
/// The compressed data as a byte vector.
pub fn compress(data: &[u8]) -> Vec<u8> {
    // Use compression level 6 (default, good balance of speed and size)
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// Zlib compression of data given in pieces, written to `W` as it is
/// produced; [`Deflater::finish`] ends the stream. The output is the same
/// as [`compress`] gives for all the data at once.
pub(crate) struct Deflater<W: std::io::Write> {
    compressor: Box<miniz_oxide::deflate::core::CompressorOxide>,
    buffer: Vec<u8>,
    inner: W,
}

impl<W: std::io::Write> Deflater<W> {
    pub(crate) fn new(inner: W) -> Self {
        use miniz_oxide::deflate::core::{create_comp_flags_from_zip_params, CompressorOxide};
        let flags = create_comp_flags_from_zip_params(6, 1, 0);
        Deflater {
            compressor: Box::new(CompressorOxide::new(flags)),
            buffer: vec![0; 64 << 10],
            inner,
        }
    }

    /// Compresses `input`, or with `finish` set ends the stream after it.
    fn run(&mut self, mut input: &[u8], finish: bool) -> std::io::Result<()> {
        use miniz_oxide::deflate::core::{compress as deflate, TDEFLFlush, TDEFLStatus};
        let flush = if finish {
            TDEFLFlush::Finish
        } else {
            TDEFLFlush::None
        };
        loop {
            let (status, consumed, written) =
                deflate(&mut self.compressor, input, &mut self.buffer, flush);
            input = &input[consumed..];
            self.inner.write_all(&self.buffer[..written])?;
            match status {
                TDEFLStatus::Done => return Ok(()),
                TDEFLStatus::Okay if input.is_empty() && !finish && written < self.buffer.len() => {
                    return Ok(())
                }
                TDEFLStatus::Okay => {}
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "zlib compression failed",
                    ))
                }
            }
        }
    }

    /// Ends the stream and returns the writer it went to.
    pub(crate) fn finish(mut self) -> std::io::Result<W> {
        self.run(&[], true)?;
        Ok(self.inner)
    }
}

impl<W: std::io::Write> std::io::Write for Deflater<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.run(data, false)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Decompresses zlib-compressed data.
///
/// This function validates the zlib header and decompresses the data using
/// the DEFLATE algorithm.
///
/// # Arguments
///
/// * `data` - The zlib-compressed data to decompress.
///
/// # Returns
///
/// The decompressed data on success, or `Error::DecompressionFailed` on failure.
///
/// # Errors
///
/// Returns `Error::DecompressionFailed` if:
/// - The input data is empty
/// - The zlib header is invalid
/// - The compressed data is corrupted or truncated
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    // Check for empty data
    if data.is_empty() {
        return Err(Error::DecompressionFailed);
    }

    // Validate zlib header (minimum 2 bytes)
    if data.len() < 2 {
        return Err(Error::DecompressionFailed);
    }

    // Validate zlib header format
    // First byte: CMF (Compression Method and Flags)
    //   - bits 0-3: CM (Compression Method) - must be 8 for DEFLATE
    //   - bits 4-7: CINFO (Compression Info) - window size
    // Second byte: FLG (Flags)
    //   - The CMF and FLG bytes must satisfy: (CMF * 256 + FLG) % 31 == 0
    if !is_valid_zlib_header(data[0], data[1]) {
        return Err(Error::DecompressionFailed);
    }

    // Decompress using miniz_oxide
    miniz_oxide::inflate::decompress_to_vec_zlib(data).map_err(|_| Error::DecompressionFailed)
}

/// Decompresses one zlib stream that must occupy all of `data` and inflate to
/// exactly `size` bytes.
///
/// The output is never allowed to grow past `size`, so the caller bounds the
/// allocation by validating the declared size first.
///
/// # Errors
///
/// Returns `Error::DecompressionFailed` if the stream is corrupt or truncated,
/// inflates to a different size, or is followed by trailing bytes.
pub fn decompress_exact(data: &[u8], size: usize) -> Result<Vec<u8>> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut output = vec![0u8; size];
    let mut overflow = [0u8; 1];
    let mut input = data;
    let mut written = 0;
    loop {
        let target: &mut [u8] = if written < size {
            &mut output[written..]
        } else {
            &mut overflow
        };
        let result = inflate(&mut state, input, target, MZFlush::None);
        if written == size && result.bytes_written > 0 {
            return Err(Error::DecompressionFailed);
        }
        input = &input[result.bytes_consumed..];
        written += result.bytes_written;
        match result.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if result.bytes_consumed > 0 || result.bytes_written > 0 => {}
            _ => return Err(Error::DecompressionFailed),
        }
    }
    if written != size || !input.is_empty() {
        return Err(Error::DecompressionFailed);
    }
    Ok(output)
}

/// Decompresses the zlib stream at the start of `data`, which must inflate to
/// exactly `size` bytes, and returns the output with the number of input
/// bytes the stream used. Bytes after the stream are left alone (as in a
/// pack, where the next entry follows).
///
/// # Errors
///
/// Returns `Error::DecompressionFailed` if the stream is corrupt, truncated
/// or inflates to a different size.
pub fn decompress_prefix(data: &[u8], size: usize) -> Result<(Vec<u8>, usize)> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut output = vec![0u8; size];
    let mut overflow = [0u8; 1];
    let mut consumed = 0;
    let mut written = 0;
    loop {
        let target: &mut [u8] = if written < size {
            &mut output[written..]
        } else {
            &mut overflow
        };
        let result = inflate(&mut state, &data[consumed..], target, MZFlush::None);
        if written == size && result.bytes_written > 0 {
            return Err(Error::DecompressionFailed);
        }
        consumed += result.bytes_consumed;
        written += result.bytes_written;
        match result.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if result.bytes_consumed > 0 || result.bytes_written > 0 => {}
            _ => return Err(Error::DecompressionFailed),
        }
    }
    if written != size {
        return Err(Error::DecompressionFailed);
    }
    Ok((output, consumed))
}

/// Inflates the start of the zlib stream in `data`: at most `max` bytes,
/// fewer if the stream ends first. Used to read headers without inflating
/// whole objects; the rest of the stream is not checked.
///
/// # Errors
///
/// Returns `Error::DecompressionFailed` if the stream is corrupt or
/// truncated before `max` bytes.
pub(crate) fn inflate_head(data: &[u8], max: usize) -> Result<Vec<u8>> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut output = vec![0u8; max];
    let mut consumed = 0;
    let mut written = 0;
    while written < max {
        let result = inflate(
            &mut state,
            &data[consumed..],
            &mut output[written..],
            MZFlush::None,
        );
        consumed += result.bytes_consumed;
        written += result.bytes_written;
        match result.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if result.bytes_consumed > 0 || result.bytes_written > 0 => {}
            _ => return Err(Error::DecompressionFailed),
        }
    }
    output.truncate(written);
    Ok(output)
}

/// Validates a zlib header.
///
/// A valid zlib header consists of two bytes where:
/// - The compression method (low 4 bits of first byte) is 8 (DEFLATE)
/// - The window size (high 4 bits of first byte) is at most 7
/// - The checksum: (CMF * 256 + FLG) % 31 == 0
fn is_valid_zlib_header(cmf: u8, flg: u8) -> bool {
    // Check compression method is DEFLATE (8)
    let compression_method = cmf & 0x0F;
    if compression_method != 8 {
        return false;
    }

    // Check window size (CINFO) is valid (0-7 for DEFLATE)
    let window_size = (cmf >> 4) & 0x0F;
    if window_size > 7 {
        return false;
    }

    // Validate checksum
    let check = (cmf as u16) * 256 + (flg as u16);
    check % 31 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflater_matches_compress_in_any_pieces() {
        use std::io::Write;
        let data: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 % 7)
            .collect();
        for piece in [1, 1000, 70_000, data.len()] {
            let mut deflater = Deflater::new(Vec::new());
            for chunk in data.chunks(piece) {
                deflater.write_all(chunk).unwrap();
            }
            let out = deflater.finish().unwrap();
            assert_eq!(decompress(&out).unwrap(), data, "{}", piece);
        }
        let empty = Deflater::new(Vec::new()).finish().unwrap();
        assert_eq!(decompress(&empty).unwrap(), b"");
    }

    #[test]
    fn test_decompress_prefix_reports_consumed_bytes() {
        let first = compress(b"hello world");
        let mut data = first.clone();
        data.extend_from_slice(b"trailing");
        let (out, used) = decompress_prefix(&data, 11).unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(used, first.len());
        assert!(decompress_prefix(&data, 5).is_err());
        assert!(decompress_prefix(&first[..first.len() - 2], 11).is_err());
    }

    // Helper to create valid zlib-compressed data
    fn compress_data(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    #[test]
    fn test_decompress_exact() {
        let original: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        let compressed = compress_data(&original);
        assert_eq!(
            decompress_exact(&compressed, original.len()).unwrap(),
            original
        );
        assert_eq!(decompress_exact(&compress_data(b""), 0).unwrap(), b"");

        for size in [0, original.len() - 1, original.len() + 1] {
            assert!(
                decompress_exact(&compressed, size).is_err(),
                "size {}",
                size
            );
        }
        let mut trailing = compressed.clone();
        trailing.push(0);
        assert!(decompress_exact(&trailing, original.len()).is_err());
        for len in 0..compressed.len() {
            assert!(decompress_exact(&compressed[..len], original.len()).is_err());
        }
        let mut corrupt = compressed;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1; // Adler-32 checksum
        assert!(decompress_exact(&corrupt, original.len()).is_err());
    }

    // C-001: Normal decompression
    #[test]
    fn test_decompress_valid_data() {
        let original = b"Hello, World!";
        let compressed = compress_data(original);

        let decompressed = decompress(&compressed).expect("decompression should succeed");
        assert_eq!(decompressed, original);
    }

    // C-001: Decompression of larger data
    #[test]
    fn test_decompress_larger_data() {
        let original: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let compressed = compress_data(&original);

        let decompressed = decompress(&compressed).expect("decompression should succeed");
        assert_eq!(decompressed, original);
    }

    // C-002: Corrupted data error
    #[test]
    fn test_decompress_corrupted_data() {
        let original = b"Hello, World!";
        let mut compressed = compress_data(original);

        // Corrupt the data by modifying some bytes in the middle
        if compressed.len() > 5 {
            compressed[4] ^= 0xFF;
            compressed[5] ^= 0xFF;
        }

        let result = decompress(&compressed);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // C-003: Empty data error
    #[test]
    fn test_decompress_empty_data() {
        let result = decompress(&[]);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // C-004: Truncated data error
    #[test]
    fn test_decompress_truncated_data() {
        let original = b"Hello, World!";
        let compressed = compress_data(original);

        // Truncate to just the header
        let truncated = &compressed[..2];
        let result = decompress(truncated);
        assert!(matches!(result, Err(Error::DecompressionFailed)));

        // Truncate to half the data
        let half_truncated = &compressed[..compressed.len() / 2];
        let result = decompress(half_truncated);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // Additional test: Invalid zlib header (wrong compression method)
    #[test]
    fn test_decompress_invalid_header_wrong_method() {
        // Create data with invalid compression method (not 8)
        let invalid = vec![0x00, 0x00, 0x00, 0x00];
        let result = decompress(&invalid);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // Additional test: Invalid zlib header (checksum fails)
    #[test]
    fn test_decompress_invalid_header_bad_checksum() {
        // Valid CM (8) but invalid checksum
        let invalid = vec![0x78, 0x00]; // 0x78 * 256 + 0x00 = 30720, 30720 % 31 != 0
        let result = decompress(&invalid);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // Additional test: Single byte (too short for header)
    #[test]
    fn test_decompress_single_byte() {
        let result = decompress(&[0x78]);
        assert!(matches!(result, Err(Error::DecompressionFailed)));
    }

    // Test zlib header validation directly
    #[test]
    fn test_is_valid_zlib_header() {
        // Common valid headers
        assert!(is_valid_zlib_header(0x78, 0x9C)); // Default compression
        assert!(is_valid_zlib_header(0x78, 0x01)); // No compression
        assert!(is_valid_zlib_header(0x78, 0xDA)); // Best compression

        // Invalid: wrong compression method
        assert!(!is_valid_zlib_header(0x00, 0x00));
        assert!(!is_valid_zlib_header(0x79, 0x9C)); // CM = 9, not 8

        // Invalid: window size too large
        assert!(!is_valid_zlib_header(0x88, 0x00)); // CINFO = 8

        // Invalid: bad checksum
        assert!(!is_valid_zlib_header(0x78, 0x00));
    }

    // C-005: Compress and decompress roundtrip
    #[test]
    fn test_compress_roundtrip() {
        let original = b"Hello, World! This is a test of compression.";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).expect("decompression should succeed");
        assert_eq!(decompressed, original);
    }

    // C-006: Compress empty data
    #[test]
    fn test_compress_empty() {
        let original = b"";
        let compressed = compress(original);
        let decompressed = decompress(&compressed).expect("decompression should succeed");
        assert_eq!(decompressed, original);
    }

    // C-007: Compress large data
    #[test]
    fn test_compress_large() {
        let original: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let compressed = compress(&original);
        let decompressed = decompress(&compressed).expect("decompression should succeed");
        assert_eq!(decompressed, original);
    }

    // C-008: Compressed data is smaller for repetitive data
    #[test]
    fn test_compress_reduces_size() {
        // Repetitive data should compress well
        let original = vec![b'a'; 1000];
        let compressed = compress(&original);
        assert!(compressed.len() < original.len());
    }
}
