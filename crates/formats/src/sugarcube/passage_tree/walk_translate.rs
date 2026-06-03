//! JS translation walks for the passage tree.
//!
//! Contains `walk_translate()`, the inner recursive walk, and helpers for
//! exact line mapping.
//!
//! ## Phase A: Selective translation
//!
//! Only **stateful macros** (ones that read/write `State.variables`) are
//! translated to JS. Navigation-only, DOM, audio, and timing macros are
//! skipped. Nav-shell macros (like `<<link>>`) skip their shell but still
//! recurse into children so that stateful children (e.g., `<<set>>`) are
//! translated.
//!
//! ## Comment awareness
//!
//! Macro nodes whose spans overlap with comment spans (`/* ... */`,
//! `/% ... %/`, `<!-- ... -->`) are completely skipped during translation.
//! This prevents macros inside block comments from producing JS output
//! like `if () { } else { }` in the virtual document.
//!
//! The output is wrapped in a function: `function passage_Name() { ... }`
//! for normal passages, or `function myWidget() { ... }` for widget
//! passages.

use super::PassageNode;
use crate::types::MacroCategory;

// ---------------------------------------------------------------------------
// ExactLineMapping
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

// ---------------------------------------------------------------------------
// New types: VarEncounter, VarTypeHint, VarAccessKind, TranslateResult
// ---------------------------------------------------------------------------

/// Type hint inferred from how a variable is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VarTypeHint {
    Number,
    String,
    Boolean,
    Array,
    Object,
    Unknown,
}

/// Whether a variable encounter is a read or write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VarAccessKind {
    Read,
    Write,
}

/// A variable encounter recorded during translation.
///
/// When we emit `State.variables.xxx = yyy;` or `if (State.variables.xxx)`,
/// we record this encounter so that downstream consumers can build a variable
/// registry, infer types, and track read/write locations.
#[derive(Debug, Clone)]
pub(crate) struct VarEncounter {
    /// The variable name without the `$` sigil (e.g., "gold" for `$gold`).
    pub name: String,
    /// Inferred type hint (Number for numeric literals, String for quoted, etc.)
    pub type_hint: VarTypeHint,
    /// Whether this is a read or write.
    pub kind: VarAccessKind,
    /// Source line within the passage body (0-based).
    pub line: u32,
    /// Byte span in the source document.
    #[allow(dead_code)] // Used by Phase C for diagnostic routing
    pub byte_span: std::ops::Range<usize>,
}

/// Result of translating a passage tree to JS.
pub(crate) struct TranslateResult {
    /// The complete JS function (includes the `function passage_Name() {` wrapper).
    pub js_function: String,
    /// Line mappings for each line in `js_function`.
    pub line_map: Vec<ExactLineMapping>,
    /// Variable encounters collected during translation.
    pub var_encounters: Vec<VarEncounter>,
}

// ---------------------------------------------------------------------------
// Macro classification: is_stateful_macro()
// ---------------------------------------------------------------------------

/// Classification of a macro for translation selectivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacroSelectivity {
    /// Stateful macro: translate to JS (reads/writes State.variables).
    Stateful,
    /// Navigation shell: skip the shell, but recurse into children
    /// because they may contain stateful macros.
    NavShell,
    /// Completely skip: no JS output, no recursion into children.
    Skip,
}

/// Classify a macro by its category for translation selectivity.
///
/// Uses the `MacroDef` catalog's `MacroCategory` to determine whether
/// a macro should be translated, skipped, or treated as a nav-shell.
fn classify_macro(name: &str, builtin_lookup: &std::collections::HashMap<&'static str, &'static crate::types::MacroDef>) -> MacroSelectivity {
    // Special cases for expression macros (<<=>> and <<->>)
    if name == "=" || name == "-" {
        return MacroSelectivity::Stateful;
    }

    // Handle "when" pseudo-macro
    if name == "when" {
        return MacroSelectivity::Stateful;
    }

    // Look up the macro in the builtin catalog
    if let Some(mdef) = builtin_lookup.get(name) {
        return match mdef.category {
            // Stateful: translate to JS
            MacroCategory::Control => MacroSelectivity::Stateful,
            MacroCategory::Variables => MacroSelectivity::Stateful,
            MacroCategory::Output => {
                // "type" macro is NOT stateful — it's a typewriter effect
                if name == "type" {
                    MacroSelectivity::Skip
                } else {
                    MacroSelectivity::Stateful
                }
            }
            MacroCategory::Forms => MacroSelectivity::Stateful,
            MacroCategory::Widgets => MacroSelectivity::Stateful,

            // Nav shell: skip the shell, recurse into children
            MacroCategory::Links => MacroSelectivity::NavShell,
            MacroCategory::Navigation => {
                // Navigation macros are inline (no children), skip entirely
                MacroSelectivity::Skip
            }

            // Completely skip
            MacroCategory::Dom => {
                // "script" is in Dom category but is stateful
                if name == "script" {
                    MacroSelectivity::Stateful
                } else {
                    MacroSelectivity::Skip
                }
            }
            MacroCategory::Timing => {
                // timed/repeat are nav-shell-like: skip the shell but
                // children may be stateful
                if name == "stop" {
                    MacroSelectivity::Skip
                } else {
                    MacroSelectivity::NavShell
                }
            }
            MacroCategory::Audio => MacroSelectivity::Skip,
        };
    }

    // User-defined callables (widget invocations) are stateful — they are
    // function calls that may touch State.variables
    // (We can't check callable_names here without passing it in, so
    //  unknown macros default to Stateful for safety — they might be
    //  widget invocations or custom macros.)
    MacroSelectivity::Stateful
}

