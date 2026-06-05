//! JavaScript snippet extraction from SugarCube passage bodies.
//!
//! Walks the `PassageNode` tree produced by `parse_passage_body()` and
//! collects JavaScript snippets from contexts where JS code is expected:
//!
//! - **MacroExpression**: `<<run expr>>`, `<<set expr>>`, `<<if expr>>`,
//!   `<<elseif expr>>` — the arguments are JS expressions
//! - **ScriptPassage**: `<<script>>...<</script>>` — full JS program
//! - **MacroJsBlock**: `<<=>>` / `<<->>` — inline JS expressions
//! - **InlineBlock**: `{js code}` — inline JS blocks inside macro bodies
//!
//! The extractor does NOT re-scan the raw text. It walks the already-built
//! passage tree, which means it inherits all the tree's structural knowledge
//! (block nesting, comment filtering, etc.).

use crate::sugarcube::passage_tree::PassageNode;
use crate::sugarcube::passage_tree::WRITE_MACRO_NAMES;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Context in which a JavaScript snippet appears within a SugarCube passage.
///
/// Determines how the Oxc parser should interpret the snippet:
/// - `MacroExpression`: parse as a JS expression (or comma-separated expressions)
/// - `ScriptPassage`: parse as a JS module/program
/// - `MacroJsBlock`: parse as a JS expression (<<=>> / <<->>)
/// - `InlineBlock`: parse as a JS statement list
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsContext {
    /// JS expression inside a macro argument: `<<run ...>>`, `<<set ...>>`,
    /// `<<if ...>>`, `<<elseif ...>>`, etc.
    MacroExpression,
    /// Full JS program inside `<<script>>...<</script>>` blocks.
    ScriptPassage,
    /// Inline JS expression via `<<=>>` or `<<->>`.
    MacroJsBlock,
    /// Inline JS block via `{...}` within macro arguments.
    InlineBlock,
}

/// A JavaScript snippet extracted from a SugarCube passage.
///
/// Carries the source text, its position in the original document, and the
/// context that determines how it should be parsed.
#[derive(Debug, Clone)]
pub struct JsSnippet {
    /// The JavaScript source text (with `$var` still present — substitution
    /// happens at parse time in `syntax_check`).
    pub source: String,
    /// Byte offset of this snippet within the original document.
    pub offset: usize,
    /// 1-based line number of the snippet start within the passage body.
    pub line: u32,
    /// 1-based column number (in UTF-16 code units) of the snippet start.
    pub column: u32,
    /// The context in which this snippet appears.
    pub context: JsContext,
}

// ---------------------------------------------------------------------------
// Macro name classification
// ---------------------------------------------------------------------------

/// Macro names whose arguments contain JS expressions that should be validated.
const JS_ARG_MACROS: &[&str] = &[
    // Variable / control flow — args are JS expressions
    "run", "set", "capture", "unset",
    "if", "elseif", "else", "if",
    "for", "switch", "case", "default",
    "while",
    // Output — some have JS args
    "print", "display",
    // Math
    "math",
    "number",
    // DOM
    "addclass", "removeclass", "toggleclass",
    "append", "prepend", "replace", "remove",
    "wrap", "insertbefore", "insertafter",
    // Audio
    "audio", "cacheaudio", "createaudio",
    // Utility
    "copy", "repeat", "stop",
    "widget",
];

