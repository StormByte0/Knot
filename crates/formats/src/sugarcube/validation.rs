//! Validation/diagnostics for SugarCube.
//!
//! Provides comprehensive validation of SugarCube passage bodies, including:
//! - Unclosed macro brackets
//! - Unclosed link brackets
//! - Block-aware structural validation (macro parent constraints)
//! - Unknown macro detection
//! - Deprecated macro warnings
//!
//! The validation uses a single-pass approach that processes all macro events
//! (open + close) in source order, maintaining a stack. When a close tag is
//! encountered, its matching open tag is found by searching the stack backward.
//! This gives proper nesting context for structural validation — e.g., `<<else>>`
//! inside `<<if>>` is correctly recognized as valid because `<<if>>` is on the
//! stack (its `<</if>>` hasn't been encountered yet).

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};
use super::blocks::{RE_MACRO, RE_MACRO_CLOSE};
use super::macros;

/// Comprehensive validation: check for common SugarCube errors.
///
/// This includes:
/// - Unclosed macro brackets
/// - Unclosed link brackets
/// - Structural validation (macro parent constraints, block-aware)
/// - Unknown macro detection
/// - Deprecated macro warnings
pub(crate) fn validate(body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
    let mut diagnostics = Vec::new();

    // ── Unclosed macro brackets ──────────────────────────────────────
    validate_macro_brackets(body, body_offset, &mut diagnostics);

    // ── Unclosed link brackets ──────────────────────────────────────
    validate_link_brackets(body, body_offset, &mut diagnostics);

    // ── Structural validation: block-aware macro validation ──────────
    validate_macro_structure(body, body_offset, &mut diagnostics);

    diagnostics
}

/// Check for unclosed `<<` / `>>` macro bracket pairs.
fn validate_macro_brackets(body: &str, body_offset: usize, diagnostics: &mut Vec<FormatDiagnostic>) {
    let mut depth = 0i32;
    let mut open_pos: Option<usize> = None;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'<' && bytes[i + 1] == b'<' {
            if depth == 0 {
                open_pos = Some(i);
            }
            depth += 1;
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'>' && bytes[i + 1] == b'>' {
            depth -= 1;
            if depth < 0 {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + i..body_offset + i + 2,
                    message: "Unexpected macro closing `>>` without matching `<<`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-unclosed-macro".into(),
                });
                depth = 0;
            }
            i += 2;
            continue;
        }
        i += 1;
    }

    if depth > 0
        && let Some(pos) = open_pos {
            diagnostics.push(FormatDiagnostic {
                range: body_offset + pos..body_offset + pos + 2,
                message: "Unclosed macro `<<` — missing `>>`".into(),
                severity: FormatDiagnosticSeverity::Warning,
                code: "sc-unclosed-macro".into(),
            });
        }
}

/// Check for unclosed `[[` / `]]` link bracket pairs.
fn validate_link_brackets(body: &str, body_offset: usize, diagnostics: &mut Vec<FormatDiagnostic>) {
    let mut link_depth = 0i32;
    let mut link_open: Option<usize> = None;
    let bytes = body.as_bytes();
    let mut j = 0;
    while j < bytes.len() {
        if j + 1 < bytes.len() && bytes[j] == b'[' && bytes[j + 1] == b'[' {
            if link_depth == 0 {
                link_open = Some(j);
            }
            link_depth += 1;
            j += 2;
            continue;
        }
        if j + 1 < bytes.len() && bytes[j] == b']' && bytes[j + 1] == b']' {
            link_depth -= 1;
            if link_depth < 0 {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + j..body_offset + j + 2,
                    message: "Unexpected link closing `]]` without matching `[[`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-broken-link".into(),
                });
                link_depth = 0;
            }
            j += 2;
            continue;
        }
        j += 1;
    }

    if link_depth > 0
        && let Some(pos) = link_open {
            diagnostics.push(FormatDiagnostic {
                range: body_offset + pos..body_offset + pos + 2,
                message: "Unclosed link `[[` — missing `]]`".into(),
                severity: FormatDiagnosticSeverity::Warning,
                code: "sc-broken-link".into(),
            });
        }
}

/// A macro event during single-pass validation.
#[derive(Debug)]
struct MacroEvent {
    /// Byte offset of the event within the body.
    offset: usize,
    /// Macro name (without `/` prefix for close tags).
    name: String,
    /// Whether this is an open or close event.
    is_open: bool,
    /// The full span of the macro tag.
    span_start: usize,
    span_end: usize,
}

