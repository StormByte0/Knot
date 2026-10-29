//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for block-level SugarCube markup markers.
///
/// SugarCube has several column-0-anchored markup constructs that produce
/// semantic tokens but had no hover handler:
///
/// | Marker | Construct | Token type |
/// |--------|-----------|------------|
/// | `!`..`!!!!!!` | Heading (levels 1-6) | `Heading` |
/// | `*`/`**`/`***` | Unordered list item | `ListMarker` |
/// | `#`/`##`/`###` | Ordered list item | `ListMarker` |
/// | `>`/`>>`/`>>>` | Line-style blockquote | `Blockquote` |
/// | `<<<` | Block-style blockquote | `BlockquoteBlock` |
/// | `----` (4+) | Horizontal rule | `HorizontalRule` |
/// | `{{{` | Code block / inline code | `CodeBlock`/`InlineCode` |
///
/// Hover fires when the cursor is on the marker run itself (the `!`/`*`/`#`/
/// `>`/`-` characters at column 0). The hover text explains what the marker
/// does and links it to the SugarCube documentation pattern.
///
/// This is a line-based scan (not AST-based) because:
/// 1. The markers are simple column-0 patterns — no need for AST traversal.
/// 2. It works even when the AST isn't yet indexed (e.g., during incremental
///    re-parse).
/// 3. It's consistent with `try_operator_hover` and `try_global_hover`,
///    which also use line-based scanning for simple patterns.
///
/// Build hover text for a markup kind from the zone map.
///
/// `marker_text` is the source text of the markup leaf (used to determine
/// the heading level, list depth, etc.). Returns `None` if the markup kind
/// doesn't have a dedicated hover (e.g., `Comment`, `Link` — those have
/// their own hover handlers).
fn build_markup_hover_text(
    kind: &knot_core::zoning::MarkupKind,
    marker_text: &str,
) -> Option<String> {
    use knot_core::zoning::MarkupKind;
    match kind {
        MarkupKind::Heading => {
            let level = marker_text.chars().take_while(|c| *c == '!').count().clamp(1, 6);
            let html_tag = match level {
                1 => "h1",
                2 => "h2",
                3 => "h3",
                4 => "h4",
                5 => "h5",
                _ => "h6",
            };
            let heading_desc = match level {
                1 => "level 1 heading — main section title (largest)",
                2 => "level 2 heading — subsection title",
                3 => "level 3 heading — sub-subsection title",
                4 => "level 4 heading — minor section title",
                5 => "level 5 heading — small heading",
                _ => "level 6 heading — smallest heading",
            };
            Some(format!(
                "**`{}` Heading** (`{}` tag)\n\n{}",
                "!".repeat(level),
                html_tag,
                heading_desc
            ))
        }
        MarkupKind::ListItem => {
            // Could be `*` (unordered) or `#` (ordered). Check the first char.
            let first = marker_text.chars().next().unwrap_or('*');
            let depth = marker_text.chars().take_while(|c| *c == first).count();
            let depth_desc = if depth == 1 {
                "top level".to_string()
            } else {
                format!("nested (depth {})", depth)
            };
            if first == '#' {
                Some(format!(
                    "**`{}` Ordered List Item**\n\nCreates a `<li>` in a `<ol>`. {}",
                    "#".repeat(depth),
                    depth_desc
                ))
            } else {
                Some(format!(
                    "**`{}` Unordered List Item**\n\nCreates a `<li>` in a `<ul>`. {}",
                    "*".repeat(depth),
                    depth_desc
                ))
            }
        }
        MarkupKind::Blockquote => {
            // Could be `>` (line) or `<<<` (block). Check the first char.
            let first = marker_text.chars().next().unwrap_or('>');
            if first == '<' {
                Some("**`<<<` Block Blockquote**\n\nOpens a block-style blockquote. Close with another `<<<` on its own line. Content between the delimiters is wrapped in `<blockquote>`.".to_string())
            } else {
                let depth = marker_text.chars().take_while(|c| *c == '>').count();
                let depth_desc = if depth == 1 {
                    "single level".to_string()
                } else {
                    format!("nested (depth {})", depth)
                };
                Some(format!(
                    "**`{}` Blockquote**\n\nCreates a `<blockquote>`. {}",
                    ">".repeat(depth),
                    depth_desc
                ))
            }
        }
        MarkupKind::HorizontalRule => {
            Some("**`----` Horizontal Rule**\n\nCreates an `<hr>` element. Requires 4+ dashes alone on a line at column 0.".to_string())
        }
        MarkupKind::CodeBlock => {
            Some("**`{{{` Code Block**\n\nOpens a raw code block. Content is NOT processed (macros/variables inside are literal). Close with `}}}` alone on its own line. Renders as `<pre><code>…</code></pre>`.".to_string())
        }
        MarkupKind::InlineCode => {
            Some("**`{{{` Inline Code**\n\nOpens inline raw code. Content is NOT processed (macros/variables inside are literal). Close with the first `}}}`. Renders as `<code>…</code>`.".to_string())
        }
        MarkupKind::Verbatim => {
            Some("**`\"\"\"` Verbatim**\n\nRaw text — no markup processing. Macros, variables, and links inside are NOT executed. Close with the first `\"\"\"`.".to_string())
        }
        MarkupKind::Table => {
            Some("**Table**\n\nTiddlyWiki-style table. Rows are `|`-delimited lines. Cells beginning with `!` are header cells.".to_string())
        }
        MarkupKind::InlineStyle => {
            Some("**`@@` Inline Style**\n\n`@@class;text@@` produces `<span class=\"class\">text</span>`.".to_string())
        }
        MarkupKind::TextFormat => {
            // Determine which format from the marker text.
            if marker_text.starts_with("''") {
                Some("**`''bold''` Bold**\n\nProduces `<strong>`.".to_string())
            } else if marker_text.starts_with("//") {
                Some("**`//italic//` Italic**\n\nProduces `<em>`.".to_string())
            } else if marker_text.starts_with("__") {
                Some("**`__underline__` Underline**\n\nProduces `<u>`.".to_string())
            } else if marker_text.starts_with("==") {
                Some("**`==strike==` Strikethrough**\n\nProduces `<del>`.".to_string())
            } else if marker_text.starts_with("~~") {
                Some("**`~~sub~~` Subscript**\n\nProduces `<sub>`.".to_string())
            } else if marker_text.starts_with("^^") {
                Some("**`^^super^^` Superscript**\n\nProduces `<sup>`.".to_string())
            } else {
                None
            }
        }
        // Comment and Link have their own dedicated hover handlers.
        MarkupKind::Comment | MarkupKind::Link => None,
    }
}

