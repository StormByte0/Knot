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
//!
//! CSS-specific roles map to DEDICATED token types (`cssProperty`,
//! `cssSelector`, `cssAtRule`) so themes can style CSS distinctly; the
//! role kinds shared with other languages (keywords, numbers, strings,
//! variables, functions, comments, punctuation) keep riding the shared
//! families (`keyword`, `number`, …) — standard practice in VS Code's own
//! CSS support.

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
            CssTokenKind::Property => SemanticTokenType::CssProperty,
            CssTokenKind::Keyword => SemanticTokenType::Keyword,
            CssTokenKind::Number => SemanticTokenType::Number,
            CssTokenKind::String => SemanticTokenType::String,
            CssTokenKind::Selector => SemanticTokenType::CssSelector,
            CssTokenKind::AtRule => SemanticTokenType::CssAtRule,
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
            code: css_diagnostic_code(&diag.message).to_string(),
        });
    }

    CssAnalysis {
        tokens,
        diagnostics,
    }
}

/// Classify a core CSS diagnostic's code from its message shape.
///
/// Core diagnostics arrive message-only: parse errors carry the parser's
/// error text while the lint messages have fixed prefixes. This is the
/// single classifier for every relay surface (stylesheet passages via
/// [`css_outcome_to_analysis`], and the embedded surfaces via
/// `embedded_diags`), so the Problems panel can filter lint warnings apart
/// from parse errors consistently — `css-parse` for the parser tier,
/// `css-unknown-property` / `css-unknown-pseudo-class` /
/// `css-unknown-pseudo-element` / `css-pseudo-element-placement` for the
/// lints.
pub fn css_diagnostic_code(message: &str) -> &'static str {
    if message.starts_with("Unknown CSS property") {
        "css-unknown-property"
    } else if message.starts_with("Unknown CSS pseudo-class") {
        "css-unknown-pseudo-class"
    } else if message.starts_with("Unknown CSS pseudo-element") {
        "css-unknown-pseudo-element"
    } else if message.starts_with("CSS pseudo-element must be the last part") {
        "css-pseudo-element-placement"
    } else {
        "css-parse"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The dedicated CSS token types carry the CSS-specific roles; shared
    /// roles (keywords, numbers, functions, punctuation) keep riding the
    /// shared families. This pins the mapping table.
    #[test]
    fn role_kinds_map_to_dedicated_and_shared_types() {
        let src = "@media (min-width: 600px) { .hud { color: var(--x); } }";
        let analysis = analyze_css(src);
        let find = |needle: &str| {
            let at = src.find(needle).expect(needle);
            analysis
                .tokens
                .iter()
                .find(|t| t.start == at)
                .unwrap_or_else(|| panic!("no token starts at `{needle}`"))
                .token_type
        };
        assert_eq!(find("@media"), SemanticTokenType::CssAtRule);
        assert_eq!(find(".hud"), SemanticTokenType::CssSelector);
        assert_eq!(find("color"), SemanticTokenType::CssProperty);
        assert_eq!(find("min-width"), SemanticTokenType::Keyword);
        assert_eq!(find("600px"), SemanticTokenType::Number);
        assert_eq!(find("var"), SemanticTokenType::Function);
        assert_eq!(find("--x"), SemanticTokenType::Variable);
        // Punctuation (`:` after `color`, the `{` braces) rides Operator.
        let colon = src.find("color:").map(|p| p + "color".len()).unwrap();
        assert_eq!(
            analysis
                .tokens
                .iter()
                .find(|t| t.start == colon)
                .expect("the `:` after `color` is a token")
                .token_type,
            SemanticTokenType::Operator,
        );
    }

    /// `style="…"` values parse as a declaration LIST: property names
    /// classify even without an enclosing block.
    #[test]
    fn declaration_list_classifies_properties() {
        let analysis = analyze_css_declarations("color: red; margin: 0");
        assert!(
            analysis
                .tokens
                .iter()
                .any(|t| t.token_type == SemanticTokenType::CssProperty
                    && t.start == 0
                    && t.length == 5)
        );
        assert!(
            analysis
                .tokens
                .iter()
                .any(|t| t.token_type == SemanticTokenType::CssProperty
                    && t.start == "color: red; ".len()
                    && t.length == 6)
        );
    }

    /// The lint codes classify by message shape — parse errors, property
    /// typos, and the selector lints land on distinct codes so the Problems
    /// panel can filter them apart on EVERY relay surface.
    #[test]
    fn diagnostic_codes_classify_by_message_shape() {
        assert_eq!(css_diagnostic_code("Unexpected token: `}`"), "css-parse");
        assert_eq!(
            css_diagnostic_code("Unknown CSS property: `colr`"),
            "css-unknown-property"
        );
        assert_eq!(
            css_diagnostic_code(
                "Unknown CSS pseudo-class: `:hvoer` — browsers drop the whole rule"
            ),
            "css-unknown-pseudo-class"
        );
        assert_eq!(
            css_diagnostic_code(
                "Unknown CSS pseudo-element: `::befor` — browsers drop the whole rule"
            ),
            "css-unknown-pseudo-element"
        );
        assert_eq!(
            css_diagnostic_code(
                "CSS pseudo-element must be the last part of the selector: `::before` — as written, browsers drop the whole rule"
            ),
            "css-pseudo-element-placement"
        );
    }

    /// End-to-end through the real parser: a stylesheet with selector
    /// mistakes carries the new lint diagnostics with the right codes.
    #[test]
    fn stylesheet_selector_lints_relay() {
        let analysis = analyze_css("a:hvoer { color: red }\np::befor { }\ndiv::before:hover { }");
        let codes: Vec<&str> = analysis
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect();
        assert!(
            codes.contains(&"css-unknown-pseudo-class"),
            "codes: {codes:?}"
        );
        assert!(
            codes.contains(&"css-unknown-pseudo-element"),
            "codes: {codes:?}"
        );
        assert!(
            codes.contains(&"css-pseudo-element-placement"),
            "codes: {codes:?}"
        );
        assert!(
            analysis
                .diagnostics
                .iter()
                .all(|d| d.severity == FormatDiagnosticSeverity::Warning)
        );
    }
}