// ---------------------------------------------------------------------------
// JS identifier sanitization
// ---------------------------------------------------------------------------

/// Sanitize a passage title into a valid JavaScript identifier.
///
/// Passage titles can contain characters that are not valid in JS identifiers:
/// spaces, `::`, hyphens, dots, apostrophes, Unicode, etc. This function
/// replaces all non-alphanumeric characters with underscores and ensures
/// the result starts with a valid JS identifier start character (letter,
/// underscore, or dollar sign).
///
/// Examples:
/// - `"Start::Intro"` → `"passage_Start__Intro"`
/// - `"my-passage"` → `"passage_my_passage"`
/// - `"Mary's Room"` → `"passage_Mary_s_Room"`
/// - `"42 Begin"` → `"passage__42_Begin"` (can't start with digit)
pub(crate) fn sanitize_js_identifier(name: &str) -> String {
    let sanitized: String = name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Ensure the identifier starts with a valid JS identifier start character
    if sanitized.is_empty() {
        return "_".to_string();
    }

    let first = sanitized.chars().next().unwrap();
    if first.is_ascii_alphabetic() || first == '_' || first == '$' {
        sanitized
    } else {
        // Starts with a digit or other invalid char — prepend underscore
        format!("_{}", sanitized)
    }
}

// ---------------------------------------------------------------------------
// Temp var collection
// ---------------------------------------------------------------------------

/// Collect unique temporary variable names (`_var`) from the tree.
///
/// Scans all nodes recursively, looking for VarRef entries with
/// `is_temporary == true`. Returns deduplicated names (without the `_`
/// prefix, matching JS `let` declaration convention).
fn collect_temp_vars(nodes: &[PassageNode], comment_spans: &[std::ops::Range<usize>]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    collect_temp_vars_inner(nodes, &mut seen, comment_spans);
    seen.into_iter().collect()
}

