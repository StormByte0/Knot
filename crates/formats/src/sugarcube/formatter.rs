//! SugarCube passage formatter (Phase 10 — the primary payoff of the zoning engine).
//!
//! Produces properly-indented, normalized SugarCube code from a passage's
//! [`ZoneMap`]. The formatter is the consumer that justifies the entire
//! zoning effort: it converts the byte-covering leaf classification into
//! indentation, line breaks, and whitespace normalization.
//!
//! ## Algorithm
//!
//! The formatter iterates `zones.iter_leaves()` in source order. For each
//! leaf, it emits the leaf's text (sliced from the original body) with
//! appropriate indentation:
//!
//! - **Prose / Markup**: emit text, indenting each line to the current depth.
//!   Internal newlines are preserved; blank lines are kept (not collapsed).
//! - **MacroTag (Open)**: if the macro has a body (block macro), put the open
//!   tag on its own line and increment the indent for subsequent body content.
//!   Inline macros (`<<set>>`, `<<print>>`) stay inline with surrounding prose.
//! - **MacroTag (Close)**: decrement indent, emit `<</name>>` on its own line.
//! - **MacroTag (Expression)**: `<<=>>expr>>` / `<<->>expr>>` — keep inline,
//!   normalize internal whitespace.
//! - **Raw**: emit as-is (JS/CSS sub-formatters are a follow-up).
//! - **Error**: refuse to format — return `None`. The user fixes the error
//!   first; formatting broken input would produce misleading output.
//!
//! ## Idempotency
//!
//! The formatter is idempotent: `format_passage(format_passage(text)) ==
//! format_passage(text)`. This is verified by a property test.
//!
//! ## Coordinate system
//!
//! All zone spans are passage-relative (0 = passage head `::`). The formatter
//! receives the body text (the content after the `:: Name` header line) and
//! the zone map. The caller is responsible for slicing the body text out of
//! the full document and for re-assembling the formatted body with the header.
//!
//! ## Out of scope
//!
//! - JS sub-formatter (oxc-formatter / dprint) — `Raw { Js }` is emitted as-is.
//! - CSS sub-formatter — `Raw { Css }` is emitted as-is.
//! - Configuration (indent style, line width) — uses tabs and sensible defaults.

use std::ops::Range;

use knot_core::zoning::{LeafKind, LeafZone, ZoneMap};

/// The indentation unit: one tab per depth level.
const INDENT_UNIT: &str = "\t";

/// Format a SugarCube passage body.
///
/// Returns `None` if the zone map contains any `Error` leaves (the formatter
/// refuses to format broken input — the user fixes the error first). Returns
/// `Some(formatted_text)` otherwise.
///
/// `body_text` is the passage body (content after the `:: Name` header line).
/// `zones` is the zone map built from the same body (spans are passage-relative,
/// matching the body text starting at offset 0 — i.e., `body_offset_in_passage`
/// has already been applied by the caller, OR the zones were built with
/// `body_offset_in_passage = 0`).
///
/// **Note on offsets**: The zone map's spans are passage-relative (0 = passage
/// head `::`). The body text starts after the header line. If the caller passes
/// the body text starting at the first byte after the header newline, and the
/// zones were built with `body_offset_in_passage` matching that offset, then
/// the zone spans are already relative to the body text start. This is the
/// convention used by `ClassifiedPassage.body_text` and the production parse
/// pipeline. The formatter slices `body_text[leaf.span.start..leaf.span.end]`
/// directly, so the caller must ensure the body text and zone spans are
/// aligned.
pub fn format_passage(body_text: &str, zones: &ZoneMap) -> Option<String> {
    // Refuse to format if there are any Error leaves — the user should fix
    // errors first. Formatting broken input would produce misleading output.
    for leaf in zones.iter_leaves() {
        if matches!(leaf.kind, LeafKind::Error { .. }) {
            return None;
        }
    }

    let leaves: Vec<&LeafZone> = zones.iter_leaves().collect();

    let mut out = String::with_capacity(body_text.len() + 256);
    let mut at_line_start = true;
    // When true, the next Prose leaf should have its leading spaces stripped
    // (because a markup marker leaf just emitted a single space separator).
    let mut strip_next_prose_leading_spaces = false;

    for (i, leaf) in leaves.iter().enumerate() {
        // Compute the indentation depth for this leaf.
        //
        // The zone map's `MacroBody.depth` is the depth of the BODY itself
        // (0 at top level). A leaf INSIDE that body (whether prose, markup,
        // or a nested macro tag) should be indented one level deeper than
        // the body, so we use `body.depth + 1`.
        //
        // **Exception**: SubMacros (`<<else>>`, `<<case>>`, `<<default>>`)
        // are structural markers that should be at the PARENT's depth (the
        // body's depth), not one level deeper. They open on their own line
        // at the parent's indentation per the formatter spec.
        //
        // For leaves at the top level (body_idx=None), depth is 0.
        let current_depth: u32 = match leaf.body_idx {
            None => 0,
            Some(idx) => {
                let effective = zones.effective_depth(idx);
                let is_never_submacro = matches!(
                    &leaf.kind,
                    LeafKind::MacroTag {
                        macro_kind: Some(knot_core::types::MacroKind::SubMacro),
                        body_requirement: Some(knot_core::types::BodyRequirement::Never),
                        ..
                    }
                );
                if is_never_submacro {
                    effective.saturating_sub(1)
                } else {
                    effective
                }
            }
        };

        let raw_text = slice_span(body_text, &leaf.span);

        // If the previous leaf was a markup marker that inserted a space,
        // strip leading spaces from this Prose leaf so we get exactly one
        // space between the marker and the content.
        let text = if strip_next_prose_leading_spaces && matches!(leaf.kind, LeafKind::Prose { .. })
        {
            strip_next_prose_leading_spaces = false;
            raw_text.trim_start_matches(' ')
        } else {
            strip_next_prose_leading_spaces = false;
            raw_text
        };

        emit_leaf(
            &mut out,
            text,
            &leaf.kind,
            current_depth,
            &mut at_line_start,
        );

        // Zone-specific: after emitting a markup marker leaf (Heading, ListItem,
        // Blockquote), normalize the space between the marker and the following
        // Prose content. The zone builder splits `!heading` into Markup(`!`) +
        // Prose(`heading`), and `!!  heading` into Markup(`!!`) + Prose(`  heading`).
        // We want exactly one space: `! heading` and `!! heading`.
        if let LeafKind::Markup(markup_kind) = &leaf.kind
            && matches!(
                markup_kind,
                knot_core::zoning::MarkupKind::Heading
                    | knot_core::zoning::MarkupKind::ListItem
                    | knot_core::zoning::MarkupKind::Blockquote
            )
            && let Some(next) = leaves.get(i + 1)
            && matches!(next.kind, LeafKind::Prose { .. })
        {
            let next_text = slice_span(body_text, &next.span);
            if !next_text.starts_with('\n') && !next_text.is_empty() {
                out.push(' ');
                strip_next_prose_leading_spaces = true;
            }
        }
    }

    // Trim trailing whitespace from the final output.
    trim_trailing_newlines(&mut out);

    Some(out)
}

