//! Variable scanning for `$var` and `_var` references.

use crate::sugarcube::ast::*;
use super::predicates::{is_ident_char, is_ident_start};

/// Scan a variable reference starting at position `start`.
///
/// Returns (var_ref, end_position).
/// `start` points to the `$` or `_` sigil.
pub(super) fn scan_variable(text: &str, start: usize, is_temporary: bool) -> (VarRef, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let _sigil = bytes[start];

    // Scan identifier
    let mut i = start + 1;
    while i < len && is_ident_char(bytes[i]) {
        i += 1;
    }

    // Scan dot-notation property path
    let path_start = i;
    let mut property_path = String::new();
    while i < len && bytes[i] == b'.' {
        i += 1; // Skip the dot
        let prop_start = i;
        while i < len && is_ident_char(bytes[i]) {
            i += 1;
        }
        if i > prop_start {
            if !property_path.is_empty() {
                property_path.push('.');
            }
            property_path.push_str(&text[prop_start..i]);
        }
    }

    let name = text[start..path_start].to_string();

    (
        VarRef {
            name,
            property_path,
            is_temporary,
            is_write: false, // Write status determined by context
            span: start..i,
        },
        i,
    )
}

/// Scan inline variable references from a text string.
///
/// This finds all `$var` and `_var` references in text that is
/// NOT inside a macro or link (those are handled by their own parsers).
pub(super) fn scan_inline_vars(text: &str, offset: usize) -> Vec<VarRef> {
    let mut refs = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let (var_ref, end) = scan_variable(text, i, false);
            let mut vr = var_ref;
            vr.span = offset + vr.span.start..offset + vr.span.end;
            refs.push(vr);
            i = end;
        } else if bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            // _var: only match at word boundary (not inside a word)
            let is_word_boundary = i == 0
                || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if is_word_boundary {
                let (var_ref, end) = scan_variable(text, i, true);
                // Verify it's a valid temporary variable (not just an underscore in text)
                // SugarCube temp vars are _ followed by an identifier
                let mut vr = var_ref;
                vr.span = offset + vr.span.start..offset + vr.span.end;
                refs.push(vr);
                i = end;
            } else {
                i += 1;
            }
        } else if bytes[i] == b'$' && i + 1 < len && bytes[i + 1] == b'$' {
            // $$ — escaped dollar, skip
            i += 2;
        } else {
            i += 1;
        }
    }

    refs
}
