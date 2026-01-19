//! Git annotated tag object implementation.

use super::commit::Signature;
use super::oid::Oid;
use super::store::{ObjectType, RawObject};
use crate::error::{Error, Result};

/// An annotated tag object.
///
/// Annotated tags are actual Git objects that contain:
/// - The object being tagged (usually a commit)
/// - The tag name
/// - The tagger's signature
/// - A tag message
///
/// This is distinct from lightweight tags, which are just refs pointing
/// directly to commits.
#[derive(Debug, Clone)]
pub struct TagObject {
    /// The OID of the object this tag points to.
    object: Oid,
    /// The type of the tagged object (usually "commit").
    object_type: String,
    /// The tag name.
    tag_name: String,
    /// The tagger's signature.
    tagger: Signature,
    /// The tag message.
    message: String,
}

impl TagObject {
    /// Parses a TagObject from a RawObject.
    ///
    /// Tag object format:
    /// ```text
    /// object <sha1>
    /// type <type>
    /// tag <tag-name>
    /// tagger <signature>
    ///
    /// <message>
    /// ```
    pub fn parse(raw: RawObject) -> Result<Self> {
        if raw.object_type != ObjectType::Tag {
            return Err(Error::TypeMismatch {
                expected: "tag",
                actual: raw.object_type.as_str(),
            });
        }

        let content = std::str::from_utf8(&raw.content).map_err(|_| Error::InvalidUtf8)?;

        let mut object: Option<Oid> = None;
        let mut object_type: Option<String> = None;
        let mut tag_name: Option<String> = None;
        let mut tagger: Option<Signature> = None;
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

            if let Some(value) = line.strip_prefix("object ") {
                object = Some(Oid::from_hex(value)?);
            } else if let Some(value) = line.strip_prefix("type ") {
                object_type = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("tag ") {
                tag_name = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("tagger ") {
                tagger = Some(parse_signature(value)?);
            }
            // Ignore other headers (e.g., gpgsig)
        }

        let object = object.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing object".to_string(),
        })?;

        let object_type = object_type.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing type".to_string(),
        })?;

        let tag_name = tag_name.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing tag name".to_string(),
        })?;

        let tagger = tagger.ok_or_else(|| Error::InvalidObject {
            oid: String::new(),
            reason: "missing tagger".to_string(),
        })?;

        Ok(TagObject {
            object,
            object_type,
            tag_name,
            tagger,
            message,
        })
    }

    /// Returns the OID of the tagged object.
    pub fn object(&self) -> &Oid {
        &self.object
    }

    /// Returns the type of the tagged object.
    pub fn object_type(&self) -> &str {
        &self.object_type
    }

    /// Returns the tag name.
    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }

    /// Returns the tagger's signature.
    pub fn tagger(&self) -> &Signature {
        &self.tagger
    }

    /// Returns the tag message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the first line of the tag message.
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// Parses a signature from a Git signature line.
///
/// Format: `Name <email> timestamp timezone`
/// Example: `John Doe <john@example.com> 1234567890 +0900`
fn parse_signature(s: &str) -> Result<Signature> {
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

    Ok(Signature::new(name, email, timestamp, tz_offset))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tag(content: &str) -> RawObject {
        RawObject {
            object_type: ObjectType::Tag,
            content: content.as_bytes().to_vec(),
        }
    }

    fn make_blob() -> RawObject {
        RawObject {
            object_type: ObjectType::Blob,
            content: vec![],
        }
    }

    const OBJECT_SHA: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    fn simple_tag() -> String {
        format!(
            "object {}\n\
             type commit\n\
             tag v1.0.0\n\
             tagger John Doe <john@example.com> 1234567890 +0900\n\
             \n\
             Release version 1.0.0\n\
             \n\
             This is the tag body.",
            OBJECT_SHA
        )
    }

    #[test]
    fn test_parse_tag() {
        let raw = make_tag(&simple_tag());
        let tag = TagObject::parse(raw).unwrap();

        assert_eq!(tag.object().to_hex(), OBJECT_SHA);
        assert_eq!(tag.object_type(), "commit");
        assert_eq!(tag.tag_name(), "v1.0.0");
    }

    #[test]
    fn test_parse_type_mismatch() {
        let raw = make_blob();
        let result = TagObject::parse(raw);
        assert!(matches!(
            result,
            Err(Error::TypeMismatch {
                expected: "tag",
                actual: "blob"
            })
        ));
    }

    #[test]
    fn test_parse_tagger() {
        let raw = make_tag(&simple_tag());
        let tag = TagObject::parse(raw).unwrap();

        let tagger = tag.tagger();
        assert_eq!(tagger.name(), "John Doe");
        assert_eq!(tagger.email(), "john@example.com");
        assert_eq!(tagger.timestamp(), 1234567890);
        assert_eq!(tagger.tz_offset(), 540); // +0900
    }

    #[test]
    fn test_parse_message() {
        let raw = make_tag(&simple_tag());
        let tag = TagObject::parse(raw).unwrap();

        assert!(tag.message().contains("Release version 1.0.0"));
        assert!(tag.message().contains("This is the tag body."));
        assert_eq!(tag.summary(), "Release version 1.0.0");
    }

    #[test]
    fn test_parse_empty_message() {
        let content = format!(
            "object {}\n\
             type commit\n\
             tag v1.0.0\n\
             tagger John Doe <john@example.com> 1234567890 +0000\n\
             ",
            OBJECT_SHA
        );
        let raw = make_tag(&content);
        let tag = TagObject::parse(raw).unwrap();

        assert_eq!(tag.message(), "");
        assert_eq!(tag.summary(), "");
    }

    #[test]
    fn test_missing_object() {
        let content = "type commit\n\
             tag v1.0.0\n\
             tagger John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Message";
        let raw = make_tag(content);
        let result = TagObject::parse(raw);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));
    }

    #[test]
    fn test_missing_tag_name() {
        let content = format!(
            "object {}\n\
             type commit\n\
             tagger John Doe <john@example.com> 1234567890 +0000\n\
             \n\
             Message",
            OBJECT_SHA
        );
        let raw = make_tag(&content);
        let result = TagObject::parse(raw);
        assert!(matches!(result, Err(Error::InvalidObject { .. })));
    }
}
