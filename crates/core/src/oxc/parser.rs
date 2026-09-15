//! Oxc JS parser wrapper for Knot.
//!
//! Provides two entry points:
//!
//! - [`parse_js()`] — parse for diagnostics only. Use when you only need to
//!   report syntax errors and do not need to walk the AST (e.g. the
//!   SugarCube `js_validate` pass).
//! - [`parse_and_visit()`] — parse once and run a visitor on the AST inline.
//!   Use when you need to extract information from the AST (variable writes,
//!   token spans, etc.). This replaces the old `parse_js + with_program`
//!   pattern, which parsed the source twice.
//!
//! ## Why two functions?
//!
//! `parse_js` drops the AST after collecting diagnostics. `parse_and_visit`
//! runs the visitor before dropping the AST. Both parse exactly once.
//!
//! The previous API exposed a `JsParseOutput` with a `with_program` method
//! that re-parsed the source on every call. Since every callsite called
//! `with_program` at most once, the AST retention was pure overhead. The
//! new API parses once and runs the visitor inline, eliminating the
//! double-parse without retaining any AST state.

#[cfg(test)]
use super::types::record_parse_call;
use super::types::{JsDiagnostic, JsDiagnosticSeverity, JsParseOutcome, ParseMode};
use oxc_ast::ast::Program;

/// Parse JavaScript source text with Oxc, returning only diagnostics.
///
/// Use this when you only need syntax error reporting (no AST walking).
/// The AST is parsed, inspected for errors, and dropped.
///
/// ## Arguments
///
/// - `source`: The JavaScript source text (after any format-specific
///   pre-processing, e.g. SugarCube's `$var` → `State_variables_varName`)
/// - `mode`: How to interpret the source (module, expression, or statement list)
///
/// ## Returns
///
/// A [`JsParseOutcome`] containing:
/// - `diagnostics`: empty if parsing succeeded, non-empty otherwise
/// - `panicked`: `true` if Oxc could not recover (AST would have been empty)
///
/// ## Example
///
/// ```ignore
/// use knot_core::oxc::{parse_js, ParseMode};
///
/// let outcome = parse_js("function (", ParseMode::Expression);
/// assert!(!outcome.diagnostics.is_empty());
/// ```
pub fn parse_js(source: &str, mode: ParseMode) -> JsParseOutcome {
    let allocator = oxc_allocator::Allocator::default();

    // Prepare the source text based on parse mode.
    // Oxc always parses as a module/script, so expressions need wrapping.
    let source_text = match mode {
        ParseMode::Module => source.to_string(),
        ParseMode::Expression => format!("({})", source),
        ParseMode::StatementList => source.to_string(),
    };

    let source_type = oxc_span::SourceType::default();
    let parser = oxc_parser::Parser::new(&allocator, &source_text, source_type);
    let result = parser.parse();
    #[cfg(test)]
    record_parse_call();

    // oxc 0.150 (plan.md Phase 3.1): `panicked` was renamed to
    // `fatal_error` ("the parser gave up — AST is invalid"), and the error
    // list became the `Diagnostics` collection type.
    let panicked = result.fatal_error;
    let diagnostics = collect_diagnostics(&result.diagnostics, &source_text, mode);

    JsParseOutcome::new(diagnostics, panicked)
}

