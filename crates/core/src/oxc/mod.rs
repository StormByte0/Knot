//! Oxc-based JavaScript parsing for Knot.
//!
//! This module provides the **JS side** of the two-parser model. It is a pure
//! parsing service: it takes JavaScript source text and a parse mode, and
//! returns either syntax diagnostics (on error) or the parsed AST (on success).
//!
//! ## Design
//!
//! This module knows **nothing** about SugarCube, Harlowe, or any specific
//! story format. It is a format-agnostic JS parsing utility. Each format:
//!
//! 1. Extracts JS snippets from its parsed content (e.g. SugarCube walks the
//!    `PassageNode` tree for `<<run>>`, `<<set>>`, `<<script>>` blocks)
//! 2. Pre-processes the snippets (e.g. SugarCube substitutes `$var` with
//!    `State_variables_varName` so Oxc sees valid JS identifiers)
//! 3. Calls [`parse_js()`] with the pre-processed source
//! 4. Handles the result:
//!    - Walk the AST via `outcome.with_program()` for token highlighting
//!      (works even with recoverable errors — oxc produces a partial AST)
//!    - Check `outcome.diagnostics` for error reporting (precise squiggles)
//!
//! ## Why this lives in `knot-core` (not `knot-formats`)
//!
//! - JS parsing is a **language infrastructure** concern, not a format concern.
//!   Multiple formats (SugarCube, Harlowe, Snowman) will all use the same
//!   Oxc parser — just with different extraction and pre-processing.
//! - The dependency flow stays clean: `knot-formats → knot-core → oxc_*`.
//! - The `knot-core` crate already houses analysis infrastructure like
//!   `AnalysisEngine` and `FormatVariableDiagnostic`; JS parsing fits
//!   alongside them.

pub mod parser;
pub mod types;

pub use types::{JsDiagnostic, JsDiagnosticSeverity, ParseMode, JsParseOutcome};
pub use parser::parse_js;

// Re-export oxc types that formats need for AST walking.
// This way formats only depend on knot-core, not on oxc crates directly.
pub use oxc_allocator::Allocator;
pub use oxc_ast::ast::Program;
