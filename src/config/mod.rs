//! Git configuration file parsing and access.
//!
//! This module provides functionality to read Git configuration files
//! (`.git/config`, `~/.gitconfig`, `/etc/gitconfig`).
//!
//! # Example
//!
//! ```no_run
//! use zerogit::config::Config;
//!
//! // Load config from a file
//! let config = Config::from_file(".git/config").unwrap();
//!
//! // Get a value
//! if let Some(name) = config.get("user", "name") {
//!     println!("User name: {}", name);
//! }
//!
//! // Get typed values
//! let auto_crlf = config.get_bool("core", "autocrlf").unwrap_or(false);
//! ```

mod parser;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::read_file;

/// A parsed Git configuration.
///
/// Git configuration files use an INI-like format with sections and key-value pairs.
/// This struct provides read-only access to configuration values.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Configuration entries stored as section -> subsection -> key -> value.
    /// Subsection is empty string for sections without subsection.
    entries: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
}

impl Config {
    /// Creates a new empty configuration.
    pub fn new() -> Self {
        Config {
            entries: BTreeMap::new(),
        }
    }

    /// Parses configuration from a file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the configuration file.
    ///
    /// # Returns
    ///
    /// A `Config` instance, or an error if the file cannot be read or parsed.
    ///
    /// Note: This method does not process include directives. Use
    /// [`from_file_with_includes`] to process includes.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = read_file(path.as_ref())?;
        let text = String::from_utf8(content).map_err(|_| Error::InvalidUtf8)?;
        Self::from_str(&text)
    }

    /// Parses configuration from a file, processing include directives.
    ///
    /// This method handles Git's `include.path` and `includeIf` directives,
    /// loading additional configuration files as specified.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the configuration file.
    ///
    /// # Returns
    ///
    /// A `Config` instance with all included files merged.
    pub fn from_file_with_includes<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let mut visited = HashSet::new();
        Self::load_with_includes_recursive(path, &mut visited)
    }

    /// Recursively loads configuration with include directives.
    fn load_with_includes_recursive(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Self> {
        // Canonicalize path to detect cycles
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Check for circular includes
        if visited.contains(&canonical) {
            return Ok(Config::new());
        }
        visited.insert(canonical.clone());

        // Load the file
        let content = match read_file(path) {
            Ok(c) => c,
            Err(_) => return Ok(Config::new()), // Missing include files are ignored
        };
        let text = String::from_utf8(content).map_err(|_| Error::InvalidUtf8)?;
        let mut config = Self::from_str(&text)?;

        // Process include directives
        let base_dir = path.parent().unwrap_or(Path::new("."));

        // Collect include paths first to avoid borrow issues
        let include_paths: Vec<PathBuf> = config
            .keys("include")
            .iter()
            .filter(|k| k.to_lowercase() == "path")
            .filter_map(|_| config.get("include", "path"))
            .map(|p| expand_path(p, base_dir))
            .collect();

        // Load and merge included configs
        for include_path in include_paths {
            if let Ok(included) = Self::load_with_includes_recursive(&include_path, visited) {
                config.merge(&included);
            }
        }

        Ok(config)
    }

    /// Parses configuration from a string.
    ///
    /// # Arguments
    ///
    /// * `content` - The configuration file content.
    ///
    /// # Returns
    ///
    /// A `Config` instance, or an error if parsing fails.
    pub fn from_str(content: &str) -> Result<Self> {
        parser::parse(content)
    }

    /// Gets a configuration value.
    ///
    /// # Arguments
    ///
    /// * `section` - The section name (e.g., "user", "core").
    /// * `key` - The key name (e.g., "name", "email").
    ///
    /// # Returns
    ///
    /// The value if found, or `None` if the key doesn't exist.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zerogit::config::Config;
    ///
    /// let config = Config::from_file(".git/config").unwrap();
    /// if let Some(name) = config.get("user", "name") {
    ///     println!("User: {}", name);
    /// }
    /// ```
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.get_subsection(section, "", key)
    }

    /// Gets a configuration value from a section with a subsection.
    ///
    /// # Arguments
    ///
    /// * `section` - The section name (e.g., "remote", "branch").
    /// * `subsection` - The subsection name (e.g., "origin", "main").
    /// * `key` - The key name (e.g., "url", "remote").
    ///
    /// # Returns
    ///
    /// The value if found, or `None` if the key doesn't exist.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zerogit::config::Config;
    ///
    /// let config = Config::from_file(".git/config").unwrap();
    /// if let Some(url) = config.get_subsection("remote", "origin", "url") {
    ///     println!("Origin URL: {}", url);
    /// }
    /// ```
    pub fn get_subsection(&self, section: &str, subsection: &str, key: &str) -> Option<&str> {
        let section_lower = section.to_lowercase();
        let key_lower = key.to_lowercase();

        self.entries
            .get(&section_lower)
            .and_then(|subs| subs.get(subsection))
            .and_then(|keys| keys.get(&key_lower))
            .map(|s| s.as_str())
    }

    /// Gets a configuration value as a boolean.
    ///
    /// Git config supports various boolean representations:
    /// - `true`, `yes`, `on`, `1` -> `true`
    /// - `false`, `no`, `off`, `0` -> `false`
    ///
    /// # Arguments
    ///
    /// * `section` - The section name.
    /// * `key` - The key name.
    ///
    /// # Returns
    ///
    /// `Ok(bool)` if the value exists and is a valid boolean,
    /// `Ok(false)` if the key doesn't exist,
    /// `Err` if the value is not a valid boolean.
    pub fn get_bool(&self, section: &str, key: &str) -> Result<bool> {
        self.get_bool_subsection(section, "", key)
    }

    /// Gets a configuration value as a boolean from a section with a subsection.
    pub fn get_bool_subsection(
        &self,
        section: &str,
        subsection: &str,
        key: &str,
    ) -> Result<bool> {
        match self.get_subsection(section, subsection, key) {
            None => Ok(false),
            Some(value) => parse_bool(value),
        }
    }

    /// Gets a configuration value as an integer.
    ///
    /// Git config supports optional suffixes:
    /// - `k` or `K` -> multiply by 1024
    /// - `m` or `M` -> multiply by 1024^2
    /// - `g` or `G` -> multiply by 1024^3
    ///
    /// # Arguments
    ///
    /// * `section` - The section name.
    /// * `key` - The key name.
    ///
    /// # Returns
    ///
    /// `Ok(i64)` if the value exists and is a valid integer,
    /// `Ok(0)` if the key doesn't exist,
    /// `Err` if the value is not a valid integer.
    pub fn get_int(&self, section: &str, key: &str) -> Result<i64> {
        self.get_int_subsection(section, "", key)
    }

    /// Gets a configuration value as an integer from a section with a subsection.
    pub fn get_int_subsection(
        &self,
        section: &str,
        subsection: &str,
        key: &str,
    ) -> Result<i64> {
        match self.get_subsection(section, subsection, key) {
            None => Ok(0),
            Some(value) => parse_int(value),
        }
    }

    /// Returns all section names in the configuration.
    ///
    /// # Returns
    ///
    /// A vector of section names (without subsections).
    pub fn sections(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    /// Returns all subsections for a given section.
    ///
    /// # Arguments
    ///
    /// * `section` - The section name.
    ///
    /// # Returns
    ///
    /// A vector of subsection names. Empty subsection ("") is excluded.
    pub fn subsections(&self, section: &str) -> Vec<&str> {
        let section_lower = section.to_lowercase();
        self.entries
            .get(&section_lower)
            .map(|subs| {
                subs.keys()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all keys in a section.
    ///
    /// # Arguments
    ///
    /// * `section` - The section name.
    ///
    /// # Returns
    ///
    /// A vector of key names.
    pub fn keys(&self, section: &str) -> Vec<&str> {
        self.keys_subsection(section, "")
    }

    /// Returns all keys in a section with a subsection.
    pub fn keys_subsection(&self, section: &str, subsection: &str) -> Vec<&str> {
        let section_lower = section.to_lowercase();
        self.entries
            .get(&section_lower)
            .and_then(|subs| subs.get(subsection))
            .map(|keys| keys.keys().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Sets a configuration value (internal use for building).
    pub(crate) fn set(&mut self, section: &str, subsection: &str, key: &str, value: &str) {
        let section_lower = section.to_lowercase();
        let key_lower = key.to_lowercase();

        self.entries
            .entry(section_lower)
            .or_default()
            .entry(subsection.to_string())
            .or_default()
            .insert(key_lower, value.to_string());
    }

    /// Merges another configuration into this one.
    ///
    /// Values from `other` will override values in `self`.
    pub fn merge(&mut self, other: &Config) {
        for (section, subsections) in &other.entries {
            for (subsection, keys) in subsections {
                for (key, value) in keys {
                    self.set(section, subsection, key, value);
                }
            }
        }
    }
}

/// Parses a string as a Git boolean value.
fn parse_bool(value: &str) -> Result<bool> {
    let lower = value.trim().to_lowercase();
    match lower.as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" | "" => Ok(false),
        _ => Err(Error::ConfigNotFound(format!(
            "invalid boolean value: {}",
            value
        ))),
    }
}

/// Parses a string as a Git integer value with optional suffix.
fn parse_int(value: &str) -> Result<i64> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }

    let (num_str, multiplier) = if let Some(stripped) = value.strip_suffix(['k', 'K']) {
        (stripped, 1024_i64)
    } else if let Some(stripped) = value.strip_suffix(['m', 'M']) {
        (stripped, 1024_i64 * 1024)
    } else if let Some(stripped) = value.strip_suffix(['g', 'G']) {
        (stripped, 1024_i64 * 1024 * 1024)
    } else {
        (value, 1_i64)
    };

    let num: i64 = num_str
        .trim()
        .parse()
        .map_err(|_| Error::ConfigNotFound(format!("invalid integer value: {}", value)))?;

    num.checked_mul(multiplier)
        .ok_or_else(|| Error::ConfigNotFound(format!("integer overflow: {}", value)))
}

/// Expands a path, handling `~` for home directory and relative paths.
fn expand_path(path: &str, base_dir: &Path) -> PathBuf {
    let path = path.trim();

    // Handle home directory expansion
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }

    // Handle absolute paths
    let path_buf = PathBuf::from(path);
    if path_buf.is_absolute() {
        return path_buf;
    }

    // Relative path - resolve against base directory
    base_dir.join(path)
}

/// Configuration source level for precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigLevel {
    /// System-wide configuration (`/etc/gitconfig` on Unix).
    System,
    /// Global user configuration (`~/.gitconfig` or `~/.config/git/config`).
    Global,
    /// Repository-local configuration (`.git/config`).
    Local,
}

