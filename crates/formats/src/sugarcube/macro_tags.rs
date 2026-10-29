//! Moved verbatim from the former `sugarcube/mod.rs` monolith
//! (pure move, plan §4.6 — no signature or logic changes).

use super::*;

// ===========================================================================
// Macro open/close pair scanning (Phase 2 infrastructure)
// ===========================================================================

/// A detected macro open/close tag pair in source text.
///
/// Used by `find_enclosing_block_macro()` and `find_unclosed_block_macros()`
/// to determine which container macros surround a cursor position and which
/// container macros are unclosed.
#[derive(Debug, Clone)]
pub(super) struct MacroTag {
    /// The macro name (e.g., "if", "for", "link").
    pub(super) name: String,
    /// Whether this is an open tag (`<<name>>`) or close tag (`<</name>>`).
    pub(super) is_close: bool,
}

/// Scan text for all `<<name>>` and `<</name>>` tags up to a byte limit.
///
/// Returns tags sorted by byte position. Skips content inside string literals,
/// block comments, and line comments within macro args (between `<<` and `>>`).
/// This is a heuristic scan — it doesn't build a full AST, but handles the
/// common cases well enough for completion filtering.
pub(super) fn scan_macro_tags(text: &str, up_to: usize) -> Vec<MacroTag> {
    let mut tags = Vec::new();
    let bytes = text.as_bytes();
    let limit = up_to.min(text.len());
    let mut i = 0;

    while i < limit {
        // Look for `<<` delimiter
        if bytes[i] == b'<' && i + 1 < limit && bytes[i + 1] == b'<' {
            // Check for close tag: `<</`
            let is_close = i + 2 < limit && bytes[i + 2] == b'/';
            let name_start = if is_close { i + 3 } else { i + 2 };

            // Extract the macro name (alphanumeric + underscore + hyphen)
            let mut name_end = name_start;
            while name_end < limit {
                let ch = bytes[name_end];
                if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                    name_end += 1;
                } else {
                    break;
                }
            }

            let name = &text[name_start..name_end];

            // Skip empty names or names that look like operators (`<=`, `<<`, `<-`)
            if name.is_empty() {
                i += 1;
                continue;
            }

            // For open tags, skip to the closing `>>` while handling strings/comments
            // For close tags, find the `>>` directly
            let mut scan_pos = name_end;
            let mut found_close = false;

            if !is_close {
                // Skip args content inside the open tag: strings, comments, etc.
                let mut in_dq = false;
                let mut in_sq = false;
                while scan_pos + 1 < limit {
                    let ch = bytes[scan_pos];
                    let next = bytes[scan_pos + 1];

                    if in_dq {
                        if ch == b'\\' {
                            scan_pos += 2; // Skip escaped char
                            continue;
                        }
                        if ch == b'"' {
                            in_dq = false;
                        }
                    } else if in_sq {
                        if ch == b'\\' {
                            scan_pos += 2;
                            continue;
                        }
                        if ch == b'\'' {
                            in_sq = false;
                        }
                    } else {
                        // Check for block comment `/* ... */`
                        if ch == b'/' && next == b'*' {
                            scan_pos += 2;
                            while scan_pos + 1 < limit {
                                if bytes[scan_pos] == b'*' && bytes[scan_pos + 1] == b'/' {
                                    scan_pos += 2;
                                    break;
                                }
                                scan_pos += 1;
                            }
                            continue;
                        }
                        // Check for line comment `//`
                        if ch == b'/' && next == b'/' {
                            scan_pos += 2;
                            while scan_pos < limit {
                                if bytes[scan_pos] == b'\n' {
                                    scan_pos += 1;
                                    break;
                                }
                                scan_pos += 1;
                            }
                            continue;
                        }
                        if ch == b'"' {
                            in_dq = true;
                        } else if ch == b'\'' {
                            in_sq = true;
                        } else if ch == b'>' && next == b'>' {
                            found_close = true;
                            scan_pos += 2;
                            break;
                        }
                    }
                    scan_pos += 1;
                }
            } else {
                // Close tag: find `>>`
                while scan_pos + 1 < limit {
                    if bytes[scan_pos] == b'>' && bytes[scan_pos + 1] == b'>' {
                        found_close = true;
                        scan_pos += 2;
                        break;
                    }
                    scan_pos += 1;
                }
            }

            if found_close {
                tags.push(MacroTag {
                    name: name.to_string(),
                    is_close,
                });
                i = scan_pos;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    tags
}

/// Build a stack of currently-open container macro names at a given byte offset.
///
/// Scans the text up to `byte_offset`, tracking `<<name>>` opens and `<</name>>`
/// closes. Returns the stack of unclosed macro names from outermost to innermost.
///
/// Only considers macros that are in the `body_macro_names()` set (Container macros).
/// Structural modifiers like `<<else>>`, `<<elseif>>`, `<<case>>` are not pushed
/// because they don't open a new scope.
pub(super) fn build_open_macro_stack_at_offset(
    text: &str,
    byte_offset: usize,
    body_macros: &HashSet<&'static str>,
) -> Vec<String> {
    let tags = scan_macro_tags(text, byte_offset);
    let mut stack: Vec<String> = Vec::new();

    for tag in &tags {
        if tag.is_close {
            // Pop the matching open tag
            if let Some(pos) = stack.iter().rposition(|n| n == &tag.name) {
                stack.truncate(pos);
            }
        } else {
            // Only push if this is a container macro
            if body_macros.contains(tag.name.as_str()) {
                stack.push(tag.name.clone());
            }
        }
    }

    stack
}

/// Find the names of the enclosing container macro(s) at a cursor position.
///
/// Returns a list from outermost to innermost. For example, when the cursor
/// is inside `<<if>><<for>>...<</for>><</if>>`, returns `["if", "for"]`.
///
/// If the cursor is at the top level (not inside any container macro), returns
/// an empty vector.
///
/// # Deprecated
///
/// This function performs an O(document-length) text scan from byte 0 to the
/// cursor on every call. Phase 4 migrated `build_macro_completions` to use
/// `passage.zones.body_stack_at()` instead (O(depth)). This function is
/// retained only as a fallback for when the zone map is unavailable, and for
/// the existing unit tests. It will be removed in Phase 7 once all callers
/// are migrated.
#[deprecated(note = "Use `passage.zones.body_stack_at()` instead (Phase 4 migration).")]
pub(super) fn find_enclosing_block_macros(
    text: &str,
    byte_offset: usize,
    body_macros: &HashSet<&'static str>,
) -> Vec<String> {
    build_open_macro_stack_at_offset(text, byte_offset, body_macros)
}
