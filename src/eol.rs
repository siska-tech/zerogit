//! End-of-line conversion between the working tree and the repository.
//!
//! This follows Git's `convert.c`: the `text`, `eol` and legacy `crlf`
//! attributes, `core.autocrlf` and `core.eol` decide a conversion for each
//! path. Content added to the repository has CRLF converted to LF ("clean");
//! content checked out has LF converted to CRLF when the path's line ending is
//! CRLF ("smudge"). For automatic text detection (`text=auto` or
//! `core.autocrlf`), files that look binary are left alone, as are files
//! whose version in the index already contains CRLF.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::attributes::AttrValue;
use crate::config::Config;

/// The attributes the conversion depends on.
pub(crate) const ATTRIBUTES: &[&str] = &["text", "crlf", "eol"];

/// `core.autocrlf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoCrlf {
    False,
    True,
    Input,
}

/// Line ending written to the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eol {
    Lf,
    Crlf,
}

/// The conversion chosen for a path (Git's `crlf_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrlfAction {
    /// No conversion.
    Binary,
    /// Text; line ending from `core.eol`/`core.autocrlf`.
    Text,
    /// Text with LF in the working tree.
    TextInput,
    /// Text with CRLF in the working tree.
    TextCrlf,
    /// Detected text; line ending from `core.eol`/`core.autocrlf`.
    Auto,
    /// Detected text with LF in the working tree.
    AutoInput,
    /// Detected text with CRLF in the working tree.
    AutoCrlf,
}

impl CrlfAction {
    fn is_auto(self) -> bool {
        matches!(
            self,
            CrlfAction::Auto | CrlfAction::AutoInput | CrlfAction::AutoCrlf
        )
    }
}

/// The repository-wide line ending settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EolSettings {
    autocrlf: AutoCrlf,
    /// `core.eol`; `None` is "native".
    core_eol: Option<Eol>,
    /// `core.safecrlf=true`: refuse an irreversible conversion on add.
    pub(crate) safecrlf: bool,
}

impl EolSettings {
    pub(crate) fn from_config(config: &Config) -> Self {
        let autocrlf = match config.get("core", "autocrlf") {
            Some(v) if v.eq_ignore_ascii_case("input") => AutoCrlf::Input,
            Some(_) if config.get_bool_or("core", "autocrlf", false) => AutoCrlf::True,
            _ => AutoCrlf::False,
        };
        let core_eol = match config.get("core", "eol") {
            Some(v) if v.eq_ignore_ascii_case("lf") => Some(Eol::Lf),
            Some(v) if v.eq_ignore_ascii_case("crlf") => Some(Eol::Crlf),
            _ => None,
        };
        // "warn" (the default) only warns; "true" refuses.
        let safecrlf = config.get_bool_or("core", "safecrlf", false);
        EolSettings {
            autocrlf,
            core_eol,
            safecrlf,
        }
    }

    /// Decides the conversion for a path from its attributes.
    pub(crate) fn action(&self, attrs: &HashMap<String, AttrValue>) -> CrlfAction {
        let from_attr = |value: Option<&AttrValue>| match value {
            Some(AttrValue::Set) => Some(CrlfAction::Text),
            Some(AttrValue::Unset) => Some(CrlfAction::Binary),
            Some(AttrValue::Value(v)) if v == "input" => Some(CrlfAction::TextInput),
            Some(AttrValue::Value(v)) if v == "auto" => Some(CrlfAction::Auto),
            _ => None,
        };
        let mut action = from_attr(attrs.get("text")).or_else(|| from_attr(attrs.get("crlf")));
        let eol = match attrs.get("eol") {
            Some(AttrValue::Value(v)) if v == "lf" => Some(Eol::Lf),
            Some(AttrValue::Value(v)) if v == "crlf" => Some(Eol::Crlf),
            _ => None,
        };
        // The eol attribute fixes the line ending, and implies text when
        // `text` is unspecified.
        action = match (action, eol) {
            (Some(CrlfAction::Text) | None, Some(Eol::Lf)) => Some(CrlfAction::TextInput),
            (Some(CrlfAction::Text) | None, Some(Eol::Crlf)) => Some(CrlfAction::TextCrlf),
            (Some(CrlfAction::Auto), Some(Eol::Lf)) => Some(CrlfAction::AutoInput),
            (Some(CrlfAction::Auto), Some(Eol::Crlf)) => Some(CrlfAction::AutoCrlf),
            (action, _) => action,
        };
        action.unwrap_or(match self.autocrlf {
            AutoCrlf::False => CrlfAction::Binary,
            AutoCrlf::Input => CrlfAction::AutoInput,
            AutoCrlf::True => CrlfAction::AutoCrlf,
        })
    }

    fn output_eol(&self, action: CrlfAction) -> Option<Eol> {
        match action {
            CrlfAction::Binary => None,
            CrlfAction::TextCrlf | CrlfAction::AutoCrlf => Some(Eol::Crlf),
            CrlfAction::TextInput | CrlfAction::AutoInput => Some(Eol::Lf),
            CrlfAction::Text | CrlfAction::Auto => {
                let crlf = match (self.autocrlf, self.core_eol) {
                    (AutoCrlf::True, _) => true,
                    (AutoCrlf::Input, _) => false,
                    (_, Some(eol)) => eol == Eol::Crlf,
                    (_, None) => cfg!(windows),
                };
                Some(if crlf { Eol::Crlf } else { Eol::Lf })
            }
        }
    }
}