impl ConfigLevel {
    /// Returns the default path for this configuration level.
    pub fn default_path(&self) -> Option<PathBuf> {
        match self {
            ConfigLevel::System => {
                #[cfg(unix)]
                {
                    Some(PathBuf::from("/etc/gitconfig"))
                }
                #[cfg(windows)]
                {
                    // On Windows, system config is typically in Program Files
                    std::env::var("ProgramFiles")
                        .ok()
                        .map(|pf| PathBuf::from(pf).join("Git").join("etc").join("gitconfig"))
                }
                #[cfg(not(any(unix, windows)))]
                {
                    None
                }
            }
            ConfigLevel::Global => {
                dirs::home_dir().map(|home| home.join(".gitconfig"))
            }
            ConfigLevel::Local => None, // Requires repository context
        }
    }
}

/// Loads configuration with the standard Git precedence.
///
/// Configuration is loaded in the following order (later overrides earlier):
/// 1. System configuration (`/etc/gitconfig`)
/// 2. Global configuration (`~/.gitconfig`)
/// 3. Local repository configuration (`.git/config`)
///
/// # Arguments
///
/// * `git_dir` - Path to the `.git` directory for local config.
///
/// # Returns
///
/// A merged `Config` with proper precedence.
pub fn load_config<P: AsRef<Path>>(git_dir: P) -> Result<Config> {
    let mut config = Config::new();

    // Load system config
    if let Some(system_path) = ConfigLevel::System.default_path() {
        if let Ok(system_config) = Config::from_file_with_includes(&system_path) {
            config.merge(&system_config);
        }
    }

    // Load global config
    if let Some(global_path) = ConfigLevel::Global.default_path() {
        if let Ok(global_config) = Config::from_file_with_includes(&global_path) {
            config.merge(&global_config);
        }
    }

    // Also check XDG config location
    if let Some(xdg_config) = xdg_config_path() {
        if let Ok(xdg_config) = Config::from_file_with_includes(&xdg_config) {
            config.merge(&xdg_config);
        }
    }

    // Load local config (highest precedence)
    let local_path = git_dir.as_ref().join("config");
    if let Ok(local_config) = Config::from_file_with_includes(&local_path) {
        config.merge(&local_config);
    }

    Ok(config)
}

