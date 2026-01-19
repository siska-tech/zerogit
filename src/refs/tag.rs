//! Git tag representation.

use crate::objects::{Oid, Signature};

/// Represents a Git tag.
///
/// Git supports two types of tags:
/// - Lightweight tags: Simple pointers to a commit (like a branch that doesn't move)
/// - Annotated tags: Full objects with tagger, date, message, and optional GPG signature
#[derive(Debug, Clone)]
pub struct Tag {
    /// The tag name (without `refs/tags/` prefix).
    name: String,
    /// The object this tag points to (commit for lightweight, or the peeled commit for annotated).
    target: Oid,
    /// The message (only for annotated tags).
    message: Option<String>,
    /// The tagger signature (only for annotated tags).
    tagger: Option<Signature>,
}

impl Tag {
    /// Creates a new lightweight tag.
    ///
    /// # Arguments
    ///
    /// * `name` - The tag name (without `refs/tags/` prefix).
    /// * `target` - The commit OID this tag points to.
    pub fn lightweight(name: impl Into<String>, target: Oid) -> Self {
        Tag {
            name: name.into(),
            target,
            message: None,
            tagger: None,
        }
    }

    /// Creates a new annotated tag.
    ///
    /// # Arguments
    ///
    /// * `name` - The tag name (without `refs/tags/` prefix).
    /// * `target` - The commit OID this tag ultimately points to.
    /// * `message` - The tag message.
    /// * `tagger` - The tagger signature.
    pub fn annotated(
        name: impl Into<String>,
        target: Oid,
        message: String,
        tagger: Signature,
    ) -> Self {
        Tag {
            name: name.into(),
            target,
            message: Some(message),
            tagger: Some(tagger),
        }
    }

    /// Returns the tag name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the target commit OID.
    pub fn target(&self) -> &Oid {
        &self.target
    }

    /// Returns the tag message if this is an annotated tag.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Returns the tagger signature if this is an annotated tag.
    pub fn tagger(&self) -> Option<&Signature> {
        self.tagger.as_ref()
    }

    /// Returns `true` if this is an annotated tag.
    pub fn is_annotated(&self) -> bool {
        self.message.is_some()
    }

    /// Returns the full reference name (with `refs/tags/` prefix).
    pub fn reference_name(&self) -> String {
        format!("refs/tags/{}", self.name)
    }

    /// Returns a short representation of the target OID (7 characters).
    pub fn short_target(&self) -> String {
        self.target.short()
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_OID: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    fn test_oid() -> Oid {
        Oid::from_hex(TEST_OID).unwrap()
    }

    #[test]
    fn test_lightweight_tag() {
        let tag = Tag::lightweight("v1.0.0", test_oid());

        assert_eq!(tag.name(), "v1.0.0");
        assert_eq!(tag.target().to_hex(), TEST_OID);
        assert!(!tag.is_annotated());
        assert!(tag.message().is_none());
        assert!(tag.tagger().is_none());
    }

    #[test]
    fn test_reference_name() {
        let tag = Tag::lightweight("v1.0.0", test_oid());
        assert_eq!(tag.reference_name(), "refs/tags/v1.0.0");
    }

    #[test]
    fn test_short_target() {
        let tag = Tag::lightweight("v1.0.0", test_oid());
        assert_eq!(tag.short_target().len(), 7);
        assert_eq!(tag.short_target(), "da39a3e");
    }

    #[test]
    fn test_display() {
        let tag = Tag::lightweight("v2.0.0", test_oid());
        assert_eq!(format!("{}", tag), "v2.0.0");
    }
}
