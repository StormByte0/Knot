//! Dynamic navigation resolution and edge classification for SugarCube.
//!
//! SugarCube supports variable-driven navigation where a passage name is
//! stored in a story variable (e.g., `<<goto $nextPassage>>`). This module
//! resolves those dynamic links by tracking `<<set $var to "literal">>`
//! patterns and then mapping variable references in navigation macros to
//! their possible passage-name values.
//!
//! Also contains the format-specific edge classifier that distinguishes
//! Jump (<<goto>>), Include (<<include>>), Call (widget invocation), and
//! Navigation (everything else) edges in the story graph.

use std::collections::HashMap;
use std::sync::LazyLock;

use knot_core::passage::{Block, Passage};
use knot_core::graph::EdgeType;

use crate::types::ResolvedNavLink;

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// Regex for <<set $var to "literal">> patterns (dynamic navigation resolution)
pub(crate) static RE_SET_STRING: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"<<set\s+([\$][A-Za-z_][A-Za-z0-9_]*)\s+to\s+"([^"]*)""#
    ).unwrap()
});

/// Regex for navigation macros with variable args (dynamic navigation resolution)
pub(crate) static RE_NAV_VAR: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"<<(?:goto|include|link|button)\s+(?:"[^"]*"\s+)?([\$][A-Za-z_][A-Za-z0-9_]*)"#
    ).unwrap()
});

// ---------------------------------------------------------------------------
// Variable → string-value map
// ---------------------------------------------------------------------------

/// Build a map from variable names to their known string-literal values.
///
/// Scans all passages for `<<set $var to "literal">>` patterns. The map
/// is used by `resolve_dynamic_navigation_links()` to expand variable
/// references in navigation macros into concrete passage names.
pub(crate) fn build_var_string_map(workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            for block in &passage.body {
                let content = match block {
                    Block::Text { content, .. } => content.as_str(),
                    Block::Macro { args, .. } => args.as_str(),
                    _ => continue,
                };
                for caps in RE_SET_STRING.captures_iter(content) {
                    if let (Some(var_match), Some(val_match)) = (caps.get(1), caps.get(2)) {
                        let var_name = var_match.as_str().to_string();
                        let string_val = val_match.as_str().to_string();
                        map.entry(var_name).or_default().push(string_val);
                    }
                }
            }
        }
    }
    for values in map.values_mut() {
        values.sort();
        values.dedup();
    }
    map
}

// ---------------------------------------------------------------------------
// Dynamic navigation link resolution
// ---------------------------------------------------------------------------

/// Resolve variable-based navigation macros to concrete passage links.
///
/// For each `<<goto $var>>`, `<<include $var>>`, `<<link "label" $var>>`,
/// or `<<button "label" $var>>` in the passage body, look up the variable
/// in `var_string_map` and produce a `ResolvedNavLink` for each known value.
pub(crate) fn resolve_dynamic_navigation_links(
    passage: &Passage,
    var_string_map: &HashMap<String, Vec<String>>,
) -> Vec<ResolvedNavLink> {
    let mut links = Vec::new();
    for block in &passage.body {
        // Only Macro blocks can contain navigation macros with variable args.
        // The block's name field tells us which macro produced the link,
        // which determines the edge type (goto→Jump, include→Include, etc.)
        let (content, macro_name) = match block {
            Block::Macro { name, args, .. } => (args.as_str(), name.as_str()),
            _ => continue,
        };
        for caps in RE_NAV_VAR.captures_iter(content) {
            if let Some(var_match) = caps.get(1) {
                let var_name = var_match.as_str().to_string();
                if let Some(known_values) = var_string_map.get(&var_name) {
                    // Classify the edge type based on the macro name,
                    // matching the same logic as extract_macro_passage_refs().
                    let edge_type_hint = match macro_name {
                        "goto" => Some(EdgeType::Jump),
                        "include" => Some(EdgeType::Include),
                        _ => None, // <<link>>, <<button>>, etc. → Navigation
                    };
                    for value in known_values {
                        links.push(ResolvedNavLink {
                            display_text: Some(format!("{} (via {})", value, var_name)),
                            target: value.clone(),
                            edge_type_hint,
                        });
                    }
                }
            }
        }
    }
    links
}

// ---------------------------------------------------------------------------
// Edge classification
// ---------------------------------------------------------------------------

/// Classify the edge type for a link from a SugarCube passage.
///
/// Called by the graph handler when the link's `edge_type_hint` is `None` —
/// primarily for regular `[[links]]` and dynamic navigation resolved from
/// variables.
///
/// Most macro-based links (`<<goto>>`, `<<include>>`) already have their
/// `edge_type_hint` set during link extraction, so this method handles
/// the remaining cases that can only be determined with full passage context.
pub(crate) fn classify_edge(
    source_passage: &Passage,
    display_text: Option<&str>,
    target: &str,
) -> Option<EdgeType> {
    // Check if this is a dynamic navigation link resolved from a variable.
    // The display text has the pattern "PassageName (via $var)".
    // We need to determine the original macro by scanning the source
    // passage body for the macro that contained this variable reference.
    if let Some(dt) = display_text {
        if dt.contains("(via ") {
            // Extract the variable name from the display text
            if let Some(var_name) = dt.split("(via ").nth(1).and_then(|s| s.strip_suffix(')')) {
                // Scan the passage body blocks for the macro that contains
                // this variable reference to determine the edge type.
                for block in &source_passage.body {
                    if let Block::Macro { name, args, .. } = block {
                        if args.contains(var_name) {
                            match name.as_str() {
                                "goto" => return Some(EdgeType::Jump),
                                "include" => return Some(EdgeType::Include),
                                _ => {} // <<link>>, <<button>> → Navigation
                            }
                        }
                    }
                }
            }
        }
    }

    // Check if the target passage is a widget (tagged [widget]).
    // Widget invocations (<<widgetName>>) are Call edges — they push
    // onto the call stack and return, unlike navigation which replaces
    // the current passage.
    if target.chars().all(|c| c.is_alphanumeric() || c == '_') {
        // Only check if the target looks like a valid widget name
        // (alphanumeric + underscores, no spaces). Scan the body for
        // a bare macro invocation matching the target name — this is
        // how widgets are invoked in SugarCube.
        for block in &source_passage.body {
            if let Block::Macro { name, .. } = block {
                if name == target {
                    return Some(EdgeType::Call);
                }
            }
        }
    }

    None // Use default Navigation classification
}