/// Returns the XDG config path for Git (~/.config/git/config).
fn xdg_config_path() -> Option<PathBuf> {
    // Check XDG_CONFIG_HOME first
    if let Ok(xdg_home) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(xdg_home).join("git").join("config");
        if path.exists() {
            return Some(path);
        }
    }

    // Fall back to ~/.config/git/config
    dirs::home_dir().map(|home| home.join(".config").join("git").join("config"))
}

/// Loads the home directory path.
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            std::env::var("USERPROFILE")
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME").ok().map(PathBuf::from)
                })
        }
        #[cfg(not(windows))]
        {
            std::env::var("HOME").ok().map(PathBuf::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_new() {
        let config = Config::new();
        assert!(config.sections().is_empty());
    }

    #[test]
    fn test_config_set_and_get() {
        let mut config = Config::new();
        config.set("user", "", "name", "John Doe");
        config.set("user", "", "email", "john@example.com");

        assert_eq!(config.get("user", "name"), Some("John Doe"));
        assert_eq!(config.get("user", "email"), Some("john@example.com"));
        assert_eq!(config.get("user", "nonexistent"), None);
    }

    #[test]
    fn test_config_case_insensitive_section_and_key() {
        let mut config = Config::new();
        config.set("User", "", "Name", "John Doe");

        // Section and key lookups should be case-insensitive
        assert_eq!(config.get("user", "name"), Some("John Doe"));
        assert_eq!(config.get("USER", "NAME"), Some("John Doe"));
        assert_eq!(config.get("User", "Name"), Some("John Doe"));
    }

    #[test]
    fn test_config_subsection() {
        let mut config = Config::new();
        config.set("remote", "origin", "url", "https://github.com/test/repo");
        config.set("remote", "upstream", "url", "https://github.com/upstream/repo");

        assert_eq!(
            config.get_subsection("remote", "origin", "url"),
            Some("https://github.com/test/repo")
        );
        assert_eq!(
            config.get_subsection("remote", "upstream", "url"),
            Some("https://github.com/upstream/repo")
        );
        // Subsection names are case-sensitive
        assert_eq!(config.get_subsection("remote", "Origin", "url"), None);
    }

    #[test]
    fn test_parse_bool() {
        assert_eq!(parse_bool("true").unwrap(), true);
        assert_eq!(parse_bool("True").unwrap(), true);
        assert_eq!(parse_bool("TRUE").unwrap(), true);
        assert_eq!(parse_bool("yes").unwrap(), true);
        assert_eq!(parse_bool("on").unwrap(), true);
        assert_eq!(parse_bool("1").unwrap(), true);

        assert_eq!(parse_bool("false").unwrap(), false);
        assert_eq!(parse_bool("False").unwrap(), false);
        assert_eq!(parse_bool("no").unwrap(), false);
        assert_eq!(parse_bool("off").unwrap(), false);
        assert_eq!(parse_bool("0").unwrap(), false);
        assert_eq!(parse_bool("").unwrap(), false);

        assert!(parse_bool("invalid").is_err());
    }

    #[test]
    fn test_parse_int() {
        assert_eq!(parse_int("42").unwrap(), 42);
        assert_eq!(parse_int("-10").unwrap(), -10);
        assert_eq!(parse_int("0").unwrap(), 0);
        assert_eq!(parse_int("").unwrap(), 0);

        // With suffixes
        assert_eq!(parse_int("1k").unwrap(), 1024);
        assert_eq!(parse_int("2K").unwrap(), 2048);
        assert_eq!(parse_int("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_int("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_int("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_int("1G").unwrap(), 1024 * 1024 * 1024);

        assert!(parse_int("abc").is_err());
    }

    #[test]
    fn test_config_get_bool() {
        let mut config = Config::new();
        config.set("core", "", "autocrlf", "true");
        config.set("core", "", "bare", "false");

        assert_eq!(config.get_bool("core", "autocrlf").unwrap(), true);
        assert_eq!(config.get_bool("core", "bare").unwrap(), false);
        // Non-existent key returns false
        assert_eq!(config.get_bool("core", "nonexistent").unwrap(), false);
    }

    #[test]
    fn test_config_get_int() {
        let mut config = Config::new();
        config.set("http", "", "postBuffer", "100m");

        assert_eq!(config.get_int("http", "postBuffer").unwrap(), 100 * 1024 * 1024);
        // Non-existent key returns 0
        assert_eq!(config.get_int("http", "nonexistent").unwrap(), 0);
    }

    #[test]
    fn test_config_sections() {
        let mut config = Config::new();
        config.set("core", "", "bare", "false");
        config.set("user", "", "name", "John");
        config.set("remote", "origin", "url", "https://example.com");

        let sections = config.sections();
        assert!(sections.contains(&"core"));
        assert!(sections.contains(&"user"));
        assert!(sections.contains(&"remote"));
    }

    #[test]
    fn test_config_subsections() {
        let mut config = Config::new();
        config.set("remote", "origin", "url", "https://example.com");
        config.set("remote", "upstream", "url", "https://upstream.com");
        config.set("remote", "", "default", "origin");

        let subsections = config.subsections("remote");
        assert!(subsections.contains(&"origin"));
        assert!(subsections.contains(&"upstream"));
        // Empty subsection is excluded
        assert!(!subsections.contains(&""));
    }

    #[test]
    fn test_config_keys() {
        let mut config = Config::new();
        config.set("user", "", "name", "John");
        config.set("user", "", "email", "john@example.com");

        let keys = config.keys("user");
        assert!(keys.contains(&"name"));
        assert!(keys.contains(&"email"));
    }

    #[test]
    fn test_config_merge() {
        let mut config1 = Config::new();
        config1.set("user", "", "name", "John");
        config1.set("core", "", "bare", "false");

        let mut config2 = Config::new();
        config2.set("user", "", "name", "Jane"); // Override
        config2.set("user", "", "email", "jane@example.com"); // New

        config1.merge(&config2);

        assert_eq!(config1.get("user", "name"), Some("Jane"));
        assert_eq!(config1.get("user", "email"), Some("jane@example.com"));
        assert_eq!(config1.get("core", "bare"), Some("false"));
    }
}
