//! Passage tree — unified single-scan parser for SugarCube passage bodies.
//!
//! Replaces the multi-pass approach where `scan_macros()`, `extract_vars()`,
//! `extract_links()`, and `build_body_blocks()` each scan independently.
//! This module produces a single `Vec<PassageNode>` tree that all downstream
//! consumers walk.
//!
//! ## Architecture
//!
//! ```text
//! Source Text
//!     |
//!     v
//! parse_passage_body()
//!     |  One scan. Produces Vec<PassageNode>.
//!     |  - Scans <<>> constructs (reuses scan_macros char scanner)
//!     |  - Identifies text gaps between constructs
//!     |  - Finds $var / _var refs in text and macro args
//!     |  - Finds [[links]] in text segments
//!     |  - Builds tree structure for block macros
//!     |  - Every node carries exact byte spans
//!     |
//!     |  Vec<PassageNode>
//!     |
//!     |--> walk_vars()       -> Vec<VarOp>       (Phase 2)
//!     |--> walk_links()      -> Vec<Link>        (Phase 2)
//!     |--> walk_blocks()     -> Vec<Block>       (this file)
//!     |--> walk_translate()  -> (String, Vec<ExactLineMapping>) (Phase 3)
//!     |--> walk_validate()   -> Vec<Diagnostic>  (Phase 4)
//!     +--> walk_tokens()     -> Vec<SemanticToken> (Phase 4)
//! ```

use std::ops::Range;

use knot_core::passage::{Block, Link, VarKind, VarOp};

use std::collections::{HashMap, HashSet};

use super::blocks::{scan_macros, ParsedMacro};
use super::links::{RE_LINK_ARROW, RE_LINK_PIPE, RE_LINK_SIMPLE};
use super::macros::{block_macro_names, folding_modifier_names};
use super::vars::{
    RE_TEMP_VAR, RE_VAR, RE_VAR_DECREMENT, RE_VAR_DOT_PATH, RE_VAR_INCREMENT,
    RE_JS_ALIAS_SPECIFIC, RE_JS_ALIAS_WHOLE, RE_JS_STATE_WRITE, RE_JS_STATE_GETVAR,
    RE_JS_STATE_SETVAR, RE_ALIAS_PROPERTY, RE_JS_VAR_ASSIGN, RE_JS_VAR_COMPOUND,
    RE_VAR_BRACKET_PROP, RE_SETTER_LINK,
};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// SugarCube-internal tree representation of a passage body node.
///
/// Lives in `crates/formats/src/sugarcube/passage_tree.rs` — NOT in the
/// core crate. Format isolation: the core `Passage` struct is unchanged.
#[derive(Debug, Clone)]
pub(crate) enum PassageNode {
    /// Plain text content (may contain inline `$var` refs and `[[links]]`)
    Text {
        content: String,
        var_refs: Vec<VarRef>,
        links: Vec<Link>,
        span: Range<usize>,
    },
    /// A macro invocation (inline or block).
    ///
    /// Block macros (e.g. `<<if>>...<</if>>`) have `children = Some(...)`;
    /// inline macros have `children = None`.
    Macro {
        parsed: ParsedMacro,
        var_refs: Vec<VarRef>,
        children: Option<Vec<PassageNode>>,
        /// Byte span of the close tag (`<</name>>`), if this is a block macro.
        /// Used by `walk_blocks()` to emit the close-tag `Block::Macro` for
        /// backward compatibility. `None` for inline macros and unclosed blocks.
        close_span: Option<Range<usize>>,
        /// Byte span covering the entire construct (open tag through close tag
        /// for block macros, or just the tag for inline macros).
        span: Range<usize>,
    },
    /// An inline expression: `<<=>>` or `<<->>`
    Expression {
        /// The macro name ("=" or "-").
        name: String,
        /// The expression content (the arguments string).
        content: String,
        var_refs: Vec<VarRef>,
        span: Range<usize>,
    },
    /// A heading or section divider (reserved for future use).
    #[allow(dead_code)] // Will be consumed by walk_validate/walk_tokens enhancements
    Heading {
        content: String,
        span: Range<usize>,
    },
    /// An incomplete or malformed construct.
    Error {
        message: String,
        span: Range<usize>,
    },
}

/// Variable reference found during tree building.
///
/// Carries the context needed to determine read vs write, which is
/// determined by the containing macro. The `is_write` flag is set during
/// tree construction based on macro context:
/// - `<<set>>`, `<<capture>>`: variables in args are writes
/// - `<<if>>`, `<<elseif>>`: variables in args are reads
/// - `$var++`/`++$var`/`$var--`/`--$var`: writes
/// - Naked `$var` in text: reads
#[derive(Debug, Clone)]
pub(crate) struct VarRef {
    /// The base variable name including sigil (e.g., "$gold", "_idx").
    pub name: String,
    /// Dot-notation property path after the base name (e.g., "sword.damage"
    /// for `$item.sword.damage`). `None` for simple variable references.
    pub property_path: Option<String>,
    /// Whether this is a temporary variable (`_var`).
    pub is_temporary: bool,
    /// Whether this reference is a write (assignment) rather than a read.
    pub is_write: bool,
    /// Exact byte range in the source document.
    pub span: Range<usize>,
}

// ---------------------------------------------------------------------------
// Helpers: JS bracket context (duplicated from links.rs to avoid modifying it)
// ---------------------------------------------------------------------------

/// Check whether a `[[` at the given position in `text` is a JavaScript
/// bracket notation context rather than a genuine Twine link.
///
/// Returns `true` if the character immediately before position `pos` is
/// one that indicates JS computed property access (`obj[[key]]`):
/// `[`, `]`, `)`, `}`, alphanumeric, `_`, or `$`.
fn is_js_bracket_context(text: &str, pos: usize) -> bool {
    if pos == 0 {
        return false;
    }
    let prev = text.as_bytes()[pos - 1];
    prev == b'['
        || prev == b']'
        || prev == b')'
        || prev == b'}'
        || prev.is_ascii_alphanumeric()
        || prev == b'_'
        || prev == b'$'
}

// ---------------------------------------------------------------------------
// Helpers: $$ escape detection
// ---------------------------------------------------------------------------

