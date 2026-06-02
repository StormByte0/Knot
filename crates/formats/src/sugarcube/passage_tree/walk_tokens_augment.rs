//! Tree-based augmentation token walks for the passage tree.
//!
//! Replaces the 10 standalone `tokens::` augmentation functions that each
//! call `blocks::scan_macros()` independently (~9× redundant body rescan
//! per passage). These walks use the existing `Vec<PassageNode>` tree
//! which already has macro positions, args, and nesting — zero rescanning.
//!
//! ## What this replaces
//!
//! | Old function             | New tree walk           | Eliminates          |
//! |--------------------------|-------------------------|---------------------|
//! | `keyword_tokens()`       | `walk_keyword_tokens()` | `scan_macros()`     |
//! | `boolean_tokens()`       | `walk_boolean_tokens()` | `scan_macros()`     |
//! | `namespace_tokens()`     | `walk_namespace_tokens()`| `scan_macros()`    |
//! | `widget_tokens()`        | `walk_widget_tokens()`  | `scan_macros()`     |
//! | `number_tokens()`        | `walk_number_tokens()`  | `scan_macros()`     |
//! | `string_tokens()`        | `walk_string_tokens()`  | `scan_macros()`     |
//! | `operator_tokens()`      | `walk_operator_tokens()`| `scan_macros()`     |
//! | `property_tokens()`      | `walk_property_tokens()`| regex on full body  |
//! | `script_passage_ref_tokens()` | `walk_passage_ref_tokens()` | 8 regexes on full body |
//! | `macro_passage_ref_tokens()`  | (merged into above)   | `scan_macros()`     |
//!
//! ## Architecture
//!
//! The core insight: every macro arg token MUST appear inside a `<<macro args>>`
//! construct, and the tree already knows every macro's position and args string.
//! So instead of `scan_macros()` + body scan, we walk the tree and scan only
//! each node's `parsed.args` string (or `content` for Text/Expression nodes).
//!
//! For Text nodes, only `property_tokens` and `passage_ref_tokens` are relevant
//! (keywords/booleans/namespaces/operators/numbers/strings only make sense
//! inside macro args, not in passage body prose).

use std::sync::LazyLock;

use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};

use super::PassageNode;
use crate::sugarcube::blocks;
use crate::sugarcube::macros;

// ---------------------------------------------------------------------------
// SugarCube keyword/operator/boolean/namespace sets
// (Duplicated from tokens.rs — these are SugarCube-specific constants,
// not tree-specific logic. We could move them to a shared location, but
// duplicating is simpler and avoids modifying tokens.rs which still has
// live callers for script/interface passages.)
// ---------------------------------------------------------------------------

const SUGARCUBE_KEYWORDS: &[&str] = &[
    "to", "into", "is", "isnot", "eq", "neq", "gt", "lt", "gte", "lte",
    "and", "or", "not", "ne", "e", "a", "b", "c",
    "from", "near", "far", "match",
];

const SUGARCUBE_BOOLEANS: &[&str] = &["true", "false"];

const SUGARCUBE_NAMESPACES: &[&str] = &[
    "State", "Engine", "Story", "Dialog", "settings",
    "setup", "Config", "UI", "Macros", "SimpleAPI",
];

const SUGARCUBE_OPERATORS: &[&str] = &["+=", "-=", "*=", "/=", "%="];

static RE_NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(\d+(?:\.\d+)?)").unwrap()
});

static RE_PROPERTY: LazyLock<regex::Regex> = LazyLock::new(|| {
    let ns_pattern = SUGARCUBE_NAMESPACES.join("|");
    regex::Regex::new(&format!(
        r"(?:{})\.([A-Za-z_][A-Za-z0-9_]*)",
        ns_pattern
    )).unwrap()
});

// ---------------------------------------------------------------------------
// Convenience: walk ALL augmentation tokens in one call
// ---------------------------------------------------------------------------

/// Walk the tree and produce ALL augmentation semantic tokens.
///
/// This is the single entry point for the normal passage path in `mod.rs`.
/// It replaces the 10 individual `tokens::*_tokens()` calls with one
/// tree walk that produces all token types at once.
///
/// For Text nodes, only property and passage-ref tokens are emitted
/// (keywords/booleans/etc. only make sense inside macro args).
pub(crate) fn walk_augment_tokens(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    walk_augment_tokens_inner(nodes, body, body_offset, &mut tokens);
    tokens
}

