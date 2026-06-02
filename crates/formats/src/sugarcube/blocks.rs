//! Block extraction and building for SugarCube.
//!
//! Contains functions for extracting macro invocations and building
//! interleaved text/macro content blocks from passage bodies.
//!
//! ## Macro extraction strategy
//!
//! Macro extraction uses a **string-aware character scanner** instead of regex.
//! This is necessary because `>` and `>>` characters can appear inside macro
//! conditions (e.g., `<<if _parts.length > 0>>`, `<<if x >> 1>>`), and regex
//! patterns cannot reliably distinguish between `>>` as a comparison operator
//! and `>>` as a macro closing delimiter.
//!
//! The scanner tracks:
//! - **Quoted strings**: `>>` inside `"..."` or `'...'` is never a macro close
//! - **Nested macros**: `<<` inside the args of another macro increases nesting
//! - **Close macros**: `<</name>>` is matched by the scanner since `/` after `<<`
//!   unambiguously identifies a close tag (no `>` in close tag names)

use knot_core::passage::Block;

/// A parsed macro tag: `<<name args>>` or `<</name>>`.
///
/// Used internally by the scanner and exposed for token generation
/// in `tokens.rs`.
#[derive(Debug, Clone)]
pub(crate) struct ParsedMacro {
    /// The macro name (e.g., "if", "set", "/if").
    pub name: String,
    /// The arguments string (empty for close tags).
    pub args: String,
    /// Byte offset of the macro name start within the body (after `<<`
    /// and `/` for close tags).
    pub name_start: usize,
    /// Byte length of the macro name.
    pub name_len: usize,
    /// Byte offset of the macro start (`<<`) within the body.
    pub start: usize,
    /// Byte offset one past the macro end (`>>`) within the body.
    pub end: usize,
}

