//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a JS function call (e.g., `myFunc()` inside
/// Try to show hover info for a template invocation (`?name` in SugarCube).
///
/// Templates are invoked with `?name` syntax in prose text. The token builder
/// emits `Function` semantic tokens for `?name` occurrences (scanning the text
/// content of `AstNode::Text` nodes). This hover handler checks if the cursor
/// is on such a token, then queries `plugin.find_template(name)` for the
/// definition.
///
/// Returns `None` if the cursor isn't on a template token, the plugin doesn't
/// know the template, or the template has no useful info to display.
pub(super) fn try_template_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // ── Path 1: Token-based detection ──────────────────────────────
    //
    // The token builder emits `Function` tokens for `?name` patterns in
    // prose. The token spans the NAME only (not the `?`), so we accept
    // cursor-on-name OR cursor-on-`?` (the byte immediately before the
    // token start).
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            // Accept cursor on the name OR on the `?` immediately before.
            let on_name = byte_offset >= abs_start && byte_offset < abs_end;
            let on_q =
                byte_offset + 1 == abs_start && text.as_bytes().get(byte_offset) == Some(&b'?');
            if on_name || on_q {
                let name = &text[abs_start..abs_end];
                // Check if this is a known template. `Function` tokens are
                // also emitted for regular JS functions and widgets, so this
                // check filters to templates only.
                let info = plugin.find_template(name)?;
                let _ = info; // info.defined_in no longer shown — user wants minimal hover
                let hover_text = format!("**`?{}`** `Template`", name);
                // Hover range covers `?name` (the `?` plus the name).
                let range_start = abs_start.saturating_sub(1);
                let hover_range = helpers::byte_range_to_lsp_range(text, &(range_start..abs_end));
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(hover_range),
                });
            }
        }
    }

    // ── Path 2: Text-scan fallback ────────────────────────────────
    //
    // If the token-based path didn't fire (e.g., the document hasn't been
    // re-tokenized yet, or the token builder's `?name` scanning missed an
    // edge case), fall back to scanning the text around the cursor for a
    // `?name` pattern. This makes hover robust against stale tokens.
    if let Some((name_start, name_end)) = scan_template_at_cursor(text, byte_offset) {
        let name = &text[name_start..name_end];
        if plugin.find_template(name).is_some() {
            let hover_text = format!("**`?{}`** `Template`", name);
            let range_start = name_start.saturating_sub(1); // include `?`
            let hover_range = helpers::byte_range_to_lsp_range(text, &(range_start..name_end));
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover_text,
                }),
                range: Some(hover_range),
            });
        }
    }

    None
}

/// Scan the text around `byte_offset` for a `?name` template invocation.
///
/// Returns `(name_start, name_end)` — the byte range of the NAME part
/// (excluding the `?`) — if the cursor is on the `?` or on the name of a
/// `?name` pattern in text, AND the name is a valid template identifier
/// (alpha/underscore start, alphanumeric/underscore/hyphen continuation).
///
/// Returns `None` if the cursor isn't on a `?name` pattern, or if the
/// pattern is preceded by a word character (which would indicate JS
/// optional chaining like `obj?.prop` rather than a template invocation).
pub(super) fn scan_template_at_cursor(text: &str, byte_offset: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if byte_offset > len {
        return None;
    }

    // Helper: is byte a template-name char?
    let is_name_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let is_name_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';

    // Helper: is byte a word char (for the `(?<!\w)` negative lookbehind)?
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // Case 1: cursor is ON the `?`.
    if bytes.get(byte_offset) == Some(&b'?') {
        // The `?` must NOT be preceded by a word char (mimic grammar's
        // `(?<!\w)` — prevents matching `obj?.prop` optional chaining).
        let preceded_by_word = byte_offset > 0
            && bytes
                .get(byte_offset - 1)
                .copied()
                .is_some_and(is_word_char);
        if preceded_by_word {
            return None;
        }
        // Scan forward for the name.
        let name_start = byte_offset + 1;
        if name_start >= len || !is_name_start(bytes[name_start]) {
            return None;
        }
        let mut name_end = name_start + 1;
        while name_end < len && is_name_char(bytes[name_end]) {
            name_end += 1;
        }
        return Some((name_start, name_end));
    }

    // Case 2: cursor is ON the name (or just past it). Scan backward for `?`.
    if byte_offset < len
        && (is_name_char(bytes[byte_offset])
            || (byte_offset > 0 && is_name_char(bytes[byte_offset - 1])))
    {
        // Walk backward to find the `?`.
        let mut probe = byte_offset;
        while probe > 0 && is_name_char(bytes[probe - 1]) {
            probe -= 1;
        }
        // `probe` now points at the first name char. The `?` should be just before.
        if probe == 0 || bytes.get(probe - 1) != Some(&b'?') {
            return None;
        }
        let q_pos = probe - 1;
        // `?` must NOT be preceded by a word char.
        let preceded_by_word = q_pos > 0 && bytes.get(q_pos - 1).copied().is_some_and(is_word_char);
        if preceded_by_word {
            return None;
        }
        // `probe` must be a name-start char.
        if !is_name_start(bytes[probe]) {
            return None;
        }
        // Scan forward for the end of the name.
        let mut name_end = probe + 1;
        while name_end < len && is_name_char(bytes[name_end]) {
            name_end += 1;
        }
        return Some((probe, name_end));
    }

    None
}