/// Extension method on `ZoneMap` to get the depth of a body by index.
///
/// This is a free function (not a method on `ZoneMap`) because `ZoneMap`
/// lives in `knot-core` and we don't want to add formatter-specific helpers
/// there. The formatter accesses `bodies` via `iter_bodies()` and looks up
/// depth by matching the body's span.
trait ZoneMapExt {
    /// Compute the effective depth for a leaf inside body `idx`.
    ///
    /// If the body is spurious (inline/unknown macro), walk up the parent
    /// chain until we find a REAL body (or reach top level). This ensures
    /// content inside spurious bodies is indented at the correct depth
    /// relative to the nearest real enclosing block macro.
    ///
    /// Example: `<<link>>` (real, depth 0) → `<<adjustStat>>` (spurious,
    /// depth 1) → content. The content should be at depth 1 (inside link),
    /// NOT depth 0. Without this walk, the spurious `<<adjustStat>>` body
    /// would mask the real `<<link>>` parent, causing wrong indentation.
    fn effective_depth(&self, idx: usize) -> u32;
}

impl ZoneMapExt for ZoneMap {
    fn effective_depth(&self, idx: usize) -> u32 {
        let mut current_idx = idx;
        loop {
            let Some(body) = self.iter_bodies().nth(current_idx) else {
                return 0;
            };
            let is_spurious = match body.body_requirement {
                Some(knot_core::types::BodyRequirement::Never) => true,
                None => true,
                Some(knot_core::types::BodyRequirement::Optional) => false,
                Some(knot_core::types::BodyRequirement::Required) => false,
            };
            if !is_spurious {
                // Real body — content is one level deeper.
                return body.depth + 1;
            }
            // Spurious body — walk up to the parent.
            match body.parent_body {
                Some(parent_idx) => current_idx = parent_idx,
                None => return 0,
            }
        }
    }
}

/// Slice a span from the body text, clamping to avoid panics on malformed input.
fn slice_span<'a>(body: &'a str, span: &Range<usize>) -> &'a str {
    let start = span.start.min(body.len());
    let end = span.end.max(start).min(body.len());
    &body[start..end]
}

