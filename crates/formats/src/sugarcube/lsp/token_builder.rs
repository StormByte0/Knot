//! Semantic token and diagnostic building for SugarCube passages.
//!
//! This module contains functions that convert parsed AST nodes and passage
//! headers into LSP semantic tokens and diagnostics.

use std::collections::HashSet;

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, SemanticToken, SemanticTokenModifier, SemanticTokenType};
use crate::sugarcube::ast;
use crate::sugarcube::macros::deprecated_macros;
use crate::sugarcube::special_passages;

/// Build semantic tokens from AST nodes.
///
/// When a node has `js_analysis`, emits tokens from its `var_ops` with
/// per-segment precision. Otherwise falls back to `var_refs` for backward
/// compat (e.g., when the annotation pass hasn't run).
///
/// `custom_macro_names` is the set of user-defined macro/widget names from
/// the custom macro registry. Macro names in this set get `Function` token
/// type instead of `Macro`, enabling distinct visual styling for user-defined
/// macros vs builtins.
pub fn build_semantic_tokens(
    nodes: &[ast::AstNode],
    tokens: &mut Vec<SemanticToken>,
    body_offset: usize,
    custom_macro_names: &HashSet<String>,
) {
    for node in nodes {
        match node {
            ast::AstNode::Macro { name, name_span, js_analysis, var_refs, children, definition_name_span, capture_target, for_loop_vars, structured_args, .. } => {
                // Macro name token — differentiate builtin vs custom/widget
                let token_type = if custom_macro_names.contains(name) {
                    SemanticTokenType::Function
                } else {
                    SemanticTokenType::Macro
                };
                // Deprecated macros get the Deprecated modifier (enables strikethrough)
                let modifier = if deprecated_macros().contains_key(name.as_str()) {
                    Some(SemanticTokenModifier::Deprecated)
                } else {
                    None
                };
                tokens.push(SemanticToken {
                    start: body_offset + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type,
                    modifier,
                });
                // For <<widget>> definitions: emit Function + Definition token
                // for the name being defined (e.g., "myHelper" in <<widget myHelper>>)
                if let Some(def_span) = definition_name_span {
                    tokens.push(SemanticToken {
                        start: body_offset + def_span.start,
                        length: def_span.end - def_span.start,
                        token_type: SemanticTokenType::Function,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                }
                // For <<capture>> macros: emit Variable + Definition token for
                // the capture target (e.g., "$target" in <<capture $target>>).
                // This provides AST-level capture highlighting that complements
                // the JS annotation pass.
                if let Some(ct) = capture_target {
                    tokens.push(SemanticToken {
                        start: body_offset + ct.span.start,
                        length: ct.span.end - ct.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                }
                // For <<for>> macros with simplified iteration form:
                // emit Variable + Definition for the index var (write target),
                // and Variable (no modifier) for the iterated var (read).
                if let Some(fl) = for_loop_vars {
                    tokens.push(SemanticToken {
                        start: body_offset + fl.index_var.span.start,
                        length: fl.index_var.span.end - fl.index_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    tokens.push(SemanticToken {
                        start: body_offset + fl.iterated_var.span.start,
                        length: fl.iterated_var.span.end - fl.iterated_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset + vr.span.start,
                            length: vr.span.end - vr.span.start,
                            token_type: SemanticTokenType::Variable,
                            modifier: if vr.is_write { Some(SemanticTokenModifier::Definition) } else { None },
                        });
                    }
                }
                // Emit tokens for structured args from catalog (Phase 6).
                // Passage references get Link tokens; labels and selectors get
                // appropriate highlighting. Variable refs in structured args
                // get Variable tokens (these complement, not replace, the
                // js_analysis/var_refs variable tokens — structured_args may
                // capture variable refs that the JS scanner doesn't classify
                // correctly for navigation macros like <<goto $dest>>).
                if let Some(sargs) = structured_args {
                    emit_structured_arg_tokens(sargs, tokens, body_offset);
                }
                // Recurse into block macro children
                if let Some(ch) = children {
                    build_semantic_tokens(ch, tokens, body_offset, custom_macro_names);
                }
            }
            ast::AstNode::Link { target, span, .. } => {
                // Link target token
                tokens.push(SemanticToken {
                    start: body_offset + span.start + 2, // past [[
                    length: target.len(),
                    token_type: SemanticTokenType::Link,
                    modifier: None,
                });
            }
            ast::AstNode::Expression { js_analysis, var_refs, .. } => {
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset + vr.span.start,
                            length: vr.span.end - vr.span.start,
                            token_type: SemanticTokenType::Variable,
                            modifier: None,
                        });
                    }
                }
            }
            ast::AstNode::Comment { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::Comment,
                    modifier: None,
                });
            }
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
            }
            ast::AstNode::Error { .. } => {}
            // MacroClose nodes are consumed by the tree builder and should not
            // appear in the final AST. If one slips through, skip it.
            ast::AstNode::MacroClose { .. } => {}
        }
    }
}

