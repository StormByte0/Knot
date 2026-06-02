//! Variable extraction walks for the passage tree.
//!
//! Contains `walk_vars()`, `walk_passage_var_refs()`, `var_ref_to_var_op()`,
//! and all the augmentation functions that add JS-aware variable detection
//! on top of the basic tree-collected VarRef entries.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use knot_core::passage::{VarKind, VarOp};

use super::super::vars::{
    RE_JS_ALIAS_SPECIFIC, RE_JS_ALIAS_WHOLE, RE_JS_STATE_WRITE, RE_JS_STATE_GETVAR,
    RE_JS_STATE_SETVAR, RE_ALIAS_PROPERTY, RE_JS_VAR_ASSIGN, RE_JS_VAR_COMPOUND,
    RE_VAR_BRACKET_PROP, RE_SETTER_LINK,
};
use super::{PassageNode, VarRef, compute_args_offset, is_dollar_escape};

// ---------------------------------------------------------------------------
// walk_vars() — Replace extract_vars()
// ---------------------------------------------------------------------------

/// Walk the tree and extract variable operations.
///
/// Replaces `extract_vars()` (30+ regex passes on raw text) with a single
/// tree walk. The tree nodes already carry basic VarRef entries from
/// `extract_var_refs_from_text()` and `extract_var_refs_from_macro_args()`.
/// This function augments those with:
///
/// - `<<run>>` body JS analysis (detects `$var = value` and `$var += value`
///   writes within JavaScript code)
/// - `State.variables.var = value` (JS direct write)
/// - `State.getVar("$var")` (JS API read)
/// - `State.setVar("$var", value)` (JS API write)
/// - JS alias tracking (`var x = State.variables` → `x.prop` = `$prop`)
/// - JS specific alias (`var x = State.variables.gold` → `$gold` read)
/// - Setter links (`[[text|passage][$var to value]]`)
/// - Bracket notation property access (`$var["property"]`)
///
/// All variable references are deduplicated by span to avoid double-counting.
///
/// **Note**: `body` and `body_offset` are needed because the augmentation
/// passes require computing document-absolute byte offsets for matches found
/// within macro args. The basic VarRefs from tree nodes already have correct
/// spans; only the augmented refs need the offset computation.
pub(crate) fn walk_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<VarOp> {
    let mut all_refs: Vec<VarRef> = Vec::new();
    collect_var_refs(nodes, &mut all_refs);

    // ── Additional patterns that require macro-context awareness ────────
    augment_run_body_vars(nodes, body, body_offset, &mut all_refs);
    augment_state_api_vars(nodes, body, body_offset, &mut all_refs);
    augment_js_alias_vars(nodes, body, body_offset, &mut all_refs);
    augment_setter_link_vars(nodes, body, body_offset, &mut all_refs);
    augment_bracket_prop_vars(nodes, body, body_offset, &mut all_refs);

    // ── Deduplicate by span and convert to VarOp ───────────────────────
    let mut seen_spans: HashSet<(usize, usize)> = HashSet::new();
    let mut vars: Vec<VarOp> = Vec::new();

    for var_ref in &all_refs {
        let key = (var_ref.span.start, var_ref.span.end);
        if seen_spans.contains(&key) {
            continue;
        }
        seen_spans.insert(key);
        vars.push(var_ref_to_var_op(var_ref));
    }

    vars
}

