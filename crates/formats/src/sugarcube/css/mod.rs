//! SugarCube CSS analysis — walks cssparser tokens and produces
//! semantic tokens + diagnostics for stylesheet passages and <<style>> blocks.

use knot_core::css::{self, CssTokenKind, CssParseOutcome};
use crate::plugin::{SemanticToken, SemanticTokenType, FormatDiagnostic, FormatDiagnosticSeverity};

#[derive(Debug, Clone, Default)]
pub struct CssAnalysis {
    pub tokens: Vec<SemanticToken>,
    pub diagnostics: Vec<FormatDiagnostic>,
}

/// Parse CSS source and produce semantic tokens + diagnostics.
/// Spans are relative to the start of `source` (caller adds body_offset).
pub fn analyze_css(source: &str) -> CssAnalysis {
    let outcome = css::parse_css(source);
    css_outcome_to_analysis(&outcome, 0)
}

/// Convert a CssParseOutcome to CssAnalysis, shifting spans by body_offset.
pub fn css_outcome_to_analysis(outcome: &CssParseOutcome, body_offset: usize) -> CssAnalysis {
    let mut tokens = Vec::new();
    let mut diagnostics = Vec::new();

    for token in &outcome.tokens {
        let sem_type = match token.kind {
            CssTokenKind::Property => SemanticTokenType::Property,
            CssTokenKind::Keyword => SemanticTokenType::Keyword,
            CssTokenKind::Number => SemanticTokenType::Number,
            CssTokenKind::String => SemanticTokenType::String,
            CssTokenKind::Selector => SemanticTokenType::Tag,
            CssTokenKind::AtRule => SemanticTokenType::Keyword,
            CssTokenKind::Variable => SemanticTokenType::Variable,
            CssTokenKind::Function => SemanticTokenType::Function,
            CssTokenKind::Comment => SemanticTokenType::Comment,
            CssTokenKind::Punctuation => SemanticTokenType::Operator,
            CssTokenKind::Whitespace => continue,
        };
        tokens.push(SemanticToken {
            start: body_offset + token.span.start,
            length: token.span.end - token.span.start,
            token_type: sem_type,
            modifier: None,
        });
    }

    for diag in &outcome.diagnostics {
        diagnostics.push(FormatDiagnostic {
            range: body_offset + diag.range.start..body_offset + diag.range.end,
            message: diag.message.clone(),
            severity: FormatDiagnosticSeverity::Error,
            code: "css-parse".to_string(),
        });
    }

    CssAnalysis { tokens, diagnostics }
}