/// Emit semantic tokens for a single `AnalyzedVarOp`.
///
/// Uses `segment_spans` for per-token highlighting when available,
/// giving exact span precision for each variable/property token.
fn emit_var_op_tokens(op: &ast::AnalyzedVarOp, tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    let modifier = if op.access_kind.is_write() {
        Some(SemanticTokenModifier::Definition)
    } else {
        None
    };

    if !op.segment_spans.is_empty() {
        for (i, seg_span) in op.segment_spans.iter().enumerate() {
            let token_type = if i == 0 {
                SemanticTokenType::Variable  // $foo — the root variable
            } else {
                SemanticTokenType::Property  // .bar, .baz — property access
            };
            tokens.push(SemanticToken {
                start: body_offset + seg_span.start,
                length: seg_span.end - seg_span.start,
                token_type,
                modifier,
            });
        }
    } else {
        tokens.push(SemanticToken {
            start: body_offset + op.span.start,
            length: op.span.end - op.span.start,
            token_type: SemanticTokenType::Variable,
            modifier,
        });
    }
}

/// Emit semantic tokens for literal spans found by oxc.
///
/// Maps `LiteralKind` to the appropriate `SemanticTokenType` and pushes
/// a token for each literal. The spans are passage-body-relative (already
/// mapped through the preprocessor), so we just add `body_offset`.
fn emit_literal_tokens(literals: &[ast::LiteralSpan], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for lit in literals {
        let token_type = match lit.kind {
            ast::LiteralKind::String  => SemanticTokenType::String,
            ast::LiteralKind::Number  => SemanticTokenType::Number,
            ast::LiteralKind::Boolean => SemanticTokenType::Boolean,
            ast::LiteralKind::Null    => SemanticTokenType::Keyword,
        };
        tokens.push(SemanticToken {
            start: body_offset + lit.span.start,
            length: lit.span.end - lit.span.start,
            token_type,
            modifier: None,
        });
    }
}

/// Emit semantic tokens for operator spans found by oxc and the preprocessor.
///
/// SugarCube keyword operators (`to`, `eq`, `and`, etc.) are classified as
/// comparison/assignment/logical operators by `OperatorKind`. Standard JS
/// operators that weren't substituted are also emitted here. Logical operators
/// get the `ControlFlow` modifier to enable distinct visual styling.
fn emit_operator_tokens(operators: &[ast::OperatorSpan], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for op in operators {
        let modifier = match op.kind {
            ast::OperatorKind::Logical => Some(SemanticTokenModifier::ControlFlow),
            _ => None,
        };
        tokens.push(SemanticToken {
            start: body_offset + op.span.start,
            length: op.span.end - op.span.start,
            token_type: SemanticTokenType::Operator,
            modifier,
        });
    }
}

/// Emit semantic tokens for namespace spans found by oxc.
///
/// SugarCube global objects like `State`, `Engine`, `Story`, `Config`, etc.
/// get `Namespace` tokens. Properties accessed on them get `Property` tokens.
/// This provides distinct visual styling for global object references that
/// aren't `$variable` accesses.
///
/// **Deduplication note**: `State.variables.x` patterns are NOT emitted here
/// because they're already covered by `AnalyzedVarOp` (which emits `Variable`
/// + `Property` tokens). Only non-variable-access patterns on globals
/// (e.g., `State.turns`, `Engine.play()`, `Config.debug`) are emitted.
fn emit_namespace_tokens(namespaces: &[ast::NamespaceSpan], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for ns in namespaces {
        // Namespace token for the global object name
        tokens.push(SemanticToken {
            start: body_offset + ns.span.start,
            length: ns.span.end - ns.span.start,
            token_type: SemanticTokenType::Namespace,
            modifier: None,
        });
        // Property tokens for each property accessed on the global
        for prop in &ns.property_spans {
            tokens.push(SemanticToken {
                start: body_offset + prop.span.start,
                length: prop.span.end - prop.span.start,
                token_type: SemanticTokenType::Property,
                modifier: None,
            });
        }
    }
}