fn walk_augment_tokens_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    tokens: &mut Vec<SemanticToken>,
) {
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                // Text nodes: only property + passage-ref tokens make sense
                scan_property_tokens(content, span.start, tokens);
                scan_passage_ref_tokens(content, span.start, tokens);
            }

            PassageNode::Macro {
                parsed,
                children,
                ..
            } => {
                let macro_name = parsed.name.as_str();

                // Skip close tags — they have no args
                if !macro_name.starts_with('/') {
                    let args_str = parsed.args.as_str();
                    if !args_str.is_empty() {
                        let args_offset =
                            super::compute_args_offset(parsed, body, body_offset);

                        // All 8 arg-level token types
                        scan_keyword_tokens(args_str, args_offset, tokens);
                        scan_boolean_tokens(args_str, args_offset, tokens);
                        scan_namespace_tokens(args_str, args_offset, tokens);
                        scan_number_tokens(args_str, args_offset, tokens);
                        scan_string_tokens(args_str, args_offset, tokens);
                        scan_operator_tokens(args_str, args_offset, tokens);
                        scan_property_tokens(args_str, args_offset, tokens);
                        scan_passage_ref_tokens(args_str, args_offset, tokens);
                        // Widget tokens — only for macro-definition macros
                        scan_widget_tokens(macro_name, args_str, args_offset, tokens);
                    }
                }

                // Recurse into children
                if let Some(children) = children {
                    walk_augment_tokens_inner(children, body, body_offset, tokens);
                }
            }

            PassageNode::Expression {
                content,
                span,
                ..
            } => {
                // Expression macros (<<=>> / <<->>) have args that can contain
                // all the same token types as regular macro args
                if !content.is_empty() {
                    // Expression args span: skip "<<name " prefix
                    // The content is the expression text; span covers the whole
                    // <<name expr>> construct. The expression text starts at
                    // span.start + 2 + name.len() + whitespace.
                    // For simplicity, we use the same offset calculation as
                    // regular macros would — but Expression doesn't have
                    // ParsedMacro fields. We can approximate: the expression
                    // content starts after the `<<= ` or `<<- ` prefix.
                    //
                    // Actually, for Expression nodes the content IS the args
                    // string and the span covers the full construct. The
                    // content's doc-absolute offset = span.start + 2 (<<) +
                    // name.len() + whitespace. But we don't store the name
                    // offset separately. The simplest correct approach:
                    // scan using content with offset = span.start, then
                    // the tokens will be relative to the start of the
                    // construct. This isn't perfectly precise for the
                    // content offset, but since Expression nodes are short
                    // (just `<<= expr>>`), the visual result is fine.
                    //
                    // Actually, let me compute it properly. The structure is:
                    //   <<name content>>
                    //   ^span.start     ^span.end
                    // name is either "=" or "-".
                    // content starts at span.start + 2 + name.len() + whitespace.
                    let name_len = if span.end - span.start > 4 { 1 } else { 1 }; // "=" or "-"
                    let prefix_len = 2 + name_len; // "<<=" or "<<-"
                    // Find first non-space after prefix
                    let full_text_offset = span.start;
                    // We need to find the content within the body
                    let body_rel_start = span.start.saturating_sub(body_offset);
                    if body_rel_start + prefix_len <= body.len() {
                        let after_prefix = &body[body_rel_start + prefix_len..body_rel_start + span.end - span.start - 2];
                        let ws = after_prefix.len() - after_prefix.trim_start().len();
                        let content_offset = full_text_offset + prefix_len + ws;
                        scan_keyword_tokens(content, content_offset, tokens);
                        scan_boolean_tokens(content, content_offset, tokens);
                        scan_namespace_tokens(content, content_offset, tokens);
                        scan_number_tokens(content, content_offset, tokens);
                        scan_string_tokens(content, content_offset, tokens);
                        scan_operator_tokens(content, content_offset, tokens);
                        scan_property_tokens(content, content_offset, tokens);
                        scan_passage_ref_tokens(content, content_offset, tokens);
                    }
                }
            }

            PassageNode::Heading { .. } | PassageNode::Error { .. } => {
                // No augmentation tokens for headings or errors
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Individual token scanners
// ---------------------------------------------------------------------------
//
// Each scanner takes a text string and an offset, scans for its specific
// token type, and appends to the tokens vec. The offset is the
// document-absolute byte position of the start of `text`.
//
// These are the same algorithms as in `tokens.rs` but operate on individual
// text segments (macro args or text content) instead of the full body.

/// Scan for SugarCube keywords (to, is, eq, and, or, not, etc.)
fn scan_keyword_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    for keyword in SUGARCUBE_KEYWORDS {
        let kw_bytes = keyword.as_bytes();
        let kw_len = kw_bytes.len();
        if kw_len == 0 { continue; }

        let mut pos = 0;
        while pos + kw_len <= len {
            if &bytes[pos..pos + kw_len] == kw_bytes {
                let before_ok = pos == 0
                    || (!bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_');
                let after_pos = pos + kw_len;
                let after_ok = after_pos >= len
                    || (!bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_');

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: offset + pos,
                        length: kw_len,
                        token_type: SemanticTokenType::Keyword,
                        modifier: Some(SemanticTokenModifier::ControlFlow),
                    });
                }
                pos += kw_len;
            } else {
                pos += 1;
            }
        }
    }
}

