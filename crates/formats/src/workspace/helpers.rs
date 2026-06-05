//! Format-agnostic text helpers.
//!
//! These utilities operate on plain text and are shared across format plugins
//! and workspace-level analyses. They are intentionally format-agnostic —
//! no SugarCube-specific or Harlowe-specific logic belongs here.

use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Comment stripping (shared across all formats — JS comments are universal)
// ---------------------------------------------------------------------------

static RE_LINE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"//[^\n]*").unwrap());
static RE_BLOCK_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/\*[\s\S]*?\*/").unwrap());

/// Strip JS comments from source text before alias extraction.
///
/// This is format-agnostic because all Twine formats use JavaScript in
/// `[script]` passages, and JS comments are the same regardless of the
/// story format layered on top.
pub fn strip_comments(src: &str) -> String {
    let no_block = RE_BLOCK_COMMENT.replace_all(src, "");
    let no_line = RE_LINE_COMMENT.replace_all(&no_block, "");
    no_line.to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute 0-based line number from a byte offset in a string.
pub fn line_from_offset(text: &str, offset: usize) -> u32 {
    text[..offset.min(text.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count() as u32
}
