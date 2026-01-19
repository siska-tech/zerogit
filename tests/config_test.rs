//! Integration tests for Config module.

use std::fs;
use tempfile::TempDir;
use zerogit::config::{Config, ConfigLevel};
use zerogit::repository::Repository;

/// Path to the simple test fixture
const SIMPLE_FIXTURE: &str = "tests/fixtures/simple";

// CF-001: Config::from_str parses basic config
#[test]
fn test_cf001_from_str_basic() {
    let content = r#"
[core]
    bare = false
    repositoryformatversion = 0
[user]
    name = Test User
    email = test@example.com
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("core", "bare"), Some("false"));
    assert_eq!(config.get("core", "repositoryformatversion"), Some("0"));
    assert_eq!(config.get("user", "name"), Some("Test User"));
    assert_eq!(config.get("user", "email"), Some("test@example.com"));
}

// CF-002: Config::from_str parses sections with subsections
#[test]
fn test_cf002_subsections() {
    let content = r#"
[remote "origin"]
    url = https://github.com/user/repo.git
    fetch = +refs/heads/*:refs/remotes/origin/*
[remote "upstream"]
    url = https://github.com/upstream/repo.git
[branch "main"]
    remote = origin
    merge = refs/heads/main
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(
        config.get_subsection("remote", "origin", "url"),
        Some("https://github.com/user/repo.git")
    );
    assert_eq!(
        config.get_subsection("remote", "upstream", "url"),
        Some("https://github.com/upstream/repo.git")
    );
    assert_eq!(
        config.get_subsection("branch", "main", "remote"),
        Some("origin")
    );
}

// CF-003: Config::get_bool parses boolean values correctly
#[test]
fn test_cf003_get_bool() {
    let content = r#"
[core]
    bare = false
    autocrlf = true
    symlinks = yes
    ignorecase = no
    precomposeunicode = on
    logallrefupdates = off
    filemode = 1
    excludesfile = 0
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get_bool("core", "bare").unwrap(), false);
    assert_eq!(config.get_bool("core", "autocrlf").unwrap(), true);
    assert_eq!(config.get_bool("core", "symlinks").unwrap(), true);
    assert_eq!(config.get_bool("core", "ignorecase").unwrap(), false);
    assert_eq!(config.get_bool("core", "precomposeunicode").unwrap(), true);
    assert_eq!(config.get_bool("core", "logallrefupdates").unwrap(), false);
    assert_eq!(config.get_bool("core", "filemode").unwrap(), true);
    assert_eq!(config.get_bool("core", "excludesfile").unwrap(), false);
    // Non-existent key returns false
    assert_eq!(config.get_bool("core", "nonexistent").unwrap(), false);
}

// CF-004: Config::get_int parses integer values with suffixes
#[test]
fn test_cf004_get_int() {
    let content = r#"
[http]
    postBuffer = 524288000
    lowSpeedLimit = 1000
[core]
    packedGitLimit = 256m
    packedGitWindowSize = 1k
    bigFileThreshold = 1g
    compression = -1
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get_int("http", "postBuffer").unwrap(), 524288000);
    assert_eq!(config.get_int("http", "lowSpeedLimit").unwrap(), 1000);
    assert_eq!(
        config.get_int("core", "packedGitLimit").unwrap(),
        256 * 1024 * 1024
    );
    assert_eq!(
        config.get_int("core", "packedGitWindowSize").unwrap(),
        1024
    );
    assert_eq!(
        config.get_int("core", "bigFileThreshold").unwrap(),
        1024 * 1024 * 1024
    );
    assert_eq!(config.get_int("core", "compression").unwrap(), -1);
    // Non-existent key returns 0
    assert_eq!(config.get_int("http", "nonexistent").unwrap(), 0);
}

// CF-005: Config sections and keys are case-insensitive
#[test]
fn test_cf005_case_insensitive() {
    let content = r#"
[CORE]
    BARE = false
[User]
    Name = John Doe
"#;

    let config = Config::from_str(content).unwrap();

    // Section and key lookups are case-insensitive
    assert_eq!(config.get("core", "bare"), Some("false"));
    assert_eq!(config.get("CORE", "BARE"), Some("false"));
    assert_eq!(config.get("Core", "Bare"), Some("false"));
    assert_eq!(config.get("user", "name"), Some("John Doe"));
    assert_eq!(config.get("USER", "NAME"), Some("John Doe"));
}

// CF-006: Subsection names are case-sensitive
#[test]
fn test_cf006_subsection_case_sensitive() {
    let content = r#"
[remote "Origin"]
    url = https://origin.example.com
[remote "origin"]
    url = https://origin-lower.example.com
"#;

    let config = Config::from_str(content).unwrap();

    // Subsection names preserve case
    assert_eq!(
        config.get_subsection("remote", "Origin", "url"),
        Some("https://origin.example.com")
    );
    assert_eq!(
        config.get_subsection("remote", "origin", "url"),
        Some("https://origin-lower.example.com")
    );
}

// CF-007: Config::from_file reads file correctly
#[test]
fn test_cf007_from_file() {
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config");

    fs::write(
        &config_path,
        r#"
[core]
    bare = false
[user]
    name = File Test
"#,
    )
    .unwrap();

    let config = Config::from_file(&config_path).unwrap();

    assert_eq!(config.get("core", "bare"), Some("false"));
    assert_eq!(config.get("user", "name"), Some("File Test"));
}

// CF-008: Config handles comments correctly
#[test]
fn test_cf008_comments() {
    let content = r#"
# This is a file comment
; This is also a comment
[core]
    # Comment in section
    bare = false ; inline comment
    ; Another comment
    repositoryformatversion = 0 # another inline
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("core", "bare"), Some("false"));
    assert_eq!(config.get("core", "repositoryformatversion"), Some("0"));
}

// CF-009: Config handles quoted values
#[test]
fn test_cf009_quoted_values() {
    let content = r#"
[user]
    name = "John Doe"
    email = "john@example.com"
[alias]
    lg = "log --graph --oneline"
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("user", "name"), Some("John Doe"));
    assert_eq!(config.get("alias", "lg"), Some("log --graph --oneline"));
}

// CF-010: Config handles escape sequences
#[test]
fn test_cf010_escape_sequences() {
    let content = r#"
[section]
    newline = Hello\nWorld
    tab = Hello\tWorld
    backslash = C:\\Users\\Test
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("section", "newline"), Some("Hello\nWorld"));
    assert_eq!(config.get("section", "tab"), Some("Hello\tWorld"));
    assert_eq!(config.get("section", "backslash"), Some("C:\\Users\\Test"));
}

// CF-011: Config::merge correctly overrides values
#[test]
fn test_cf011_merge() {
    let base_content = r#"
[user]
    name = Base User
    email = base@example.com
[core]
    bare = false
"#;

    let override_content = r#"
[user]
    name = Override User
[new]
    key = value
"#;

    let mut base = Config::from_str(base_content).unwrap();
    let override_config = Config::from_str(override_content).unwrap();

    base.merge(&override_config);

    assert_eq!(base.get("user", "name"), Some("Override User"));
    assert_eq!(base.get("user", "email"), Some("base@example.com"));
    assert_eq!(base.get("core", "bare"), Some("false"));
    assert_eq!(base.get("new", "key"), Some("value"));
}

// CF-012: Config::sections returns all section names
#[test]
fn test_cf012_sections() {
    let content = r#"
[core]
    bare = false
[user]
    name = Test
[remote "origin"]
    url = https://example.com
"#;

    let config = Config::from_str(content).unwrap();
    let sections = config.sections();

    assert!(sections.contains(&"core"));
    assert!(sections.contains(&"user"));
    assert!(sections.contains(&"remote"));
}

// CF-013: Config::subsections returns subsection names
#[test]
fn test_cf013_subsections() {
    let content = r#"
[remote "origin"]
    url = https://origin.example.com
[remote "upstream"]
    url = https://upstream.example.com
"#;

    let config = Config::from_str(content).unwrap();
    let subsections = config.subsections("remote");

    assert!(subsections.contains(&"origin"));
    assert!(subsections.contains(&"upstream"));
}

// CF-014: Config::keys returns keys in a section
#[test]
fn test_cf014_keys() {
    let content = r#"
[user]
    name = Test
    email = test@example.com
    signingkey = ABCD1234
"#;

    let config = Config::from_str(content).unwrap();
    let keys = config.keys("user");

    assert!(keys.contains(&"name"));
    assert!(keys.contains(&"email"));
    assert!(keys.contains(&"signingkey"));
}

// CF-015: ConfigLevel::default_path returns expected paths
#[test]
fn test_cf015_config_level_paths() {
    // Global config should point to home directory
    let global_path = ConfigLevel::Global.default_path();
    assert!(global_path.is_some());
    if let Some(path) = global_path {
        assert!(path.to_string_lossy().contains(".gitconfig"));
    }

    // System config path depends on platform
    #[cfg(unix)]
    {
        let system_path = ConfigLevel::System.default_path();
        assert_eq!(
            system_path,
            Some(std::path::PathBuf::from("/etc/gitconfig"))
        );
    }

    // Local config needs repository context
    let local_path = ConfigLevel::Local.default_path();
    assert!(local_path.is_none());
}

// CF-016: Repository::config() returns merged configuration
#[test]
fn test_cf016_repository_config() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();
    let config = repo.config();

    // Should succeed even if only local config exists
    assert!(config.is_ok());
}

// CF-017: Repository::config_local() returns only local configuration
#[test]
fn test_cf017_repository_config_local() {
    let repo = Repository::open(SIMPLE_FIXTURE).unwrap();
    let config = repo.config_local();

    assert!(config.is_ok());
    let config = config.unwrap();

    // Check that we can read some basic config
    // The exact values depend on the fixture, but core section should exist
    assert!(config.sections().contains(&"core"));
}

// CF-018: Config handles values with equals signs
#[test]
fn test_cf018_value_with_equals() {
    let content = r#"
[alias]
    st = status --short
    lg = log --oneline --graph --all
[filter "lfs"]
    clean = git-lfs clean -- %f
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("alias", "st"), Some("status --short"));
    assert_eq!(config.get("alias", "lg"), Some("log --oneline --graph --all"));
    assert_eq!(
        config.get_subsection("filter", "lfs", "clean"),
        Some("git-lfs clean -- %f")
    );
}

// CF-019: Config handles empty values
#[test]
fn test_cf019_empty_values() {
    let content = r#"
[section]
    empty =
    novalue
"#;

    let config = Config::from_str(content).unwrap();

    assert_eq!(config.get("section", "empty"), Some(""));
    // Keys without = are not parsed as key-value pairs
    assert_eq!(config.get("section", "novalue"), None);
}

// CF-020: Config::from_file_with_includes processes include directives
#[test]
fn test_cf020_include_directive() {
    let temp = TempDir::new().unwrap();

    // Create included config file
    let included_path = temp.path().join("included.gitconfig");
    fs::write(
        &included_path,
        r#"
[user]
    name = Included User
"#,
    )
    .unwrap();

    // Create main config file with include directive
    let main_path = temp.path().join("config");
    fs::write(
        &main_path,
        format!(
            r#"
[include]
    path = {}
[core]
    bare = false
"#,
            included_path.display()
        ),
    )
    .unwrap();

    let config = Config::from_file_with_includes(&main_path).unwrap();

    assert_eq!(config.get("core", "bare"), Some("false"));
    assert_eq!(config.get("user", "name"), Some("Included User"));
}

// CF-021: Config handles relative include paths
#[test]
fn test_cf021_relative_include_path() {
    let temp = TempDir::new().unwrap();

    // Create included config file in subdirectory
    let subdir = temp.path().join("subdir");
    fs::create_dir(&subdir).unwrap();
    let included_path = subdir.join("extra.gitconfig");
    fs::write(
        &included_path,
        r#"
[extra]
    value = from-include
"#,
    )
    .unwrap();

    // Create main config file with relative include path
    let main_path = temp.path().join("config");
    fs::write(
        &main_path,
        r#"
[include]
    path = subdir/extra.gitconfig
[main]
    value = from-main
"#,
    )
    .unwrap();

    let config = Config::from_file_with_includes(&main_path).unwrap();

    assert_eq!(config.get("main", "value"), Some("from-main"));
    assert_eq!(config.get("extra", "value"), Some("from-include"));
}

// CF-022: Config handles missing include files gracefully
#[test]
fn test_cf022_missing_include() {
    let temp = TempDir::new().unwrap();

    // Create main config file with include to non-existent file
    let main_path = temp.path().join("config");
    fs::write(
        &main_path,
        r#"
[include]
    path = /nonexistent/file.gitconfig
[core]
    bare = false
"#,
    )
    .unwrap();

    // Should succeed, ignoring missing include
    let config = Config::from_file_with_includes(&main_path).unwrap();

    assert_eq!(config.get("core", "bare"), Some("false"));
}
