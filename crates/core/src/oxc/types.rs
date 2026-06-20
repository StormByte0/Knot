//! Types for the Oxc JS parsing service.

use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;

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
/// Positions are in the source text that was passed to `parse_js()`
/// (after any format-specific pre-processing like `$var` substitution).
/// The **caller** (format) is responsible for mapping these positions back
/// to the original document coordinates, since only the format knows what
/// pre-processing transformations were applied.
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
/// Oxc has built-in error recovery: when it encounters a recoverable syntax
/// error, it records the error and continues parsing, producing a partial AST.
/// This means we can almost always walk the AST for token highlighting even
/// when the source has syntax errors — only the broken parts are missing.
///
/// ## Design
///
/// This is a **struct** (not an enum) because both the AST and the diagnostics
/// are always present (though the AST may be empty if the parser panicked on
/// an unrecoverable error). Callers should:
///
/// 1. Call [`with_program()`] to walk the AST for token highlighting — this
///    works even when `diagnostics` is non-empty, producing partial highlighting.
/// 2. Check `diagnostics` separately for error reporting — each diagnostic
///    has a precise byte range so VSCode can squiggle exactly the broken span.
///
/// ## Lifetime
///
/// The `Program` borrows from the `Allocator`. Both are bundled in a
/// `JsParseOutput` struct that owns the allocator and provides a
/// `with_program()` method for safe AST access.
///
/// [`with_program()`]: JsParseOutcome::with_program
pub struct JsParseOutcome {
    /// The parsed AST (may be partial if there were recoverable errors).
    /// Will be empty only if the parser panicked on an unrecoverable error.
    output: Option<JsParseOutput>,
    /// Syntax diagnostics. Empty if parsing succeeded. Non-empty if there
    /// were recoverable errors (AST is still available via `output`) or
    /// unrecoverable errors (AST is empty, `output` is `None`).
    pub diagnostics: Vec<JsDiagnostic>,
    /// Whether the parser panicked (could not recover). When `true`, the
    /// AST is empty and only early diagnostics are available.
    pub panicked: bool,
}

impl JsParseOutcome {
    /// Create a successful outcome (no errors, AST available).
    pub(crate) fn success(output: JsParseOutput) -> Self {
        Self {
            output: Some(output),
            diagnostics: Vec::new(),
            panicked: false,
        }
    }

    /// Create a partial outcome (errors present, but AST is still available
    /// for walking). This is the common case — oxc recovered from errors.
    pub(crate) fn partial(output: JsParseOutput, diagnostics: Vec<JsDiagnostic>) -> Self {
        Self {
            output: Some(output),
            diagnostics,
            panicked: false,
        }
    }

    /// Create a failed outcome (parser panicked, no AST available).
    pub(crate) fn failed(diagnostics: Vec<JsDiagnostic>) -> Self {
        Self {
            output: None,
            diagnostics,
            panicked: true,
        }
    }

    /// Returns `true` if the AST is available for walking (even if there
    /// were recoverable errors).
    pub fn has_ast(&self) -> bool {
        self.output.is_some()
    }

    /// Returns `true` if parsing had no errors at all.
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Access the parsed AST within a closure.
    ///
    /// The closure receives a reference to the `Program` AST, which borrows
    /// from the internal allocator. If the parser panicked and no AST is
    /// available, the closure is NOT called and this returns `None`.
    ///
    /// This works even when `diagnostics` is non-empty — oxc's error recovery
    /// produces a partial AST that can be walked for token highlighting.
    pub fn with_program<F, R>(&self, visitor: F) -> Option<R>
    where
        F: FnOnce(&Program<'_>) -> R,
    {
        let output = self.output.as_ref()?;
        Some(output.with_program(visitor))
    }
}

/// Owns the Oxc allocator and the parsed program.
///
/// The `Program` AST references memory in the `Allocator`, so both must
/// be kept together. Use [`with_program()`] to access the AST within a
/// closure where the lifetime is properly scoped.
///
/// [`with_program()`]: JsParseOutput::with_program
pub struct JsParseOutput {
    /// The allocator that owns the AST's memory. Must live as long as the
    /// program references are held.
    allocator: Allocator,
    /// The source text that was parsed (after any wrapping for expressions).
    /// Must live as long as the program references are held.
    source_text: String,
}

impl JsParseOutput {
    /// Create a new `JsParseOutput` by parsing the given source.
    ///
    /// This is called internally by `parse_js()`. The allocator is used
    /// to parse the source, and both are stored together.
    pub(crate) fn new(allocator: Allocator, source_text: String) -> Self {
        Self {
            allocator,
            source_text,
        }
    }

    /// Access the parsed AST within a closure.
    ///
    /// The closure receives a reference to the `Program` AST, which borrows
    /// from the internal allocator. The closure can walk the AST, extract
    /// information, and return an owned result. The `Program` reference
    /// must NOT escape the closure.
    pub fn with_program<F, R>(&self, visitor: F) -> R
    where
        F: FnOnce(&Program<'_>) -> R,
    {
        let source_type = oxc_span::SourceType::default();
        let parser = oxc_parser::Parser::new(&self.allocator, &self.source_text, source_type);
        let result = parser.parse();
        visitor(&result.program)
    }

    /// Access the allocator and source text for custom parsing.
    ///
    /// For advanced use cases where the format needs to re-parse or
    /// inspect the raw parse result (including any non-fatal warnings).
    pub fn with_raw_parse<F, R>(&self, visitor: F) -> R
    where
        F: FnOnce(oxc_parser::ParserReturn<'_>) -> R,
    {
        let source_type = oxc_span::SourceType::default();
        let parser = oxc_parser::Parser::new(&self.allocator, &self.source_text, source_type);
        let result = parser.parse();
        visitor(result)
    }
}