/// Block-aware structural validation.
///
/// Processes ALL macro events (open + close) in source order, maintaining
/// a stack. When a close tag is encountered, searches the stack backward
/// for the matching open tag.
///
/// This approach correctly handles structural constraints like `<<else>>`
/// inside `<<if>>` — the `<<if>>` is on the stack because `<</if>>` hasn't
/// been seen yet, so the constraint check passes.
fn validate_macro_structure(body: &str, body_offset: usize, diagnostics: &mut Vec<FormatDiagnostic>) {
    let constraints = macros::structural_constraints();
    let deprecated = macros::deprecated_macros();
    let known_macros = macros::known_macro_names();

    // ── Collect all macro events in source order ──────────────────────
    let mut events: Vec<MacroEvent> = Vec::new();

    // Open macros: <<name ...>> or <<name>>
    for caps in RE_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let name = caps.get(1).unwrap().as_str().to_string();
        events.push(MacroEvent {
            offset: m.start(),
            name,
            is_open: true,
            span_start: body_offset + m.start(),
            span_end: body_offset + m.end(),
        });
    }

    // Close macros: <</name>>
    for caps in RE_MACRO_CLOSE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let name = caps.get(1).unwrap().as_str().to_string();
        events.push(MacroEvent {
            offset: m.start(),
            name,
            is_open: false,
            span_start: body_offset + m.start(),
            span_end: body_offset + m.end(),
        });
    }

    // Sort by source position
    events.sort_by_key(|e| e.offset);

    // ── Process events in source order ────────────────────────────────
    // Stack entries: (name, span_start)
    let mut open_stack: Vec<(String, usize)> = Vec::new();

    for event in &events {
        if event.is_open {
            // Check for deprecated macros
            if let Some(msg) = deprecated.get(event.name.as_str()) {
                diagnostics.push(FormatDiagnostic {
                    range: event.span_start..event.span_end,
                    message: format!("Deprecated macro: {}", msg),
                    severity: FormatDiagnosticSeverity::Info,
                    code: "sc-deprecated-macro".into(),
                });
            }

            // Check for unknown macros
            if !known_macros.contains(event.name.as_str()) {
                diagnostics.push(FormatDiagnostic {
                    range: event.span_start..event.span_end,
                    message: format!("Unknown SugarCube macro `<<{}>>`", event.name),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "sc-unknown-macro".into(),
                });
            }

            // Validate structural constraints
            // Check if the open stack contains a valid parent
            if let Some(valid_parents) = constraints.get(event.name.as_str()) {
                let has_valid_parent = open_stack.iter().rev().any(|(parent, _)| {
                    valid_parents.contains(parent.as_str())
                });
                if !has_valid_parent {
                    let parent_list: Vec<String> = valid_parents
                        .iter()
                        .map(|p| format!("`<<{}>>`", p))
                        .collect();
                    diagnostics.push(FormatDiagnostic {
                        range: event.span_start..event.span_end,
                        message: format!(
                            "`<<{}>>` must be inside {}",
                            event.name,
                            parent_list.join(" or ")
                        ),
                        severity: FormatDiagnosticSeverity::Error,
                        code: "sc-container-structure".into(),
                    });
                }
            }

            // Push block macros onto the stack
            let is_block = macros::is_block_macro(&event.name);
            if is_block {
                open_stack.push((event.name.clone(), event.span_start));
            }
        } else {
            // Close macro: find and pop the matching open tag from the stack
            // Search backward for the matching name
            if let Some(idx) = open_stack.iter().rposition(|(name, _)| *name == event.name) {
                open_stack.remove(idx);
            }
            // If no matching open tag found, we don't report an error here
            // because the unclosed-macro bracket check above handles
            // structural issues with mismatched tags.
        }
    }

    // ── Report unclosed block macros ──────────────────────────────────
    for (name, span_start) in &open_stack {
        diagnostics.push(FormatDiagnostic {
            range: *span_start..span_start + 4 + name.len(),
            message: format!("Unclosed block macro `<<{}>>` — missing `<</{}>>`", name, name),
            severity: FormatDiagnosticSeverity::Warning,
            code: "sc-unclosed-block".into(),
        });
    }
}
