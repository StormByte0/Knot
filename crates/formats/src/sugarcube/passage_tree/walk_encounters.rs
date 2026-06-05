//! Variable encounter collection from the passage tree.
//!
//! Contains `walk_encounters()`, which walks the passage tree and collects
//! `VarEncounter` entries for all stateful macros. This is the sole source
//! of variable-tracking data for the `VariableTree` side table.
//!
//! ## Phase A: Selective walk
//!
//! Only **stateful macros** (ones that read/write `State.variables`) produce
//! `VarEncounter` entries. Navigation-only, DOM, audio, and timing macros
//! are skipped. Nav-shell macros (like `<<link>>`) skip their shell but
//! still recurse into children so that stateful children (e.g., `<<set>>`)
//! are recorded.
//!
//! ## Comment awareness
//!
//! Macro nodes whose spans overlap with comment spans (`/* ... */`,
//! `/% ... %/`, `<!-- ... -->`) are completely skipped. This prevents
//! macros inside block comments from being treated as variable references.

use super::PassageNode;
use crate::types::MacroCategory;

// ---------------------------------------------------------------------------
// VarEncounter, VarTypeHint, VarAccessKind
// ---------------------------------------------------------------------------

/// Type hint inferred from how a variable is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VarTypeHint {
    Number,
    String,
    Boolean,
    Array,
    Object,
    Unknown,
}

/// Whether a variable encounter is a read or write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VarAccessKind {
    Read,
    Write,
}

/// A variable encounter recorded during tree walking.
///
/// When a stateful macro reads or writes a `$var`, we record this encounter
/// so that downstream consumers can build a variable registry, infer types,
/// and track read/write locations.
#[derive(Debug, Clone)]
pub(crate) struct VarEncounter {
    /// The variable name without the `$` sigil (e.g., "gold" for `$gold`).
    pub name: String,
    /// Inferred type hint (Number for numeric literals, String for quoted, etc.)
    pub type_hint: VarTypeHint,
    /// Whether this is a read or write.
    pub kind: VarAccessKind,
    /// Source line within the passage body (0-based).
    pub line: u32,
    /// Byte span in the source document.
    #[allow(dead_code)] // Used by Phase C for diagnostic routing
    pub byte_span: std::ops::Range<usize>,
}

// ---------------------------------------------------------------------------
// Macro classification
// ---------------------------------------------------------------------------

/// Classification of a macro for walk selectivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacroSelectivity {
    /// Stateful macro: collect var encounters (reads/writes State.variables).
    Stateful,
    /// Navigation shell: skip the shell, but recurse into children
    /// because they may contain stateful macros.
    NavShell,
    /// Completely skip: no var encounters, no recursion into children.
    Skip,
}

/// Classify a macro by its category for walk selectivity.
///
/// Uses the `MacroDef` catalog's `MacroCategory` to determine whether
/// a macro should produce var encounters, be skipped, or treated as a
/// nav-shell.
fn classify_macro(name: &str, builtin_lookup: &std::collections::HashMap<&'static str, &'static crate::types::MacroDef>, callable_names: &std::collections::HashSet<&str>) -> MacroSelectivity {
    // Special cases for expression macros (<<=>> and <<->>)
    if name == "=" || name == "-" {
        return MacroSelectivity::Stateful;
    }

    // Handle "when" pseudo-macro
    if name == "when" {
        return MacroSelectivity::Stateful;
    }

    // User-defined callables (widget invocations / custom macros) are stateful
    if callable_names.contains(name) {
        return MacroSelectivity::Stateful;
    }

    // Look up the macro in the builtin catalog
    if let Some(mdef) = builtin_lookup.get(name) {
        return match mdef.category {
            // Stateful: collect var encounters
            MacroCategory::Control => MacroSelectivity::Stateful,
            MacroCategory::Variables => MacroSelectivity::Stateful,
            MacroCategory::Output => {
                // "type" macro is NOT stateful — it's a typewriter effect
                if name == "type" {
                    MacroSelectivity::Skip
                } else {
                    MacroSelectivity::Stateful
                }
            }
            MacroCategory::Forms => MacroSelectivity::Stateful,
            MacroCategory::Widgets => MacroSelectivity::Stateful,

            // Nav shell: skip the shell, recurse into children
            MacroCategory::Links => MacroSelectivity::NavShell,
            MacroCategory::Navigation => {
                // Navigation macros are inline (no children), skip entirely
                MacroSelectivity::Skip
            }

            // Completely skip
            MacroCategory::Dom => {
                // "script" is in Dom category but is stateful
                if name == "script" {
                    MacroSelectivity::Stateful
                } else {
                    MacroSelectivity::Skip
                }
            }
            MacroCategory::Timing => {
                // timed/repeat are nav-shell-like: skip the shell but
                // children may be stateful
                if name == "stop" {
                    MacroSelectivity::Skip
                } else {
                    MacroSelectivity::NavShell
                }
            }
            MacroCategory::Audio => MacroSelectivity::Skip,
        };
    }

    // Unknown macros default to Stateful for safety — they might be
    // widget invocations or custom macros we haven't detected.
    MacroSelectivity::Stateful
}