/// Emit semantic tokens for function definitions found by oxc.
///
/// Function declarations and named function expressions in JS contexts
/// (inside `<<run>>`, `<<script>>`, etc.) get `Function` tokens with
/// the `Definition` modifier. This covers patterns like:
/// - `function myHelper() { ... }` → Function token on "myHelper"
/// - `var calculateScore = function() { ... }` → Function token on "calculateScore"
/// - `const add = () => { ... }` → Function token on "add"
fn emit_function_def_tokens(function_defs: &[ast::FunctionDefInfo], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for func_def in function_defs {
        tokens.push(SemanticToken {
            start: body_offset + func_def.name_offset,
            length: func_def.name.len(),
            token_type: SemanticTokenType::Function,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}

/// Emit semantic tokens for structured macro arguments (Phase 6).
///
/// Each `StructuredMacroArg` gets a token type based on its `ParsedArgKind`:
///
/// - `PassageRef` → `Link` token (same as `[[ ]]` links, enabling consistent
///   passage name highlighting and go-to-definition)
/// - `Label` → `String` token (display text in link/button macros)
/// - `Selector` → `String` token (CSS selectors)
/// - `String` → `String` token (generic string values like speed "2s")
/// - `VariableRef` → `Variable` token (variable used as passage target, etc.)
/// - `Expression` → no token (JS expressions are handled by oxc)
///
/// **Deduplication**: `VariableRef` tokens from structured_args may overlap
/// with tokens from `js_analysis`/`var_refs`. The token builder emits both
/// because structured_args captures the *semantic role* (e.g., `$dest` as a
/// dynamic passage target) while var_ops captures the *JS semantics* (read
/// vs write). The editor typically uses the last-emitted token for a position,
/// so this is acceptable — the user sees the correct highlighting either way.
fn emit_structured_arg_tokens(sargs: &[ast::StructuredMacroArg], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for sarg in sargs {
        let token_type = match sarg.kind {
            ast::ParsedArgKind::PassageRef => SemanticTokenType::Link,
            ast::ParsedArgKind::Label => SemanticTokenType::String,
            ast::ParsedArgKind::Selector => SemanticTokenType::String,
            ast::ParsedArgKind::String => SemanticTokenType::String,
            ast::ParsedArgKind::VariableRef => SemanticTokenType::Variable,
            ast::ParsedArgKind::Expression => continue, // Handled by oxc
        };
        tokens.push(SemanticToken {
            start: body_offset + sarg.span.start,
            length: sarg.span.end - sarg.span.start,
            token_type,
            modifier: None,
        });
    }
}

/// Build diagnostics from AST error nodes and deprecated macro usage.
///
/// Emits:
/// - Error diagnostics for parse errors and unclosed block macros
/// - Hint diagnostics for deprecated macro usage (with deprecation message)
pub fn build_diagnostics(nodes: &[ast::AstNode], diagnostics: &mut Vec<FormatDiagnostic>, body_offset: usize) {
    let dep_macros = deprecated_macros();
    for node in nodes {
        if let ast::AstNode::Error { message, span } = node {
            diagnostics.push(FormatDiagnostic {
                range: body_offset + span.start..body_offset + span.end,
                message: message.clone(),
                severity: FormatDiagnosticSeverity::Error,
                code: "sc-parse".to_string(),
            });
        }
        if let ast::AstNode::Macro { children, name, name_span, close_span, .. } = node {
            // Unclosed block macro diagnostic
            if children.is_some() && close_span.is_none() {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + name_span.start..body_offset + name_span.end,
                    message: format!("Unclosed block macro: <<{}>>", name),
                    severity: FormatDiagnosticSeverity::Error,
                    code: "sc-unclosed".to_string(),
                });
            }
            // Deprecated macro usage diagnostic
            if let Some(msg) = dep_macros.get(name.as_str()) {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + name_span.start..body_offset + name_span.end,
                    message: (*msg).to_string(),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "sc-deprecated".to_string(),
                });
            }
            if let Some(ch) = children {
                build_diagnostics(ch, diagnostics, body_offset);
            }
        }
    }
}

