//! Git commit object implementation.

use super::oid::Oid;
use super::store::{ObjectType, RawObject};
use crate::error::{Error, Result};

/// A signature representing an author or committer.
///
/// Contains the name, email, timestamp, and timezone offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The name of the person.
    name: String,
    /// The email address.
    email: String,
    /// Unix timestamp (seconds since epoch).
    timestamp: i64,
    /// Timezone offset in minutes (e.g., +0900 = 540, -0500 = -300).
    tz_offset: i32,
}

impl Signature {
    /// Creates a new Signature.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the person.
    /// * `email` - The email address.
    /// * `timestamp` - Unix timestamp (seconds since epoch).
    /// * `tz_offset` - Timezone offset in minutes.
    pub fn new(
        name: impl Into<String>,
        email: impl Into<String>,
        timestamp: i64,
        tz_offset: i32,
    ) -> Self {
        Signature {
            name: name.into(),
            email: email.into(),
            timestamp,
            tz_offset,
        }
    }

    /// Returns the name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the email address.
    pub fn email(&self) -> &str {
        &self.email
    }

    /// Returns the Unix timestamp.
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    /// Returns the timezone offset in minutes.
    pub fn tz_offset(&self) -> i32 {
        self.tz_offset
    }

    /// Parses a signature from a Git signature line.
    ///
    /// Format: `Name <email> timestamp timezone`
    /// Example: `John Doe <john@example.com> 1234567890 +0900`
    fn parse(s: &str) -> Result<Self> {
        // Find the email part enclosed in < >
        let email_start = s.find('<').ok_or(Error::InvalidUtf8)?;
        let email_end = s.find('>').ok_or(Error::InvalidUtf8)?;

        if email_start >= email_end {
            return Err(Error::InvalidUtf8);
        }

        let name = s[..email_start].trim().to_string();
        let email = s[email_start + 1..email_end].to_string();

        // Parse timestamp and timezone after the email
        let after_email = s[email_end + 1..].trim();
        let mut parts = after_email.split_whitespace();

        let timestamp: i64 = parts
            .next()
            .ok_or(Error::InvalidUtf8)?
            .parse()
            .map_err(|_| Error::InvalidUtf8)?;

        let tz_str = parts.next().ok_or(Error::InvalidUtf8)?;
        let tz_offset = parse_timezone(tz_str)?;

        Ok(Signature {
            name,
            email,
            timestamp,
            tz_offset,
        })
    }
}

/// Parses a timezone string like "+0900" or "-0500" into minutes offset.
fn parse_timezone(s: &str) -> Result<i32> {
    if s.len() != 5 {
        return Err(Error::InvalidUtf8);
    }

    let sign = match s.chars().next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return Err(Error::InvalidUtf8),
    };

    let hours: i32 = s[1..3].parse().map_err(|_| Error::InvalidUtf8)?;
    let minutes: i32 = s[3..5].parse().map_err(|_| Error::InvalidUtf8)?;

    Ok(sign * (hours * 60 + minutes))
}

/// A Git commit object.
///
/// Contains information about a snapshot of the repository including
/// the tree, parent commits, author, committer, and message.
#[derive(Debug, Clone)]
pub struct Commit {
    /// The OID (SHA-1 hash) of this commit.
    oid: Oid,
    /// The tree object this commit points to.
    tree: Oid,
    /// Parent commit(s). Empty for root commits.
    parents: Vec<Oid>,
    /// The author of the changes.
    author: Signature,
    /// The person who created this commit.
    committer: Signature,
    /// The commit message.
    message: String,
}