/// Extract macros from a passage body using a string-aware character scanner.
///
/// This function scans the body text character by character, properly handling:
/// - `>` and `>>` inside quoted strings (not treated as macro closers)
/// - `>` and `>>` inside macro conditions (e.g., `<<if x > 0>>`)
/// - Nested macro tags within arguments
/// - Close macro tags (`<</name>>`)
///
/// The scanner works by finding each `<<` opening, then scanning forward to
/// find the matching `>>` close, skipping over quoted strings and nested
/// `<<...>>` pairs. This is the same approach used by the bracket validator
/// and the signature help handler.
pub(crate) fn scan_macros(body: &str) -> Vec<ParsedMacro> {
    let mut macros = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for `<<`
        if i + 1 < len && bytes[i] == b'<' && bytes[i + 1] == b'<' {
            let macro_start = i;

            // Check if this is a close tag: `<</`
            let is_close = i + 2 < len && bytes[i + 2] == b'/';

            if is_close {
                // Close macro: <</name>>
                // Scan forward for the name and closing >>
                let name_start = i + 3; // skip `<</`
                let mut name_end = name_start;

                // First char: letter or underscore
                if name_end < len && (bytes[name_end].is_ascii_alphabetic() || bytes[name_end] == b'_') {
                    name_end += 1;
                    // Subsequent chars: letters, digits, underscore
                    while name_end < len
                        && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'_')
                    {
                        name_end += 1;
                    }
                }

                // Look for >> immediately after the name
                if name_end > name_start
                    && name_end + 1 < len
                    && bytes[name_end] == b'>'
                    && bytes[name_end + 1] == b'>'
                {
                    let name = &body[name_start..name_end];
                    let name_len = name.len();
                    macros.push(ParsedMacro {
                        name: format!("/{}", name),
                        args: String::new(),
                        name_start,
                        name_len,
                        start: macro_start,
                        end: name_end + 2,
                    });
                    i = name_end + 2;
                    continue;
                }

                // Not a valid close tag — skip past `<<`
                i += 2;
                continue;
            }

            // Open macro: <<name args>>
            // Parse the macro name
            let name_start = i + 2; // skip `<<`
            let mut name_end = name_start;

            // Name can start with letter, _, =, or - (for print shorthand)
            if name_end < len
                && (bytes[name_end].is_ascii_alphabetic()
                    || bytes[name_end] == b'_'
                    || bytes[name_end] == b'='
                    || bytes[name_end] == b'-')
            {
                name_end += 1;
                // Subsequent chars: letters, digits, underscore
                while name_end < len
                    && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'_')
                {
                    name_end += 1;
                }
            }

            if name_end == name_start {
                // No valid name after `<<` — not a macro
                i += 2;
                continue;
            }

            let name = &body[name_start..name_end];
            let name_len = name.len();

            // Now scan forward from name_end to find the closing `>>`
            // We need to handle:
            // - Quoted strings: skip contents inside "..." and '...'
            // - Nested <<...>>: track nesting depth
            // - The actual >> that closes THIS macro
            let mut scan = name_end;
            let mut in_string: Option<u8> = None; // Some(b'"') or Some(b'\'')

            while scan < len {
                // Handle string literals
                if let Some(quote) = in_string {
                    if bytes[scan] == b'\\' && scan + 1 < len {
                        scan += 2; // skip escaped char
                        continue;
                    }
                    if bytes[scan] == quote {
                        in_string = None;
                    }
                    scan += 1;
                    continue;
                }

                // Enter string
                if bytes[scan] == b'"' || bytes[scan] == b'\'' {
                    in_string = Some(bytes[scan]);
                    scan += 1;
                    continue;
                }

                // Check for nested `<<` — these start a nested macro
                if scan + 1 < len && bytes[scan] == b'<' && bytes[scan + 1] == b'<' {
                    // This is a nested macro opening. We need to skip past it
                    // and its closing `>>` to find the `>>` that closes OUR macro.
                    let _nested_start = scan;
                    scan += 2; // skip `<<`

                    // Check for nested close tag
                    if scan < len && bytes[scan] == b'/' {
                        // Nested close tag — just scan to its >>
                        while scan + 1 < len {
                            if bytes[scan] == b'>' && bytes[scan + 1] == b'>' {
                                scan += 2;
                                break;
                            }
                            scan += 1;
                        }
                    } else {
                        // Nested open tag — scan to its closing >> (handling
                        // further nesting recursively is complex, so we use
                        // a depth counter)
                        let mut nested_depth = 1;
                        let mut nested_in_string: Option<u8> = None;

                        while scan < len && nested_depth > 0 {
                            // String handling inside nested macro
                            if let Some(quote) = nested_in_string {
                                if bytes[scan] == b'\\' && scan + 1 < len {
                                    scan += 2;
                                    continue;
                                }
                                if bytes[scan] == quote {
                                    nested_in_string = None;
                                }
                                scan += 1;
                                continue;
                            }

                            if bytes[scan] == b'"' || bytes[scan] == b'\'' {
                                nested_in_string = Some(bytes[scan]);
                                scan += 1;
                                continue;
                            }

                            if scan + 1 < len && bytes[scan] == b'<' && bytes[scan + 1] == b'<' {
                                nested_depth += 1;
                                scan += 2;
                                continue;
                            }

                            if scan + 1 < len && bytes[scan] == b'>' && bytes[scan + 1] == b'>' {
                                nested_depth -= 1;
                                scan += 2;
                                if nested_depth == 0 {
                                    break;
                                }
                                continue;
                            }

                            scan += 1;
                        }
                    }
                    continue;
                }

                // Check for `>>` — the closing delimiter
                if scan + 1 < len && bytes[scan] == b'>' && bytes[scan + 1] == b'>' {
                    // Check for `>>>` (JS unsigned right shift) — not a macro close.
                    // `>>>` is a triple-chevron operator, not two `>` closers + one `>` opener.
                    if scan + 2 < len && bytes[scan + 2] == b'>' {
                        scan += 3; // skip >>>
                        continue;
                    }

                    // Heuristic: check if this `>>` is a right-shift operator
                    // inside a macro condition rather than the closing delimiter.
                    //
                    // In `<<if x >> 1>>`, the first `>>` is a right-shift and
                    // the second `>>` is the macro closer. We detect this by
                    // checking if `>>` is followed by expression-like content
                    // (space + digit/letter/variable) AND there is a later `>>`
                    // that could be the real closer.
                    //
                    // We also check for `>>=` (right-shift assignment) which
                    // is never a macro closer.
                    if is_right_shift_operator(body, bytes, len, scan) {
                        scan += 2; // skip >> (it's an operator, not a closer)
                        continue;
                    }

                    // Found the closing `>>` for this macro
                    let macro_end = scan + 2;
                    let args = if name_end < scan {
                        body[name_end..scan].trim().to_string()
                    } else {
                        String::new()
                    };

                    macros.push(ParsedMacro {
                        name: name.to_string(),
                        args,
                        name_start,
                        name_len,
                        start: macro_start,
                        end: macro_end,
                    });
                    scan = macro_end;
                    break;
                }

                scan += 1;
            }

            i = scan;
            continue;
        }

        i += 1;
    }

    macros
}

