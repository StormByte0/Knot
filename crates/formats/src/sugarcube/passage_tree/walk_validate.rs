//! Diagnostics walks for the passage tree.
//!
//! Contains `walk_validate()` and its inner recursive walk that produces
//! syntax/semantic diagnostics from the tree structure.

use super::PassageNode;

// ---------------------------------------------------------------------------
// walk_validate() — Tree-based diagnostics (replaces validation::validate)
// ---------------------------------------------------------------------------

/// Walk the tree and produce syntax/semantic diagnostics.
///
/// Replaces the three separate passes in `validation::validate()`:
/// 1. `validate_macro_brackets()` — unclosed `<<` / `>>`
/// 2. `validate_link_brackets()` — unclosed `[[` / `]]`
/// 3. `validate_macro_structure()` — structural + unknown + deprecated checks
///
/// The tree already contains the structural information, so this walk:
/// - Reports `Error` nodes as unclosed/malformed constructs
/// - Reports unknown macros (not in `known_macro_names()`)
/// - Reports deprecated macros (in `deprecated_macros()`)
/// - Reports structural constraint violations (modifier macros outside
///   their required parent block)
/// - Reports unclosed block macros (Macro nodes with `close_span = None`)
pub(crate) fn walk_validate(
    nodes: &[PassageNode],
    body_offset: usize,
) -> Vec<crate::plugin::FormatDiagnostic> {
    let constraints = super::super::macros::structural_constraints();
    let deprecated = super::super::macros::deprecated_macros();
    let known_macros = super::super::macros::known_macro_names();

    let mut diagnostics = Vec::new();
    walk_validate_inner(
        nodes,
        body_offset,
        &constraints,
        &deprecated,
        &known_macros,
        &Vec::new(), // parent stack at root level
        &mut diagnostics,
    );
    diagnostics
}

fn walk_validate_inner(
    nodes: &[PassageNode],
    body_offset: usize,
    constraints: &std::collections::HashMap<&str, std::collections::HashSet<&str>>,
    deprecated: &std::collections::HashMap<&str, &str>,
    known_macros: &std::collections::HashSet<&str>,
    parent_stack: &[String], // names of currently-open block macros
    diagnostics: &mut Vec<crate::plugin::FormatDiagnostic>,
) {
    use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};

    for node in nodes {
        match node {
            PassageNode::Error { message, span } => {
                diagnostics.push(FormatDiagnostic {
                    range: span.start..span.end,
                    message: message.clone(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-parse-error".into(),
                });
            }

            PassageNode::Macro {
                parsed,
                children,
                close_span,
                span,
                ..
            } => {
                let macro_name = parsed.name.as_str();
                let is_block = children.is_some();
                let doc_start = span.start;
                let doc_end = span.start + (parsed.end - parsed.start);

                // ── Deprecated macro warning ──────────────────────────
                if let Some(msg) = deprecated.get(macro_name) {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!("Deprecated macro: {}", msg),
                        severity: FormatDiagnosticSeverity::Info,
                        code: "sc-deprecated-macro".into(),
                    });
                }

                // ── Unknown macro hint ────────────────────────────────
                if !known_macros.contains(macro_name) {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!("Unknown SugarCube macro `<<{}>>`", macro_name),
                        severity: FormatDiagnosticSeverity::Hint,
                        code: "sc-unknown-macro".into(),
                    });
                }

                // ── Structural constraint check ──────────────────────
                // Modifier macros (else, elseif, case, default) must be
                // inside their parent block.
                if let Some(valid_parents) = constraints.get(macro_name) {
                    let has_valid_parent = parent_stack.iter().rev().any(|p| {
                        valid_parents.contains(p.as_str())
                    });
                    if !has_valid_parent {
                        let parent_list: Vec<String> = valid_parents
                            .iter()
                            .map(|p| format!("`<<{}>>`", p))
                            .collect();
                        diagnostics.push(FormatDiagnostic {
                            range: doc_start..doc_end,
                            message: format!(
                                "`<<{}>>` must be inside {}",
                                macro_name,
                                parent_list.join(" or ")
                            ),
                            severity: FormatDiagnosticSeverity::Error,
                            code: "sc-container-structure".into(),
                        });
                    }
                }

                // ── Unclosed block macro warning ──────────────────────
                if is_block && close_span.is_none() {
                    diagnostics.push(FormatDiagnostic {
                        range: doc_start..doc_end,
                        message: format!(
                            "Unclosed block macro `<<{}>>` — missing `<</{}>>`",
                            macro_name, macro_name
                        ),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sc-unclosed-block".into(),
                    });
                }

                // ── Recurse into children ─────────────────────────────
                if let Some(children) = children {
                    let mut new_stack = parent_stack.to_vec();
                    if super::super::macros::is_block_macro(macro_name) {
                        new_stack.push(macro_name.to_string());
                    }
                    walk_validate_inner(
                        children,
                        body_offset,
                        constraints,
                        deprecated,
                        known_macros,
                        &new_stack,
                        diagnostics,
                    );
                }
            }

            PassageNode::Text { .. } | PassageNode::Expression { .. } | PassageNode::Heading { .. } => {
                // No diagnostics for text, expression, or heading nodes
            }
        }
    }
}
