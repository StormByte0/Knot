//! Semantic token and diagnostic building for SugarCube passages.
//!
//! This module contains functions that convert parsed AST nodes and passage
//! headers into semantic tokens and diagnostics.
//!
//! ## Passage-Relative Offsets
//!
//! All token offsets produced by this module are **passage-relative**: byte 0
//! is the `::` prefix of the passage header. This design enables incremental
//! passage updates — when a single passage is edited, only that passage's
//! token group needs to be regenerated, and the passage's document offset
//! is applied at the LSP boundary.
//!
//! The conversion pipeline is:
//! ```text
//! AST spans (body-relative, 0 = after header newline)
//!   → add body_offset_in_passage (offset of body start from passage head)
//!   → passage-relative tokens (0 = passage head `::`)
//!   → at LSP boundary: add passage_offset → document-absolute byte offset
//! ```

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
///
/// **Passage-relative offsets**: All emitted tokens have `start` values
/// relative to the passage head (`::` prefix). The `body_offset_in_passage`
/// parameter is the byte offset of the body start relative to the passage
/// head (i.e., `body_document_offset - header_start`). AST body spans are relative
/// to the body start, so adding `body_offset_in_passage` converts them to
/// passage-relative offsets.
pub fn build_semantic_tokens(
    nodes: &[ast::AstNode],
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
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
                    start: body_offset_in_passage + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type,
                    modifier,
                });
                // For <<widget>> definitions: emit Function + Definition token
                // for the name being defined (e.g., "myHelper" in <<widget myHelper>>)
                if let Some(def_span) = definition_name_span {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + def_span.start,
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
                        start: body_offset_in_passage + ct.span.start,
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
                        start: body_offset_in_passage + fl.index_var.span.start,
                        length: fl.index_var.span.end - fl.index_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + fl.iterated_var.span.start,
                        length: fl.iterated_var.span.end - fl.iterated_var.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset_in_passage);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
                    // Emit function call site tokens from oxc analysis
                    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + vr.span.start,
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
                    emit_structured_arg_tokens(sargs, tokens, body_offset_in_passage);
                }
                // Recurse into block macro children
                if let Some(ch) = children {
                    build_semantic_tokens(ch, tokens, body_offset_in_passage, custom_macro_names);
                }
            }
            ast::AstNode::Link { target, span, .. } => {
                // Link target token
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start + 2, // past [[
                    length: target.len(),
                    token_type: SemanticTokenType::Link,
                    modifier: None,
                });
            }
            ast::AstNode::Expression { js_analysis, var_refs, .. } => {
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset_in_passage);
                    }
                    // Emit literal tokens (strings, numbers, booleans, null)
                    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
                    // Emit operator tokens (SugarCube keywords + JS operators)
                    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
                    // Emit namespace tokens (SugarCube global objects)
                    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
                    // Emit function definition tokens from oxc analysis
                    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
                    // Emit function call site tokens from oxc analysis
                    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
                } else {
                    for vr in var_refs {
                        tokens.push(SemanticToken {
                            start: body_offset_in_passage + vr.span.start,
                            length: vr.span.end - vr.span.start,
                            token_type: SemanticTokenType::Variable,
                            modifier: None,
                        });
                    }
                }
            }
            ast::AstNode::Comment { span, .. } => {
                tokens.push(SemanticToken {
                    start: body_offset_in_passage + span.start,
                    length: span.end - span.start,
                    token_type: SemanticTokenType::Comment,
                    modifier: None,
                });
            }
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + vr.span.start,
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

/// Build semantic tokens from a script passage's `JsAnalysis`.
///
/// Script passages contain pure JS (no SugarCube syntax). Their analysis is
/// stored on `PassageAst::script_js_analysis` rather than on AST nodes. This
/// function emits all token types (variables, literals, operators, namespaces,
/// function defs, function calls) from that analysis.
pub fn build_script_passage_tokens(
    analysis: &ast::JsAnalysis,
    tokens: &mut Vec<SemanticToken>,
    body_offset_in_passage: usize,
) {
    for op in &analysis.var_ops {
        emit_var_op_tokens(op, tokens, body_offset_in_passage);
    }
    emit_literal_tokens(&analysis.literal_spans, tokens, body_offset_in_passage);
    emit_operator_tokens(&analysis.operator_spans, tokens, body_offset_in_passage);
    emit_namespace_tokens(&analysis.namespace_spans, tokens, body_offset_in_passage);
    emit_function_def_tokens(&analysis.function_defs, tokens, body_offset_in_passage);
    emit_function_call_tokens(&analysis.function_calls, tokens, body_offset_in_passage);
}

