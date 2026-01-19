//! Git configuration file parser.
//!
//! Parses the INI-like format used by Git configuration files.

use super::Config;
use crate::error::Result;

/// Parses a Git configuration file content into a `Config` instance.
pub fn parse(content: &str) -> Result<Config> {
    let mut config = Config::new();
    let mut current_section = String::new();
    let mut current_subsection = String::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Section header
        if line.starts_with('[') {
            if let Some((section, subsection)) = parse_section_header(line) {
                current_section = section;
                current_subsection = subsection;
            }
            continue;
        }

        // Key-value pair
        if !current_section.is_empty() {
            if let Some((key, value)) = parse_key_value(line) {
                config.set(&current_section, &current_subsection, &key, &value);
            }
        }
    }

    Ok(config)
}

/// Parses a section header like `[section]` or `[section "subsection"]`.
fn parse_section_header(line: &str) -> Option<(String, String)> {
    let line = line.trim();

    if !line.starts_with('[') || !line.ends_with(']') {
        return None;
    }

    // Remove brackets
    let inner = &line[1..line.len() - 1];

    // Check for subsection: [section "subsection"]
    if let Some(quote_start) = inner.find('"') {
        let section = inner[..quote_start].trim().to_string();
        let rest = &inner[quote_start + 1..];

        // Find closing quote
        if let Some(quote_end) = rest.rfind('"') {
            let subsection = unescape_subsection(&rest[..quote_end]);
            return Some((section, subsection));
        }
    }

    // Simple section without subsection
    Some((inner.trim().to_string(), String::new()))
}

/// Unescapes a subsection name.
///
/// Git allows escaping backslash and double-quote in subsection names.
fn unescape_subsection(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    '\\' | '"' => {
                        result.push(next);
                        chars.next();
                    }
                    _ => {
                        result.push(c);
                    }
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Parses a key-value line like `key = value` or `key=value`.
fn parse_key_value(line: &str) -> Option<(String, String)> {
    // Find the first = sign
    let eq_pos = line.find('=')?;

    let key = line[..eq_pos].trim().to_string();
    let value_part = line[eq_pos + 1..].trim();

    let value = parse_value(value_part);

    if key.is_empty() {
        return None;
    }

    Some((key, value))
}

/// Parses a value, handling quotes and escapes.
fn parse_value(s: &str) -> String {
    let s = s.trim();

    // Handle inline comments (not within quotes)
    let s = remove_inline_comment(s);

    // Handle quoted values
    if s.starts_with('"') {
        if let Some(end) = s[1..].find('"') {
            return unescape_value(&s[1..1 + end]);
        }
    }

    // Plain value
    unescape_value(&s)
}

/// Removes inline comments from a value.
fn remove_inline_comment(s: &str) -> &str {
    let mut in_quotes = false;
    let mut escape_next = false;

    for (i, c) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match c {
            '\\' => escape_next = true,
            '"' => in_quotes = !in_quotes,
            '#' | ';' if !in_quotes => return s[..i].trim_end(),
            _ => {}
        }
    }

    s
}

