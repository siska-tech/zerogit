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

    // Helper to create valid zlib-compressed data
    fn compress_data(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
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