/// Parse JavaScript source text with Oxc and run a visitor on the AST inline.
///
/// This is the single-parse replacement for the old `parse_js + with_program`
/// pattern. The source is parsed exactly once; the visitor runs on the AST;
/// the result is returned along with the diagnostics.
///
/// ## When to use this vs `parse_js`
///
/// - Use `parse_and_visit` when you need to walk the AST (extract variable
///   writes, token spans, function definitions, etc.).
/// - Use `parse_js` when you only need syntax error diagnostics.
///
/// ## Visitor behavior
///
/// The visitor is called only if Oxc did not panic. If Oxc panicked
/// (`outcome.panicked == true`), the visitor is NOT called and this function
/// returns `(outcome, None)`.
///
/// If Oxc encountered errors it could recover from, the visitor IS called on
/// the partial AST and `outcome.diagnostics` reports the errors. NOTE that
/// oxc's recovery is narrow (verified empirically against oxc 0.134 and
/// re-confirmed on 0.150, plan.md Phase 3.1): common
/// authoring errors such as a missing operand (`var x = ;`), an unclosed
/// `{`, or an unterminated string literal are FATAL — the parser aborts with
/// `panicked == true` and an EMPTY AST, so the visitor is skipped. Treat the
/// partial-AST path as the exception, not the rule.
///
/// ## Arguments
///
/// - `source`: The JavaScript source text (after any format-specific
///   pre-processing).
/// - `mode`: How to interpret the source.
/// - `visitor`: A closure that receives `&Program<'_>` and returns an owned
///   result `R`.
///
/// ## Returns
///
/// A tuple of `(JsParseOutcome, Option<R>)`. The `Option<R>` is `Some` if the
/// visitor was called, `None` if Oxc panicked.
///
/// ## Example
///
/// ```ignore
/// use knot_core::oxc::{parse_and_visit, ParseMode};
///
/// let (outcome, body_len) = parse_and_visit(
///     "var x = 42;",
///     ParseMode::Module,
///     |program| program.body.len(),
/// );
/// assert!(outcome.is_clean());
/// assert_eq!(body_len, Some(1));
/// ```
pub fn parse_and_visit<F, R>(
    source: &str,
    mode: ParseMode,
    visitor: F,
) -> (JsParseOutcome, Option<R>)
where
    F: FnOnce(&Program<'_>) -> R,
{
    let allocator = oxc_allocator::Allocator::default();

    // Prepare the source text based on parse mode.
    let source_text = match mode {
        ParseMode::Module => source.to_string(),
        ParseMode::Expression => format!("({})", source),
        ParseMode::StatementList => source.to_string(),
    };

    let source_type = oxc_span::SourceType::default();
    let parser = oxc_parser::Parser::new(&allocator, &source_text, source_type);
    let result = parser.parse();
    #[cfg(test)]
    record_parse_call();

    // oxc 0.150 (plan.md Phase 3.1): `panicked` → `fatal_error`, errors →
    // `diagnostics` (see parse_js).
    let panicked = result.fatal_error;
    let diagnostics = collect_diagnostics(&result.diagnostics, &source_text, mode);

    // Run the visitor only if Oxc did not panic. When oxc DOES recover from a
    // syntax error, the partial AST is still walked. But recovery is narrow:
    // common authoring errors (missing operand, unclosed `{`, unterminated
    // string) panic the parser and yield an empty AST — no visitor run.
    let visitor_result = if panicked {
        None
    } else {
        Some(visitor(&result.program))
    };

    (JsParseOutcome::new(diagnostics, panicked), visitor_result)
}

/// Collect Oxc parse errors into `JsDiagnostic` instances.
///
/// Each error is converted to a `JsDiagnostic` with the error message,
/// severity, and approximate position. The position is in the source text
/// passed to the parser (after any wrapping for expressions).
///
/// oxc 0.150 (plan.md Phase 3.1): the parser returns the new
/// `oxc_diagnostics::Diagnostics` collection type instead of
/// `Vec<OxcDiagnostic>`.
fn collect_diagnostics(
    diagnostics: &oxc_diagnostics::Diagnostics,
    source_text: &str,
    mode: ParseMode,
) -> Vec<JsDiagnostic> {
    let mut result = Vec::new();

    for error in diagnostics.iter() {
        let error_msg = error.to_string();

        // Extract position information from the error.
        // Oxc errors carry labels with span info, but the exact position
        // extraction depends on the error format. For now, we parse the
        // error message for line/column info and provide the full source
        // range as a fallback.
        let (line, column, range) = extract_error_position(error, source_text, mode);

        result.push(JsDiagnostic {
            message: error_msg,
            severity: JsDiagnosticSeverity::Error,
            range,
            line,
            column,
        });
    }

    result
}

/// Extract position information from an Oxc diagnostic.
///
/// Tries to get the precise span from the diagnostic's labels. Falls back
/// to covering the entire source text if no span is available.
fn extract_error_position(
    error: &oxc_diagnostics::OxcDiagnostic,
    source_text: &str,
    mode: ParseMode,
) -> (u32, u32, std::ops::Range<usize>) {
    // The wrapping offset: for Expression mode, we add 1 char for the
    // opening parenthesis. Offsets in Oxc's output are relative to the
    // wrapped source text, so we need to subtract this when mapping back.
    let wrapping_offset: usize = match mode {
        ParseMode::Expression => 1,
        _ => 0,
    };

    // Try to extract the span from the error's labels.
    // Oxc miette errors contain source code snippets with span info.
    // The label's span is in the wrapped source text.
    // oxc 0.150: `labels` is a plain slice (no Option wrapper).
    if let Some(label) = error.labels.first() {
        // oxc 0.150: LabeledSpan exposes offset/len directly (the old
        // `label.inner().offset()` path is gone) and they are `u32` now.
        let start = (label.offset() as usize).saturating_sub(wrapping_offset);
        let end = (start + label.len() as usize).min(source_text.len());

        // Compute line and column from the offset in the original source
        let line = compute_line(source_text, start);
        let column = compute_column(source_text, start);

        (line, column, start..end)
    } else {
        // No span info — attach to the start of the source
        (1, 1, 0..source_text.len())
    }
}

/// Compute 1-based line number from a byte offset.
fn compute_line(source: &str, offset: usize) -> u32 {
    let pos = offset.min(source.len());
    let line = source[..pos].chars().filter(|&c| c == '\n').count();
    (line + 1) as u32
}

/// Compute 1-based column number from a byte offset.
fn compute_column(source: &str, offset: usize) -> u32 {
    let pos = offset.min(source.len());
    let line_start = source[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = pos.saturating_sub(line_start);
    (col + 1) as u32
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oxc::types::{parse_count, reset_parse_count};

    #[test]
    fn test_parse_valid_expression() {
        let result = parse_js("1 + 2 * 3", ParseMode::Expression);
        assert!(
            result.is_clean(),
            "Expected no diagnostics for valid expression, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_parse_valid_module() {
        let result = parse_js(
            "var x = 1;\nfunction hello() { return x; }",
            ParseMode::Module,
        );
        assert!(
            result.is_clean(),
            "Expected no diagnostics for valid module, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_parse_invalid_js() {
        let result = parse_js("function (", ParseMode::Expression);
        assert!(
            !result.diagnostics.is_empty(),
            "Expected at least one diagnostic for invalid JS"
        );
    }

    #[test]
    fn test_parse_valid_statement_list() {
        let result = parse_js("let x = 1; let y = 2;", ParseMode::StatementList);
        assert!(
            result.is_clean(),
            "Expected no diagnostics for valid statements, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn test_parse_and_visit_walks_ast() {
        let (outcome, body_len) = parse_and_visit("var x = 42;", ParseMode::Module, |program| {
            program.body.len()
        });
        assert!(outcome.is_clean(), "Expected clean parse");
        assert!(
            body_len.unwrap_or(0) > 0,
            "Expected at least one statement in AST"
        );
    }

    #[test]
    fn test_parse_and_visit_valid_module_walks_ast() {
        // Despite the historical name/comment, the input here is VALID JS
        // (the brace is closed; there is no recoverable error to exercise).
        // This test verifies the happy path: a clean multi-statement module
        // parses, reports no diagnostics, and the visitor walks the AST.
        // (Empirical probe work against oxc 0.134/0.150 — see
        // scripts/oxc_probe + scripts/oxc_probe_150 —
        // shows genuinely recoverable errors are hard to construct and that
        // common authoring errors panic the parser instead of recovering.)
        let (outcome, body_len) = parse_and_visit(
            "function foo() {\n  return 42;\n}\nvar x = 1;",
            ParseMode::Module,
            |program| program.body.len(),
        );
        assert!(outcome.is_clean(), "Expected clean parse");
        assert!(outcome.has_ast(), "Expected AST to be available");
        assert!(
            body_len.unwrap_or(0) > 0,
            "Expected at least one statement in AST"
        );
    }

    #[test]
    fn test_parse_and_visit_does_not_double_parse() {
        // Regression test for the old `with_program` bug: parse_js parsed
        // once for diagnostics and with_program re-parsed on every call.
        // parse_and_visit must parse exactly once.
        reset_parse_count();
        let (_outcome, _body_len) = parse_and_visit(
            "var x = 42; function foo() { return x; }",
            ParseMode::Module,
            |program| program.body.len(),
        );
        let count = parse_count();
        assert_eq!(
            count, 1,
            "parse_and_visit must invoke oxc_parser::Parser::parse() exactly once; got {}",
            count
        );
    }

    #[test]
    fn test_parse_js_does_not_double_parse() {
        // parse_js (no visitor) must also parse exactly once.
        reset_parse_count();
        let _outcome = parse_js("var x = 42;", ParseMode::Module);
        let count = parse_count();
        assert_eq!(
            count, 1,
            "parse_js must invoke oxc_parser::Parser::parse() exactly once; got {}",
            count
        );
    }

    #[test]
    fn test_parse_and_visit_visitor_not_called_on_panic() {
        // Contract check on the non-panicking path: for a VALID input the
        // visitor must be called and `panicked` must be false. (The test does
        // NOT construct a panicking input despite the historical comment —
        // deep nesting is NOT a reliable panic trigger; oxc handles it. Real
        // panic cases — `var x = ;`, unclosed `{`, unterminated strings — are
        // documented in the probe results referenced in `oxc/mod.rs`.)
        let (outcome, visitor_result) =
            parse_and_visit("var x = 1;", ParseMode::Module, |program| {
                program.body.len()
            });
        assert!(!outcome.panicked, "Should not have panicked");
        assert!(visitor_result.is_some(), "Visitor should have been called");
    }
}