/// Character statistics of content (Git's `struct text_stat`).
#[derive(Debug, Clone, Copy, Default)]
struct Stats {
    nul: usize,
    lonecr: usize,
    lonelf: usize,
    crlf: usize,
    printable: usize,
    nonprintable: usize,
}

fn gather_stats(buf: &[u8]) -> Stats {
    let mut stats = Stats::default();
    let mut i = 0;
    while i < buf.len() {
        let c = buf[i];
        if c == b'\r' {
            if buf.get(i + 1) == Some(&b'\n') {
                stats.crlf += 1;
                i += 1;
            } else {
                stats.lonecr += 1;
            }
        } else if c == b'\n' {
            stats.lonelf += 1;
        } else if c == 127 {
            stats.nonprintable += 1;
        } else if c < 32 {
            match c {
                // BS, HT, ESC and FF
                b'\x08' | b'\t' | b'\x1b' | b'\x0c' => stats.printable += 1,
                0 => {
                    stats.nul += 1;
                    stats.nonprintable += 1;
                }
                _ => stats.nonprintable += 1,
            }
        } else {
            stats.printable += 1;
        }
        i += 1;
    }
    // A trailing EOF character (^Z) is not counted.
    if buf.last() == Some(&0x1a) {
        stats.nonprintable = stats.nonprintable.saturating_sub(1);
    }
    stats
}

fn is_binary(stats: &Stats) -> bool {
    stats.lonecr > 0 || stats.nul > 0 || (stats.printable >> 7) < stats.nonprintable
}

/// Whether Git counts content as text containing CRLF (used to leave such
/// files alone under automatic conversion).
pub(crate) fn has_crlf_text(content: &[u8]) -> bool {
    if !content.contains(&b'\r') {
        return false;
    }
    let stats = gather_stats(content);
    !is_binary(&stats) && stats.crlf > 0
}

fn will_convert_lf_to_crlf(stats: &Stats, action: CrlfAction) -> bool {
    if stats.lonelf == 0 {
        return false;
    }
    if action.is_auto() && (stats.lonecr > 0 || stats.crlf > 0 || is_binary(stats)) {
        return false;
    }
    true
}

/// The result of converting work tree content for the repository.
#[derive(Debug)]
pub(crate) struct Cleaned<'a> {
    pub(crate) content: Cow<'a, [u8]>,
    /// Checking the content out again would not reproduce the file
    /// (what `core.safecrlf` reports).
    pub(crate) irreversible: bool,
}

/// Converts work tree content to repository content (CRLF to LF).
///
/// `index_has_crlf` is asked only when automatic conversion applies to text
/// containing CRLF; it reports whether the path's blob in the index is text
/// with CRLF, in which case the file is left as it is.
pub(crate) fn to_git<'a>(
    settings: &EolSettings,
    action: CrlfAction,
    content: &'a [u8],
    index_has_crlf: impl FnOnce() -> bool,
) -> Cleaned<'a> {
    let unchanged = Cleaned {
        content: Cow::Borrowed(content),
        irreversible: false,
    };
    if action == CrlfAction::Binary || content.is_empty() {
        return unchanged;
    }
    let stats = gather_stats(content);
    let mut convert = true;
    if action.is_auto() {
        if is_binary(&stats) {
            return unchanged;
        }
        if stats.crlf > 0 && index_has_crlf() {
            convert = false;
        }
    }

    // Simulate add and checkout to see whether the file survives.
    let mut round_trip = stats;
    if convert {
        round_trip.lonelf += round_trip.crlf;
        round_trip.crlf = 0;
    }
    if settings.output_eol(action) == Some(Eol::Crlf)
        && will_convert_lf_to_crlf(&round_trip, action)
    {
        round_trip.crlf += round_trip.lonelf;
        round_trip.lonelf = 0;
    }
    let irreversible =
        (stats.crlf > 0 && round_trip.crlf == 0) || (stats.lonelf > 0 && round_trip.lonelf == 0);

    if !convert || stats.crlf == 0 {
        return Cleaned {
            content: Cow::Borrowed(content),
            irreversible,
        };
    }
    let mut out = Vec::with_capacity(content.len() - stats.crlf);
    if action.is_auto() {
        // Detected text has no lone CR, so every CR precedes LF.
        out.extend(content.iter().copied().filter(|&c| c != b'\r'));
    } else {
        for (i, &c) in content.iter().enumerate() {
            if !(c == b'\r' && content.get(i + 1) == Some(&b'\n')) {
                out.push(c);
            }
        }
    }
    Cleaned {
        content: Cow::Owned(out),
        irreversible,
    }
}