/// Emit a single leaf's text into the output buffer.
///
/// The formatter is **zone-aware**: it uses the zone classification to decide
/// what's safe to format. In SugarCube, every `\n` becomes a `<br>` in the
/// rendered output, so the formatter must NOT add, remove, or collapse
/// newlines in content that renders to the player.
///
/// Zone dispatch:
/// - **`Prose { is_prose: true }`** + **`Markup(_)`** → renders to player.
///   Preserves all newlines verbatim. Only normalizes indentation (strip
///   existing, re-apply canonical), except block markers (`!`, `*`, `#`, `>`,
///   `|`) which must stay at column 0 for SugarCube to recognize them.
/// - **`MacroTag`** → structural, doesn't render. Safe to normalize
///   whitespace inside the tag (`<<set  $x>>` → `<<set $x>>`), indent.
/// - **`Prose { is_prose: false }`** → inside `<<script>>`/`<<silently>>`.
///   Treat like code: safe to normalize indentation.
/// - **`Raw`** → JS/CSS. Emit as-is (defer to language sub-formatter).
/// - **`Error`** → refused by `format_passage` (returns `None`).
fn emit_leaf(out: &mut String, text: &str, kind: &LeafKind, depth: u32, at_line_start: &mut bool) {
    match kind {
        LeafKind::Prose { is_prose } => {
            if *is_prose {
                // Player-rendering prose: preserve all newlines, only
                // normalize indentation.
                emit_player_text(out, text, depth, at_line_start);
            } else {
                // Non-rendering prose (inside <<script>>/<<silently>>):
                // treat like code, safe to normalize indentation.
                emit_code_text(out, text, depth, at_line_start);
            }
        }
        LeafKind::Markup(markup_kind) => {
            // Markup renders to the player — preserve all newlines.
            // Normalize the marker spacing (!heading → ! heading).
            emit_markup(out, text, *markup_kind, depth, at_line_start);
        }
        LeafKind::MacroTag { .. } => {
            // Structural — doesn't render. Safe to normalize whitespace
            // inside the tag and apply indentation.
            emit_macro_tag(out, text, depth, at_line_start);
        }
        LeafKind::Raw { .. } => {
            // JS/CSS — emit as-is, preserving newlines (a language
            // sub-formatter would handle internal formatting).
            emit_player_text(out, text, depth, at_line_start);
        }
        LeafKind::Error { .. } => {
            // Should be unreachable — format_passage() returns None early.
            emit_player_text(out, text, depth, at_line_start);
        }
    }
}

/// Emit markup text with SugarCube-specific normalization.
///
/// Markup renders to the player, so all newlines are preserved verbatim.
/// The only transformation is marker spacing normalization:
/// - **Heading** (`!`, `!!`, etc.): ensure exactly one space after the `!`s.
///   `!heading` → `! heading`, `!!  heading` → `!! heading`.
/// - **ListItem** (`*`, `**`, `#`, `##`): ensure exactly one space after the marker.
///   `*item` → `* item`, `*  item` → `* item`.
/// - **Blockquote** (`>`): ensure exactly one space after `>`.
///   `>quote` → `> quote`.
/// - Other markup: emit as-is (preserve newlines).
fn emit_markup(
    out: &mut String,
    text: &str,
    markup_kind: knot_core::zoning::MarkupKind,
    depth: u32,
    at_line_start: &mut bool,
) {
    use knot_core::zoning::MarkupKind;

    match markup_kind {
        MarkupKind::Heading | MarkupKind::ListItem | MarkupKind::Blockquote => {
            let normalized = normalize_markup_marker(text, markup_kind);
            emit_player_text(out, &normalized, depth, at_line_start);
        }
        _ => {
            emit_player_text(out, text, depth, at_line_start);
        }
    }
}

/// Normalize a markup marker line: ensure exactly one space after the marker.
///
/// - `!heading` → `! heading`
/// - `!!  heading` → `!! heading`
/// - `*item` → `* item`
/// - `>quote` → `> quote`
///
/// Only normalizes the FIRST line if it starts with a marker. Subsequent lines
/// (after newlines) are left as-is — they may be continuation lines that don't
/// have a marker.
fn normalize_markup_marker(text: &str, kind: knot_core::zoning::MarkupKind) -> String {
    use knot_core::zoning::MarkupKind;

    let marker_chars: &[char] = match kind {
        MarkupKind::Heading => &['!'],
        MarkupKind::ListItem => &['*', '#'],
        MarkupKind::Blockquote => &['>'],
        _ => return text.to_string(),
    };

    let mut lines = text.split_inclusive('\n');
    let first = lines.next();

    let mut result = String::with_capacity(text.len());

    if let Some(first_line) = first {
        let is_newline_terminated = first_line.ends_with('\n');
        let content = if is_newline_terminated {
            &first_line[..first_line.len() - 1]
        } else {
            first_line
        };

        let marker_count = content
            .chars()
            .take_while(|c| marker_chars.contains(c))
            .count();

        if marker_count > 0 {
            let marker = &content[..marker_count];
            let rest = content[marker_count..].trim_start_matches(' ');
            if rest.is_empty() {
                result.push_str(marker);
            } else {
                result.push_str(marker);
                result.push(' ');
                result.push_str(rest);
            }
        } else {
            result.push_str(content);
        }

        if is_newline_terminated {
            result.push('\n');
        }
    }

    for line in lines {
        result.push_str(line);
    }

    result
}

