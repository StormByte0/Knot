//! Syntax detection for SugarCube macro constructs.
//!
//! This module contains functions for detecting macro positions on a line
//! and scanning for macro block open/close events used by the folding-range
//! handler.

use crate::plugin::MacroAtPosition;
use crate::plugin::MacroBlockEvent;
use super::macros;

/// Find the SugarCube macro at a given cursor position on a line.
///
/// Searches for `<<name ...>>` and `<</name>>` patterns on the line,
/// handling nested delimiters and string contexts. Returns information
/// about the macro that contains the cursor position, including the
/// macro name, the full range of the macro construct, the range of
/// just the name, and whether the macro is unclosed.
///
/// This is used by hover, completion, and signature-help handlers to
/// detect which macro the cursor is inside without hardcoding `<<>>`
/// detection logic.
pub(super) fn find_macro_at_position_impl(
    line: &str,
    byte_pos: usize,
) -> Option<MacroAtPosition> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i + 1 < len {
        // Look for << opening
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            let open_start = i;
            i += 2;

            // Check for close tag: <</
            let is_close = i < len && bytes[i] == b'/';
            if is_close {
                i += 1;
            }

            // Skip whitespace after << or <</
            while i < len && bytes[i] == b' ' {
                i += 1;
            }

            // Extract the macro name
            let name_start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-') {
                i += 1;
            }
            let name_end = i;

            if name_start == name_end {
                // No name found (e.g., << with nothing after)
                continue;
            }

            let name = &line[name_start..name_end];

            // Skip whitespace and args to find >>
            let mut depth = 1;
            while i + 1 < len && depth > 0 {
                if bytes[i] == b'<' && bytes[i + 1] == b'<' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'>' && bytes[i + 1] == b'>' {
                    depth -= 1;
                    if depth == 0 {
                        i += 2;
                        break;
                    }
                    i += 2;
                } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                    // Skip string literal
                    let quote = bytes[i];
                    i += 1;
                    while i < len && bytes[i] != quote {
                        if bytes[i] == b'\\' && i + 1 < len {
                            i += 2; // skip escaped char
                        } else {
                            i += 1;
                        }
                    }
                    if i < len {
                        i += 1; // skip closing quote
                    }
                } else {
                    i += 1;
                }
            }

            let open_end = i;
            let is_unclosed = depth > 0;

            // Check if byte_pos falls within this macro construct
            if byte_pos >= open_start && byte_pos <= open_end {
                return Some(MacroAtPosition {
                    name: name.to_string(),
                    full_range: open_start..open_end,
                    name_range: name_start..name_end,
                    is_unclosed,
                });
            }

            // If unclosed and cursor is past the opening, it's inside
            if is_unclosed && byte_pos > open_start {
                return Some(MacroAtPosition {
                    name: name.to_string(),
                    full_range: open_start..len,
                    name_range: name_start..name_end,
                    is_unclosed: true,
                });
            }
        } else {
            i += 1;
        }
    }

    None
}

/// Scan a line for SugarCube macro block open/close events.
///
/// Detects `<<name>>` (open) and `<</name>>` (close) patterns on a
/// single line of source text. Returns a list of `MacroBlockEvent`
/// instances for the folding-range handler to pair into folding regions.
///
/// Also detects modifier macros (`<<elseif>>`, `<<else>>`) which create
/// subdivision points within a block macro's folding range.
pub(super) fn scan_line_for_macro_events_impl(
    line: &str,
    line_idx: u32,
) -> Vec<MacroBlockEvent> {
    let mut events = Vec::new();
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // Folding modifier macros — they don't open/close blocks but subdivide them
    let folding_modifiers: &[&str] = &["else", "elseif"];

    while i + 1 < len {
        // Look for << opening
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            let _open_start = i;
            i += 2;

            // Check for close tag: <</
            let is_close_tag = i < len && bytes[i] == b'/';
            if is_close_tag {
                i += 1;
            }

            // Skip whitespace
            while i < len && bytes[i] == b' ' {
                i += 1;
            }

            // Extract the macro name
            let name_start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-') {
                i += 1;
            }
            let name_end = i;

            if name_start == name_end {
                // No name — skip
                continue;
            }

            let name = &line[name_start..name_end];

            // Skip to the end of this tag (>>)
            let mut depth = 1;
            while i + 1 < len && depth > 0 {
                if bytes[i] == b'<' && bytes[i + 1] == b'<' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'>' && bytes[i + 1] == b'>' {
                    depth -= 1;
                    if depth == 0 {
                        i += 2;
                        break;
                    }
                    i += 2;
                } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                    let quote = bytes[i];
                    i += 1;
                    while i < len && bytes[i] != quote {
                        if bytes[i] == b'\\' && i + 1 < len {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    if i < len {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }

            // Determine if this is an open, close, or modifier event
            if is_close_tag {
                // Close tag: <</name>>
                events.push(MacroBlockEvent {
                    name: name.to_string(),
                    line: line_idx,
                    is_open: false,
                });
            } else if folding_modifiers.contains(&name) {
                // Modifier: <<else>> or <<elseif>> — treat as a close+open
                // pair so the folding handler creates subdivision
                events.push(MacroBlockEvent {
                    name: name.to_string(),
                    line: line_idx,
                    is_open: true, // modifiers subdivide the current block
                });
            } else {
                // Open tag: <<name>>
                // Only report as open if this is a known block macro
                // (inline macros like <<set>>, <<print>> don't create folds)
                if is_block_macro_name(name) {
                    events.push(MacroBlockEvent {
                        name: name.to_string(),
                        line: line_idx,
                        is_open: true,
                    });
                }
            }
        } else {
            i += 1;
        }
    }

    events
}

/// Check if a macro name corresponds to a block macro (one that has a
/// close tag and creates a folding region).
///
/// Block macros are the ones that can have child content between their
/// open and close tags: <<if>>...<</if>>, <<for>>...<</for>>, etc.
/// Inline macros like <<set>>, <<print>>, <<goto>> are not block macros.
pub(super) fn is_block_macro_name(name: &str) -> bool {
    // Check against the static block macro names from the catalog
    let block_names = macros::block_macro_names();
    block_names.contains(name)
}
