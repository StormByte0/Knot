//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a close tag (`<</name>>`).
///
/// Close tags don't have span data in `macro_invocations` (which tracks
/// open tags only), so we detect via line-scanning for the `<</` pattern.
/// Shows "Close tag for `<<name>>`" so users know what they're closing.
pub(super) fn try_close_tag_hover(
    text: &str,
    byte_offset: usize,
    passage: Option<&knot_core::Passage>,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    // ── Phase 6: zone-map based detection ──────────────────────────
    //
    // Try the zone map first. If the cursor is on a `MacroTag { part: Close }`
    // leaf, fire close-tag hover immediately. This is O(log n) and more
    // accurate than the backward byte-scan.
    //
    // Falls back to the line-based backward scan if the zone map doesn't
    // find a close tag (e.g., the user is mid-typing `<</` and the parser
    // hasn't recognized it yet).
    if let Some(passage) = passage {
        let passage_offset = byte_offset.saturating_sub(passage.passage_offset);
        if let Some(leaf) = passage.zones.leaf_at(passage_offset)
            && let knot_core::zoning::LeafKind::MacroTag {
                macro_name,
                part: knot_core::zoning::TagPart::Close,
                ..
            } = &leaf.kind
        {
            // Only show hover if this is a known builtin macro.
            if plugin.find_macro(macro_name).is_some() {
                let close_label = plugin.format_close_macro_label(macro_name);
                let hover_text = format!(
                    "**`{}`** `Close tag`\n\nCloses the `{}` block.",
                    close_label,
                    plugin.format_macro_label(macro_name)
                );
                // Convert the leaf's passage-relative span to an LSP range.
                let abs_span = passage.abs_range(&leaf.span);
                let range = helpers::byte_range_to_lsp_range(text, &abs_span);
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(range),
                });
            }
        }
    }

    // ── Fallback: line-based backward scan for mid-typing cases ─────
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);
    let bytes = line.as_bytes();

    // Find the `<</` sequence that the cursor is inside.
    // Walk backward from the cursor to find `<</`.
    let mut tag_start = None;
    let mut search = byte_pos;
    while search >= 2 {
        if search <= bytes.len()
            && search >= 3
            && bytes[search - 3] == b'<'
            && bytes[search - 2] == b'<'
            && bytes[search - 1] == b'/'
        {
            tag_start = Some(search - 3);
            break;
        }
        search -= 1;
        // Don't walk past a `>>` (we'd be in a different tag).
        if search < bytes.len()
            && search >= 1
            && bytes[search - 1] == b'>'
            && search >= 2
            && bytes[search - 2] == b'>'
        {
            return None;
        }
    }
    let tag_start = tag_start?;
    // The name starts at tag_start + 3 (after `<</`).
    let name_start = tag_start + 3;
    if name_start >= bytes.len() {
        return None;
    }
    // Find end of name (alphanumeric + underscore + hyphen).
    let mut name_end = name_start;
    while name_end < bytes.len() {
        let b = bytes[name_end];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            name_end += 1;
        } else {
            break;
        }
    }
    if name_end == name_start {
        return None;
    }
    // Cursor must be within the tag (from `<</` to the closing `>>`).
    // Find the closing `>>` after the name.
    let mut tag_end = name_end;
    while tag_end + 1 < bytes.len() {
        if bytes[tag_end] == b'>' && bytes[tag_end + 1] == b'>' {
            tag_end += 2;
            break;
        }
        tag_end += 1;
    }
    if byte_pos < tag_start || byte_pos > tag_end {
        return None;
    }

    let name = &line[name_start..name_end];
    // Only show hover if this is a known builtin macro (otherwise we'd
    // show "Close tag for" on arbitrary text that looks like a close tag).
    // The `find_macro` call confirms it's a builtin; we don't need the
    // `MacroDef` itself — we just need to know the name is valid.
    let _ = plugin.find_macro(name)?;

    let close_label = plugin.format_close_macro_label(name);
    // Wrap the close label in backticks (rendered as inline code) so VS
    // Code's markdown renderer doesn't strip `<</link>>` as unknown HTML.
    // Use the plugin's `format_close_macro_label` (returns `<</link>>` for
    // SugarCube) instead of a literal `{name}/` (which rendered as `link/`
    // — wrong on two counts: it dropped the `<<>>` delimiters AND left the
    // trailing slash that's only used internally as a catalog key).
    let hover_text = format!(
        "**`{}`** `Close tag`\n\nCloses the `{}` block.",
        close_label,
        plugin.format_macro_label(name)
    );
    let utf16_start = helpers::utf16_len_up_to(line, tag_start);
    let utf16_end = helpers::utf16_len_up_to(line, tag_end);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(Range {
            start: Position {
                line: line_idx as u32,
                character: utf16_start,
            },
            end: Position {
                line: line_idx as u32,
                character: utf16_end,
            },
        }),
    })
}