/// Emit text that renders to the player — preserve all newlines verbatim.
///
/// In SugarCube, every `\n` becomes a `<br>` in the rendered output. This
/// function preserves the exact newline structure of the source — it does NOT
/// trim, collapse, or add newlines. The only transformation is indentation
/// normalization:
///
/// - Strips existing leading whitespace on each line and re-applies canonical
///   indentation (depth × `\t`).
/// - Block markers (`!`, `*`, `#`, `>`, `|`) are NOT indented when inside
///   macro bodies — they must be at column 0 for SugarCube to recognize them.
fn emit_player_text(out: &mut String, text: &str, depth: u32, at_line_start: &mut bool) {
    if text.is_empty() {
        return;
    }

    // Normalize \r\n → \n, then strip any remaining lone \r. A lone \r at
    // the end of a zone leaf is NOT a line break — it's the first byte of a
    // \r\n pair that was split across zone boundaries by the zone builder
    // (the \n is at the start of the next leaf). Converting it to \n would
    // create a spurious extra newline. Stripping it is safe because the next
    // leaf starts with \n, which provides the line break.
    let owned_normalized;
    let text = if text.contains('\r') {
        owned_normalized = text.replace("\r\n", "\n").replace('\r', "");
        owned_normalized.as_str()
    } else {
        text
    };

    let indent = indent_str(depth);

    for line in text.split_inclusive('\n') {
        let is_newline_terminated = line.ends_with('\n');
        let line_content = if is_newline_terminated {
            &line[..line.len() - 1]
        } else {
            line
        };

        if *at_line_start {
            let stripped = line_content.trim_start_matches(['\t', ' ']);
            let is_block_marker = stripped
                .chars()
                .next()
                .is_some_and(|c| matches!(c, '!' | '*' | '#' | '>' | '|'));

            if !stripped.is_empty() && !(is_block_marker && depth > 0) {
                out.push_str(&indent);
            }
            out.push_str(stripped);
        } else {
            out.push_str(line_content);
        }

        if is_newline_terminated {
            out.push('\n');
            *at_line_start = true;
        } else if !line_content.trim().is_empty() {
            *at_line_start = false;
        }
    }
}

/// Emit non-rendering text (inside `<<script>>`/`<<silently>>`) — treat like
/// code. Safe to normalize indentation without worrying about newlines
/// affecting the rendered output (these zones don't render to the player).
fn emit_code_text(out: &mut String, text: &str, depth: u32, at_line_start: &mut bool) {
    if text.is_empty() {
        return;
    }

    let owned_normalized;
    let text = if text.contains('\r') {
        owned_normalized = text.replace("\r\n", "\n").replace('\r', "");
        owned_normalized.as_str()
    } else {
        text
    };

    let indent = indent_str(depth);

    for line in text.split_inclusive('\n') {
        let is_newline_terminated = line.ends_with('\n');
        let line_content = if is_newline_terminated {
            &line[..line.len() - 1]
        } else {
            line
        };

        if *at_line_start {
            let stripped = line_content.trim_start_matches(['\t', ' ']);
            if !stripped.is_empty() {
                out.push_str(&indent);
            }
            out.push_str(stripped);
        } else {
            out.push_str(line_content);
        }

        if is_newline_terminated {
            out.push('\n');
            *at_line_start = true;
        } else if !line_content.trim().is_empty() {
            *at_line_start = false;
        }
    }
}

/// Emit a macro tag (open, close, or expression).
///
/// The tag is emitted with indentation if at a line start. No newlines are
/// added before or after the tag — the newlines between macro tags and
/// surrounding content are in the Prose/Markup leaves, which preserve them
/// verbatim. This ensures the formatter doesn't change the rendered output
/// (in SugarCube, every `\n` becomes a `<br>`).
fn emit_macro_tag(out: &mut String, text: &str, depth: u32, at_line_start: &mut bool) {
    if *at_line_start {
        out.push_str(&indent_str(depth));
    }
    out.push_str(&normalize_macro_tag(text));
    *at_line_start = false;
}

/// Normalize a macro tag: trim ends and collapse multiple internal spaces
/// to a single space.
///
/// `<<adjustStat "stress"  9>>` → `<<adjustStat "stress" 9>>`
///
/// **Important**: spaces inside string literals are preserved. We track
/// single/double quote state to avoid collapsing spaces inside strings.
fn normalize_macro_tag(text: &str) -> String {
    // Normalize \r\n → \n and strip \r, then trim. This handles Windows
    // (CRLF) line endings inside macro tags, following the same pattern
    // used in emit_player_text above.
    let normalized = text.replace("\r\n", "\n").replace('\r', "");
    let trimmed = normalized.trim();
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_was_space = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for c in trimmed.chars() {
        match c {
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                prev_was_space = false;
                result.push(c);
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                prev_was_space = false;
                result.push(c);
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !prev_was_space {
                    result.push(' ');
                }
                prev_was_space = true;
            }
            _ => {
                prev_was_space = false;
                result.push(c);
            }
        }
    }

    result
}