/// Check if a `$var` regex match at position `match_start` in `text` is a
/// `$$` escape markup and should be excluded.
///
/// SugarCube uses `$$` as the escape markup: `$$name` outputs literal `$name`
/// and is NOT a variable reference. Two checks:
/// 1. The match is preceded by another `$` (the match is the second `$` in `$$name`)
/// 2. The match itself starts with `$$` (regex matched `$$name` where the
///    second `$` is a valid variable-name character)
fn is_dollar_escape(text: &str, match_start: usize) -> bool {
    // Check 1: preceded by another $
    if match_start > 0 && text.as_bytes()[match_start - 1] == b'$' {
        return true;
    }
    // Check 2: the match itself starts with $$ (e.g., $$name)
    if text[match_start..].starts_with("$$") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Variable reference extraction from text segments
// ---------------------------------------------------------------------------

/// Extract variable references from a plain text segment.
///
/// All references found in plain text are reads, except increment/decrement
/// patterns (`$var++`, `++$var`, `$var--`, `--$var`) which are writes.
fn extract_var_refs_from_text(text: &str, text_offset: usize) -> Vec<VarRef> {
    let mut refs = Vec::new();
    // Track byte ranges (relative to `text`) already covered by more specific
    // patterns (dot-path, increment/decrement) to avoid double-counting.
    let mut covered: Vec<Range<usize>> = Vec::new();

    // 1. Dot-path references: $var.prop.path (most specific — process first)
    for caps in RE_VAR_DOT_PATH.captures_iter(text) {
        let full = caps.get(0).unwrap();
        if is_dollar_escape(text, full.start()) {
            continue;
        }
        let full_str = full.as_str();
        let dot_pos = full_str.find('.').unwrap_or(full_str.len());
        let name = full_str[..dot_pos].to_string();
        let property_path = if dot_pos + 1 < full_str.len() {
            Some(full_str[dot_pos + 1..].to_string())
        } else {
            None
        };

        refs.push(VarRef {
            name,
            property_path,
            is_temporary: false,
            is_write: false,
            span: text_offset + full.start()..text_offset + full.end(),
        });
        covered.push(full.start()..full.end());
    }

    // 2. Increment patterns: $var++ / ++$var (write)
    for caps in RE_VAR_INCREMENT.captures_iter(text) {
        let full = caps.get(0).unwrap();
        // Check $$ escape at the position of the $ sigil, not the start of
        // the match (which may be ++ for pre-increment)
        let dollar_pos_in_text = full.start() + full.as_str().find('$').unwrap_or(0);
        if is_dollar_escape(text, dollar_pos_in_text) {
            continue;
        }
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        if covered.iter().any(|s| var_match.start() >= s.start && var_match.end() <= s.end) {
            continue;
        }
        let dollar_pos = full.as_str().find('$').unwrap_or(0);
        let name = format!("${}", var_match.as_str());
        let var_abs_start = text_offset + full.start() + dollar_pos;
        let var_abs_end = var_abs_start + name.len();

        refs.push(VarRef {
            name,
            property_path: None,
            is_temporary: false,
            is_write: true,
            span: var_abs_start..var_abs_end,
        });
        // Mark the full ++ pattern as covered
        covered.push(full.start()..full.end());
    }

    // 3. Decrement patterns: $var-- / --$var (write)
    for caps in RE_VAR_DECREMENT.captures_iter(text) {
        let full = caps.get(0).unwrap();
        let dollar_pos_in_text = full.start() + full.as_str().find('$').unwrap_or(0);
        if is_dollar_escape(text, dollar_pos_in_text) {
            continue;
        }
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        if covered.iter().any(|s| var_match.start() >= s.start && var_match.end() <= s.end) {
            continue;
        }
        let dollar_pos = full.as_str().find('$').unwrap_or(0);
        let name = format!("${}", var_match.as_str());
        let var_abs_start = text_offset + full.start() + dollar_pos;
        let var_abs_end = var_abs_start + name.len();

        refs.push(VarRef {
            name,
            property_path: None,
            is_temporary: false,
            is_write: true,
            span: var_abs_start..var_abs_end,
        });
        covered.push(full.start()..full.end());
    }

    // 4. Regular $var references (not in dot-path, not in inc/dec)
    for caps in RE_VAR.captures_iter(text) {
        let full = caps.get(0).unwrap();
        if is_dollar_escape(text, full.start()) {
            continue;
        }
        // Skip if already covered by dot-path or inc/dec
        if covered.iter().any(|s| full.start() >= s.start && full.end() <= s.end) {
            continue;
        }

        refs.push(VarRef {
            name: full.as_str().to_string(),
            property_path: None,
            is_temporary: false,
            is_write: false,
            span: text_offset + full.start()..text_offset + full.end(),
        });
    }

    // 5. Temporary variable references: _var
    for caps in RE_TEMP_VAR.captures_iter(text) {
        let full = caps.get(0).unwrap();
        // Skip if preceded by alphanumeric (part of another identifier, e.g., foo_bar)
        if full.start() > 0 && text.as_bytes()[full.start() - 1].is_ascii_alphanumeric() {
            continue;
        }

        refs.push(VarRef {
            name: full.as_str().to_string(),
            property_path: None,
            is_temporary: true,
            is_write: false,
            span: text_offset + full.start()..text_offset + full.end(),
        });
    }

    refs
}

// ---------------------------------------------------------------------------
// Variable reference extraction from macro arguments
// ---------------------------------------------------------------------------

/// Macros whose arguments write to variables.
const WRITE_MACRO_NAMES: &[&str] = &["set", "capture", "unset"];

/// Compute the byte offset of the (trimmed) arguments string within the
/// source document, given the ParsedMacro and body_offset.
///
/// The ParsedMacro stores `args` as the trimmed content between the macro
/// name and the closing `>>`. We need the exact byte offset of the first
/// non-whitespace character of the args within the body, then add
/// `body_offset` to make it document-absolute.
fn compute_args_offset(m: &ParsedMacro, body: &str, body_offset: usize) -> usize {
    let name_end = m.name_start + m.name_len;
    let closing_gt_start = m.end.saturating_sub(2); // position of the first '>' in '>>'
    if name_end >= closing_gt_start {
        // No args at all
        return body_offset + name_end;
    }
    let raw_args = &body[name_end..closing_gt_start];
    let leading_ws = raw_args.len() - raw_args.trim_start().len();
    body_offset + name_end + leading_ws
}

/// Extract variable references from a macro's arguments string.
///
/// Determines read/write based on the macro context:
/// - `<<set>>`, `<<capture>>`, `<<unset>>`: variables are writes
/// - `<<if>>`, `<<elseif>>`: variables are reads
/// - All others: variables are reads
/// - Increment/decrement patterns: always writes
fn extract_var_refs_from_macro_args(
    macro_name: &str,
    args: &str,
    args_offset: usize,
) -> Vec<VarRef> {
    if args.is_empty() {
        return Vec::new();
    }

    let mut refs = Vec::new();
    let mut covered: Vec<Range<usize>> = Vec::new();

    let is_write_macro = WRITE_MACRO_NAMES.contains(&macro_name);

    // 1. Dot-path references: $var.prop.path
    for caps in RE_VAR_DOT_PATH.captures_iter(args) {
        let full = caps.get(0).unwrap();
        if is_dollar_escape(args, full.start()) {
            continue;
        }
        let full_str = full.as_str();
        let dot_pos = full_str.find('.').unwrap_or(full_str.len());
        let name = full_str[..dot_pos].to_string();
        let property_path = if dot_pos + 1 < full_str.len() {
            Some(full_str[dot_pos + 1..].to_string())
        } else {
            None
        };

        refs.push(VarRef {
            name,
            property_path,
            is_temporary: false,
            is_write: is_write_macro,
            span: args_offset + full.start()..args_offset + full.end(),
        });
        covered.push(full.start()..full.end());
    }

    // 2. Increment patterns: $var++ / ++$var (always write)
    for caps in RE_VAR_INCREMENT.captures_iter(args) {
        let full = caps.get(0).unwrap();
        let dollar_pos_in_args = full.start() + full.as_str().find('$').unwrap_or(0);
        if is_dollar_escape(args, dollar_pos_in_args) {
            continue;
        }
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        if covered.iter().any(|s| var_match.start() >= s.start && var_match.end() <= s.end) {
            continue;
        }
        let dollar_pos = full.as_str().find('$').unwrap_or(0);
        let name = format!("${}", var_match.as_str());
        let var_abs_start = args_offset + full.start() + dollar_pos;
        let var_abs_end = var_abs_start + name.len();

        refs.push(VarRef {
            name,
            property_path: None,
            is_temporary: false,
            is_write: true,
            span: var_abs_start..var_abs_end,
        });
        covered.push(full.start()..full.end());
    }

    // 3. Decrement patterns: $var-- / --$var (always write)
    for caps in RE_VAR_DECREMENT.captures_iter(args) {
        let full = caps.get(0).unwrap();
        let dollar_pos_in_args = full.start() + full.as_str().find('$').unwrap_or(0);
        if is_dollar_escape(args, dollar_pos_in_args) {
            continue;
        }
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        if covered.iter().any(|s| var_match.start() >= s.start && var_match.end() <= s.end) {
            continue;
        }
        let dollar_pos = full.as_str().find('$').unwrap_or(0);
        let name = format!("${}", var_match.as_str());
        let var_abs_start = args_offset + full.start() + dollar_pos;
        let var_abs_end = var_abs_start + name.len();

        refs.push(VarRef {
            name,
            property_path: None,
            is_temporary: false,
            is_write: true,
            span: var_abs_start..var_abs_end,
        });
        covered.push(full.start()..full.end());
    }

    // 4. Regular $var references
    for caps in RE_VAR.captures_iter(args) {
        let full = caps.get(0).unwrap();
        if is_dollar_escape(args, full.start()) {
            continue;
        }
        if covered.iter().any(|s| full.start() >= s.start && full.end() <= s.end) {
            continue;
        }

        refs.push(VarRef {
            name: full.as_str().to_string(),
            property_path: None,
            is_temporary: false,
            is_write: is_write_macro,
            span: args_offset + full.start()..args_offset + full.end(),
        });
    }

    // 5. Temporary variable references: _var
    for caps in RE_TEMP_VAR.captures_iter(args) {
        let full = caps.get(0).unwrap();
        if full.start() > 0 && args.as_bytes()[full.start() - 1].is_ascii_alphanumeric() {
            continue;
        }

        refs.push(VarRef {
            name: full.as_str().to_string(),
            property_path: None,
            is_temporary: true,
            is_write: is_write_macro,
            span: args_offset + full.start()..args_offset + full.end(),
        });
    }

    refs
}

// ---------------------------------------------------------------------------
// Link extraction from text segments
// ---------------------------------------------------------------------------

/// Extract `[[...]]` links from a text segment.
///
/// Uses the same three-regex approach as `links::extract_links()`:
/// 1. Arrow-style: `[[Display->Target]]`
/// 2. Pipe-style: `[[Display|Target]]`
/// 3. Simple: `[[Target]]`
///
/// Filters JS bracket notation false positives and overlap between
/// link types (simple links that are sub-spans of arrow/pipe links).
fn extract_links_from_text(text: &str, text_offset: usize) -> Vec<Link> {
    let mut links = Vec::new();

    // Arrow-style links: [[Display->Target]]
    for caps in RE_LINK_ARROW.captures_iter(text) {
        let m = caps.get(0).unwrap();
        if is_js_bracket_context(text, m.start()) {
            continue;
        }
        let display = caps.get(1).unwrap().as_str().trim().to_string();
        let target = caps.get(2).unwrap().as_str().trim().to_string();
        links.push(Link {
            display_text: Some(display),
            target,
            span: text_offset + m.start()..text_offset + m.end(),
            edge_type_hint: None,
        });
    }

    // Pipe-style links: [[Display|Target]]
    for caps in RE_LINK_PIPE.captures_iter(text) {
        let m = caps.get(0).unwrap();
        if is_js_bracket_context(text, m.start()) {
            continue;
        }
        let display = caps.get(1).unwrap().as_str().trim().to_string();
        let target = caps.get(2).unwrap().as_str().trim().to_string();
        links.push(Link {
            display_text: Some(display),
            target,
            span: text_offset + m.start()..text_offset + m.end(),
            edge_type_hint: None,
        });
    }

    // Simple links: [[Target]]
    // Collect arrow/pipe spans for overlap filtering
    let arrow_pipe_spans: Vec<Range<usize>> = RE_LINK_ARROW
        .captures_iter(text)
        .chain(RE_LINK_PIPE.captures_iter(text))
        .filter_map(|caps| {
            let m = caps.get(0)?;
            Some(m.start()..m.end())
        })
        .collect();

    for caps in RE_LINK_SIMPLE.captures_iter(text) {
        let m = caps.get(0).unwrap();
        let span = m.start()..m.end();

        // Filter overlaps with arrow/pipe links
        if arrow_pipe_spans
            .iter()
            .any(|s| span.start >= s.start && span.end <= s.end)
        {
            continue;
        }

        // Filter JS bracket notation
        if is_js_bracket_context(text, m.start()) {
            continue;
        }

        let target = caps.get(1).unwrap().as_str().trim().to_string();
        links.push(Link {
            display_text: None,
            target,
            span: text_offset + m.start()..text_offset + m.end(),
            edge_type_hint: None,
        });
    }

    links
}

// ---------------------------------------------------------------------------
// Tree building: stack frame
// ---------------------------------------------------------------------------

/// A stack frame tracking an open block macro during tree building.
struct StackFrame {
    /// The macro name (without `/` prefix), e.g., "if", "for", "link".
    macro_name: String,
    /// The ParsedMacro for the open tag.
    open_parsed: ParsedMacro,
    /// Variable references extracted from the open tag's arguments.
    var_refs: Vec<VarRef>,
    /// Children collected so far between the open tag and the close tag.
    children: Vec<PassageNode>,
}

// ---------------------------------------------------------------------------
// parse_passage_body()
// ---------------------------------------------------------------------------

/// Parse a SugarCube passage body into a structured tree.
///
/// This is the single source of truth for all downstream consumers.
/// One scan produces `Vec<PassageNode>` with:
/// - Macro spans from `scan_macros()` (battle-tested character scanner)
/// - Text gaps between macros
/// - Variable references in text and macro args
/// - Links in text segments
/// - Tree structure for block macros
/// - Exact byte spans on every node
///
/// Does NOT: translate, validate, or emit diagnostics.
pub(crate) fn parse_passage_body(body: &str, body_offset: usize) -> Vec<PassageNode> {
    // Special case: empty body
    if body.trim().is_empty() {
        return Vec::new();
    }

    let macros = scan_macros(body);

    // Special case: no macros — entire body is a single text node
    if macros.is_empty() {
        return vec![PassageNode::Text {
            content: body.to_string(),
            var_refs: extract_var_refs_from_text(body, body_offset),
            links: extract_links_from_text(body, body_offset),
            span: body_offset..body_offset + body.len(),
        }];
    }

    let block_names = block_macro_names();
    let modifier_names = folding_modifier_names();

    // Root-level nodes and nesting stack
    let mut root: Vec<PassageNode> = Vec::new();
    let mut stack: Vec<StackFrame> = Vec::new();
    let mut cursor: usize = 0;

    for m in &macros {
        // ── Text gap before this macro ──────────────────────────────
        if cursor < m.start {
            let gap = &body[cursor..m.start];
            if !gap.trim().is_empty() {
                let text_node = PassageNode::Text {
                    content: gap.to_string(),
                    var_refs: extract_var_refs_from_text(gap, body_offset + cursor),
                    links: extract_links_from_text(gap, body_offset + cursor),
                    span: body_offset + cursor..body_offset + m.start,
                };
                push_node(text_node, &mut root, &mut stack);
            }
        }

        // ── Classify and process the macro ──────────────────────────
        let is_close = m.name.starts_with('/');
        let macro_base_name: &str = if is_close { &m.name[1..] } else { &m.name };

        if is_close {
            // ── Close tag: <</name>> ────────────────────────────────
            let close_span = body_offset + m.start..body_offset + m.end;

            // Find the matching open macro on the stack (search from top)
            let match_idx = stack.iter().rposition(|f| f.macro_name == macro_base_name);

            if let Some(idx) = match_idx {
                // Pop any unclosed blocks above the matching one.
                // These become Error nodes (or Macro nodes with close_span=None).
                while stack.len() > idx + 1 {
                    let unclosed = stack.pop().unwrap();
                    let open_start = body_offset + unclosed.open_parsed.start;
                    let unclosed_node = PassageNode::Macro {
                        parsed: unclosed.open_parsed,
                        var_refs: unclosed.var_refs,
                        children: Some(unclosed.children),
                        close_span: None,
                        span: open_start..body_offset + body.len(),
                    };
                    // Add to the new top's children (or root)
                    push_node(unclosed_node, &mut root, &mut stack);
                }

                // Pop the matching frame and create the Macro node
                let frame = stack.pop().unwrap();
                let open_start = body_offset + frame.open_parsed.start;
                let close_end = close_span.end;
                let macro_node = PassageNode::Macro {
                    parsed: frame.open_parsed,
                    var_refs: frame.var_refs,
                    children: Some(frame.children),
                    close_span: Some(close_span),
                    span: open_start..close_end,
                };
                push_node(macro_node, &mut root, &mut stack);
            } else {
                // Unmatched close tag — create an Error node
                push_node(
                    PassageNode::Error {
                        message: format!("Unmatched close tag <</{}>>", macro_base_name),
                        span: close_span,
                    },
                    &mut root,
                    &mut stack,
                );
            }
        } else if modifier_names.contains(macro_base_name) {
            // ── Modifier macro (else, elseif, case, default) ────────
            // Phase 1: treated as siblings within the current block context.
            // Full branch tracking is Phase 3.
            let args_offset = compute_args_offset(m, body, body_offset);
            let var_refs = extract_var_refs_from_macro_args(macro_base_name, &m.args, args_offset);

            let node = PassageNode::Macro {
                parsed: m.clone(),
                var_refs,
                children: None,
                close_span: None,
                span: body_offset + m.start..body_offset + m.end,
            };
            push_node(node, &mut root, &mut stack);
        } else if macro_base_name == "=" || macro_base_name == "-" {
            // ── Expression macro: <<=>> or <<->> ────────────────────
            let args_offset = compute_args_offset(m, body, body_offset);
            let var_refs = extract_var_refs_from_macro_args(macro_base_name, &m.args, args_offset);

            let node = PassageNode::Expression {
                name: m.name.clone(),
                content: m.args.clone(),
                var_refs,
                span: body_offset + m.start..body_offset + m.end,
            };
            push_node(node, &mut root, &mut stack);
        } else if block_names.contains(macro_base_name) {
            // ── Block macro open tag: push onto stack ───────────────
            let args_offset = compute_args_offset(m, body, body_offset);
            let var_refs = extract_var_refs_from_macro_args(macro_base_name, &m.args, args_offset);

            stack.push(StackFrame {
                macro_name: m.name.clone(),
                open_parsed: m.clone(),
                var_refs,
                children: Vec::new(),
            });
        } else {
            // ── Inline macro ────────────────────────────────────────
            let args_offset = compute_args_offset(m, body, body_offset);
            let var_refs = extract_var_refs_from_macro_args(macro_base_name, &m.args, args_offset);

            let node = PassageNode::Macro {
                parsed: m.clone(),
                var_refs,
                children: None,
                close_span: None,
                span: body_offset + m.start..body_offset + m.end,
            };
            push_node(node, &mut root, &mut stack);
        }

        cursor = m.end;
    }

    // ── Trailing text after the last macro ──────────────────────────
    if cursor < body.len() {
        let trailing = &body[cursor..];
        if !trailing.trim().is_empty() {
            let text_node = PassageNode::Text {
                content: trailing.to_string(),
                var_refs: extract_var_refs_from_text(trailing, body_offset + cursor),
                links: extract_links_from_text(trailing, body_offset + cursor),
                span: body_offset + cursor..body_offset + body.len(),
            };
            push_node(text_node, &mut root, &mut stack);
        }
    }

    // ── Handle unclosed block macros remaining on the stack ─────────
    // Pop from the top, creating Macro nodes with close_span=None.
    // These represent blocks that were opened but never closed.
    while let Some(frame) = stack.pop() {
        let open_start = body_offset + frame.open_parsed.start;
        let node = PassageNode::Macro {
            parsed: frame.open_parsed,
            var_refs: frame.var_refs,
            children: Some(frame.children),
            close_span: None,
            span: open_start..body_offset + body.len(),
        };
        push_node(node, &mut root, &mut stack);
    }

    root
}

/// Push a node into the current context: if the stack is non-empty, add to
/// the top frame's children; otherwise add to the root list.
fn push_node(node: PassageNode, root: &mut Vec<PassageNode>, stack: &mut Vec<StackFrame>) {
    if let Some(frame) = stack.last_mut() {
        frame.children.push(node);
    } else {
        root.push(node);
    }
}

// ---------------------------------------------------------------------------
// walk_blocks()
// ---------------------------------------------------------------------------

/// Walk the tree and produce a flat `Vec<Block>` for backward compatibility.
///
/// Replaces `build_body_blocks()` — produces identical output. Text nodes →
/// `Block::Text`, Macro nodes → `Block::Macro` (including close tags for
/// block macros), Expression nodes → `Block::Macro`, Error nodes →
/// `Block::Incomplete`.
///
/// The walk is depth-first: for a block macro like `<<if>>...<</if>>`, it
/// emits:
/// 1. `Block::Macro` for the open tag (`<<if ...>>`)
/// 2. Recursively walks children
/// 3. `Block::Macro` for the close tag (`<</if>>`)
pub(crate) fn walk_blocks(nodes: &[PassageNode]) -> Vec<Block> {
    let mut blocks = Vec::new();

    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                blocks.push(Block::Text {
                    content: content.clone(),
                    span: span.clone(),
                });
            }
            PassageNode::Macro {
                parsed,
                children,
                close_span,
                span,
                ..
            } => {
                // Open tag: Block::Macro
                // The open tag span is derived from the ParsedMacro (body-relative)
                // converted to document-absolute during tree building.
                // span.start = body_offset + parsed.start
                // open tag end = body_offset + parsed.end
                let open_tag_end = span.start + (parsed.end - parsed.start);
                blocks.push(Block::Macro {
                    name: parsed.name.clone(),
                    args: parsed.args.clone(),
                    span: span.start..open_tag_end,
                });

                // Recursively walk children
                if let Some(children) = children {
                    let child_blocks = walk_blocks(children);
                    blocks.extend(child_blocks);
                }

                // Close tag: Block::Macro (for block macros)
                if let Some(close_span) = close_span {
                    blocks.push(Block::Macro {
                        name: format!("/{}", parsed.name),
                        args: String::new(),
                        span: close_span.clone(),
                    });
                }
            }
            PassageNode::Expression {
                name, content, span, ..
            } => {
                // Map Expression to Block::Macro for backward compatibility.
                // The current system treats <<=>> and <<->> as Block::Macro.
                blocks.push(Block::Macro {
                    name: name.clone(),
                    args: content.clone(),
                    span: span.clone(),
                });
            }
            PassageNode::Heading { content, span } => {
                blocks.push(Block::Heading {
                    content: content.clone(),
                    span: span.clone(),
                });
            }
            PassageNode::Error { span, .. } => {
                blocks.push(Block::Incomplete {
                    content: String::new(),
                    span: span.clone(),
                });
            }
        }
    }

    blocks
}