/// Converts repository content to work tree content (LF to CRLF where the
/// path's line ending is CRLF).
pub(crate) fn to_worktree<'a>(
    settings: &EolSettings,
    action: CrlfAction,
    content: &'a [u8],
) -> Cow<'a, [u8]> {
    if content.is_empty() || settings.output_eol(action) != Some(Eol::Crlf) {
        return Cow::Borrowed(content);
    }
    let stats = gather_stats(content);
    if !will_convert_lf_to_crlf(&stats, action) {
        return Cow::Borrowed(content);
    }
    let mut out = Vec::with_capacity(content.len() + stats.lonelf);
    let mut previous = 0u8;
    for &c in content {
        if c == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(c);
        previous = c;
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(autocrlf: &str, eol: Option<&str>) -> EolSettings {
        let mut text = format!("[core]\n\tautocrlf = {}\n", autocrlf);
        if let Some(eol) = eol {
            text.push_str(&format!("\teol = {}\n", eol));
        }
        EolSettings::from_config(&Config::from_str(&text).unwrap())
    }

    fn attrs(pairs: &[(&str, AttrValue)]) -> HashMap<String, AttrValue> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn test_action_from_settings_and_attributes() {
        let none = attrs(&[]);
        assert_eq!(settings("false", None).action(&none), CrlfAction::Binary);
        assert_eq!(settings("true", None).action(&none), CrlfAction::AutoCrlf);
        assert_eq!(settings("input", None).action(&none), CrlfAction::AutoInput);
        let s = settings("false", None);
        assert_eq!(
            s.action(&attrs(&[("text", AttrValue::Set)])),
            CrlfAction::Text
        );
        assert_eq!(
            s.action(&attrs(&[("text", AttrValue::Unset)])),
            CrlfAction::Binary
        );
        assert_eq!(
            s.action(&attrs(&[("text", AttrValue::Value("auto".into()))])),
            CrlfAction::Auto
        );
        assert_eq!(
            s.action(&attrs(&[("eol", AttrValue::Value("crlf".into()))])),
            CrlfAction::TextCrlf
        );
        assert_eq!(
            s.action(&attrs(&[
                ("text", AttrValue::Value("auto".into())),
                ("eol", AttrValue::Value("lf".into()))
            ])),
            CrlfAction::AutoInput
        );
        assert_eq!(
            s.action(&attrs(&[("crlf", AttrValue::Value("input".into()))])),
            CrlfAction::TextInput
        );
        // -text wins over eol.
        assert_eq!(
            settings("true", None).action(&attrs(&[
                ("text", AttrValue::Unset),
                ("eol", AttrValue::Value("crlf".into()))
            ])),
            CrlfAction::Binary
        );
    }

    #[test]
    fn test_round_trip_with_autocrlf() {
        let s = settings("true", None);
        let worktree = to_worktree(&s, CrlfAction::AutoCrlf, b"a\nb\n");
        assert_eq!(&*worktree, b"a\r\nb\r\n");
        let cleaned = to_git(&s, CrlfAction::AutoCrlf, &worktree, || false);
        assert_eq!(&*cleaned.content, b"a\nb\n");
        assert!(!cleaned.irreversible);
    }

    #[test]
    fn test_auto_leaves_binary_and_index_crlf_alone() {
        let s = settings("true", None);
        let binary = b"a\r\nb\0";
        assert_eq!(
            &*to_git(&s, CrlfAction::AutoCrlf, binary, || false).content,
            binary
        );
        assert_eq!(
            &*to_worktree(&s, CrlfAction::AutoCrlf, b"a\nb\0"),
            b"a\nb\0"
        );
        let text = b"a\r\nb\r\n";
        assert_eq!(
            &*to_git(&s, CrlfAction::AutoCrlf, text, || true).content,
            text
        );
        // A lone CR makes detected text binary.
        assert_eq!(
            &*to_git(&s, CrlfAction::AutoCrlf, b"a\rb\r\n", || false).content,
            b"a\rb\r\n"
        );
        // Explicit text converts CRLF and keeps a lone CR.
        assert_eq!(
            &*to_git(&s, CrlfAction::Text, b"a\rb\r\n", || false).content,
            b"a\rb\n"
        );
    }

    #[test]
    fn test_mixed_line_endings_are_irreversible() {
        let s = settings("true", None);
        // With CRLF output, the LF-only line would come back as CRLF.
        let cleaned = to_git(&s, CrlfAction::TextCrlf, b"a\r\nb\n", || false);
        assert_eq!(&*cleaned.content, b"a\nb\n");
        assert!(cleaned.irreversible);
        // With LF output, the CRLF line would come back as LF.
        let s = settings("false", Some("lf"));
        let cleaned = to_git(&s, CrlfAction::Text, b"a\r\nb\n", || false);
        assert!(cleaned.irreversible);
    }

    #[test]
    fn test_input_never_writes_crlf() {
        let s = settings("input", None);
        assert_eq!(&*to_worktree(&s, CrlfAction::AutoInput, b"a\n"), b"a\n");
        assert_eq!(
            &*to_git(&s, CrlfAction::AutoInput, b"a\r\n", || false).content,
            b"a\n"
        );
    }
}
