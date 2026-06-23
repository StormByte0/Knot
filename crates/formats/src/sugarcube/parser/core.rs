//! Core parser — main parse loop and text flushing.

use std::ops::Range;
use crate::sugarcube::ast::*;
use super::predicates::is_ident_start;
use super::variable_scan::{scan_variable, scan_inline_vars};
use super::macro_parser::parse_macro;
use super::link_parser::parse_link;
use super::comment::{
    parse_block_comment,
    parse_cstyle_comment,
    parse_js_line_comment,
    parse_html_comment,
    parse_html_conditional_comment,
};

/// Mutable parser state threaded through the main parse loop.
///
/// `offset` is the byte offset within the passage body where this segment
/// starts (0 for the top level, nonzero for nested content inside block
/// macros or inline styles). It is added to local byte positions to produce
/// body-relative spans stored in AST nodes.
///
/// `col` is the current column (0-based) within the line. It is reset to 0
/// whenever a `\n` is consumed and incremented by the byte length of each
/// other character consumed. Block-level SugarCube constructs (headings,
/// lists, blockquotes, horizontal rules, tables, block code) require `col
/// == 0` — SugarCube's Wikifier anchors all block parsers at column 0 and
/// tolerates NO leading whitespace (see plan.md §3.3).
///
/// `col` is also used by the `//` heuristic to decide whether `//` is a
/// line comment vs. prose — though the existing `bytes[i-1] == b'\n'`
/// peek is kept as a fallback for backward compatibility during the
/// Phase 1 refactor.
struct ParseCtx {
    offset: usize,
    col: usize,
}

/// Parse body text into AST nodes.
///
/// `offset` is the byte offset within the body where this segment starts
/// (0 for the top level, nonzero for nested content inside block macros).
pub(super) fn parse_body(text: &str, offset: usize) -> Vec<AstNode> {
    parse_body_with_ctx(text, &mut ParseCtx { offset, col: 0 })
}

