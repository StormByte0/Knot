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
// Temp var collection
// ---------------------------------------------------------------------------

/// Collect unique temporary variable names (`_var`) from the tree.
///
/// Scans all nodes recursively, looking for VarRef entries with
/// `is_temporary == true`. Returns deduplicated names (without the `_`
/// prefix, matching JS `let` declaration convention).
fn collect_temp_vars(nodes: &[PassageNode]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    collect_temp_vars_inner(nodes, &mut seen);
    seen.into_iter().collect()
}

fn collect_temp_vars_inner(nodes: &[PassageNode], seen: &mut std::collections::BTreeSet<String>) {
    for node in nodes {
        match node {
            PassageNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    if vr.is_temporary {
                        // Strip the _ prefix for the JS `let` declaration
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
                    collect_temp_vars_inner(children, seen);
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
pub(crate) fn walk_translate(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    callables: &[crate::types::UserCallable],
    passage_name: &str,
    is_widget: bool,
) -> TranslateResult {
    let ctx = super::super::virtual_doc::TranslationContext::new(callables);
    let mut js_body = String::new();
    let mut line_mappings = Vec::new();
    let mut var_encounters = Vec::new();

    // Collect temp vars and emit `let _varname;` declarations
    let temp_vars = collect_temp_vars(nodes);

    // Build function header
    let func_name = if is_widget {
        // Widget passages: use the first widget name from callables defined
        // in this passage, or fall back to the passage name
        callables
            .iter()
            .find(|c| c.kind == crate::types::UserCallableKind::Widget && c.defined_in == passage_name)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| passage_name.replace(' ', "_"))
    } else {
        format!("passage_{}", passage_name.replace(' ', "_"))
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
    );

    js_output.push_str(&js_body);

    // Close the function
    js_output.push_str("}\n");
    line_mappings.push(ExactLineMapping {
        original_line: 0,
        original_start_byte: body_offset,
    });

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
fn walk_translate_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    ctx: &super::super::virtual_doc::TranslationContext,
    indent: usize,
    js_output: &mut String,
    line_mappings: &mut Vec<ExactLineMapping>,
    var_encounters: &mut Vec<VarEncounter>,
) {
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                // Text nodes: skip in the new selective translation.
                // Text between macros is not JS — it's rendered HTML.
                // (Previously, translate_text_segment was called, which
                // emitted empty lines for blank text. We now skip entirely.)
                let _ = (content, span);
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
                                super::super::virtual_doc::translate_expression(args)
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
