//! Parser for the SHA-1 packed-refs file.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::objects::Oid;

/// Validates a ref name before using it as a relative filesystem path.
pub(super) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "@"
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name
            .bytes()
            .any(|b| b <= 32 || b == 127 || b"~^:?*[\\".contains(&b))
        || name
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(Error::InvalidRefName(name.to_owned()));
    }
    Ok(())
}

/// Peeled lines are validated but never replace the tag object's own OID.
pub(super) fn parse(content: &str) -> Result<BTreeMap<String, Oid>> {
    let mut refs = BTreeMap::new();
    let mut can_peel = false;
    for (index, line) in content.lines().enumerate() {
        let invalid = || Error::InvalidPackedRefs {
            line: index + 1,
            reason: "invalid or duplicate reference record".to_owned(),
        };
        if line.is_empty() || line.starts_with('#') {
            can_peel = false;
            continue;
        }
        if let Some(peeled) = line.strip_prefix('^') {
            if !can_peel || Oid::from_hex(peeled).is_err() {
                return Err(invalid());
            }
            can_peel = false;
            continue;
        }
        let (hex, name) = line.split_once(' ').ok_or_else(invalid)?;
        if !name.starts_with("refs/") || validate_name(name).is_err() {
            return Err(invalid());
        }
        let oid = Oid::from_hex(hex).map_err(|_| invalid())?;
        if refs.insert(name.to_owned(), oid).is_some() {
            return Err(invalid());
        }
        can_peel = true;
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";
    const PEELED: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

    #[test]
    fn peeled_record_preserves_tag_oid_and_handles_crlf() {
        let refs = parse(&format!(
            "# pack-refs with: peeled\r\n{OID} refs/tags/v1\r\n^{PEELED}\r\n"
        ))
        .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs["refs/tags/v1"].to_hex(), OID);
    }

    #[test]
    fn rejects_malformed_duplicate_or_orphan_records() {
        for content in [
            format!("^{OID}\n"),
            format!("{OID} refs/tags/v1\n^{PEELED}\n^{PEELED}\n"),
            format!("{OID} refs/heads/main\n{PEELED} refs/heads/main\n"),
            format!("{OID} refs/../HEAD\n"),
            format!("{OID} refs/heads/main extra\n"),
            "invalid refs/heads/main\n".to_owned(),
        ] {
            assert!(matches!(
                parse(&content),
                Err(Error::InvalidPackedRefs { .. })
            ));
        }
    }
}
