//! Semantic token and diagnostic building for SugarCube passages.
//!
//! This module contains functions that convert parsed AST nodes and passage
//! headers into LSP semantic tokens and diagnostics.

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, SemanticToken, SemanticTokenModifier, SemanticTokenType};
use super::ast;
use super::special_passages;

/// Build semantic tokens from AST nodes.
pub(super) fn build_semantic_tokens(nodes: &[ast::AstNode], tokens: &mut Vec<SemanticToken>, body_offset: usize) {
    for node in nodes {
        match node {
            ast::AstNode::Macro { name: _, name_span, var_refs, children, .. } => {
                // Macro name token
                tokens.push(SemanticToken {
                    start: body_offset + name_span.start,
                    length: name_span.end - name_span.start,
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });
                // Variable references in args
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if vr.is_write { Some(SemanticTokenModifier::Definition) } else { None },
                    });
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
            ast::AstNode::Expression { kind: _, span: _, var_refs, .. } => {
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: body_offset + vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
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

/// Build diagnostics from AST error nodes.
pub(super) fn build_diagnostics(nodes: &[ast::AstNode], diagnostics: &mut Vec<FormatDiagnostic>, body_offset: usize) {
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
pub(super) fn build_header_tokens(header: &crate::header::TweeHeader, is_special: bool) -> Vec<SemanticToken> {
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
pub(super) fn self_classify_tag(tag: &str) -> Option<SemanticTokenModifier> {
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
