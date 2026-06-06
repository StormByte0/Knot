//! Inline JavaScript validation via oxc for SugarCube passages.
//!
//! This module validates JS snippets extracted from SugarCube AST nodes
//! (<<set>>, <<run>>, <<script>>, <<=>>, <<->>) by:
//!
//! 1. Collecting JS snippets from the passage AST via `collect_js_snippets()`
//! 2. Preprocessing each snippet with `js_preprocess::preprocess_for_oxc()`
//! 3. Parsing with `knot_core::oxc::parse_js()`
//! 4. On `JsParseOutcome::Error`, converting JS diagnostics to `FormatDiagnostic`
//!    with byte-offset mapping through the preprocessor's substitution table
//!
//! ## Position Mapping
//!
//! The position mapping chain is:
//!
//! ```text
//! oxc diagnostic range  (in preprocessed source)
//!       │
//!       ▼  preprocessed.map_range_to_original()
//! original JS range     (in SugarCube source, relative to snippet start)
//!       │
//!       ▼  + snippet.body_offset
//! passage body range    (relative to body start in the document)
//!       │
//!       ▼  + body_offset (passed from parse() pipeline)
//! document range        (absolute byte range for LSP)
//! ```
//!
//! For expression-mode snippets (<<set>>, <<run>>), the oxc parser wraps
//! the source in parentheses — we need to subtract the 1-byte offset
//! before mapping.

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};
use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};

use super::ast::{self, JsSnippet};

/// Validate all JS snippets in a passage AST and return diagnostics.
///
/// This is the main entry point called from the parse pipeline after
/// a passage has been parsed by the SugarCube parser. It collects all
/// JS-containing macro arguments and block bodies, preprocesses them,
/// and validates them with oxc.
///
/// `body_offset` is the byte offset of the passage body start within
/// the document. All returned diagnostic ranges are document-absolute.
pub fn validate_inline_js(
    nodes: &[ast::AstNode],
    body_offset: usize,
) -> Vec<FormatDiagnostic> {
    let snippets = ast::collect_js_snippets(nodes);
    let mut diagnostics = Vec::new();

    for snippet in &snippets {
        let snippet_diagnostics = validate_snippet(snippet, body_offset);
        diagnostics.extend(snippet_diagnostics);
    }

    diagnostics
}

/// Validate a single JS snippet.
fn validate_snippet(snippet: &JsSnippet, body_offset: usize) -> Vec<FormatDiagnostic> {
    // Preprocess $var references for oxc
    let preprocessed = super::js_preprocess::preprocess_for_oxc(&snippet.source);

    // Determine parse mode: block scripts get Module, inline expressions get Expression
    let js_mode = if snippet.is_block {
        JsParseMode::Module
    } else {
        JsParseMode::Expression
    };

    match parse_js(&preprocessed.source, js_mode) {
        JsParseOutcome::Success(_) => {
            // JS is valid — no diagnostics
            Vec::new()
        }
        JsParseOutcome::Error(js_diagnostics) => {
            // Convert JS diagnostics to format diagnostics with position mapping
            js_diagnostics
                .into_iter()
                .filter_map(|js_diag| {
                    convert_js_diagnostic(&js_diag, &preprocessed, snippet, body_offset, js_mode)
                })
                .collect()
        }
    }
}