/// Produce the indentation string for a given depth.
fn indent_str(depth: u32) -> String {
    INDENT_UNIT.repeat(depth as usize)
}

/// Trim trailing newlines from the output buffer.
fn trim_trailing_newlines(s: &mut String) {
    while s.ends_with('\n') {
        s.pop();
    }
    // Also trim trailing whitespace (tabs/spaces on the last line).
    while s.ends_with(' ') || s.ends_with('\t') {
        s.pop();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;
    use crate::sugarcube::registries::CustomMacroRegistry;
    use crate::zoning::build_from_ast;

    /// Helper: parse a body, build zones, and format.
    /// Returns None if the formatter refused (Error leaves present).
    fn format_body(body: &str) -> Option<String> {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let zones = build_from_ast(&ast.nodes, 0, &CustomMacroRegistry::new());
        format_passage(body, &zones)
    }

    /// Helper: parse, format, and assert the output matches a snapshot.
    /// Uses simple string comparison (not insta) to avoid snapshot file
    /// management in this phase — the formatted output is deterministic.
    fn assert_format_eq(body: &str, expected: &str) {
        let formatted = format_body(body).unwrap_or_else(|| {
            panic!(
                "formatter refused to format (Error leaves present) for body: {:?}",
                body
            )
        });
        assert_eq!(
            formatted, expected,
            "formatter output mismatch for body:\n{:?}\n--- got ---\n{}\n--- expected ---\n{}",
            body, formatted, expected
        );
    }

    /// Pure prose is passed through with no indentation changes.
    #[test]
    fn test_pure_prose() {
        assert_format_eq("Hello world.", "Hello world.");
    }

    /// A single inline macro stays inline.
    #[test]
    fn test_inline_macro() {
        assert_format_eq("<<set $x to 1>>", "<<set $x to 1>>");
    }

    /// A block macro gets its body indented.
    #[test]
    fn test_block_macro_indent() {
        assert_format_eq("<<if $x>>\nhello\n<</if>>", "<<if $x>>\n\thello\n<</if>>");
    }

    /// Nested block macros indent at each level.
    #[test]
    fn test_nested_block_macros() {
        assert_format_eq(
            "<<if $x>>\n<<link \"Go\">>\nclick\n<</link>>\n<</if>>",
            "<<if $x>>\n\t<<link \"Go\">>\n\t\tclick\n\t<</link>>\n<</if>>",
        );
    }

    /// `<<else>>` is a SubMacro — opens on its own line at the parent's depth.
    #[test]
    fn test_if_else() {
        assert_format_eq(
            "<<if $x>>\nyes\n<<else>>\nno\n<</if>>",
            "<<if $x>>\n\tyes\n<<else>>\n\tno\n<</if>>",
        );
    }

    /// `<<switch>>` / `<<case>>` indentation.
    #[test]
    fn test_switch_case() {
        assert_format_eq(
            "<<switch $x>>\n<<case 1>>\none\n<<default>>\nother\n<</switch>>",
            "<<switch $x>>\n\t<<case 1>>\n\t\tone\n\t<<default>>\n\t\tother\n<</switch>>",
        );
    }

    /// `<<script>>` body is emitted as-is (no JS sub-formatter yet).
    #[test]
    fn test_script_raw_body() {
        let body = "<<script>>\nvar x = 1;\n<</script>>";
        let formatted = format_body(body).expect("formatter should accept <<script>>");
        // The script body should be indented one level. We don't normalize JS.
        assert!(
            formatted.contains("<<script>>"),
            "formatted output should contain <<script>>: {:?}",
            formatted
        );
        assert!(
            formatted.contains("<</script>>"),
            "formatted output should contain <</script>>: {:?}",
            formatted
        );
        assert!(
            formatted.contains("var x = 1;"),
            "formatted output should preserve JS content: {:?}",
            formatted
        );
    }

    /// Inline macros in prose stay inline.
    #[test]
    fn test_inline_macro_in_prose() {
        let formatted = format_body("You have <<print $gold>> gold.").unwrap();
        assert!(
            formatted.contains("You have <<print $gold>> gold."),
            "inline macro should stay inline: {:?}",
            formatted
        );
    }

    /// The formatter is idempotent: format(format(x)) == format(x).
    #[test]
    fn test_idempotent_simple() {
        let body = "<<if $x>>\nhello\n<</if>>";
        let once = format_body(body).unwrap();
        let twice = format_body(&once).unwrap();
        assert_eq!(
            once, twice,
            "formatter is not idempotent:\nonce: {:?}\ntwice: {:?}",
            once, twice
        );
    }

    /// Idempotency on a more complex example.
    #[test]
    fn test_idempotent_complex() {
        let body = "<<if $x>>\n<<link \"Go\">>\nclick here\n<</link>>\n<<else>>\nno link\n<</if>>";
        let once = format_body(body).unwrap();
        let twice = format_body(&once).unwrap();
        assert_eq!(once, twice, "formatter is not idempotent on complex input");
    }

    /// The formatter refuses to format passages with Error leaves.
    #[test]
    fn test_refuses_error_input() {
        // An unclosed block macro produces an Error leaf? Let's check.
        // Actually, unclosed macros produce a MacroBody with unclosed=true,
        // not an Error leaf. Error leaves come from orphan close tags and
        // parse errors. Let's use an orphan close tag.
        let body = "<</if>>";
        let result = format_body(body);
        // An orphan close tag produces an Error leaf (OrphanClose kind).
        // The formatter should refuse.
        assert!(
            result.is_none(),
            "formatter should refuse to format input with Error leaves, got: {:?}",
            result
        );
    }

    /// Blank lines between top-level block macros are preserved — the
    /// formatter does NOT collapse them because in SugarCube every `\n`
    /// becomes a `<br>` in the rendered output.
    #[test]
    fn test_collapse_blank_lines_between_blocks() {
        let body = "<<if $x>>\nhello\n<</if>>\n\n\n\n<<if $y>>\nworld\n<</if>>";
        let formatted = format_body(body).unwrap();
        // Blank lines are preserved (not collapsed) — changing them would
        // change the rendered output.
        assert!(
            formatted.contains("<</if>>\n\n\n\n<<if $y>>"),
            "blank lines should be preserved, got: {:?}",
            formatted
        );
    }

    /// Markup (headings, lists) is preserved with indentation.
    #[test]
    fn test_markup_preserved() {
        let body = "! Heading\n* list item";
        let formatted = format_body(body).unwrap();
        assert!(
            formatted.contains("! Heading"),
            "heading should be preserved: {:?}",
            formatted
        );
        assert!(
            formatted.contains("* list item"),
            "list item should be preserved: {:?}",
            formatted
        );
    }

    // ===================================================================
    // Phase 10 bugfix tests — empty-body macros, close-tag indentation
    // ===================================================================

    /// An empty-body block macro (`<<link>><</link>>`) stays inline — no
    /// newline or indentation between the open and close tags.
    #[test]
    fn test_empty_body_macro_stays_inline() {
        assert_format_eq("<<link>><</link>>", "<<link>><</link>>");
    }

    /// An empty-body macro with arguments stays inline.
    #[test]
    fn test_empty_body_macro_with_args_stays_inline() {
        assert_format_eq("<<link \"Go\">><</link>>", "<<link \"Go\">><</link>>");
    }

    /// An empty-body macro in prose stays inline with the surrounding text.
    #[test]
    fn test_empty_body_macro_in_prose() {
        let body = "Click <<link \"here\">><</link>> to continue.";
        let formatted = format_body(body).unwrap();
        assert!(
            formatted.contains("Click <<link \"here\">><</link>> to continue."),
            "empty-body macro should stay inline in prose: {:?}",
            formatted
        );
    }

    /// A block macro with content gets the close tag at the open tag's depth
    /// (one less than the body content). This verifies the -1 indentation.
    #[test]
    fn test_close_tag_indentation_is_minus_one() {
        // <<if>> at depth 0, body content at depth 1, <</if>> at depth 0.
        assert_format_eq("<<if $x>>\nhello\n<</if>>", "<<if $x>>\n\thello\n<</if>>");
    }

    /// Nested block macros: close tags dedent correctly at each level.
    #[test]
    fn test_nested_close_tag_indentation() {
        // <<if>> depth 0, <<link>> depth 1, body depth 2,
        // <</link>> depth 1, <</if>> depth 0.
        assert_format_eq(
            "<<if $x>>\n<<link \"Go\">>\nclick\n<</link>>\n<</if>>",
            "<<if $x>>\n\t<<link \"Go\">>\n\t\tclick\n\t<</link>>\n<</if>>",
        );
    }

    /// Empty-body macro followed by content: the empty macro is inline,
    /// then the next block macro starts on a new line.
    #[test]
    fn test_empty_body_macro_then_block_macro() {
        let body = "<<link \"Go\">><</link>>\n<<if $x>>\ntext\n<</if>>";
        let formatted = format_body(body).unwrap();
        assert!(
            formatted.starts_with("<<link \"Go\">><</link>>"),
            "empty-body macro should be at start, inline: {:?}",
            formatted
        );
        assert!(
            formatted.contains("<<if $x>>"),
            "if macro should follow on new line: {:?}",
            formatted
        );
    }

    /// Empty `<<if>>` with immediate `<</if>>` stays inline.
    #[test]
    fn test_empty_if_stays_inline() {
        assert_format_eq("<<if $x>><</if>>", "<<if $x>><</if>>");
    }

    /// Idempotency with empty-body macros.
    #[test]
    fn test_idempotent_empty_body() {
        let body = "<<link \"Go\">><</link>>";
        let once = format_body(body).unwrap();
        let twice = format_body(&once).unwrap();
        assert_eq!(
            once, twice,
            "formatter is not idempotent on empty-body macro:\nonce: {:?}\ntwice: {:?}",
            once, twice
        );
    }

    // ===================================================================
    // Spacing normalization tests — existing indentation is consumed
    // ===================================================================

    /// 2-space indented body content is normalized to tab indentation.
    #[test]
    fn test_consumes_2space_indent() {
        assert_format_eq("<<if $x>>\n  hello\n<</if>>", "<<if $x>>\n\thello\n<</if>>");
    }

    /// 4-space indented body content is normalized to tab indentation.
    #[test]
    fn test_consumes_4space_indent() {
        assert_format_eq(
            "<<if $x>>\n    hello\n<</if>>",
            "<<if $x>>\n\thello\n<</if>>",
        );
    }

    /// Mixed tabs and spaces are normalized to tab indentation.
    #[test]
    fn test_consumes_mixed_indent() {
        assert_format_eq(
            "<<if $x>>\n  \thello\n<</if>>",
            "<<if $x>>\n\thello\n<</if>>",
        );
    }

    /// Nested macros with 2-space indentation are normalized to tabs at each level.
    #[test]
    fn test_consumes_nested_2space_indent() {
        assert_format_eq(
            "<<if $x>>\n  <<link \"Go\">>\n    click\n  <</link>>\n<</if>>",
            "<<if $x>>\n\t<<link \"Go\">>\n\t\tclick\n\t<</link>>\n<</if>>",
        );
    }

    /// An indented close tag is dedented to the correct depth.
    #[test]
    fn test_consumes_indented_close_tag() {
        assert_format_eq("<<if $x>>\nhello\n  <</if>>", "<<if $x>>\n\thello\n<</if>>");
    }

    /// Leading whitespace on top-level prose is stripped (it's indentation, not content).
    #[test]
    fn test_strips_leading_whitespace_on_prose() {
        assert_format_eq("  indented prose", "indented prose");
    }

    /// Leading whitespace before a top-level inline macro is stripped.
    #[test]
    fn test_strips_leading_whitespace_before_macro() {
        assert_format_eq("  <<set $x to 1>>", "<<set $x to 1>>");
    }

    /// Internal spaces (after non-whitespace) are preserved — only LEADING
    /// whitespace (indentation) is stripped.
    #[test]
    fn test_preserves_internal_spaces() {
        let formatted = format_body("! Heading with spaces").unwrap();
        assert_eq!(formatted, "! Heading with spaces");
    }

    /// Idempotency holds regardless of input indentation style.
    #[test]
    fn test_idempotent_with_space_indent() {
        let body = "<<if $x>>\n  <<link \"Go\">>\n    click\n  <</link>>\n<</if>>";
        let once = format_body(body).unwrap();
        let twice = format_body(&once).unwrap();
        assert_eq!(
            once, twice,
            "formatter is not idempotent on space-indented input:\nonce: {:?}\ntwice: {:?}",
            once, twice
        );
    }

    /// A newline-only body is preserved — `<<link>>\n<</link>>` stays as-is
    /// because the `\n` becomes a `<br>` in the rendered output. Collapsing
    /// it would change the game.
    #[test]
    fn test_newline_only_body_collapses() {
        // The formatter preserves the newline — it does NOT collapse.
        assert_format_eq("<<link>>\n<</link>>", "<<link>>\n<</link>>");
    }

    /// A whitespace-only body is preserved (not collapsed).
    #[test]
    fn test_whitespace_only_body_collapses() {
        let formatted = format_body("<<link>>  \n  <</link>>").unwrap();
        // The newline is preserved; only indentation is normalized.
        assert!(
            formatted.contains("<<link>>"),
            "should contain open tag: {:?}",
            formatted
        );
        assert!(
            formatted.contains("<</link>>"),
            "should contain close tag: {:?}",
            formatted
        );
    }

    /// A newline-only body with args is preserved (not collapsed).
    #[test]
    fn test_newline_only_body_with_args_collapses() {
        assert_format_eq("<<link \"Go\">>\n<</link>>", "<<link \"Go\">>\n<</link>>");
    }

    // ===================================================================
    // Markup normalization tests — zone-specific formatting rules
    // ===================================================================

    /// Heading without space after `!` gets normalized: `!heading` → `! heading`.
    #[test]
    fn test_heading_marker_normalized() {
        assert_format_eq("!heading", "! heading");
    }

    /// Heading with extra spaces gets normalized: `!!  heading` → `!! heading`.
    #[test]
    fn test_heading_extra_spaces_normalized() {
        assert_format_eq("!!  heading", "!! heading");
    }

    /// List item without space after `*` gets normalized: `*item` → `* item`.
    #[test]
    fn test_list_item_marker_normalized() {
        assert_format_eq("*item", "* item");
    }

    /// List item with `#` marker gets normalized: `#item` → `# item`.
    #[test]
    fn test_numbered_list_marker_normalized() {
        assert_format_eq("#item", "# item");
    }

    /// List item with extra spaces gets normalized: `*  item` → `* item`.
    #[test]
    fn test_list_item_extra_spaces_normalized() {
        assert_format_eq("*  item", "* item");
    }

    /// Blockquote without space after `>` gets normalized: `>quote` → `> quote`.
    #[test]
    fn test_blockquote_marker_normalized() {
        assert_format_eq(">quote", "> quote");
    }

    /// Heading inside a macro body is normalized but NOT indented — block
    /// markers (`!`, `*`, `#`, `>`) must be at column 0 for SugarCube to
    /// recognize them. Indenting `!` would break the heading.
    #[test]
    fn test_heading_in_macro_body() {
        assert_format_eq(
            "<<if $x>>\n!heading\n<</if>>",
            "<<if $x>>\n! heading\n<</if>>",
        );
    }

    // ===================================================================
    // Inline-CSS zoning split — formatter round-trip tests
    // ===================================================================
    // `style="…"` values zone as Raw(Css) leaves inside the tag's Raw(Html)
    // leaves (see zoning::emit_open_tag_leaves). The formatter emits every
    // Raw leaf via `emit_player_text`, so the SPLIT must be invisible in
    // the output: same bytes, same line structure.

    /// A tag with a style attribute round-trips byte-identically.
    #[test]
    fn test_style_attribute_tag_round_trips() {
        assert_format_eq(
            "<div class=\"hud\" style=\"color: red;\">text</div>",
            "<div class=\"hud\" style=\"color: red;\">text</div>",
        );
    }

    /// Multi-line style values: the split must produce the SAME output the
    /// single-leaf path always produced. `emit_player_text` normalizes
    /// continuation lines in EVERY Raw leaf (leading whitespace stripped,
    /// re-indented by depth — the established contract for JS/CSS bodies
    /// too), so a multi-line style value de-indents to column 0 at depth 0
    /// both before and after the inline-CSS zoning split.
    #[test]
    fn test_multiline_style_value_matches_single_leaf_contract() {
        assert_format_eq(
            "<div style=\"color: red;\n  margin: 0\">x</div>",
            "<div style=\"color: red;\nmargin: 0\">x</div>",
        );
    }

    /// Idempotency with markup normalization.
    #[test]
    fn test_idempotent_with_markup() {
        let body = "!heading\n*item\n>quote";
        let once = format_body(body).unwrap();
        let twice = format_body(&once).unwrap();
        assert_eq!(
            once, twice,
            "formatter is not idempotent on markup input:\nonce: {:?}\ntwice: {:?}",
            once, twice
        );
    }

    /// Unknown/custom macros (like `<<questLog>>`, `<<warn>>`) without close
    /// tags should NOT cause following prose to be indented.
    #[test]
    fn test_unknown_macro_no_indent() {
        let body = "<<questLog \"first-day\" \"desc\">>\n\n\tThe work is repetitive.";
        let formatted = format_body(body).unwrap();
        assert!(
            !formatted.contains("\tThe work"),
            "prose after unknown macro should NOT be indented: {:?}",
            formatted
        );
    }

    /// Unknown macro followed by a `<<link>>` — the link should be at depth 0.
    #[test]
    fn test_unknown_macro_then_link_no_extra_indent() {
        let body = "<<warn>>\n\n\t<<link \"Go\" \"Start\">><</link>>";
        let formatted = format_body(body).unwrap();
        assert!(
            !formatted.contains("\t<<link"),
            "link should NOT be tab-indented after unknown macro: {:?}",
            formatted
        );
    }

    /// Blank lines between top-level block macros are preserved (not
    /// collapsed), and the formatter is idempotent.
    #[test]
    fn test_blank_between_macros_idempotent() {
        let body = "<<if $x>>\n\thello\n<</if>>\n\n\n<<if $y>>\n\tworld\n<</if>>";
        let f1 = format_body(body).unwrap();
        let f2 = format_body(&f1).unwrap();
        let f3 = format_body(&f2).unwrap();
        assert_eq!(
            f1, f2,
            "not idempotent: f1 != f2\nf1: {:?}\nf2: {:?}",
            f1, f2
        );
        assert_eq!(f2, f3, "still growing: f2 != f3");
        // Blank lines are preserved (not collapsed to one).
        assert!(
            f1.contains("<</if>>\n\n\n<<if $y>>"),
            "blank lines should be preserved: {:?}",
            f1
        );
    }
}
