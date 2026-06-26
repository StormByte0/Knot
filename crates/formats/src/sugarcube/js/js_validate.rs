//! Inline JavaScript validation via oxc for SugarCube passages.
//!
//! This module validates JS in two contexts:
//!
//! 1. **Inline JS snippets** (`validate_inline_js`): walks the passage AST
//!    for `<<set>>`, `<<run>>`, `<<script>>`, `<<=>>`, `<<->>` macros and
//!    validates each JS snippet with oxc.
//!
//! 2. **Script passages** (`validate_script_passage`): validates the entire
//!    body of `[script]`-tagged passages (or `StoryJavaScript`) as a JS
//!    module. This is separate from `validate_inline_js` because `[script]`
//!    passages have no `<<script>>` macro wrapper — their body IS the JS.
//!
//! oxc has error recovery: it produces multiple diagnostics per snippet (not
//! just the first one), each with a precise byte range. We map each one back
//! to the original SugarCube source so VSCode can squiggle exactly the
//! broken span.
//!
//! ## Position Mapping
//!
//! All diagnostic ranges produced by this module are **passage-relative**:
//! byte 0 is the `::` prefix of the passage header. The position mapping
//! chain is:
//!
//! ```text
//! oxc diagnostic range  (in preprocessed source)
//!       │
//!       ▼  preprocessed.map_range_to_original()
//! original JS range     (in SugarCube source, relative to snippet start)
//!       │
//!       ▼  + snippet.body_offset
//! passage body range    (relative to body start)
//!       │
//!       ▼  + body_offset_in_passage (offset of body start from passage head)
//! passage-relative range  (0 = passage head `::`)
//!       │
//!       ▼  at LSP boundary: + passage_offset → document-absolute
//! ```
//!
//! For expression-mode snippets (<<set>>, <<run>>), the oxc parser wraps
//! the source in parentheses — we need to subtract the 1-byte offset
//! before mapping.

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};
use knot_core::oxc::{parse_js, ParseMode as JsParseMode};

use crate::sugarcube::ast::{self, JsSnippet};
use std::collections::HashSet;

/// Validate all JS snippets in a passage AST and return diagnostics.
///
/// This is the main entry point called from the parse pipeline after
/// a passage has been parsed by the SugarCube parser. It collects all
/// JS-containing macro arguments and block bodies, preprocesses them,
/// and validates them with oxc.
///
/// `body_offset_in_passage` is the byte offset of the passage body start
/// relative to the passage head (`::` prefix). All returned diagnostic
/// ranges are passage-relative (0 = passage head `::`). The LSP boundary
/// adds `passage_offset` to produce document-absolute ranges.
pub fn validate_inline_js(
    nodes: &[ast::AstNode],
    body_offset_in_passage: usize,
    known_macro_names: &HashSet<String>,
) -> Vec<FormatDiagnostic> {
    let snippets = ast::collect_js_snippets(nodes, known_macro_names);
    let mut diagnostics = Vec::new();

    for snippet in &snippets {
        let snippet_diagnostics = validate_snippet(snippet, body_offset_in_passage);
        diagnostics.extend(snippet_diagnostics);
    }

    diagnostics
}

