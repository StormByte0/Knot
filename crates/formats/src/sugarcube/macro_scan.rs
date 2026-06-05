//! Macro scanning and position detection for SugarCube.
//!
//! Provides regex-based detection of `<<macro>>` / `<</macro>>` constructs
//! on a single line, used by:
//!
//! - `find_macro_at_position()` — hover/completion trigger detection
//! - `scan_line_for_macro_events()` — folding range computation
//!
//! The regexes here are also used by the TextMate grammar and must stay
//! in sync with the `.tmLanguage.json` definitions.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::plugin::{MacroAtPosition, MacroBlockEvent};

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// Regex for open macro tags: <<name args>>
pub(crate) static RE_MACRO_OPEN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+((?:[^>]|>[^>])*?))?>>").unwrap()
});

/// Regex for close macro tags: <</name>>
pub(crate) static RE_MACRO_CLOSE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap()
});

// ---------------------------------------------------------------------------
// Macro position detection (hover / completion trigger)
// ---------------------------------------------------------------------------

/// Find the `<<macro>>` or `<</macro>>` construct at a given byte position
/// on a line.
///
/// `byte_pos` is a UTF-16 code unit offset (from LSP). We convert to a byte
/// offset for string comparison, then return byte ranges that the handler
/// converts back to UTF-16 for LSP responses.
pub(crate) fn find_macro_at_position(
    line: &str,
    byte_pos: usize,
) -> Option<MacroAtPosition> {
    let mut search_from = 0;
    while let Some(rel_start) = line[search_from..].find("<<") {
        let abs_start = search_from + rel_start;

        // Check for close-tag: <</name>>
        if line[abs_start..].starts_with("<</") {
            if let Some(rel_end) = line[abs_start..].find(">>") {
                let abs_end = abs_start + rel_end + 2;

                if byte_pos >= abs_start && byte_pos <= abs_end {
                    let inner = &line[abs_start + 3..abs_end - 2];
                    let name = inner.split_whitespace().next().unwrap_or(inner).trim();
                    let name_byte_start = abs_start + 3;
                    let name_byte_end = name_byte_start + name.len();
                    return Some(MacroAtPosition {
                        name: name.to_string(),
                        full_range: abs_start..abs_end,
                        name_range: name_byte_start..name_byte_end,
                        is_unclosed: false,
                    });
                }
                search_from = abs_end;
                continue;
            }
        }

        // Open tag: <<name args>>
        if let Some(rel_end) = line[abs_start..].find(">>") {
            let abs_end = abs_start + rel_end + 2;

            if byte_pos >= abs_start && byte_pos <= abs_end {
                let content_start = abs_start + 2;
                let content_end = abs_end - 2;
                let content = &line[content_start..content_end];
                let macro_name = content.split_whitespace().next().unwrap_or(content).trim();
                let name_byte_start = content_start;
                let name_byte_end = content_start + macro_name.len();
                return Some(MacroAtPosition {
                    name: macro_name.to_string(),
                    full_range: abs_start..abs_end,
                    name_range: name_byte_start..name_byte_end,
                    is_unclosed: false,
                });
            }
            search_from = abs_end;
        } else {
            // Unclosed macro — cursor might be inside
            if byte_pos >= abs_start {
                let content_start = abs_start + 2;
                let content = &line[content_start..];
                let macro_name = content.split_whitespace().next().unwrap_or(content).trim();
                let name_byte_start = content_start;
                let name_byte_end = content_start + macro_name.len();
                return Some(MacroAtPosition {
                    name: macro_name.to_string(),
                    full_range: abs_start..line.len(),
                    name_range: name_byte_start..name_byte_end,
                    is_unclosed: true,
                });
            }
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Macro block event scanning (folding ranges)
// ---------------------------------------------------------------------------

/// Scan a line for macro open/close events, used to compute folding ranges.
///
/// Only block macros (those with matching close tags like `<<if>>…<</if>>`)
/// produce open events; all close tags produce close events.
pub(crate) fn scan_line_for_macro_events(
    line: &str,
    line_idx: u32,
    block_macro_names: &HashSet<&'static str>,
) -> Vec<MacroBlockEvent> {
    let mut events = Vec::new();

    // Open macros: <<name ...>> — use the same regex as the TextMate grammar
    for caps in RE_MACRO_OPEN.captures_iter(line) {
        if let Some(name_match) = caps.get(1) {
            let name = name_match.as_str();
            if block_macro_names.contains(name) {
                events.push(MacroBlockEvent {
                    name: name.to_string(),
                    line: line_idx,
                    is_open: true,
                });
            }
        }
    }

    // Close macros: <</name>>
    for caps in RE_MACRO_CLOSE.captures_iter(line) {
        if let Some(name_match) = caps.get(1) {
            let name = name_match.as_str();
            events.push(MacroBlockEvent {
                name: name.to_string(),
                line: line_idx,
                is_open: false,
            });
        }
    }

    events
}
