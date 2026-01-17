//! Object ID (SHA-1 hash) representation.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// The length of a SHA-1 hash in bytes.
pub const OID_BYTES: usize = 20;

/// The length of a SHA-1 hash as a hexadecimal string.
pub const OID_HEX_LEN: usize = 40;

/// A Git object ID (SHA-1 hash).
///
/// This type represents a 20-byte SHA-1 hash that uniquely identifies
/// a Git object (blob, tree, commit, or tag).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid {
    bytes: [u8; OID_BYTES],
}

impl Oid {
    /// Creates an Oid from a 40-character hexadecimal string.
    ///
    /// # Arguments
    ///
    /// * `hex` - A 40-character hexadecimal string (case-insensitive).
    ///
    /// # Returns
    ///
    /// The Oid on success, or `Error::InvalidOid` if the string is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use zerogit::objects::Oid;
    ///
    /// let oid = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
    /// assert_eq!(oid.to_hex(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    /// ```
    pub fn from_hex(hex: &str) -> Result<Self> {
        if hex.len() != OID_HEX_LEN {
            return Err(Error::InvalidOid(hex.to_string()));
        }

        let mut bytes = [0u8; OID_BYTES];

        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let high =
                hex_digit_to_value(chunk[0]).ok_or_else(|| Error::InvalidOid(hex.to_string()))?;
            let low =
                hex_digit_to_value(chunk[1]).ok_or_else(|| Error::InvalidOid(hex.to_string()))?;
            bytes[i] = (high << 4) | low;
        }

        Ok(Oid { bytes })
    }

    /// Creates an Oid from a 20-byte array.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A 20-byte array containing the raw SHA-1 hash.
    ///
    /// # Examples
    ///
    /// ```
    /// use zerogit::objects::Oid;
    ///
    /// let bytes = [0u8; 20];
    /// let oid = Oid::from_bytes(bytes);
    /// ```
    pub fn from_bytes(bytes: [u8; OID_BYTES]) -> Self {
        Oid { bytes }
    }

    /// Returns the hexadecimal string representation of this Oid.
    ///
    /// The returned string is always lowercase and 40 characters long.
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(OID_HEX_LEN);
        for byte in &self.bytes {
            hex.push(HEX_CHARS[(byte >> 4) as usize]);
            hex.push(HEX_CHARS[(byte & 0x0f) as usize]);
        }
        hex
    }

    /// Returns a short (7-character) hexadecimal representation of this Oid.
    ///
    /// This is commonly used for display purposes where the full hash
    /// is not necessary.
    pub fn short(&self) -> String {
        self.to_hex()[..7].to_string()
    }

    /// Returns a reference to the raw 20-byte array.
    pub fn as_bytes(&self) -> &[u8; OID_BYTES] {
        &self.bytes
    }
}

/// Hexadecimal characters for encoding.
const HEX_CHARS: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// Converts a hexadecimal ASCII character to its numeric value.
fn hex_digit_to_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

impl FromStr for Oid {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Oid::from_hex(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known SHA-1 hash of empty string
    const EMPTY_SHA1: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    // O-001: from_hex with valid lowercase hex string
    #[test]
    fn test_from_hex_lowercase() {
        let oid = Oid::from_hex(EMPTY_SHA1).unwrap();
        assert_eq!(oid.to_hex(), EMPTY_SHA1);
    }

    // O-002: from_hex with uppercase normalizes to lowercase
    #[test]
    fn test_from_hex_uppercase_normalizes() {
        let upper = EMPTY_SHA1.to_uppercase();
        let oid = Oid::from_hex(&upper).unwrap();
        assert_eq!(oid.to_hex(), EMPTY_SHA1);
    }

    // O-003: from_hex with mixed case normalizes to lowercase
    #[test]
    fn test_from_hex_mixed_case() {
        let mixed = "DA39a3EE5e6b4B0d3255BFEF95601890afd80709";
        let oid = Oid::from_hex(mixed).unwrap();
        assert_eq!(oid.to_hex(), EMPTY_SHA1);
    }

    // O-004: from_hex with invalid length returns error
    #[test]
    fn test_from_hex_invalid_length() {
        // Too short
        let result = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd8070");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Too long
        let result = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd807090");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Empty
        let result = Oid::from_hex("");
        assert!(matches!(result, Err(Error::InvalidOid(_))));
    }

    // O-005: from_hex with invalid characters returns error
    #[test]
    fn test_from_hex_invalid_chars() {
        // Contains 'g'
        let result = Oid::from_hex("ga39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Contains space
        let result = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd8070 ");
        assert!(matches!(result, Err(Error::InvalidOid(_))));

        // Contains special character
        let result = Oid::from_hex("da39a3ee5e6b4b0d3255bfef95601890afd8070!");
        assert!(matches!(result, Err(Error::InvalidOid(_))));
    }

    // O-006: from_bytes creates Oid correctly
    #[test]
    fn test_from_bytes() {
        let bytes: [u8; 20] = [
            0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
            0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09,
        ];
        let oid = Oid::from_bytes(bytes);
        assert_eq!(oid.to_hex(), EMPTY_SHA1);
    }

    // O-007: short() returns first 7 characters
    #[test]
    fn test_short() {
        let oid = Oid::from_hex(EMPTY_SHA1).unwrap();
        assert_eq!(oid.short(), "da39a3e");
        assert_eq!(oid.short().len(), 7);
    }

    // O-008: Display trait outputs full hex
    #[test]
    fn test_display() {
        let oid = Oid::from_hex(EMPTY_SHA1).unwrap();
        let display = format!("{}", oid);
        assert_eq!(display, EMPTY_SHA1);
    }

    // O-009: FromStr trait works like from_hex
    #[test]
    fn test_from_str() {
        let oid: Oid = EMPTY_SHA1.parse().unwrap();
        assert_eq!(oid.to_hex(), EMPTY_SHA1);

        // Invalid should fail
        let result: Result<Oid> = "invalid".parse();
        assert!(result.is_err());
    }

    // Additional: Debug trait outputs short form
    #[test]
    fn test_debug() {
        let oid = Oid::from_hex(EMPTY_SHA1).unwrap();
        let debug = format!("{:?}", oid);
        assert_eq!(debug, "Oid(da39a3e)");
    }

    // Additional: as_bytes returns correct bytes
    #[test]
    fn test_as_bytes() {
        let expected: [u8; 20] = [
            0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
            0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09,
        ];
        let oid = Oid::from_hex(EMPTY_SHA1).unwrap();
        assert_eq!(oid.as_bytes(), &expected);
    }

    // Additional: Oid implements Eq, Hash, Ord
    #[test]
    fn test_traits() {
        let oid1 = Oid::from_hex(EMPTY_SHA1).unwrap();
        let oid2 = Oid::from_hex(EMPTY_SHA1).unwrap();
        let oid3 = Oid::from_hex("0000000000000000000000000000000000000000").unwrap();

        // Eq
        assert_eq!(oid1, oid2);
        assert_ne!(oid1, oid3);

        // Ord
        assert!(oid3 < oid1);

        // Hash (can be used in HashMap)
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(oid1);
        assert!(set.contains(&oid2));
    }

    // Additional: Clone and Copy
    #[test]
    fn test_clone_copy() {
        let oid1 = Oid::from_hex(EMPTY_SHA1).unwrap();
        let oid2 = oid1; // Copy
        let oid3 = oid1.clone(); // Clone
        assert_eq!(oid1, oid2);
        assert_eq!(oid1, oid3);
    }
}
