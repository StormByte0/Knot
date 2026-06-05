//! Dot-notation property map and shape-aware type inference for SugarCube variables.

use std::collections::{HashMap, HashSet};

use knot_core::passage::VarOp;

use crate::types::{PropertyKind, PropertyMapEntry};
use super::state_registry::build_state_variable_registry;

// ---------------------------------------------------------------------------
// Dot-notation property map
// ---------------------------------------------------------------------------

/// Build a map of variable dot-path → set of immediate child property names.
///
/// Scans all variable operations across the workspace and builds a tree:
/// `{"item": {"sword": {}, "shield": {}}, "player": {"name": {}, "health": {}}}`
///
/// Returns a `HashMap<String, HashSet<String>>` mapping parent paths to their
/// immediate children. Used for dot-notation completion (e.g., `$item.` →
/// suggest "sword", "shield").
pub(crate) fn extract_object_property_map(
    vars_by_passage: &[Vec<&VarOp>],
) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();

    for vars in vars_by_passage {
        for var in vars {
            if var.is_temporary {
                continue;
            }

            // Only consider variables with dots in their name
            if !var.name.contains('.') {
                continue;
            }

            // Must start with $ for SugarCube
            if !var.name.starts_with('$') {
                continue;
            }

            // Split the name into path segments
            let without_sigil = &var.name[1..]; // strip $
            let segments: Vec<&str> = without_sigil.split('.').collect();

            // Build the property map by walking the path
            // For "$item.sword.damage", add:
            //   "$item" → {"sword"}
            //   "$item.sword" → {"damage"}
            for i in 0..segments.len().saturating_sub(1) {
                let parent = if i == 0 {
                    format!("${}", segments[0])
                } else {
                    format!("${}", segments[..=i].join("."))
                };
                let child = segments[i + 1].to_string();
                map.entry(parent).or_default().insert(child);
            }
        }
    }

    map
}

// ---------------------------------------------------------------------------
// Shape-aware property map
// ---------------------------------------------------------------------------

/// Build a shape-aware property map that enriches the basic child-name map
/// with structural type information (`PropertyKind`) and array element shapes.
///
/// This is consumed by the completion handler for dot-notation completion.
/// It infers the kind of each variable from its assignment patterns:
/// - `<<set $var to {}>>` or `$var.prop = val` → Object
/// - `<<set $var to []>>` or `$var[0]` → Array
/// - `<<set $var to 42>>` or no children → Scalar
/// - No assignment patterns found → Unknown
///
/// For arrays, if element properties can be determined from `$var[0].prop`
/// patterns, they are stored in `element_shape`.
pub(crate) fn build_shape_aware_property_map(
    workspace: &knot_core::Workspace,
) -> HashMap<String, PropertyMapEntry> {
    // Build the basic property map first
    let vars_by_passage: Vec<Vec<&VarOp>> = workspace
        .documents()
        .flat_map(|doc| doc.passages.iter().map(|p| p.vars.iter().collect()))
        .collect();
    let basic_map = extract_object_property_map(&vars_by_passage);

    // Build the state variable registry for kind inference
    let registry = build_state_variable_registry(workspace);

    let mut result: HashMap<String, PropertyMapEntry> = HashMap::new();

    // For each base variable in the registry, determine its kind and children
    for (dollar_name, state_var) in &registry {
        let kind = infer_variable_kind_from_properties(dollar_name, &state_var.known_properties, &basic_map);
        let children: Vec<String> = basic_map
            .get(dollar_name)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();

        // For arrays, try to determine element shape from bracket-notation patterns
        let element_shape = if kind == PropertyKind::Array {
            infer_array_element_shape(dollar_name, &basic_map)
        } else {
            None
        };

        result.insert(dollar_name.clone(), PropertyMapEntry {
            kind,
            children,
            element_shape,
        });
    }

    // Also add entries for nested property paths (e.g., "$player.state")
    // that appear as keys in the basic map but aren't base variables
    for (path, children_set) in &basic_map {
        if result.contains_key(path) {
            continue; // Already added as a base variable
        }
        // Infer kind from whether this path has children
        let kind = if children_set.is_empty() {
            PropertyKind::Scalar
        } else {
            // Has children → likely an object property
            PropertyKind::Object
        };
        let children: Vec<String> = children_set.iter().cloned().collect();

        result.insert(path.clone(), PropertyMapEntry {
            kind,
            children,
            element_shape: None,
        });
    }

    result
}

/// Infer the `PropertyKind` of a base variable from its known properties
/// and the basic property map.
pub(crate) fn infer_variable_kind_from_properties(
    dollar_name: &str,
    known_properties: &HashSet<String>,
    basic_map: &HashMap<String, HashSet<String>>,
) -> PropertyKind {
    // If the variable has known dot-notation properties, it's an Object
    if !known_properties.is_empty() {
        // Check if any property suggests an array (numeric-like keys or [*] patterns)
        // For now, if it has named properties, treat it as Object
        return PropertyKind::Object;
    }

    // Check if the variable has children in the basic map
    if let Some(children) = basic_map.get(dollar_name) {
        if !children.is_empty() {
            return PropertyKind::Object;
        }
    }

    // No properties or children → likely Scalar
    PropertyKind::Scalar
}

/// For an array variable, try to infer the element shape from bracket-notation
/// patterns in the basic property map (e.g., `$items[0].name` → element has `name`).
///
/// Returns `Some(PropertyMapEntry)` if element properties can be determined,
/// `None` if the element shape is unknown.
pub(crate) fn infer_array_element_shape(
    _dollar_name: &str,
    _basic_map: &HashMap<String, HashSet<String>>,
) -> Option<Box<PropertyMapEntry>> {
    // TODO: Implement bracket-notation element shape inference.
    // This requires scanning for patterns like `$var[0].prop` and
    // collecting common properties across all indexed accesses.
    // For now, return None (unknown element shape).
    None
}