// ---------------------------------------------------------------------------
// walk_vars() — Replace extract_vars()
// ---------------------------------------------------------------------------

/// Walk the tree and extract variable operations.
///
/// Replaces `extract_vars()` (30+ regex passes on raw text) with a single
/// tree walk. The tree nodes already carry basic VarRef entries from
/// `extract_var_refs_from_text()` and `extract_var_refs_from_macro_args()`.
/// This function augments those with:
///
/// - `<<run>>` body JS analysis (detects `$var = value` and `$var += value`
///   writes within JavaScript code)
/// - `State.variables.var = value` (JS direct write)
/// - `State.getVar("$var")` (JS API read)
/// - `State.setVar("$var", value)` (JS API write)
/// - JS alias tracking (`var x = State.variables` → `x.prop` = `$prop`)
/// - JS specific alias (`var x = State.variables.gold` → `$gold` read)
/// - Setter links (`[[text|passage][$var to value]]`)
/// - Bracket notation property access (`$var["property"]`)
///
/// All variable references are deduplicated by span to avoid double-counting.
///
/// **Note**: `body` and `body_offset` are needed because the augmentation
/// passes require computing document-absolute byte offsets for matches found
/// within macro args. The basic VarRefs from tree nodes already have correct
/// spans; only the augmented refs need the offset computation.
pub(crate) fn walk_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<VarOp> {
    let mut all_refs: Vec<VarRef> = Vec::new();
    collect_var_refs(nodes, &mut all_refs);

    // ── Additional patterns that require macro-context awareness ────────
    augment_run_body_vars(nodes, body, body_offset, &mut all_refs);
    augment_state_api_vars(nodes, body, body_offset, &mut all_refs);
    augment_js_alias_vars(nodes, body, body_offset, &mut all_refs);
    augment_setter_link_vars(nodes, body, body_offset, &mut all_refs);
    augment_bracket_prop_vars(nodes, body, body_offset, &mut all_refs);

    // ── Deduplicate by span and convert to VarOp ───────────────────────
    let mut seen_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut vars: Vec<VarOp> = Vec::new();

    for var_ref in &all_refs {
        let key = (var_ref.span.start, var_ref.span.end);
        if seen_spans.contains(&key) {
            continue;
        }
        seen_spans.insert(key);
        vars.push(var_ref_to_var_op(var_ref));
    }

    vars
}