/// Extract macros from a passage body and produce content blocks.
///
/// Uses a **string-aware character scanner** instead of regex to correctly
/// handle `>` and `>>` characters inside macro conditions and quoted strings.
///
/// Both open macros (`<<name args>>`) and close macros (`<</name>>`) are
/// extracted in **source position order** (since the scanner processes them
/// sequentially from left to right).
#[allow(dead_code)] // Replaced by walk_blocks() in passage_tree.rs (Phase 1)
pub(crate) fn extract_macros(body: &str, body_offset: usize) -> Vec<Block> {
    let parsed = scan_macros(body);

    parsed
        .into_iter()
        .map(|m| Block::Macro {
            name: m.name,
            args: m.args,
            span: body_offset + m.start..body_offset + m.end,
        })
        .collect()
}

/// Build content blocks from the body text, interleaving text and macro
/// blocks without duplication.
///
/// Collects macro spans, then creates text blocks only for the gaps
/// between macros (or the whole body if no macros are present).
///
/// **Precondition**: The `macros` slice must be sorted by span start position.
/// `extract_macros()` guarantees this.
#[allow(dead_code)] // Replaced by walk_blocks() in passage_tree.rs (Phase 1)
pub(crate) fn build_body_blocks(body: &str, body_offset: usize, macros: &[Block]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();

    if macros.is_empty() {
        // No macros — the entire body is a single text block
        if !body.trim().is_empty() {
            blocks.push(Block::Text {
                content: body.to_string(),
                span: body_offset..body_offset + body.len(),
            });
        }
        return blocks;
    }

    // Since macros are now sorted by source position, we can iterate
    // them directly instead of maintaining a separate sorted span list.
    let mut cursor: usize = 0;

    for macro_block in macros {
        let Block::Macro { span, .. } = macro_block else {
            continue;
        };
        let rel_start = span.start - body_offset;
        let rel_end = span.end - body_offset;

        // Add text block for the gap before this macro (if non-empty)
        if cursor < rel_start {
            let gap = &body[cursor..rel_start];
            if !gap.trim().is_empty() {
                blocks.push(Block::Text {
                    content: gap.to_string(),
                    span: body_offset + cursor..body_offset + rel_start,
                });
            }
        }

        // Add the macro block itself
        blocks.push(macro_block.clone());

        cursor = rel_end;
    }

    // Add trailing text after the last macro
    if cursor < body.len() {
        let trailing = &body[cursor..];
        if !trailing.trim().is_empty() {
            blocks.push(Block::Text {
                content: trailing.to_string(),
                span: body_offset + cursor..body_offset + body.len(),
            });
        }
    }

    blocks
}

