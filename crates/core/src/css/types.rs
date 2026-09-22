//! CSS types — the stable token/diagnostic outcome produced by
//! [`crate::css::parser`] and [`crate::css::fallback`].
//!
//! This module is the CSS equivalent of [`crate::oxc::types`] for JS and
//! [`crate::html::types`] for HTML. The types below are the **isolation
//! boundary** around the pinned `oxc-css-parser` crate (`=0.0.15` — 0.0.x
//! API churn is real): downstream callers (`sugarcube::css::analyze_css`,
//! the token builder, the parse pipeline) see only these shapes, never a
//! crate type, so a parser bump cannot leak past `knot-core`.
//!
//! ## Coordinate system
//!
//! All spans are **byte offsets into the source string passed to
//! [`crate::css::parse_css`] / [`crate::css::parse_css_declarations`]** —
//! NOT passage-relative, NOT document-absolute. Callers that parse a
//! sub-region (a `<style>` body, a `style="…"` value) shift the spans by
//! the region's offset themselves — the same contract as the oxc JS and
//! html5gum HTML integrations (see the format-layer call sites in
//! `token_builder.rs` for the shift arithmetic).
//!
//! ## Invariants
//!
//! The token stream of a [`CssParseOutcome`] is always **sorted by start
//! offset and non-overlapping** (the parser sorts and dedupes before
//! returning; the fallback scanner emits in scan order by construction).
//! This is what the downstream semantic-token pipeline depends on —
//! [`crate::css::parse_css`] guarantees it, so consumers never re-sort.

use std::ops::Range;

/// The highlight role a CSS token plays.
///
/// Kinds follow the *role* a construct plays in the stylesheet (not its
/// lexical class): a declaration name is a [`Property`](Self::Property)
/// wherever it appears, a bare ident is a [`Keyword`](Self::Keyword) in
/// value position but a [`Selector`](Self::Selector) in rule position.
/// This matches the Phase 4.2 scanner's classification so themes and tests
/// kept working when the real parser arrived (plan.md Phase 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssTokenKind {
    /// A declaration name (`color`, `margin`, `--main`). Custom-property
    /// names included — same kind, distinguished by consumers when needed.
    Property,
    /// A bare identifier in value position (`red`, `no-repeat`), a feature
    /// name in a media query (`min-width`), `!important`, at-rule prelude
    /// names (`and`/`or`/`not`).
    Keyword,
    /// Numbers, dimensions (number + unit, e.g. `1.5em`), percentages, hex
    /// colors (`#ff00aa`), and unicode ranges.
    Number,
    /// Quoted strings, in values and at-rule arguments.
    String,
    /// Selector parts: tag names, `.class`, `#id`, attribute/pseudo names,
    /// raw idents in rule position.
    Selector,
    /// At-rule name **with the leading `@`** (`@media`) — the parser extends
    /// the crate's name span one byte back over the sigil so the token
    /// covers the whole spelling.
    AtRule,
    /// Custom properties (`--x`) in value position, postcss `$var` families,
    /// Less/`@{}` interpolation tokens.
    Variable,
    /// A function name (`calc`, `var`, `url`).
    Function,
    /// `/* … */` comments (collected separately by the parser, not AST
    /// nodes).
    Comment,
    /// Structural punctuation: braces, colons, semicolons, commas,
    /// delimiters, operators.
    Punctuation,
    /// **Reserved, never emitted** by either the parser or the fallback
    /// scanner (whitespace is skipped, not tokenized). Kept in the enum so
    /// the semantic-token mapping table stays exhaustive — it maps this to
    /// "skip" explicitly.
    Whitespace,
}

/// One classified CSS token with its source extent.
///
/// The span is non-empty (zero-width spans are filtered at emission — see
/// `push` in `parser.rs`), sorted against its siblings, and source-relative
/// (see the module's coordinate-system note).
#[derive(Debug, Clone)]
pub struct CssToken {
    /// The highlight role this token plays.
    pub kind: CssTokenKind,
    /// Byte range in the source passed to the parse entry point.
    pub span: Range<usize>,
}

/// Severity of a CSS diagnostic.
///
/// The parser itself only reports [`Error`](Self::Error) today (relayed
/// recoverable parse errors). The Warning/Info tiers exist so future lint
/// passes (e.g. "duplicate property in the same block") can flow through
/// the same type without a formats-side change — the severity mapping in
/// `sugarcube::css` already handles all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssDiagnosticSeverity {
    /// A definite syntax problem the parser recovered from (or could not
    /// recover from). Rendered as an error to the author.
    Error,
    /// Suspicious-but-parseable input. Reserved for future lint passes —
    /// the parser itself only reports errors today.
    Warning,
    /// Informational notes. Reserved for future lint passes.
    Info,
}

/// A CSS syntax diagnostic.
///
/// Ranges are source-relative (same coordinate system as [`CssToken`]).
/// The message is the parser's own error text; the fallback scanner's
/// unclosed-block summary is the only non-parser diagnostic produced
/// today.
#[derive(Debug, Clone)]
pub struct CssDiagnostic {
    /// Human-readable error message.
    pub message: String,
    /// Byte range in the source passed to the parse entry point.
    pub range: Range<usize>,
    /// Severity — always `Error` from the parser today (see
    /// [`CssDiagnosticSeverity`]).
    pub severity: CssDiagnosticSeverity,
}

/// The outcome of parsing CSS: the classified token stream plus relayed
/// diagnostics.
///
/// This is the CSS counterpart of [`crate::oxc::JsParseOutcome`] for JS.
/// Unlike JS (whose AST may be blanked by a fatal parse), the token stream
/// is **always usable for highlighting** — the parser recovers at statement
/// level (css-syntax-3), and a hard failure falls back to the tolerant
/// scanner so tokens never go dark (see `parser.rs` / `fallback.rs`).
#[derive(Debug, Clone)]
pub struct CssParseOutcome {
    /// Classified, sorted, non-overlapping tokens (see the module's
    /// invariants note).
    pub tokens: Vec<CssToken>,
    /// Recoverable parse errors relayed with real severity — the user-facing
    /// policy is "show breaking code when written", never hide. Empty when
    /// the CSS was clean.
    pub diagnostics: Vec<CssDiagnostic>,
}

impl CssParseOutcome {
    /// Returns `true` when there were no diagnostics at all — the CSS
    /// parsed clean. (Parity with `JsParseOutcome::is_clean`; highlighting
    /// needs no such check — the token stream is always usable.)
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }
}
