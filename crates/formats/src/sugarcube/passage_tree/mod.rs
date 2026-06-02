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

mod walk_vars;
mod walk_links;
mod walk_translate;
mod walk_validate;
mod walk_tokens;
mod walk_tokens_augment;

// Re-exports: all public items from submodules are available via `passage_tree::`
#[allow(unused_imports)] // Re-exports used by other crate modules and tests
pub(crate) use walk_vars::{walk_vars, walk_passage_var_refs, var_ref_to_var_op};
pub(crate) use walk_links::walk_links;
pub(crate) use walk_translate::{
    walk_translate, TranslateResult, VarEncounter, VarTypeHint, VarAccessKind,
    ExactLineMapping,
};
pub(crate) use walk_validate::walk_validate;
pub(crate) use walk_tokens::walk_tokens;
pub(crate) use walk_tokens_augment::{walk_augment_tokens, walk_macro_passage_ref_tokens};

use std::ops::Range;

use knot_core::passage::{Block, Link};

use super::blocks::{scan_macros, ParsedMacro};
use super::links::{RE_LINK_ARROW, RE_LINK_PIPE, RE_LINK_SIMPLE};
use super::macros::{block_macro_names, folding_modifier_names};
use super::vars::{
    RE_TEMP_VAR, RE_VAR, RE_VAR_DECREMENT, RE_VAR_DOT_PATH, RE_VAR_INCREMENT,
};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// SugarCube-internal tree representation of a passage body node.
///
/// Lives in `crates/formats/src/sugarcube/passage_tree/` — NOT in the
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
pub(crate) fn is_js_bracket_context(text: &str, pos: usize) -> bool {
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
pub(crate) fn is_dollar_escape(text: &str, match_start: usize) -> bool {
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
pub(crate) const WRITE_MACRO_NAMES: &[&str] = &["set", "capture", "unset"];

/// Compute the byte offset of the (trimmed) arguments string within the
/// source document, given the ParsedMacro and body_offset.
///
/// The ParsedMacro stores `args` as the trimmed content between the macro
/// name and the closing `>>`. We need the exact byte offset of the first
/// non-whitespace character of the args within the body, then add
/// `body_offset` to make it document-absolute.
pub(crate) fn compute_args_offset(m: &ParsedMacro, body: &str, body_offset: usize) -> usize {
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
        use knot_core::passage::VarKind;

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