/// Recursively collect VarRef entries from all tree nodes.
fn collect_var_refs(nodes: &[PassageNode], refs: &mut Vec<VarRef>) {
    for node in nodes {
        match node {
            PassageNode::Text { var_refs, .. } => {
                refs.extend(var_refs.iter().cloned());
            }
            PassageNode::Macro {
                var_refs, children, ..
            } => {
                refs.extend(var_refs.iter().cloned());
                if let Some(children) = children {
                    collect_var_refs(children, refs);
                }
            }
            PassageNode::Expression { var_refs, .. } => {
                refs.extend(var_refs.iter().cloned());
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Augment variable refs with `<<run>>` body JS analysis.
///
/// The basic tree extraction treats all `$var` in `<<run>>` args as reads.
/// This function detects JS write patterns within `<<run>>` bodies:
/// - `$var = value` (simple assignment, not == or ===)
/// - `$var += value` (compound assignment: +=, -=, *=, /=, %=)
fn augment_run_body_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            if parsed.name == "run" && !parsed.args.is_empty() {
                let js_text = &parsed.args;
                let args_offset = compute_args_offset(parsed, body, body_offset);

                // Track spans already covered to avoid double-counting
                let mut covered_spans: Vec<Range<usize>> = Vec::new();

                // Compound assignments: $var +=, -=, *=, /=, %=
                for js_caps in RE_JS_VAR_COMPOUND.captures_iter(js_text) {
                    let full = js_caps.get(0).unwrap();
                    let var_match = js_caps.get(1).unwrap();

                    // Skip $$ escape
                    if is_dollar_escape(js_text, full.start()) {
                        continue;
                    }
                    let is_double_dollar = full.as_str().starts_with("$$");
                    if is_double_dollar {
                        continue;
                    }

                    let name = format!("${}", var_match.as_str());
                    let var_start = args_offset + full.start();
                    let var_end = var_start + name.len();

                    // Skip if already in covered spans
                    if covered_spans.iter().any(|s| var_start >= s.start && var_end <= s.end) {
                        continue;
                    }

                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                    covered_spans.push(var_start..var_end);
                }

                // Simple assignments: $var = (but NOT ==, ===, or compound)
                for js_caps in RE_JS_VAR_ASSIGN.captures_iter(js_text) {
                    let full = js_caps.get(0).unwrap();
                    let var_match = js_caps.get(1).unwrap();

                    // Skip $$ escape
                    if is_dollar_escape(js_text, full.start()) {
                        continue;
                    }
                    let is_double_dollar = full.as_str().starts_with("$$");
                    if is_double_dollar {
                        continue;
                    }

                    // Skip if this is a compound assignment (already handled)
                    let is_compound = RE_JS_VAR_COMPOUND
                        .captures_iter(js_text)
                        .any(|cc| cc.get(0).unwrap().start() == full.start());
                    if is_compound {
                        continue;
                    }

                    // Skip if this is == or === (comparison, not assignment)
                    let after_match = js_text.get(full.end()..).unwrap_or("");
                    if after_match.starts_with('=') {
                        continue;
                    }

                    let name = format!("${}", var_match.as_str());
                    let var_start = args_offset + full.start();
                    let var_end = var_start + name.len();

                    // Skip if already in covered spans
                    if covered_spans.iter().any(|s| var_start >= s.start && var_end <= s.end) {
                        continue;
                    }

                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                    covered_spans.push(var_start..var_end);
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_run_body_vars(children, body, body_offset, refs);
        }
    }
}

/// Augment with `State.variables.var = value`, `State.getVar()`, `State.setVar()`.
///
/// These JavaScript API patterns can appear inside any macro's args (especially
/// `<<run>>`, `<<set>>`, `<<script>>` blocks) and also in text segments
/// (e.g., in `<<print>>` expressions or inline SugarCube markup).
fn augment_state_api_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            let args = &parsed.args;
            if args.is_empty() {
                continue;
            }
            let args_offset = compute_args_offset(parsed, body, body_offset);

            // State.variables.var = value → WRITE
            for caps in RE_JS_STATE_WRITE.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                // Skip if already recorded at this span
                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }

            // State.getVar("$var") → READ
            for caps in RE_JS_STATE_GETVAR.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: false,
                        span: var_start..var_end,
                    });
                }
            }

            // State.setVar("$var", value) → WRITE
            for caps in RE_JS_STATE_SETVAR.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Also check text nodes for State API patterns (e.g., in <<print>>)
        if let PassageNode::Text { content, span, .. } = node {
            let text_offset = span.start;

            for caps in RE_JS_STATE_WRITE.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }

            for caps in RE_JS_STATE_GETVAR.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: false,
                        span: var_start..var_end,
                    });
                }
            }

            for caps in RE_JS_STATE_SETVAR.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_state_api_vars(children, body, body_offset, refs);
        }
    }
}

/// Augment with JS alias tracking.
///
/// Detects `var x = State.variables` (whole-object alias) and
/// `var x = State.variables.gold` (specific-variable alias), then resolves
/// `x.prop` references as `$prop` reads/writes.
fn augment_js_alias_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // First pass: collect all macro args texts to scan for alias declarations.
    // We need a flat view of all macro args and text content to build the
    // alias map, then a second pass to resolve alias property accesses.

    // Collect (content, content_offset) pairs for alias scanning.
    let mut content_pairs: Vec<(&str, usize)> = Vec::new();
    collect_content_for_alias_scan(nodes, body, body_offset, &mut content_pairs);

    // Build whole-object alias map: alias_name → (alias_offset, body-relative position)
    let mut whole_aliases: HashMap<String, usize> = HashMap::new();

    for (content, offset) in &content_pairs {
        // Whole-object alias: var x = State.variables (NOT State.variables.something)
        for caps in RE_JS_ALIAS_WHOLE.captures_iter(content) {
            let alias_name = caps.get(1).unwrap().as_str().to_string();
            let full = caps.get(0).unwrap();
            let alias_offset = *offset + full.start();

            // Check that this isn't also a specific alias
            let is_specific = RE_JS_ALIAS_SPECIFIC
                .captures_iter(content)
                .any(|specific_caps| {
                    specific_caps.get(0).unwrap().start() == full.start()
                });

            if !is_specific {
                whole_aliases.insert(alias_name, alias_offset);
            }
        }

        // Specific-variable alias: var x = State.variables.gold → $gold read
        for caps in RE_JS_ALIAS_SPECIFIC.captures_iter(content) {
            let _alias_name = caps.get(1).unwrap().as_str();
            let sc_var = caps.get(2).unwrap().as_str();
            let full = caps.get(0).unwrap();
            let var_start = *offset + full.start();
            let var_end = *offset + full.end();

            let dollar_name = format!("${}", sc_var);
            let is_dup = refs.iter().any(|r| {
                r.name == dollar_name && r.span.start == var_start
            });
            if !is_dup {
                refs.push(VarRef {
                    name: dollar_name,
                    property_path: None,
                    is_temporary: false,
                    is_write: false,
                    span: var_start..var_end,
                });
            }
        }
    }

    // Resolve whole-object alias property accesses
    if !whole_aliases.is_empty() {
        for (content, offset) in &content_pairs {
            for caps in RE_ALIAS_PROPERTY.captures_iter(content) {
                let alias_name = caps.get(1).unwrap().as_str();
                let property = caps.get(2).unwrap().as_str();
                let full = caps.get(0).unwrap();

                if let Some(&alias_offset) = whole_aliases.get(alias_name) {
                    let prop_start = *offset + full.start();
                    // Skip the alias declaration itself
                    if prop_start <= alias_offset {
                        continue;
                    }

                    let prop_end = *offset + full.end();
                    let dollar_name = format!("${}", property);

                    // Determine if this is a write by checking what follows
                    let after_match = &content[full.end()..];
                    let is_write = after_match.trim_start().starts_with('=')
                        && !after_match.trim_start().starts_with("==")
                        && !after_match.trim_start().starts_with("===");

                    let is_dup = refs.iter().any(|r| {
                        r.span.start == prop_start && r.span.end == prop_end
                    });

                    if !is_dup {
                        refs.push(VarRef {
                            name: dollar_name,
                            property_path: None,
                            is_temporary: false,
                            is_write,
                            span: prop_start..prop_end,
                        });
                    }
                }
            }
        }
    }
}