/// Unescapes a value string.
fn unescape_value(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    'n' => {
                        result.push('\n');
                        chars.next();
                    }
                    't' => {
                        result.push('\t');
                        chars.next();
                    }
                    '\\' => {
                        result.push('\\');
                        chars.next();
                    }
                    '"' => {
                        result.push('"');
                        chars.next();
                    }
                    _ => {
                        result.push(c);
                    }
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_section() {
        let (section, subsection) = parse_section_header("[core]").unwrap();
        assert_eq!(section, "core");
        assert_eq!(subsection, "");
    }

    #[test]
    fn test_parse_section_with_subsection() {
        let (section, subsection) = parse_section_header("[remote \"origin\"]").unwrap();
        assert_eq!(section, "remote");
        assert_eq!(subsection, "origin");
    }

    #[test]
    fn test_parse_section_with_escaped_subsection() {
        let (section, subsection) = parse_section_header("[section \"sub\\\"section\"]").unwrap();
        assert_eq!(section, "section");
        assert_eq!(subsection, "sub\"section");
    }

    #[test]
    fn test_parse_key_value_simple() {
        let (key, value) = parse_key_value("name = John Doe").unwrap();
        assert_eq!(key, "name");
        assert_eq!(value, "John Doe");
    }

    #[test]
    fn test_parse_key_value_no_spaces() {
        let (key, value) = parse_key_value("bare=true").unwrap();
        assert_eq!(key, "bare");
        assert_eq!(value, "true");
    }

    #[test]
    fn test_parse_key_value_quoted() {
        let (key, value) = parse_key_value("name = \"John Doe\"").unwrap();
        assert_eq!(key, "name");
        assert_eq!(value, "John Doe");
    }

    #[test]
    fn test_parse_key_value_with_comment() {
        let (key, value) = parse_key_value("name = John # this is a comment").unwrap();
        assert_eq!(key, "name");
        assert_eq!(value, "John");
    }

    #[test]
    fn test_parse_key_value_with_semicolon_comment() {
        let (key, value) = parse_key_value("name = John ; this is a comment").unwrap();
        assert_eq!(key, "name");
        assert_eq!(value, "John");
    }

    #[test]
    fn test_parse_key_value_escaped() {
        let (key, value) = parse_key_value("message = Hello\\nWorld").unwrap();
        assert_eq!(key, "message");
        assert_eq!(value, "Hello\nWorld");
    }

    #[test]
    fn test_parse_full_config() {
        let content = r#"
[core]
    bare = false
    repositoryformatversion = 0

[user]
    name = John Doe
    email = john@example.com

[remote "origin"]
    url = https://github.com/user/repo.git
    fetch = +refs/heads/*:refs/remotes/origin/*

[branch "main"]
    remote = origin
    merge = refs/heads/main
"#;

        let config = parse(content).unwrap();

        assert_eq!(config.get("core", "bare"), Some("false"));
        assert_eq!(config.get("core", "repositoryformatversion"), Some("0"));
        assert_eq!(config.get("user", "name"), Some("John Doe"));
        assert_eq!(config.get("user", "email"), Some("john@example.com"));
        assert_eq!(
            config.get_subsection("remote", "origin", "url"),
            Some("https://github.com/user/repo.git")
        );
        assert_eq!(
            config.get_subsection("remote", "origin", "fetch"),
            Some("+refs/heads/*:refs/remotes/origin/*")
        );
        assert_eq!(
            config.get_subsection("branch", "main", "remote"),
            Some("origin")
        );
        assert_eq!(
            config.get_subsection("branch", "main", "merge"),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn test_parse_comments() {
        let content = r#"
# This is a comment
; This is also a comment
[core]
    # Comment in section
    bare = false ; inline comment
"#;

        let config = parse(content).unwrap();
        assert_eq!(config.get("core", "bare"), Some("false"));
    }

    #[test]
    fn test_parse_empty_value() {
        let content = r#"
[section]
    key =
"#;

        let config = parse(content).unwrap();
        assert_eq!(config.get("section", "key"), Some(""));
    }

    #[test]
    fn test_parse_value_with_equals() {
        let content = r#"
[alias]
    st = status --short
"#;

        let config = parse(content).unwrap();
        assert_eq!(config.get("alias", "st"), Some("status --short"));
    }

    #[test]
    fn test_parse_case_sensitivity() {
        let content = r#"
[CORE]
    BARE = false
"#;

        let config = parse(content).unwrap();
        // Section and key are case-insensitive
        assert_eq!(config.get("core", "bare"), Some("false"));
        assert_eq!(config.get("CORE", "BARE"), Some("false"));
    }

    #[test]
    fn test_parse_subsection_case_sensitivity() {
        let content = r#"
[remote "Origin"]
    url = https://example.com
"#;

        let config = parse(content).unwrap();
        // Subsection names are case-sensitive
        assert_eq!(
            config.get_subsection("remote", "Origin", "url"),
            Some("https://example.com")
        );
        assert_eq!(config.get_subsection("remote", "origin", "url"), None);
    }
}
