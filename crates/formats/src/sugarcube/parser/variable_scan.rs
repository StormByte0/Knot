//! Variable scanning for `$var` and `_var` references.

use super::predicates::{is_ident_char, is_ident_start, is_var_ident_char};
use crate::sugarcube::ast::*;

/// Scan a variable reference starting at position `start`.
///
/// Returns (var_ref, end_position).
/// `start` points to the `$` or `_` sigil.
pub(super) fn scan_variable(text: &str, start: usize, is_temporary: bool) -> (VarRef, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let _sigil = bytes[start];

    // Scan identifier.
    //
    // P1-6 (study / plan.md Phase 1.6): this previously used `is_ident_char`
    // (which includes `-` for MACRO names like `<<link-replace>>`), so
    // `$my-var` scanned the whole `$my-var` as one variable name — wrong on
    // two counts: SugarCube variable names never contain hyphens, and the
    // over-long name produced wrong entries in the registries/completions.
    // `is_var_ident_char` matches the documented variable rule
    // (predicates.rs: `$my` + `-var` as prose).
    let mut i = start + 1;
    while i < len && is_var_ident_char(bytes[i]) {
        i += 1;
    }

    // Scan dot-notation property path
    let path_start = i;
    let mut property_path = String::new();
    while i < len && bytes[i] == b'.' {
        let dot_pos = i;
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
        } else {
            // The dot was not followed by an identifier (e.g., `$var.` at
            // end of text, or `$var. ` with a space). Rewind `i` back to
            // the dot position so the span doesn't include the trailing dot.
            // The dot is NOT part of the variable reference — it's either
            // a sentence-ending period or the start of a new construct.
            i = dot_pos;
            break;
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
///
/// Skips content inside `/* ... */` block comments and `// ...` line
/// comments, since variable references in comments are not real references.
///
/// `offset` is added to each returned `VarRef.span` so callers can produce
/// spans in any coordinate space (body-relative, passage-relative, etc.).
pub(in crate::sugarcube) fn scan_inline_vars(text: &str, offset: usize) -> Vec<VarRef> {
    let mut refs = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        // ── Block comments: /* ... */ ── skip entirely
        if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                // Advance by full UTF-8 character to avoid mid-char position.
                i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        // ── Line comments: // ... ── skip to end of line
        if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                // Advance by full UTF-8 character to avoid mid-char position.
                i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
            continue;
        }

        if bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let (var_ref, end) = scan_variable(text, i, false);
            let mut vr = var_ref;
            vr.span = offset + vr.span.start..offset + vr.span.end;
            refs.push(vr);
            i = end;
        } else if bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            // _var: only match at word boundary (not inside a word)
            let is_word_boundary =
                i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if is_word_boundary {
                let (var_ref, end) = scan_variable(text, i, true);
                // Verify it's a valid temporary variable (not just an underscore in text)
                // SugarCube temp vars are _ followed by an identifier
                let mut vr = var_ref;
                vr.span = offset + vr.span.start..offset + vr.span.end;
                refs.push(vr);
                i = end;
            } else {
                // Advance by full UTF-8 character to avoid mid-char position.
                i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
        } else if bytes[i] == b'$' && i + 1 < len && bytes[i + 1] == b'$' {
            // $$ — escaped dollar, skip
            i += 2;
        } else {
            // Advance by full UTF-8 character to avoid mid-char position.
            i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }

    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-6 (study / plan.md Phase 1.6): variable names never contain
    /// hyphens. `$my-var` must scan as `$my` (span ends at the `-`), not
    /// `$my-var`; the old `is_ident_char` scan (which includes `-` for
    /// macro names like `<<link-replace>>`) swallowed the hyphen and
    /// produced a wrong variable name in registries/completions.
    #[test]
    fn hyphenated_prose_does_not_extend_variable_name() {
        let text = "$my-var";
        let (var_ref, end) = scan_variable(text, 0, false);
        assert_eq!(var_ref.name, "$my");
        assert_eq!(end, 3, "scan must stop at the hyphen");
        assert_eq!(var_ref.span, 0..3);
    }
}
