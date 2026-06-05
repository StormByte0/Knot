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
/// - [`JsParseOutcome::Success`]: The source parsed without syntax errors.
///   The AST (`Program`) is available for the format to walk and extract
///   format-specific information (variable references, function calls, etc.)
///
/// - [`JsParseOutcome::Error`]: Syntax errors were found. The format should
///   convert these diagnostics to `FormatDiagnostic` instances.
///
/// ## Lifetime
///
/// The `Program` in the `Success` variant borrows from the `Allocator`.
/// Both are bundled in a `JsParseOutput` struct that owns the allocator
/// and provides a `with_program()` method for safe AST access.
pub enum JsParseOutcome {
    /// Parsing succeeded. The AST is available for analysis.
    Success(JsParseOutput),

    /// Parsing failed. Syntax diagnostics describe the errors.
    Error(Vec<JsDiagnostic>),
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
    ///
    /// ## Example
    ///
    /// ```ignore
    /// let output: JsParseOutput = /* ... */;
    /// let var_names: Vec<String> = output.with_program(|program| {
    ///     // Walk the AST and collect identifier names
    ///     // ...
    ///     vec!["x".to_string()]
    /// });
    /// ```
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