/// Internal parse loop with explicit context (offset + column tracking).
///
/// This is the workhorse. The public `parse_body` wrapper just creates a
/// fresh `ParseCtx` with `col: 0` (top-level call always starts at column 0).
/// Recursive calls (e.g. from `parse_inline_style`) pass a context with the
/// correct initial column for the nested content.
fn parse_body_with_ctx(text: &str, ctx: &mut ParseCtx) -> Vec<AstNode> {
    let offset = ctx.offset;
    let mut nodes = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;
    let bytes = text.as_bytes();
    let len = bytes.len();

    while i < len {
        // Try to match a delimiter at the current position
        let matched = match bytes[i] {
            b'<' if i + 1 < len && bytes[i + 1] == b'<' => {
                // << — macro open, OR <<< — block-style blockquote (disambiguation).
                //
                // SugarCube's `quoteByBlock` parser matches `^<<<\n` at column 0
                // (plan.md §3.8.2). This is UNDOCUMENTED but in the source.
                // `<<<` starts with `<<`, so we must check for it BEFORE macro
                // parsing. The disambiguation (plan.md §8.1.2):
                //   - If ctx.col == 0 AND bytes[i+2] == b'<' AND (i+3 == len OR
                //     bytes[i+3] == b'\n'), it's a block-style blockquote opener.
                //   - Otherwise, it's a `<<` macro (the third `<` is part of the
                //     macro name/args, which is unusual but not forbidden).
                let is_blockquote_block = ctx.col == 0
                    && i + 2 < len
                    && bytes[i + 2] == b'<'
                    && (i + 3 == len || bytes[i + 3] == b'\n');
                if is_blockquote_block {
                    let start = i;
                    let node = parse_blockquote_block(text, &mut i, ctx, start);
                    resync_col_after_advance(text, start, i, ctx);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                } else {
                    // << — macro open
                    let start = i;
                    i += 2;
                    ctx.col += 2;
                    let node = parse_macro(text, &mut i, offset, start);
                    resync_col_after_advance(text, start, i, ctx);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                }
            }
            b'[' if i + 1 < len && bytes[i + 1] == b'[' => {
                // [[ — link
                let start = i;
                i += 2;
                ctx.col += 2;
                let node = parse_link(text, &mut i, offset, start);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'%' => {
                // /% or /%% — Twine/SugarCube comment
                let start = i;
                let is_sugarcube = i + 2 < len && bytes[i + 2] == b'%';
                let delim_len = if is_sugarcube { 3 } else { 2 };
                i += delim_len;
                ctx.col += delim_len;
                let node = parse_block_comment(text, &mut i, offset + start, is_sugarcube);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                // /* — C-style block comment (CSS/JS)
                let start = i;
                i += 2;
                ctx.col += 2;
                let node = parse_cstyle_comment(text, &mut i, offset + start);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                // // — Could be italic formatting (//text//) or a JS line comment.
                // Check for italic first: if there's a closing // on the same line,
                // it's formatting, not a comment.
                let has_closing_double_slash = {
                    let mut k = i + 2;
                    let mut found = false;
                    while k + 1 < len && bytes[k] != b'\n' {
                        if bytes[k] == b'/' && bytes[k + 1] == b'/' {
                            found = true;
                            break;
                        }
                        k += 1;
                    }
                    found
                };

                if has_closing_double_slash {
                    // Italic formatting: //text//
                    let start = i;
                    i += 2;
                    let content_start = i;
                    while i + 1 < len && !(bytes[i] == b'/' && bytes[i + 1] == b'/') {
                        i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                    }
                    let content = text[content_start..i].to_string();
                    if i + 1 < len {
                        i += 2; // skip closing //
                    }
                    resync_col_after_advance(text, start, i, ctx);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(AstNode::TextFormat {
                        kind: TextFormatKind::Italic,
                        content,
                        span: offset + start..offset + i,
                    })
                } else {
                    // Not italic — check if this // is a line comment.
                    //
                    // In SugarCube prose, // is ALWAYS a comment (unless it's
                    // italic formatting, which we already checked above). The
                    // only exception is // inside a URL like http://example.com
                    // — but there // is preceded by ':', not whitespace.
                    //
                    // So the rule is simple: if // is at line start OR preceded
                    // by whitespace (space/tab), it's a comment. We do NOT
                    // require a space AFTER // — `//comment` (no space) is just
                    // as valid a comment as `// comment`.
                    let is_comment_context = if i + 2 <= len {
                        // Prefer the threaded column counter (composable across
                        // nested parse_body calls). Fall back to the byte-peek
                        // for the start-of-input edge case to preserve existing
                        // behavior during the Phase 1 refactor.
                        let at_line_start = ctx.col == 0 || (i > 0 && bytes[i - 1] == b'\n');
                        let preceded_by_whitespace = i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t');
                        at_line_start || preceded_by_whitespace
                    } else {
                        true
                    };
                    if is_comment_context {
                        let start = i;
                        i += 2;
                        ctx.col += 2;
                        let node = parse_js_line_comment(text, &mut i, offset + start);
                        resync_col_after_advance(text, start, i, ctx);
                        flush_text(text, &mut text_start, start, offset, &mut nodes);
                        Some(node)
                    } else {
                        let adv = text[i..].chars().next().map_or(1, |c| c.len_utf8());
                        i += adv;
                        ctx.col += adv;
                        None
                    }
                }
            }
            b'\'' if i + 1 < len && bytes[i + 1] == b'\'' => {
                // '' — bold formatting: ''text''
                let start = i;
                i += 2;
                ctx.col += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'\'' && bytes[i + 1] == b'\'') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Bold, content, span: offset + start..offset + i,
                })
            }
            b'_' if i + 1 < len && bytes[i + 1] == b'_' => {
                // __ — underline formatting: __text__
                let start = i;
                i += 2;
                ctx.col += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'_' && bytes[i + 1] == b'_') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Underline, content, span: offset + start..offset + i,
                })
            }
            b'=' if i + 1 < len && bytes[i + 1] == b'=' => {
                // == — strike formatting: ==text==
                let start = i;
                i += 2;
                ctx.col += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'=' && bytes[i + 1] == b'=') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Strike, content, span: offset + start..offset + i,
                })
            }
            b'~' if i + 1 < len && bytes[i + 1] == b'~' => {
                // ~~ — subscript formatting: ~~text~~
                let start = i;
                i += 2;
                ctx.col += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'~' && bytes[i + 1] == b'~') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Sub, content, span: offset + start..offset + i,
                })
            }
            b'^' if i + 1 < len && bytes[i + 1] == b'^' => {
                // ^^ — superscript formatting: ^^text^^
                let start = i;
                i += 2;
                ctx.col += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'^' && bytes[i + 1] == b'^') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Super, content, span: offset + start..offset + i,
                })
            }
            b'<' if i + 3 < len && &text[i..i + 4] == "<!--" => {
                // <!-- — HTML comment (or conditional comment <!--[if ...]>)
                let start = i;
                i += 4;
                ctx.col += 4;
                // Check for conditional comment: <!--[if ...]>
                let is_conditional = text[i..].trim_start().starts_with("[if");
                if is_conditional {
                    let node = parse_html_conditional_comment(text, &mut i, offset + start);
                    resync_col_after_advance(text, start, i, ctx);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                } else {
                    let node = parse_html_comment(text, &mut i, offset + start);
                    resync_col_after_advance(text, start, i, ctx);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                }
            }
            b'$' if i + 1 < len && bytes[i + 1] == b'$' => {
                // $$ — escaped dollar, include in text
                i += 2;
                ctx.col += 2;
                None
            }
            b'$' if i + 1 < len && is_ident_start(bytes[i + 1]) => {
                // $var — story variable in text
                let (_var_ref, end) = scan_variable(text, i, false);
                // Don't create a separate node for inline vars in text.
                // Instead, they'll be picked up when we flush the text node.
                let adv = end - i;
                i = end;
                ctx.col += adv;
                // We don't break the text gap here — inline $vars in prose
                // are part of the text flow. The var_refs will be extracted
                // from the text content when the text node is flushed.
                // However, we DO want to track the position for the text
                // node's var_refs list. So let's just advance and let
                // flush_text extract them.
                None
            }
            b'@' if i + 1 < len && (bytes[i + 1] == b'@' || bytes[i + 1] == b'.' || bytes[i + 1] == b'#' || is_ident_start(bytes[i + 1])) => {
                // @ or @@ — SugarCube inline styling
                // Double-at: @@class;text@@
                // Single-at: @class;text@ (class may start with . or # for CSS selectors)
                let is_double_at = bytes[i + 1] == b'@';
                let start = i;
                let adv = if is_double_at { 2 } else { 1 };
                i += adv;
                ctx.col += adv;
                let node = parse_inline_style(text, &mut i, ctx, start, is_double_at);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'{' if i + 2 < len && bytes[i + 1] == b'{' && bytes[i + 2] == b'{' => {
                // {{{ — code block (block form) or inline code (inline form).
                //
                // SugarCube disambiguates by position (plan.md §3.10, §AD-10):
                //   - Block code:  `{{{` at column 0 AND immediately followed by `\n`.
                //                  Content is raw, rendered as `<pre><code>…</code></pre>`.
                //                  Closing `}}}` must be alone on its own line at column 0.
                //   - Inline code: `{{{` anywhere else (mid-line, or at col 0 without `\n`).
                //                  Content is raw, rendered as `<code>…</code>`.
                //                  Closing `}}}` is the first one found (non-greedy).
                //
                // Both forms are RAW ZONES — macros, variables, and links inside
                // are NOT processed (SugarCube uses `.text()`, no `subWikify`).
                // This is the critical bug fix: previously `{{{ <<set $x to 1>> }}}`
                // would execute the `<<set>>` macro because there was no `b'{'`
                // arm and the `<<` fell through to the macro parser.
                let start = i;
                let at_col_zero = ctx.col == 0;
                i += 3;
                ctx.col += 3;
                let is_block = at_col_zero && i < len && bytes[i] == b'\n';
                let node = if is_block {
                    parse_code_block(text, &mut i, offset + start)
                } else {
                    parse_inline_code(text, &mut i, offset + start)
                };
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'!' if ctx.col == 0 => {
                // ! — heading markup: `!` through `!!!!!!` (1-6 levels).
                //
                // SugarCube's `heading` parser (plan.md §3.5):
                //   - Match: `^!{1,6}` — 1 to 6 exclamation marks at column 0.
                //   - NO leading whitespace allowed (column-0 anchored).
                //   - NO required space after the `!` run — `!Heading` and `! Heading`
                //     are both valid; the space becomes part of the heading text.
                //   - A 7th `!` matches as a 6-level heading; the 7th `!` becomes
                //     the first character of heading content.
                //   - Content = rest of line (up to `\n`), recursively parsed via
                //     `subWikify` — so macros, variables, and links INSIDE heading
                //     text ARE processed (not raw).
                //   - HTML output: `<h1>` through `<h6>`.
                //
                // The `ctx.col == 0` guard ensures column-0 anchoring. A `!`
                // mid-line falls through to the catch-all and becomes plain text.
                let start = i;
                let node = parse_heading(text, &mut i, ctx, start);
                // `parse_heading` advances `i` to the end of line (the `\n`
                // position or end of text). The main loop's `b'\n'` arm will
                // consume the `\n` and reset `ctx.col`. We resync `ctx.col`
                // here in case the heading was the last line (no `\n`).
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'-' if ctx.col == 0 && is_horizontal_rule_line(&text[i..]) => {
                // ---- — horizontal rule (4+ dashes alone on a line).
                //
                // SugarCube's `horizontalRule` parser (plan.md §3.6):
                //   - Match: `^----+\s*$` — 4 or more dashes at column 0, with
                //     only trailing whitespace allowed.
                //   - `---` (3 dashes) is NOT a horizontal rule — it renders as
                //     literal text (likely `—` + `-` via the `emdash` parser).
                //   - HTML output: `<hr>` (void element).
                //
                // The `ctx.col == 0` guard ensures column-0 anchoring. The
                // `is_horizontal_rule_line` helper checks the rest-of-line
                // pattern. A `-` that doesn't match the HR pattern (e.g. `--`
                // for emdash, or `- item` for a list — though lists aren't
                // supported in SugarCube) falls through to the catch-all.
                let start = i;
                let node = parse_horizontal_rule(text, &mut i, offset + start);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'>' if ctx.col == 0 => {
                // > — line-style blockquote: `>`, `>>`, `>>>`, etc.
                //
                // SugarCube's `quoteByLine` parser (plan.md §3.8.1):
                //   - Match: `^>+` — one or more `>` at column 0.
                //   - Depth = `>` count (1 = `>`, 2 = `>>`, etc.).
                //   - NO leading whitespace allowed.
                //   - NO required space after `>` — `>Text` and `> Text` both work.
                //   - Content = rest of line (up to `\n`), recursively parsed via
                //     `subWikify` — macros, variables, links ARE processed.
                //   - Multi-line: every line of a multi-line blockquote must
                //     begin with `>`. No "lazy continuation".
                //   - HTML output: nested `<blockquote>` elements + `<br>`.
                //
                // The `ctx.col == 0` guard ensures column-0 anchoring. A `>`
                // mid-line falls through to the catch-all (this is important —
                // SugarCube doesn't use `>` for anything else at mid-line).
                let start = i;
                let node = parse_blockquote_line(text, &mut i, ctx, start);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'*' if ctx.col == 0 => {
                // * — unordered list item: `*`, `**`, `***`, etc. at column 0.
                //
                // SugarCube's `list` parser (plan.md §3.7):
                //   - Match: `^(?:(?:\*+)|(?:#+))` — a run of all-`*` or all-`#`.
                //   - `*` = unordered list (ul), `#` = ordered list (ol).
                //   - Depth = marker character count (NOT indentation).
                //   - NO mixed markers (`*#` not supported — regex matches
                //     all-`*` or all-`#` only).
                //   - NO leading whitespace allowed.
                //   - NO required space after marker — `*item` and `* item`
                //     both work; the space becomes part of item text.
                //   - Content = rest of line (up to `\n`), recursively parsed
                //     via `subWikify` — macros, variables, links ARE processed.
                //   - HTML output: `<ul>`/`<ol>` wrapping `<li>`, nested by depth.
                //
                // The `ctx.col == 0` guard ensures column-0 anchoring. A `*`
                // mid-line falls through to the catch-all (SugarCube doesn't
                // use `*` for bold — it uses `''bold''`).
                let start = i;
                let node = parse_list_item(text, &mut i, ctx, start, false);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'#' if ctx.col == 0 => {
                // # — ordered list item: `#`, `##`, `###`, etc. at column 0.
                //
                // Same as `b'*'` above but for ordered lists (`<ol>`).
                // See plan.md §3.7 for full details.
                //
                // The `ctx.col == 0` guard ensures column-0 anchoring. A `#`
                // mid-line falls through to the catch-all (SugarCube doesn't
                // use `#` for Markdown-style ATX headings — it uses `!`).
                let start = i;
                let node = parse_list_item(text, &mut i, ctx, start, true);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'|' if ctx.col == 0 && is_table_row_line(&text[i..]) => {
                // | — TiddlyWiki-style table row (plan.md §3.9).
                //
                // SugarCube's `table` parser (undocumented but in source):
                //   - Each row is a line starting with `|`, ending with `|`
                //     optionally followed by a one-letter row-type suffix
                //     (`h`/`f`/`c`/`k`).
                //   - Cells are separated by `|`.
                //   - A cell beginning with `!` is a header cell (`<th>`).
                //   - A cell containing only `>` triggers colspan.
                //   - A cell containing only `~` triggers rowspan.
                //   - Cell content is recursively parsed (macros execute).
                //
                // The `is_table_row_line` guard checks the full row pattern
                // (starts with `|`, ends with `|` or `|` + suffix). A `|` that
                // doesn't match (e.g. `| not a table row`) falls through to
                // plain text.
                //
                // `parse_table` scans ALL consecutive table-row lines and
                // groups them into a single `Table` node.
                let start = i;
                let node = parse_table(text, &mut i, offset);
                resync_col_after_advance(text, start, i, ctx);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'\n' => {
                i += 1;
                ctx.col = 0;
                None
            }
            _ => {
                let adv = text[i..].chars().next().map_or(1, |c| c.len_utf8());
                i += adv;
                ctx.col += adv;
                None
            }
        };

        if let Some(node) = matched {
            nodes.push(node);
            // After a delimiter has been consumed (macro, link, comment),
            // the text gap must start at the current position (past the
            // consumed content), not at the delimiter's start position.
            // Without this, the final flush would include the delimiter
            // content as text, causing variables/links inside comments to
            // be incorrectly extracted.
            text_start = i;
        }
    }

    // Flush remaining text
    flush_text(text, &mut text_start, len, offset, &mut nodes);

    nodes
}

/// Resync `ctx.col` after a sub-parser has advanced `i` from `start` to `end`.
///
/// Sub-parsers (`parse_macro`, `parse_link`, `parse_comment*`, etc.) consume
/// an arbitrary number of bytes including potential newlines. Rather than
/// teach each sub-parser about `ctx.col`, we recompute the column by scanning
/// the consumed slice `text[start..end]` and counting bytes since the last
/// `\n`. This is O(n) per delimiter but delimiters are rare relative to text,
/// so the overhead is negligible.
///
/// The column after the consumed slice equals the byte distance from the
/// last `\n` in `text[start..end]` to `end`. If there is no `\n` in the
/// slice, we add the slice length to the existing `ctx.col` (we were
/// mid-line before the delimiter, and the delimiter didn't cross a newline).
fn resync_col_after_advance(text: &str, start: usize, end: usize, ctx: &mut ParseCtx) {
    if end <= start {
        return;
    }
    let slice = &text[start..end];
    if let Some(rel_nl) = slice.rfind('\n') {
        // Last newline in the slice is at `start + rel_nl`.
        // Column after the slice = byte distance from that newline to `end`,
        // i.e. `end - (start + rel_nl + 1)`.
        ctx.col = end - (start + rel_nl + 1);
    } else {
        // No newline in the slice — add slice byte length to current col.
        ctx.col += end - start;
    }
}

/// Flush accumulated text into a Text node.
///
/// `text_start` is updated to `end` after flushing.
fn flush_text(
    text: &str,
    text_start: &mut usize,
    end: usize,
    offset: usize,
    nodes: &mut Vec<AstNode>,
) {
    if *text_start >= end {
        return;
    }
    let content = text[*text_start..end].to_string();
    if content.is_empty() {
        return;
    }

    // Extract inline variable references from this text gap
    let var_refs = scan_inline_vars(&content, offset + *text_start);

    nodes.push(AstNode::Text {
        content,
        var_refs,
        span: offset + *text_start..offset + end,
        is_prose: true, // Default: top-level text is always prose.
        // The tree builder will set is_prose = false for Text nodes
        // inside non-rendering macros (<<silently>>, <<script>>, <<style>>).
    });
    *text_start = end;
}

/// Parse SugarCube inline styling markup (`@@class;text@@` or `@class;text@`).
///
/// `i` points to the first character after the opening `@@` or `@`.
/// `start` is the position of the first `@` in `text`.
/// `is_double_at` is `true` for `@@...@@`, `false` for `@...@`.
///
/// For double-at: the class is between `@@` and `;`, the body is between
/// `;` and `@@`. The close delimiter is `@@`.
///
/// For single-at: same structure but with single `@` delimiters.
///
/// The `ctx` parameter carries the body offset and the current column. After
/// parsing, `ctx.col` is resync'd to reflect the column position past the
/// closing delimiter, so the caller's main loop continues with the correct
/// line-start awareness.
fn parse_inline_style(
    text: &str,
    i: &mut usize,
    ctx: &mut ParseCtx,
    start: usize,
    is_double_at: bool,
) -> AstNode {
    let offset = ctx.offset;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Find the class name (up to ; or the close delimiter)
    let (class, class_span, body_start) = find_class_and_body_start(
        text, *i, offset, is_double_at,
    );

    // Find the closing delimiter
    let close_delim = if is_double_at { "@@" } else { "@" };
    let body_end = text[body_start - offset..]
        .find(close_delim)
        .map(|pos| body_start - offset + pos)
        .unwrap_or(len);

    let body_content = if body_start - offset < body_end {
        &text[body_start - offset..body_end]
    } else {
        ""
    };

    // Recursively parse the body content for variables, links, etc.
    //
    // The recursive `parse_body` call needs an initial column. We compute
    // it by counting bytes since the last `\n` in `text[..body_start - offset]`
    // — this gives the column at which the body content starts, so any
    // block-level construct at the very first column of the inline style
    // body would be recognized (though in practice inline styles rarely
    // span lines, and SugarCube's `@@...@@` is an inline construct).
    let children = if !body_content.is_empty() {
        let body_local_start = body_start - offset;
        let initial_col = compute_initial_col(text, body_local_start);
        let mut child_ctx = ParseCtx { offset: body_start, col: initial_col };
        parse_body_with_ctx(body_content, &mut child_ctx)
    } else {
        Vec::new()
    };

    // Advance past the closing delimiter
    *i = body_end + close_delim.len();

    // Resync the caller's column to match the new position past the closing
    // delimiter.
    resync_col_after_advance(text, start, *i, ctx);

    AstNode::InlineStyle {
        class,
        class_span,
        children,
        span: offset + start..offset + *i,
    }
}

/// Compute the initial column for a recursive `parse_body` call starting at
/// byte position `local_start` within `text`.
///
/// The column equals the byte distance from the last `\n` before `local_start`
/// to `local_start`. If `local_start` is 0 (start of text) or there is no
/// preceding `\n`, the column is `local_start` itself (counting from the
/// start of the text). This matches the column-tracking invariant maintained
/// by the main loop: `col` is bytes-since-last-newline.
fn compute_initial_col(text: &str, local_start: usize) -> usize {
    if local_start == 0 {
        return 0;
    }
    let prefix = &text[..local_start];
    match prefix.rfind('\n') {
        Some(nl_pos) => local_start - (nl_pos + 1),
        None => local_start,
    }
}

/// Parse a block code section: `{{{\n...\n}}}`.
///
/// `*i` is positioned just after the `{{{` (i.e., at the `\n` that follows).
/// `span_start` is the body-relative byte offset of the opening `{{{`.
///
/// SugarCube's `monospacedByBlock` parser requires:
///   - `{{{` immediately followed by `\n` at column 0 (already verified by caller).
///   - Closing `}}}` alone on its own line at column 0.
///
/// Content is RAW — no macro/variable/link processing. Rendered as
/// `<pre><code>…</code></pre>` (plan.md §3.10.1).
///
/// On return, `*i` points just past the closing `}}}` (and its trailing
/// newline, if any). If unclosed, consumes to end of text.
fn parse_code_block(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // *i is at the `\n` after `{{{`. Content starts after that newline.
    let content_start = *i + 1;

    // Scan for a line consisting of exactly `}}}` (optionally followed by
    // a newline or end-of-text). We walk line-by-line through the content.
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut close_offset = None;  // byte offset (relative to content_start) where the `}}}` line starts
    let mut search = content_start;
    while search < len {
        // Find the next `\n` from `search`.
        let nl_pos = match text[search..].find('\n') {
            Some(rel) => search + rel,
            None => break,  // no more newlines; no closing `}}}` line
        };
        // The line AFTER this newline starts at `nl_pos + 1`.
        let line_start = nl_pos + 1;
        if line_start + 3 <= len && &text[line_start..line_start + 3] == "}}}" {
            // Check that `}}}` is alone on its line: the next char must be
            // `\n`, `\r\n`, or end-of-text.
            let after = line_start + 3;
            let alone = after >= len
                || bytes[after] == b'\n'
                || (bytes[after] == b'\r' && after + 1 < len && bytes[after + 1] == b'\n');
            if alone {
                // `close_offset` is relative to `content_start`.
                close_offset = Some(line_start - content_start);
                break;
            }
        }
        search = line_start;
    }

    let (content, span_end) = if let Some(close_rel) = close_offset {
        let close_abs = content_start + close_rel;
        let content = text[content_start..close_abs].to_string();
        // Skip past `}}}` (3 bytes) and an optional trailing `\n`.
        let mut end = close_abs + 3;
        if end < len && bytes[end] == b'\n' {
            end += 1;
        } else if end + 1 < len && bytes[end] == b'\r' && bytes[end + 1] == b'\n' {
            end += 2;
        }
        (content, end)
    } else {
        // Unclosed — consume rest of text.
        let content = text[content_start..].to_string();
        (content, len)
    };

    *i = span_end;
    AstNode::CodeBlock {
        content,
        span: span_start..span_start + (span_end - span_start),
    }
}

/// Parse inline code: `{{{...}}}` (non-greedy, first `}}}` closes).
///
/// `*i` is positioned just after the `{{{`. `span_start` is the body-relative
/// byte offset of the opening `{{{`.
///
/// SugarCube's `formatByChar` `{{{` case uses a non-greedy regex
/// `/\{\{\{((?:.|\n)*?)\}\}\}/gm` — the first `}}}` closes the construct.
/// Content is RAW (no macro/variable/link processing), rendered as
/// `<code>…</code>` (plan.md §3.10.2).
///
/// On return, `*i` points just past the closing `}}}`. If unclosed,
/// consumes to end of text.
fn parse_inline_code(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // *i is just after `{{{`.
    let content_start = *i;

    // Non-greedy scan for the first `}}}`.
    let close_offset = text[content_start..].find("}}}");

    let (content, span_end) = if let Some(close_rel) = close_offset {
        let close_abs = content_start + close_rel;
        let content = text[content_start..close_abs].to_string();
        let end = close_abs + 3;  // skip past `}}}`
        (content, end)
    } else {
        // Unclosed — consume rest of text.
        let content = text[content_start..].to_string();
        (content, text.len())
    };

    *i = span_end;
    AstNode::InlineCode {
        content,
        span: span_start..span_start + (span_end - span_start),
    }
}

/// Parse a heading: `!` through `!!!!!!` (1-6 levels) at column 0.
///
/// `*i` is positioned at the first `!`. `span_start` is the body-relative
/// byte offset of the first `!` (passed via `ctx.offset + start` by caller).
///
/// SugarCube's `heading` parser (plan.md §3.5):
///   - Scans 1-6 `!` characters. A 7th `!` is NOT consumed as part of the
///     marker — it becomes the first character of heading content.
///   - Content = rest of line (up to `\n` or end of text).
///   - Content is recursively parsed via `subWikify` — macros, variables,
///     and links INSIDE heading text ARE processed (heading is a "container",
///     not a "raw zone").
///   - HTML output: `<h1>` through `<h6>`.
///
/// On return:
///   - `*i` points to the `\n` that terminates the heading line (or to
///     `text.len()` if the heading is the last line with no trailing `\n`).
///     The main loop's `b'\n'` arm will consume the `\n` and reset `ctx.col`.
///   - The returned `AstNode::Heading` span covers `!` run through end of
///     line (exclusive of `\n`).
fn parse_heading(text: &str, i: &mut usize, ctx: &mut ParseCtx, start: usize) -> AstNode {
    let offset = ctx.offset;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Count `!` characters — up to 6 (the max heading level).
    // A 7th `!` is left for the content.
    let mut level: u8 = 0;
    while *i < len && bytes[*i] == b'!' && level < 6 {
        *i += 1;
        level += 1;
    }
    // `level` is now 1..=6. `*i` points just past the last consumed `!`.

    // Find end of line (next `\n` or end of text).
    let content_start = *i;
    let end_of_line = text[content_start..].find('\n')
        .map(|pos| content_start + pos)
        .unwrap_or(len);

    // Extract the content substring (after `!` run, up to end of line).
    let content = &text[content_start..end_of_line];

    // Recursively parse the heading content for macros, variables, links, etc.
    //
    // The recursive `parse_body_with_ctx` call needs:
    //   - `offset`: the body-relative offset where content starts
    //     (= `offset + content_start`).
    //   - `col`: the column at which content starts in the original text.
    //     Since the `!` run consumed `level` bytes at column 0, the content
    //     starts at column `level`.
    let children = if !content.is_empty() {
        let mut child_ctx = ParseCtx {
            offset: offset + content_start,
            col: level as usize,
        };
        parse_body_with_ctx(content, &mut child_ctx)
    } else {
        Vec::new()
    };

    // Advance `*i` to the end of line (the `\n` position or end of text).
    // The main loop will consume the `\n` via the `b'\n'` arm.
    *i = end_of_line;

    // Span covers `!` run through end of line (exclusive of `\n`).
    AstNode::Heading {
        level,
        children,
        span: offset + start..offset + end_of_line,
    }
}

/// Check if the rest of a line (starting at `text`) matches the horizontal
/// rule pattern: 4+ dashes followed by optional whitespace and end-of-line.
///
/// SugarCube's `horizontalRule` parser matches `^----+\s*$` (plan.md §3.6).
/// This helper checks the equivalent from the current position:
///   - 4 or more `-` characters, then
///   - zero or more spaces/tabs, then
///   - `\n`, `\r\n`, or end-of-string.
///
/// Returns `false` for `---` (3 dashes), `--` (emdash), or `---- text`
/// (trailing non-whitespace). The caller has already verified `ctx.col == 0`.
fn is_horizontal_rule_line(text: &str) -> bool {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    // Count dashes — need at least 4.
    while i < len && bytes[i] == b'-' {
        i += 1;
    }
    if i < 4 {
        return false;
    }
    // Skip trailing whitespace (spaces/tabs only — SugarCube's `\s` in this
    // context means horizontal whitespace, since we're on a single line).
    while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // Must be at end-of-line or end-of-text.
    i == len || bytes[i] == b'\n' || (bytes[i] == b'\r' && i + 1 < len && bytes[i + 1] == b'\n')
}

/// Parse a horizontal rule: `----` (4+ dashes alone on a line at column 0).
///
/// `*i` is positioned at the first `-`. `span_start` is the body-relative
/// byte offset of the first `-`.
///
/// SugarCube's `horizontalRule` parser (plan.md §3.6):
///   - 4+ dashes, then optional trailing whitespace, then end-of-line.
///   - HTML output: `<hr>` (void element, no body).
///
/// On return, `*i` points to the `\n` that terminates the HR line (or to
/// `text.len()` if the HR is the last line with no trailing `\n`). The main
/// loop's `b'\n'` arm will consume the `\n` and reset `ctx.col`.
///
/// The span covers the dash run only (NOT trailing whitespace, NOT the `\n`).
/// This matches the SugarCube source — the `hr` element has no content.
fn parse_horizontal_rule(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let dash_start = *i;
    // Consume the dash run.
    while *i < len && bytes[*i] == b'-' {
        *i += 1;
    }
    let dash_end = *i;
    // Skip trailing whitespace (NOT part of the span — matches SugarCube's
    // behavior where the `<hr>` element has no content).
    while *i < len && (bytes[*i] == b' ' || bytes[*i] == b'\t') {
        *i += 1;
    }
    // Don't consume the `\n` — let the main loop handle it.
    // (If we're at end of text, *i == len, which is fine.)

    AstNode::HorizontalRule {
        span: span_start..span_start + (dash_end - dash_start),
    }
}

/// Parse a line-style blockquote: `>`, `>>`, `>>>`, etc. at column 0.
///
/// `*i` is positioned at the first `>`. `ctx` carries the body offset and
/// current column. `start` is the body-relative byte offset of the first `>`.
///
/// SugarCube's `quoteByLine` parser (plan.md §3.8.1):
///   - Scans 1+ `>` characters. Depth = `>` count.
///   - Content = rest of line (up to `\n`), recursively parsed via
///     `subWikify` — macros, variables, links ARE processed.
///   - NO required space after `>` — `>Text` and `> Text` both work.
///   - Multi-line: every line of a multi-line blockquote must begin with `>`.
///     Each `>` line is a SEPARATE `Blockquote` node — the caller (graph
///     builder / renderer) reconstructs nesting from depth.
///
/// On return, `*i` points to the `\n` (or end of text). The main loop's
/// `b'\n'` arm will consume the `\n`. Span covers `>` run through end of line
/// (exclusive of `\n`).
fn parse_blockquote_line(text: &str, i: &mut usize, ctx: &mut ParseCtx, start: usize) -> AstNode {
    let offset = ctx.offset;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Count `>` characters — depth = count (no hardcoded limit).
    let mut depth: u8 = 0;
    while *i < len && bytes[*i] == b'>' {
        *i += 1;
        depth += 1;
        // Safety: depth is u8, but SugarCube has no limit. If someone writes
        // 256+ `>`s, we cap at 255 (still renders as deeply nested blockquotes).
        // This is extremely unlikely in practice.
        if depth == 255 {
            break;
        }
    }

    // Find end of line (next `\n` or end of text).
    let content_start = *i;
    let end_of_line = text[content_start..].find('\n')
        .map(|pos| content_start + pos)
        .unwrap_or(len);

    // Extract the content substring (after `>` run, up to end of line).
    let content = &text[content_start..end_of_line];

    // Recursively parse the blockquote line content.
    //
    // The recursive `parse_body_with_ctx` call needs:
    //   - `offset`: `offset + content_start`
    //   - `col`: `depth` (content starts at column `depth` in the original text)
    let children = if !content.is_empty() {
        let mut child_ctx = ParseCtx {
            offset: offset + content_start,
            col: depth as usize,
        };
        parse_body_with_ctx(content, &mut child_ctx)
    } else {
        Vec::new()
    };

    // Advance `*i` to end of line (NOT past `\n`).
    *i = end_of_line;

    AstNode::Blockquote {
        depth,
        children,
        span: offset + start..offset + end_of_line,
    }
}

/// Parse a list item: `*`/`**`/`#`/`##` etc. at column 0.
///
/// `*i` is positioned at the first marker character. `ctx` carries the body
/// offset and current column. `start` is the body-relative byte offset of the
/// first marker character. `ordered` is `false` for `*` (unordered) and
/// `true` for `#` (ordered).
///
/// SugarCube's `list` parser (plan.md §3.7):
///   - Scans a run of identical marker characters (`*+` or `#+`).
///   - Depth = marker character count.
///   - NO mixed markers (`*#` not supported — only the first `*` would match,
///     the `#` becomes literal text).
///   - Content = rest of line (up to `\n`), recursively parsed via `subWikify`
///     — macros, variables, links ARE processed.
///   - NO required space after marker — `*item` and `* item` both work.
///   - HTML output: `<ul>`/`<ol>` wrapping `<li>`, nested by depth.
///
/// On return, `*i` points to the `\n` (or end of text). The main loop's
/// `b'\n'` arm will consume the `\n`. Span covers marker run through end of
/// line (exclusive of `\n`).
fn parse_list_item(
    text: &str,
    i: &mut usize,
    ctx: &mut ParseCtx,
    start: usize,
    ordered: bool,
) -> AstNode {
    let offset = ctx.offset;
    let bytes = text.as_bytes();
    let len = bytes.len();
    let marker_char = if ordered { b'#' } else { b'*' };

    // Count marker characters — depth = count (no hardcoded limit).
    // SugarCube's regex `^(?:(\*+)|(#+))` matches a run of all-`*` or all-`#`.
    // We scan until a non-marker character is found. A mixed marker like `*#`
    // stops at the first `#` (the `#` becomes content).
    let marker_start = *i;
    while *i < len && bytes[*i] == marker_char {
        *i += 1;
    }
    let marker_end = *i;
    let marker = text[marker_start..marker_end].to_string();
    let depth: u8 = marker.len() as u8; // capped at 255 (u8 max)

    // Find end of line (next `\n` or end of text).
    let content_start = *i;
    let end_of_line = text[content_start..].find('\n')
        .map(|pos| content_start + pos)
        .unwrap_or(len);

    // Extract the content substring (after marker run, up to end of line).
    let content = &text[content_start..end_of_line];

    // Recursively parse the list item content.
    //
    // The recursive `parse_body_with_ctx` call needs:
    //   - `offset`: `offset + content_start`
    //   - `col`: `depth` (content starts at column `depth` in the original text)
    let children = if !content.is_empty() {
        let mut child_ctx = ParseCtx {
            offset: offset + content_start,
            col: depth as usize,
        };
        parse_body_with_ctx(content, &mut child_ctx)
    } else {
        Vec::new()
    };

    // Advance `*i` to end of line (NOT past `\n`).
    *i = end_of_line;

    AstNode::ListItem {
        depth,
        ordered,
        marker,
        children,
        span: offset + start..offset + end_of_line,
    }
}

// ---------------------------------------------------------------------------
// Table parsing (Phase 6, plan.md §3.9)
// ---------------------------------------------------------------------------

/// Check if the line starting at `text` is a valid TiddlyWiki table row.
///
/// A valid table row:
///   - Starts with `|` at column 0 (caller already verified `ctx.col == 0`).
///   - Ends with `|` OR `|` followed by a single suffix letter (`h`/`f`/`c`/`k`).
///   - Has at least 2 characters (minimum `||` for an empty row).
///
/// Returns `false` for `| not a row` (no closing `|`), `|` alone (too short),
/// or `|cell|x` (suffix `x` is not `h`/`f`/`c`/`k`).
fn is_table_row_line(text: &str) -> bool {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len < 2 || bytes[0] != b'|' {
        return false;
    }
    // Find end of line (exclusive of `\n`).
    let line_end = text.find('\n').unwrap_or(len);
    let line_bytes = &bytes[..line_end];
    let line_len = line_bytes.len();
    if line_len < 2 {
        return false;
    }
    // Case 1: line ends with `|` (no suffix).
    if line_bytes[line_len - 1] == b'|' {
        return true;
    }
    // Case 2: line ends with `|` + single suffix letter [fhck].
    if line_len >= 3 && line_bytes[line_len - 2] == b'|' {
        let suffix = line_bytes[line_len - 1];
        return matches!(suffix, b'h' | b'f' | b'c' | b'k');
    }
    false
}

/// Determine the row type and closing-`|` position from a table row line.
///
/// `line` is the row text WITHOUT the trailing `\n`. Returns `(row_type, closing_pipe_pos)`
/// where `closing_pipe_pos` is the byte index of the closing `|` within `line`.
///
/// - `|cell|cell|` → `(Body, line.len() - 1)`
/// - `|cell|cell|h` → `(Header, line.len() - 2)`
/// - `|cell|cell|f` → `(Footer, line.len() - 2)`
/// - `|cell|cell|c` → `(Caption, line.len() - 2)`
/// - `|cell|cell|k` → `(Class, line.len() - 2)`
fn parse_table_row_suffix(line: &str) -> (TableRowType, usize) {
    let bytes = line.as_bytes();
    let len = bytes.len();
    // Caller (is_table_row_line) already verified the pattern.
    if len >= 2 && bytes[len - 1] == b'|' {
        return (TableRowType::Body, len - 1);
    }
    // Suffix case: `|` + single letter.
    if len >= 3 && bytes[len - 2] == b'|' {
        let suffix = bytes[len - 1];
        let row_type = match suffix {
            b'h' => TableRowType::Header,
            b'f' => TableRowType::Footer,
            b'c' => TableRowType::Caption,
            b'k' => TableRowType::Class,
            _ => TableRowType::Body, // shouldn't happen (guard checked)
        };
        return (row_type, len - 2);
    }
    (TableRowType::Body, len.saturating_sub(1)) // fallback
}

/// Parse a TiddlyWiki table: consecutive `|...|` lines at column 0.
///
/// `*i` is positioned at the first `|` of the first row. `offset` is the
/// body-relative offset (0 for top-level, nonzero for nested content inside
/// block macros or inline styles). Body-relative positions are computed as
/// `offset + text_relative_pos`.
///
/// Scans forward line-by-line. Each line must match `is_table_row_line`.
/// Stops when a non-table-row line is found (or end of text). Groups all rows
/// into a single `AstNode::Table` node.
///
/// Row classification:
///   - `h` suffix → `header` (first `h` row) + stored in `rows`.
///   - `f` suffix → `footer` (first `f` row) + stored in `rows`.
///   - `c` suffix → `caption` (cell content extracted as caption string).
///   - `k` suffix → `class` (cell content extracted as class string).
///   - no suffix → `rows` (body row).
///
/// ALL rows (including `h`/`f`) are stored in `rows` in document order, with
/// their `row_type` set correctly. `header`/`footer` are additional references
/// to the first `h`/`f` row for consumer convenience.
fn parse_table(text: &str, i: &mut usize, offset: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();

    let mut all_rows: Vec<TableRow> = Vec::new();
    let mut header: Option<TableRow> = None;
    let mut footer: Option<TableRow> = None;
    let mut caption: Option<String> = None;
    let mut caption_span: Option<Range<usize>> = None;
    let mut class: Option<String> = None;
    let mut class_span: Option<Range<usize>> = None;

    while *i < len {
        // Check if current line is a table row.
        if bytes[*i] != b'|' || !is_table_row_line(&text[*i..]) {
            break;
        }

        // Find end of line.
        let line_start = *i;
        let line_end = text[*i..].find('\n')
            .map(|pos| *i + pos)
            .unwrap_or(len);
        let line = &text[line_start..line_end];

        // Determine row type and closing `|` position (relative to line start).
        let (row_type, closing_pipe_local) = parse_table_row_suffix(line);
        let closing_pipe_abs = line_start + closing_pipe_local;

        // Cell text is between opening `|` and closing `|`.
        let cells_start = line_start + 1; // past opening `|`
        let cells_end = closing_pipe_abs; // at closing `|`
        let cells_text = &text[cells_start..cells_end];

        // Parse cells (split by `|`, recursive content).
        let cells = parse_table_cells(cells_text, cells_start, offset);

        // Body-relative span of this row line.
        let row_span = offset + line_start..offset + line_end;

        match row_type {
            TableRowType::Caption => {
                // Caption row: extract cell content as caption string.
                let caption_text: String = cells.iter()
                    .flat_map(|c| c.children.iter().filter_map(|n| {
                        if let AstNode::Text { content, .. } = n { Some(content.as_str()) } else { None }
                    }))
                    .collect::<Vec<_>>()
                    .join("");
                caption = Some(caption_text);
                caption_span = Some(row_span.clone());
            }
            TableRowType::Class => {
                // Class row: extract cell content as class string.
                let class_text: String = cells.iter()
                    .flat_map(|c| c.children.iter().filter_map(|n| {
                        if let AstNode::Text { content, .. } = n { Some(content.as_str()) } else { None }
                    }))
                    .collect::<Vec<_>>()
                    .join("");
                class = Some(class_text);
                class_span = Some(row_span.clone());
            }
            TableRowType::Header => {
                let row = TableRow { cells, row_type, span: row_span };
                if header.is_none() {
                    header = Some(row.clone());
                }
                all_rows.push(row);
            }
            TableRowType::Footer => {
                let row = TableRow { cells, row_type, span: row_span };
                if footer.is_none() {
                    footer = Some(row.clone());
                }
                all_rows.push(row);
            }
            TableRowType::Body => {
                all_rows.push(TableRow { cells, row_type, span: row_span });
            }
        }

        // Advance to end of line.
        *i = line_end;
        // If there's a `\n`, consume it to check the next line.
        if *i < len && bytes[*i] == b'\n' {
            *i += 1;
        } else {
            // End of text — no more lines.
            break;
        }
    }

    // Compute the table span: from the first `|` to the last consumed position.
    // If no rows were parsed (shouldn't happen — guard checked), span is empty.
    let table_start = offset + (all_rows.first()
        .map(|r| r.span.start - offset)
        .unwrap_or(0));
    let table_end = offset + (*i);

    AstNode::Table {
        header,
        rows: all_rows,
        footer,
        caption,
        caption_span,
        class,
        class_span,
        span: table_start..table_end,
    }
}

/// Parse table cells from the text between the opening `|` and closing `|`.
///
/// `cells_text` is the raw text between `|` delimiters (e.g. `"cell1|cell2"`).
/// `cells_start` is the text-relative byte offset where `cells_text` starts.
/// `offset` is the body-relative offset (for computing cell spans as `offset + text_pos`).
///
/// Cells are split by `|`. Each cell is classified:
///   - Content starting with `!` → header cell (`is_header = true`, `!` stripped).
///   - Content that is just `>` (trimmed) → colspan cell.
///   - Content that is just `~` (trimmed) → rowspan cell.
///   - Otherwise → normal cell.
///
/// Cell content is recursively parsed via `parse_body_with_ctx`.
fn parse_table_cells(cells_text: &str, cells_start: usize, offset: usize) -> Vec<TableCell> {
    let mut cells = Vec::new();
    let bytes = cells_text.as_bytes();
    let len = bytes.len();
    let mut cell_start = 0usize;
    let mut j = 0usize;

    while j <= len {
        if j == len || bytes[j] == b'|' {
            let cell_text = &cells_text[cell_start..j];
            let cell_text_start = cells_start + cell_start;
            let cell_text_end = cells_start + j;

            // Classify cell.
            let trimmed = cell_text.trim();
            let (is_header, colspan, rowspan) = if trimmed == ">" {
                (false, true, false)
            } else if trimmed == "~" {
                (false, false, true)
            } else if cell_text.starts_with('!') {
                (true, false, false)
            } else {
                (false, false, false)
            };

            // For header cells, strip the leading `!` from the content.
            let (content_text, content_start) = if is_header {
                (&cell_text[1..], cell_text_start + 1)
            } else {
                (cell_text, cell_text_start)
            };

            // Recursively parse the cell content.
            let children = if !content_text.is_empty() {
                let mut child_ctx = ParseCtx {
                    offset: offset + content_start,
                    col: 0,
                };
                parse_body_with_ctx(content_text, &mut child_ctx)
            } else {
                Vec::new()
            };

            cells.push(TableCell {
                children,
                is_header,
                colspan,
                rowspan,
                span: offset + cell_text_start..offset + cell_text_end,
            });

            cell_start = j + 1;
        }
        if j < len {
            j += cells_text[j..].chars().next().map_or(1, |c| c.len_utf8());
        } else {
            break;
        }
    }

    cells
}

/// Parse a block-style blockquote: `<<<\n...\n<<<` (undocumented but in source).
///
/// `*i` is positioned at the first `<` of the opening `<<<`. `ctx` carries
/// the body offset and current column (which must be 0 — verified by caller).
/// `start` is the body-relative byte offset of the opening `<<<`.
///
/// SugarCube's `quoteByBlock` parser (plan.md §3.8.2):
///   - Opening: a line consisting of exactly `<<<` (followed by `\n`).
///   - Closing: another `<<<` line.
///   - Content: everything between (may span multiple paragraphs), recursively
///     parsed via `subWikify` — macros, variables, links ARE processed.
///   - HTML output: a single `<blockquote>` wrapping all content.
///
/// On return, `*i` points just past the closing `<<<` and its trailing `\n`
/// (if any). If unclosed, consumes to end of text.
///
/// Span covers the opening `<<<` through the end of the closing `<<<` line
/// (inclusive of the closing `\n` if present).
fn parse_blockquote_block(text: &str, i: &mut usize, ctx: &mut ParseCtx, start: usize) -> AstNode {
    let offset = ctx.offset;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Consume the opening `<<<` (3 bytes). The caller verified `<<<` is
    // followed by `\n` or end-of-text, so after this `*i` points at the `\n`.
    *i += 3;

    // Content starts after the `\n` following the opening `<<<`.
    // If `*i == len` (opening `<<<` at end of text with no `\n`), content is empty.
    let content_start = if *i < len && bytes[*i] == b'\n' {
        *i + 1
    } else {
        *i // no newline — content is empty (degenerate case)
    };

    // Scan line-by-line for the closing `<<<` (a line consisting of exactly
    // `<<<` followed by `\n` or end-of-text, at column 0).
    let mut close_line_start: Option<usize> = None;
    let mut search = content_start;
    while search < len {
        // Find the next `\n` from `search`.
        let nl_pos = match text[search..].find('\n') {
            Some(rel) => search + rel,
            None => break, // no more newlines; no closing `<<<` line
        };
        let line_start = nl_pos + 1;
        // Check if this line is exactly `<<<` (followed by `\n` or end-of-text).
        if line_start + 3 <= len && &text[line_start..line_start + 3] == "<<<" {
            let after = line_start + 3;
            let alone = after >= len
                || bytes[after] == b'\n'
                || (bytes[after] == b'\r' && after + 1 < len && bytes[after + 1] == b'\n');
            if alone {
                close_line_start = Some(line_start);
                break;
            }
        }
        search = line_start;
    }

    let (children, span_end, close_span) = if let Some(close_start) = close_line_start {
        // Content is between `content_start` and `close_start`.
        let content = &text[content_start..close_start];
        let children = if !content.is_empty() {
            // Compute the initial column for the recursive parse. Content
            // starts at column 0 (it's on its own line after the opening `<<<\n`).
            let mut child_ctx = ParseCtx {
                offset: offset + content_start,
                col: 0,
            };
            parse_body_with_ctx(content, &mut child_ctx)
        } else {
            Vec::new()
        };
        // Closing `<<<` span is `close_start..close_start + 3`.
        // Span end: past the closing `<<<` and its trailing `\n` (if any).
        let mut end = close_start + 3;
        if end < len && bytes[end] == b'\n' {
            end += 1;
        } else if end + 1 < len && bytes[end] == b'\r' && bytes[end + 1] == b'\n' {
            end += 2;
        }
        (children, end, Some(offset + close_start..offset + close_start + 3))
    } else {
        // Unclosed — consume rest of text as content.
        let content = &text[content_start..];
        let children = if !content.is_empty() {
            let mut child_ctx = ParseCtx {
                offset: offset + content_start,
                col: 0,
            };
            parse_body_with_ctx(content, &mut child_ctx)
        } else {
            Vec::new()
        };
        (children, len, None)
    };

    *i = span_end;

    AstNode::BlockquoteBlock {
        children,
        open_span: offset + start..offset + start + 3,
        close_span,
        span: offset + start..offset + span_end,
    }
}

/// Find the class name and body start position in an inline style construct.
///
/// Returns `(class, class_span, body_start)` where `body_start` is the
/// passage-body-relative byte offset where the body content begins.
fn find_class_and_body_start(
    text: &str,
    content_start: usize,
    offset: usize,
    is_double_at: bool,
) -> (String, std::ops::Range<usize>, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Find ; separating class from body, or the close delimiter if no ;
    let mut j = content_start;
    while j < len {
        if bytes[j] == b';' {
            // Found the class/body separator
            let class = text[content_start..j].to_string();
            let class_span = offset + content_start..offset + j;
            return (class, class_span, offset + j + 1); // body starts after ;
        }
        // Check for close delimiter
        if is_double_at && j + 1 < len && bytes[j] == b'@' && bytes[j + 1] == b'@' {
            break;
        }
        if !is_double_at && bytes[j] == b'@' {
            break;
        }
        j += text[j..].chars().next().map_or(1, |c| c.len_utf8());
    }

    // No ; found — the entire content is the class, no body
    let class = text[content_start..j].to_string();
    let class_span = offset + content_start..offset + j;
    (class, class_span, offset + j)
}

#[cfg(test)]
mod tests {
    use crate::sugarcube::ast::{AstNode, CommentKind, ParseMode, LinkSource, TextFormatKind};

    #[test]
    fn line_comment_no_space_after_slashes_is_recognized() {
        // //comment (no space after //) should be recognized as a comment,
        // not treated as prose text. This was the user's bug report:
        // `<<link "x" "y">>  //content1` was getting a Prose token instead
        // of a Comment token because the old heuristic required a space
        // AFTER // (followed_by_space).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "//content1", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        assert!(matches!(&ast.nodes[0], AstNode::Comment { kind: CommentKind::JsLine, .. }),
            "//content1 should be a Comment node, got {:?}", ast.nodes[0]);
    }

    #[test]
    fn line_comment_after_macro_no_space_is_recognized() {
        // <<link "x" "y">>  //content1 — the //content1 has no space after //
        // but IS preceded by spaces (after >>). Should be a Comment.
        // Note: <<link>> is a Required-body macro, so it goes on the stack
        // and the comment becomes its child (not a top-level node).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<link \"x\" \"y\">>  //content1", 0, ParseMode::Normal,
        );

        fn has_comment_recursive(nodes: &[AstNode]) -> bool {
            for n in nodes {
                if matches!(n, AstNode::Comment { kind: CommentKind::JsLine, .. }) {
                    return true;
                }
                if let AstNode::Macro { children: Some(ch), .. } = n {
                    if has_comment_recursive(ch) {
                        return true;
                    }
                }
            }
            false
        }
        assert!(has_comment_recursive(&ast.nodes),
            "should have a Comment node for //content1 somewhere in the tree");
    }

    #[test]
    fn line_comment_inside_block_no_space_is_recognized() {
        // The user's exact scenario: //content2 and //content3 inside
        // a <<link>> block, with no space after //.
        let input = "<<link \"Chat\" \"Coworker\">>  //content1\n  <<if true>>  //content2\n    <<adjustStat \"stress\" -3>>  //content3\n    <<addTime 10>>\n  <</if>>\n<</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);

        // Collect all Comment nodes by walking the tree
        fn count_comments(nodes: &[AstNode]) -> usize {
            let mut count = 0;
            for n in nodes {
                if matches!(n, AstNode::Comment { .. }) {
                    count += 1;
                }
                if let AstNode::Macro { children: Some(ch), .. } = n {
                    count += count_comments(ch);
                }
            }
            count
        }
        let comment_count = count_comments(&ast.nodes);
        assert_eq!(comment_count, 3,
            "should have 3 Comment nodes (//content1, //content2, //content3), got {}", comment_count);
    }

    #[test]
    fn trailing_text_after_inline_macro_not_swallowed() {
        // Regression: inline macros like <<set>> were pushed onto the tree
        // builder's stack, swallowing trailing text/comments into
        // pending_children and dropping them when finalized as inline.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<set $x to 1>> some narrative text", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 2, "should have Macro + Text nodes");
        assert!(matches!(&ast.nodes[0], AstNode::Macro { name, .. } if name == "set"));
        match &ast.nodes[1] {
            AstNode::Text { content, .. } => assert!(content.contains("narrative text")),
            other => panic!("expected Text node, got {:?}", other),
        }
    }

    #[test]
    fn parse_simple_text() {
        let ast = crate::sugarcube::parser::parse_passage_body("Hello world", 0, ParseMode::Normal);
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Text { content, .. } => assert_eq!(content, "Hello world"),
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_variable_in_text() {
        let ast = crate::sugarcube::parser::parse_passage_body("You have $gold coins.", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert_eq!(ast.var_ops[0].name, "$gold");
        assert!(!ast.var_ops[0].is_write);
    }

    #[test]
    fn parse_temp_variable() {
        let ast = crate::sugarcube::parser::parse_passage_body("<<set _i to 0>>", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert!(ast.var_ops.iter().any(|v| v.name == "_i" && v.is_temporary && v.is_write));
    }

    #[test]
    fn escaped_dollar() {
        let ast = crate::sugarcube::parser::parse_passage_body("$$notavar", 0, ParseMode::Normal);
        // $$ should not be treated as a variable reference
        match &ast.nodes[0] {
            AstNode::Text { content, var_refs, .. } => {
                assert!(content.contains("$$notavar"));
                assert!(var_refs.is_empty());
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn stylesheet_mode_empty() {
        let ast = crate::sugarcube::parser::parse_passage_body("body { color: red; }", 0, ParseMode::Stylesheet);
        assert!(ast.nodes.is_empty());
    }

    #[test]
    fn script_mode_empty() {
        let ast = crate::sugarcube::parser::parse_passage_body("var x = 5;", 0, ParseMode::Script);
        assert!(ast.nodes.is_empty());
    }

    #[test]
    fn graph_connections_from_ast() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"[[Forest]] <<goto "Cave">> <<include "Shop">>"#,
            0,
            ParseMode::Normal,
        );
        let connections = ast.graph_connections();
        assert!(connections.iter().any(|c| c.target == "Forest" && c.edge_type == knot_core::graph::EdgeType::Navigation));
        assert!(connections.iter().any(|c| c.target == "Cave" && c.edge_type == knot_core::graph::EdgeType::Navigation));
        assert!(connections.iter().any(|c| c.target == "Shop" && c.edge_type == knot_core::graph::EdgeType::Include));
    }

    #[test]
    fn interface_mode_extracts_data_passage() {
        let html = r#"<div id="story"><div data-passage="Sidebar"></div></div>"#;
        let ast = crate::sugarcube::parser::parse_passage_body(html, 0, ParseMode::Interface);
        let dp_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::DataPassage).collect();
        assert_eq!(dp_links.len(), 1);
        assert_eq!(dp_links[0].target, "Sidebar");
    }

    #[test]
    fn data_passage_extraction() {
        let html = r#"<div data-passage="SidebarStats"></div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "SidebarStats");
        assert_eq!(links[0].source, LinkSource::DataPassage);
    }

    #[test]
    fn data_passage_single_quotes() {
        let html = "<div data-passage='MyPassage'></div>";
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "MyPassage");
    }

    #[test]
    fn data_passage_multiple() {
        let html = r#"<div data-passage="P1"></div><div data-passage="P2"></div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "P1");
        assert_eq!(links[1].target, "P2");
    }

    #[test]
    fn data_passage_ignores_comments() {
        let html = r#"<div data-passage="RealTarget">/* "FakeTarget" */</div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "RealTarget");
    }

    // ── Prose context tests ──────────────────────────────────────────────

    #[test]
    fn prose_top_level_text_is_prose() {
        // Top-level text in a passage body is always prose
        let ast = crate::sugarcube::parser::parse_passage_body("Hello world", 0, ParseMode::Normal);
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Text { content, is_prose, .. } => {
                assert_eq!(content, "Hello world");
                assert!(*is_prose, "top-level text should be prose");
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn prose_inside_if_is_prose() {
        // Text inside <<if>> body is prose — it renders to the player.
        // Using a simple <<if>> without <<else>> to avoid the known tree builder
        // issue where <<else>> (a BodyRequirement::Never inline clause marker)
        // consumes subsequent text as pending_children that get discarded.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<if true>>go to town<</if>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "if");
                match &children[0] {
                    AstNode::Text { content, is_prose, .. } => {
                        assert!(content.contains("go to town"));
                        assert!(*is_prose, "text inside <<if>> should be prose");
                    }
                    _ => panic!("Expected Text node inside <<if>>"),
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_silently_is_not_prose() {
        // Text inside <<silently>> is NOT prose — it's executed but not rendered
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<silently>>some text<</silently>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "silently");
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<silently>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_script_is_not_prose() {
        // <<script>> body is not prose — it's code
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<script>>var x = 1;<</script>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "script");
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<script>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_mixed_context() {
        // Text both inside and outside <<silently>>
        let ast = crate::sugarcube::parser::parse_passage_body(
            "visible text<<silently>>hidden code<</silently>>more visible",
            0,
            ParseMode::Normal,
        );
        // The tree builder nests children into <<silently>>, so:
        //   [Text("visible text"), Macro("silently", children=[...]), Text("more visible")]
        // But "more visible" might end up inside the silently macro's children
        // if the tree builder picks it up before the close tag. Let's check
        // the actual structure flexibly.
        let top_text_nodes: Vec<_> = ast.nodes.iter()
            .filter_map(|n| match n {
                AstNode::Text { content, is_prose, .. } => Some((content.clone(), *is_prose)),
                _ => None,
            })
            .collect();

        // "visible text" should be prose
        let visible = top_text_nodes.iter().find(|(c, _)| c.contains("visible text"));
        assert!(visible.is_some(), "should find 'visible text' as top-level node");
        assert!(visible.unwrap().1, "top-level text before <<silently>> should be prose");

        // Find the silently macro and verify its children are NOT prose
        let silently = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, children, .. } if name == "silently" => children.clone(),
            _ => None,
        });
        if let Some(silently_children) = silently {
            for child in &silently_children {
                if let AstNode::Text { content, is_prose, .. } = child {
                    if content.contains("hidden") {
                        assert!(!*is_prose, "text inside <<silently>> should NOT be prose");
                    }
                }
            }
        }

        // "more visible" should be prose (it's after <</silently>>)
        let more_visible = top_text_nodes.iter().find(|(c, _)| c.contains("more visible"));
        if let Some((_, is_prose)) = more_visible {
            assert!(is_prose, "top-level text after <<silently>> should be prose");
        }
        // If "more visible" isn't a top-level node, it may be inside the
        // silently macro — but that shouldn't happen with proper close-tag pairing.
    }

    #[test]
    fn prose_nested_if_inside_silently() {
        // <<silently>><<if>>text<</if>><</silently>> — text inside nested <<if>>
        // should still be non-prose because the parent <<silently>> suppresses rendering
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<silently>><<if true>>hidden<</if>><</silently>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "silently");
                // Find the <<if>> macro inside
                for child in children {
                    if let AstNode::Macro { name, children: Some(if_children), .. } = child {
                        assert_eq!(name, "if");
                        for if_child in if_children {
                            if let AstNode::Text { is_prose, .. } = if_child {
                                assert!(!*is_prose,
                                    "text inside <<if>> nested in <<silently>> should NOT be prose");
                            }
                        }
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_done_is_not_prose() {
        // <<done>> executes code after rendering — its body is not narrative prose.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<done>><<set $x to 1>><</done>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "done");
                // The tree builder should have marked any Text children as non-prose
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<done>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn expression_sigil_emits_macro_token() {
        // <<=>> should emit a Macro token for the = sigil
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<= $hp>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        // Should have at least a Macro token for the = sigil and a Variable token for $hp
        let macro_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!macro_tokens.is_empty(), "<<=>> should emit a Macro token for the sigil");
        // The sigil token should be at offset 2 (past <<) and length 1
        let sigil = &macro_tokens[0];
        assert_eq!(sigil.start, 2, "sigil token should start at offset 2");
        assert_eq!(sigil.length, 1, "sigil token length should be 1");
        assert!(sigil.modifier.is_none(), "<<=>> sigil should have no modifier");
    }

    #[test]
    fn silent_expression_sigil_has_control_flow_modifier() {
        // <<->> should emit a Macro token with ControlFlow modifier
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<- $hp>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let sigil_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!sigil_tokens.is_empty(), "<<->> should emit a Macro token for the sigil");
        let sigil = &sigil_tokens[0];
        assert_eq!(sigil.start, 2, "sigil token should start at offset 2");
        assert_eq!(sigil.length, 1, "sigil token length should be 1");
        assert_eq!(sigil.modifier, Some(SemanticTokenModifier::ControlFlow),
            "<<->> sigil should have ControlFlow modifier");
    }

    #[test]
    fn inline_macro_emits_open_close_delimiter_tokens() {
        // <<set $hp to 10>> should emit two MacroDelimiter tokens:
        //   - `<<` at offset 0, length 2
        //   - `>>` at offset 14, length 2
        // Both with no modifier (inline macro, not deprecated).
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<set $hp to 10>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 2, "inline macro should emit exactly 2 delimiter tokens (<< and >>)");

        // Sort by start offset to make assertions order-independent
        let mut delims_sorted = delims.clone();
        delims_sorted.sort_by_key(|t| t.start);

        // First delimiter: `<<`
        assert_eq!(delims_sorted[0].start, 0, "`<<` should start at offset 0");
        assert_eq!(delims_sorted[0].length, 2, "`<<` should be length 2");
        assert!(delims_sorted[0].modifier.is_none(),
            "inline non-deprecated macro delimiters should have no modifier");

        // Second delimiter: `>>` — at the end of the input minus 2
        let expected_close_start = input.len() - 2;
        assert_eq!(delims_sorted[1].start, expected_close_start,
            "`>>` should start at offset {}", expected_close_start);
        assert_eq!(delims_sorted[1].length, 2, "`>>` should be length 2");
        assert!(delims_sorted[1].modifier.is_none(),
            "inline non-deprecated macro delimiters should have no modifier");
    }

    #[test]
    fn block_macro_emits_four_delimiter_tokens_with_depth() {
        // <<if $hp gte 10>>Alive<</if>> should emit four MacroDelimiter tokens:
        //   - `<<` at offset 0 (open)
        //   - `>>` at offset 16 (open end)
        //   - `<</` at offset 22 (close start)
        //   - `>>` at offset 28 (close end)
        // Top-level block macro is at depth 0 → all four delimiters get None
        // (base delimiter color). Depth modifiers only kick in when nested.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<if $hp gte 10>>Alive<</if>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 4,
            "block macro should emit 4 delimiter tokens (<<, >>, <</, >>), got {:?}", delims);

        // All four should have None (top-level, depth 0 = base color)
        for (i, d) in delims.iter().enumerate() {
            assert!(d.modifier.is_none(),
                "delimiter {} should have NO modifier (depth 0 = base color), got {:?}", i, d.modifier);
        }

        // Verify the `<</` is 3 bytes
        let slash_open = delims.iter()
            .find(|t| t.length == 3)
            .expect("should have one 3-byte delimiter (`<</`)");
        assert!(slash_open.start >= 17,
            "`<</` should start after the open tag's `>>`");
    }

    #[test]
    fn nested_block_macros_delimiters_track_depth() {
        // Outer <<if>> at depth 0 (top-level), inner <<if>> at depth 1.
        // Outer delimiters → None (base, depth 0)
        // Inner delimiters → BlockDepth1 (depth 1, inside one block)
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<if $a>><<if $b>>nested<</if>><</if>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let depth0_delims = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)
                && t.modifier.is_none())
            .count();
        let depth1_delims = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)
                && t.modifier == Some(SemanticTokenModifier::BlockDepth1))
            .count();

        // Outer block macro: 4 delimiters at depth 0 (None) — <<, >>, <</, >>
        // Inner block macro: 4 delimiters at depth 1 (BlockDepth1) — <<, >>, <</, >>
        assert_eq!(depth0_delims, 4, "outer macro should contribute 4 base-color delimiters (None modifier)");
        assert_eq!(depth1_delims, 4, "inner macro should contribute 4 BlockDepth1 delimiters");
    }

    #[test]
    fn inline_macro_inside_block_one_deeper_than_block() {
        // Depth semantics: DELIMITERS track nesting depth, but the macro
        // NAME does NOT — the name always uses the base `macro` color so
        // the identifier stays visually stable regardless of nesting.
        //
        // So `<<set>>` inside `<<link>>`:
        //   - `link` name → None (base macro color)
        //   - `set` name  → None (base macro color)
        //   - `<<link>>` delimiters → None (depth 0 = base delimiter color)
        //   - `<<set>>` delimiters  → BlockDepth1 (depth 1, inside one block)
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<link \"Go\" \"Forest\">><<set $x to 1>><</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        // <<link>> name at offset 2, length 4 — should be None (base color, no depth)
        let link_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.start == 2 && t.length == 4)
            .expect("should find link name token");
        assert!(link_name.modifier.is_none(),
            "<<link>> name should have NO depth modifier (base macro color), got {:?}",
            link_name.modifier);

        // <<set>> name at offset 24, length 3 — should be None (base color, no depth)
        let set_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.start == 24 && t.length == 3)
            .expect("should find set name token");
        assert!(set_name.modifier.is_none(),
            "<<set>> name should have NO depth modifier (base macro color), got {:?}",
            set_name.modifier);

        // <<link>> delimiters (offset 0) → None (depth 0 = base color)
        let link_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 0)
            .expect("should find `<<` delimiter for <<link>>");
        assert!(link_open_delim.modifier.is_none(),
            "<<link>> `<<` delimiter should have NO modifier (depth 0 = base), got {:?}",
            link_open_delim.modifier);

        // <<set>> delimiters (offset 22, 35) → BlockDepth1 (depth 1, inside link)
        let set_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 22)
            .expect("should find `<<` delimiter for inner <<set>>");
        assert_eq!(set_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "inner `<<` delimiter should be BlockDepth1 (inside one block), got {:?}",
            set_open_delim.modifier);

        let set_close_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 35)
            .expect("should find `>>` delimiter for inner <<set>>");
        assert_eq!(set_close_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "inner `>>` delimiter should be BlockDepth1 (inside one block), got {:?}",
            set_close_delim.modifier);
    }

    #[test]
    fn deeply_nested_inline_macro_inside_two_blocks() {
        // The user's exact scenario from chat:
        //
        //   <<link>>                  // delimiters: None (depth 0 = base)
        //     <<if true>>             // delimiters: BlockDepth1 (depth 1)
        //       <<adjustStat ...>>    // delimiters: BlockDepth2 (depth 2) ← key assertion
        //     <</if>>
        //   <</link>>
        //
        // The macro NAMES (link, if, adjustStat) all use the base `macro`
        // color — NO depth modifier on names. Only the delimiters track depth.
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<link \"Chat\" \"Coworker\">><<if true>><<adjustStat \"stress\" -3>><</if>><</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let macro_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();

        // All macro NAMES should have NO depth modifier — base macro color only.
        // (link at offset 2 len 4, if at offset 28 len 2, adjustStat at offset 39 len 10)
        for t in &macro_tokens {
            assert!(t.modifier.is_none(),
                "macro name at offset {} should have NO depth modifier (base color only), got {:?}",
                t.start, t.modifier);
        }

        // Delimiters track depth — verify the `<<` before each name.
        // <<link>> `<<` at offset 0 → None (depth 0 = base color)
        let link_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 0)
            .expect("should find `<<` delimiter for <<link>>");
        assert!(link_open_delim.modifier.is_none(),
            "<<link>> `<<` delimiter should have NO modifier (depth 0 = base), got {:?}",
            link_open_delim.modifier);

        // <<if>> `<<` at offset 26 (28 - 2) → BlockDepth1 (depth 1, inside link)
        let if_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 26)
            .expect("should find `<<` delimiter for <<if>>");
        assert_eq!(if_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "<<if>> `<<` delimiter should be BlockDepth1 (inside one block), got {:?}",
            if_open_delim.modifier);

        // <<adjustStat>> `<<` at offset 37 (39 - 2) → BlockDepth2 (depth 2, inside link+if)
        let adjust_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 37)
            .expect("should find `<<` delimiter at offset 37 (immediately before adjustStat)");
        assert_eq!(adjust_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth2),
            "<<adjustStat>>'s `<<` delimiter should be BlockDepth2 (inside two blocks), got {:?}",
            adjust_open_delim.modifier);
    }

    #[test]
    fn top_level_inline_macro_has_no_depth_modifier() {
        // Sanity: a bare `<<set>>` at the top level (not inside any block)
        // should still get `None` for its modifier — no enclosing block to
        // inherit depth from. This guards against the fix above over-applying
        // depth modifiers to top-level inline macros.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<set $x to 1>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let set_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.length == 3)
            .expect("should find `set` name token");
        assert!(set_name.modifier.is_none(),
            "top-level `<<set>>` should have no depth modifier, got {:?}",
            set_name.modifier);

        // All delimiter tokens at top level should also have no modifier
        for t in tokens.iter().filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)) {
            assert!(t.modifier.is_none(),
                "top-level delimiter at offset {} should have no modifier, got {:?}",
                t.start, t.modifier);
        }
    }

    #[test]
    fn expression_macro_emits_delimiter_tokens() {
        // <<= $hp>> should emit two MacroDelimiter tokens for `<<` and `>>`.
        // The sigil (`=`) stays as a Macro token — delimiters are separate.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<= $hp>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 2,
            "expression macro should emit 2 delimiter tokens (<< and >>)");

        let mut sorted = delims.clone();
        sorted.sort_by_key(|t| t.start);
        assert_eq!(sorted[0].start, 0, "`<<` at offset 0");
        assert_eq!(sorted[0].length, 2, "`<<` length 2");
        assert_eq!(sorted[1].start, input.len() - 2, "`>>` at end-2");
        assert_eq!(sorted[1].length, 2, "`>>` length 2");
    }

    #[test]
    fn delimiter_tokens_are_distinct_type_from_name() {
        // Sanity: the macro NAME token must be `Macro`, not `MacroDelimiter`,
        // and vice versa. This guards against accidental collapse.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<set $x to 1>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let has_macro_name = tokens.iter().any(|t| matches!(t.token_type, SemanticTokenType::Macro));
        let has_delimiter = tokens.iter().any(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter));
        assert!(has_macro_name, "should have a Macro token for the name `set`");
        assert!(has_delimiter, "should have MacroDelimiter tokens for << >>");
    }

    #[test]
    fn print_and_expression_emit_equivalent_variable_tokens() {
        // <<print $hp>> and <<= $hp>> should emit the same Variable tokens
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast_print = crate::sugarcube::parser::parse_passage_body(
            "<<print $hp>>", 0, ParseMode::Normal,
        );
        let ast_expr = crate::sugarcube::parser::parse_passage_body(
            "<<= $hp>>", 0, ParseMode::Normal,
        );

        let mut tokens_print = Vec::new();
        let mut tokens_expr = Vec::new();
        build_semantic_tokens(&ast_print.nodes, &mut tokens_print, 0, &HashSet::new());
        build_semantic_tokens(&ast_expr.nodes, &mut tokens_expr, 0, &HashSet::new());

        let var_tokens_print: Vec<_> = tokens_print.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .collect();
        let var_tokens_expr: Vec<_> = tokens_expr.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .collect();

        assert!(!var_tokens_print.is_empty(), "<<print>> should emit Variable tokens");
        assert!(!var_tokens_expr.is_empty(), "<<=>> should emit Variable tokens");
        // Both should have the same number of Variable tokens for the same expression
        assert_eq!(var_tokens_print.len(), var_tokens_expr.len(),
            "<<print>> and <<=>> should emit the same number of Variable tokens");
    }

    #[test]
    fn inline_style_double_at() {
        // @@.highlight;important text@@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;important text@@", 0, ParseMode::Normal,
        );
        let style_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::InlineStyle { class, .. } => Some(class.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        assert_eq!(style_node, ".highlight");

        // Verify children contain prose text
        let style = ast.nodes.iter().find_map(|n| match n {
            node @ AstNode::InlineStyle { .. } => Some(node.clone()),
            _ => None,
        }).unwrap();
        match &style {
            AstNode::InlineStyle { children, .. } => {
                assert!(!children.is_empty(), "InlineStyle should have children");
                let has_prose = children.iter().any(|c| matches!(c, AstNode::Text { is_prose: true, .. }));
                assert!(has_prose, "children should contain prose text");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn inline_style_single_at() {
        // @.red;warning text@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@.red;warning text@", 0, ParseMode::Normal,
        );
        let style_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::InlineStyle { class, .. } => Some(class.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        assert_eq!(style_node, ".red");
    }

    #[test]
    fn inline_style_with_variable() {
        // @@.highlight;You have $gold coins.@@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;You have $gold coins.@@",
            0,
            ParseMode::Normal,
        );
        let style = ast.nodes.iter().find_map(|n| match n {
            node @ AstNode::InlineStyle { .. } => Some(node.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        match &style {
            AstNode::InlineStyle { class, children, .. } => {
                assert_eq!(class, ".highlight");
                // Children should contain a Text node with a $gold variable ref
                let text_with_var = children.iter().any(|c| {
                    matches!(c, AstNode::Text { var_refs, .. } if !var_refs.is_empty())
                });
                assert!(text_with_var, "children should contain Text with variable refs");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn inline_style_emits_token() {
        // Verify InlineStyle semantic token emission
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;important@@",
            0,
            ParseMode::Normal,
        );
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let style_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::InlineStyle))
            .collect();
        assert!(!style_tokens.is_empty(), "should emit InlineStyle token for class name");
        assert_eq!(style_tokens[0].length, ".highlight".len(),
            "InlineStyle token should cover the class name");
    }

    #[test]
    fn text_format_bold() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is ''bold'' text", 0, ParseMode::Normal,
        );
        let bold_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Bold, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Bold TextFormat node");
        assert_eq!(bold_node, "bold");
    }

    #[test]
    fn text_format_italic() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is //italic// text", 0, ParseMode::Normal,
        );
        let italic_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Italic, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Italic TextFormat node");
        assert_eq!(italic_node, "italic");
    }

    #[test]
    fn text_format_strike() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is ==struck== text", 0, ParseMode::Normal,
        );
        let node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Strike, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Strike TextFormat node");
        assert_eq!(node, "struck");
    }

    #[test]
    fn text_format_emits_token() {
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body(
            "Some ''bold'' text", 0, ParseMode::Normal,
        );
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let format_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::TextFormat))
            .collect();
        assert!(!format_tokens.is_empty(), "should emit TextFormat token for bold");
    }

    #[test]
    fn text_format_with_multibyte_utf8() {
        // Regression test: unclosed text-format delimiters followed by
        // multi-byte UTF-8 characters (e.g. em dash —) must not panic.
        // Previously the byte-by-byte scan could land inside a multi-byte
        // char, causing a panic on string slicing.
        let cases = [
            ("''bold —", TextFormatKind::Bold),
            ("//italic —", TextFormatKind::Italic),
            ("==strike —", TextFormatKind::Strike),
            ("__underline —", TextFormatKind::Underline),
            ("~~sub —", TextFormatKind::Sub),
            ("^^super —", TextFormatKind::Super),
        ];
        for (input, _expected_kind) in &cases {
            // Must not panic
            let _ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        }
    }

    #[test]
    fn text_format_closed_with_multibyte_content() {
        // Text-format markup with multi-byte characters inside should parse correctly
        let ast = crate::sugarcube::parser::parse_passage_body(
            "''bold — dash''", 0, ParseMode::Normal,
        );
        let node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Bold, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Bold TextFormat node");
        assert_eq!(node, "bold — dash");
    }

    #[test]
    fn prose_with_em_dash_no_panic() {
        // Plain prose with em dashes should never panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "The state — never here — is tracked.", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn macro_args_with_multibyte_utf8() {
        // Regression test: macro arguments containing multi-byte UTF-8
        // characters (e.g. em dash — in strings or comments) must not panic.
        // The scanner previously advanced by single bytes, which could land
        // inside a multi-byte char, causing a panic on string slicing.
        let cases = [
            // Em dash inside a quoted string in macro args
            r#"<<set $x = "a—b">>"#,
            // Em dash inside a block comment in macro args
            "<<set $x = 1 /* comment — with dash */ + 2>>",
            // Em dash in a line comment in macro args
            "<<set $x = 1 // comment — dash\n+ 3>>",
            // Em dash in plain args (not in string or comment)
            "<<set $x = 1>>", // no em dash but safe
            // Multiple em dashes
            "The — quick — brown — fox",
        ];
        for input in &cases {
            let _ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        }
    }

    #[test]
    fn expression_macro_with_multibyte_utf8() {
        // <<= and <<->> with multi-byte chars in the expression
        let _ast1 = crate::sugarcube::parser::parse_passage_body(
            "<<= 'hello—world'>>", 0, ParseMode::Normal,
        );
        let _ast2 = crate::sugarcube::parser::parse_passage_body(
            "<<- 'silent—expr'>>", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn script_macro_with_multibyte_utf8_body() {
        // <<script>> body with em dashes should not panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "<<script>>\n// comment — dash\nvar x = 1;\n<</script>>",
            0, ParseMode::Normal,
        );
    }

    #[test]
    fn style_macro_with_multibyte_utf8_body() {
        // <<style>> body with em dashes should not panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "<<style>>\n/* style — dash */\n.foo { color: red; }\n<</style>>",
            0, ParseMode::Normal,
        );
    }

    #[test]
    fn inline_vars_with_multibyte_utf8() {
        // Variable scanning in text with multi-byte chars
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "The value — $gs.x — is tracked.", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn special_twee_like_content_no_panic() {
        // Simulates the content pattern from _special.twee that triggered
        // the original UTF-8 boundary panic. The file contains <<set>> macros
        // with JS object literals containing comments with em dashes, e.g.:
        //   <<set $SLOTS = {
        //     "slot": { description: "tracked — never here" },
        //   }>>
        // The em dash inside the macro args caused byte 275 to land inside
        // the 3-byte UTF-8 sequence, panicking on string slicing.
        let content = r#"/*
  REGISTRIES
  Read-only master definitions. Set once here, never mutated.
  Dynamic state for all entities lives exclusively in $gs.
  All registry values are accessed by key string.
*/
<<set $SLOTS = {
  "underwear-top":    { description: "Worn — against the skin" },
  "underwear-bottom": { description: "Lower — body coverage" },
  "legwear":          { description: "Stockings — tights, socks" },
  "bottom":           { description: "Skirt — trousers, shorts" },
}>>
<<set $ITEMS = {
  "blazer": { label: "Blazer", type: "top", description: "A tailored blazer — sharp and professional." },
  "skirt":  { label: "Skirt",  type: "bottom", description: "A pleated skirt — elegant." },
}>>
/* The inventory — never stored directly in items */
<<set $NPCS = {
  "mai": { name: "Mai", description: "Your coworker — friendly and observant" },
}>>
Some narrative text with — em dashes — and $gs.inventory references."#;
        let _ast = crate::sugarcube::parser::parse_passage_body(content, 0, ParseMode::Normal);
        // If we get here without panicking, the fix works.
    }

    #[test]
    fn block_comment_inside_set_args_emits_comment_token() {
        // Comments inside <<set>> JS expressions (e.g. inside object literals)
        // should be recognized as Comment tokens via the JS annotation pass.
        // oxc strips comments from the AST, so we scan the raw preprocessed
        // source separately in js_walk::extract_comments().
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = { /* inner comment */ a: 1 }>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for /* inner comment */ inside <<set>> args");
    }

    #[test]
    fn line_comment_inside_set_args_emits_comment_token() {
        // // line comments inside <<set>> JS expressions should also be
        // recognized as Comment tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = { a: 1, // inner line comment\n b: 2 }>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for // inner line comment inside <<set>> args, got {} comment tokens",
            comment_tokens.len());
    }

    #[test]
    fn multiline_block_comment_inside_set_args_emits_comment_token() {
        // Multi-line /* */ block comments inside <<set>> JS expressions
        // should be recognized as a single Comment token spanning ALL lines
        // including the closing */.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = {\n  /* this is\n     a multi-line\n     comment */\n  a: 1\n}>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for multi-line /* */ inside <<set>> args, got {} comment tokens",
            comment_tokens.len());

        // The comment token should span the FULL multi-line comment including
        // the closing */ and all content lines.
        let full_comment_text: String = comment_tokens.iter()
            .map(|t| text[t.start.min(text.len())..(t.start + t.length).min(text.len())].to_string())
            .collect();
        assert!(full_comment_text.contains("*/"),
            "comment token should include the closing */, got: {:?}", full_comment_text);
        assert!(full_comment_text.contains("multi-line"),
            "comment token should include 'multi-line' from line 2, got: {:?}", full_comment_text);
        assert!(full_comment_text.contains("this is"),
            "comment token should include 'this is' from line 1, got: {:?}", full_comment_text);
    }

    #[test]
    fn set_array_of_objects_emits_literal_tokens() {
        // <<set $arr = [{a:1}, {b:2}]>> — array of objects.
        // The array handler must recurse into nested ObjectExpression
        // elements so property values get literal tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $arr = [{a:1}, {b:2}]>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let number_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Number))
            .collect();
        assert_eq!(number_tokens.len(), 2,
            "should have 2 Number tokens for 1 and 2 inside objects in array, got {}", number_tokens.len());
    }

    #[test]
    fn set_nested_array_emits_literal_tokens() {
        // <<set $arr = [[1,2], [3,4]]>> — array of arrays.
        // The array handler must recurse into nested ArrayExpression
        // elements so inner literals get tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $arr = [[1,2], [3,4]]>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let number_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Number))
            .collect();
        assert_eq!(number_tokens.len(), 4,
            "should have 4 Number tokens for 1,2,3,4 inside nested arrays, got {}", number_tokens.len());
    }

    #[test]
    fn prose_token_does_not_overlap_variable_tokens() {
        // Naked $variables in prose should get Variable tokens that are
        // NOT overlapped by the Prose token. The Prose token is split
        // around variable positions so each position has exactly one
        // semantic token type.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\nYou have $gold coins.\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let all_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .collect();

        let var_token = all_tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .expect("should have a Variable token for $gold");

        for t in &all_tokens {
            if matches!(t.token_type, SemanticTokenType::Prose) {
                let var_start = var_token.start;
                let var_end = var_token.start + var_token.length;
                let prose_start = t.start;
                let prose_end = t.start + t.length;
                let overlaps = var_start < prose_end && prose_start < var_end;
                assert!(!overlaps,
                    "Prose token [{},{}) should not overlap Variable token [{},{})",
                    prose_start, prose_end, var_start, var_end);
            }
        }
    }

    #[test]
    fn template_invocation_includes_question_mark() {
        // ?playerName in prose should get a Function token that INCLUDES
        // the ? sigil, so the whole ?playerName is visually distinct.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\nWelcome ?playerName to the game.\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let func_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Function))
            .collect();
        assert!(!func_tokens.is_empty(), "should have a Function token for ?playerName");

        let tok = &func_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert!(token_text.starts_with('?'),
            "template token should include the ? sigil, got: {:?}", token_text);
    }

    // ── Phase 1 tests (plan.md §7.1.6) ────────────────────────────────────
    //
    // These tests verify that the Phase 1 refactor (line-start tracking via
    // `ParseCtx`, new AST variants, new SemanticTokenType variants) is
    // behavior-preserving. The parser does NOT yet emit any of the new
    // variants — those come in Phases 2-6. These tests ensure:
    //
    //   1. The column counter correctly distinguishes column-0 from mid-line
    //      positions (verified indirectly via the `//` comment heuristic,
    //      which now uses `ctx.col == 0` as one of its line-start signals).
    //   2. Existing parsing behavior is unchanged after the refactor.
    //   3. The new AST variants are constructible (compile-time check).

    #[test]
    fn phase1_line_start_comment_at_column_zero() {
        // //comment at column 0 should be a line comment.
        // This exercises the `ctx.col == 0` branch in the `//` heuristic.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "//comment at col 0", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        assert!(matches!(&ast.nodes[0], AstNode::Comment { kind: CommentKind::JsLine, .. }),
            "expected JsLine comment at col 0, got {:?}", ast.nodes[0]);
    }

    #[test]
    fn phase1_mid_line_double_slash_not_a_comment() {
        // text//more — the // is NOT at line start and NOT preceded by
        // whitespace, so it's neither italic (no closing //) nor a comment.
        // It should be plain prose text.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "text//more", 0, ParseMode::Normal,
        );
        // Should produce a single Text node (the // is just text).
        assert_eq!(ast.nodes.len(), 1, "expected single Text node, got {} nodes: {:?}", ast.nodes.len(), ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::Text { .. }),
            "expected Text node, got {:?}", ast.nodes[0]);
    }

    #[test]
    fn phase1_comment_after_newline_at_column_zero() {
        // Line 1 text\n//comment — the // on line 2 is at column 0.
        // This exercises the column reset on `\n` in the main loop.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "line one\n//comment", 0, ParseMode::Normal,
        );
        // Should have: Text("line one\n") + Comment(JsLine)
        // OR the comment might flush differently. The key assertion is
        // that a JsLine comment exists.
        fn has_js_line_comment(nodes: &[AstNode]) -> bool {
            for n in nodes {
                if matches!(n, AstNode::Comment { kind: CommentKind::JsLine, .. }) {
                    return true;
                }
            }
            false
        }
        assert!(has_js_line_comment(&ast.nodes),
            "expected a JsLine comment on line 2, got nodes: {:?}", ast.nodes);
    }

    #[test]
    fn phase1_multiline_text_preserves_column_tracking() {
        // Multiple lines with // at various positions. This is a regression
        // test to ensure the column counter correctly resets on each \n.
        let text = "first line\n//comment line\nthird line\n//another comment";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        // Count JsLine comments — should be 2 (lines 2 and 4).
        fn count_js_line_comments(nodes: &[AstNode]) -> usize {
            nodes.iter().filter(|n| matches!(n, AstNode::Comment { kind: CommentKind::JsLine, .. })).count()
        }
        let comment_count = count_js_line_comments(&ast.nodes);
        assert_eq!(comment_count, 2,
            "expected 2 JsLine comments (lines 2 and 4), got {}: nodes={:?}",
            comment_count, ast.nodes);
    }

    #[test]
    fn phase1_new_ast_variants_are_constructible() {
        // Compile-time check: ensure the new AstNode variants exist and
        // have the expected fields. This test doesn't assert runtime
        // behavior — it just confirms the variants are usable.
        use crate::sugarcube::ast::{TableRow, TableCell, TableRowType};
        use std::ops::Range;

        let _heading = AstNode::Heading {
            level: 1,
            children: vec![],
            span: Range { start: 0, end: 10 },
        };
        let _hr = AstNode::HorizontalRule {
            span: Range { start: 0, end: 4 },
        };
        let _list_item = AstNode::ListItem {
            depth: 1,
            ordered: false,
            marker: "*".to_string(),
            children: vec![],
            span: Range { start: 0, end: 6 },
        };
        let _blockquote = AstNode::Blockquote {
            depth: 1,
            children: vec![],
            span: Range { start: 0, end: 10 },
        };
        let _blockquote_block = AstNode::BlockquoteBlock {
            children: vec![],
            open_span: Range { start: 0, end: 3 },
            close_span: Some(Range { start: 10, end: 13 }),
            span: Range { start: 0, end: 13 },
        };
        let _table = AstNode::Table {
            header: None,
            rows: vec![],
            footer: None,
            caption: None,
            caption_span: None,
            class: None,
            class_span: None,
            span: Range { start: 0, end: 20 },
        };
        let _code_block = AstNode::CodeBlock {
            content: "raw code".to_string(),
            span: Range { start: 0, end: 20 },
        };
        let _inline_code = AstNode::InlineCode {
            content: "code".to_string(),
            span: Range { start: 0, end: 10 },
        };

        // TableRow / TableCell / TableRowType
        let _row = TableRow {
            cells: vec![TableCell {
                children: vec![],
                is_header: false,
                colspan: false,
                rowspan: false,
                span: Range { start: 0, end: 5 },
            }],
            row_type: TableRowType::Body,
            span: Range { start: 0, end: 10 },
        };
        let _header_row_type = TableRowType::Header;
        let _caption_row_type = TableRowType::Caption;
        let _class_row_type = TableRowType::Class;
        let _footer_row_type = TableRowType::Footer;

        // If this compiles, the new variants are correctly defined.
        // (We don't assert anything — this is a structural compile check.)
    }

    #[test]
    fn phase1_new_semantic_token_types_exist() {
        // Compile-time check: ensure the new SemanticTokenType variants
        // exist and have the correct wire names.
        use crate::plugin::SemanticTokenType;

        assert_eq!(SemanticTokenType::Heading.lsp_name(), "heading");
        assert_eq!(SemanticTokenType::HorizontalRule.lsp_name(), "horizontalRule");
        assert_eq!(SemanticTokenType::ListMarker.lsp_name(), "listMarker");
        assert_eq!(SemanticTokenType::Blockquote.lsp_name(), "blockquote");
        assert_eq!(SemanticTokenType::BlockquoteBlock.lsp_name(), "blockquoteBlock");
        assert_eq!(SemanticTokenType::Table.lsp_name(), "table");
        assert_eq!(SemanticTokenType::CodeBlock.lsp_name(), "codeBlock");
        assert_eq!(SemanticTokenType::InlineCode.lsp_name(), "inlineCode");

        // Verify legend indices are 22-29 (appended at end to preserve 0-21).
        assert_eq!(SemanticTokenType::Heading.legend_index(), 22);
        assert_eq!(SemanticTokenType::HorizontalRule.legend_index(), 23);
        assert_eq!(SemanticTokenType::ListMarker.legend_index(), 24);
        assert_eq!(SemanticTokenType::Blockquote.legend_index(), 25);
        assert_eq!(SemanticTokenType::BlockquoteBlock.legend_index(), 26);
        assert_eq!(SemanticTokenType::Table.legend_index(), 27);
        assert_eq!(SemanticTokenType::CodeBlock.legend_index(), 28);
        assert_eq!(SemanticTokenType::InlineCode.legend_index(), 29);

        // Verify existing indices are unchanged (0-21).
        assert_eq!(SemanticTokenType::PassageHeader.legend_index(), 0);
        assert_eq!(SemanticTokenType::MacroDelimiter.legend_index(), 21);
    }

    #[test]
    fn phase1_inline_style_recursive_parse_preserves_behavior() {
        // Regression test: the parse_inline_style refactor (now takes
        // &mut ParseCtx instead of offset) must preserve existing behavior.
        // @@.highlight;Hello [[Forest]]@@ should produce an InlineStyle
        // with a Link child.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;Hello [[Forest]]@@", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single InlineStyle node, got {} nodes", ast.nodes.len());
        match &ast.nodes[0] {
            AstNode::InlineStyle { children, class, .. } => {
                assert_eq!(class, ".highlight");
                // Should have a Text node ("Hello ") and a Link node.
                let has_link = children.iter().any(|c| matches!(c, AstNode::Link { .. }));
                assert!(has_link, "expected a Link child in InlineStyle, got: {:?}", children);
            }
            other => panic!("expected InlineStyle, got {:?}", other),
        }
    }

    // ── Phase 2a tests (plan.md §7.2.7) — Code blocks {{{...}}} ────────────
    //
    // These tests verify the critical bug fix: macros inside `{{{...}}}`
    // code blocks must NOT execute. Previously, the absence of a `b'{'`
    // arm meant `{{{ <<set $x to 1>> }}}` would parse the `<<set>>` as a
    // real macro, mutating `$x`. Now the entire construct is captured as
    // a single raw CodeBlock/InlineCode node.

    #[test]
    fn phase2a_inline_code_does_not_execute_macros() {
        // {{{ <<set $x to 1>> }}} — single line, so this is INLINE code.
        // The `<<set>>` must NOT be parsed as a macro.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "{{{ <<set $x to 1>> }}}", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single InlineCode node, got {} nodes: {:?}", ast.nodes.len(), ast.nodes);
        match &ast.nodes[0] {
            AstNode::InlineCode { content, span } => {
                assert_eq!(content, " <<set $x to 1>> ",
                    "InlineCode content should be the raw text between triple-braces");
                // Span should cover the entire construct including delimiters.
                assert_eq!(span.start, 0, "span start should be 0");
                assert_eq!(span.end, 23, "span end should be 23 (full construct length)");
            }
            other => panic!("expected InlineCode, got {:?}", other),
        }
        // Critical: verify NO Macro nodes were produced.
        let has_macro = ast.nodes.iter().any(|n| matches!(n, AstNode::Macro { .. }));
        assert!(!has_macro, "CRITICAL: <<set>> macro was parsed/executed inside InlineCode! nodes: {:?}", ast.nodes);
    }

    #[test]
    fn phase2a_block_code_multiline_does_not_execute_macros() {
        // {{{
        // <<set $x to 1>>
        // }}}
        // This is BLOCK code ({{{ at col 0 followed by \n, }}} on own line).
        let text = "{{{\n<<set $x to 1>>\n}}}";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single CodeBlock node, got {} nodes: {:?}", ast.nodes.len(), ast.nodes);
        match &ast.nodes[0] {
            AstNode::CodeBlock { content, span } => {
                assert_eq!(content, "<<set $x to 1>>\n",
                    "CodeBlock content should be the raw text between the opening and closing lines");
                assert_eq!(span.start, 0, "span start should be 0");
                assert_eq!(span.end, text.len(), "span end should cover the entire construct");
            }
            other => panic!("expected CodeBlock, got {:?}", other),
        }
        // Critical: verify NO Macro nodes were produced.
        let has_macro = ast.nodes.iter().any(|n| matches!(n, AstNode::Macro { .. }));
        assert!(!has_macro, "CRITICAL: <<set>> macro was parsed/executed inside CodeBlock! nodes: {:?}", ast.nodes);
    }

    #[test]
    fn phase2a_inline_code_with_variables_not_interpolated() {
        // Variables inside {{{...}}} should be literal, not interpolated.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "The variable {{{$name}}} is shown.", 0, ParseMode::Normal,
        );
        // Should produce: Text("The variable ") + InlineCode("$name") + Text(" is shown.")
        assert_eq!(ast.nodes.len(), 3, "expected 3 nodes (Text + InlineCode + Text), got: {:?}", ast.nodes);
        let inline_code = ast.nodes.iter().find(|n| matches!(n, AstNode::InlineCode { .. }));
        assert!(inline_code.is_some(), "expected an InlineCode node, got: {:?}", ast.nodes);
        if let Some(AstNode::InlineCode { content, .. }) = inline_code {
            assert_eq!(content, "$name", "InlineCode content should be the raw '$name'");
        }
        // Verify no variable references were extracted from the InlineCode content.
        // (var_refs are only on Text and Macro nodes, not on InlineCode.)
    }

    #[test]
    fn phase2a_inline_code_with_links_not_processed() {
        // Links inside {{{...}}} should be literal text, not turned into Link nodes.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "See {{{[[Forest]]}}} for details.", 0, ParseMode::Normal,
        );
        // Should produce: Text("See ") + InlineCode("[[Forest]]") + Text(" for details.")
        assert_eq!(ast.nodes.len(), 3, "expected 3 nodes (Text + InlineCode + Text), got: {:?}", ast.nodes);
        let has_link = ast.nodes.iter().any(|n| matches!(n, AstNode::Link { .. }));
        assert!(!has_link, "CRITICAL: [[Forest]] was parsed as a Link inside InlineCode! nodes: {:?}", ast.nodes);
        let inline_code = ast.nodes.iter().find(|n| matches!(n, AstNode::InlineCode { .. }));
        if let Some(AstNode::InlineCode { content, .. }) = inline_code {
            assert_eq!(content, "[[Forest]]", "InlineCode content should be the raw '[[Forest]]'");
        }
    }

    #[test]
    fn phase2a_block_code_disambiguation_requires_newline() {
        // {{{ at col 0 but NOT followed by \n is INLINE code, not block.
        // E.g., a passage body starting with "{{{code}}}" (no newline after {{{).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "{{{code}}}", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::InlineCode { content, .. } => {
                assert_eq!(content, "code", "should be InlineCode (no newline after opening triple-brace)");
            }
            AstNode::CodeBlock { .. } => panic!("should be InlineCode, not CodeBlock (no newline after opening triple-brace)"),
            other => panic!("expected InlineCode, got {:?}", other),
        }
    }

    #[test]
    fn phase2a_block_code_mid_line_is_inline() {
        // {{{ NOT at col 0 is always inline code, even if followed by content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "text {{{code}}} more", 0, ParseMode::Normal,
        );
        // Should produce: Text("text ") + InlineCode("code") + Text(" more")
        assert_eq!(ast.nodes.len(), 3, "expected 3 nodes, got: {:?}", ast.nodes);
        let has_inline_code = ast.nodes.iter().any(|n| matches!(n, AstNode::InlineCode { .. }));
        assert!(has_inline_code, "expected an InlineCode node");
        let has_code_block = ast.nodes.iter().any(|n| matches!(n, AstNode::CodeBlock { .. }));
        assert!(!has_code_block, "should NOT be a CodeBlock (not at col 0)");
    }

    #[test]
    fn phase2a_unclosed_inline_code_consumes_to_end() {
        // Unclosed {{{ should consume the rest of the text as InlineCode content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "text {{{unclosed code here", 0, ParseMode::Normal,
        );
        // Should produce: Text("text ") + InlineCode("unclosed code here")
        assert_eq!(ast.nodes.len(), 2, "expected 2 nodes (Text + InlineCode), got: {:?}", ast.nodes);
        if let Some(AstNode::InlineCode { content, .. }) = ast.nodes.iter().find(|n| matches!(n, AstNode::InlineCode { .. })) {
            assert_eq!(content, "unclosed code here");
        }
    }

    #[test]
    fn phase2a_unclosed_block_code_consumes_to_end() {
        // Unclosed block code ({{{ at col 0 + \n, but no closing }}}) should
        // consume the rest of the text as CodeBlock content.
        let text = "{{{\nunclosed block code\nmore code\nno closing";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single CodeBlock node, got: {:?}", ast.nodes);
        if let AstNode::CodeBlock { content, .. } = &ast.nodes[0] {
            assert_eq!(content, "unclosed block code\nmore code\nno closing");
        }
    }

    #[test]
    fn phase2a_inline_code_emits_codeblock_token() {
        // Verify the token builder emits an InlineCode semantic token.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\nSome {{{inline code}}} here.\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let inline_code_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::InlineCode))
            .collect();
        assert_eq!(inline_code_tokens.len(), 1,
            "expected exactly 1 InlineCode token, got {}: {:?}",
            inline_code_tokens.len(), inline_code_tokens);

        let tok = &inline_code_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert!(token_text.starts_with("{{{") && token_text.ends_with("}}}"),
            "InlineCode token should span the full triple-brace construct, got: {:?}",
            token_text);
    }

    #[test]
    fn phase2a_block_code_emits_codeblock_token() {
        // Verify the token builder emits a CodeBlock semantic token.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n{{{\nblock code\n}}}\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let code_block_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::CodeBlock))
            .collect();
        assert_eq!(code_block_tokens.len(), 1,
            "expected exactly 1 CodeBlock token, got {}: {:?}",
            code_block_tokens.len(), code_block_tokens);

        let tok = &code_block_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert!(token_text.starts_with("{{{"),
            "CodeBlock token should start with opening triple-brace, got: {:?}", token_text);
        assert!(token_text.contains("}}}"),
            "CodeBlock token should contain the closing triple-brace, got: {:?}", token_text);
    }

    #[test]
    fn phase2a_inline_code_followed_by_macro() {
        // Inline code followed by a macro — the macro should still execute
        // (it's outside the code block).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "{{{code}}} <<set $y to 2>>", 0, ParseMode::Normal,
        );
        // Should produce: InlineCode("code") + Text(" ") + Macro("set")
        assert_eq!(ast.nodes.len(), 3, "expected 3 nodes (InlineCode + Text + Macro), got: {:?}", ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::InlineCode { content, .. } if content == "code"),
            "first node should be InlineCode(\"code\"), got: {:?}", ast.nodes[0]);
        assert!(matches!(&ast.nodes[2], AstNode::Macro { name, .. } if name == "set"),
            "third node should be Macro(\"set\"), got: {:?}", ast.nodes[2]);
    }

    // ── Architecture invariant tests ───────────────────────────────────────
    //
    // These tests verify the architectural principle that ALL downstream
    // consumers (links, var_ops, tokens, diagnostics) walk the AST produced
    // by the parser — NOT the raw body text. Code blocks (`{{{...}}}`) are
    // raw zones: their content must not be scanned for links, variables, or
    // passage references.
    //
    // If any of these tests fail, it means a consumer is scanning raw text
    // instead of walking the AST — an architecture violation that must be
    // fixed by routing that consumer through the AST.

    #[test]
    fn arch_code_block_content_not_extracted_as_link() {
        // A [[link]] inside a code block is literal text, not a real link.
        // The parser should NOT produce a Link node for it.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "{{{ [[Forest]] }}}", 0, ParseMode::Normal,
        );
        // Should produce a single InlineCode node containing "[[Forest]]".
        assert_eq!(ast.nodes.len(), 1, "expected single InlineCode, got: {:?}", ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::InlineCode { content, .. } if content == " [[Forest]] "),
            "expected InlineCode containing the literal link text, got: {:?}", ast.nodes[0]);
        // The links collection on PassageAst must be empty — the [[Forest]]
        // inside the code block is not a real link.
        assert!(ast.links.is_empty(),
            "architectural violation: links were extracted from inside a code block! links: {:?}",
            ast.links);
    }

    #[test]
    fn arch_code_block_content_not_extracted_as_var_op() {
        // A $variable inside a code block is literal text, not a variable read.
        // The parser should NOT produce var_ops for it.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "{{{ $score }}}", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single InlineCode, got: {:?}", ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::InlineCode { content, .. } if content == " $score "),
            "expected InlineCode containing the literal variable text, got: {:?}", ast.nodes[0]);
        // The var_ops collection on PassageAst must be empty.
        assert!(ast.var_ops.is_empty(),
            "architectural violation: var_ops were extracted from inside a code block! var_ops: {:?}",
            ast.var_ops);
    }

    #[test]
    fn arch_block_code_content_not_extracted_as_link_or_var() {
        // Multi-line block code with both a link and a variable inside —
        // neither should be extracted.
        let text = "{{{\n[[Forest]] and $score\n}}}";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single CodeBlock, got: {:?}", ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::CodeBlock { content, .. } if content.contains("[[Forest]]") && content.contains("$score")),
            "expected CodeBlock containing literal link and variable text, got: {:?}", ast.nodes[0]);
        assert!(ast.links.is_empty(),
            "architectural violation: links extracted from block code! links: {:?}", ast.links);
        assert!(ast.var_ops.is_empty(),
            "architectural violation: var_ops extracted from block code! var_ops: {:?}", ast.var_ops);
    }

    // NOTE on extract_data_passage_refs:
    // There is one known architecture gap: `extract_data_passage_refs` (in
    // extraction.rs) scans the RAW body text for `data-passage="..."`
    // attributes after stripping comments. It does NOT walk the AST, so a
    // `data-passage` attribute inside a `{{{...}}}` code block would be
    // incorrectly extracted as a passage reference.
    //
    // This is a PRE-EXISTING limitation (before Phase 2a, code blocks were
    // plain text, so `data-passage` inside would also have been extracted).
    // Phase 2a didn't regress this — but it didn't fix it either.
    //
    // The fix is to make `extract_data_passage_refs` walk the AST and skip
    // CodeBlock/InlineCode nodes (or, more precisely, to only scan Text
    // nodes that are NOT inside a code block). This is deferred to a future
    // phase because:
    //   1. `data-passage` inside code blocks is rare in practice.
    //   2. The fix requires threading "am I inside a code block?" context
    //      through the AST walk, which is a non-trivial refactor.
    //   3. Phase 2a's critical bug (macros executing inside code blocks) is
    //      already fixed; this is a lesser issue.
    //
    // See plan.md §4 (to be updated) for the deferred task.

    // ── Phase 2b tests (plan.md §7.2.7) — checkbox/radiobutton catalog fix ─
    //
    // These tests verify that the catalog arg schemas for `<<checkbox>>` and
    // `<<radiobutton>>` match SugarCube's documented signatures:
    //   <<checkbox receiverName uncheckedValue checkedValue [autocheck|checked]>>
    //   <<radiobutton receiverName checkedValue [autocheck|checked]>>
    //
    // Previously the catalog had a spurious "label" arg at position 0 and
    // swapped checked/unchecked values for checkbox. Now the variable is at
    // position 0, and the values are in the correct order.

    #[test]
    fn phase2b_checkbox_variable_at_position_zero() {
        // <<checkbox "$color" "red" "blue">>
        // The variable "$color" should be at position 0 (VariableRef),
        // "red" at position 1 (uncheckedValue, String),
        // "blue" at position 2 (checkedValue, String).
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<checkbox "$color" "red" "blue">>"#, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Macro node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "checkbox");
                let args = structured_args.as_ref().expect("checkbox should have structured_args");
                assert_eq!(args.len(), 3, "checkbox should have 3 structured args, got: {:?}", args);
                // Position 0: variable reference ($color)
                assert_eq!(args[0].kind, ParsedArgKind::VariableRef,
                    "arg 0 should be VariableRef (the receiver variable), got: {:?}", args[0].kind);
                // Position 1: unchecked value ("red")
                assert_eq!(args[1].kind, ParsedArgKind::String,
                    "arg 1 should be String (uncheckedValue), got: {:?}", args[1].kind);
                // Position 2: checked value ("blue")
                assert_eq!(args[2].kind, ParsedArgKind::String,
                    "arg 2 should be String (checkedValue), got: {:?}", args[2].kind);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase2b_radiobutton_variable_at_position_zero() {
        // <<radiobutton "$color" "blue">>
        // The variable "$color" should be at position 0 (VariableRef),
        // "blue" at position 1 (checkedValue, String).
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<radiobutton "$color" "blue">>"#, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Macro node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "radiobutton");
                let args = structured_args.as_ref().expect("radiobutton should have structured_args");
                assert_eq!(args.len(), 2, "radiobutton should have 2 structured args, got: {:?}", args);
                // Position 0: variable reference ($color)
                assert_eq!(args[0].kind, ParsedArgKind::VariableRef,
                    "arg 0 should be VariableRef (the receiver variable), got: {:?}", args[0].kind);
                // Position 1: checked value ("blue")
                assert_eq!(args[1].kind, ParsedArgKind::String,
                    "arg 1 should be String (checkedValue), got: {:?}", args[1].kind);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase2b_checkbox_var_refs_extracted_from_receiver_arg() {
        // The receiver variable "$color" should appear in the macro's var_refs
        // (it's a write target). This verifies the catalog fix enables proper
        // variable extraction — previously the "label" arg at position 0
        // misclassified the variable as a display label.
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<checkbox "$color" "red" "blue">>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { var_refs, .. } => {
                assert!(!var_refs.is_empty(),
                    "checkbox should have var_refs for the receiver variable, got: {:?}", var_refs);
                // VarRef.name includes the sigil (e.g., "$color", not "color").
                let has_color = var_refs.iter().any(|v| v.name == "$color");
                assert!(has_color,
                    "var_refs should include '$color' (the receiver variable), got: {:?}", var_refs);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase2b_radiobutton_var_refs_extracted_from_receiver_arg() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<radiobutton "$color" "blue">>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { var_refs, .. } => {
                assert!(!var_refs.is_empty(),
                    "radiobutton should have var_refs for the receiver variable, got: {:?}", var_refs);
                let has_color = var_refs.iter().any(|v| v.name == "$color");
                assert!(has_color,
                    "var_refs should include '$color' (the receiver variable), got: {:?}", var_refs);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase2b_checkbox_snippet_has_correct_value_order() {
        // Verify the checkbox snippet has unchecked THEN checked (not swapped).
        use crate::sugarcube::macros::macro_snippet;
        let snippet = macro_snippet("checkbox").expect("checkbox should have a snippet");
        // The snippet should have unchecked at placeholder 2 and checked at placeholder 3.
        assert!(snippet.contains(r#""${2:unchecked}""#),
            "checkbox snippet should have unchecked at placeholder 2, got: {}", snippet);
        assert!(snippet.contains(r#""${3:checked}""#),
            "checkbox snippet should have checked at placeholder 3, got: {}", snippet);
    }

    #[test]
    fn phase2b_checkbox_completion_form_has_correct_value_order() {
        // Verify the CHECKBOX_FORMS completion form has unchecked THEN checked.
        use crate::sugarcube::macros::macro_completion_forms;
        let forms = macro_completion_forms("checkbox").expect("checkbox should have completion forms");
        let primary = forms.iter().find(|f| f.sort_priority == 0)
            .expect("checkbox should have a primary form (sort_priority 0)");
        assert!(primary.label.contains(r#""unchecked" "checked""#),
            "primary checkbox form label should have unchecked THEN checked, got: {}", primary.label);
        assert!(primary.snippet.contains(r#""${2:unchecked}""#),
            "primary checkbox form snippet should have unchecked at placeholder 2, got: {}", primary.snippet);
        assert!(primary.snippet.contains(r#""${3:checked}""#),
            "primary checkbox form snippet should have checked at placeholder 3, got: {}", primary.snippet);
    }

    // ── Phase 3 tests (plan.md §7.3) — Headings (`!` through `!!!!!!`) ─────
    //
    // These tests verify heading parsing per SugarCube's `heading` parser
    // (plan.md §3.5):
    //   - 1-6 `!` characters at column 0 produce a Heading node.
    //   - Level = number of `!` (1=h1, 2=h2, ..., 6=h6).
    //   - A 7th `!` becomes the first character of heading content.
    //   - NO leading whitespace allowed (column-0 anchored).
    //   - Content is recursively parsed — macros, variables, and links
    //     INSIDE heading text ARE processed (heading is a container, not
    //     a raw zone).
    //   - No required space after `!` run — `!Heading` and `! Heading`
    //     are both valid.

    #[test]
    fn phase3_heading_level_1() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "!Hello World", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, span } => {
                assert_eq!(*level, 1, "level should be 1 for single '!'");
                assert_eq!(span.start, 0, "span should start at 0");
                assert_eq!(span.end, 12, "span should cover '!Hello World' (12 bytes)");
                // Content "Hello World" should be a single Text node.
                assert_eq!(children.len(), 1, "expected 1 child (Text), got: {:?}", children);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "Hello World"),
                    "child should be Text('Hello World'), got: {:?}", children[0]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_level_3() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "!!!Section Title", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, .. } => {
                assert_eq!(*level, 3, "level should be 3 for '!!!'");
                assert_eq!(children.len(), 1, "expected 1 child (Text), got: {:?}", children);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "Section Title"),
                    "child should be Text('Section Title'), got: {:?}", children[0]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_level_6_max() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "!!!!!!Deepest", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, .. } => {
                assert_eq!(*level, 6, "level should be 6 for '!!!!!!'");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "Deepest"),
                    "child should be Text('Deepest'), got: {:?}", children[0]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_seventh_bang_becomes_content() {
        // !!! !!!! is 7 `!` — 6 consumed as marker (level 6), 7th is content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "!!!!!!!Seven", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, .. } => {
                assert_eq!(*level, 6, "level should be capped at 6 even with 7 '!'");
                // Content should start with the 7th '!'.
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "!Seven"),
                    "7th '!' should be content: expected Text('!Seven'), got: {:?}", children[0]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_with_space_after_bangs() {
        // `! Heading` — the space becomes part of the content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Heading", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, .. } => {
                assert_eq!(*level, 1);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == " Heading"),
                    "space after '!' should be part of content, got: {:?}", children[0]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_with_leading_whitespace_not_a_heading() {
        // ` ! Heading` — leading space means NOT at column 0, so NOT a heading.
        // Falls through to plain Text.
        let ast = crate::sugarcube::parser::parse_passage_body(
            " ! Heading", 0, ParseMode::Normal,
        );
        // Should be a single Text node (the `!` is just text mid-line).
        assert_eq!(ast.nodes.len(), 1, "expected single Text node, got: {:?}", ast.nodes);
        assert!(matches!(&ast.nodes[0], AstNode::Text { .. }),
            "leading space should prevent heading parsing, got: {:?}", ast.nodes[0]);
        // Verify no Heading nodes.
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Heading { .. })),
            "should NOT have a Heading node (leading space)");
    }

    #[test]
    fn phase3_heading_mid_line_not_a_heading() {
        // `text ! not a heading` — `!` is mid-line, not at column 0.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "text ! not a heading", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Heading { .. })),
            "mid-line '!' should NOT produce a Heading, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase3_heading_macros_execute_inside() {
        // CRITICAL: macros inside heading text ARE processed (per §3.5).
        // `! Some <<set $x to 1>> heading` — the <<set>> is a real Macro node.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Some <<set $x to 1>> heading", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, .. } => {
                assert_eq!(*level, 1);
                // Should have: Text(" Some ") + Macro("set") + Text(" heading")
                // (the content starts with a space after `!`, so first Text is " Some ")
                assert_eq!(children.len(), 3, "expected 3 children (Text + Macro + Text), got: {:?}", children);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == " Some "),
                    "first child should be Text(' Some ') — content starts with space after '!', got: {:?}", children[0]);
                assert!(matches!(&children[1], AstNode::Macro { name, .. } if name == "set"),
                    "second child should be Macro('set'), got: {:?}", children[1]);
                assert!(matches!(&children[2], AstNode::Text { content, .. } if content == " heading"),
                    "third child should be Text(' heading'), got: {:?}", children[2]);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_with_variable_reference() {
        // Variables inside heading text ARE interpolated (not literal).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Hello $name", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { children, .. } => {
                // Should have a Text node containing "Hello $name" — the $name
                // is an inline var ref extracted from the text gap.
                let text_node = children.iter().find(|n| matches!(n, AstNode::Text { .. }));
                assert!(text_node.is_some(), "expected a Text child, got: {:?}", children);
                if let Some(AstNode::Text { content, var_refs, .. }) = text_node {
                    assert!(content.contains("$name"), "content should contain '$name': {}", content);
                    assert!(var_refs.iter().any(|v| v.name == "$name"),
                        "var_refs should include '$name', got: {:?}", var_refs);
                }
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_with_link() {
        // Links inside heading text ARE processed.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Go to [[Forest]]", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { children, .. } => {
                let has_link = children.iter().any(|n| matches!(n, AstNode::Link { .. }));
                assert!(has_link, "expected a Link child inside heading, got: {:?}", children);
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_followed_by_text_on_next_line() {
        // Heading on line 1, prose on line 2 — both should parse correctly.
        // The heading span ends at the `\n` (exclusive). The `\n` is then
        // consumed by the main loop and merges with the next line's prose
        // into a single Text node.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Title\nSome prose text.\n", 0, ParseMode::Normal,
        );
        // Expected: Heading + Text (the \n + prose + \n merged into one Text node)
        assert_eq!(ast.nodes.len(), 2, "expected 2 nodes (Heading + Text), got: {:?}", ast.nodes);
        // First node: Heading
        assert!(matches!(&ast.nodes[0], AstNode::Heading { level, .. } if *level == 1),
            "first node should be Heading level 1, got: {:?}", ast.nodes[0]);
        // The heading span should NOT include the \n.
        if let AstNode::Heading { span, .. } = &ast.nodes[0] {
            assert_eq!(span.end, 7, "heading span should end at 7 (before \\n), got: {:?}", span);
        }
        // Second node: Text containing the prose (the \n merges into it).
        let has_prose = ast.nodes.iter().any(|n| {
            matches!(n, AstNode::Text { content, .. } if content.contains("Some prose text"))
        });
        assert!(has_prose, "expected prose text after heading, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase3_heading_no_trailing_newline() {
        // Heading at end of text with no trailing \n.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "! Last heading", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Heading node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Heading { level, children, span } => {
                assert_eq!(*level, 1);
                assert_eq!(span.end, 14, "span should cover the full '! Last heading' (14 bytes)");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == " Last heading"));
            }
            other => panic!("expected Heading, got {:?}", other),
        }
    }

    #[test]
    fn phase3_heading_emits_heading_token_and_content_tokens() {
        // Verify the token builder emits a Heading token for the `!` run
        // AND prose tokens for the content.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n! Hello World\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let heading_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Heading))
            .collect();
        assert_eq!(heading_tokens.len(), 1,
            "expected exactly 1 Heading token, got {}: {:?}",
            heading_tokens.len(), heading_tokens);

        let tok = &heading_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, "!",
            "Heading token should cover just the '!' marker, got: {:?}", token_text);

        // There should also be a Prose token for "Hello World".
        let prose_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Prose))
            .collect();
        assert!(!prose_tokens.is_empty(),
            "expected at least 1 Prose token for heading content, got 0");
    }

    #[test]
    fn phase3_heading_with_macro_emits_macro_token() {
        // Verify macros inside headings get their own token (recursive tokenization).
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n! <<set $x to 1>> heading\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        // Should have a Heading token for `!`.
        let heading_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Heading))
            .collect();
        assert_eq!(heading_tokens.len(), 1, "expected 1 Heading token");

        // Should have a Macro token for `set` (proving the heading's children
        // were recursively tokenized).
        let macro_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!macro_tokens.is_empty(),
            "expected at least 1 Macro token for <<set>> inside heading, got 0");
    }

    // ── Phase 4 tests — Horizontal rule + blockquotes ─────────────────────
    //
    // Tests for:
    //   - Horizontal rule (`----`, 4+ dashes alone on a line at col 0)
    //   - Line-style blockquote (`>`, `>>`, etc. at col 0, recursive content)
    //   - Block-style blockquote (`<<<\n...\n<<<`, undocumented but in source)

    // ── Horizontal rule tests ─────────────────────────────────────────────

    #[test]
    fn phase4_horizontal_rule_basic() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "----", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single HorizontalRule node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::HorizontalRule { span } => {
                assert_eq!(span.start, 0);
                assert_eq!(span.end, 4, "span should cover the 4 dashes");
            }
            other => panic!("expected HorizontalRule, got {:?}", other),
        }
    }

    #[test]
    fn phase4_horizontal_rule_five_dashes() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "-----", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::HorizontalRule { span } => {
                assert_eq!(span.end, 5, "span should cover 5 dashes");
            }
            other => panic!("expected HorizontalRule, got {:?}", other),
        }
    }

    #[test]
    fn phase4_horizontal_rule_with_trailing_whitespace() {
        // `----   ` — trailing whitespace is allowed, NOT part of span.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "----   ", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::HorizontalRule { span } => {
                assert_eq!(span.end, 4, "span should cover only the 4 dashes, not trailing whitespace");
            }
            other => panic!("expected HorizontalRule, got {:?}", other),
        }
    }

    #[test]
    fn phase4_three_dashes_not_a_horizontal_rule() {
        // `---` (3 dashes) is NOT a horizontal rule.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "---", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::HorizontalRule { .. })),
            "--- should NOT be a HorizontalRule, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_horizontal_rule_with_trailing_text_not_a_hr() {
        // `---- text` — trailing non-whitespace means NOT a horizontal rule.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "---- text", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::HorizontalRule { .. })),
            "---- text should NOT be a HorizontalRule (trailing non-whitespace), got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_horizontal_rule_with_leading_space_not_a_hr() {
        // ` ----` — leading space means NOT at column 0, so NOT a horizontal rule.
        let ast = crate::sugarcube::parser::parse_passage_body(
            " ----", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::HorizontalRule { .. })),
            "leading space should prevent HR parsing, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_horizontal_rule_emits_token() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n----\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let hr_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::HorizontalRule))
            .collect();
        assert_eq!(hr_tokens.len(), 1, "expected 1 HorizontalRule token");
        let tok = &hr_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, "----", "HorizontalRule token should cover the 4 dashes, got: {:?}", token_text);
    }

    // ── Line-style blockquote tests ───────────────────────────────────────

    #[test]
    fn phase4_blockquote_line_depth_1() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            ">Some text", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Blockquote node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Blockquote { depth, children, span } => {
                assert_eq!(*depth, 1, "depth should be 1 for single '>'");
                assert_eq!(span.start, 0);
                assert_eq!(span.end, 10, "span should cover '>Some text'");
                assert_eq!(children.len(), 1, "expected 1 child (Text)");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "Some text"),
                    "child should be Text('Some text'), got: {:?}", children[0]);
            }
            other => panic!("expected Blockquote, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_line_depth_2() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            ">>Nested", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Blockquote { depth, children, .. } => {
                assert_eq!(*depth, 2, "depth should be 2 for '>>'");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "Nested"));
            }
            other => panic!("expected Blockquote, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_line_with_space_after_marker() {
        // `> Text` — space becomes part of content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "> Text", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Blockquote { depth, children, .. } => {
                assert_eq!(*depth, 1);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == " Text"),
                    "space after '>' should be part of content, got: {:?}", children[0]);
            }
            other => panic!("expected Blockquote, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_line_with_leading_space_not_a_blockquote() {
        // ` > text` — leading space means NOT at column 0.
        let ast = crate::sugarcube::parser::parse_passage_body(
            " > text", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Blockquote { .. })),
            "leading space should prevent blockquote parsing, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_blockquote_line_macros_execute_inside() {
        // Macros inside blockquote content ARE processed.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "> Hello <<set $x to 1>> world", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Blockquote { children, .. } => {
                let has_macro = children.iter().any(|n| matches!(n, AstNode::Macro { name, .. } if name == "set"));
                assert!(has_macro, "expected Macro('set') child inside blockquote, got: {:?}", children);
            }
            other => panic!("expected Blockquote, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_line_with_link() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "> Go to [[Forest]]", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Blockquote { children, .. } => {
                let has_link = children.iter().any(|n| matches!(n, AstNode::Link { .. }));
                assert!(has_link, "expected Link child inside blockquote, got: {:?}", children);
            }
            other => panic!("expected Blockquote, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_line_emits_token_and_content() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n> Hello world\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let bq_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Blockquote))
            .collect();
        assert_eq!(bq_tokens.len(), 1, "expected 1 Blockquote token");
        let tok = &bq_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, ">", "Blockquote token should cover just the '>' marker, got: {:?}", token_text);

        // Should also have Prose tokens for content.
        let prose_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Prose))
            .collect();
        assert!(!prose_tokens.is_empty(), "expected Prose tokens for blockquote content");
    }

    // ── Block-style blockquote tests (`<<<...<<<`) ────────────────────────

    #[test]
    fn phase4_blockquote_block_basic() {
        let text = "<<<\nSome content\n<<<";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single BlockquoteBlock node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::BlockquoteBlock { children, open_span, close_span, span } => {
                assert!(open_span.start == 0 && open_span.end == 3, "open_span should cover opening '<<<': {:?}", open_span);
                assert!(close_span.is_some(), "close_span should be Some (block was closed)");
                // Content should include "Some content\n"
                let has_content = children.iter().any(|n| {
                    matches!(n, AstNode::Text { content, .. } if content.contains("Some content"))
                });
                assert!(has_content, "expected content 'Some content' in children, got: {:?}", children);
                // Full span should cover from opening <<< to end of closing <<<.
                assert_eq!(span.start, 0, "full span should start at 0");
                assert_eq!(span.end, text.len(), "full span should cover the entire construct");
            }
            other => panic!("expected BlockquoteBlock, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_block_with_macros_inside() {
        // Macros inside block-style blockquote ARE processed.
        let text = "<<<\n<<set $x to 1>>\n<<<";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::BlockquoteBlock { children, .. } => {
                let has_macro = children.iter().any(|n| matches!(n, AstNode::Macro { name, .. } if name == "set"));
                assert!(has_macro, "expected Macro('set') inside blockquote block, got: {:?}", children);
            }
            other => panic!("expected BlockquoteBlock, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_block_unclosed() {
        // Unclosed blockquote block — consumes to end of text.
        let text = "<<<\nSome unclosed content\nmore text";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single BlockquoteBlock node, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::BlockquoteBlock { close_span, children, .. } => {
                assert!(close_span.is_none(), "unclosed block should have close_span = None");
                let has_content = children.iter().any(|n| {
                    matches!(n, AstNode::Text { content, .. } if content.contains("Some unclosed content"))
                });
                assert!(has_content, "expected unclosed content in children, got: {:?}", children);
            }
            other => panic!("expected BlockquoteBlock, got {:?}", other),
        }
    }

    #[test]
    fn phase4_blockquote_block_disambiguation_from_macro() {
        // `<<<\n` at col 0 is a blockquote block, NOT a macro.
        // Verify no Macro node is produced for the opening `<<<`.
        let text = "<<<\ncontent\n<<<";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        // Should NOT have any top-level Macro nodes (the `<<<` is not a macro).
        let has_macro = ast.nodes.iter().any(|n| matches!(n, AstNode::Macro { .. }));
        assert!(!has_macro, "<<< should NOT be parsed as a macro, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_triple_less_not_at_col_zero_is_macro() {
        // `text <<<\n` — `<<<` not at col 0 should be parsed as macro `<<` + `<`.
        // (This is unusual but the `<<` macro arm handles it.)
        let text = "text <<<\n";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        // Should NOT have a BlockquoteBlock (not at col 0).
        let has_bqb = ast.nodes.iter().any(|n| matches!(n, AstNode::BlockquoteBlock { .. }));
        assert!(!has_bqb, "<<< not at col 0 should NOT be a BlockquoteBlock, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase4_blockquote_block_emits_tokens() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<<\nContent here\n<<<\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let bqb_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::BlockquoteBlock))
            .collect();
        assert_eq!(bqb_tokens.len(), 2, "expected 2 BlockquoteBlock tokens (open + close), got: {}", bqb_tokens.len());

        // Both tokens should cover `<<<`.
        for tok in &bqb_tokens {
            let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
            assert_eq!(token_text, "<<<", "BlockquoteBlock token should cover '<<<', got: {:?}", token_text);
        }

        // Should also have Prose tokens for content.
        let prose_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Prose))
            .collect();
        assert!(!prose_tokens.is_empty(), "expected Prose tokens for blockquote block content");
    }

    #[test]
    fn phase4_blockquote_block_multi_paragraph() {
        // Block-style blockquote can span multiple paragraphs.
        let text = "<<<\nParagraph 1\n\nParagraph 2\n<<<";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single BlockquoteBlock, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::BlockquoteBlock { children, close_span, .. } => {
                assert!(close_span.is_some(), "block should be closed");
                // Should contain text from both paragraphs.
                let all_text: String = children.iter()
                    .filter_map(|n| match n {
                        AstNode::Text { content, .. } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                assert!(all_text.contains("Paragraph 1"), "missing 'Paragraph 1' in: {}", all_text);
                assert!(all_text.contains("Paragraph 2"), "missing 'Paragraph 2' in: {}", all_text);
            }
            other => panic!("expected BlockquoteBlock, got {:?}", other),
        }
    }

    // ── Phase 5 tests — Lists (`*` / `#`) ─────────────────────────────────
    //
    // Tests for SugarCube list parsing (plan.md §3.7):
    //   - `*` = unordered (ul), `#` = ordered (ol).
    //   - Depth = marker char count (NOT indentation).
    //   - NO mixed markers (`*#` not supported).
    //   - NO leading whitespace allowed.
    //   - Content is recursively parsed (macros execute inside).

    #[test]
    fn phase5_unordered_list_depth_1() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "*item", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single ListItem, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::ListItem { depth, ordered, marker, children, span } => {
                assert_eq!(*depth, 1, "depth should be 1");
                assert!(!*ordered, "should be unordered (false)");
                assert_eq!(marker, "*", "marker should be '*'");
                assert_eq!(span.start, 0);
                assert_eq!(span.end, 5, "span should cover '*item'");
                assert_eq!(children.len(), 1);
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "item"),
                    "child should be Text('item'), got: {:?}", children[0]);
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_ordered_list_depth_1() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "#item", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single ListItem, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::ListItem { depth, ordered, marker, children, .. } => {
                assert_eq!(*depth, 1);
                assert!(*ordered, "should be ordered (true)");
                assert_eq!(marker, "#", "marker should be '#'");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "item"));
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_unordered_list_depth_2() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "**nested", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { depth, ordered, marker, children, .. } => {
                assert_eq!(*depth, 2, "depth should be 2 for '**'");
                assert!(!*ordered);
                assert_eq!(marker, "**", "marker should be '**'");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "nested"));
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_ordered_list_depth_3() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "###deep", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { depth, ordered, marker, children, .. } => {
                assert_eq!(*depth, 3, "depth should be 3 for '###'");
                assert!(*ordered);
                assert_eq!(marker, "###");
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "deep"));
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_list_with_space_after_marker() {
        // `* item` — space becomes part of content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "* item", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { children, .. } => {
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == " item"),
                    "space after '*' should be part of content, got: {:?}", children[0]);
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_list_with_leading_space_not_a_list() {
        // ` *item` — leading space means NOT at column 0.
        let ast = crate::sugarcube::parser::parse_passage_body(
            " *item", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::ListItem { .. })),
            "leading space should prevent list parsing, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase5_mixed_markers_not_supported() {
        // `*#item` — mixed markers NOT supported. Only `*` matches (depth 1),
        // the `#` becomes literal content.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "*#item", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single ListItem, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::ListItem { depth, ordered, marker, children, .. } => {
                assert_eq!(*depth, 1, "only the first '*' should match");
                assert!(!*ordered, "should be unordered (only * matched)");
                assert_eq!(marker, "*");
                // Content should start with '#' (the unmatched marker).
                assert!(matches!(&children[0], AstNode::Text { content, .. } if content == "#item"),
                    "'#' should be literal content (not a marker), got: {:?}", children[0]);
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_list_macros_execute_inside() {
        // Macros inside list item content ARE processed.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "* Hello <<set $x to 1>> world", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { children, .. } => {
                let has_macro = children.iter().any(|n| matches!(n, AstNode::Macro { name, .. } if name == "set"));
                assert!(has_macro, "expected Macro('set') child inside list item, got: {:?}", children);
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_list_with_variable_reference() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "* Hello $name", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { children, .. } => {
                let text_node = children.iter().find(|n| matches!(n, AstNode::Text { .. }));
                assert!(text_node.is_some());
                if let Some(AstNode::Text { content, var_refs, .. }) = text_node {
                    assert!(content.contains("$name"));
                    assert!(var_refs.iter().any(|v| v.name == "$name"),
                        "var_refs should include '$name', got: {:?}", var_refs);
                }
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_list_with_link() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "* Go to [[Forest]]", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::ListItem { children, .. } => {
                let has_link = children.iter().any(|n| matches!(n, AstNode::Link { .. }));
                assert!(has_link, "expected Link child inside list item, got: {:?}", children);
            }
            other => panic!("expected ListItem, got {:?}", other),
        }
    }

    #[test]
    fn phase5_multiple_list_items() {
        // Multiple list items on consecutive lines.
        let text = "* first\n* second\n* third";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        let list_items: Vec<_> = ast.nodes.iter()
            .filter(|n| matches!(n, AstNode::ListItem { .. }))
            .collect();
        assert_eq!(list_items.len(), 3, "expected 3 ListItem nodes, got: {}", list_items.len());
    }

    #[test]
    fn phase5_nested_list_items() {
        // Nested list items with varying depth.
        let text = "* top\n** nested\n*** deeper";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        let list_items: Vec<_> = ast.nodes.iter()
            .filter_map(|n| match n {
                AstNode::ListItem { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(list_items, vec![1, 2, 3], "expected depths 1, 2, 3, got: {:?}", list_items);
    }

    #[test]
    fn phase5_mixed_ul_and_ol() {
        // Mixed unordered and ordered list items (same-depth type switching).
        let text = "* ul item\n## ol item";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        let list_items: Vec<_> = ast.nodes.iter()
            .filter_map(|n| match n {
                AstNode::ListItem { ordered, depth, .. } => Some((*ordered, *depth)),
                _ => None,
            })
            .collect();
        assert_eq!(list_items.len(), 2);
        assert_eq!(list_items[0], (false, 1), "first item should be ul depth 1");
        assert_eq!(list_items[1], (true, 2), "second item should be ol depth 2");
    }

    #[test]
    fn phase5_list_mid_line_asterisk_not_a_list() {
        // `text * not a list` — `*` is mid-line, not at column 0.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "text * not a list", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::ListItem { .. })),
            "mid-line '*' should NOT produce a ListItem, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase5_list_emits_listmarker_token_and_content() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n* item text\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let list_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::ListMarker))
            .collect();
        assert_eq!(list_tokens.len(), 1, "expected 1 ListMarker token");
        let tok = &list_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, "*", "ListMarker token should cover just the '*' marker, got: {:?}", token_text);

        // Should also have Prose tokens for content.
        let prose_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Prose))
            .collect();
        assert!(!prose_tokens.is_empty(), "expected Prose tokens for list item content");
    }

    #[test]
    fn phase5_ordered_list_emits_listmarker_token() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n## numbered\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let list_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::ListMarker))
            .collect();
        assert_eq!(list_tokens.len(), 1, "expected 1 ListMarker token");
        let tok = &list_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, "##", "ListMarker token should cover '##', got: {:?}", token_text);
    }

    #[test]
    fn phase5_list_with_macro_emits_macro_token() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n* <<set $x to 1>> done\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        // Should have a ListMarker token for `*`.
        let list_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::ListMarker))
            .collect();
        assert_eq!(list_tokens.len(), 1, "expected 1 ListMarker token");

        // Should have a Macro token for `set` (proving recursive tokenization).
        let macro_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!macro_tokens.is_empty(),
            "expected at least 1 Macro token for <<set>> inside list item, got 0");
    }

    // ── Phase 6 tests — Tables (TiddlyWiki syntax) ───────────────────────
    //
    // Tests for SugarCube table parsing (plan.md §3.9):
    //   - Rows: `|cell|cell|...|[fhck]?` at column 0.
    //   - Row-type suffix: h (header), f (footer), c (caption), k (class).
    //   - Header cells: cell content starting with `!` → `<th>`.
    //   - Colspan: cell content `>` only. Rowspan: `~` only.
    //   - Cell content recursively parsed (macros execute).

    #[test]
    fn phase6_table_basic_body_row() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "|cell1|cell2|", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Table, got: {:?}", ast.nodes);
        match &ast.nodes[0] {
            AstNode::Table { rows, header, footer, caption, class, .. } => {
                assert_eq!(rows.len(), 1, "expected 1 row");
                assert!(header.is_none(), "no header row expected");
                assert!(footer.is_none());
                assert!(caption.is_none());
                assert!(class.is_none());
                let row = &rows[0];
                assert_eq!(row.cells.len(), 2, "expected 2 cells");
                assert!(matches!(row.row_type, crate::sugarcube::ast::TableRowType::Body));
                // Cell content should be Text nodes.
                assert!(matches!(&row.cells[0].children[0], AstNode::Text { content, .. } if content == "cell1"));
                assert!(matches!(&row.cells[1].children[0], AstNode::Text { content, .. } if content == "cell2"));
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_multiple_rows() {
        let text = "|r1c1|r1c2|\n|r2c1|r2c2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                assert_eq!(rows.len(), 2, "expected 2 rows");
                assert!(matches!(&rows[0].cells[0].children[0], AstNode::Text { content, .. } if content == "r1c1"));
                assert!(matches!(&rows[1].cells[1].children[0], AstNode::Text { content, .. } if content == "r2c2"));
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_header_row() {
        let text = "|!H1|!H2|h\n|b1|b2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, header, .. } => {
                assert!(header.is_some(), "expected header row");
                assert_eq!(rows.len(), 2, "expected 2 rows (header + body)");
                // Header cells should have is_header=true.
                let h = header.as_ref().unwrap();
                assert!(h.cells.iter().all(|c| c.is_header), "all header cells should have is_header=true");
                // Body row cells should have is_header=false.
                let body = &rows[1];
                assert!(body.cells.iter().all(|c| !c.is_header), "body cells should have is_header=false");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_footer_row() {
        let text = "|b1|b2|\n|f1|f2|f";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, footer, .. } => {
                assert!(footer.is_some(), "expected footer row");
                assert_eq!(rows.len(), 2, "expected 2 rows (body + footer)");
                assert!(matches!(rows[1].row_type, crate::sugarcube::ast::TableRowType::Footer));
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_caption_row() {
        let text = "|My Caption|c\n|b1|b2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { caption, rows, .. } => {
                assert!(caption.is_some(), "expected caption");
                assert_eq!(caption.as_deref(), Some("My Caption"), "caption text mismatch");
                // Caption row is NOT stored in rows (only body/header/footer are).
                assert_eq!(rows.len(), 1, "expected 1 body row (caption row not in rows)");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_class_row() {
        let text = "|myclass|k\n|b1|b2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { class, rows, .. } => {
                assert!(class.is_some(), "expected class");
                assert_eq!(class.as_deref(), Some("myclass"), "class text mismatch");
                assert_eq!(rows.len(), 1, "expected 1 body row");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_colspan_cell() {
        let text = "|>|b2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                let row = &rows[0];
                assert!(row.cells[0].colspan, "first cell should have colspan=true");
                assert!(!row.cells[1].colspan, "second cell should have colspan=false");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_rowspan_cell() {
        let text = "|~|b2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                let row = &rows[0];
                assert!(row.cells[0].rowspan, "first cell should have rowspan=true");
                assert!(!row.cells[1].rowspan, "second cell should have rowspan=false");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_not_a_table_row_no_closing_pipe() {
        // `| not a table row` — no closing `|`, so NOT a table.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "| not a table row", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Table { .. })),
            "should NOT be a Table (no closing |), got: {:?}", ast.nodes);
    }

    #[test]
    fn phase6_table_not_a_table_invalid_suffix() {
        // `|cell|x` — suffix `x` is not h/f/c/k, so NOT a table row.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "|cell|x", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Table { .. })),
            "should NOT be a Table (invalid suffix x), got: {:?}", ast.nodes);
    }

    #[test]
    fn phase6_table_with_leading_space_not_a_table() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            " |cell|", 0, ParseMode::Normal,
        );
        assert!(!ast.nodes.iter().any(|n| matches!(n, AstNode::Table { .. })),
            "leading space should prevent table parsing, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase6_table_macros_execute_inside_cells() {
        let text = "|<<set $x to 1>>|cell2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                let row = &rows[0];
                let has_macro = row.cells[0].children.iter()
                    .any(|n| matches!(n, AstNode::Macro { name, .. } if name == "set"));
                assert!(has_macro, "expected Macro('set') in first cell, got: {:?}", row.cells[0].children);
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_with_variable_in_cell() {
        let text = "|Hello $name|cell2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                let cell = &rows[0].cells[0];
                let text_node = cell.children.iter().find(|n| matches!(n, AstNode::Text { .. }));
                assert!(text_node.is_some());
                if let Some(AstNode::Text { var_refs, .. }) = text_node {
                    assert!(var_refs.iter().any(|v| v.name == "$name"),
                        "expected '$name' in var_refs, got: {:?}", var_refs);
                }
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_with_link_in_cell() {
        let text = "|[[Forest]]|cell2|";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                let has_link = rows[0].cells[0].children.iter()
                    .any(|n| matches!(n, AstNode::Link { .. }));
                assert!(has_link, "expected Link in first cell");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    #[test]
    fn phase6_table_followed_by_text() {
        // Table on lines 1-2, prose on line 3.
        let text = "|r1c1|r1c2|\n|r2c1|r2c2|\nSome prose.";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        // Should have: Table + Text (prose).
        let has_table = ast.nodes.iter().any(|n| matches!(n, AstNode::Table { .. }));
        let has_prose = ast.nodes.iter().any(|n| {
            matches!(n, AstNode::Text { content, .. } if content.contains("Some prose"))
        });
        assert!(has_table, "expected a Table node");
        assert!(has_prose, "expected prose text after table, got: {:?}", ast.nodes);
    }

    #[test]
    fn phase6_table_emits_table_token_and_content() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n|cell1|cell2|\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let table_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Table))
            .collect();
        assert_eq!(table_tokens.len(), 1, "expected 1 Table token (opening |)");
        let tok = &table_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert_eq!(token_text, "|", "Table token should cover opening |, got: {:?}", token_text);

        // Should also have Prose tokens for cell content.
        let prose_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Prose))
            .collect();
        assert!(!prose_tokens.is_empty(), "expected Prose tokens for cell content");
    }

    #[test]
    fn phase6_table_with_macro_emits_macro_token() {
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n|<<set $x to 1>>|cell|\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        // Should have a Table token for opening `|`.
        let table_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Table))
            .collect();
        assert_eq!(table_tokens.len(), 1, "expected 1 Table token");

        // Should have a Macro token for `set` (proving cell content was recursively tokenized).
        let macro_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!macro_tokens.is_empty(),
            "expected at least 1 Macro token for <<set>> inside table cell, got 0");
    }

    #[test]
    fn phase6_table_empty_cells() {
        // `|||` — two empty cells.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "|||", 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Table { rows, .. } => {
                assert_eq!(rows[0].cells.len(), 2, "expected 2 empty cells");
                assert!(rows[0].cells.iter().all(|c| c.children.is_empty()),
                    "empty cells should have no children");
            }
            other => panic!("expected Table, got {:?}", other),
        }
    }

    // ── Phase 7a tests — Generalize raw-body mechanism ───────────────────
    //
    // Tests for:
    //   - `<<script>>` still has raw body (catalog-driven, not hardcoded).
    //   - `<<style>>` and `<<css>>` are no longer recognized as macros.
    //   - `body_is_raw` field is catalog-driven.

    #[test]
    fn phase7a_script_still_has_raw_body() {
        // <<script>> should still capture its body as raw text (no macro parsing).
        let text = "<<script>>\n<<set $x to 1>>\n<</script>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Macro node");
        match &ast.nodes[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "script");
                // The body should be captured as a raw Text child (not parsed).
                assert!(children.is_some(), "script should have children (raw body)");
                let ch = children.as_ref().unwrap();
                // The <<set>> inside should NOT be a Macro child — it should
                // be part of the raw Text content.
                let has_macro_child = ch.iter().any(|n| matches!(n, AstNode::Macro { .. }));
                assert!(!has_macro_child,
                    "CRITICAL: <<set>> inside <<script>> should NOT be parsed as a Macro! children: {:?}", ch);
                // The raw text should contain the <<set>> literally.
                let has_text_with_set = ch.iter().any(|n| {
                    matches!(n, AstNode::Text { content, .. } if content.contains("<<set $x to 1>>"))
                });
                assert!(has_text_with_set, "expected raw Text containing '<<set>>', got: {:?}", ch);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7a_css_not_a_macro() {
        // `<<css>>` was removed from the catalog — it doesn't exist in SugarCube.
        // It should now be parsed as a regular macro with no special raw-body
        // handling (the tree builder will try to pair it with <</css>> if present).
        let text = "<<css>>\nbody { color: red; }\n<</css>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        // `<<css>>` is no longer in the catalog, so it's treated as an unknown
        // macro. It should still be parsed (the parser handles unknown macros),
        // but it should NOT have raw body — its children should be parsed
        // normally (macros/links/vars inside would be processed).
        let css_macro = ast.nodes.iter().find(|n| {
            matches!(n, AstNode::Macro { name, .. } if name == "css")
        });
        assert!(css_macro.is_some(), "expected a Macro named 'css' (unknown macro, still parsed)");
        if let Some(AstNode::Macro { children, .. }) = css_macro {
            // Children should be parsed normally (not raw Text).
            // The body "body { color: red; }\n" should be a Text node (prose).
            if let Some(ch) = children {
                let has_text = ch.iter().any(|n| matches!(n, AstNode::Text { .. }));
                assert!(has_text, "expected Text children (not raw body) for unknown 'css' macro");
            }
        }
    }

    #[test]
    fn phase7a_style_not_in_catalog() {
        // `<<style>>` was never in the catalog — it was only in the hardcoded
        // parser check. Now that the check is catalog-driven, `<<style>>` is
        // treated as an unknown macro (no raw body).
        use crate::sugarcube::macros::find_macro;
        assert!(find_macro("style").is_none(), "<<style>> should NOT be in the catalog");
        assert!(find_macro("css").is_none(), "<<css>> should NOT be in the catalog");
        assert!(find_macro("script").is_some(), "<<script>> should be in the catalog");
    }

    #[test]
    fn phase7a_script_body_is_raw_in_catalog() {
        // Verify the catalog has body_is_raw: true for script only.
        use crate::sugarcube::macros::find_macro;
        let script_def = find_macro("script").expect("script should be in catalog");
        assert!(script_def.body_is_raw, "script should have body_is_raw: true");

        // Check a few other macros have body_is_raw: false.
        for name in &["if", "for", "link", "set", "print", "widget"] {
            let def = find_macro(name).expect(&format!("{} should be in catalog", name));
            assert!(!def.body_is_raw, "{} should have body_is_raw: false", name);
        }
    }

    #[test]
    fn phase7a_script_with_macros_outside_still_works() {
        // Macros OUTSIDE <<script>> should still execute normally.
        let text = "<<set $y to 2>><<script>>\nraw code\n<</script>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 2, "expected 2 nodes (Macro + Macro)");
        assert!(matches!(&ast.nodes[0], AstNode::Macro { name, .. } if name == "set"),
            "first node should be Macro('set'), got: {:?}", ast.nodes[0]);
        assert!(matches!(&ast.nodes[1], AstNode::Macro { name, .. } if name == "script"),
            "second node should be Macro('script'), got: {:?}", ast.nodes[1]);
    }

    // ── Phase 7b tests — Add missing macros ──────────────────────────────
    //
    // Tests for:
    //   - <<silent>> (NEW v2.37.0, replacement for <<silently>>).
    //   - <<do>> / <<redo>> (NEW v2.37.0).
    //   - <<choice>> (deprecated v2.37.0 but present).
    //   - <<setplaylist>> / <<stopallaudio>> (removed v2.37.0, kept deprecated).
    //   - All deprecated macros have deprecation messages.

    #[test]
    fn phase7b_silent_in_catalog() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("silent").expect("<<silent>> should be in catalog");
        assert!(!def.deprecated, "<<silent>> should NOT be deprecated (it's the replacement)");
        assert_eq!(def.body, crate::types::BodyRequirement::Required);
        assert_eq!(def.kind, crate::types::MacroKind::Container);
    }

    #[test]
    fn phase7b_do_in_catalog() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("do").expect("<<do>> should be in catalog");
        assert!(!def.deprecated, "<<do>> should NOT be deprecated");
        assert_eq!(def.body, crate::types::BodyRequirement::Required);
        assert_eq!(def.kind, crate::types::MacroKind::Container);
    }

    #[test]
    fn phase7b_redo_in_catalog() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("redo").expect("<<redo>> should be in catalog");
        assert!(!def.deprecated, "<<redo>> should NOT be deprecated");
        assert_eq!(def.body, crate::types::BodyRequirement::Never);
        assert_eq!(def.kind, crate::types::MacroKind::Inline);
    }

    #[test]
    fn phase7b_choice_in_catalog_and_deprecated() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("choice").expect("<<choice>> should be in catalog");
        assert!(def.deprecated, "<<choice>> should be deprecated (v2.37.0)");
        assert!(def.deprecation_message.is_some(), "<<choice>> should have a deprecation message");
        assert_eq!(def.body, crate::types::BodyRequirement::Never);
        assert_eq!(def.kind, crate::types::MacroKind::Inline);
    }

    #[test]
    fn phase7b_setplaylist_in_catalog_and_deprecated() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("setplaylist").expect("<<setplaylist>> should be in catalog (deprecated)");
        assert!(def.deprecated, "<<setplaylist>> should be deprecated");
        assert!(def.deprecation_message.is_some());
    }

    #[test]
    fn phase7b_stopallaudio_in_catalog_and_deprecated() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("stopallaudio").expect("<<stopallaudio>> should be in catalog (deprecated)");
        assert!(def.deprecated, "<<stopallaudio>> should be deprecated");
        assert!(def.deprecation_message.is_some());
    }

    #[test]
    fn phase7b_silently_still_deprecated() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("silently").expect("<<silently>> should still be in catalog (deprecated)");
        assert!(def.deprecated, "<<silently>> should be deprecated");
        assert!(def.deprecation_message.is_some());
    }

    #[test]
    fn phase7b_all_removed_macros_are_deprecated() {
        // Per Q8: keep removed macros but mark them deprecated.
        use crate::sugarcube::macros::find_macro;
        for name in &["click", "display", "forget", "remember", "setplaylist", "stopallaudio", "silently", "choice", "actions"] {
            let def = find_macro(name).unwrap_or_else(|| panic!("{} should be in catalog", name));
            assert!(def.deprecated, "{} should be deprecated", name);
            assert!(def.deprecation_message.is_some(), "{} should have a deprecation message", name);
        }
    }

    #[test]
    fn phase7b_new_macros_have_snippets() {
        // All new macros should have snippets for completion.
        use crate::sugarcube::macros::macro_snippet;
        for name in &["silent", "do", "redo", "choice", "setplaylist", "stopallaudio"] {
            assert!(macro_snippet(name).is_some(), "{} should have a snippet", name);
        }
    }

    #[test]
    fn phase7b_silent_parses_as_block_macro() {
        // <<silent>> should parse as a block macro with body content.
        let text = "<<silent>>\n<<set $x to 1>>\n<</silent>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1, "expected single Macro node");
        match &ast.nodes[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "silent");
                assert!(children.is_some(), "<<silent>> should have children (body content)");
                // The <<set>> inside should be parsed as a real Macro child
                // (silent is NOT raw-body — its content is parsed normally).
                let ch = children.as_ref().unwrap();
                let has_macro = ch.iter().any(|n| matches!(n, AstNode::Macro { name, .. } if name == "set"));
                assert!(has_macro, "expected Macro('set') child inside <<silent>>, got: {:?}", ch);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7b_do_parses_as_block_macro() {
        let text = "<<do>>\nSome content\n<</do>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "do");
                assert!(children.is_some());
                let ch = children.as_ref().unwrap();
                let has_text = ch.iter().any(|n| {
                    matches!(n, AstNode::Text { content, .. } if content.contains("Some content"))
                });
                assert!(has_text, "expected 'Some content' in <<do>> body, got: {:?}", ch);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7b_redo_parses_as_inline_macro() {
        let text = "<<redo>>";
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "redo");
                // redo is inline (body: Never) — children should be None.
                assert!(children.is_none(), "<<redo>> should have no children (inline macro)");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7b_choice_parses_as_inline_macro() {
        let text = r#"<<choice "Forest" "Go to forest">>"#;
        let ast = crate::sugarcube::parser::parse_passage_body(
            text, 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "choice");
                assert!(children.is_none(), "<<choice>> should have no children (inline macro)");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    // ── Phase 7c/7d tests — New MacroArgKind variants + arg schema fixes ──

    #[test]
    fn phase7cd_textbox_has_4_args() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("textbox").expect("textbox should be in catalog");
        let args = def.args.expect("textbox should have args");
        assert_eq!(args.len(), 4, "textbox should have 4 args (receiverName, defaultValue, passage, autofocus)");
        assert_eq!(args[0].label, "receiverName");
        assert_eq!(args[1].label, "defaultValue");
        assert_eq!(args[2].label, "passage");
        assert!(args[2].is_passage_ref, "passage arg should be is_passage_ref");
        assert_eq!(args[3].label, "autofocus");
        assert_eq!(args[3].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7cd_numberbox_has_4_args_with_number_kind() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("numberbox").expect("numberbox should be in catalog");
        let args = def.args.expect("numberbox should have args");
        assert_eq!(args.len(), 4);
        assert_eq!(args[1].label, "defaultValue");
        assert_eq!(args[1].kind, crate::types::MacroArgKind::Number, "defaultValue should be Number kind");
        assert_eq!(args[3].label, "autofocus");
        assert_eq!(args[3].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7cd_textarea_has_3_args_no_passage() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("textarea").expect("textarea should be in catalog");
        let args = def.args.expect("textarea should have args");
        assert_eq!(args.len(), 3, "textarea should have 3 args (receiverName, defaultValue, autofocus)");
        assert_eq!(args[2].label, "autofocus");
        assert_eq!(args[2].kind, crate::types::MacroArgKind::Keyword);
        // Verify NO passage arg
        assert!(!args.iter().any(|a| a.is_passage_ref), "textarea should NOT have a passage arg");
    }

    #[test]
    fn phase7cd_option_has_selected_keyword() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("option").expect("option should be in catalog");
        let args = def.args.expect("option should have args");
        assert_eq!(args.len(), 3, "option should have 3 args (label, value, selected)");
        assert_eq!(args[0].label, "label");
        assert_eq!(args[2].label, "selected");
        assert_eq!(args[2].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7cd_include_has_element_name() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("include").expect("include should be in catalog");
        let args = def.args.expect("include should have args");
        assert_eq!(args.len(), 2, "include should have 2 args (passageName, elementName)");
        assert_eq!(args[0].label, "passageName");
        assert!(args[0].is_passage_ref);
        assert_eq!(args[1].label, "elementName");
    }

    #[test]
    fn phase7cd_widget_has_container_keyword() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("widget").expect("widget should be in catalog");
        let args = def.args.expect("widget should have args");
        assert_eq!(args.len(), 2, "widget should have 2 args (widgetName, container)");
        assert_eq!(args[0].label, "widgetName");
        assert_eq!(args[1].label, "container");
        assert_eq!(args[1].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7cd_script_has_language_keyword() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("script").expect("script should be in catalog");
        let args = def.args.expect("script should now have args");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].label, "language");
        assert_eq!(args[0].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7cd_cacheaudio_has_track_and_source() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("cacheaudio").expect("cacheaudio should be in catalog");
        let args = def.args.expect("cacheaudio should now have args");
        assert_eq!(args.len(), 2, "cacheaudio should have 2 args (trackId, sourceList)");
        assert_eq!(args[0].label, "trackId");
        assert_eq!(args[1].label, "sourceList");
    }

    #[test]
    fn phase7cd_new_macro_arg_kinds_exist() {
        // Verify the new MacroArgKind variants exist and are distinct.
        use crate::types::MacroArgKind;
        assert_ne!(MacroArgKind::Keyword, MacroArgKind::String);
        assert_ne!(MacroArgKind::Link, MacroArgKind::String);
        assert_ne!(MacroArgKind::Image, MacroArgKind::String);
        assert_ne!(MacroArgKind::Number, MacroArgKind::Expression);
    }

    #[test]
    fn phase7cd_new_parsed_arg_kinds_exist() {
        // Verify the new ParsedArgKind variants exist.
        use crate::sugarcube::ast::ParsedArgKind;
        let _kw = ParsedArgKind::Keyword;
        let _link = ParsedArgKind::LinkMarkup;
        let _img = ParsedArgKind::ImageMarkup;
        let _num = ParsedArgKind::Number;
    }

    #[test]
    fn phase7cd_numberbox_number_arg_classified_as_number() {
        // <<numberbox "$x" 100>> — the 100 should be classified as Number.
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<numberbox "$x" 100>>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "numberbox");
                let args = structured_args.as_ref().expect("should have structured_args");
                assert_eq!(args.len(), 2, "should have 2 args");
                assert_eq!(args[0].kind, ParsedArgKind::VariableRef, "arg 0 should be VariableRef");
                assert_eq!(args[1].kind, ParsedArgKind::Number, "arg 1 (100) should be Number, got: {:?}", args[1].kind);
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7cd_textarea_autofocus_classified_as_keyword() {
        // <<textarea "$x" "default" autofocus>> — autofocus should be Keyword.
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<textarea "$x" "default" autofocus>>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "textarea");
                let args = structured_args.as_ref().expect("should have structured_args");
                assert_eq!(args.len(), 3);
                assert_eq!(args[2].kind, ParsedArgKind::Keyword, "autofocus should be Keyword, got: {:?}", args[2].kind);
                assert_eq!(args[2].value, "autofocus");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7cd_widget_container_classified_as_keyword() {
        // <<widget "say" container>> — container should be Keyword.
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<widget "say" container>>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "widget");
                let args = structured_args.as_ref().expect("should have structured_args");
                assert_eq!(args.len(), 2);
                assert_eq!(args[1].kind, ParsedArgKind::Keyword, "container should be Keyword, got: {:?}", args[1].kind);
                assert_eq!(args[1].value, "container");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7cd_option_selected_classified_as_keyword() {
        // <<option "Red" "red" selected>> — selected should be Keyword.
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<option "Red" "red" selected>>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "option");
                let args = structured_args.as_ref().expect("should have structured_args");
                assert_eq!(args.len(), 3);
                assert_eq!(args[2].kind, ParsedArgKind::Keyword, "selected should be Keyword, got: {:?}", args[2].kind);
                assert_eq!(args[2].value, "selected");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    #[test]
    fn phase7cd_include_element_name_classified_as_string() {
        // <<include "Forest" "div">> — elementName should be String.
        use crate::sugarcube::ast::ParsedArgKind;
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"<<include "Forest" "div">>"#, 0, ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, structured_args, .. } => {
                assert_eq!(name, "include");
                let args = structured_args.as_ref().expect("should have structured_args");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0].kind, ParsedArgKind::PassageRef, "arg 0 should be PassageRef");
                assert_eq!(args[1].kind, ParsedArgKind::String, "elementName should be String, got: {:?}", args[1].kind);
                assert_eq!(args[1].value, "div");
            }
            other => panic!("expected Macro, got {:?}", other),
        }
    }

    // ── Phase 7d completion tests — remaining macro schemas ──────────────

    #[test]
    fn phase7d_all_non_expression_macros_have_args() {
        // Verify that all macros that should have declared args DO have args.
        // JS-expression macros (if, set, for, etc.) correctly have args: None
        // because their args go to oxc. All other macros should have args: Some.
        use crate::sugarcube::macros::builtin_macros;

        let js_expr_macros: &[&str] = &[
            "if", "elseif", "else", "for", "break", "continue",
            "switch", "set", "run", "print", "=", "-",
            "silent", "silently", "next",
            "createaudiogroup", "createplaylist",
            "stop", "default",
            "unset", "capture", "waitforaudio",
            "redo", "stopallaudio",
            "nobr", "done",
            "waitforaudio",
            "code", // raw-body macro with no args
        ];

        for m in builtin_macros() {
            if js_expr_macros.contains(&m.name) {
                continue;
            }
            // All other macros should have args: Some
            assert!(m.args.is_some(),
                "'{}' should have args: Some (not None) — only JS-expression macros should have None",
                m.name);
        }
    }

    #[test]
    fn phase7d_case_has_variadic_expression() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("case").expect("case should be in catalog");
        let args = def.args.expect("case should have args");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].label, "valueList");
        // valueList is String kind (space-separated values, NOT a single JS expression).
        // This prevents oxc from trying to parse "Sam" "Jordan" as invalid JS.
        assert_eq!(args[0].kind, crate::types::MacroArgKind::String);
    }

    #[test]
    fn phase7d_type_has_full_signature() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("type").expect("type should be in catalog");
        let args = def.args.expect("type should have args");
        assert!(args.len() >= 7, "type should have at least 7 args (speed + 6 optional), got: {}", args.len());
        assert_eq!(args[0].label, "speed");
        assert!(args[0].is_required, "speed should be required");
    }

    #[test]
    fn phase7d_cycle_has_once_and_autoselect() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("cycle").expect("cycle should be in catalog");
        let args = def.args.expect("cycle should have args");
        assert_eq!(args.len(), 3, "cycle should have 3 args (receiverName, once, autoselect)");
        assert_eq!(args[1].label, "once");
        assert_eq!(args[1].kind, crate::types::MacroArgKind::Keyword);
        assert_eq!(args[2].label, "autoselect");
        assert_eq!(args[2].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7d_listbox_has_autoselect() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("listbox").expect("listbox should be in catalog");
        let args = def.args.expect("listbox should have args");
        assert_eq!(args.len(), 2, "listbox should have 2 args (receiverName, autoselect)");
        assert_eq!(args[1].label, "autoselect");
        assert_eq!(args[1].kind, crate::types::MacroArgKind::Keyword);
    }

    #[test]
    fn phase7d_link_button_use_linktext_label() {
        use crate::sugarcube::macros::find_macro;
        for name in &["link", "button"] {
            let def = find_macro(name).unwrap_or_else(|| panic!("{} should be in catalog", name));
            let args = def.args.expect(&format!("{} should have args", name));
            assert_eq!(args[0].label, "linkText", "{} arg 0 should be labeled 'linkText'", name);
        }
    }

    #[test]
    fn phase7d_audio_has_trackidlist_and_actionlist() {
        use crate::sugarcube::macros::find_macro;
        let def = find_macro("audio").expect("audio should be in catalog");
        let args = def.args.expect("audio should have args");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].label, "trackIdList");
        assert_eq!(args[1].label, "actionList");
        assert_eq!(args[1].kind, crate::types::MacroArgKind::Keyword);
    }

    // ── Bug fix tests — case/default unclosed + StoryData ────────────────

    #[test]
    fn bugfix_case_default_no_unclosed_diagnostic() {
        // <<case>> and <<default>> have BodyRequirement::Optional, so they
        // should NOT produce "Unclosed block macro" diagnostics when used
        // without closing tags inside <<switch>>.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<switch $x>>\n<<case 1>>\nOne\n<<case 2>>\nTwo\n<<default>>\nOther\n<</switch>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let unclosed_diags: Vec<_> = result.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-unclosed")
            .collect();

        assert!(unclosed_diags.is_empty(),
            "case/default should NOT produce unclosed diagnostics, got: {:?}",
            unclosed_diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn bugfix_case_without_close_no_unclosed_diagnostic() {
        // <<case>> without <</case>> should NOT produce unclosed diagnostic.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<switch $x>>\n<<case 1>>\nOne\n<<default>>\nOther\n<</switch>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let unclosed: Vec<_> = result.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-unclosed" && d.message.contains("case"))
            .collect();
        assert!(unclosed.is_empty(),
            "<<case>> without <</case>> should NOT be unclosed, got: {:?}", unclosed);

        let unclosed_default: Vec<_> = result.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-unclosed" && d.message.contains("default"))
            .collect();
        assert!(unclosed_default.is_empty(),
            "<<default>> without <</default>> should NOT be unclosed, got: {:?}", unclosed_default);
    }

    #[test]
    fn bugfix_switch_unclosed_still_reported() {
        // <<switch>> without <</switch>> SHOULD still produce unclosed diagnostic.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<switch $x>>\n<<case 1>>\nOne\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let switch_unclosed: Vec<_> = result.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-unclosed" && d.message.contains("switch"))
            .collect();
        assert!(!switch_unclosed.is_empty(),
            "<<switch>> without <</switch>> SHOULD be unclosed");
    }

    #[test]
    fn bugfix_storydata_no_panic() {
        // StoryData with JSON body should not panic.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: StoryData\n{\n\t\"ifid\": \"D674C58C-DEFA-4F70-B7A2-27742230C0FC\",\n\t\"format\": \"SugarCube\",\n\t\"format-version\": \"2.37.0\",\n\t\"start\": \"Start\",\n\t\"zoom\": 1,\n\t\"tag-colors\": {\n\t\t\"forest\": \"#3a5a40\",\n\t\t\"town\": \"#8a817c\",\n\t\t\"dungeon\": \"#3d2c2e\"\n\t}\n}\n:: Start\nHello\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);
        assert!(!result.token_groups.is_empty(), "should have token groups");
    }

    // ── Depth propagation tests — segmenters don't add nesting ──────────

    #[test]
    fn depth_elseif_segmenter_does_not_add_depth_to_children() {
        // <<else>>/<<elseif>> have BodyRequirement::Never — they're inline
        // segmenters, NOT nesting levels. Content after <<elseif>> is a
        // direct child of <<if>>, NOT a child of <<elseif>>.
        //
        // So <<set $x>> and <<set $y>> are BOTH at depth 1 (inside <<if>>),
        // and both should get BlockDepth1 for their delimiters.
        // <<elseif>> itself is a sibling of <<set $x>> at depth 1, but the
        // token builder adjusts its effective_depth to 0 (segmenter) → None.
        //
        // The key assertion: NO BlockDepth3 should appear.
        use crate::plugin::{FormatPluginMut, SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<if $a>>\n<<set $x to 1>>\n<<elseif $b>>\n<<set $y to 2>>\n<</if>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let delimiters: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();

        let depth3_count = delimiters.iter()
            .filter(|t| t.modifier == Some(SemanticTokenModifier::BlockDepth3))
            .count();

        assert!(depth3_count == 0,
            "There should be NO BlockDepth3 — <<set>> inside <<if>> (whether before or after <<elseif>>) should be BlockDepth1. depth3_count={}", depth3_count);
    }

    #[test]
    fn depth_case_segmenter_does_not_add_depth_to_children() {
        // Same test but for <<switch>>/<<case>>:
        //
        // <<switch $x>>    depth=0 → BlockDepth1
        // <<case 1>>       depth=1, eff=0 → BlockDepth1 (segmenter)
        //   <<set $a>>     depth=1 (eff+1) → BlockDepth2
        // <<default>>      depth=1, eff=0 → BlockDepth1 (segmenter)
        //   <<set $b>>     depth=1 (eff+1) → BlockDepth2
        // <</switch>>
        use crate::plugin::{FormatPluginMut, SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<switch $x>>\n<<case 1>>\n<<set $a to 1>>\n<<default>>\n<<set $b to 2>>\n<</switch>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let delimiters: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();

        let depth3_count = delimiters.iter()
            .filter(|t| t.modifier == Some(SemanticTokenModifier::BlockDepth3))
            .count();

        assert!(depth3_count == 0,
            "There should be NO BlockDepth3 delimiters — <<set>> inside <<case>>/<<default>> should be BlockDepth2. depth3_count={}", depth3_count);
    }

    #[test]
    fn depth_deeply_nested_with_segmenters() {
        // Complex nesting with segmenters:
        //
        // <<if $a>>         depth=0, eff=0 → BlockDepth1
        //   <<if $b>>       depth=1, eff=1 → BlockDepth2
        //     <<set $x>>    depth=2, eff=2 → BlockDepth3
        //   <</if>>
        // <<elseif $c>>     depth=1, eff=0 → BlockDepth1 (segmenter)
        //   <<if $d>>       depth=1, eff=1 → BlockDepth2 (correct! not BlockDepth3)
        //     <<set $y>>    depth=2, eff=2 → BlockDepth3
        //   <</if>>
        // <</if>>
        use crate::plugin::{FormatPluginMut, SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<if $a>>\n<<if $b>>\n<<set $x to 1>>\n<</if>>\n<<elseif $c>>\n<<if $d>>\n<<set $y to 2>>\n<</if>>\n<</if>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let delimiters: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();

        let depth4_count = delimiters.iter()
            .filter(|t| t.modifier == Some(SemanticTokenModifier::BlockDepth4))
            .count();

        assert!(depth4_count == 0,
            "There should be NO BlockDepth4 — <<if $d>> inside <<elseif>> should be BlockDepth2, its <<set>> should be BlockDepth3. depth4_count={}", depth4_count);
    }
}
