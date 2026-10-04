//! Editing configuration files in place.
//!
//! Changes touch only the affected lines, so comments, ordering and other
//! entries are kept as they were, like `git config` does.

use std::path::Path;

use crate::error::{Error, Result};
use crate::infra::write_file_atomic;

/// A parsed section header line.
fn header(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let inner = line.strip_prefix('[')?;
    let end = inner.rfind(']')?;
    let inner = &inner[..end];
    match inner.find('"') {
        Some(quote) => {
            let name = inner[..quote].trim().to_lowercase();
            let rest = &inner[quote + 1..];
            let close = rest.rfind('"')?;
            let mut sub = String::new();
            let mut chars = rest[..close].chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        sub.push(next);
                    }
                } else {
                    sub.push(c);
                }
            }
            Some((name, sub))
        }
        None => Some((inner.trim().to_lowercase(), String::new())),
    }
}

/// The key of a `key = value` (or bare `key`) line, lowercased.
fn key_of(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') || line.starts_with('[') {
        return None;
    }
    let end = line
        .find(|c: char| c == '=' || c.is_whitespace())
        .unwrap_or(line.len());
    Some(line[..end].to_lowercase())
}

fn format_header(section: &str, subsection: &str) -> String {
    if subsection.is_empty() {
        format!("[{}]", section)
    } else {
        format!(
            "[{} \"{}\"]",
            section,
            subsection.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

fn format_value(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    let needs_quotes = value.starts_with(' ')
        || value.ends_with(' ')
        || value.contains('#')
        || value.contains(';');
    if needs_quotes {
        format!("\"{}\"", escaped)
    } else {
        escaped
    }
}

/// A configuration file being edited.
pub(crate) struct ConfigFile {
    lines: Vec<String>,
}

impl ConfigFile {
    /// Reads a file; a missing file is empty.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let text = match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(ConfigFile {
            lines: text.lines().map(str::to_owned).collect(),
        })
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let mut text = self.lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        write_file_atomic(path, text.as_bytes())
    }

    /// The line ranges (header line, end) of every matching section.
    fn sections(&self, section: &str, subsection: &str) -> Vec<(usize, usize)> {
        let section = section.to_lowercase();
        let mut result = Vec::new();
        let mut current: Option<usize> = None;
        for (i, line) in self.lines.iter().enumerate() {
            if let Some((name, sub)) = header(line) {
                if let Some(start) = current.take() {
                    result.push((start, i));
                }
                if name == section && sub == subsection {
                    current = Some(i);
                }
            }
        }
        if let Some(start) = current {
            result.push((start, self.lines.len()));
        }
        result
    }

    /// Lines holding `key` in matching sections.
    fn key_lines(&self, section: &str, subsection: &str, key: &str) -> Vec<usize> {
        let key = key.to_lowercase();
        self.sections(section, subsection)
            .into_iter()
            .flat_map(|(start, end)| start + 1..end)
            .filter(|&i| key_of(&self.lines[i]).as_deref() == Some(key.as_str()))
            .collect()
    }

    /// Sets a single-valued key, replacing its last occurrence (and removing
    /// earlier ones), or adding it to the section.
    pub(crate) fn set(&mut self, section: &str, subsection: &str, key: &str, value: &str) {
        let lines = self.key_lines(section, subsection, key);
        let entry = format!("\t{} = {}", key, format_value(value));
        match lines.split_last() {
            Some((&last, earlier)) => {
                self.lines[last] = entry;
                for &i in earlier.iter().rev() {
                    self.lines.remove(i);
                }
            }
            None => self.add(section, subsection, key, value),
        }
    }

    /// Adds a value (for multi-valued keys), creating the section if needed.
    pub(crate) fn add(&mut self, section: &str, subsection: &str, key: &str, value: &str) {
        let entry = format!("\t{} = {}", key, format_value(value));
        match self.sections(section, subsection).last() {
            Some(&(_, end)) => {
                // After the section's last non-blank line.
                let mut at = end;
                while at > 0 && self.lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                self.lines.insert(at, entry);
            }
            None => {
                self.lines.push(format_header(section, subsection));
                self.lines.push(entry);
            }
        }
    }

    /// Removes every value of a key.
    pub(crate) fn unset_all(&mut self, section: &str, subsection: &str, key: &str) {
        for i in self.key_lines(section, subsection, key).into_iter().rev() {
            self.lines.remove(i);
        }
    }

    /// Removes matching sections entirely. Returns whether any existed.
    pub(crate) fn remove_section(&mut self, section: &str, subsection: &str) -> bool {
        let sections = self.sections(section, subsection);
        for &(start, end) in sections.iter().rev() {
            self.lines.drain(start..end);
        }
        !sections.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn edit(text: &str, f: impl FnOnce(&mut ConfigFile)) -> String {
        let mut file = ConfigFile {
            lines: text.lines().map(str::to_owned).collect(),
        };
        f(&mut file);
        let mut out = file.lines.join("\n");
        out.push('\n');
        out
    }

    #[test]
    fn test_set_add_and_remove() {
        let text = "# comment\n[core]\n\tbare = false\n[remote \"origin\"]\n\turl = old\n\n[user]\n\tname = A\n";
        let out = edit(text, |f| {
            f.set("remote", "origin", "url", "https://example.com/r.git");
            f.add(
                "remote",
                "origin",
                "fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            );
            f.add("remote", "origin", "fetch", "+refs/tags/*:refs/tags/*");
            f.set("branch", "main", "remote", "origin");
        });
        assert_eq!(
            out,
            "# comment\n[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://example.com/r.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\tfetch = +refs/tags/*:refs/tags/*\n\n[user]\n\tname = A\n[branch \"main\"]\n\tremote = origin\n"
        );
        let config = Config::from_str(&out).unwrap();
        assert_eq!(config.get_all("remote", "origin", "fetch").len(), 2);

        let out = edit(&out, |f| {
            assert!(f.remove_section("remote", "origin"));
            f.unset_all("user", "", "name");
        });
        assert_eq!(
            out,
            "# comment\n[core]\n\tbare = false\n[user]\n[branch \"main\"]\n\tremote = origin\n"
        );
    }

    #[test]
    fn test_values_are_quoted_when_needed() {
        let out = edit("", |f| f.set("alias", "", "x", " a#b\"c\\d "));
        let config = Config::from_str(&out).unwrap();
        assert_eq!(config.get("alias", "x"), Some(" a#b\"c\\d "));
    }
}