// ---------------------------------------------------------------------------
// Comment span overlap check
// ---------------------------------------------------------------------------

/// Check whether a node's span overlaps with any comment span.
///
/// Uses body-relative spans for both. The `comment_spans` are body-relative
/// (as returned by `find_all_comment_spans`), and `node_span` is
/// document-absolute (as stored in PassageNode). We convert `node_span`
/// to body-relative by subtracting `body_offset`.
fn is_in_comment(comment_spans: &[std::ops::Range<usize>], node_span: &std::ops::Range<usize>) -> bool {
    // Quick exit: if no comment spans, nothing is in a comment
    if comment_spans.is_empty() {
        return false;
    }
    comment_spans.iter().any(|cs| {
        node_span.start < cs.end && cs.start < node_span.end
    })
}

// ---------------------------------------------------------------------------
// walk_encounters()
// ---------------------------------------------------------------------------

/// Walk the tree and collect `VarEncounter` entries for all stateful macros.
///
/// ## Selective walk
///
/// Only stateful macros (ones that read/write `State.variables`) produce
/// var encounters. Navigation-only, DOM, audio, and timing macros are
/// skipped. Nav-shell macros (like `<<link>>`) skip their shell but
/// recurse into children so that stateful children are recorded.
///
/// ## Comment awareness
///
/// Macro nodes whose spans overlap with comment spans are completely
/// skipped, preventing macros inside `/* ... */` block comments from
/// being treated as variable references.
///
/// ## Parameters
///
/// - `nodes`: The passage tree from `parse_passage_body()`
/// - `body`: The original passage body text (needed for line number computation)
/// - `body_offset`: The byte offset of the body within the source document
/// - `callable_names`: Set of user-defined callable names (custom macros + widgets)
/// - `comment_spans`: Body-relative byte ranges of comments to skip
pub(crate) fn walk_encounters(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    callable_names: &std::collections::HashSet<&str>,
    comment_spans: &[std::ops::Range<usize>],
) -> Vec<VarEncounter> {
    // Build the builtin macro lookup from the catalog
    let builtin_lookup: std::collections::HashMap<&'static str, &'static crate::types::MacroDef> =
        crate::sugarcube::macros::builtin_macros()
            .iter()
            .map(|m| (m.name, m))
            .collect();

    let mut var_encounters = Vec::new();

    walk_encounters_inner(
        nodes, body, body_offset, &builtin_lookup, callable_names,
        &mut var_encounters, comment_spans,
    );

    var_encounters
}