/// Recursively collect VarRef entries from all tree nodes.
fn collect_var_refs(nodes: &[PassageNode], refs: &mut Vec<VarRef>) {
    for node in nodes {
        match node {
            PassageNode::Text { var_refs, .. } => {
                refs.extend(var_refs.iter().cloned());
            }
            PassageNode::Macro {
                var_refs, children, ..
            } => {
                refs.extend(var_refs.iter().cloned());
                if let Some(children) = children {
                    collect_var_refs(children, refs);
                }
            }
            PassageNode::Expression { var_refs, .. } => {
                refs.extend(var_refs.iter().cloned());
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Augment variable refs with `<<run>>` body JS analysis.
///
/// The basic tree extraction treats all `$var` in `<<run>>` args as reads.
/// This function detects JS write patterns within `<<run>>` bodies:
/// - `$var = value` (simple assignment, not == or ===)
/// - `$var += value` (compound assignment: +=, -=, *=, /=, %=)
fn augment_run_body_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            if parsed.name == "run" && !parsed.args.is_empty() {
                let js_text = &parsed.args;
                let args_offset = compute_args_offset(parsed, body, body_offset);

                // Track spans already covered to avoid double-counting
                let mut covered_spans: Vec<Range<usize>> = Vec::new();

                // Compound assignments: $var +=, -=, *=, /=, %=
                for js_caps in RE_JS_VAR_COMPOUND.captures_iter(js_text) {
                    let full = js_caps.get(0).unwrap();
                    let var_match = js_caps.get(1).unwrap();

                    // Skip $$ escape
                    if is_dollar_escape(js_text, full.start()) {
                        continue;
                    }
                    let is_double_dollar = full.as_str().starts_with("$$");
                    if is_double_dollar {
                        continue;
                    }

                    let name = format!("${}", var_match.as_str());
                    let var_start = args_offset + full.start();
                    let var_end = var_start + name.len();

                    // Skip if already in covered spans
                    if covered_spans.iter().any(|s| var_start >= s.start && var_end <= s.end) {
                        continue;
                    }

                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                    covered_spans.push(var_start..var_end);
                }

                // Simple assignments: $var = (but NOT ==, ===, or compound)
                for js_caps in RE_JS_VAR_ASSIGN.captures_iter(js_text) {
                    let full = js_caps.get(0).unwrap();
                    let var_match = js_caps.get(1).unwrap();

                    // Skip $$ escape
                    if is_dollar_escape(js_text, full.start()) {
                        continue;
                    }
                    let is_double_dollar = full.as_str().starts_with("$$");
                    if is_double_dollar {
                        continue;
                    }

                    // Skip if this is a compound assignment (already handled)
                    let is_compound = RE_JS_VAR_COMPOUND
                        .captures_iter(js_text)
                        .any(|cc| cc.get(0).unwrap().start() == full.start());
                    if is_compound {
                        continue;
                    }

                    // Skip if this is == or === (comparison, not assignment)
                    let after_match = js_text.get(full.end()..).unwrap_or("");
                    if after_match.starts_with('=') {
                        continue;
                    }

                    let name = format!("${}", var_match.as_str());
                    let var_start = args_offset + full.start();
                    let var_end = var_start + name.len();

                    // Skip if already in covered spans
                    if covered_spans.iter().any(|s| var_start >= s.start && var_end <= s.end) {
                        continue;
                    }

                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                    covered_spans.push(var_start..var_end);
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_run_body_vars(children, body, body_offset, refs);
        }
    }
}

/// Augment with `State.variables.var = value`, `State.getVar()`, `State.setVar()`.
///
/// These JavaScript API patterns can appear inside any macro's args (especially
/// `<<run>>`, `<<set>>`, `<<script>>` blocks) and also in text segments
/// (e.g., in `<<print>>` expressions or inline SugarCube markup).
fn augment_state_api_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    for node in nodes {
        if let PassageNode::Macro { parsed, .. } = node {
            let args = &parsed.args;
            if args.is_empty() {
                continue;
            }
            let args_offset = compute_args_offset(parsed, body, body_offset);

            // State.variables.var = value → WRITE
            for caps in RE_JS_STATE_WRITE.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                // Skip if already recorded at this span
                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }

            // State.getVar("$var") → READ
            for caps in RE_JS_STATE_GETVAR.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: false,
                        span: var_start..var_end,
                    });
                }
            }

            // State.setVar("$var", value) → WRITE
            for caps in RE_JS_STATE_SETVAR.captures_iter(args) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = args_offset + full.start();
                let var_end = args_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Also check text nodes for State API patterns (e.g., in <<print>>)
        if let PassageNode::Text { content, span, .. } = node {
            let text_offset = span.start;

            for caps in RE_JS_STATE_WRITE.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }

            for caps in RE_JS_STATE_GETVAR.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: false,
                        span: var_start..var_end,
                    });
                }
            }

            for caps in RE_JS_STATE_SETVAR.captures_iter(content) {
                let sc_var = caps.get(1).unwrap().as_str();
                let full = caps.get(0).unwrap();
                let dollar_name = format!("${}", sc_var);
                let var_start = text_offset + full.start();
                let var_end = text_offset + full.end();

                let is_dup = refs.iter().any(|r| {
                    r.name == dollar_name && r.span.start == var_start
                });
                if !is_dup {
                    refs.push(VarRef {
                        name: dollar_name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_state_api_vars(children, body, body_offset, refs);
        }
    }
}

/// Augment with JS alias tracking.
///
/// Detects `var x = State.variables` (whole-object alias) and
/// `var x = State.variables.gold` (specific-variable alias), then resolves
/// `x.prop` references as `$prop` reads/writes.
fn augment_js_alias_vars(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // First pass: collect all macro args texts to scan for alias declarations.
    // We need a flat view of all macro args and text content to build the
    // alias map, then a second pass to resolve alias property accesses.

    // Collect (content, content_offset) pairs for alias scanning.
    let mut content_pairs: Vec<(&str, usize)> = Vec::new();
    collect_content_for_alias_scan(nodes, body, body_offset, &mut content_pairs);

    // Build whole-object alias map: alias_name → (alias_offset, body-relative position)
    let mut whole_aliases: HashMap<String, usize> = HashMap::new();

    for (content, offset) in &content_pairs {
        // Whole-object alias: var x = State.variables (NOT State.variables.something)
        for caps in RE_JS_ALIAS_WHOLE.captures_iter(content) {
            let alias_name = caps.get(1).unwrap().as_str().to_string();
            let full = caps.get(0).unwrap();
            let alias_offset = *offset + full.start();

            // Check that this isn't also a specific alias
            let is_specific = RE_JS_ALIAS_SPECIFIC
                .captures_iter(content)
                .any(|specific_caps| {
                    specific_caps.get(0).unwrap().start() == full.start()
                });

            if !is_specific {
                whole_aliases.insert(alias_name, alias_offset);
            }
        }

        // Specific-variable alias: var x = State.variables.gold → $gold read
        for caps in RE_JS_ALIAS_SPECIFIC.captures_iter(content) {
            let _alias_name = caps.get(1).unwrap().as_str();
            let sc_var = caps.get(2).unwrap().as_str();
            let full = caps.get(0).unwrap();
            let var_start = *offset + full.start();
            let var_end = *offset + full.end();

            let dollar_name = format!("${}", sc_var);
            let is_dup = refs.iter().any(|r| {
                r.name == dollar_name && r.span.start == var_start
            });
            if !is_dup {
                refs.push(VarRef {
                    name: dollar_name,
                    property_path: None,
                    is_temporary: false,
                    is_write: false,
                    span: var_start..var_end,
                });
            }
        }
    }

    // Resolve whole-object alias property accesses
    if !whole_aliases.is_empty() {
        for (content, offset) in &content_pairs {
            for caps in RE_ALIAS_PROPERTY.captures_iter(content) {
                let alias_name = caps.get(1).unwrap().as_str();
                let property = caps.get(2).unwrap().as_str();
                let full = caps.get(0).unwrap();

                if let Some(&alias_offset) = whole_aliases.get(alias_name) {
                    let prop_start = *offset + full.start();
                    // Skip the alias declaration itself
                    if prop_start <= alias_offset {
                        continue;
                    }

                    let prop_end = *offset + full.end();
                    let dollar_name = format!("${}", property);

                    // Determine if this is a write by checking what follows
                    let after_match = &content[full.end()..];
                    let is_write = after_match.trim_start().starts_with('=')
                        && !after_match.trim_start().starts_with("==")
                        && !after_match.trim_start().starts_with("===");

                    let is_dup = refs.iter().any(|r| {
                        r.span.start == prop_start && r.span.end == prop_end
                    });

                    if !is_dup {
                        refs.push(VarRef {
                            name: dollar_name,
                            property_path: None,
                            is_temporary: false,
                            is_write,
                            span: prop_start..prop_end,
                        });
                    }
                }
            }
        }
    }
}

/// Collect (content_text, document_absolute_offset) pairs from all nodes
/// for JS alias scanning. This gives us a flat view of all text that might
/// contain alias declarations and their usages.
fn collect_content_for_alias_scan<'a>(
    nodes: &'a [PassageNode],
    body: &str,
    body_offset: usize,
    pairs: &mut Vec<(&'a str, usize)>,
) {
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                pairs.push((content.as_str(), span.start));
            }
            PassageNode::Macro {
                parsed, children, ..
            } => {
                if !parsed.args.is_empty() {
                    let args_offset = compute_args_offset(parsed, body, body_offset);
                    pairs.push((parsed.args.as_str(), args_offset));
                }
                if let Some(children) = children {
                    collect_content_for_alias_scan(children, body, body_offset, pairs);
                }
            }
            PassageNode::Expression {
                content, span, ..
            } => {
                pairs.push((content.as_str(), span.start));
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Augment with setter link variable refs.
///
/// Setter links: `[[text|passage][$var to value]]` or `[[text|passage][$var = value]]`
/// These assign variables during link navigation, so the variable is a write.
fn augment_setter_link_vars(
    nodes: &[PassageNode],
    _body: &str,
    _body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // Setter links appear inside text segments (in [[...]] syntax).
    // The tree's extract_links_from_text doesn't extract setter vars,
    // and extract_var_refs_from_text doesn't handle setter syntax.
    // We scan text nodes for setter link patterns.
    for node in nodes {
        if let PassageNode::Text { content, span, .. } = node {
            for caps in RE_SETTER_LINK.captures_iter(content) {
                let var_match = caps.get(1).unwrap();
                let full = caps.get(0).unwrap();
                let name = format!("${}", var_match.as_str());

                // Find the $var position within the setter
                let var_rel_start = full.as_str().find('$').unwrap_or(0);
                let var_start = span.start + full.start() + var_rel_start;
                let var_end = var_start + name.len();

                // Skip if already recorded at this span
                let is_dup = refs.iter().any(|r| {
                    r.span.start == var_start && r.span.end <= var_end
                });
                if !is_dup {
                    refs.push(VarRef {
                        name,
                        property_path: None,
                        is_temporary: false,
                        is_write: true,
                        span: var_start..var_end,
                    });
                }
            }
        }

        // Recurse into children
        if let PassageNode::Macro {
            children: Some(children),
            ..
        } = node
        {
            augment_setter_link_vars(children, _body, _body_offset, refs);
        }
    }
}

/// Augment with bracket-notation property access.
///
/// `$var["property"]` or `$var['property']` — bracket-notation property access
/// that records both the base variable read and the property path.
fn augment_bracket_prop_vars(
    nodes: &[PassageNode],
    _body: &str,
    _body_offset: usize,
    refs: &mut Vec<VarRef>,
) {
    // Bracket-notation can appear in text segments and macro args.
    // The basic extraction in the tree doesn't handle this, so we scan
    // both text and macro nodes.
    for node in nodes {
        match node {
            PassageNode::Text { content, span, .. } => {
                let text_offset = span.start;
                scan_bracket_notation(content, text_offset, refs);
            }
            PassageNode::Macro {
                parsed, children, ..
            } => {
                if !parsed.args.is_empty() {
                    let args_offset =
                        compute_args_offset(parsed, _body, _body_offset);
                    scan_bracket_notation(&parsed.args, args_offset, refs);
                }
                if let Some(children) = children {
                    augment_bracket_prop_vars(children, _body, _body_offset, refs);
                }
            }
            PassageNode::Expression {
                content, span, ..
            } => {
                scan_bracket_notation(content, span.start, refs);
            }
            PassageNode::Heading { .. } | PassageNode::Error { .. } => {}
        }
    }
}

/// Scan text for bracket-notation property access: `$var["property"]`.
pub(crate) fn scan_bracket_notation(text: &str, text_offset: usize, refs: &mut Vec<VarRef>) {
    for caps in RE_VAR_BRACKET_PROP.captures_iter(text) {
        let var_match = caps.get(1).unwrap();
        let prop_match = caps.get(2).unwrap();
        let full = caps.get(0).unwrap();

        // Skip $$ escape
        if is_dollar_escape(text, full.start()) {
            continue;
        }
        let is_double_dollar = full.as_str().starts_with("$$");
        if is_double_dollar {
            continue;
        }

        let var_start = text_offset + full.start();
        let var_end = text_offset + full.end();

        let base_name = format!("${}", var_match.as_str());
        let prop_path = format!("{}.{}", base_name, prop_match.as_str());

        // Skip if already recorded at this span
        let is_dup = refs.iter().any(|r| {
            r.name == prop_path && r.span.start == var_start
        });
        if !is_dup {
            refs.push(VarRef {
                name: prop_path,
                property_path: Some(prop_match.as_str().to_string()),
                is_temporary: false,
                is_write: false,
                span: var_start..var_end,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// walk_passage_var_refs() — Tree-based replacement for extract_virtual_var_accesses
// ---------------------------------------------------------------------------

/// Walk the tree and produce `PassageVarRef` entries with exact line numbers.
///
/// Replaces the old `extract_virtual_var_accesses()` path which:
/// 1. Built the entire virtual document (translating ALL passages to JS)
/// 2. Ran regex on the JS output to find `State.variables.x` patterns
/// 3. Mapped back to source lines via the (lossy) proportional line map
///
/// This function instead:
/// 1. Walks the tree directly (no virtual document build needed)
/// 2. Uses `walk_vars()` which already handles all var patterns
/// 3. Computes exact line numbers from byte spans (no proportional mapping)
///
/// The result is both faster (no full vdoc build) and more accurate (exact
/// line numbers instead of proportional approximation).
pub(crate) fn walk_passage_var_refs(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    passage_name: &str,
    file_uri: &str,
) -> Vec<crate::types::PassageVarRef> {
    let var_ops = walk_vars(nodes, body, body_offset);

    var_ops
        .into_iter()
        .filter(|v| !v.is_temporary)
        .map(|v| {
            let line = line_from_span(v.span.start, body, body_offset);
            crate::types::PassageVarRef {
                variable_name: v.name,
                is_write: matches!(v.kind, VarKind::Init),
                line,
                file_uri: file_uri.to_string(),
                passage_name: passage_name.to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VarRef → VarOp conversion (used by walk_vars() in Phase 2)
// ---------------------------------------------------------------------------

/// Convert a `VarRef` to a `VarOp` for the core `Passage.vars` field.
///
/// This is a straightforward mapping:
/// - `is_write = true` → `VarKind::Init`
/// - `is_write = false` → `VarKind::Read`
/// - `is_temporary` passes through
/// - `name` is the full name (including property path if present)
pub(crate) fn var_ref_to_var_op(var_ref: &VarRef) -> VarOp {
    let full_name = match &var_ref.property_path {
        Some(path) => format!("{}.{}", var_ref.name, path),
        None => var_ref.name.clone(),
    };
    VarOp {
        name: full_name,
        kind: if var_ref.is_write {
            VarKind::Init
        } else {
            VarKind::Read
        },
        span: var_ref.span.clone(),
        is_temporary: var_ref.is_temporary,
    }
}

// ---------------------------------------------------------------------------
// Helper: line number from byte span
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
