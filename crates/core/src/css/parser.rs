//! CSS parser entry point — currently a placeholder.
//!
//! `parse_css()` returns an empty [`CssParseOutcome`] (no tokens, no
//! diagnostics). This keeps the type signature stable so a future CSS
//! crate can be plugged in here without touching any downstream caller
//! (`sugarcube::css::analyze_css`, `token_builder`, `parse_pipeline`).
//!
//! ## Why empty
//!
//! The previous implementation used `cssparser` (Mozilla's tokenizer) with
//! a hand-rolled state machine. It produced tokens but no diagnostics, and
//! the state machine was fragile around `@media` nesting and custom
//! properties. Rather than ship a half-working parser, we are explicitly
//! marking CSS as unserved until a proper CSS crate is integrated.
//!
//! ## What this means for users
//!
//! - Stylesheet passages (`[stylesheet]` tag) produce no semantic tokens.
//! - Inline `<<style>>` / `<<css>>` blocks produce no semantic tokens.
//! - The parse pipeline emits an `Info`-level diagnostic on each
//!   stylesheet passage so the absence is visible, not silent.
//!
//! ## Re-introducing CSS
//!
//! A future implementation should:
//! 1. Add the chosen CSS crate to `Cargo.toml`.
//! 2. Re-implement `parse_css()` to populate `tokens` and `diagnostics`.
//! 3. Leave `CssTokenKind` / `CssToken` / `CssDiagnostic` / `CssParseOutcome`
//!    unchanged unless a coordinated change is made across
//!    `knot-formats/src/sugarcube/css/mod.rs` (which maps these to
//!    `SemanticToken` / `FormatDiagnostic`).

use super::types::CssParseOutcome;

/// Parse CSS source text and return classified tokens + diagnostics.
///
/// **Currently a no-op**: always returns an empty outcome. See the module
/// docs for the rationale and re-integration plan.
pub fn parse_css(_source: &str) -> CssParseOutcome {
    CssParseOutcome {
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    }
}