fn collect_temp_vars_inner(nodes: &[PassageNode], seen: &mut std::collections::BTreeSet<String>, comment_spans: &[std::ops::Range<usize>]) {
    for node in nodes {
        // Skip nodes inside comments
        let span = match node {
            PassageNode::Text { span, .. } => span,
            PassageNode::Macro { span, .. } => span,
            PassageNode::Expression { span, .. } => span,
            PassageNode::Heading { span, .. } => span,
            PassageNode::Error { span, .. } => span,
        };
        if is_in_comment(comment_spans, span) {
            continue;
        }

        match node {
            PassageNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    if vr.is_temporary {
                        let name = vr.name.trim_start_matches('_');
                        if !name.is_empty() {
                            seen.insert(name.to_string());
                        }
                    }
                }
            }
            PassageNode::Macro { var_refs, children, .. } => {
                for vr in var_refs {
                    if vr.is_temporary {
                        let name = vr.name.trim_start_matches('_');
                        if !name.is_empty() {
                            seen.insert(name.to_string());
                        }
                    }
                }
                if let Some(children) = children {
                    collect_temp_vars_inner(children, seen, comment_spans);
                }
            }
            PassageNode::Expression { var_refs, .. } => {
                for vr in var_refs {
                    if vr.is_temporary {
                        let name = vr.name.trim_start_matches('_');
                        if !name.is_empty() {
                            seen.insert(name.to_string());
                        }
                    }
                }
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Comment span overlap check
// ---------------------------------------------------------------------------

/// Check whether a node's span overlaps with any comment span.
///
/// Uses body-relative spans for both. The `comment_spans` are body-relative
/// (as returned by `find_all_comment_spans`), and `node_span` is
/// document-absolute (as stored in PassageNode). We convert `node_span`
/// to body-relative by subtracting `body_offset`.
fn is_in_comment(comment_spans: &[std::ops::Range<usize>], node_span: &std::ops::Range<usize>) -> bool {
    // Quick exit: if no comment spans, nothing is in a comment
    if comment_spans.is_empty() {
        return false;
    }
    comment_spans.iter().any(|cs| {
        node_span.start < cs.end && cs.start < node_span.end
    })
}

// ---------------------------------------------------------------------------
// walk_translate()
// ---------------------------------------------------------------------------

/// Walk the tree and produce translated JS with exact line mapping.
///
/// ## Phase A: Selective translation
///
/// Only stateful macros (ones that read/write `State.variables`) are
/// translated. Navigation-only, DOM, audio, and timing macros are skipped.
/// Nav-shell macros (like `<<link>>`) skip their shell but recurse into
/// children so that stateful children are translated.
///
/// ## Comment awareness
///
/// Macro nodes whose spans overlap with comment spans are completely
/// skipped, preventing macros inside `/* ... */` block comments from
/// producing JS output in the virtual document.
///
/// ## Function wrapper
///
/// The output is wrapped in a function:
/// - Normal passages: `function passage_Name() { ... }`
/// - Widget passages: `function myWidget() { ... }` (no `passage_` prefix)
///
/// Temp vars are declared at the top: `let _varname;`
///
/// ## Parameters
///
/// - `nodes`: The passage tree from `parse_passage_body()`
/// - `body`: The original passage body text (needed for line number computation)
/// - `body_offset`: The byte offset of the body within the source document
/// - `callables`: User-defined callables (custom macros + widgets)
/// - `passage_name`: The name of the passage being translated
/// - `is_widget`: Whether this passage is a widget passage (tagged [widget])
/// - `comment_spans`: Body-relative byte ranges of comments to skip
pub(crate) fn walk_translate(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    callables: &[crate::types::UserCallable],
    passage_name: &str,
    is_widget: bool,
    comment_spans: &[std::ops::Range<usize>],
) -> TranslateResult {
    let ctx = super::super::virtual_doc::TranslationContext::new(callables);
    let mut js_body = String::new();
    let mut line_mappings = Vec::new();
    let mut var_encounters = Vec::new();

    // Collect temp vars and emit `let _varname;` declarations
    // (skip temp vars inside comments)
    let temp_vars = collect_temp_vars(nodes, comment_spans);

    // Build function header — sanitize the passage name to a valid JS identifier
    let func_name = if is_widget {
        // Widget passages: use the first widget name from callables defined
        // in this passage, or fall back to the passage name (sanitized)
        callables
            .iter()
            .find(|c| c.kind == crate::types::UserCallableKind::Widget && c.defined_in == passage_name)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| sanitize_js_identifier(passage_name))
    } else {
        format!("passage_{}", sanitize_js_identifier(passage_name))
    };

    let mut js_output = format!("function {}() {{\n", func_name);
    // Line 0 maps to... nothing specific, it's the function header.
    // We map it to line 0 of the passage body as a sentinel.
    line_mappings.push(ExactLineMapping {
        original_line: 0,
        original_start_byte: body_offset,
    });

    // Emit temp var declarations
    let indent_str = "  ".repeat(1);
    for var_name in &temp_vars {
        js_output.push_str(&format!("{}let _{};\n", indent_str, var_name));
        line_mappings.push(ExactLineMapping {
            original_line: 0,
            original_start_byte: body_offset,
        });
    }

    // Translate the body (with indent=1 inside the function)
    walk_translate_inner(
        nodes, body, body_offset, &ctx, 1,
        &mut js_body, &mut line_mappings, &mut var_encounters,
        comment_spans,
    );

    js_output.push_str(&js_body);

    // Close the function
    js_output.push_str("}\n");
    line_mappings.push(ExactLineMapping {
        original_line: 0,
        original_start_byte: body_offset,
    });

    // INVARIANT: js_output line count MUST equal line_mappings length.
    // Every line in js_output (function header, temp var declarations,
    // translated body, closing brace) must have exactly one entry in
    // line_mappings. The downstream consumer (assemble_line_map /
    // assemble_annotated_line_map) indexes line_mappings by virtual doc
    // line number, so any mismatch causes incorrect error mapping.
    let line_count = js_output.lines().count();
    debug_assert_eq!(
        line_count, line_mappings.len(),
        "walk_translate: js_function has {} lines but line_map has {} entries — they must match 1:1",
        line_count, line_mappings.len(),
    );

    TranslateResult {
        js_function: js_output,
        line_map: line_mappings,
        var_encounters,
    }
}

/// Inner recursive walk for `walk_translate()`.
///
/// Emits translated JS and records exact line mappings for each output line.
/// Only stateful macros are translated; nav-shell macros recurse into children
/// without emitting their shell; completely-skipped macros produce no output.
/// Macro nodes inside comment spans are completely skipped.
fn walk_translate_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    ctx: &super::super::virtual_doc::TranslationContext,
    indent: usize,
    js_output: &mut String,
    line_mappings: &mut Vec<ExactLineMapping>,
    var_encounters: &mut Vec<VarEncounter>,
    comment_spans: &[std::ops::Range<usize>],
) {
    for node in nodes {
        // ── Comment check: skip any node whose span overlaps a comment ──
        // This prevents macros inside /* ... */ from being translated.
        // We check the span before dispatching on node type.
        let node_span = match node {
            PassageNode::Text { span, .. } => span,
            PassageNode::Macro { span, .. } => span,
            PassageNode::Expression { span, .. } => span,
            PassageNode::Heading { span, .. } => span,
            PassageNode::Error { span, .. } => span,
        };
        if is_in_comment(comment_spans, node_span) {
            continue;
        }

        match node {
            PassageNode::Text { content, var_refs, span, .. } => {
                // Text nodes inside block macros (if/else/for/etc.) may
                // contain variable references that affect state. We emit
                // these as "read" expressions so the virtual doc tracks
                // variable usage inside control flow bodies.
                //
                // At the top level (indent=1), text between macros is just
                // rendered HTML and we skip it (Phase A selective translation).
                // But inside block macros, the text represents conditional or
                // loop body content that may reference variables.
                let has_var_refs = !var_refs.is_empty();
                if has_var_refs && indent > 1 {
                    // Emit variable reads from text content
                    let indent_str = "  ".repeat(indent);
                    let source_line = line_from_span(span.start, body, body_offset);

                    // Collect VarEncounter entries for text node var refs
                    for vr in var_refs {
                        if vr.is_temporary {
                            continue;
                        }
                        let name = vr.name.trim_start_matches('$');
                        if name.is_empty() {
                            continue;
                        }
                        // Text var refs are always reads
                        var_encounters.push(VarEncounter {
                            name: name.to_string(),
                            type_hint: VarTypeHint::Unknown,
                            kind: VarAccessKind::Read,
                            line: source_line,
                            byte_span: vr.span.clone(),
                        });
                    }

                    // Translate $var references in the text to State.variables.var
                    let translated = super::super::virtual_doc::translate_dollar_refs_in_js(content);
                    // Only emit if the translation produced actual variable references
                    if translated.contains("State.variables.") {
                        let text_js = format!("{}/* read: {} */;\n", indent_str, translated.trim());
                        append_with_mapping(&text_js, source_line, span.start, js_output, line_mappings);
                    }
                } else {
                    let _ = (content, span);
                }
            }

            PassageNode::Macro {
                parsed,
                var_refs,
                children,
                close_span,
                span,
            } => {
                let macro_name = parsed.name.as_str();
                let args = parsed.args.as_str();
                let source_line = line_from_span(span.start, body, body_offset);

                let selectivity = classify_macro(macro_name, &ctx.builtin_lookup);

                match selectivity {
                    MacroSelectivity::Stateful => {
                        // Collect VarEncounter entries from this node's var_refs
                        collect_var_encounters(var_refs, source_line, body, body_offset, var_encounters);

                        // Check if this is a block macro (has children)
                        let is_block = children.is_some();

                        if macro_name == "script" && is_block {
                            // <<script>> blocks: raw JS with $var refs translated
                            let js_body = super::super::virtual_doc::translate_dollar_refs_in_js(
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
                            let open_js = super::super::virtual_doc::translate_block_open(ctx, macro_name, args, indent);
                            append_with_mapping(&open_js, source_line, span.start, js_output, line_mappings);

                            // Recursively translate children with indent+1
                            if let Some(children) = children {
                                walk_translate_inner(
                                    children, body, body_offset, ctx, indent + 1,
                                    js_output, line_mappings, var_encounters,
                                    comment_spans,
                                );
                            }

                            // Close tag
                            if let Some(close_span) = close_span {
                                let close_line = line_from_span(close_span.start, body, body_offset);
                                let close_js = super::super::virtual_doc::translate_close_tag(macro_name, indent);
                                append_with_mapping(&close_js, close_line, close_span.start, js_output, line_mappings);
                            }
                        } else if super::super::virtual_doc::is_block_macro(ctx, macro_name) {
                            // Block macro that has no children (unclosed) — emit open only
                            let open_js = super::super::virtual_doc::translate_block_open(ctx, macro_name, args, indent);
                            append_with_mapping(&open_js, source_line, span.start, js_output, line_mappings);
                        } else if ctx.builtin_lookup.contains_key(macro_name) || macro_name == "when" {
                            // Inline builtin macro
                            let inline_js = super::super::virtual_doc::translate_inline_macro(ctx, macro_name, args, indent);
                            append_with_mapping(&inline_js, source_line, span.start, js_output, line_mappings);
                        } else if ctx.callable_names.contains(macro_name) {
                            // User-defined callable (widget invocation / custom macro)
                            let indent_str = "  ".repeat(indent);
                            let translated_args = if args.is_empty() {
                                String::new()
                            } else {
                                super::super::virtual_doc::translate_callable_args(args)
                            };
                            let callable_js = if translated_args.is_empty() {
                                format!("{}{}();\n", indent_str, macro_name)
                            } else {
                                format!("{}{}({});\n", indent_str, macro_name, translated_args)
                            };
                            append_with_mapping(&callable_js, source_line, span.start, js_output, line_mappings);
                        } else {
                            // Unknown macro — treat as stateful (it might be a
                            // user callable we haven't detected, or a macro that
                            // affects state in ways we can't determine)
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

                    MacroSelectivity::NavShell => {
                        // Nav-shell macro: skip the shell (no open/close tag JS),
                        // but recurse into children to find stateful macros.
                        if let Some(children) = children {
                            walk_translate_inner(
                                children, body, body_offset, ctx, indent,
                                js_output, line_mappings, var_encounters,
                                comment_spans,
                            );
                        }
                        // No close tag emitted for skipped shells
                    }

                    MacroSelectivity::Skip => {
                        // Completely skip: no JS output, no recursion.
                    }
                }
            }

            PassageNode::Expression { content, span, .. } => {
                // Expression macro: <<=>> or <<->>
                // These are always stateful (they read vars for output)
                let source_line = line_from_span(span.start, body, body_offset);
                let indent_str = "  ".repeat(indent);
                let translated_expr = super::super::virtual_doc::translate_expression(content);
                let expr_js = format!("{}/* print: {} */;\n", indent_str, translated_expr);
                append_with_mapping(&expr_js, source_line, span.start, js_output, line_mappings);
            }

            PassageNode::Heading { span, .. } => {
                // Headings don't produce JS output
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

// ---------------------------------------------------------------------------
// VarEncounter collection
// ---------------------------------------------------------------------------

/// Collect VarEncounter entries from a macro node's var_refs.
///
/// Only collects story variables (`$var`, not `_var` temp vars).
/// Type hints are inferred from context:
/// - Write macros (`set`, `capture`, `unset`): try to infer from RHS literal
/// - Read macros: Unknown (reads don't tell us type)
fn collect_var_encounters(
    var_refs: &[super::VarRef],
    source_line: u32,
    body: &str,
    body_offset: usize,
    encounters: &mut Vec<VarEncounter>,
) {
    for vr in var_refs {
        // Skip temporary variables
        if vr.is_temporary {
            continue;
        }

        // Strip the $ sigil to get the base name
        let name = vr.name.trim_start_matches('$');
        if name.is_empty() {
            continue;
        }

        let kind = if vr.is_write {
            VarAccessKind::Write
        } else {
            VarAccessKind::Read
        };

        // Type hint: for writes, try to infer from context; for reads, Unknown
        let type_hint = if vr.is_write {
            infer_type_hint_from_span(vr.span.clone(), body, body_offset)
        } else {
            VarTypeHint::Unknown
        };

        encounters.push(VarEncounter {
            name: name.to_string(),
            type_hint,
            kind,
            line: source_line,
            byte_span: vr.span.clone(),
        });
    }
}

/// Try to infer a type hint from the source text around a variable write span.
///
/// Looks at the text after the variable reference for assignment patterns
/// like `= 10` (Number), `= "hello"` (String), `= true` (Boolean),
/// `= []` (Array), `= {}` (Object).
fn infer_type_hint_from_span(
    var_span: std::ops::Range<usize>,
    body: &str,
    body_offset: usize,
) -> VarTypeHint {
    // Convert doc-absolute span to body-relative offset
    let _start = var_span.start.saturating_sub(body_offset);
    let end = var_span.end.saturating_sub(body_offset);

    if end > body.len() {
        return VarTypeHint::Unknown;
    }

    // Look at the text after the variable reference for an assignment
    let after = body[end..].trim_start();

    // Check for assignment operator
    if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("===") {
        let rhs = after[1..].trim_start();
        return infer_type_from_rhs(rhs);
    }

    // Check for `to` (SugarCube assignment syntax)
    if after.starts_with("to ") || after.starts_with("to\t") {
        let rhs = after[3..].trim_start();
        return infer_type_from_rhs(rhs);
    }

    VarTypeHint::Unknown
}

/// Infer type from the right-hand side of an assignment.
fn infer_type_from_rhs(rhs: &str) -> VarTypeHint {
    if rhs.is_empty() {
        return VarTypeHint::Unknown;
    }

    // Check for numeric literal
    let first = rhs.chars().next().unwrap();
    if first.is_ascii_digit() || first == '-' || first == '+' || first == '.' {
        // Try to parse as number
        let num_str: String = rhs.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E').collect();
        if num_str.parse::<f64>().is_ok() {
            return VarTypeHint::Number;
        }
    }

    // Check for string literal
    if first == '"' || first == '\'' {
        return VarTypeHint::String;
    }

    // Check for boolean
    if rhs.starts_with("true") || rhs.starts_with("false") {
        return VarTypeHint::Boolean;
    }

    // Check for array literal
    if first == '[' {
        return VarTypeHint::Array;
    }

    // Check for object literal
    if first == '{' {
        return VarTypeHint::Object;
    }

    VarTypeHint::Unknown
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_js_identifier_simple() {
        assert_eq!(sanitize_js_identifier("Start"), "Start");
    }

    #[test]
    fn test_sanitize_js_identifier_with_spaces() {
        assert_eq!(sanitize_js_identifier("My Passage"), "My_Passage");
    }

    #[test]
    fn test_sanitize_js_identifier_with_double_colon() {
        assert_eq!(sanitize_js_identifier("Start::Intro"), "Start__Intro");
    }

    #[test]
    fn test_sanitize_js_identifier_with_hyphen() {
        assert_eq!(sanitize_js_identifier("my-passage"), "my_passage");
    }

    #[test]
    fn test_sanitize_js_identifier_with_apostrophe() {
        assert_eq!(sanitize_js_identifier("Mary's Room"), "Mary_s_Room");
    }

    #[test]
    fn test_sanitize_js_identifier_starts_with_digit() {
        assert_eq!(sanitize_js_identifier("42 Begin"), "_42_Begin");
    }

    #[test]
    fn test_sanitize_js_identifier_empty() {
        assert_eq!(sanitize_js_identifier(""), "_");
    }

    #[test]
    fn test_sanitize_js_identifier_already_valid() {
        assert_eq!(sanitize_js_identifier("myWidget"), "myWidget");
    }

    #[test]
    fn test_comment_span_check_skips_macro() {
        // A macro span [10, 30) that overlaps with comment span [5, 35)
        let comment_spans = vec![5..35];
        let node_span = 10..30;
        assert!(is_in_comment(&comment_spans, &node_span));
    }

    #[test]
    fn test_comment_span_check_allows_non_comment() {
        // A macro span [40, 50) that does NOT overlap with comment span [5, 35)
        let comment_spans = vec![5..35];
        let node_span = 40..50;
        assert!(!is_in_comment(&comment_spans, &node_span));
    }

    #[test]
    fn test_comment_span_check_empty() {
        let comment_spans: Vec<std::ops::Range<usize>> = vec![];
        let node_span = 10..30;
        assert!(!is_in_comment(&comment_spans, &node_span));
    }

    /// Integration test covering four original bugs in the SugarCube virtual doc system.
    ///
    /// Bug 1: Macros inside `/* ... */` comments must NOT produce JS output.
    /// Bug 2: Passage titles with special chars must produce valid JS function names.
    /// Bug 3: `<<if>>` / `<<else>>` bodies must contain translated child content.
    /// Bug 4: `build_script_passage_js()` must correctly transform `this.args`,
    ///        `this.name`, and `this.error()` in standalone Macro.add functions.
    #[test]
    fn test_four_original_bugs() {
        use crate::sugarcube::comments;
        use crate::sugarcube::custom_macros;
        use crate::sugarcube::passage_tree::parse_passage_body;
        use crate::types::{UserCallable, UserCallableKind};

        // ═══════════════════════════════════════════════════════════════════
        // Bug 1: Comment stripping
        // ═══════════════════════════════════════════════════════════════════
        // A macro inside `/* ... */` should NOT produce any JS output.
        // The `<<if $y>>` and `<<set $z to 2>>` inside the comment must be
        // skipped entirely; only `<<set $x to 1>>` and `<<set $w to 3>>`
        // should appear in the output.
        {
            let body = r#"<<set $x to 1>> /* <<if $y>> <<set $z to 2>> <</if>> */ <<set $w to 3>>"#;
            let body_offset = 0;
            let nodes = parse_passage_body(body, body_offset);
            let comment_spans = comments::find_all_comment_spans(body, false);

            let result = walk_translate(
                &nodes, body, body_offset, &[], "TestBug1", false, &comment_spans,
            );

            let js = &result.js_function;

            // Macros inside the comment should NOT appear
            assert!(
                !js.contains("State.variables.y"),
                "Bug 1: $y from inside comment should not appear in JS, got:\n{}",
                js
            );
            assert!(
                !js.contains("State.variables.z"),
                "Bug 1: $z from inside comment should not appear in JS, got:\n{}",
                js
            );

            // Macros outside the comment should appear
            assert!(
                js.contains("State.variables.x = 1"),
                "Bug 1: $x set should appear in JS, got:\n{}",
                js
            );
            assert!(
                js.contains("State.variables.w = 3"),
                "Bug 1: $w set should appear in JS, got:\n{}",
                js
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // Bug 2: Function name normalization
        // ═══════════════════════════════════════════════════════════════════
        // Passage titles with special chars (spaces, `::`, hyphens)
        // should produce valid JS function names.
        {
            let body = "<<set $x to 1>>";
            let body_offset = 0;
            let nodes = parse_passage_body(body, body_offset);

            // "Start::Intro" → function passage_Start__Intro()
            let result = walk_translate(
                &nodes, body, body_offset, &[], "Start::Intro", false, &[],
            );
            assert!(
                result.js_function.starts_with("function passage_Start__Intro()"),
                "Bug 2: 'Start::Intro' should produce function passage_Start__Intro(), got:\n{}",
                result.js_function
            );

            // "my-passage" → function passage_my_passage()
            let result = walk_translate(
                &nodes, body, body_offset, &[], "my-passage", false, &[],
            );
            assert!(
                result.js_function.starts_with("function passage_my_passage()"),
                "Bug 2: 'my-passage' should produce function passage_my_passage(), got:\n{}",
                result.js_function
            );

            // "Mary's Room" → function passage_Mary_s_Room()
            let result = walk_translate(
                &nodes, body, body_offset, &[], "Mary's Room", false, &[],
            );
            assert!(
                result.js_function.starts_with("function passage_Mary_s_Room()"),
                "Bug 2: 'Mary\'s Room' should produce function passage_Mary_s_Room(), got:\n{}",
                result.js_function
            );

            // The function name must be a valid JS identifier (no spaces, colons, hyphens)
            let func_name_line = result.js_function.lines().next().unwrap();
            assert!(
                !func_name_line.contains("::"),
                "Bug 2: Function name should not contain '::', got: {}",
                func_name_line
            );
            assert!(
                !func_name_line.contains('-'),
                "Bug 2: Function name should not contain '-', got: {}",
                func_name_line
            );
            assert!(
                !func_name_line.contains("'"),
                "Bug 2: Function name should not contain apostrophe, got: {}",
                func_name_line
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // Bug 3: if/else body content
        // ═══════════════════════════════════════════════════════════════════
        // `<<if $x>><<set $y to 1>><<else>><<set $y to 2>><</if>>` should
        // produce JS with both if and else bodies containing the set macros,
        // NOT empty bodies like `if (State.variables.x) { } else { }`.
        {
            let body = "<<if $x>><<set $y to 1>><<else>><<set $y to 2>><</if>>";
            let body_offset = 0;
            let nodes = parse_passage_body(body, body_offset);

            let result = walk_translate(
                &nodes, body, body_offset, &[], "Bug3", false, &[],
            );

            let js = &result.js_function;

            // The if-body should contain the first set
            assert!(
                js.contains("State.variables.y = 1"),
                "Bug 3: if-body should contain 'State.variables.y = 1', got:\n{}",
                js
            );

            // The else-body should contain the second set
            assert!(
                js.contains("State.variables.y = 2"),
                "Bug 3: else-body should contain 'State.variables.y = 2', got:\n{}",
                js
            );

            // Verify the structure: if block with else
            assert!(
                js.contains("if (State.variables.x)"),
                "Bug 3: should contain 'if (State.variables.x)', got:\n{}",
                js
            );
            assert!(
                js.contains("} else {"),
                "Bug 3: should contain '}} else {{', got:\n{}",
                js
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // Bug 4: Macro.add handler extraction
        // ═══════════════════════════════════════════════════════════════════
        // `build_script_passage_js()` should emit standalone functions with:
        // - `this.args` → `args`
        // - `this.name` → string literal replacement
        // - `return this.error()` → `throw new Error()` (not `return throw new Error()`)
        {
            let body = r#"Macro.add('myMacro', {
    handler: function() {
        var hours = this.args[0];
        var macroName = this.name;
        if (!hours) { return this.error("missing args"); }
        State.variables.time += hours;
    }
});"#;

            let custom_macros = vec![
                UserCallable {
                    name: "myMacro".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(1),
                    defined_in: "Bug4Script".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 0,
                    body: Some(
                        "var hours = this.args[0];\nvar macroName = this.name;\nif (!hours) { return this.error(\"missing args\"); }\nState.variables.time += hours;".to_string()
                    ),
                },
            ];

            let (js, _line_map) = custom_macros::build_script_passage_js(
                "Bug4Script", body, 0, &custom_macros,
            );

            // The standalone function should exist
            assert!(
                js.contains("function myMacro(...args)"),
                "Bug 4: should contain standalone 'function myMacro(...args)', got:\n{}",
                js
            );

            // Extract the standalone function body (after the function declaration line)
            let func_start = js.find("function myMacro(...args)")
                .expect("Bug 4: could not find standalone myMacro function");
            let func_end = js[func_start..].rfind("}\n")
                .map(|i| func_start + i + 2)
                .unwrap_or(js.len());
            let standalone_func = &js[func_start..func_end];

            // this.args → args
            assert!(
                !standalone_func.contains("this.args"),
                "Bug 4: standalone function should not contain 'this.args', got:\n{}",
                standalone_func
            );
            assert!(
                standalone_func.contains("args[0]"),
                "Bug 4: standalone function should contain 'args[0]', got:\n{}",
                standalone_func
            );

            // this.name → 'myMacro' string literal
            assert!(
                !standalone_func.contains("this.name"),
                "Bug 4: standalone function should not contain 'this.name', got:\n{}",
                standalone_func
            );
            assert!(
                standalone_func.contains("'myMacro'"),
                "Bug 4: standalone function should contain string literal 'myMacro', got:\n{}",
                standalone_func
            );

            // return this.error("msg") → throw new Error("msg")
            // Must NOT produce `return throw new Error(...)` which is invalid JS
            assert!(
                !standalone_func.contains("this.error"),
                "Bug 4: standalone function should not contain 'this.error', got:\n{}",
                standalone_func
            );
            assert!(
                !standalone_func.contains("return throw"),
                "Bug 4: standalone function must not contain 'return throw' (invalid JS), got:\n{}",
                standalone_func
            );
            assert!(
                standalone_func.contains("throw new Error("),
                "Bug 4: standalone function should contain 'throw new Error(', got:\n{}",
                standalone_func
            );
            assert!(
                standalone_func.contains("missing args"),
                "Bug 4: throw new Error should preserve the message, got:\n{}",
                standalone_func
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // Bug 5: Custom macro invocations must be translated as function
        // calls, not as /* unknown */ comments.
        //
        // When a [script] passage defines custom macros via Macro.add(),
        // and a normal passage invokes those macros, walk_translate() must
        // receive the callable names in its `callables` parameter so that
        // invocations are translated as function calls (e.g., `addTime(25)`)
        // rather than `/* unknown: <<addTime 25>> */`.
        // ═══════════════════════════════════════════════════════════════════
        {
            // Simulate callables that would be extracted from a script passage
            let callables = vec![
                UserCallable {
                    name: "setSceneLoc".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(1),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 0,
                    body: Some("State.variables.scene = this.args[0];".to_string()),
                },
                UserCallable {
                    name: "earn".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(1),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 5,
                    body: Some("State.variables.gold += this.args[0];".to_string()),
                },
                UserCallable {
                    name: "addTime".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(1),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 10,
                    body: Some("State.variables.time += this.args[0];".to_string()),
                },
                UserCallable {
                    name: "adjustStat".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(2),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 15,
                    body: Some("var stat = this.args[0]; var amount = this.args[1];".to_string()),
                },
                UserCallable {
                    name: "task".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(1),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 20,
                    body: None,
                },
                UserCallable {
                    name: "questLog".to_string(),
                    kind: UserCallableKind::CustomMacro,
                    arg_count: Some(2),
                    defined_in: "Macros".to_string(),
                    file_uri: "file:///test.tw".to_string(),
                    defined_at_line: 25,
                    body: None,
                },
            ];

            // Normal passage body that invokes these custom macros
            let body = r#"<<setSceneLoc "file-review-bay">>
<<earn 2500>>
<<task 1.5>>
<<questLog "first-day" "Cleared the oldest requests from the waiting stack.">>
<<adjustStat "stress"  9>>
<<adjustStat "stamina" -8>>
<<addTime 25>>
<<task 2.0>>
<<addTime 5>>"#;
            let body_offset = 0;
            let nodes = parse_passage_body(body, body_offset);
            let comment_spans: Vec<std::ops::Range<usize>> = vec![];

            let result = walk_translate(
                &nodes, body, body_offset, &callables, "Work", false, &comment_spans,
            );

            let js = &result.js_function;

            // NONE of the custom macro invocations should produce /* unknown */
            assert!(
                !js.contains("/* unknown */"),
                "Bug 5: Custom macro invocations should NOT produce /* unknown */ comments, got:\n{}",
                js
            );

            // All custom macros should be translated as function calls
            assert!(
                js.contains("setSceneLoc("),
                "Bug 5: setSceneLoc should be translated as a function call, got:\n{}",
                js
            );
            assert!(
                js.contains("earn("),
                "Bug 5: earn should be translated as a function call, got:\n{}",
                js
            );
            assert!(
                js.contains("addTime("),
                "Bug 5: addTime should be translated as a function call, got:\n{}",
                js
            );
            assert!(
                js.contains("adjustStat("),
                "Bug 5: adjustStat should be translated as a function call, got:\n{}",
                js
            );
            assert!(
                js.contains("task("),
                "Bug 5: task should be translated as a function call, got:\n{}",
                js
            );
            assert!(
                js.contains("questLog("),
                "Bug 5: questLog should be translated as a function call, got:\n{}",
                js
            );
        }
    }
}
