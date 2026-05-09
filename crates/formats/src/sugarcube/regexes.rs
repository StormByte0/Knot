//! Lazy-compiled regex statics shared across all SugarCube submodules.
//!
//! All regexes are compiled once via `once_cell::sync::Lazy` and reused across
//! every parse call. They are `pub(crate)` so sibling modules in this crate
//! can access them directly.

use once_cell::sync::Lazy;
use regex::Regex;

// ---------------------------------------------------------------------------
// Link patterns
// ---------------------------------------------------------------------------

/// [[Target]] — simple passage link
pub(crate) static RE_LINK_SIMPLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap());

/// [[Display->Target]] — arrow-style link
pub(crate) static RE_LINK_ARROW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap());

/// [[Display|Target]] — pipe-style link
pub(crate) static RE_LINK_PIPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap());

// ---------------------------------------------------------------------------
// Variable patterns
// ---------------------------------------------------------------------------

/// $variableName or $variableName.property.path — SugarCube persistent variable reference.
/// Capture group 1: root variable name (without `$`).
/// The full match includes any `.property.path` suffix.
pub(crate) static RE_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)").unwrap());

/// _variableName or _variableName.property.path — SugarCube temporary variable reference.
/// Capture group 1: root variable name (without `_`).
/// The full match includes any `.property.path` suffix.
pub(crate) static RE_TEMP_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"_([A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)").unwrap());

/// <<set $var to ...>> or <<set $var.property.path to ...>> — write macro for persistent vars.
/// Capture group 1: root variable name (without `$`). The full match includes
/// any dot-accessed property path.
pub(crate) static RE_SET_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\s+to\b").unwrap());

/// <<set _var to ...>> or <<set _var.property.path to ...>> — write macro for temporary vars.
/// Capture group 1: root variable name (without `_`). The full match includes
/// any dot-accessed property path.
pub(crate) static RE_SET_TEMP_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+_([A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\s+to\b").unwrap());

// ---------------------------------------------------------------------------
// Macro patterns
// ---------------------------------------------------------------------------

/// <<name ...>> — any open macro
pub(crate) static RE_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+([^>]*?))?>>").unwrap());

/// <</name>> — closing macro tag
pub(crate) static RE_MACRO_CLOSE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap());

// ---------------------------------------------------------------------------
// Implicit passage reference patterns
// ---------------------------------------------------------------------------

/// HTML data-passage attribute — implicit passage reference
pub(crate) static RE_DATA_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"data-passage\s*=\s*["']([^"']+)["']"#).unwrap());

/// Engine.play() — implicit passage reference
pub(crate) static RE_ENGINE_PLAY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Engine.goto() — implicit passage reference
pub(crate) static RE_ENGINE_GOTO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.get() — implicit passage reference
pub(crate) static RE_STORY_GET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.passage() — implicit passage reference
pub(crate) static RE_STORY_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["']"#).unwrap());

// ---------------------------------------------------------------------------
// Navigation macro patterns
// ---------------------------------------------------------------------------

/// Navigation macros with a single passage argument: <<goto "target">>,
/// <<include "target">>, <<display "target">>, <<actions "target">>.
/// Supports both double and single quoted strings.
pub(crate) static RE_NAV_MACRO_SINGLE_ARG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"<<(?:goto|include|display|actions)\s+["']([^"']+)["']"#
    ).unwrap()
});

/// Link/button macros with label + passage arguments:
/// <<link "label" "target">>, <<button "label" "target">>,
/// <<linkappend "label" "target">>, <<linkprepend "label" "target">>,
/// <<linkreplace "label" "target">>, <<click "label" "target">>.
/// Supports both double and single quoted strings.
pub(crate) static RE_NAV_MACRO_LABEL_PASSAGE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"<<(?:link|button|linkappend|linkprepend|linkreplace|click)\s+["'][^"']*["']\s+["']([^"']+)["']"#
    ).unwrap()
});
