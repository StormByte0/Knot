//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a SugarCube operator (e.g., `gt`, `to`, `eq`).
///
/// Detects the word at the cursor position and checks if it's a known
/// operator via `plugin.describe_operator()`. Returns a plain-English
/// description so users can model their story logic without memorizing
/// the operator names.
///
/// **Scoping**: SugarCube keyword operators (`to`, `eq`, `gt`, `and`, etc.)
/// are ONLY valid inside macro expression contexts (e.g., `<<set $x to 5>>`,
/// `<<if $hp gt 0>>`). They are NOT operators when they appear in:
/// - Prose text (e.g., "Jump to combat demo")
/// - Link display text (e.g., `[[Jump to combat demo|CombatEncounter]]`)
/// - String literals inside macro args (e.g., `<<link "Go to forest">>`)
///
/// The token builder already correctly emits `Operator` semantic tokens
/// ONLY from oxc JS analysis of macro expressions. So this handler checks
/// whether an `Operator` token exists at the cursor position before firing.
/// If no Operator token is found, the word is just prose/text, not an
/// operator — return `None` so other hover handlers get a chance.
pub(super) fn try_operator_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    // ── Scope check: only fire if there's an Operator token at the cursor ──
    //
    // The token builder emits Operator tokens exclusively from oxc JS
    // analysis (inside macro expressions like `<<set $x to 5>>`). Words
    // like `to` in prose or link text do NOT get Operator tokens. So
    // checking for an Operator token at the cursor is the precise way
    // to distinguish a real operator from a coincidental word match.
    let mut has_operator_token = false;
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start
                && byte_offset < abs_end
                && matches!(
                    token.token_type,
                    knot_formats::plugin::SemanticTokenType::Operator
                )
            {
                has_operator_token = true;
                break;
            }
        }
        if has_operator_token {
            break;
        }
    }
    if !has_operator_token {
        return None;
    }

    // Extract the word at the cursor position.
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);

    // Find word boundaries around the cursor.
    //
    // SugarCube has two forms of operators:
    //   1. Keyword operators: `to`, `eq`, `and`, `def`, etc. — alphanumeric.
    //   2. Symbolic operators: `&&`, `||`, `!`, `===`, `!==`, `>`, `<`, etc.
    //
    // The extraction logic must handle BOTH forms. If the cursor is on an
    // alphanumeric char, we extract the alphanumeric word (keyword form).
    // If the cursor is on a symbolic operator char, we extract the symbolic
    // run (e.g., `&&`, `===`). This is safe because the `has_operator_token`
    // check above already confirmed we're inside an Operator semantic token —
    // we won't misfire on symbolic chars in prose or strings.
    let bytes = line.as_bytes();
    if byte_pos > bytes.len() {
        return None;
    }

    /// Returns true if `b` is a symbolic operator character.
    /// These are the characters that make up JS symbolic operators:
    /// `&`, `|`, `!`, `=`, `<`, `>`, `+`, `-`, `*`, `/`, `%`.
    fn is_operator_char(b: u8) -> bool {
        matches!(
            b,
            b'&' | b'|' | b'!' | b'=' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%'
        )
    }

    let cursor_byte = if byte_pos < bytes.len() {
        bytes[byte_pos]
    } else {
        // Cursor at end of line — check the last char
        if byte_pos == 0 {
            return None;
        }
        bytes[byte_pos - 1]
    };

    let (start, end) = if cursor_byte.is_ascii_alphanumeric() || cursor_byte == b'_' {
        // Keyword operator form — extract alphanumeric word.
        let mut start = byte_pos;
        while start > 0 {
            let b = bytes[start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = byte_pos;
        while end < bytes.len() {
            let b = bytes[end];
            if b.is_ascii_alphanumeric() || b == b'_' {
                end += 1;
            } else {
                break;
            }
        }
        (start, end)
    } else if is_operator_char(cursor_byte) {
        // Symbolic operator form — extract the symbolic run.
        let mut start = byte_pos;
        while start > 0 && is_operator_char(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = byte_pos;
        while end < bytes.len() && is_operator_char(bytes[end]) {
            end += 1;
        }
        (start, end)
    } else {
        // Cursor is on a non-operator, non-alphanumeric char (space, paren, etc.)
        return None;
    };

    if start == end {
        return None;
    }
    let word = &line[start..end];

    // Check if this word is a known operator.
    let desc = plugin.describe_operator(word)?;
    let hover_text = format!("**{}** `Operator`\n\n{}", word, desc);
    let utf16_start = helpers::utf16_len_up_to(line, start);
    let utf16_end = helpers::utf16_len_up_to(line, end);
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
