//! Navigation link resolution for SugarCube dynamic navigation macros.
//!
//! This module builds a map of variable → string literal values and resolves
//! dynamic navigation macros like `<<goto $dest>>` into concrete passage names.

use std::collections::HashMap;
use knot_core::passage::{Block, Passage};
use crate::types::ResolvedNavLink;
use super::variable_tree::VariableTree;

/// Build a map of variable name → set of known string literal values.
///
/// Walks all passages in the workspace looking for patterns where a
/// variable is assigned a string literal value, e.g.:
/// - `<<set $dest to "Forest">>`
/// - `<<set $dest to 'Forest'>>`
///
/// This map is then used by `resolve_dynamic_navigation_links` to resolve
/// dynamic navigation macros like `<<goto $dest>>` into concrete passage
/// names for the story graph.
pub(super) fn build_var_string_map_impl(
    workspace: &knot_core::Workspace,
    _var_tree: &VariableTree,
) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();

    // Walk all passages in the workspace looking for variable assignments
    // to string literals
    for doc in workspace.documents() {
        for passage in &doc.passages {
            for block in &passage.body {
                if let Block::Macro { name, args, .. } = block
                    && name.eq_ignore_ascii_case("set")
                    && let Some((var_name, string_val)) = extract_set_string_literal(args)
                {
                    map.entry(var_name).or_default().push(string_val);
                }
            }
        }
    }

    map
}

/// Extract a variable name and string literal value from a `<<set>>` macro's
/// argument string.
///
/// Matches patterns like:
/// - `$var to "value"` → Some(("$var", "value"))
/// - `$var to 'value'` → Some(("$var", "value"))
/// - `$var = "value"` → Some(("$var", "value"))
///
/// Returns `None` if the args don't match a simple string assignment pattern.
pub(super) fn extract_set_string_literal(args: &str) -> Option<(String, String)> {
    let trimmed = args.trim();

    // Find the variable name (must start with $ or _)
    let var_end = trimmed.find(|c: char| c.is_whitespace()).unwrap_or(trimmed.len());
    if var_end == 0 {
        return None;
    }
    let var_name = &trimmed[..var_end];
    if !var_name.starts_with('$') && !var_name.starts_with('_') {
        return None;
    }

    // Skip to after "to" or "="
    let rest = trimmed[var_end..].trim();
    let after_assign = if let Some(after_to) = rest.strip_prefix("to") {
        after_to.trim()
    } else if let Some(after_eq) = rest.strip_prefix("=") {
        after_eq.trim()
    } else {
        return None;
    };

    // Extract string literal
    if (after_assign.starts_with('"') && after_assign.ends_with('"') && after_assign.len() >= 2)
        || (after_assign.starts_with('\'') && after_assign.ends_with('\'') && after_assign.len() >= 2)
    {
        let string_val = &after_assign[1..after_assign.len() - 1];
        Some((var_name.to_string(), string_val.to_string()))
    } else {
        None
    }
}

/// Resolve dynamic navigation links from a passage.
///
/// For each `<<goto $var>>`, `<<include $var>>`, `<<link $var>>`,
/// `<<button $var>>` in the passage, look up `$var` in the var_string_map
/// to find known string literal values. Return resolved link targets
/// with appropriate edge type hints.
pub(super) fn resolve_dynamic_navigation_links_impl(
    passage: &Passage,
    var_string_map: &HashMap<String, Vec<String>>,
) -> Vec<ResolvedNavLink> {
    let mut results = Vec::new();

    // Macro names that navigate to other passages
    let nav_macros: &[&str] = &["goto", "include", "link", "button"];

    for block in &passage.body {
        if let Block::Macro { name, args, .. } = block {
            if !nav_macros.contains(&name.as_str()) {
                continue;
            }

            // Extract the variable reference from args
            let trimmed = args.trim();

            // Check if args is a variable reference ($var)
            if trimmed.starts_with('$') {
                let var_name = trimmed; // Use full trimmed args as var name
                if let Some(values) = var_string_map.get(var_name) {
                    // Determine edge type from macro name
                    let edge_hint = match name.as_str() {
                        "goto" => Some(knot_core::graph::EdgeType::Jump),
                        "include" => Some(knot_core::graph::EdgeType::Include),
                        "link" | "button" => Some(knot_core::graph::EdgeType::Navigation),
                        _ => None,
                    };

                    for val in values {
                        results.push(ResolvedNavLink {
                            display_text: Some(format!("via {}", var_name)),
                            target: val.clone(),
                            edge_type_hint: edge_hint,
                        });
                    }
                }
            }
        }
    }

    results
}
