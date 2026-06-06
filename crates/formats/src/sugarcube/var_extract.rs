//! Variable extraction and property map building for SugarCube.
//!
//! This module extracts variable references from passages, builds shape-aware
//! property maps for dot-notation completion, and constructs the state
//! variable registry used by the LSP server.

use std::collections::HashMap;
use knot_core::Workspace;
use crate::plugin::SourceTextProvider;
use crate::types::{PassageVarRef, PropertyKind, PropertyMapEntry, StateVariable};
use super::variable_tree::VariableTree;

/// Extract variable references for a specific passage from the variable tree.
///
/// Walks the VariableTree to find all variable operations (reads and writes)
/// that occur in the named passage, and converts them to format-agnostic
/// `PassageVarRef` instances with line numbers computed from the source text.
pub(super) fn extract_passage_variable_refs_impl(
    var_tree: &VariableTree,
    _workspace: &Workspace,
    source_text: &dyn SourceTextProvider,
    passage_name: &str,
) -> Vec<PassageVarRef> {
    let mut refs = Vec::new();

    // Get all variables that have accesses in this passage
    for (var_name, entry) in var_tree.iter() {
        for access in &entry.accesses {
            if access.passage_name != passage_name {
                continue;
            }

            // Compute line number from byte offset using source text
            let line = source_text
                .get_source_text(&access.file_uri)
                .map(|text| compute_line_from_offset(text, access.span.start))
                .unwrap_or(0);

            refs.push(PassageVarRef {
                variable_name: var_name.clone(),
                is_write: access.is_write,
                line,
                file_uri: access.file_uri.clone(),
                passage_name: access.passage_name.clone(),
            });
        }

        // Also add entries for property paths
        for prop in &entry.known_properties {
            // Property accesses are tracked in the base variable's accesses
            // but the property path itself is recorded separately.
            // We emit a synthetic ref for each property path seen in this passage.
            let full_name = format!("{}.{}", var_name, prop);
            let has_write_in_passage = entry.accesses.iter().any(|a| {
                a.passage_name == passage_name && a.is_write
            });

            refs.push(PassageVarRef {
                variable_name: full_name,
                is_write: has_write_in_passage,
                line: 0, // Property-level line resolution needs deeper tracking
                file_uri: entry.accesses.first().map(|a| a.file_uri.clone()).unwrap_or_default(),
                passage_name: passage_name.to_string(),
            });
        }
    }

    refs
}

/// Build a shape-aware property map for dot-notation completion.
///
/// For each variable in the tree, infers its structural kind (Scalar, Object,
/// Array, Unknown) from assignment patterns, and builds a `PropertyMapEntry`
/// with immediate child property names and element shapes for arrays.
pub(super) fn build_shape_aware_property_map_impl(
    var_tree: &VariableTree,
) -> HashMap<String, PropertyMapEntry> {
    let mut map = HashMap::new();

    for (var_name, entry) in var_tree.iter() {
        let properties: Vec<String> = entry.known_properties.iter().cloned().collect();

        // Infer kind from property patterns:
        // - If the variable has child properties, it's an Object
        // - If the variable has a "length" property or is assigned [], it's an Array
        // - Otherwise, Unknown (we don't have assignment value analysis yet)
        let kind = infer_property_kind(&properties);

        let children = if kind == PropertyKind::Object {
            properties.clone()
        } else {
            Vec::new()
        };

        let element_shape = if kind == PropertyKind::Array && !properties.is_empty() {
            // For arrays, properties represent element shape
            Some(Box::new(PropertyMapEntry {
                kind: PropertyKind::Object,
                children: properties,
                element_shape: None,
            }))
        } else {
            None
        };

        map.entry(var_name.clone()).or_insert(PropertyMapEntry {
            kind,
            children,
            element_shape,
        });
    }

    map
}

/// Infer the structural kind of a variable from its known properties.
pub(super) fn infer_property_kind(properties: &[String]) -> PropertyKind {
    if properties.is_empty() {
        return PropertyKind::Unknown;
    }

    // Heuristic: if "length" is a known property and there are numeric-like
    // properties or push/pop methods, treat as Array
    if properties.iter().any(|p| p == "length" || p == "push" || p == "pop") {
        return PropertyKind::Array;
    }

    // If there are any child properties, treat as Object
    PropertyKind::Object
}

/// Build a state variable registry from the variable tree.
///
/// Converts the VariableTree's internal representation into the
/// format-agnostic `StateVariable` type used by the server for
/// variable availability analysis and diagnostics.
pub(super) fn build_state_variable_registry_impl(
    var_tree: &VariableTree,
) -> HashMap<String, StateVariable> {
    use crate::types::{VarAccessKind, VarLocation};

    let mut registry = HashMap::new();

    for (var_name, entry) in var_tree.iter() {
        let dollar_name = if var_name.starts_with('$') || var_name.starts_with('_') {
            var_name.clone()
        } else if entry.is_temporary {
            format!("_{}", var_name)
        } else {
            format!("${}", var_name)
        };

        let base_name = if let Some(stripped) = var_name.strip_prefix('$').or_else(|| var_name.strip_prefix('_')) {
            stripped.to_string()
        } else {
            var_name.clone()
        };

        let mut write_locations = Vec::new();
        let mut read_locations = Vec::new();

        for access in &entry.accesses {
            // Determine the access kind — for the base variable level,
            // we don't have property_path on VarAccess, so we use
            // the base Assign/Read kinds
            let kind = if access.is_write {
                VarAccessKind::Assign
            } else {
                VarAccessKind::Read
            };

            let location = VarLocation {
                passage_name: access.passage_name.clone(),
                file_uri: access.file_uri.clone(),
                span: access.span.clone(),
                kind,
            };

            if access.is_write {
                write_locations.push(location);
            } else {
                read_locations.push(location);
            }
        }

        // Add PropertyRead/PropertyWrite entries from known_properties
        for prop in &entry.known_properties {
            // Check if any write access in this passage wrote a property
            let prop_write_loc = VarLocation {
                passage_name: String::new(), // Would need per-property tracking
                file_uri: String::new(),
                span: 0..0,
                kind: VarAccessKind::PropertyWrite { path: prop.clone() },
            };
            let prop_read_loc = VarLocation {
                passage_name: String::new(),
                file_uri: String::new(),
                span: 0..0,
                kind: VarAccessKind::PropertyRead { path: prop.clone() },
            };
            // Add property reads (conservative — assume all properties are read)
            read_locations.push(prop_read_loc);
            write_locations.push(prop_write_loc);
        }

        registry.insert(var_name.clone(), StateVariable {
            base_name,
            dollar_name,
            known_properties: entry.known_properties.clone(),
            write_locations,
            read_locations,
            first_available: None, // Computed later by graph-BFS
            seeded_by_special: entry.seeded_by_special,
        });
    }

    registry
}

/// Compute a 0-based line number from a byte offset in source text.
pub(super) fn compute_line_from_offset(source: &str, offset: usize) -> u32 {
    let pos = offset.min(source.len());
    source[..pos].chars().filter(|&c| c == '\n').count() as u32
}