impl Commit {
    /// Parses a Commit from a RawObject with its OID.
    ///
    /// Commit format:
    /// ```text
    /// tree <sha1>
    /// parent <sha1>  (zero or more)
    /// author <signature>
    /// committer <signature>
    ///
    /// <message>
    /// ```
    ///
    /// # Arguments
    ///
    /// * `oid` - The OID (SHA-1 hash) of this commit.
    /// * `raw` - The raw object data.
    pub fn parse(oid: Oid, raw: RawObject) -> Result<Self> {
        if raw.object_type != ObjectType::Commit {
            return Err(Error::TypeMismatch {
                expected: "commit",
                actual: raw.object_type.as_str(),
            });
        }

        let content = std::str::from_utf8(&raw.content).map_err(|_| Error::InvalidUtf8)?;

        let mut tree: Option<Oid> = None;
        let mut parents = Vec::new();
        let mut author: Option<Signature> = None;
        let mut committer: Option<Signature> = None;
        let mut message = String::new();

        let mut in_message = false;

        for line in content.lines() {
            if in_message {
                if !message.is_empty() {
                    message.push('\n');
                }
                message.push_str(line);
                continue;
            }

            if line.is_empty() {
                in_message = true;
                continue;
            }

            if let Some(value) = line.strip_prefix("tree ") {
                tree = Some(Oid::from_hex(value)?);
            } else if let Some(value) = line.strip_prefix("parent ") {
                parents.push(Oid::from_hex(value)?);
            } else if let Some(value) = line.strip_prefix("author ") {
                author = Some(Signature::parse(value)?);
            } else if let Some(value) = line.strip_prefix("committer ") {
                committer = Some(Signature::parse(value)?);
            }
            // Ignore other headers (e.g., gpgsig, encoding)
        }

        let tree = tree.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing tree".to_string(),
        })?;

        let author = author.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing author".to_string(),
        })?;

        let committer = committer.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing committer".to_string(),
        })?;

        Ok(Commit {
            oid,
            tree,
            parents,
            author,
            committer,
            message,
        })
    }

    /// Returns the OID (SHA-1 hash) of this commit.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Returns the tree object ID.
    pub fn tree(&self) -> &Oid {
        &self.tree
    }

    /// Returns the parent commit IDs.
    pub fn parents(&self) -> &[Oid] {
        &self.parents
    }

    /// Returns the first parent, if any.
    pub fn parent(&self) -> Option<&Oid> {
        self.parents.first()
    }

    /// Returns the author signature.
    pub fn author(&self) -> &Signature {
        &self.author
    }

    /// Returns the committer signature.
    pub fn committer(&self) -> &Signature {
        &self.committer
    }

    /// Returns the full commit message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the first line of the commit message (the summary).
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }

    /// Returns true if this is a root commit (no parents).
    pub fn is_root(&self) -> bool {
        self.parents.is_empty()
    }

    /// Returns true if this is a merge commit (multiple parents).
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_commit(content: &str) -> RawObject {
        RawObject {
            object_type: ObjectType::Commit,
            content: content.as_bytes().to_vec(),
        }
    }

    fn make_blob() -> RawObject {
        RawObject {
            object_type: ObjectType::Blob,
            content: vec![],
        }
    }

    const TREE_SHA: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    const PARENT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const COMMIT_SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

    fn dummy_oid() -> Oid {
        Oid::from_hex(COMMIT_SHA).unwrap()
    }

    fn simple_commit() -> String {
        format!(
            "tree {}\n\
             author John Doe <john@example.com> 1234567890 +0900\n\
             committer Jane Doe <jane@example.com> 1234567899 -0500\n\
             \n\
             Initial commit\n\
             \n\
             This is the body.",
            TREE_SHA
        )
    }

    fn commit_with_parent() -> String {
        format!(
            "tree {}\n\
             parent {}\n\
             author John Doe <john@example.com> 1234567890 +0000\n\
             committer John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Second commit",
            TREE_SHA, PARENT_SHA
        )
    }

    // CM-001: Parse commit from RawObject
    #[test]
    fn test_parse_commit() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();
        assert_eq!(commit.tree().to_hex(), TREE_SHA);
    }

    // CM-OID: oid() returns the commit's OID
    #[test]
    fn test_oid() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();
        assert_eq!(commit.oid().to_hex(), COMMIT_SHA);
    }

    // CM-002: Parse returns TypeMismatch for non-commit
    #[test]
    fn test_parse_type_mismatch() {
        let raw = make_blob();
        let result = Commit::parse(dummy_oid(), raw);
        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "commit",
                actual: "blob"
            })
        ));
    }

    // CM-003: Parse commit with parent
    #[test]
    fn test_parse_with_parent() {
        let raw = make_commit(&commit_with_parent());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert_eq!(commit.parents().len(), 1);
        assert_eq!(commit.parent().unwrap().to_hex(), PARENT_SHA);
        assert!(!commit.is_root());
        assert!(!commit.is_merge());
    }

    // CM-004: Parse root commit (no parent)
    #[test]
    fn test_parse_root_commit() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert!(commit.parents().is_empty());
        assert!(commit.parent().is_none());
        assert!(commit.is_root());
    }

    // CM-005: Parse merge commit (multiple parents)
    #[test]
    fn test_parse_merge_commit() {
        let parent2 = "abcdef0123456789abcdef0123456789abcdef01";
        let content = format!(
            "tree {}\n\
             parent {}\n\
             parent {}\n\
             author John Doe <john@example.com> 1234567890 +0000\n\
             committer John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Merge branch 'feature'",
            TREE_SHA, PARENT_SHA, parent2
        );
        let raw = make_commit(&content);
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert_eq!(commit.parents().len(), 2);
        assert!(commit.is_merge());
    }

    // CM-006: Parse author and committer signatures
    #[test]
    fn test_parse_signatures() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        let author = commit.author();
        assert_eq!(author.name(), "John Doe");
        assert_eq!(author.email(), "john@example.com");
        assert_eq!(author.timestamp(), 1234567890);
        assert_eq!(author.tz_offset(), 540); // +0900 = 9*60 = 540

        let committer = commit.committer();
        assert_eq!(committer.name(), "Jane Doe");
        assert_eq!(committer.email(), "jane@example.com");
        assert_eq!(committer.timestamp(), 1234567899);
        assert_eq!(committer.tz_offset(), -300); // -0500 = -5*60 = -300
    }

    // CM-007: Parse timezone correctly
    #[test]
    fn test_parse_timezone() {
        assert_eq!(parse_timezone("+0000").unwrap(), 0);
        assert_eq!(parse_timezone("+0900").unwrap(), 540);
        assert_eq!(parse_timezone("-0500").unwrap(), -300);
        assert_eq!(parse_timezone("+1200").unwrap(), 720);
        assert_eq!(parse_timezone("-1100").unwrap(), -660);
        assert_eq!(parse_timezone("+0530").unwrap(), 330); // India

        assert!(parse_timezone("0000").is_err());
        assert!(parse_timezone("+000").is_err());
        assert!(parse_timezone("invalid").is_err());
    }

    // CM-008: message() returns full message
    #[test]
    fn test_message() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        let msg = commit.message();
        assert!(msg.contains("Initial commit"));
        assert!(msg.contains("This is the body."));
    }

    // CM-009: summary() returns first line
    #[test]
    fn test_summary() {
        let raw = make_commit(&simple_commit());
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert_eq!(commit.summary(), "Initial commit");
    }

    // Additional: Empty message
    #[test]
    fn test_empty_message() {
        let content = format!(
            "tree {}\n\
             author John Doe <john@example.com> 1234567890 +0000\n\
             committer John Doe <john@example.com> 1234567890 +0000\n\
             ",
            TREE_SHA
        );
        let raw = make_commit(&content);
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert_eq!(commit.message(), "");
        assert_eq!(commit.summary(), "");
    }

    // Additional: Single line message
    #[test]
    fn test_single_line_message() {
        let content = format!(
            "tree {}\n\
             author John Doe <john@example.com> 1234567890 +0000\n\
             committer John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Single line",
            TREE_SHA
        );
        let raw = make_commit(&content);
        let commit = Commit::parse(dummy_oid(), raw).unwrap();

        assert_eq!(commit.message(), "Single line");
        assert_eq!(commit.summary(), "Single line");
    }

    // Additional: Missing tree should error
    #[test]
    fn test_missing_tree() {
        let content = "author John Doe <john@example.com> 1234567890 +0000\n\
             committer John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Message";
        let raw = make_commit(content);
        let result = Commit::parse(dummy_oid(), raw);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));
    }

    // Additional: Signature parsing
    #[test]
    fn test_signature_parse() {
        let sig = Signature::parse("John Doe <john@example.com> 1234567890 +0900").unwrap();
        assert_eq!(sig.name(), "John Doe");
        assert_eq!(sig.email(), "john@example.com");
        assert_eq!(sig.timestamp(), 1234567890);
        assert_eq!(sig.tz_offset(), 540);

        // Name with special characters
        let sig = Signature::parse("José García <jose@example.com> 1234567890 +0000").unwrap();
        assert_eq!(sig.name(), "José García");
    }
}