pub(super) fn try_block_markup_hover(
    text: &str,
    byte_offset: usize,
    passage: Option<&knot_core::Passage>,
) -> Option<Hover> {
    // ── Phase 6: zone-map based detection ──────────────────────────
    //
    // Try the zone map first. If the cursor is on a `Markup(...)` leaf,
    // fire the appropriate block-markup hover. This replaces the column-0
    // marker detection with an O(log n) zone lookup.
    //
    // Falls back to the line-based column-0 detection if the zone map
    // doesn't find a markup leaf (e.g., the user is mid-typing a marker
    // and the parser hasn't recognized it yet, or for inline `{{{` which
    // may not be at column 0).
    if let Some(passage) = passage {
        let passage_offset = byte_offset.saturating_sub(passage.passage_offset);
        if let Some(leaf) = passage.zones.leaf_at(passage_offset)
            && let knot_core::zoning::LeafKind::Markup(kind) = &leaf.kind
        {
            // Build hover text from the markup kind. We need to extract
            // the marker text from the source to get the level/depth.
            let abs_span = passage.abs_range(&leaf.span);
            let marker_text = &text[abs_span.start.min(text.len())..abs_span.end.min(text.len())];
            if let Some(hover_text) = build_markup_hover_text(kind, marker_text) {
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

    // ── Fallback: line-based column-0 detection ─────────────────────
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);

    let bytes = line.as_bytes();
    if bytes.is_empty() || byte_pos > bytes.len() {
        return None;
    }

    // ── Mid-line inline code `{{{...}}}` ───────────────────────────
    // Inline code can appear anywhere in a line, not just at column 0.
    // Scan for `{{{` at or near the cursor position. This handles cases
    // like `Some {{{inline code}} here.` where `{{{` is at byte_pos 5.
    // The cursor could be on any of the 3 `{` bytes, so we check starts
    // from byte_pos-2 through byte_pos (clamped to valid range).
    //
    // Skip this when the line starts with `{{{` at column 0 AND the cursor
    // is within the first 3 bytes — that case is handled by the column-0
    // check below (which distinguishes Code Block vs Inline Code).
    let is_col0_triple_brace =
        bytes.len() >= 3 && bytes[0] == b'{' && bytes[1] == b'{' && bytes[2] == b'{';
    if !(is_col0_triple_brace && byte_pos < 3) {
        let scan_start = byte_pos.saturating_sub(2);
        for start in scan_start..=byte_pos {
            if start + 3 <= bytes.len()
                && bytes[start] == b'{'
                && bytes[start + 1] == b'{'
                && bytes[start + 2] == b'{'
                && byte_pos < start + 3
            {
                let utf16_start = helpers::utf16_len_up_to(line, start);
                let utf16_end = helpers::utf16_len_up_to(line, start + 3);
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: "**`{{{` Inline Code**\n\nOpens inline raw code. Content is NOT processed (macros/variables inside are literal). Close with the first `}}}`. Renders as `<code>…</code>`.".to_string(),
                    }),
                    range: Some(Range {
                        start: Position { line: line_idx as u32, character: utf16_start },
                        end: Position { line: line_idx as u32, character: utf16_end },
                    }),
                });
            }
        }
    }

    // All block-level markers are column-0 anchored. If the cursor is not
    // at column 0, none of these can fire.
    // (byte_pos is the byte offset within the line; column 0 means byte_pos == 0
    // OR the cursor is on a leading-whitespace prefix — but SugarCube requires
    // NO leading whitespace for block markup, so we check byte_pos == 0.)
    //
    // However, the cursor might be ON the marker run (e.g., on the 2nd `!` of
    // `!!`), so we check if byte_pos is within the marker run starting at col 0.
    let first = bytes[0];

    // Determine the marker run and its hover content.
    let (marker_end, hover_text): (usize, String) = if first == b'!' {
        // Heading: `!` through `!!!!!!` (1-6 levels)
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'!' && end < 6 {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        // Cursor must be within the `!` run.
        if byte_pos >= end {
            return None;
        }
        let level = end;
        let html_tag = match level {
            1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        };
        let heading_desc = match level {
            1 => "level 1 heading — main section title (largest)",
            2 => "level 2 heading — subsection title",
            3 => "level 3 heading — sub-subsection title",
            4 => "level 4 heading — minor section title",
            5 => "level 5 heading — small heading",
            _ => "level 6 heading — smallest heading",
        };
        (
            end,
            format!(
                "**`{}` Heading** (`{}` tag)\n\n{}",
                "!".repeat(level),
                html_tag,
                heading_desc
            ),
        )
    } else if first == b'*' {
        // Unordered list item: `*`, `**`, `***`, etc.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'*' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos >= end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 {
            "top level".to_string()
        } else {
            format!("nested (depth {})", depth)
        };
        (
            end,
            format!(
                "**`{}` Unordered List Item**\n\nCreates a `<li>` in a `<ul>`. {}",
                "*".repeat(depth),
                depth_desc
            ),
        )
    } else if first == b'#' {
        // Ordered list item: `#`, `##`, `###`, etc.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'#' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos >= end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 {
            "top level".to_string()
        } else {
            format!("nested (depth {})", depth)
        };
        (
            end,
            format!(
                "**`{}` Ordered List Item**\n\nCreates a `<li>` in an `<ol>`. {}",
                "#".repeat(depth),
                depth_desc
            ),
        )
    } else if first == b'>' {
        // Line-style blockquote: `>`, `>>`, `>>>`, etc.
        // Note: `<<<` (block-style blockquote) also starts with `<`, but we
        // check `>` here. `<<<` is handled separately below.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'>' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos >= end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 {
            "single level".to_string()
        } else {
            format!("nested (depth {})", depth)
        };
        (
            end,
            format!(
                "**`{}` Blockquote**\n\nCreates a `<blockquote>`. {}",
                ">".repeat(depth),
                depth_desc
            ),
        )
    } else if first == b'-' {
        // Horizontal rule: `----` (4+ dashes) alone on a line.
        // Also handles `<<<` block-style blockquote — wait, `<<<` starts with
        // `<` not `-`. This branch only handles horizontal rules.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'-' {
            end += 1;
        }
        if end < 4 {
            return None; // Need at least 4 dashes for a horizontal rule.
        }
        // Check that the rest of the line is only whitespace (HR must be alone).
        let rest = &line[end..];
        if !rest.trim().is_empty() {
            return None;
        }
        if byte_pos >= end {
            return None;
        }
        (end, "**`----` Horizontal Rule**\n\nCreates an `<hr>` element. Requires 4+ dashes alone on a line at column 0.".to_string())
    } else if first == b'<' && bytes.len() >= 3 && bytes[1] == b'<' && bytes[2] == b'<' {
        // Block-style blockquote: `<<<` alone on a line.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'<' {
            end += 1;
        }
        if end != 3 {
            return None; // Must be exactly 3 `<` characters.
        }
        // Check that the rest is whitespace or newline.
        let rest = &line[end..];
        if !rest.trim().is_empty() {
            return None;
        }
        if byte_pos >= end {
            return None;
        }
        (end, "**`<<<` Block Blockquote**\n\nOpens a block-style blockquote. Close with another `<<<` on its own line. Content between the delimiters is wrapped in `<blockquote>`.".to_string())
    } else if first == b'{' && bytes.len() >= 3 && bytes[1] == b'{' && bytes[2] == b'{' {
        // Code block: `{{{\n...\n}}}` (block form at column 0).
        // Inline code `{{{...}}}` (mid-line) is NOT column-0 anchored, but we
        // still handle it here for hover purposes — the cursor on `{{{` should
        // show code block info regardless of position.
        if byte_pos > 3 {
            return None;
        }
        // Check if it's block form. `text.lines()` strips the trailing `\n`,
        // so a block-form line is exactly `{{{` (3 bytes, nothing else).
        // Inline form has content after `{{{` on the same line (len > 3).
        let is_block = bytes.len() == 3;
        if is_block {
            (3, "**`{{{` Code Block**\n\nOpens a raw code block. Content is NOT processed (macros/variables inside are literal). Close with `}}}` alone on its own line. Renders as `<pre><code>…</code></pre>`.".to_string())
        } else {
            (3, "**`{{{` Inline Code**\n\nOpens inline raw code. Content is NOT processed (macros/variables inside are literal). Close with the first `}}}`. Renders as `<code>…</code>`.".to_string())
        }
    } else {
        return None;
    };

    // Compute the UTF-16 range for the marker run.
    let utf16_start = helpers::utf16_len_up_to(line, 0);
    let utf16_end = helpers::utf16_len_up_to(line, marker_end);
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
