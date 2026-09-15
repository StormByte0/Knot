//! SugarCube CSS analysis — converts `CssParseOutcome` (from `knot_core::css`)
//! into `SemanticToken`s + `FormatDiagnostic`s for stylesheet passages and
//! `<<style>>` blocks.
//!
//! Thin pass-through over `knot_core::css::parse_css` (plan.md Phase 6: the
//! core service parses with the `oxc-css-parser` crate and maps its
//! span-carrying AST to tokens; recoverable parse errors are relayed with
//! real severity instead of being hidden — see plan.md §0.1 policy):
//! the core parser populates tokens + diagnostics, and this mapping table
//! converts them to `SemanticToken`s / `FormatDiagnostic`s for all
//! CSS-bearing constructs (stylesheets, `<style>` bodies,
//! `<<style>>`/`<<css>>` blocks, `style="…"` attribute values).

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity, SemanticToken, SemanticTokenType};
use knot_core::css::{self, CssDiagnosticSeverity, CssParseOutcome, CssTokenKind};

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

/// Parse a CSS *declaration list* (`prop: value;` without a block — the
/// `style="…"` attribute shape, plan.md Phase 4.2) and produce semantic
/// tokens. Spans are relative to the start of `source`.
pub fn analyze_css_declarations(source: &str) -> CssAnalysis {
    let outcome = css::parse_css_declarations(source);
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
            severity: css_severity_to_format(diag.severity),
            code: "css-parse".to_string(),
        });
    }

    CssAnalysis {
        tokens,
        diagnostics,
    }
}

/// Relay the core CSS severities 1:1. The crate reports parse errors; the
/// Warning/Info tiers exist for future lint passes and map through already
/// so new core severities surface without another formats-side change.
fn css_severity_to_format(severity: CssDiagnosticSeverity) -> FormatDiagnosticSeverity {
    match severity {
        CssDiagnosticSeverity::Error => FormatDiagnosticSeverity::Error,
        CssDiagnosticSeverity::Warning => FormatDiagnosticSeverity::Warning,
        CssDiagnosticSeverity::Info => FormatDiagnosticSeverity::Info,
    }
}

/// Shift relayed CSS diagnostics by `offset` (the pipeline's
/// `body_offset_in_passage`) so they can be appended directly to a
/// passage's diagnostic list, which expects passage-relative ranges —
/// the same shift the token stream already applies per token.
pub fn shift_css_diagnostics(
    diagnostics: Vec<FormatDiagnostic>,
    offset: usize,
) -> Vec<FormatDiagnostic> {
    diagnostics
        .into_iter()
        .map(|d| FormatDiagnostic {
            range: offset + d.range.start..offset + d.range.end,
            ..d
        })
        .collect()
}