/// Inner recursive walk for `walk_encounters()`.
///
/// Collects `VarEncounter` entries from stateful macros. Nav-shell macros
/// recurse into children without recording encounters for the shell itself.
/// Completely-skipped macros produce no output and do not recurse.
/// Macro nodes inside comment spans are completely skipped.
fn walk_encounters_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    builtin_lookup: &std::collections::HashMap<&'static str, &'static crate::types::MacroDef>,
    callable_names: &std::collections::HashSet<&str>,
    var_encounters: &mut Vec<VarEncounter>,
    comment_spans: &[std::ops::Range<usize>],
) {
    for node in nodes {
        // ── Comment check: skip any node whose span overlaps a comment ──
        // This prevents macros inside /* ... */ from being processed.
        let node_span = match node {
            PassageNode::Text { span, .. } => span,
            PassageNode::Macro { span, .. } => span,
            PassageNode::Expression { span, .. } => span,
            PassageNode::Heading { span, .. } => span,
            PassageNode::Error { span, .. } => span,
        };
        if is_in_comment(comment_spans, node_span) {
            continue;
        }

        match node {
            PassageNode::Text { var_refs, span, .. } => {
                // Text nodes inside block macros (if/else/for/etc.) may
                // contain variable references that affect state. We record
                // these as "read" encounters.
                //
                // At the top level (inside the function body but not inside
                // any block macro), text between macros is just rendered
                // HTML — but we still record var refs for completeness.
                let source_line = line_from_span(span.start, body, body_offset);

                for vr in var_refs {
                    if vr.is_temporary {
                        continue;
                    }
                    let name = vr.name.trim_start_matches('$');
                    if name.is_empty() {
                        continue;
                    }
                    // Text var refs are always reads
                    var_encounters.push(VarEncounter {
                        name: name.to_string(),
                        type_hint: VarTypeHint::Unknown,
                        kind: VarAccessKind::Read,
                        line: source_line,
                        byte_span: vr.span.clone(),
                    });
                }
            }

            PassageNode::Macro {
                parsed,
                var_refs,
                children,
                span,
                ..
            } => {
                let macro_name = parsed.name.as_str();
                let source_line = line_from_span(span.start, body, body_offset);

                let selectivity = classify_macro(macro_name, builtin_lookup, callable_names);

                match selectivity {
                    MacroSelectivity::Stateful => {
                        // Collect VarEncounter entries from this node's var_refs
                        collect_var_encounters(var_refs, source_line, body, body_offset, var_encounters);

                        // If this is a block macro, recurse into children
                        if let Some(children) = children {
                            walk_encounters_inner(
                                children, body, body_offset, builtin_lookup, callable_names,
                                var_encounters, comment_spans,
                            );
                        }
                    }

                    MacroSelectivity::NavShell => {
                        // Nav-shell macro: skip the shell (no var encounters for the shell),
                        // but recurse into children to find stateful macros.
                        if let Some(children) = children {
                            walk_encounters_inner(
                                children, body, body_offset, builtin_lookup, callable_names,
                                var_encounters, comment_spans,
                            );
                        }
                    }

                    MacroSelectivity::Skip => {
                        // Completely skip: no var encounters, no recursion.
                    }
                }
            }

            PassageNode::Expression { var_refs, span, .. } => {
                // Expression macro: <<=>> or <<->>
                // These are always stateful (they read vars for output)
                let source_line = line_from_span(span.start, body, body_offset);
                collect_var_encounters(var_refs, source_line, body, body_offset, var_encounters);
            }

            PassageNode::Heading { .. } => {
                // Headings don't produce var encounters
            }

            PassageNode::Error { .. } => {
                // Error nodes don't produce var encounters
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VarEncounter collection
// ---------------------------------------------------------------------------

/// Collect VarEncounter entries from a node's var_refs.
///
/// Only collects story variables (`$var`, not `_var` temp vars).
/// Type hints are inferred from context:
/// - Write macros (`set`, `capture`, `unset`): try to infer from RHS literal
/// - Read macros: Unknown (reads don't tell us type)
fn collect_var_encounters(
    var_refs: &[super::VarRef],
    source_line: u32,
    body: &str,
    body_offset: usize,
    encounters: &mut Vec<VarEncounter>,
) {
    for vr in var_refs {
        // Skip temporary variables
        if vr.is_temporary {
            continue;
        }

        // Strip the $ sigil to get the base name
        let name = vr.name.trim_start_matches('$');
        if name.is_empty() {
            continue;
        }

        let kind = if vr.is_write {
            VarAccessKind::Write
        } else {
            VarAccessKind::Read
        };

        // Type hint: for writes, try to infer from context; for reads, Unknown
        let type_hint = if vr.is_write {
            infer_type_hint_from_span(vr.span.clone(), body, body_offset)
        } else {
            VarTypeHint::Unknown
        };

        encounters.push(VarEncounter {
            name: name.to_string(),
            type_hint,
            kind,
            line: source_line,
            byte_span: vr.span.clone(),
        });
    }
}

/// Try to infer a type hint from the source text around a variable write span.
///
/// Looks at the text after the variable reference for assignment patterns
/// like `= 10` (Number), `= "hello"` (String), `= true` (Boolean),
/// `= []` (Array), `= {}` (Object).
fn infer_type_hint_from_span(
    var_span: std::ops::Range<usize>,
    body: &str,
    body_offset: usize,
) -> VarTypeHint {
    // Convert doc-absolute span to body-relative offset
    let _start = var_span.start.saturating_sub(body_offset);
    let end = var_span.end.saturating_sub(body_offset);

    if end > body.len() {
        return VarTypeHint::Unknown;
    }

    // Look at the text after the variable reference for an assignment
    let after = body[end..].trim_start();

    // Check for assignment operator
    if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("===") {
        let rhs = after[1..].trim_start();
        return infer_type_from_rhs(rhs);
    }

    // Check for `to` (SugarCube assignment syntax)
    if after.starts_with("to ") || after.starts_with("to\t") {
        let rhs = after[3..].trim_start();
        return infer_type_from_rhs(rhs);
    }

    VarTypeHint::Unknown
}

/// Infer type from the right-hand side of an assignment.
fn infer_type_from_rhs(rhs: &str) -> VarTypeHint {
    if rhs.is_empty() {
        return VarTypeHint::Unknown;
    }

    // Check for numeric literal
    let first = rhs.chars().next().unwrap();
    if first.is_ascii_digit() || first == '-' || first == '+' || first == '.' {
        // Try to parse as number
        let num_str: String = rhs.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E').collect();
        if num_str.parse::<f64>().is_ok() {
            return VarTypeHint::Number;
        }
    }

    // Check for string literal
    if first == '"' || first == '\'' {
        return VarTypeHint::String;
    }

    // Check for boolean
    if rhs.starts_with("true") || rhs.starts_with("false") {
        return VarTypeHint::Boolean;
    }

    // Check for array literal
    if first == '[' {
        return VarTypeHint::Array;
    }

    // Check for object literal
    if first == '{' {
        return VarTypeHint::Object;
    }

    VarTypeHint::Unknown
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the 0-based line number from a document-absolute byte offset.
///
/// Counts the number of `\n` characters in `body[..offset_within_body]`
/// to determine the line number. This is more reliable than `.lines()`
/// because `.lines()` doesn't count trailing empty lines.
fn line_from_span(doc_offset: usize, body: &str, body_offset: usize) -> u32 {
    let body_relative = doc_offset.saturating_sub(body_offset);
    let safe_end = body_relative.min(body.len());
    if safe_end == 0 {
        return 0;
    }
    // Count newlines before the offset position
    body[..safe_end].bytes().filter(|&b| b == b'\n').count() as u32
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comment_span_check_skips_macro() {
        // A macro span [10, 30) that overlaps with comment span [5, 35)
        let comment_spans = vec![5..35];
        let node_span = 10..30;
        assert!(is_in_comment(&comment_spans, &node_span));
    }

    #[test]
    fn test_comment_span_check_allows_non_comment() {
        // A macro span [40, 50) that does NOT overlap with comment span [5, 35)
        let comment_spans = vec![5..35];
        let node_span = 40..50;
        assert!(!is_in_comment(&comment_spans, &node_span));
    }

    #[test]
    fn test_comment_span_check_empty() {
        let comment_spans: Vec<std::ops::Range<usize>> = vec![];
        let node_span = 10..30;
        assert!(!is_in_comment(&comment_spans, &node_span));
    }
}