/// Build header tokens for a passage.
pub fn build_header_tokens(header: &crate::header::TweeHeader, is_special: bool) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // :: prefix token
    let header_type = if is_special {
        SemanticTokenType::SpecialPassageHeader
    } else {
        SemanticTokenType::PassageHeader
    };
    tokens.push(SemanticToken {
        start: header.header_start,
        length: 2, // ::
        token_type: header_type,
        modifier: None,
    });

    // Passage name token
    let name_type = if is_special {
        SemanticTokenType::SpecialPassage
    } else {
        SemanticTokenType::PassageName
    };
    let name_len = header.name.len();
    tokens.push(SemanticToken {
        start: header.name_start,
        length: name_len,
        token_type: name_type,
        modifier: None,
    });

    // Tag tokens — only the tag names, with appropriate modifiers
    for tag in &header.tags {
        if let Some(tag_pos) = header.tags_raw.find(tag.as_str()) {
            // Classify the tag to determine its modifier
            let modifier = self_classify_tag(tag);
            tokens.push(SemanticToken {
                start: header.name_start + tag_pos,
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier,
            });
        }
    }

    tokens
}

/// Classify a tag and return the appropriate semantic token modifier.
pub fn self_classify_tag(tag: &str) -> Option<SemanticTokenModifier> {
    // Core tags: [script], [stylesheet], [style]
    for def in knot_core::passage::twine_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Legacy core tags
    for def in knot_core::passage::legacy_core_special_passages() {
        if def.match_strategy == knot_core::passage::MatchStrategy::Tag
            && tag.eq_ignore_ascii_case(&def.name)
        {
            return Some(SemanticTokenModifier::TwineCore);
        }
    }
    // Format-specific tags: [init], [widget]
    for def in special_passages::tag_matched_special_passages() {
        if tag.eq_ignore_ascii_case(&def.name) {
            return Some(SemanticTokenModifier::StoryFormat);
        }
    }
    None
}

/// Build semantic tokens for a JSON body (used for StoryData passages).
pub fn build_json_body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;

    while pos < len {
        match bytes[pos] {
            b'"' => {
                let start = pos;
                pos += 1;
                let mut is_escaped = false;
                while pos < len {
                    if is_escaped {
                        is_escaped = false;
                        pos += 1;
                        continue;
                    }
                    if bytes[pos] == b'\\' {
                        is_escaped = true;
                        pos += 1;
                        continue;
                    }
                    if bytes[pos] == b'"' {
                        pos += 1;
                        break;
                    }
                    pos += 1;
                }
                let end = pos;

                let mut lookahead = pos;
                while lookahead < len && bytes[lookahead] == b' ' || lookahead < len && bytes[lookahead] == b'\t' || lookahead < len && bytes[lookahead] == b'\n' || lookahead < len && bytes[lookahead] == b'\r' {
                    lookahead += 1;
                }
                let is_property_name = lookahead < len && bytes[lookahead] == b':';

                if is_property_name {
                    let content_start = start + 1;
                    let content_end = end.saturating_sub(1);
                    if content_start < content_end {
                        tokens.push(SemanticToken {
                            start: body_offset + content_start,
                            length: content_end - content_start,
                            token_type: SemanticTokenType::Property,
                            modifier: Some(SemanticTokenModifier::TwineCore),
                        });
                    }
                } else {
                    let content_start = start + 1;
                    let content_end = end.saturating_sub(1);
                    if content_start < content_end {
                        tokens.push(SemanticToken {
                            start: body_offset + content_start,
                            length: content_end - content_start,
                            token_type: SemanticTokenType::String,
                            modifier: None,
                        });
                    }
                }
            }
            b'0'..=b'9' | b'-' => {
                let start = pos;
                if bytes[pos] == b'-' {
                    pos += 1;
                }
                while pos < len && (bytes[pos].is_ascii_digit() || bytes[pos] == b'.' || bytes[pos] == b'e' || bytes[pos] == b'E' || bytes[pos] == b'+' || bytes[pos] == b'-') {
                    pos += 1;
                }
                if pos == start + 1 && bytes[start] == b'-' {
                    continue;
                }
                let end = pos;
                if end > start {
                    tokens.push(SemanticToken {
                        start: body_offset + start,
                        length: end - start,
                        token_type: SemanticTokenType::Number,
                        modifier: None,
                    });
                }
            }
            b't' => {
                if body[pos..].starts_with("true") {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: 4,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                    pos += 4;
                    continue;
                }
                pos += 1;
            }
            b'f' => {
                if body[pos..].starts_with("false") {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: 5,
                        token_type: SemanticTokenType::Boolean,
                        modifier: None,
                    });
                    pos += 5;
                    continue;
                }
                pos += 1;
            }
            b'n' => {
                if body[pos..].starts_with("null") {
                    tokens.push(SemanticToken {
                        start: body_offset + pos,
                        length: 4,
                        token_type: SemanticTokenType::Keyword,
                        modifier: None,
                    });
                    pos += 4;
                    continue;
                }
                pos += 1;
            }
            _ => {
                pos += 1;
            }
        }
    }

    tokens
}
