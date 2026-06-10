//! Semantic token and diagnostic building for SugarCube passages.
//!
//! This module contains functions that convert parsed AST nodes and passage
//! headers into LSP semantic tokens and diagnostics.

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, SemanticToken, SemanticTokenModifier, SemanticTokenType};
use crate::sugarcube::ast;
use crate::sugarcube::special_passages;

/// Build semantic tokens from AST nodes.
///
/// When a node has `js_analysis`, emits tokens from its `var_ops` with
/// per-segment precision. Otherwise falls back to `var_refs` for backward
/// compat (e.g., when the annotation pass hasn't run).
pub fn build_semantic_tokens(nodes: &[ast::AstNode], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for node in nodes {
        match node {
            ast::AstNode::Macro { name: _, name_span, js_analysis, var_refs, children, .. } => {
                // Macro name token
                tokens.push(SemanticToken {
                    start: body_offset + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });
                // Variable references: prefer js_analysis, fallback to var_refs
                if let Some(analysis) = js_analysis {
                    for op in &analysis.var_ops {
                        emit_var_op_tokens(op, tokens, body_offset);
                    }
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
                // Recurse into block macro children
                if let Some(ch) = children {
                    build_semantic_tokens(ch, tokens, body_offset);
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
        for seg_span in &op.segment_spans {
            tokens.push(SemanticToken {
                start: body_offset + seg_span.start,
                length: seg_span.end - seg_span.start,
                token_type: SemanticTokenType::Variable,
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

/// Build diagnostics from AST error nodes.
pub fn build_diagnostics(nodes: &[ast::AstNode], diagnostics: &mut Vec<FormatDiagnostic>, body_offset: usize) {
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
            if children.is_some() && close_span.is_none() {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + name_span.start..body_offset + name_span.end,
                    message: format!("Unclosed block macro: <<{}>>", name),
                    severity: FormatDiagnosticSeverity::Error,
                    code: "sc-unclosed".to_string(),
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
                while pos < len && (bytes[pos].is_ascii_digit() || bytes[pos] == b'.' || bytes[pos] == b'e' || bytes[pos] == b'E' || bytes[pos] == b'+' || bytes[pos] == b'-') && pos > start || bytes[pos].is_ascii_digit() {
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