/// Heuristic to determine if `>>` at position `pos` in the body is a JavaScript
/// right-shift operator rather than a macro closing delimiter.
///
/// Returns `true` if:
/// 1. `>>=` follows (right-shift assignment — never a macro close)
/// 2. `>>` is followed by expression-like content AND there is a later `>>`
///    that could be the real macro closer
///
/// This handles cases like:
/// - `<<if x >> 1>>` — first `>>` is right-shift, second is closer
/// - `<<if $mask >> 2 >> 0>>` — first `>>` is right-shift, second is unsigned
///   right-shift, third is closer
/// - `<<set $x >>= 5>>` — `>>=` is right-shift assignment, final `>>` is closer
fn is_right_shift_operator(_body: &str, bytes: &[u8], len: usize, pos: usize) -> bool {
    // Check for >>= (right-shift assignment) — always an operator
    if pos + 2 < len && bytes[pos + 2] == b'=' {
        return true;
    }

    // Check if `>>` is followed by expression-like content.
    // Skip whitespace after `>>`, then check the next character.
    let after = pos + 2;
    let mut peek = after;
    while peek < len && (bytes[peek] == b' ' || bytes[peek] == b'\t') {
        peek += 1;
    }

    if peek >= len {
        // `>>` at end of input — it's the closer
        return false;
    }

    let next_non_space = bytes[peek];

    // If the next non-space character is not an expression continuation,
    // this `>>` is likely the macro closer.
    let is_expression_char = next_non_space.is_ascii_digit()
        || next_non_space.is_ascii_alphabetic()
        || next_non_space == b'$'
        || next_non_space == b'_'
        || next_non_space == b'('
        || next_non_space == b'-'  // negative number
        || next_non_space == b'!'; // logical not

    if !is_expression_char {
        return false;
    }

    // `>>` is followed by expression-like content. Now check if there's
    // a later `>>` (outside strings) that could be the real closer.
    // If there is, this `>>` is likely a right-shift operator.
    let mut lookahead = after;
    let mut la_in_string: Option<u8> = None;

    while lookahead < len {
        // Handle string literals in lookahead
        if let Some(quote) = la_in_string {
            if bytes[lookahead] == b'\\' && lookahead + 1 < len {
                lookahead += 2;
                continue;
            }
            if bytes[lookahead] == quote {
                la_in_string = None;
            }
            lookahead += 1;
            continue;
        }

        if bytes[lookahead] == b'"' || bytes[lookahead] == b'\'' {
            la_in_string = Some(bytes[lookahead]);
            lookahead += 1;
            continue;
        }

        // If we see `<<`, a nested macro starts — stop lookahead
        // because the next `>>` would close the nested macro, not ours.
        // However, if there are matching `<<`/`>>` pairs, we need to
        // skip past them. For simplicity, stop at `<<`.
        if lookahead + 1 < len && bytes[lookahead] == b'<' && bytes[lookahead + 1] == b'<' {
            break;
        }

        // Found a later `>>` — this means the current `>>` is likely
        // a right-shift operator and the later `>>` is the closer.
        if lookahead + 1 < len && bytes[lookahead] == b'>' && bytes[lookahead + 1] == b'>' {
            // But check for `>>>` — if it's `>>>`, it's an unsigned right shift,
            // not our closer. Skip it.
            if lookahead + 2 < len && bytes[lookahead + 2] == b'>' {
                lookahead += 3;
                continue;
            }
            // Check for `>>=` — right-shift assignment, skip it too
            if lookahead + 2 < len && bytes[lookahead + 2] == b'=' {
                lookahead += 3;
                continue;
            }
            return true; // Found a later >> that's the real closer
        }

        lookahead += 1;
    }

    // No later >> found — this >> is the only candidate, so it's the closer
    false
}