/// Scan for boolean literals (true, false)
fn scan_boolean_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    for boolean in SUGARCUBE_BOOLEANS {
        let bool_bytes = boolean.as_bytes();
        let bool_len = bool_bytes.len();

        let mut pos = 0;
        while pos + bool_len <= len {
            if &bytes[pos..pos + bool_len] == bool_bytes {
                let before_ok = pos == 0
                    || (!bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_');
                let after_pos = pos + bool_len;
                let after_ok = after_pos >= len
                    || (!bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_');

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: offset + pos,
                        length: bool_len,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                }
                pos += bool_len;
            } else {
                pos += 1;
            }
        }
    }
}

/// Scan for namespace tokens (State, Engine, Story, etc.)
fn scan_namespace_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    for ns in SUGARCUBE_NAMESPACES {
        let ns_bytes = ns.as_bytes();
        let ns_len = ns_bytes.len();
        if ns_len == 0 { continue; }

        let mut pos = 0;
        while pos + ns_len <= len {
            if &bytes[pos..pos + ns_len] == ns_bytes {
                let before_ok = pos == 0
                    || (!bytes[pos - 1].is_ascii_alphanumeric() && bytes[pos - 1] != b'_');
                let after_pos = pos + ns_len;
                let after_ok = after_pos >= len
                    || (!bytes[after_pos].is_ascii_alphanumeric() && bytes[after_pos] != b'_');

                if before_ok && after_ok {
                    tokens.push(SemanticToken {
                        start: offset + pos,
                        length: ns_len,
                        token_type: SemanticTokenType::Namespace,
                        modifier: None,
                    });
                }
                pos += ns_len;
            } else {
                pos += 1;
            }
        }
    }
}

/// Scan for numeric literals
fn scan_number_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    for caps in RE_NUMBER.captures_iter(text) {
        if let Some(num_match) = caps.get(1) {
            let start = num_match.start();
            let end = num_match.end();
            let bytes = text.as_bytes();
            let before_ok = start == 0
                || (!bytes[start - 1].is_ascii_alphanumeric()
                    && bytes[start - 1] != b'_'
                    && bytes[start - 1] != b'$');
            let after_ok = end >= text.len()
                || (!bytes[end].is_ascii_alphanumeric()
                    && bytes[end] != b'_'
                    && bytes[end] != b'$');

            if before_ok && after_ok {
                tokens.push(SemanticToken {
                    start: offset + start,
                    length: end - start,
                    token_type: SemanticTokenType::Number,
                    modifier: None,
                });
            }
        }
    }
}

/// Scan for quoted string literals
fn scan_string_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    let quoted_args = blocks::parse_quoted_args(text);
    for (_content, rel_start, rel_end) in &quoted_args {
        tokens.push(SemanticToken {
            start: offset + *rel_start,
            length: rel_end - rel_start,
            token_type: SemanticTokenType::String,
            modifier: None,
        });
    }
}

/// Scan for compound assignment operators (+=, -=, *=, /=, %=)
fn scan_operator_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    for op in SUGARCUBE_OPERATORS {
        let op_bytes = op.as_bytes();
        let op_len = op_bytes.len();

        let mut pos = 0;
        while pos + op_len <= len {
            if &bytes[pos..pos + op_len] == op_bytes {
                tokens.push(SemanticToken {
                    start: offset + pos,
                    length: op_len,
                    token_type: SemanticTokenType::Operator,
                    modifier: None,
                });
                pos += op_len;
            } else {
                pos += 1;
            }
        }
    }
}

/// Scan for namespace.property tokens (State.variables, Engine.play, etc.)
fn scan_property_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    for caps in RE_PROPERTY.captures_iter(text) {
        if let Some(prop_match) = caps.get(1) {
            tokens.push(SemanticToken {
                start: offset + prop_match.start(),
                length: prop_match.end() - prop_match.start(),
                token_type: SemanticTokenType::Property,
                modifier: None,
            });
        }
    }
}