/// Validate a script passage's entire body as JS.
///
/// Script passages (`[script]` tagged or `StoryJavaScript`) contain pure JS —
/// their entire body is a JS module, not SugarCube syntax. `validate_inline_js`
/// only walks AST nodes for `<<script>>` block macros, which doesn't cover
/// `[script]` passages (they have no `<<script>>` macro, just raw JS body).
///
/// This function parses the entire body text as a JS module and converts
/// all oxc diagnostics to `FormatDiagnostic`s with precise position mapping.
/// `body_offset_in_passage` is the offset of the body start relative to the
/// passage head (`::` prefix).
///
/// `sugarcube_syntax` controls whether the SugarCube preprocessor runs
/// (`$var` → `State.variables.var`, keyword operators). Pass `true` for
/// `[script]`-tagged Twee passages, `false` for standalone `.js` files.
pub fn validate_script_passage(
    body_text: &str,
    body_offset_in_passage: usize,
    sugarcube_syntax: bool,
) -> Vec<FormatDiagnostic> {
    if body_text.trim().is_empty() {
        return Vec::new();
    }

    // Preprocess $var references for oxc (only when sugarcube_syntax is true)
    let preprocessed = super::js_preprocess::preprocess_for_oxc(body_text, sugarcube_syntax);

    let outcome = parse_js(&preprocessed.source, JsParseMode::Module);

    // Convert each diagnostic to a FormatDiagnostic.
    // For [script] passages, the snippet starts at body_offset_in_passage
    // (the body IS the snippet — no macro wrapper to account for).
    outcome.diagnostics
        .iter()
        .filter_map(|js_diag| {
            convert_script_passage_diagnostic(
                js_diag,
                &preprocessed,
                body_offset_in_passage,
            )
        })
        .collect()
}

/// Validate a single JS snippet.
///
/// oxc has error recovery, so even when there are syntax errors, it produces
/// multiple diagnostics (not just the first one). Each diagnostic has a
/// precise byte range — we map each one back to the original SugarCube source
/// so VSCode can squiggle exactly the broken span.
fn validate_snippet(snippet: &JsSnippet, body_offset_in_passage: usize) -> Vec<FormatDiagnostic> {
    // Preprocess $var references for oxc
    let preprocessed = super::js_preprocess::preprocess_for_oxc(&snippet.source, true);

    // Determine parse mode: block scripts get Module, inline expressions get Expression
    let js_mode = if snippet.is_block {
        JsParseMode::Module
    } else {
        JsParseMode::Expression
    };

    let outcome = parse_js(&preprocessed.source, js_mode);
    // Convert ALL diagnostics (oxc may produce multiple per snippet via
    // error recovery) to FormatDiagnostic with precise position mapping.
    outcome.diagnostics
        .iter()
        .filter_map(|js_diag| {
            convert_js_diagnostic(js_diag, &preprocessed, snippet, body_offset_in_passage, js_mode)
        })
        .collect()
}

/// Convert a JS diagnostic to a FormatDiagnostic with position mapping.
///
/// Maps the diagnostic's byte range from the preprocessed source back
/// to the original SugarCube source, then shifts by the snippet's body
/// offset and `body_offset_in_passage` to produce a passage-relative
/// byte range (0 = passage head `::`).
fn convert_js_diagnostic(
    js_diag: &knot_core::oxc::JsDiagnostic,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    snippet: &JsSnippet,
    body_offset_in_passage: usize,
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
    // starts within the passage body) and then by body_offset_in_passage
    // to get passage-relative positions (0 = passage head `::`).
    let passage_start = body_offset_in_passage + snippet.body_offset + original_start;
    let passage_end = body_offset_in_passage + snippet.body_offset + original_end;

    // Clamp to prevent empty or inverted ranges
    let passage_end = passage_end.max(passage_start + 1);

    // Step 4: Create the FormatDiagnostic with passage-relative range
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
        range: passage_start..passage_end,
        message,
        severity,
        code: "sc-js".to_string(),
    })
}