/// Parse quoted string arguments from a macro's argument string.
///
/// Extracts the content of `"..."` and `'...'` quoted strings from the
/// args portion of a macro invocation. This handles:
/// - `<<goto "PassageName">>` → ["PassageName"]
/// - `<<link "Label" "PassageName">>` → ["Label", "PassageName"]
/// - `<<include 'Some Passage'>>` → ["Some Passage"]
///
/// Returns tuples of (content, rel_start, rel_end) where rel_start/rel_end
/// are byte offsets relative to the args string, covering the content
/// INSIDE the quotes (not including the quote characters themselves).
///
/// Shared by `links.rs` (for macro passage reference extraction) and
/// `tokens.rs` (for semantic token generation).
pub(crate) fn parse_quoted_args(args: &str) -> Vec<(String, usize, usize)> {
    let mut result = Vec::new();
    let mut chars = args.char_indices().peekable();

    while let Some(&(_pos, c)) = chars.peek() {
        if c == '"' || c == '\'' {
            let quote = c;
            chars.next(); // consume opening quote
            let content_start = chars.peek().map(|&(i, _)| i).unwrap_or(args.len());
            let mut content = String::new();
            let mut content_end = content_start;
            while let Some(&(i, cc)) = chars.peek() {
                if cc == quote {
                    content_end = i;
                    chars.next(); // consume closing quote
                    break;
                }
                content.push(cc);
                content_end = i + cc.len_utf8();
                chars.next();
            }
            if !content.is_empty() {
                result.push((content, content_start, content_end));
            }
        } else {
            chars.next(); // skip non-quote characters
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_simple_macro() {
        let macros = scan_macros("<<set $x to 5>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "set");
        assert_eq!(macros[0].args, "$x to 5");
    }

    #[test]
    fn scan_if_with_gt_condition() {
        let macros = scan_macros("<<if _parts.length > 0>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "if");
        assert_eq!(macros[0].args, "_parts.length > 0");
    }

    #[test]
    fn scan_if_with_double_gt_condition() {
        // <<if x >> 1>> — the >> in the condition should NOT be treated
        // as the macro closer. The real closer is the last >>.
        let macros = scan_macros("<<if x >> 1>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "if");
        assert_eq!(macros[0].args, "x >> 1");
    }

    #[test]
    fn scan_nested_if_with_gt() {
        let body = "<<if _parts.length > 0>>\n  <<if _parts.length > 1>>ok<</if>>\n<<else>>\n  nope\n<</if>>";
        let macros = scan_macros(body);
        assert_eq!(macros.len(), 5);

        // <<if _parts.length > 0>>
        assert_eq!(macros[0].name, "if");
        assert_eq!(macros[0].args, "_parts.length > 0");

        // <<if _parts.length > 1>>
        assert_eq!(macros[1].name, "if");
        assert_eq!(macros[1].args, "_parts.length > 1");

        // <</if>>
        assert_eq!(macros[2].name, "/if");

        // <<else>>
        assert_eq!(macros[3].name, "else");
        assert_eq!(macros[3].args, "");

        // <</if>>
        assert_eq!(macros[4].name, "/if");
    }

    #[test]
    fn scan_gt_in_string() {
        // >> inside a quoted string should not close the macro
        let macros = scan_macros(r#"<<if $x gt "a>>b">>yes<</if>>"#);
        assert_eq!(macros.len(), 2);
        assert_eq!(macros[0].name, "if");
        assert!(macros[0].args.contains("a>>b"));
    }

    #[test]
    fn scan_print_shorthand() {
        let macros = scan_macros("<<= _parts[0] >>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "=");
    }

    #[test]
    fn scan_close_macro() {
        let macros = scan_macros("<</if>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "/if");
    }

    #[test]
    fn scan_set_with_string_arg() {
        let macros = scan_macros(r#"<<set $x to "hello">>"#);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "set");
        assert!(macros[0].args.contains("hello"));
    }

    #[test]
    fn scan_right_shift_assignment() {
        // >>= (right-shift assignment) should not be treated as a macro close
        let macros = scan_macros("<<set $x >>= 5>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "set");
        assert!(macros[0].args.contains(">>="));
    }

    #[test]
    fn scan_unsigned_right_shift() {
        // >>> (unsigned right shift) should not be treated as a macro close
        let macros = scan_macros("<<if $mask >>> 2 >> 0>>");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "if");
        assert!(macros[0].args.contains(">>>"));
    }

    #[test]
    fn scan_full_nested_example() {
        // The exact example from the bug report
        let body = r#"<<if _parts.length > 0>>
  <<= _parts[0] >>
  <<if _parts.length > 1>> +<<= _parts.length - 1 >><</if>>
<<else>>
  &mdash;
<</if>>"#;
        let macros = scan_macros(body);
        // Should find: <<if>>, <<=>>, <<if>>, <<=>>, <</if>>, <<else>>, <</if>>
        assert_eq!(macros.len(), 7);

        // First <<if _parts.length > 0>>
        assert_eq!(macros[0].name, "if");
        assert!(macros[0].args.contains("> 0"));

        // <<= _parts[0] >>
        assert_eq!(macros[1].name, "=");

        // Second <<if _parts.length > 1>>
        assert_eq!(macros[2].name, "if");
        assert!(macros[2].args.contains("> 1"));

        // <<= _parts.length - 1 >>
        assert_eq!(macros[3].name, "=");

        // <</if>> (closes inner)
        assert_eq!(macros[4].name, "/if");

        // <<else>>
        assert_eq!(macros[5].name, "else");

        // <</if>> (closes outer)
        assert_eq!(macros[6].name, "/if");
    }
}