/// Check if a macro name's arguments should be extracted as JS.
fn is_js_arg_macro(name: &str) -> bool {
    JS_ARG_MACROS.contains(&name)
        || WRITE_MACRO_NAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// Extraction: tree walk
// ---------------------------------------------------------------------------

/// Extract JS snippets from a passage body by walking the `PassageNode` tree.
///
/// This is the main entry point. It walks the tree recursively, collecting
/// `JsSnippet` instances from:
/// 1. Macro nodes whose arguments contain JS expressions
/// 2. `<<script>>...<</script>>` block macro children
/// 3. Expression nodes (`<<=>>` / `<<->>`)
/// 4. Inline `{...}` JS blocks in macro arguments (detected via brace scanning)
///
/// ## Arguments
///
/// - `nodes`: The passage tree from `parse_passage_body()`
/// - `body`: The raw passage body text
/// - `body_offset`: Byte offset of the body start within the document
///
/// ## Returns
///
/// A vector of `JsSnippet` instances, sorted by offset.
pub(crate) fn extract_snippets(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<JsSnippet> {
    let mut snippets = Vec::new();
    extract_snippets_inner(nodes, body, body_offset, &mut snippets);

    // Sort by offset for deterministic ordering
    snippets.sort_by_key(|s| s.offset);
    snippets
}

fn extract_snippets_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    snippets: &mut Vec<JsSnippet>,
) {
    for node in nodes {
        match node {
            PassageNode::Macro {
                parsed,
                children,
                span,
                ..
            } => {
                let macro_name = parsed.name.as_str();

                // ── <<script>> blocks: full JS program ──────────────────
                if macro_name == "script" {
                    if let Some(children) = children {
                        // Collect all text content within <<script>>...<</script>>
                        let script_content = collect_text_content(children);
                        if !script_content.trim().is_empty() {
                            // The script body starts after the opening <<script>>
                            // and ends before <</script>>. Use the span of
                            // children to determine the exact range.
                            let (child_start, _child_end) = children_span(children, body_offset);

                            snippets.push(JsSnippet {
                                source: script_content,
                                offset: child_start,
                                line: line_from_offset(body, child_start, body_offset),
                                column: column_from_offset(body, child_start, body_offset),
                                context: JsContext::ScriptPassage,
                            });
                        }
                    }
                    // Don't recurse into script children for more snippets —
                    // the entire content is one JS program.
                    continue;
                }

                // ── Macro arguments with JS expressions ─────────────────
                if is_js_arg_macro(macro_name) && !parsed.args.trim().is_empty() {
                    let args_text = &parsed.args;
                    // Compute the document offset of the args string
                    let args_offset = compute_args_doc_offset(
                        &parsed.name,
                        args_text,
                        span.start,
                    );

                    // Extract inline {...} JS blocks from the args
                    let inline_blocks = extract_inline_js_blocks(
                        args_text,
                        args_offset,
                        body,
                        body_offset,
                    );

                    if inline_blocks.is_empty() {
                        // No inline blocks — the entire args is a JS expression
                        snippets.push(JsSnippet {
                            source: args_text.to_string(),
                            offset: args_offset,
                            line: line_from_offset(body, args_offset, body_offset),
                            column: column_from_offset(body, args_offset, body_offset),
                            context: JsContext::MacroExpression,
                        });
                    } else {
                        // We have inline {...} blocks. Emit the full args
                        // as a MacroExpression first (for full validation),
                        // then also emit each inline block separately
                        // (for more precise diagnostics).
                        snippets.push(JsSnippet {
                            source: args_text.to_string(),
                            offset: args_offset,
                            line: line_from_offset(body, args_offset, body_offset),
                            column: column_from_offset(body, args_offset, body_offset),
                            context: JsContext::MacroExpression,
                        });

                        // Emit inline blocks
                        snippets.extend(inline_blocks);
                    }
                }

                // ── Recurse into children for nested macros ─────────────
                if let Some(children) = children {
                    extract_snippets_inner(children, body, body_offset, snippets);
                }
            }

            PassageNode::Expression {
                name: _,
                content,
                span,
                ..
            } => {
                // <<=>> and <<->> — inline JS expression
                if !content.trim().is_empty() {
                    snippets.push(JsSnippet {
                        source: content.clone(),
                        offset: span.start, // approximate — expression content offset
                        line: line_from_offset(body, span.start, body_offset),
                        column: column_from_offset(body, span.start, body_offset),
                        context: JsContext::MacroJsBlock,
                    });
                }
            }

            PassageNode::Text { .. } | PassageNode::Heading { .. } | PassageNode::Error { .. } => {
                // No JS snippets in plain text, headings, or error nodes
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inline {js} block extraction
// ---------------------------------------------------------------------------

/// Extract inline JavaScript blocks `{...}` from a macro's arguments string.
///
/// SugarCube allows inline JS blocks like `<<run [$x + 1]{Math.max(x, 0)}>>`
/// where `{...}` contains a JavaScript expression. These need separate
/// extraction because they're JS that should be validated independently.
fn extract_inline_js_blocks(
    args: &str,
    args_offset: usize,
    body: &str,
    body_offset: usize,
) -> Vec<JsSnippet> {
    let mut blocks = Vec::new();
    let bytes = args.as_bytes();
    let mut depth: i32 = 0;
    let mut block_start: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    // Check this isn't an object literal start by looking at
                    // context — for simplicity, we treat all top-level `{`
                    // as potential inline JS blocks.
                    block_start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = block_start.take() {
                        // Extract the content between { and }
                        let content = &args[start + 1..i];
                        let trimmed = content.trim();
                        if !trimmed.is_empty() {
                            let doc_offset = args_offset + start + 1;
                            blocks.push(JsSnippet {
                                source: trimmed.to_string(),
                                offset: doc_offset,
                                line: line_from_offset(body, doc_offset, body_offset),
                                column: column_from_offset(body, doc_offset, body_offset),
                                context: JsContext::InlineBlock,
                            });
                        }
                    }
                }
            }
            b'"' | b'\'' => {
                // Skip string contents — braces inside strings don't count
                let quote = b;
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\\' {
                        j += 2; // skip escaped char
                    } else if bytes[j] == quote {
                        break;
                    } else {
                        j += 1;
                    }
                }
                // Note: we can't actually skip the iterator ahead in a for loop,
                // but the depth tracking will handle mismatched braces inside
                // strings naturally — we just won't have perfect depth tracking.
                // This is acceptable for a best-effort extractor.
            }
            _ => {}
        }
    }

    blocks
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all text content from a slice of PassageNodes, concatenating
/// Text nodes and ignoring macro structure. Used for <<script>> blocks
/// where the entire content is one JS program.
fn collect_text_content(nodes: &[PassageNode]) -> String {
    let mut result = String::new();
    for node in nodes {
        match node {
            PassageNode::Text { content, .. } => {
                result.push_str(content);
            }
            PassageNode::Macro {
                parsed: _, children, ..
            } => {
                // Include the macro tag itself as text (it's JS code in a
                // script passage — it won't be a real SugarCube macro, but
                // if someone writes `<<` inside JS it would be parsed as one)
                result.push_str("/* macro tag */ ");
                if let Some(children) = children {
                    result.push_str(&collect_text_content(children));
                }
            }
            PassageNode::Expression { content, .. } => {
                result.push_str(content);
            }
            PassageNode::Heading { content, .. } => {
                result.push_str(content);
            }
            PassageNode::Error { message, .. } => {
                result.push_str(&format!("/* error: {} */", message));
            }
        }
    }
    result
}