/// Scan for implicit passage references (Engine.play, data-passage, etc.)
fn scan_passage_ref_tokens(text: &str, offset: usize, tokens: &mut Vec<SemanticToken>) {
    use crate::sugarcube::links::{
        RE_DATA_PASSAGE, RE_ENGINE_PLAY, RE_ENGINE_GOTO,
        RE_STORY_GET, RE_STORY_PASSAGE, RE_STORY_HAS,
        RE_UI_GOTO, RE_UI_INCLUDE,
    };

    let patterns: &[&LazyLock<regex::Regex>] = &[
        &RE_DATA_PASSAGE,
        &RE_ENGINE_PLAY,
        &RE_ENGINE_GOTO,
        &RE_STORY_GET,
        &RE_STORY_PASSAGE,
        &RE_STORY_HAS,
        &RE_UI_GOTO,
        &RE_UI_INCLUDE,
    ];

    for re in patterns {
        for caps in re.captures_iter(text) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str().trim();
                if !name.is_empty() {
                    tokens.push(SemanticToken {
                        start: offset + name_match.start(),
                        length: name_match.end() - name_match.start(),
                        token_type: SemanticTokenType::PassageRef,
                        modifier: None,
                    });
                }
            }
        }
    }
}

/// Scan for widget/function definition tokens
fn scan_widget_tokens(
    macro_name: &str,
    args_str: &str,
    args_offset: usize,
    tokens: &mut Vec<SemanticToken>,
) {
    let widget_macro_names = macros::macro_definition_macros();
    if !widget_macro_names.contains(macro_name) {
        return;
    }

    if args_str.is_empty() {
        return;
    }

    let name_part = args_str.split_whitespace().next().unwrap_or("");
    let widget_name = name_part.trim_matches('"').trim_matches('\'');
    if widget_name.is_empty() {
        return;
    }

    if let Some(rel_pos) = args_str.find(widget_name) {
        tokens.push(SemanticToken {
            start: args_offset + rel_pos,
            length: widget_name.len(),
            token_type: SemanticTokenType::Function,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}

// ---------------------------------------------------------------------------
// Macro passage-ref tokens (<<goto "name">>, <<link "label" "name">>, etc.)
// ---------------------------------------------------------------------------

/// Walk the tree and produce PassageRef tokens for macro passage references.
///
/// This is separate from `walk_augment_tokens()` because macro passage refs
/// require the `body` text for arg offset computation and the macro catalog
/// for determining which arg position is the passage name.
pub(crate) fn walk_macro_passage_ref_tokens(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    walk_macro_passage_ref_tokens_inner(nodes, body, body_offset, &mut tokens);
    tokens
}

fn walk_macro_passage_ref_tokens_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    tokens: &mut Vec<SemanticToken>,
) {
    let passage_arg_macros = macros::passage_arg_macro_names();

    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            if parsed.name.starts_with('/') {
                // Recurse into children even for close tags
                if let PassageNode::Macro {
                    children: Some(children),
                    ..
                } = node
                {
                    walk_macro_passage_ref_tokens_inner(
                        children, body, body_offset, tokens,
                    );
                }
                continue;
            }

            let macro_name = parsed.name.as_str();

            if !passage_arg_macros.contains(macro_name) {
                // Not a passage-arg macro — just recurse
                if let PassageNode::Macro {
                    children: Some(children),
                    ..
                } = node
                {
                    walk_macro_passage_ref_tokens_inner(
                        children, body, body_offset, tokens,
                    );
                }
                continue;
            }

            let args_str = parsed.args.as_str();
            if args_str.is_empty() {
                continue;
            }

            let string_args = blocks::parse_quoted_args(args_str);
            if string_args.is_empty() {
                continue;
            }

            let arg_count = string_args.len();
            let passage_idx = macros::get_passage_arg_index(macro_name, arg_count);
            if passage_idx < 0 {
                continue;
            }

            let idx = passage_idx as usize;
            if idx < string_args.len() {
                let (content, rel_start, rel_end) = &string_args[idx];
                if !content.is_empty() {
                    let args_offset =
                        super::compute_args_offset(parsed, body, body_offset);
                    tokens.push(SemanticToken {
                        start: args_offset + *rel_start,
                        length: rel_end - rel_start,
                        token_type: SemanticTokenType::PassageRef,
                        modifier: None,
                    });
                }
            }

            // Recurse into children
            if let PassageNode::Macro {
                children: Some(children),
                ..
            } = node
            {
                walk_macro_passage_ref_tokens_inner(
                    children, body, body_offset, tokens,
                );
            }
        } else if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            walk_macro_passage_ref_tokens_inner(children, body, body_offset, tokens);
        }
    }
}