/// Collect (content_text, document_absolute_offset) pairs from all nodes
/// for JS alias scanning. This gives us a flat view of all text that might
/// contain alias declarations and their usages.
fn collect_content_for_alias_scan<'a>(
    nodes: &'a [PassageNode],
    body: &str,
    body_offset: usize,
    pairs: &mut Vec<(&'a str, usize)>,
) {
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                pairs.push((content.as_str(), span.start));
            }
            PassageNode::Macro {
                parsed, children, ..
            } => {
                if !parsed.args.is_empty() {
                    let args_offset = compute_args_offset(parsed, body, body_offset);
                    pairs.push((parsed.args.as_str(), args_offset));
                }
                if let Some(children) = children {
                    collect_content_for_alias_scan(children, body, body_offset, pairs);
                }
            }
            PassageNode::Expression {
                content, span, ..
            } => {
                pairs.push((content.as_str(), span.start));
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Augment with setter link variable refs.
///
/// Setter links: `[[text|passage][$var to value]]` or `[[text|passage][$var = value]]`
/// These assign variables during link navigation, so the variable is a write.
fn augment_setter_link_vars(
    nodes: &[PassageNode],
    _body: &str,
    _body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // Setter links appear inside text segments (in [[...]] syntax).
    // The tree's extract_links_from_text doesn't extract setter vars,
    // and extract_var_refs_from_text doesn't handle setter syntax.
    // We scan text nodes for setter link patterns.
    for node in nodes {
        if let PassageNode::Text { content, span, .. } = node {
            for caps in RE_SETTER_LINK.captures_iter(content) {
                let var_match = caps.get(1).unwrap();
                let full = caps.get(0).unwrap();
                let name = format!("${}", var_match.as_str());

                // Find the $var position within the setter
                let var_rel_start = full.as_str().find('$').unwrap_or(0);
                let var_start = span.start + full.start() + var_rel_start;
                let var_end = var_start + name.len();

                // Skip if already recorded at this span
                let is_dup = refs.iter().any(|r| {
                    r.span.start == var_start && r.span.end <= var_end
                });
                if !is_dup {
                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_setter_link_vars(children, _body, _body_offset, refs);
        }
    }
}

/// Augment with bracket-notation property access.
///
/// `$var["property"]` or `$var['property']` — bracket-notation property access
/// that records both the base variable read and the property path.
fn augment_bracket_prop_vars(
    nodes: &[PassageNode],
    _body: &str,
    _body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // Bracket-notation can appear in text segments and macro args.
    // The basic extraction in the tree doesn't handle this, so we scan
    // both text and macro nodes.
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                let text_offset = span.start;
                scan_bracket_notation(content, text_offset, refs);
            }
            PassageNode::Macro {
                parsed, children, ..
            } => {
                if !parsed.args.is_empty() {
                    let args_offset =
                        compute_args_offset(parsed, _body, _body_offset);
                    scan_bracket_notation(&parsed.args, args_offset, refs);
                }
                if let Some(children) = children {
                    augment_bracket_prop_vars(children, _body, _body_offset, refs);
                }
            }
            PassageNode::Expression {
                content, span, ..
            } => {
                scan_bracket_notation(content, span.start, refs);
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Scan text for bracket-notation property access: `$var["property"]`.
fn scan_bracket_notation(text: &str, text_offset: usize, refs: &mut Vec<VarRef>) {
    for caps in RE_VAR_BRACKET_PROP.captures_iter(text) {
        let var_match = caps.get(1).unwrap();
        let prop_match = caps.get(2).unwrap();
        let full = caps.get(0).unwrap();

        // Skip $$ escape
        if is_dollar_escape(text, full.start()) {
            continue;
        }
        let is_double_dollar = full.as_str().starts_with("$$");
        if is_double_dollar {
            continue;
        }

        let var_start = text_offset + full.start();
        let var_end = text_offset + full.end();

        let base_name = format!("${}", var_match.as_str());
        let prop_path = format!("{}.{}", base_name, prop_match.as_str());

        // Skip if already recorded at this span
        let is_dup = refs.iter().any(|r| {
            r.name == prop_path && r.span.start == var_start
        });
        if !is_dup {
            refs.push(VarRef {
                name: prop_path,
                property_path: Some(prop_match.as_str().to_string()),
                is_temporary: false,
                is_write: false,
                span: var_start..var_end,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// walk_links() — Replace extract_links() + implicit + macro passage refs
// ---------------------------------------------------------------------------

/// Walk the tree and extract all passage links.
///
/// Replaces `extract_links()` + `extract_implicit_passage_refs()` +
/// `extract_macro_passage_refs()` with a single tree walk. Collects:
///
/// - `[[...]]` links from text nodes (already extracted by the tree builder)
/// - Implicit passage refs from text and macro args (`Engine.play()`,
///   `data-passage`, `Story.get()`, `Story.has()`, `UI.goto()`,
///   `UI.include()`, etc.)
/// - Macro passage refs from `<<goto>>`, `<<link>>`, `<<include>>`,
///   `<<button>>`, etc.
///
/// All links are deduplicated by `(display_text, target)` to avoid
/// double-counting the same passage reference.
pub(crate) fn walk_links(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<Link> {
    let mut links: Vec<Link> = Vec::new();

    // Collect [[links]] from tree nodes (already extracted)
    collect_tree_links(nodes, &mut links);

    // Collect implicit passage refs from text and macro args
    collect_implicit_refs(nodes, body, body_offset, &mut links);

    // Collect macro passage refs (<<goto>>, <<link>>, <<include>>, etc.)
    collect_macro_passage_refs(nodes, body, body_offset, &mut links);

    // Deduplicate by (display_text, target)
    let mut seen: HashSet<(Option<String>, String)> = HashSet::new();
    links.retain(|link| {
        let key = (link.display_text.clone(), link.target.clone());
        seen.insert(key)
    });

    links
}

/// Recursively collect [[links]] from text nodes in the tree.
fn collect_tree_links(nodes: &[PassageNode], links: &mut Vec<Link>) {
    for node in nodes {
        match node {
            PassageNode::Text { links: node_links, .. } => {
                links.extend(node_links.iter().cloned());
            }
            PassageNode::Macro { children, .. } => {
                if let Some(children) = children {
                    collect_tree_links(children, links);
                }
            }
            PassageNode::Expression { .. }
            | PassageNode::Heading { .. }
            | PassageNode::Error { .. } => {}
        }
    }
}

/// Collect implicit passage references from text and macro args.
///
/// Detects patterns like `data-passage="..."`, `Engine.play("...")`,
/// `Story.get("...")`, `Story.has("...")`, `UI.goto("...")`,
/// `UI.include("...")` that reference passages but aren't standard
/// `[[links]]` or `<<macro>>` passage-args.
fn collect_implicit_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    links: &mut Vec<Link>,
) {
    use super::links::{
        RE_DATA_PASSAGE, RE_ENGINE_PLAY, RE_ENGINE_GOTO,
        RE_STORY_GET, RE_STORY_PASSAGE, RE_STORY_HAS,
        RE_UI_GOTO, RE_UI_INCLUDE,
    };

    let patterns: &[(&std::sync::LazyLock<regex::Regex>, Option<knot_core::graph::EdgeType>)] = &[
        (&RE_DATA_PASSAGE, None),
        (&RE_ENGINE_PLAY, None),
        (&RE_ENGINE_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_STORY_GET, None),
        (&RE_STORY_PASSAGE, None),
        (&RE_STORY_HAS, None),
        (&RE_UI_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_UI_INCLUDE, Some(knot_core::graph::EdgeType::Include)),
    ];

    for node in nodes {
        // Scan text nodes
        if let PassageNode::Text { content, span, .. } = node {
            let text_offset = span.start;
            for (re, edge_hint) in patterns {
                for caps in re.captures_iter(content) {
                    if let Some(target_match) = caps.get(1) {
                        let full_match = caps.get(0).unwrap();
                        let target = target_match.as_str().trim().to_string();
                        if !target.is_empty() {
                            links.push(Link {
                                display_text: None,
                                target,
                                span: text_offset + full_match.start()
                                    ..text_offset + full_match.end(),
                                edge_type_hint: *edge_hint,
                            });
                        }
                    }
                }
            }
        }

        // Scan macro args
        if let PassageNode::Macro { parsed, .. } = node {
            if !parsed.args.is_empty() {
                let args_offset = compute_args_offset(parsed, body, body_offset);
                for (re, edge_hint) in patterns {
                    for caps in re.captures_iter(&parsed.args) {
                        if let Some(target_match) = caps.get(1) {
                            let full_match = caps.get(0).unwrap();
                            let target = target_match.as_str().trim().to_string();
                            if !target.is_empty() {
                                links.push(Link {
                                    display_text: None,
                                    target,
                                    span: args_offset + full_match.start()
                                        ..args_offset + full_match.end(),
                                    edge_type_hint: *edge_hint,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            collect_implicit_refs(children, body, body_offset, links);
        }
    }
}

/// Collect passage references from macro invocations.
///
/// Uses the macro catalog's `passage_arg_macro_names()` to determine which
/// macros have passage-ref arguments, and `get_passage_arg_index()` to find
/// the correct argument position.
fn collect_macro_passage_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    links: &mut Vec<Link>,
) {
    let passage_arg_macros = super::macros::passage_arg_macro_names();

    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            // Skip close tags
            if parsed.name.starts_with('/') {
                continue;
            }

            let macro_name = parsed.name.as_str();

            // Only process macros that have passage-ref arguments
            if !passage_arg_macros.contains(macro_name) {
                // Recurse into children even if this macro isn't a passage ref
                if let PassageNode::Macro {
                    children: Some(children),
                    ..
                } = node
                {
                    collect_macro_passage_refs(children, body, body_offset, links);
                }
                continue;
            }

            let args_str = parsed.args.as_str();
            if args_str.is_empty() {
                continue;
            }

            // Parse quoted string arguments from the args string.
            let string_args = super::blocks::parse_quoted_args(args_str);

            if string_args.is_empty() {
                continue;
            }

            // Determine which argument is the passage reference.
            let arg_count = string_args.len();
            let passage_idx = super::macros::get_passage_arg_index(macro_name, arg_count);

            if passage_idx < 0 {
                continue;
            }

            let idx = passage_idx as usize;
            if idx < string_args.len() {
                let (content, rel_start, rel_end) = &string_args[idx];
                if !content.is_empty() {
                    let name_end_in_body = parsed.name_start + parsed.name_len;
                    let body_after_name = &body[name_end_in_body..parsed.end.saturating_sub(2)];
                    let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                    let args_offset_in_body = name_end_in_body + trimmed_start;

                    // Classify edge type
                    let edge_type_hint = match macro_name {
                        "goto" => Some(knot_core::graph::EdgeType::Jump),
                        "include" => Some(knot_core::graph::EdgeType::Include),
                        "return" | "back" => Some(knot_core::graph::EdgeType::Navigation),
                        _ => None,
                    };

                    links.push(Link {
                        display_text: None,
                        target: content.clone(),
                        span: body_offset + args_offset_in_body + *rel_start
                            ..body_offset + args_offset_in_body + *rel_end,
                        edge_type_hint,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            collect_macro_passage_refs(children, body, body_offset, links);
        }
    }
}

// ---------------------------------------------------------------------------
// walk_translate() — Replace translate_macros_to_js() + proportional mapping
// ---------------------------------------------------------------------------

/// Exact line mapping from a translated JS line to the original source position.
///
/// Unlike the proportional `LineMapping` which distributes translated lines
/// across original lines by ratio, this mapping is derived directly from the
/// `PassageNode` spans — each JS output line is mapped to the exact source
/// line of the tree node that produced it.
///
/// This is a SugarCube-internal type — not promoted to the core crate.
/// It is converted to `LineMapping` when wiring into the virtual doc pipeline.
#[derive(Debug, Clone)]
pub(crate) struct ExactLineMapping {
    /// The 0-based line number within the original source file.
    pub original_line: u32,
    /// The byte offset of the start of the source construct in the document.
    /// Reserved for byte-precise diagnostics in future walk_validate() enhancements.
    #[allow(dead_code)] // Will be consumed by enhanced diagnostics
    pub original_start_byte: usize,
}

/// Walk the tree and produce translated JS with exact line mapping.
///
/// Replaces `translate_macros_to_js()` (regex-based recursive descent) +
/// `build_format_section_line_map()` (proportional mapping). The tree walk
/// produces **identical JS output** but also records which tree node's span
/// "owns" each output line, giving exact source mapping.
///
/// ## Key differences from the old translator
///
/// - **No regex re-parsing**: The tree already has macro structure, so no
///   `RE_MACRO_TAG` scanning or `find_matching_close()` is needed.
/// - **Exact line mapping**: Each output line maps to the exact source span
///   from the tree node, replacing proportional approximation.
/// - **Same JS output**: The translation dispatch logic is identical to the
///   old translator — we reuse `translate_block_open()`, `translate_close_tag()`,
///   `translate_inline_macro()`, etc. from `virtual_doc.rs`.
///
/// ## Parameters
///
/// - `nodes`: The passage tree from `parse_passage_body()`
/// - `body`: The original passage body text (needed for line number computation)
/// - `body_offset`: The byte offset of the body within the source document
/// - `callables`: User-defined callables (custom macros + widgets)
pub(crate) fn walk_translate(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    callables: &[crate::types::UserCallable],
) -> (String, Vec<ExactLineMapping>) {
    let ctx = super::virtual_doc::TranslationContext::new(callables);
    let mut js_output = String::new();
    let mut line_mappings = Vec::new();

    walk_translate_inner(
        nodes, body, body_offset, &ctx, 0,
        &mut js_output, &mut line_mappings,
    );

    (js_output, line_mappings)
}

/// Inner recursive walk for `walk_translate()`.
///
/// Emits translated JS and records exact line mappings for each output line.
/// The `source_span` parameter tracks which tree node's span "owns" the
/// current output — when we encounter a new node, we update this to that
/// node's span.
fn walk_translate_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    ctx: &super::virtual_doc::TranslationContext,
    indent: usize,
    js_output: &mut String,
    line_mappings: &mut Vec<ExactLineMapping>,
) {
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                // Translate the text segment using the same logic as
                // translate_text_segment() — but emit line mappings.
                let source_line = line_from_span(span.start, body, body_offset);
                let translated = super::virtual_doc::translate_text_segment(content, indent);

                // Each line of the translated output maps to this node's source line
                append_with_mapping(&translated, source_line, span.start, js_output, line_mappings);
            }

            PassageNode::Macro {
                parsed,
                var_refs: _,
                children,
                close_span,
                span,
            } => {
                let macro_name = parsed.name.as_str();
                let args = parsed.args.as_str();
                let source_line = line_from_span(span.start, body, body_offset);

                // Check if this is a block macro (has children)
                let is_block = children.is_some();

                if macro_name == "script" && is_block {
                    // <<script>> blocks: raw JS with $var refs translated
                    let js_body = super::virtual_doc::translate_dollar_refs_in_js(
                        &children.as_ref().unwrap().iter().filter_map(|n| {
                            if let PassageNode::Text { content, .. } = n { Some(content.as_str()) } else { None }
                        }).collect::<Vec<_>>().join(""),
                    );
                    let indent_str = "  ".repeat(indent);
                    for line in js_body.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            let js_line = format!("{}{}\n", indent_str, trimmed);
                            line_mappings.push(ExactLineMapping {
                                original_line: source_line,
                                original_start_byte: span.start,
                            });
                            js_output.push_str(&js_line);
                        }
                    }
                } else if is_block {
                    // Block macro: emit open tag, translate children, emit close tag
                    let open_js = super::virtual_doc::translate_block_open(ctx, macro_name, args, indent);
                    append_with_mapping(&open_js, source_line, span.start, js_output, line_mappings);

                    // Recursively translate children with indent+1
                    if let Some(children) = children {
                        walk_translate_inner(
                            children, body, body_offset, ctx, indent + 1,
                            js_output, line_mappings,
                        );
                    }

                    // Close tag
                    if let Some(close_span) = close_span {
                        let close_line = line_from_span(close_span.start, body, body_offset);
                        let close_js = super::virtual_doc::translate_close_tag(macro_name, indent);
                        append_with_mapping(&close_js, close_line, close_span.start, js_output, line_mappings);
                    }
                } else if super::virtual_doc::is_block_macro(ctx, macro_name) {
                    // Block macro that has no children (unclosed) — emit open only
                    let open_js = super::virtual_doc::translate_block_open(ctx, macro_name, args, indent);
                    append_with_mapping(&open_js, source_line, span.start, js_output, line_mappings);
                } else if ctx.builtin_lookup.contains_key(macro_name) || macro_name == "when" {
                    // Inline builtin macro
                    let inline_js = super::virtual_doc::translate_inline_macro(ctx, macro_name, args, indent);
                    append_with_mapping(&inline_js, source_line, span.start, js_output, line_mappings);
                } else if ctx.callable_names.contains(macro_name) {
                    // User-defined callable
                    let indent_str = "  ".repeat(indent);
                    let translated_args = if args.is_empty() {
                        String::new()
                    } else {
                        super::virtual_doc::translate_expression(args)
                    };
                    let callable_js = if translated_args.is_empty() {
                        format!("{}{}();\n", indent_str, macro_name)
                    } else {
                        format!("{}{}({});\n", indent_str, macro_name, translated_args)
                    };
                    append_with_mapping(&callable_js, source_line, span.start, js_output, line_mappings);
                } else {
                    // Unknown macro
                    let indent_str = "  ".repeat(indent);
                    let full_tag = format!("<<{}{}{}>>",
                        macro_name,
                        if args.is_empty() { "" } else { " " },
                        args
                    );
                    let unknown_js = format!("{}/* unknown: {} */;\n", indent_str, full_tag);
                    append_with_mapping(&unknown_js, source_line, span.start, js_output, line_mappings);
                }
            }

            PassageNode::Expression { content, span, .. } => {
                // Expression macro: <<=>> or <<->>
                let source_line = line_from_span(span.start, body, body_offset);
                let indent_str = "  ".repeat(indent);
                let translated_expr = super::virtual_doc::translate_expression(content);
                let expr_js = format!("{}/* print: {} */;\n", indent_str, translated_expr);
                append_with_mapping(&expr_js, source_line, span.start, js_output, line_mappings);
            }

            PassageNode::Heading { span, .. } => {
                // Headings don't produce JS output in the current translator
                let _ = span;
            }

            PassageNode::Error { message, span } => {
                let source_line = line_from_span(span.start, body, body_offset);
                let indent_str = "  ".repeat(indent);
                let error_js = format!("{}/* error: {} */;\n", indent_str, message);
                append_with_mapping(&error_js, source_line, span.start, js_output, line_mappings);
            }
        }
    }
}

/// Append translated JS text to the output, recording a line mapping for
/// each line emitted. All lines in this chunk map to the same source line.
///
/// Handles the case where `js_text` may contain blank lines (e.g., from
/// `translate_text_segment` which emits `\n` for empty source lines).
/// We split on `\n` and track each line, including empty ones.
fn append_with_mapping(
    js_text: &str,
    source_line: u32,
    source_start_byte: usize,
    js_output: &mut String,
    line_mappings: &mut Vec<ExactLineMapping>,
) {
    // Handle the special case where js_text is a single newline
    // (blank line from translate_text_segment)
    if js_text == "\n" {
        line_mappings.push(ExactLineMapping {
            original_line: source_line,
            original_start_byte: source_start_byte,
        });
        js_output.push('\n');
        return;
    }

    let mut line_start = 0;
    for (i, ch) in js_text.char_indices() {
        if ch == '\n' {
            let line = &js_text[line_start..i];
            line_mappings.push(ExactLineMapping {
                original_line: source_line,
                original_start_byte: source_start_byte,
            });
            js_output.push_str(line);
            js_output.push('\n');
            line_start = i + 1;
        }
    }
    // Handle trailing content without newline
    if line_start < js_text.len() {
        let line = &js_text[line_start..];
        line_mappings.push(ExactLineMapping {
            original_line: source_line,
            original_start_byte: source_start_byte,
        });
        js_output.push_str(line);
        js_output.push('\n');
    }
}

/// Compute the 0-based line number from a document-absolute byte offset.
///
/// Counts the number of `\n` characters in `body[..offset_within_body]`
/// to determine the line number. This is more reliable than `.lines()`
/// because `.lines()` doesn't count trailing empty lines.
fn line_from_span(doc_offset: usize, body: &str, body_offset: usize) -> u32 {
    let body_relative = doc_offset.saturating_sub(body_offset);
    let safe_end = body_relative.min(body.len());
    if safe_end == 0 {
        return 0;
    }
    // Count newlines before the offset position
    body[..safe_end].bytes().filter(|&b| b == b'\n').count() as u32
}

// ---------------------------------------------------------------------------
// walk_passage_var_refs() — Tree-based replacement for extract_virtual_var_accesses
// ---------------------------------------------------------------------------

/// Walk the tree and produce `PassageVarRef` entries with exact line numbers.
///
/// Replaces the old `extract_virtual_var_accesses()` path which:
/// 1. Built the entire virtual document (translating ALL passages to JS)
/// 2. Ran regex on the JS output to find `State.variables.x` patterns
/// 3. Mapped back to source lines via the (lossy) proportional line map
///
/// This function instead:
/// 1. Walks the tree directly (no virtual document build needed)
/// 2. Uses `walk_vars()` which already handles all var patterns
/// 3. Computes exact line numbers from byte spans (no proportional mapping)
///
/// The result is both faster (no full vdoc build) and more accurate (exact
/// line numbers instead of proportional approximation).
pub(crate) fn walk_passage_var_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    passage_name: &str,
    file_uri: &str,
) -> Vec<crate::types::PassageVarRef> {
    let var_ops = walk_vars(nodes, body, body_offset);

    var_ops
        .into_iter()
        .filter(|v| !v.is_temporary)
        .map(|v| {
            let line = line_from_span(v.span.start, body, body_offset);
            crate::types::PassageVarRef {
                variable_name: v.name,
                is_write: matches!(v.kind, VarKind::Init),
                line,
                file_uri: file_uri.to_string(),
                passage_name: passage_name.to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VarRef → VarOp conversion (used by walk_vars() in Phase 2)
// ---------------------------------------------------------------------------

/// Convert a `VarRef` to a `VarOp` for the core `Passage.vars` field.
///
/// This is a straightforward mapping:
/// - `is_write = true` → `VarKind::Init`
/// - `is_write = false` → `VarKind::Read`
/// - `is_temporary` passes through
/// - `name` is the full name (including property path if present)
pub(crate) fn var_ref_to_var_op(var_ref: &VarRef) -> VarOp {
    let full_name = match &var_ref.property_path {
        Some(path) => format!("{}.{}", var_ref.name, path),
        None => var_ref.name.clone(),
    };
    VarOp {
        name: full_name,
        kind: if var_ref.is_write {
            VarKind::Init
        } else {
            VarKind::Read
        },
        span: var_ref.span.clone(),
        is_temporary: var_ref.is_temporary,
    }
}

// ---------------------------------------------------------------------------
// walk_validate() — Tree-based diagnostics (replaces validation::validate)
// ---------------------------------------------------------------------------

/// Walk the tree and produce syntax/semantic diagnostics.
///
/// Replaces the three separate passes in `validation::validate()`:
/// 1. `validate_macro_brackets()` — unclosed `<<` / `>>`
/// 2. `validate_link_brackets()` — unclosed `[[` / `]]`
/// 3. `validate_macro_structure()` — structural + unknown + deprecated checks
///
/// The tree already contains the structural information, so this walk:
/// - Reports `Error` nodes as unclosed/malformed constructs
/// - Reports unknown macros (not in `known_macro_names()`)
/// - Reports deprecated macros (in `deprecated_macros()`)
/// - Reports structural constraint violations (modifier macros outside
///   their required parent block)
/// - Reports unclosed block macros (Macro nodes with `close_span = None`)
pub(crate) fn walk_validate(
    nodes: &[PassageNode],
    body_offset: usize,
) -> Vec<crate::plugin::FormatDiagnostic> {
    use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};

    let constraints = super::macros::structural_constraints();
    let deprecated = super::macros::deprecated_macros();
    let known_macros = super::macros::known_macro_names();

    let mut diagnostics = Vec::new();
    walk_validate_inner(
        nodes,
        body_offset,
        &constraints,
        &deprecated,
        &known_macros,
        &Vec::new(), // parent stack at root level
        &mut diagnostics,
    );
    diagnostics
}

fn walk_validate_inner(
    nodes: &[PassageNode],
    body_offset: usize,
    constraints: &std::collections::HashMap<&str, std::collections::HashSet<&str>>,
    deprecated: &std::collections::HashMap<&str, &str>,
    known_macros: &std::collections::HashSet<&str>,
    parent_stack: &[String], // names of currently-open block macros
    diagnostics: &mut Vec<crate::plugin::FormatDiagnostic>,
) {
    use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};

    for node in nodes {
        match node {
            PassageNode::Error { message, span } => {
                diagnostics.push(FormatDiagnostic {
                    range: span.start..span.end,
                    message: message.clone(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-parse-error".into(),
                });
            }

            PassageNode::Macro {
                parsed,
                children,
                close_span,
                span,
                ..
            } => {
                let macro_name = parsed.name.as_str();
                let is_block = children.is_some();
                let doc_start = span.start;
                let doc_end = span.start + (parsed.end - parsed.start);

                // ── Deprecated macro warning ──────────────────────────
                if let Some(msg) = deprecated.get(macro_name) {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!("Deprecated macro: {}", msg),
                        severity: FormatDiagnosticSeverity::Info,
                        code: "sc-deprecated-macro".into(),
                    });
                }

                // ── Unknown macro hint ────────────────────────────────
                if !known_macros.contains(macro_name) {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!("Unknown SugarCube macro `<<{}>>`", macro_name),
                        severity: FormatDiagnosticSeverity::Hint,
                        code: "sc-unknown-macro".into(),
                    });
                }

                // ── Structural constraint check ──────────────────────
                // Modifier macros (else, elseif, case, default) must be
                // inside their parent block.
                if let Some(valid_parents) = constraints.get(macro_name) {
                    let has_valid_parent = parent_stack.iter().rev().any(|p| {
                        valid_parents.contains(p.as_str())
                    });
                    if !has_valid_parent {
                        let parent_list: Vec<String> = valid_parents
                            .iter()
                            .map(|p| format!("`<<{}>>`", p))
                            .collect();
                        diagnostics.push(FormatDiagnostic {
                            range: doc_start..doc_end,
                            message: format!(
                                "`<<{}>>` must be inside {}",
                                macro_name,
                                parent_list.join(" or ")
                            ),
                            severity: FormatDiagnosticSeverity::Error,
                            code: "sc-container-structure".into(),
                        });
                    }
                }

                // ── Unclosed block macro warning ──────────────────────
                if is_block && close_span.is_none() {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!(
                            "Unclosed block macro `<<{}>>` — missing `<</{}>>`",
                            macro_name, macro_name
                        ),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sc-unclosed-block".into(),
                    });
                }

                // ── Recurse into children ─────────────────────────────
                if let Some(children) = children {
                    let mut new_stack = parent_stack.to_vec();
                    if super::macros::is_block_macro(macro_name) {
                        new_stack.push(macro_name.to_string());
                    }
                    walk_validate_inner(
                        children,
                        body_offset,
                        constraints,
                        deprecated,
                        known_macros,
                        &new_stack,
                        diagnostics,
                    );
                }
            }

            PassageNode::Text { .. } | PassageNode::Expression { .. } | PassageNode::Heading { .. } => {
                // No diagnostics for text, expression, or heading nodes
            }
        }
    }
}

// ---------------------------------------------------------------------------
// walk_tokens() — Tree-based semantic tokens (replaces tokens::body_tokens)
// ---------------------------------------------------------------------------

/// Walk the tree and produce semantic tokens for the passage body.
///
/// Replaces `tokens::body_tokens()` which re-scans with `blocks::scan_macros()`
/// and multiple regex passes. This walk uses the tree's structural information
/// directly:
///
/// - **Macro tokens**: Name position from `ParsedMacro.name_start/name_len`
/// - **Variable tokens**: From `VarRef` entries on tree nodes (write = Definition)
/// - **Link tokens**: From `Link` entries on Text nodes
///
/// Additional token types (keyword, boolean, namespace, number, string, operator,
/// property, PassageRef) still use the existing `tokens.rs` helpers because they
/// require text-level scanning that doesn't benefit from tree structure. These
/// are called as augmentation passes after the tree-based core tokens.
pub(crate) fn walk_tokens(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<crate::plugin::SemanticToken> {
    use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};

    let mut tokens = Vec::new();
    walk_tokens_inner(nodes, body_offset, &mut tokens);

    tokens
}

fn walk_tokens_inner(
    nodes: &[PassageNode],
    body_offset: usize,
    tokens: &mut Vec<crate::plugin::SemanticToken>,
) {
    use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};

    for node in nodes {
        match node {
            PassageNode::Text { links, var_refs, span, .. } => {
                // Variable tokens from text nodes (reads)
                for vr in var_refs {
                    let is_init = vr.is_write;
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if is_init {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }

                // Link tokens — highlight only the passage name portion
                for link in links {
                    // The link.target is the passage name. We need to find its
                    // byte position within the link span.
                    // For [[Target]], highlight just "Target"
                    // For [[Display->Target]], highlight just "Target"
                    // For [[Display|Target]], highlight just "Target"
                    //
                    // The tree builder already computes link spans covering the
                    // full [[...]] construct. We need to find the target portion.
                    // Since we have the full span and the target string, we can
                    // search for the target within the span.
                    let span_text_offset = vr_span_to_text_offset(&link.span, body_offset);
                    // We can't easily slice body here without the original text.
                    // Instead, emit a token for the target name found within the span.
                    // The caller (mod.rs) has access to body and can compute positions.
                    // For now, emit a Link token covering the target portion.
                    // This is handled by the augmentation passes in tokens.rs.
                    // The tree-based walk provides the structural tokens (Macro, Variable)
                    // and delegates link/passage-ref tokens to the existing helpers.
                }
            }

            PassageNode::Macro {
                parsed,
                var_refs,
                children,
                ..
            } => {
                // Macro name token — highlight only the name, not <<args>>
                tokens.push(SemanticToken {
                    start: body_offset + parsed.name_start,
                    length: parsed.name_len,
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });

                // Variable tokens from macro args
                for vr in var_refs {
                    let is_init = vr.is_write;
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if is_init {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }

                // Recurse into children
                if let Some(children) = children {
                    walk_tokens_inner(children, body_offset, tokens);
                }
            }

            PassageNode::Expression { name, var_refs, span, .. } => {
                // Expression macros (<<=>> / <<->>) get a Macro token for
                // the name. The name position can be derived from the span:
                // <<name>> → name is at span.start + 2, length = name.len()
                tokens.push(SemanticToken {
                    start: span.start + 2, // skip "<<"
                    length: name.len(),
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });

                // Variable tokens from expression args
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if vr.is_write {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }
            }

            PassageNode::Heading { .. } | PassageNode::Error { .. } => {
                // No semantic tokens for headings or errors
            }
        }
    }
}

/// Helper: convert a VarRef-style span back to text-relative offset.
/// Not currently used directly — link token positions are computed by
/// the augmentation passes in `tokens.rs`.
#[allow(dead_code)]
fn vr_span_to_text_offset(span: &std::ops::Range<usize>, body_offset: usize) -> usize {
    span.start.saturating_sub(body_offset)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_text() {
        let nodes = parse_passage_body("Hello world", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { content, .. } => assert_eq!(content, "Hello world"),
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_empty_body() {
        let nodes = parse_passage_body("   ", 0);
        assert!(nodes.is_empty());
    }

    #[test]
    fn parse_simple_macro() {
        let nodes = parse_passage_body("<<set $x to 5>>", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { parsed, children, .. } => {
                assert_eq!(parsed.name, "set");
                assert_eq!(parsed.args, "$x to 5");
                assert!(children.is_none()); // inline macro
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_macro_with_text() {
        let nodes = parse_passage_body("Hello <<set $x to 5>> world", 0);
        assert_eq!(nodes.len(), 3);
        match &nodes[0] {
            PassageNode::Text { content, .. } => assert_eq!(content, "Hello "),
            _ => panic!("Expected Text node at 0"),
        }
        match &nodes[1] {
            PassageNode::Macro { parsed, .. } => assert_eq!(parsed.name, "set"),
            _ => panic!("Expected Macro node at 1"),
        }
        match &nodes[2] {
            PassageNode::Text { content, .. } => assert_eq!(content, " world"),
            _ => panic!("Expected Text node at 2"),
        }
    }

    #[test]
    fn parse_block_macro() {
        let body = "<<if $x>>yes<</if>>";
        let nodes = parse_passage_body(body, 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro {
                parsed, children, close_span, ..
            } => {
                assert_eq!(parsed.name, "if");
                assert!(children.is_some());
                assert!(close_span.is_some());
                let children = children.as_ref().unwrap();
                assert_eq!(children.len(), 1);
                match &children[0] {
                    PassageNode::Text { content, .. } => assert_eq!(content, "yes"),
                    _ => panic!("Expected Text child"),
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_if_else() {
        let body = "<<if $x>>yes<<else>>no<</if>>";
        let nodes = parse_passage_body(body, 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { parsed, children, .. } => {
                assert_eq!(parsed.name, "if");
                let children = children.as_ref().unwrap();
                // Phase 1: else is a sibling modifier within the if block
                assert_eq!(children.len(), 3);
                match &children[0] {
                    PassageNode::Text { content, .. } => assert_eq!(content, "yes"),
                    _ => panic!("Expected Text at 0"),
                }
                match &children[1] {
                    PassageNode::Macro { parsed, .. } => assert_eq!(parsed.name, "else"),
                    _ => panic!("Expected Macro(else) at 1"),
                }
                match &children[2] {
                    PassageNode::Text { content, .. } => assert_eq!(content, "no"),
                    _ => panic!("Expected Text at 2"),
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_expression_macro() {
        let body = "<<= $x + 1 >>";
        let nodes = parse_passage_body(body, 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Expression { name, content, .. } => {
                assert_eq!(name, "=");
                assert_eq!(content, "$x + 1");
            }
            _ => panic!("Expected Expression node"),
        }
    }

    #[test]
    fn parse_var_refs_in_text() {
        let nodes = parse_passage_body("You have $gold coins.", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { var_refs, .. } => {
                assert_eq!(var_refs.len(), 1);
                assert_eq!(var_refs[0].name, "$gold");
                assert!(!var_refs[0].is_write);
                assert!(!var_refs[0].is_temporary);
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_var_refs_in_set_macro() {
        let nodes = parse_passage_body("<<set $x to 5>>", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { var_refs, .. } => {
                assert_eq!(var_refs.len(), 1);
                assert_eq!(var_refs[0].name, "$x");
                assert!(var_refs[0].is_write); // set = write
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_var_refs_in_if_macro() {
        let nodes = parse_passage_body("<<if $ready>>ok<</if>>", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { var_refs, .. } => {
                assert_eq!(var_refs.len(), 1);
                assert_eq!(var_refs[0].name, "$ready");
                assert!(!var_refs[0].is_write); // if = read
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_temp_var() {
        let nodes = parse_passage_body("<<for _i to 0>><</for>>", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { var_refs, .. } => {
                assert_eq!(var_refs.len(), 1);
                assert_eq!(var_refs[0].name, "_i");
                assert!(var_refs[0].is_temporary);
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_dollar_dollar_escape() {
        let nodes = parse_passage_body("$$notavar is text", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { var_refs, .. } => {
                // $$notavar should NOT be detected as a variable reference
                assert!(var_refs.is_empty());
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_dot_path_var() {
        let nodes = parse_passage_body("You have $item.sword.", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { var_refs, .. } => {
                assert_eq!(var_refs.len(), 1);
                assert_eq!(var_refs[0].name, "$item");
                assert_eq!(var_refs[0].property_path.as_deref(), Some("sword"));
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_links_in_text() {
        let nodes = parse_passage_body("Go to [[Forest]].", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { links, .. } => {
                assert_eq!(links.len(), 1);
                assert_eq!(links[0].target, "Forest");
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_arrow_link() {
        let nodes = parse_passage_body("[[Go north->Forest]]", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Text { links, .. } => {
                assert_eq!(links.len(), 1);
                assert_eq!(links[0].target, "Forest");
                assert_eq!(links[0].display_text.as_deref(), Some("Go north"));
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn walk_blocks_produces_flat_list() {
        let body = "<<if $x>>yes<<else>>no<</if>>";
        let nodes = parse_passage_body(body, 0);
        let blocks = walk_blocks(&nodes);

        // Should produce: Macro(if), Text(yes), Macro(else), Text(no), Macro(/if)
        assert_eq!(blocks.len(), 5);
        match &blocks[0] {
            Block::Macro { name, .. } => assert_eq!(name, "if"),
            _ => panic!("Expected Macro at 0"),
        }
        match &blocks[1] {
            Block::Text { content, .. } => assert_eq!(content, "yes"),
            _ => panic!("Expected Text at 1"),
        }
        match &blocks[2] {
            Block::Macro { name, .. } => assert_eq!(name, "else"),
            _ => panic!("Expected Macro(else) at 2"),
        }
        match &blocks[3] {
            Block::Text { content, .. } => assert_eq!(content, "no"),
            _ => panic!("Expected Text at 3"),
        }
        match &blocks[4] {
            Block::Macro { name, .. } => assert_eq!(name, "/if"),
            _ => panic!("Expected Macro(/if) at 4"),
        }
    }

    #[test]
    fn walk_blocks_expression_as_macro() {
        let body = "<<= $x + 1 >>";
        let nodes = parse_passage_body(body, 0);
        let blocks = walk_blocks(&nodes);

        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Macro { name, args, .. } => {
                assert_eq!(name, "=");
                assert_eq!(args, "$x + 1");
            }
            _ => panic!("Expected Macro block for expression"),
        }
    }

    #[test]
    fn walk_blocks_matches_current_system() {
        // This test validates that walk_blocks produces the same output
        // as the current build_body_blocks() for a simple case.
        let body = "Hello <<set $x to 5>> world";
        let nodes = parse_passage_body(body, 0);
        let blocks = walk_blocks(&nodes);

        // Current system produces:
        // Text("Hello "), Macro("set", "$x to 5"), Text(" world")
        assert_eq!(blocks.len(), 3);
        match &blocks[0] {
            Block::Text { content, .. } => assert_eq!(content, "Hello "),
            _ => panic!("Expected Text at 0"),
        }
        match &blocks[1] {
            Block::Macro { name, args, .. } => {
                assert_eq!(name, "set");
                assert_eq!(args, "$x to 5");
            }
            _ => panic!("Expected Macro at 1"),
        }
        match &blocks[2] {
            Block::Text { content, .. } => assert_eq!(content, " world"),
            _ => panic!("Expected Text at 2"),
        }
    }

    #[test]
    fn var_ref_to_var_op_conversion() {
        let var_ref = VarRef {
            name: "$item".to_string(),
            property_path: Some("sword.damage".to_string()),
            is_temporary: false,
            is_write: true,
            span: 10..30,
        };
        let var_op = var_ref_to_var_op(&var_ref);
        assert_eq!(var_op.name, "$item.sword.damage");
        assert_eq!(var_op.kind, VarKind::Init);
        assert!(!var_op.is_temporary);
        assert_eq!(var_op.span, 10..30);
    }

    #[test]
    fn parse_unclosed_block() {
        let body = "<<if $x>>yes";
        let nodes = parse_passage_body(body, 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro {
                parsed, children, close_span, ..
            } => {
                assert_eq!(parsed.name, "if");
                assert!(close_span.is_none()); // unclosed!
                let children = children.as_ref().unwrap();
                assert_eq!(children.len(), 1);
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn parse_nested_blocks() {
        let body = "<<if $a>>outer<<if $b>>inner<</if>><</if>>";
        let nodes = parse_passage_body(body, 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { parsed, children, .. } => {
                assert_eq!(parsed.name, "if");
                let children = children.as_ref().unwrap();
                assert_eq!(children.len(), 2); // "outer" + inner <<if>>
                match &children[1] {
                    PassageNode::Macro { parsed, children, .. } => {
                        assert_eq!(parsed.name, "if");
                        let inner_children = children.as_ref().unwrap();
                        assert_eq!(inner_children.len(), 1); // "inner"
                    }
                    _ => panic!("Expected inner Macro node"),
                }
            }
            _ => panic!("Expected outer Macro node"),
        }
    }

    #[test]
    fn parse_increment_decrement() {
        let nodes = parse_passage_body("<<set $x++>>", 0);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            PassageNode::Macro { var_refs, .. } => {
                // $x++ should be detected as a write
                let x_ref = var_refs.iter().find(|r| r.name == "$x").unwrap();
                assert!(x_ref.is_write);
            }
            _ => panic!("Expected Macro node"),
        }
    }
}
