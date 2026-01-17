//! SHA-1 hash implementation (RFC 3174).

/// SHA-1 hash size in bytes.
pub const SHA1_SIZE: usize = 20;

/// Initial hash values for SHA-1.
const H0: u32 = 0x67452301;
const H1: u32 = 0xEFCDAB89;
const H2: u32 = 0x98BADCFE;
const H3: u32 = 0x10325476;
const H4: u32 = 0xC3D2E1F0;

/// SHA-1 round constants.
const K: [u32; 4] = [0x5A827999, 0x6ED9EBA1, 0x8F1BBCDC, 0xCA62C1D6];

/// Internal state for SHA-1 computation.
struct Sha1State {
    h: [u32; 5],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Sha1State {
    /// Creates a new SHA-1 state with initial values.
    fn new() -> Self {
        Self {
            h: [H0, H1, H2, H3, H4],
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    /// Updates the hash state with input data.
    fn update(&mut self, data: &[u8]) {
        let mut offset = 0;
        self.total_len += data.len() as u64;

        // If we have buffered data, try to complete a block
        if self.buffer_len > 0 {
            let needed = 64 - self.buffer_len;
            let to_copy = needed.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + to_copy]
                .copy_from_slice(&data[..to_copy]);
            self.buffer_len += to_copy;
            offset = to_copy;

            if self.buffer_len == 64 {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }

        // Process complete blocks
        while offset + 64 <= data.len() {
            let block: [u8; 64] = data[offset..offset + 64].try_into().unwrap();
            self.process_block(&block);
            offset += 64;
        }

        // Buffer remaining data
        if offset < data.len() {
            let remaining = data.len() - offset;
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Processes a single 512-bit (64-byte) block.
    fn process_block(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];

        // Prepare message schedule
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        // Initialize working variables
        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];

        // Main loop
        #[allow(clippy::needless_range_loop)]
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), K[0]),
                20..=39 => (b ^ c ^ d, K[1]),
                40..=59 => ((b & c) | (b & d) | (c & d), K[2]),
                60..=79 => (b ^ c ^ d, K[3]),
                _ => unreachable!(),
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        // Update hash values
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }

    /// Finalizes the hash computation and returns the digest.
    fn finalize(mut self) -> [u8; SHA1_SIZE] {
        let bit_len = self.total_len * 8;

        // Append padding bit
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        // If not enough room for length, process current block and start new one
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..64].fill(0);
            let block = self.buffer;
            self.process_block(&block);
            self.buffer_len = 0;
        }

        // Pad with zeros
        self.buffer[self.buffer_len..56].fill(0);

        // Append length in bits as big-endian u64
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());

        let block = self.buffer;
        self.process_block(&block);

        // Produce final hash
        let mut result = [0u8; SHA1_SIZE];
        for (i, &h) in self.h.iter().enumerate() {
            result[i * 4..i * 4 + 4].copy_from_slice(&h.to_be_bytes());
        }
        result
    }
}

/// Computes the SHA-1 hash of the given data.
///
/// Returns a 20-byte array containing the SHA-1 digest.
///
/// Usage: `let hash = sha1(b"hello world");`
#[allow(dead_code)]
pub fn sha1(data: &[u8]) -> [u8; SHA1_SIZE] {
    let mut state = Sha1State::new();
    state.update(data);
    state.finalize()
}

/// Computes the SHA-1 hash of a Git object.
///
/// Git objects are hashed as: `{type} {size}\0{content}`
///
/// Usage: `let hash = hash_object("blob", b"hello");`
///
/// The empty blob hash is `e69de29bb2d1d6434b8b29ae775ad8c2e48c5391`.
pub fn hash_object(object_type: &str, content: &[u8]) -> [u8; SHA1_SIZE] {
    let header = format!("{} {}\0", object_type, content.len());
    let mut state = Sha1State::new();
    state.update(header.as_bytes());
    state.update(content);
    state.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Converts a byte slice to a hex string.
    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    // H-001: Empty data hash
    #[test]
    fn test_sha1_empty() {
        let hash = sha1(b"");
        assert_eq!(to_hex(&hash), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    // H-002: "hello world" hash
    #[test]
    fn test_sha1_hello_world() {
        let hash = sha1(b"hello world");
        assert_eq!(to_hex(&hash), "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
    }

    // H-003: Binary data hash
    #[test]
    fn test_sha1_binary() {
        let data: Vec<u8> = (0u8..=255).collect();
        let hash = sha1(&data);
        assert_eq!(to_hex(&hash), "4916d6bdb7f78e6803698cab32d1586ea457dfc8");
    }

    // H-004: Large data hash
    #[test]
    fn test_sha1_large() {
        // 1MB of 'a'
        let data = vec![b'a'; 1024 * 1024];
        let hash = sha1(&data);
        assert_eq!(to_hex(&hash), "454027d64e3b855735552d42230eea1cbd645fa0");
    }

    // H-005: Git object format hash (empty blob)
    #[test]
    fn test_hash_object_empty_blob() {
        let hash = hash_object("blob", b"");
        assert_eq!(to_hex(&hash), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn test_hash_object_hello_blob() {
        // "hello\n" blob - matches `echo "hello" | git hash-object --stdin`
        let hash = hash_object("blob", b"hello\n");
        assert_eq!(to_hex(&hash), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn test_sha1_abc() {
        // Standard test vector from RFC 3174
        let hash = sha1(b"abc");
        assert_eq!(to_hex(&hash), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn test_sha1_448_bits() {
        // Another standard test vector
        let hash = sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(to_hex(&hash), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }

    #[test]
    fn test_sha1_incremental() {
        // Test that incremental update produces same result as single call
        let data = b"hello world this is a test of incremental hashing";

        let hash1 = sha1(data);

        let mut state = Sha1State::new();
        state.update(b"hello ");
        state.update(b"world ");
        state.update(b"this is a test of incremental hashing");
        let hash2 = state.finalize();

        assert_eq!(hash1, hash2);
    }
}
