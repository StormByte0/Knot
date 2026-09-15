//! Types for the Oxc JS parsing service.

use std::ops::Range;

// ---------------------------------------------------------------------------
// Parse mode
// ---------------------------------------------------------------------------

/// Determines how the Oxc parser should interpret the source text.
///
/// Different JS contexts within a story format require different parse modes:
/// - Macro arguments like `<<run expr>>` contain JS expressions
/// - Script passages contain full JS programs
/// - Inline blocks contain JS statements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    /// Parse as a JS module/program (full top-level statements and declarations).
    /// Used for `<<script>>...<</script>>` blocks and [script] tagged passages.
    Module,

    /// Parse as a JS expression.
    /// The source is wrapped in parentheses before parsing so Oxc accepts
    /// bare expressions. Used for macro arguments: `<<run expr>>`,
    /// `<<set expr>>`, `<<if cond>>`, etc.
    Expression,

    /// Parse as a JS statement list.
    /// Like `Module` but without import/export. Used for inline `{...}` JS
    /// blocks within macro arguments.
    StatementList,
}

// ---------------------------------------------------------------------------
// Diagnostic types
// ---------------------------------------------------------------------------

/// Severity of a JavaScript syntax diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsDiagnosticSeverity {
    /// A syntax error that prevents parsing.
    Error,
    /// A potential issue or unusual construct.
    Warning,
}

/// A JavaScript syntax diagnostic from Oxc parsing.
///
/// Positions are in the source text that was passed to `parse_js()` /
/// `parse_and_visit()` (after any format-specific pre-processing like `$var`
/// substitution). The **caller** (format) is responsible for mapping these
/// positions back to the original document coordinates, since only the format
/// knows what pre-processing transformations were applied.
#[derive(Debug, Clone)]
pub struct JsDiagnostic {
    /// Human-readable error message from Oxc.
    pub message: String,
    /// Severity of the diagnostic.
    pub severity: JsDiagnosticSeverity,
    /// Byte range in the source text passed to `parse_js()`.
    pub range: Range<usize>,
    /// 1-based line number in the source text passed to `parse_js()`.
    pub line: u32,
    /// 1-based column number in the source text passed to `parse_js()`.
    pub column: u32,
}

// ---------------------------------------------------------------------------
// Parse outcome
// ---------------------------------------------------------------------------

/// The outcome of parsing JavaScript with Oxc.
///
/// This is returned by both [`crate::oxc::parse_js`] (when only diagnostics
/// are needed) and [`crate::oxc::parse_and_visit`] (when the caller also
/// wants to walk the AST).
///
/// ## Design
///
/// This struct intentionally does **not** retain the parsed AST. Retaining it
/// would require a self-referential struct (the `Program` borrows from the
/// `Allocator`), which means either the `ouroboros` crate (new dependency)
/// or hand-rolled unsafe code. Neither is justified: every callsite that
/// needs the AST calls the visitor exactly once, so the visitor can run
/// inline during the parse call. See `parse_and_visit`.
///
/// ## Fields
///
/// - `diagnostics`: empty if parsing succeeded. Non-empty if oxc reported
///   syntax errors. NOTE which errors are which (verified empirically against
///   oxc 0.134): most hand-written errors — missing operand (`var x = ;`),
///   unclosed `{`, unterminated string literal — are UNRECOVERABLE (AST was
///   empty, visitor NOT called). Recoverable errors that still yield a
///   walkable partial AST are possible but uncommon.
/// - `panicked`: `true` if Oxc could not recover. When `true`, the AST was
///   empty and `parse_and_visit` did not call the visitor.
#[derive(Debug, Clone)]
pub struct JsParseOutcome {
    /// Syntax diagnostics. Empty if parsing succeeded.
    pub diagnostics: Vec<JsDiagnostic>,
    /// Whether the parser panicked (could not recover). When `true`, the
    /// AST was empty and any visitor passed to `parse_and_visit` was NOT
    /// called.
    pub panicked: bool,
}

impl JsParseOutcome {
    /// Construct a parse outcome from its parts.
    ///
    /// This is `pub(crate)` because the canonical constructors are
    /// `parse_js` and `parse_and_visit` in `parser.rs`. Format plugins
    /// consume `JsParseOutcome` but never construct one directly.
    pub(crate) fn new(diagnostics: Vec<JsDiagnostic>, panicked: bool) -> Self {
        Self {
            diagnostics,
            panicked,
        }
    }

    /// Returns `true` if parsing had no errors at all.
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Returns `true` if the AST was available for walking (i.e. the parser
    /// did not panic). This is `!self.panicked`.
    ///
    /// Note: `true` does NOT guarantee `diagnostics` is empty — when oxc DOES
    /// recover from a syntax error it produces a partial AST, and in that case
    /// `has_ast()` is `true` while `diagnostics` is non-empty. However oxc's
    /// recovery is narrow: for most authoring errors (missing operand, unclosed
    /// brace, unterminated string) the parser panics and this returns `false`
    /// with an empty AST.
    pub fn has_ast(&self) -> bool {
        !self.panicked
    }
}

// ---------------------------------------------------------------------------
// Test-only parse counter
// ---------------------------------------------------------------------------

// Thread-local counter for the number of `oxc_parser::Parser::parse()` calls
// made by `parse_js` and `parse_and_visit`. Test-only — used to assert that
// `parse_and_visit` parses exactly once (regression protection against
// re-introducing a `with_program`-style double-parse).
#[cfg(test)]
thread_local! {
    pub(crate) static PARSE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn record_parse_call() {
    PARSE_COUNT.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
pub(crate) fn parse_count() -> usize {
    PARSE_COUNT.with(|c| c.get())
}

#[cfg(test)]
pub(crate) fn reset_parse_count() {
    PARSE_COUNT.with(|c| c.set(0));
}