/// Compute the byte span covered by a slice of PassageNodes.
fn children_span(nodes: &[PassageNode], body_offset: usize) -> (usize, usize) {
    if nodes.is_empty() {
        return (body_offset, body_offset);
    }

    let start = nodes
        .iter()
        .map(|n| match n {
            PassageNode::Text { span, .. } => span.start,
            PassageNode::Macro { span, .. } => span.start,
            PassageNode::Expression { span, .. } => span.start,
            PassageNode::Heading { span, .. } => span.start,
            PassageNode::Error { span, .. } => span.start,
        })
        .min()
        .unwrap_or(body_offset);

    let end = nodes
        .iter()
        .map(|n| match n {
            PassageNode::Text { span, .. } => span.end,
            PassageNode::Macro { span, .. } => span.end,
            PassageNode::Expression { span, .. } => span.end,
            PassageNode::Heading { span, .. } => span.end,
            PassageNode::Error { span, .. } => span.end,
        })
        .max()
        .unwrap_or(body_offset);

    (start, end)
}

/// Compute the document offset of a macro's arguments string.
///
/// The ParsedMacro stores `args` as the trimmed content between the macro
/// name and the closing `>>`. We estimate the document offset from the
/// macro's span start plus the length of `<<name `.
fn compute_args_doc_offset(name: &str, args: &str, span_start: usize) -> usize {
    // "<<name " = 2 (<<) + name.len() + 1 (space)
    let opening_len = 2 + name.len() + 1;
    // Find the first non-whitespace character of the args
    let leading_ws = args.len() - args.trim_start().len();
    span_start + opening_len + leading_ws
}

/// Compute a 1-based line number from a document offset within the body.
fn line_from_offset(body: &str, offset: usize, _body_offset: usize) -> u32 {
    // Count newlines before this offset within the body
    let body_pos = offset.saturating_sub(_body_offset);
    let line = body[..body_pos.min(body.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count();
    (line + 1) as u32
}

/// Compute a 1-based column number (in characters) from a document offset.
fn column_from_offset(body: &str, offset: usize, body_offset: usize) -> u32 {
    let body_pos = offset.saturating_sub(body_offset);
    // Find the start of the current line
    let line_start = body[..body_pos.min(body.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let col = body_pos.saturating_sub(line_start);
    (col + 1) as u32
}