/// Emit semantic tokens for a single `AnalyzedVarOp`.
///
/// Uses `segment_spans` for per-token highlighting when available,
/// giving exact span precision for each variable/property token.
fn emit_var_op_tokens(op: &ast::AnalyzedVarOp, tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
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
                start: body_offset_in_passage + seg_span.start,
                length: seg_span.end - seg_span.start,
                token_type,
                modifier,
            });
        }
    } else {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + op.span.start,
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
/// mapped through the preprocessor), so we just add `body_offset_in_passage`.
fn emit_literal_tokens(literals: &[ast::LiteralSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for lit in literals {
        let token_type = match lit.kind {
            ast::LiteralKind::String  => SemanticTokenType::String,
            ast::LiteralKind::Number  => SemanticTokenType::Number,
            ast::LiteralKind::Boolean => SemanticTokenType::Boolean,
            ast::LiteralKind::Null    => SemanticTokenType::Keyword,
        };
        tokens.push(SemanticToken {
            start: body_offset_in_passage + lit.span.start,
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
fn emit_operator_tokens(operators: &[ast::OperatorSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for op in operators {
        let modifier = match op.kind {
            ast::OperatorKind::Logical => Some(SemanticTokenModifier::ControlFlow),
            _ => None,
        };
        tokens.push(SemanticToken {
            start: body_offset_in_passage + op.span.start,
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
fn emit_namespace_tokens(namespaces: &[ast::NamespaceSpan], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for ns in namespaces {
        // Namespace token for the global object name
        tokens.push(SemanticToken {
            start: body_offset_in_passage + ns.span.start,
            length: ns.span.end - ns.span.start,
            token_type: SemanticTokenType::Namespace,
            modifier: None,
        });
        // Property tokens for each property accessed on the global
        for prop in &ns.property_spans {
            tokens.push(SemanticToken {
                start: body_offset_in_passage + prop.span.start,
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
fn emit_function_def_tokens(function_defs: &[ast::FunctionDefInfo], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for func_def in function_defs {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + func_def.name_offset,
            length: func_def.name.len(),
            token_type: SemanticTokenType::Function,
            modifier: Some(SemanticTokenModifier::Definition),
        });
    }
}

/// Emit semantic tokens for function call sites found by oxc.
///
/// When an identifier that was preprocessed from a SugarCube `$var` or `_var`
/// is used as a function call target (e.g., `_myHelper()`), it should be
/// classified as a function call, not a variable reference. This function
/// emits `Function` tokens for those call sites.
fn emit_function_call_tokens(function_calls: &[ast::FunctionCallInfo], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
    for call in function_calls {
        tokens.push(SemanticToken {
            start: body_offset_in_passage + call.span.start,
            length: call.span.end - call.span.start,
            token_type: SemanticTokenType::Function,
            modifier: None,
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
fn emit_structured_arg_tokens(sargs: &[ast::StructuredMacroArg], tokens: &mut Vec<SemanticToken>, body_offset_in_passage: usize) {
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
            start: body_offset_in_passage + sarg.span.start,
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
///
/// All diagnostic ranges are **passage-relative**: byte 0 is the `::` prefix
/// of the passage header. The `body_offset_in_passage` parameter converts
/// AST body spans (which are relative to body start) to passage-relative
/// offsets. The LSP boundary adds `passage_offset` to produce
/// document-absolute ranges.
pub fn build_diagnostics(nodes: &[ast::AstNode], diagnostics: &mut Vec<FormatDiagnostic>, body_offset_in_passage: usize) {
    let dep_macros = deprecated_macros();
    for node in nodes {
        if let ast::AstNode::Error { message, span } = node {
            diagnostics.push(FormatDiagnostic {
                range: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                message: message.clone(),
                severity: FormatDiagnosticSeverity::Error,
                code: "sc-parse".to_string(),
            });
        }
        if let ast::AstNode::Macro { children, name, name_span, close_span, .. } = node {
            // Unclosed block macro diagnostic
            if children.is_some() && close_span.is_none() {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset_in_passage + name_span.start..body_offset_in_passage + name_span.end,
                    message: format!("Unclosed block macro: <<{}>>", name),
                    severity: FormatDiagnosticSeverity::Error,
                    code: "sc-unclosed".to_string(),
                });
            }
            // Deprecated macro usage diagnostic
            if let Some(msg) = dep_macros.get(name.as_str()) {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset_in_passage + name_span.start..body_offset_in_passage + name_span.end,
                    message: (*msg).to_string(),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "sc-deprecated".to_string(),
                });
            }
            if let Some(ch) = children {
                build_diagnostics(ch, diagnostics, body_offset_in_passage);
            }
        }
    }
}

/// Find a tag name's byte offset within `[...]` bracket blocks.
///
/// Scans only inside `[...]` blocks in `tags_raw`, avoiding false matches
/// on the passage name portion. Returns the byte offset relative to the
/// start of `tags_raw` (which the caller adjusts by `name_start` to get
/// a document-absolute offset).
///
/// For example, given `tags_raw = "DarkForest [dark scary]"` and tag `"dark"`,
/// this returns `Some(12)` (pointing to "dark" inside the brackets), NOT `Some(0)`
/// (which would incorrectly point to "Dark" in "DarkForest").
fn find_tag_in_brackets(tags_raw: &str, tag: &str) -> Option<usize> {
    let bytes = tags_raw.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Find the next '['
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }

        // Found '[' — find matching ']'
        let bracket_start = i;
        let mut j = i + 1;
        while j < len && bytes[j] != b']' {
            j += 1;
        }

        if j >= len {
            // Unclosed bracket — no more blocks to search
            break;
        }

        // We have a complete `[...]` block. Search for the tag
        // as a whole word within this block's interior.
        let interior = &tags_raw[bracket_start + 1..j];
        if let Some(pos_in_interior) = find_word_in_text(interior, tag) {
            return Some(bracket_start + 1 + pos_in_interior);
        }

        // Move past this bracket block
        i = j + 1;
    }

    None
}

/// Find a whole-word occurrence of `word` in `text`, returning the byte offset.
///
/// A "whole word" means the match is bounded by non-alphanumeric characters
/// (or the start/end of text). This prevents "dark" from matching "darkness".
fn find_word_in_text(text: &str, word: &str) -> Option<usize> {
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();
    let text_bytes = text.as_bytes();
    let text_len = text_bytes.len();

    if word_len == 0 || word_len > text_len {
        return None;
    }

    let mut search_start = 0;
    while search_start <= text_len - word_len {
        // Find the next occurrence of the word text
        if let Some(pos) = text[search_start..].find(word) {
            let abs_pos = search_start + pos;

            // Check that the character before the match is a word boundary
            let before_ok = if abs_pos == 0 {
                true
            } else {
                let prev = text_bytes[abs_pos - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };

            // Check that the character after the match is a word boundary
            let after_pos = abs_pos + word_len;
            let after_ok = if after_pos >= text_len {
                true
            } else {
                let next = text_bytes[after_pos];
                !next.is_ascii_alphanumeric() && next != b'_'
            };

            if before_ok && after_ok {
                return Some(abs_pos);
            }

            // Not a whole-word match — advance past this occurrence
            search_start = abs_pos + 1;
        } else {
            break;
        }
    }

    None
}

/// Build header tokens for a passage.
///
/// Produces semantic tokens for the `::` prefix, the passage name, and
/// each tag within `[...]` blocks. All byte offsets are **passage-relative**:
/// byte 0 is the `::` prefix of the passage header. The `TweeHeader` fields
/// `header_start` and `name_start` are document-absolute, so we subtract
/// `header_start` to produce passage-relative offsets.
pub fn build_header_tokens(header: &crate::header::TweeHeader, is_special: bool) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // All header offsets are relative to the passage head (:: prefix).
    // The TweeHeader stores document-absolute offsets, so we subtract
    // header_start to make them passage-relative.
    let head = header.header_start;

    // :: prefix token — always at passage-relative offset 0
    let header_type = if is_special {
        SemanticTokenType::SpecialPassageHeader
    } else {
        SemanticTokenType::PassageHeader
    };
    tokens.push(SemanticToken {
        start: 0, // :: is always at the very start of the passage
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
        start: header.name_start - head,
        length: name_len,
        token_type: name_type,
        modifier: None,
    });

    // Tag tokens — only the tag names, with appropriate modifiers.
    //
    // We search for each tag inside `[...]` bracket blocks within
    // `tags_raw`, NOT by doing a simple `find()` on the entire string.
    // A naive `find()` could match a tag name that appears as a
    // substring of the passage name (e.g., tag "dark" matching inside
    // "DarkForest" in `tags_raw = "DarkForest [dark]"`).
    //
    // `tags_raw[0]` aligns with `name_start` in the document, so
    // `name_start + tag_pos - head` gives the passage-relative offset.
    for tag in &header.tags {
        if let Some(tag_pos) = find_tag_in_brackets(&header.tags_raw, tag) {
            let modifier = self_classify_tag(tag);
            tokens.push(SemanticToken {
                start: header.name_start - head + tag_pos,
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
pub fn build_json_body_tokens(body: &str, body_offset_in_passage: usize) -> Vec<SemanticToken> {
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
                            start: body_offset_in_passage + content_start,
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
                            start: body_offset_in_passage + content_start,
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
                        start: body_offset_in_passage + start,
                        length: end - start,
                        token_type: SemanticTokenType::Number,
                        modifier: None,
                    });
                }
            }
            b't' => {
                if body[pos..].starts_with("true") {
                    tokens.push(SemanticToken {
                        start: body_offset_in_passage + pos,
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
                        start: body_offset_in_passage + pos,
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
                        start: body_offset_in_passage + pos,
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