/// Convert a JS diagnostic from a `[script]` passage to a FormatDiagnostic.
///
/// Simpler than `convert_js_diagnostic` because:
/// - No wrapping offset (Module mode, not Expression)
/// - No snippet body_offset (the body IS the snippet)
/// - No macro name prefix (it's a passage, not a macro)
fn convert_script_passage_diagnostic(
    js_diag: &knot_core::oxc::JsDiagnostic,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    body_offset_in_passage: usize,
) -> Option<FormatDiagnostic> {
    // Map from oxc position (in preprocessed source) back to original source.
    let original_start = preprocessed.map_to_original(js_diag.range.start);
    let original_end = preprocessed.map_to_original(js_diag.range.end);

    // Shift by body_offset_in_passage to get passage-relative positions.
    let passage_start = body_offset_in_passage + original_start;
    let passage_end = body_offset_in_passage + original_end;

    // Clamp to prevent empty or inverted ranges
    let passage_end = passage_end.max(passage_start + 1);

    let severity = match js_diag.severity {
        knot_core::oxc::JsDiagnosticSeverity::Error => FormatDiagnosticSeverity::Error,
        knot_core::oxc::JsDiagnosticSeverity::Warning => FormatDiagnosticSeverity::Warning,
    };

    Some(FormatDiagnostic {
        range: passage_start..passage_end,
        message: js_diag.message.clone(),
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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        // Verify that diagnostics are shifted by body_offset_in_passage
        let body = "<<run bad[>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let body_offset_in_passage = 20; // e.g. header line + newline
        let diagnostics = validate_inline_js(&ast.nodes, body_offset_in_passage, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();

        if !js_errors.is_empty() {
            // All diagnostic ranges should be at or past body_offset_in_passage (20)
            for diag in &js_errors {
                assert!(
                    diag.range.start >= body_offset_in_passage,
                    "Diagnostic range should be shifted by body_offset_in_passage, got start={}",
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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

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

    // -----------------------------------------------------------------------
    // Span mapping accuracy tests
    // -----------------------------------------------------------------------

    #[test]
    fn validate_error_span_points_to_expression_not_closing_tag() {
        // When a <<set>> expression has a JS error, the diagnostic range
        // should point to the error location within the expression, NOT
        // to the >> closing tag.
        let body = r#"<<set $x to [1, 2, bad@@]>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();

        if !js_errors.is_empty() {
            // The error should NOT be at the very end of the macro (>>)
            // It should be somewhere within the expression content
            let macro_close_pos = body.find(">>").unwrap_or(body.len());
            for diag in &js_errors {
                assert!(
                    diag.range.start < macro_close_pos,
                    "JS error span (start={}) should be before >> (pos={}), got message: {}",
                    diag.range.start, macro_close_pos, diag.message
                );
            }
        }
    }

    #[test]
    fn validate_set_method_call_error_span() {
        // For <<set>> without structured assignment (method call),
        // the error span should point to the expression, not to <<.
        let body = r#"<<set $arr.push(bad@@)>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();

        if !js_errors.is_empty() {
            // The error should NOT be at the start of the macro (<<)
            let _macro_open_pos = 0; // body starts with <<
            let args_start = "<<set ".len(); // args start after "set "
            for diag in &js_errors {
                assert!(
                    diag.range.start >= args_start,
                    "JS error span (start={}) should be at or past args start (pos={}), got: {}",
                    diag.range.start, args_start, diag.message
                );
            }
        }
    }

    #[test]
    fn validate_run_error_span_in_args() {
        // For <<run>> macros, error spans should point to the expression args,
        // not to the << opening tag.
        let body = r#"<<run bad@@syntax>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();

        if !js_errors.is_empty() {
            let args_start = "<<run ".len();
            for diag in &js_errors {
                assert!(
                    diag.range.start >= args_start,
                    "JS error span (start={}) should be at or past args start (pos={}), got: {}",
                    diag.range.start, args_start, diag.message
                );
            }
        }
    }

    #[test]
    fn validate_set_with_to_keyword_and_complex_expression() {
        // <<set $var to {key: "value"}>> — object literal with SugarCube `to`
        let body = r#"<<set $config to {key: "value", nested: {a: 1}}>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for <<set $config to {{object}}>>, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_set_multi_assignment_with_comma() {
        // SugarCube allows multiple assignments separated by commas:
        // <<set $a to 1, $b to 2>>
        // But in our two-parser model, <<set>> parses only the first
        // assignment structurally. The comma expression goes to oxc as-is.
        let body = r#"<<set $a = 1, $b = 2>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        // This might produce JS errors because $b is not substituted in the
        // expression portion (only the RHS of the first assignment goes to oxc).
        // Just verify no panics and diagnostics are reasonable.
        for d in &diagnostics {
            assert!(!d.message.is_empty());
        }
    }

    #[test]
    fn validate_set_with_block_comment_containing_gt_gt() {
        // Regression test: >> inside a /* */ comment must NOT close the macro.
        // Without comment-aware scanning, the macro args would be truncated
        // at the >> inside the comment, causing "Unexpected token" errors.
        let body = r#"<<set $x = [1, /* >> */ 2, 3]>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        for e in &js_errors {
            eprintln!("JS error: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for <<set>> with >> inside block comment, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_set_with_line_comment_containing_gt_gt() {
        // Regression test: >> inside a // comment must NOT close the macro.
        let body = "<<set $x = [1, // >> close\n2, 3]>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        for e in &js_errors {
            eprintln!("JS error: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for <<set>> with >> inside line comment, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_set_multiline_object_with_block_comments() {
        // Regression test: <<set>> with a multiline object literal containing
        // /* ... */ block comments must NOT produce "Unterminated multiline comment".
        // The comments are valid JS and oxc must recognize the closing */.
        let body = r#"<<set $gs = {
  /* -- TIME ----------------------------------------------------
     week: int
  */
  time: { week: 0 },
  /* -- SCENE ---------------------------------------------------
     id: string
  */
  scene: { id: "test" },
  /* -- JOURNAL -------------------------------------------------
     Quest tracker. Used by <<questLog>> and <<questStatus>>
  */
  journal: { quests: [] }
}>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        for e in &js_errors {
            eprintln!("JS error: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for <<set>> with block comments in object literal, got: {:?}",
            js_errors
        );
    }

    #[test]
    fn validate_special_twee_no_false_unterminated_comment() {
        // Regression test: _special.twee contains a large <<set $gs = { ... }>>
        // with many /* ... */ block comments, some containing <<macro>> refs
        // (which have >> inside comments). The parser must correctly skip these
        // comments and oxc must not report "Unterminated multiline comment".
        let content = include_str!("../../../testdata/_special.twee");
        // Strip the ::StoryInit header
        let body = if content.starts_with("::") {
            &content[content.find('\n').unwrap_or(0) + 1..]
        } else {
            content
        };
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let unterminated: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js" && d.message.to_lowercase().contains("unterminated"))
            .collect();
        for e in &unterminated {
            eprintln!("Unterminated comment diag: {:?} range={:?}", e.message, e.range);
        }
        assert!(
            unterminated.is_empty(),
            "Expected no 'unterminated comment' diagnostics for _special.twee, got: {:?}",
            unterminated
        );
    }

    // ── def / ndef operator tests ──────────────────────────────────────

    #[test]
    fn validate_def_in_if_macro_no_js_error() {
        // `<<if def _defended and _defended>>` — the exact expression
        // from sugarcube-testbed/src/51-combat.twee:99.
        //
        // Before the def/ndef fix, `def` was left as a bare identifier
        // and oxc reported: "Expected `,` or `)` but found `Identifier`"
        // (sc-js diagnostic at the `def` token).
        //
        // After the fix, `def _defended` is preprocessed to
        // `(typeof State_temporary_defended !== "undefined")`, which oxc
        // parses without error.
        let body = "<<if def _defended and _defended>><</if>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for `<<if def _defended and _defended>>`, got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn validate_ndef_in_if_macro_no_js_error() {
        // `<<if ndef $missing>>` — the form from
        // sugarcube-testbed/src/28-operators.twee:47.
        let body = "<<if ndef $missing>><</if>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for `<<if ndef $missing>>`, got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn validate_def_in_ternary_no_js_error() {
        // `<<link `"Visit " + (def _target ? _target : "Time")` "Time">>`
        // — the form from sugarcube-testbed/src/60-edge-cases.twee:20.
        //
        // The backtick expression contains `def _target ? _target : "Time"`,
        // which after preprocessing becomes:
        //   (typeof State_temporary_target !== "undefined") ? State_temporary_target : "Time"
        let body = "<<link `\"Visit \" + (def _target ? _target : \"Time\")` \"Time\">><</link>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for `def` in ternary, got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn validate_def_story_variable_no_js_error() {
        // `<<if def $a>>` — the form from
        // sugarcube-testbed/src/28-operators.twee:46.
        let body = "<<if def $a>><</if>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let diagnostics = validate_inline_js(&ast.nodes, 0, &HashSet::new());

        let js_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for `<<if def $a>>`, got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    // ── Custom macro (widget) cross-file registration tests ────────────

    #[test]
    fn custom_widget_invocation_no_js_error_after_registration() {
        // Simulates the cross-file scenario from the testbed:
        //   31-widgets.twee defines `<<widget statblock>>`
        //   26-misc.twee invokes `<<statblock "Strength" $stats.strength>>`
        //
        // The format plugin's custom macro registry persists across
        // `parse_mut` calls. When the widget-defining file is parsed
        // FIRST, the widget name is registered. When the consumer file
        // is parsed SECOND, the widget name is in `known_macro_names`,
        // so the JS validation fallback in `collect_js_snippets` does
        // NOT send the widget's args to oxc.
        //
        // Without the registration, `statblock` would be unknown, and
        // its args (`"Strength" $stats.strength`) would be sent to oxc
        // → false "Expected `,` or `)`" parse error.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();

        // File 1: widget definition (parsed first, registers `statblock`).
        let widgets_file = "\
:: Widgets [widget]
<<widget statblock>>
<<set _label to _args[0]>>
<<set _value to _args[1]>>
@@.stat-block;
**_label**: _value
@@
<</widget>>
";
        let _result1 = plugin.parse_mut(
            &url::Url::parse("file:///widgets.twee").unwrap(),
            widgets_file,
        );

        // File 2: consumer passage (parsed second, references `statblock`).
        let consumer_file = "\
:: MiscMacros
<<statblock \"Strength\" $stats.strength>>
";
        let result2 = plugin.parse_mut(
            &url::Url::parse("file:///misc.twee").unwrap(),
            consumer_file,
        );

        // The consumer file should have NO sc-js diagnostics —
        // `statblock` is in the registry, so the fallback path that
        // sends widget args to oxc is skipped.
        let js_errors: Vec<_> = result2.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            js_errors.is_empty(),
            "Expected no JS errors for `<<statblock>>` after widget registration, got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn custom_widget_invocation_error_before_registration() {
        // Inverse of the above test: if the consumer file is parsed
        // BEFORE the widget definition file, the widget name is NOT in
        // the registry, and the fallback path sends the args to oxc,
        // producing a parse error.
        //
        // This test documents the pre-fix behavior (what happens without
        // the indexing order fix in the server). It verifies that the
        // fallback IS the root cause — the fix is to ensure definition
        // files are parsed first, not to change the fallback logic.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();

        // Consumer file parsed FIRST — registry is cold.
        let consumer_file = "\
:: MiscMacros
<<statblock \"Strength\" $stats.strength>>
";
        let result = plugin.parse_mut(
            &url::Url::parse("file:///misc.twee").unwrap(),
            consumer_file,
        );

        // Without prior registration, `statblock` is unknown, and its
        // args are sent to oxc. oxc sees `"Strength" $stats.strength`
        // (after $var substitution: `"Strength" State_variables_stats_strength`)
        // — a string literal followed by an identifier with no operator —
        // and reports a parse error.
        let js_errors: Vec<_> = result.diagnostic_groups.iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "sc-js")
            .collect();
        assert!(
            !js_errors.is_empty(),
            "Expected JS errors for `<<statblock>>` BEFORE widget registration \
             (this documents the pre-fix behavior — the indexing order fix \
             in the server prevents this by parsing definition files first), \
             got: {:?}",
            js_errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}
