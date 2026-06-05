//! JavaScript syntax validation walk for the SugarCube passage tree.
//!
//! This module bridges the SugarCube passage tree with `knot_core::oxc` —
//! the format-agnostic JS parsing service in the core crate.
//!
//! ## Flow
//!
//! ```text
//! PassageNode tree
//!     │
//!     v
//! js_extractor::extract_snippets()
//!     │  Walks the tree for JS-bearing contexts
//!     │  (<<run>>, <<set>>, <<script>>, <<=>>, etc.)
//!     v
//! Vec<JsSnippet>
//!     │
//!     v  For each snippet:
//! js_preprocess::preprocess_sugarcube_js()
//!     │  Substitutes $var → State_variables_varName
//!     │  Substitutes operator aliases (to → =, eq → ===, etc.)
//!     v
//! knot_core::oxc::parse_js(preprocessed, parse_mode)
//!     │
//!     ├── JsParseOutcome::Error(diagnostics)
//!     │       Map positions back to original source
//!     │       Convert to FormatDiagnostic
//!     │
//!     └── JsParseOutcome::Success(JsParseOutput)
//!             Walk AST for SugarCube-specific analysis
//!             (Phase 3: variable read/write, function calls, etc.)
//!             For now: snippet is valid JS, no diagnostics
//! ```

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};
use crate::sugarcube::js_extractor::{JsContext, JsSnippet, extract_snippets};
use crate::sugarcube::js_preprocess::preprocess_sugarcube_js;
use crate::sugarcube::passage_tree::PassageNode;

use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode};

/// Walk the passage tree and validate JavaScript syntax in macro arguments,
/// script blocks, and inline expressions.
///
/// This is the entry point for SugarCube JS validation. It:
/// 1. Extracts JS snippets from the tree
/// 2. Pre-processes each snippet (SugarCube-specific $var and operator substitution)
/// 3. Parses with `knot_core::oxc::parse_js()`
/// 4. On error: maps positions back and converts to `FormatDiagnostic`
/// 5. On success: the AST is available for format-specific analysis (future)
pub(crate) fn walk_js_check(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<FormatDiagnostic> {
    let snippets = extract_snippets(nodes, body, body_offset);

    let mut diagnostics = Vec::new();
    for snippet in &snippets {
        let snippet_diags = validate_snippet(snippet);
        diagnostics.extend(snippet_diags);
    }

    diagnostics
}

/// Validate a single JS snippet using the full pipeline:
/// pre-process → parse → handle result.
fn validate_snippet(snippet: &JsSnippet) -> Vec<FormatDiagnostic> {
    // Determine the parse mode from the snippet context
    let parse_mode = match snippet.context {
        JsContext::ScriptPassage => ParseMode::Module,
        JsContext::MacroExpression | JsContext::MacroJsBlock => ParseMode::Expression,
        JsContext::InlineBlock => ParseMode::StatementList,
    };

    // Pre-process: $var substitution + operator alias substitution
    let preprocessed = preprocess_sugarcube_js(&snippet.source, snippet.offset);

    // Parse with Oxc via the core parsing service
    match parse_js(&preprocessed.js_source, parse_mode) {
        JsParseOutcome::Success(_output) => {
            // Parsing succeeded. The AST is available in `_output`.
            // Future: walk the AST for SugarCube-specific analysis:
            // - Find State_variables_* identifiers (corresponding to $var)
            // - Determine variable read/write from assignment context
            // - Find function calls for completion
            // For now: the snippet is valid JS, no diagnostics.
            Vec::new()
        }
        JsParseOutcome::Error(js_diagnostics) => {
            // Map diagnostics back to original SugarCube positions
            js_diagnostics
                .into_iter()
                .map(|js_diag| {
                    // Map the diagnostic range from the pre-processed source
                    // back to the original document coordinates
                    let original_range = preprocessed.offset_mapper.map_range_back(&js_diag.range);

                    FormatDiagnostic {
                        range: original_range,
                        message: format!("JS: {}", js_diag.message),
                        severity: FormatDiagnosticSeverity::Error,
                        code: "sc-js-syntax".into(),
                    }
                })
                .collect()
        }
    }
}