/// Convert a JS diagnostic to a FormatDiagnostic with position mapping.
///
/// Maps the diagnostic's byte range from the preprocessed source back
/// to the original SugarCube source, then shifts by the snippet's body
/// offset and the passage's body offset to produce a document-absolute
/// byte range.
fn convert_js_diagnostic(
    js_diag: &knot_core::oxc::JsDiagnostic,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    snippet: &JsSnippet,
    body_offset: usize,
    js_mode: JsParseMode,
) -> Option<FormatDiagnostic> {
    // Step 1: Unwrap the expression-mode parenthesization offset.
    // When oxc parses in Expression mode, the source is wrapped as `(source)`.
    // The oxc parser's error positions are in the wrapped source, so we need
    // to subtract 1 from start positions (the opening paren) to get back
    // to positions in the preprocessed source.
    let wrapping_offset: usize = match js_mode {
        JsParseMode::Expression => 1,
        _ => 0,
    };

    // Step 2: Map from oxc position (in preprocessed source) back to
    // the original SugarCube source position (relative to snippet start).
    let adjusted_start = js_diag.range.start.saturating_sub(wrapping_offset);
    let adjusted_end = js_diag.range.end.saturating_sub(wrapping_offset);

    let original_start = preprocessed.map_to_original(adjusted_start);
    let original_end = preprocessed.map_to_original(adjusted_end);

    // Step 3: Shift by the snippet's body offset (where this snippet
    // starts within the passage body) and then by the passage's
    // body_offset to get document-absolute positions.
    let doc_start = body_offset + snippet.body_offset + original_start;
    let doc_end = body_offset + snippet.body_offset + original_end;

    // Clamp to prevent empty or inverted ranges
    let doc_end = doc_end.max(doc_start + 1);

    // Step 4: Create the FormatDiagnostic
    let severity = match js_diag.severity {
        knot_core::oxc::JsDiagnosticSeverity::Error => FormatDiagnosticSeverity::Error,
        knot_core::oxc::JsDiagnosticSeverity::Warning => FormatDiagnosticSeverity::Warning,
    };

    // Prefix the message with the macro name for context
    let message = if snippet.macro_name == "=" || snippet.macro_name == "-" {
        format!("In <<{}>> expression: {}", snippet.macro_name, js_diag.message)
    } else {
        format!("In <<{}>> macro: {}", snippet.macro_name, js_diag.message)
    };

    Some(FormatDiagnostic {
        range: doc_start..doc_end,
        message,
        severity,
        code: "sc-js".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser;

    #[test]
    fn validate_valid_set_macro() {
        // <<set $hp to 100>> — valid JS after preprocessing
        let body = "<<set $hp to 100>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        // Should produce no diagnostics — the JS expression is valid
        // after $hp → State_variables_hp and to → =
        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for valid <<set>> macro, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_invalid_js_in_run_macro() {
        // <<run function(>> — invalid JS (unclosed paren)
        let body = "<<run function(>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        // Should produce at least one JS diagnostic
        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            !js_errors.is_empty(),
            "Expected JS errors for invalid <<run>> macro"
        );
    }

    #[test]
    fn validate_valid_print_expression() {
        // <<print $gold>> — valid expression after preprocessing
        let body = "<<print $gold>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for valid <<print>> expression, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_body_offset_shifts_ranges() {
        // Verify that diagnostics are shifted by body_offset
        let body = "<<run bad[>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 100);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();

        if !js_errors.is_empty() {
            // All diagnostic ranges should be at or past body_offset (100)
            for diag in &js_errors {
                assert!(
                    diag.range.start >= 100,
                    "Diagnostic range should be shifted by body_offset, got start={}",
                    diag.range.start
                );
            }
        }
    }

    #[test]
    fn validate_empty_snippet_no_diagnostics() {
        // <<set >> with no expression — should produce no JS diagnostics
        // (empty args are not collected as snippets)
        let body = "<<set >>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for empty <<set>> macro, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_multiple_snippets_in_passage() {
        // Multiple JS-containing macros in one passage
        let body = "<<set $x to 1>><<run Math.sqrt(4)>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for valid multi-macro passage, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_multiline_set_with_comments() {
        // Realistic multi-line <<set>> with C-style comments
        let body = r#"<<set $UI_PROFILES = [
  /* comment */
  {
    id: "base"
  }
]>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        for e in &js_errors {
            eprintln!("JS error: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for multi-line <<set>> with comments, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_full_ui_profiles_set() {
        // Full realistic <<set>> from user's project: multi-line array with
        // nested objects, C-style comments, and complex property paths.
        let body = r#"<<set $UI_PROFILES = [

  /* -- PROFILE 0: base (pre-employment) -------- */
  {
    id:      "base",
    extends: null,
    left: [
      {
        title:  null,
        showIf: null,
        rows: [
          [
            { t: "string", label: "Date",  v: "dateLabel",  fmt: null, showIf: null },
            { t: "string", label: "Time",  v: "timeLabel",  fmt: null, showIf: null }
          ],
          [
            { t: "string", label: "Shift", v: "shiftLabel", fmt: null, showIf: "meta.employed" }
          ]
        ]
      },
      {
        title:  null,
        showIf: null,
        rows: [
          [ { t: "progress", label: "Stress",  v: "stats.stress",  min: 0, max: 100, style: "danger", showIf: null } ],
          [ { t: "progress", label: "Stamina", v: "stats.stamina", min: 0, max: 100, style: "good",   showIf: null } ]
        ]
      },
      {
        title:  null,
        showIf: null,
        rows: [
          [
            { t: "number", label: "Balance", v: "finance.balance", fmt: "money",   showIf: null },
            { t: "number", label: "Debt",    v: "finance.debt",    fmt: "money",   showIf: null }
          ],
          [
            { t: "steps", label: "Warnings", v: "status.strikes", max: 3, style: "danger", showIf: null }
          ]
        ]
      }
    ]
  },

  /* -- PROFILE 1: employed ---- */
  {
    id:      "employed",
    extends: 0,
    left: [
      {
        title:  null,
        showIf: null,
        rows: [
          [
            { t: "string", label: "Date",  v: "dateLabel",  fmt: null, showIf: null },
            { t: "string", label: "Time",  v: "timeLabel",  fmt: null, showIf: null }
          ],
          [
            { t: "string", label: "Shift", v: "shiftLabel", fmt: null, showIf: null }
          ]
        ]
      }
    ]
  }

]>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0);

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        for e in &js_errors {
            eprintln!("JS error: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for full <<set $UI_PROFILES>>, got: {:?}",
            js_errors
        );
    }
}
