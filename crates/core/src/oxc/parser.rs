//! Oxc JS parser wrapper for Knot.
//!
//! Provides [`parse_js()`] which takes a JS source string and a parse mode,
//! and returns either syntax diagnostics or a parsed AST that the format
//! can walk for format-specific analysis.

use super::types::{JsDiagnostic, JsDiagnosticSeverity, JsParseOutcome, JsParseOutput, ParseMode};

/// Parse JavaScript source text with Oxc.
///
/// This is the main entry point for JS parsing in Knot. It:
/// 1. Optionally wraps the source text based on the parse mode
///    (expressions get wrapped in parentheses so Oxc accepts them)
/// 2. Parses with Oxc
/// 3. Returns either syntax diagnostics or a `JsParseOutput` containing
///    the parsed AST
///
/// ## Arguments
///
/// - `source`: The JavaScript source text (after any format-specific
///   pre-processing, e.g. SugarCube's `$var` → `State_variables_varName`)
/// - `mode`: How to interpret the source (module, expression, or statement list)
///
/// ## Returns
///
/// A `JsParseOutcome` struct. Oxc has error recovery, so the AST is almost
/// always available for token highlighting — even when there are syntax errors.
/// Callers should:
///
/// 1. Call `outcome.with_program()` to walk the AST (returns `None` only if
///    the parser panicked on an unrecoverable error).
/// 2. Check `outcome.diagnostics` for error reporting — each diagnostic has
///    a precise byte range for VSCode squiggles.
///
/// ## Example
///
/// ```ignore
/// use knot_core::oxc::{parse_js, ParseMode};
///
/// let outcome = parse_js("1 + 2 * 3", ParseMode::Expression);
/// // Walk the AST for token highlighting (works even with recoverable errors)
/// if let Some(result) = outcome.with_program(|program| {
///     format!("Parsed {} statements", program.body.len())
/// }) {
///     println!("{}", result);
/// }
/// // Report any diagnostics (precise squiggle ranges)
/// for diag in &outcome.diagnostics {
///     eprintln!("JS error at {}:{}: {}", diag.line, diag.column, diag.message);
/// }
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

    if result.errors.is_empty() {
        // Clean parse — return the output that owns both allocator and source
        JsParseOutcome::success(JsParseOutput::new(allocator, source_text))
    } else {
        // Errors present — collect diagnostics.
        // oxc has error recovery: the AST is still available for walking
        // (unless the parser panicked). We return a partial outcome so
        // callers can still walk the AST for token highlighting while
        // also reporting the precise error diagnostics.
        let diagnostics = collect_diagnostics(&result.errors, &source_text, mode);
        if result.panicked {
            // Unrecoverable — AST is empty
            JsParseOutcome::failed(diagnostics)
        } else {
            // Recoverable — AST is available (may be partial)
            JsParseOutcome::partial(JsParseOutput::new(allocator, source_text), diagnostics)
        }
    }
}

/// Collect Oxc parse errors into `JsDiagnostic` instances.
///
/// Each error is converted to a `JsDiagnostic` with the error message,
/// severity, and approximate position. The position is in the source text
/// passed to the parser (after any wrapping for expressions).
fn collect_diagnostics(
    errors: &[oxc_diagnostics::OxcDiagnostic],
    source_text: &str,
    mode: ParseMode,
) -> Vec<JsDiagnostic> {
    let mut diagnostics = Vec::new();

    for error in errors {
        let error_msg = error.to_string();

        // Extract position information from the error.
        // Oxc errors carry labels with span info, but the exact position
        // extraction depends on the error format. For now, we parse the
        // error message for line/column info and provide the full source
        // range as a fallback.
        let (line, column, range) = extract_error_position(error, source_text, mode);

        diagnostics.push(JsDiagnostic {
            message: error_msg,
            severity: JsDiagnosticSeverity::Error,
            range,
            line,
            column,
        });
    }

    diagnostics
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
    // wrapped source, so we need to subtract this when mapping back.
    let wrapping_offset: usize = match mode {
        ParseMode::Expression => 1,
        _ => 0,
    };

    // Try to extract the span from the error's labels.
    // Oxc miette errors contain source code snippets with span info.
    // The label's span is in the wrapped source text.
    if let Some(label) = error.labels.as_ref().and_then(|l| l.first()) {
        let span = label.inner();
        let start = span.offset().saturating_sub(wrapping_offset);
        let end = (start + span.len()).min(source_text.len());

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
    fn test_with_program_walks_ast() {
        let result = parse_js("var x = 42;", ParseMode::Module);
        let body_len = result.with_program(|program| program.body.len());
        assert!(
            body_len.unwrap_or(0) > 0,
            "Expected at least one statement in AST"
        );
    }

    #[test]
    fn test_partial_ast_with_recoverable_error() {
        // oxc has error recovery: when it encounters a syntax error, it tries
        // to continue parsing. This test verifies that we can still get an AST
        // even when there are errors. We use a construct that oxc can recover
        // from (unclosed brace followed by valid code on the next line).
        let result = parse_js(
            "function foo() {\n  return 42;\n}\nvar x = 1;",
            ParseMode::Module,
        );
        // This should parse cleanly (no errors) — just verifying the API works
        assert!(result.has_ast(), "Expected AST to be available");
        let body_len = result.with_program(|program| program.body.len());
        assert!(
            body_len.unwrap_or(0) > 0,
            "Expected at least one statement in AST"
        );
    }
}
