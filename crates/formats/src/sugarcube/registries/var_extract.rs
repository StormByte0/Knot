//! Variable extraction and property map building for SugarCube.
//!
//! This module extracts variable references from passages, builds shape-aware
//! property maps for dot-notation completion, and constructs the state
//! variable registry used by the LSP server.
//!
//! All `VarAccess` entries in the `VariableTree` store **passage-body-relative**
//! line numbers and spans. This module converts them to **document-absolute**
//! line numbers at the output boundary using the `PassagePositionMap`.

use super::variable_tree::{NO_NODE, NodeId, PassagePositionMap, VarArena, VariableTree};
use crate::plugin::SourceTextProvider;
use crate::types::{
    PassageTempVarSummary, PassageVarRef, PropertyKind, PropertyMapEntry, StateVariable,
    VarAccessKind as TypesVarAccessKind,
};
use knot_core::Workspace;
use std::collections::{HashMap, HashSet};

/// Extract variable references for a specific passage from the variable tree.
///
/// Walks the VariableTree to find all variable operations (reads and writes)
/// that occur in the named passage. Uses the hierarchical tree structure to
/// report both root-level and property-level accesses with proper line numbers.
///
/// Line numbers are converted from passage-relative to document-absolute
/// using the `passage_positions` map at the output boundary.
pub fn extract_passage_variable_refs_impl(
    var_tree: &VariableTree,
    _workspace: &Workspace,
    _source_text: &dyn SourceTextProvider,
    passage_name: &str,
    passage_positions: &PassagePositionMap,
) -> Vec<PassageVarRef> {
    let mut refs = Vec::new();

    for (var_name, var_id) in var_tree.iter() {
        // Walk the tree to collect all accesses (root + children) for this passage
        collect_refs_from_arena_node(
            var_tree.arena(),
            var_id,
            &var_name,
            passage_name,
            passage_positions,
            &mut refs,
        );
    }

    refs
}

/// Extract passage-scoped temporary variable summaries for a specific passage.
///
/// Like [`extract_passage_variable_refs_impl`], but walks only the
/// per-passage temp root instead of the persistent root. This is the
/// SugarCube-specific entry point that backs
/// [`FormatPlugin::extract_passage_temp_variables`](crate::plugin::FormatPlugin::extract_passage_temp_variables).
///
/// Returns one [`PassageTempVarSummary`] per distinct temporary variable
/// name (`_foo`, `_bar`, ...) declared in the passage, with all of that
/// variable's reads and writes (including property accesses like
/// `_obj.prop`) grouped together. The `refs` vector inside each summary
/// is sorted by line number for stable display.
///
/// Line numbers are converted from passage-relative to document-absolute
/// using the `passage_positions` map at the output boundary, identical
/// to the persistent-variable path.
pub fn extract_passage_temp_variables_impl(
    var_tree: &VariableTree,
    _workspace: &Workspace,
    _source_text: &dyn SourceTextProvider,
    passage_name: &str,
    passage_positions: &PassagePositionMap,
) -> Vec<PassageTempVarSummary> {
    let mut summaries: Vec<PassageTempVarSummary> = Vec::new();

    for (var_name, var_id) in var_tree.iter_temp_for_passage(passage_name) {
        let mut refs: Vec<PassageVarRef> = Vec::new();
        collect_refs_from_arena_node(
            var_tree.arena(),
            var_id,
            &var_name,
            passage_name,
            passage_positions,
            &mut refs,
        );

        if refs.is_empty() {
            // No direct accesses in this passage for this temp var — skip.
            // This shouldn't happen in practice (a temp root only exists
            // when at least one `_var` was recorded), but guard anyway.
            continue;
        }

        let write_count = refs.iter().filter(|r| r.is_write).count() as u32;
        let read_count = refs.iter().filter(|r| !r.is_write).count() as u32;

        // Sort refs by line for display, then by variable name so property
        // accesses on the same line stay in a deterministic order.
        refs.sort_by(|a, b| {
            a.line
                .cmp(&b.line)
                .then_with(|| a.variable_name.cmp(&b.variable_name))
        });

        summaries.push(PassageTempVarSummary {
            name: var_name,
            write_count,
            read_count,
            refs,
        });
    }

    // Stable alphabetic order by temp var name.
    summaries.sort_by(|a, b| a.name.cmp(&b.name));

    summaries
}

/// Recursively collect passage variable refs from an arena node and its children.
///
/// Uses `try_get` for child/sibling traversal to gracefully handle freed
/// nodes that might still be in a live parent's child chain.
fn collect_refs_from_arena_node(
    arena: &VarArena,
    node_id: NodeId,
    full_name: &str,
    passage_name: &str,
    passage_positions: &PassagePositionMap,
    refs: &mut Vec<PassageVarRef>,
) {
    let node = match arena.try_get(node_id) {
        Some(n) => n,
        None => return, // Freed or invalid node — skip
    };
    for access in &node.meta.refs {
        if access.passage_name != passage_name {
            continue;
        }
        // Include both direct and propagated accesses.
        // Per the propagation model (see docs/variable-write-propagation-model.md),
        // propagated writes represent the focus-level aggregate view:
        // `$a = {name:"x", weight:1}` shows 2 writes on `$a` (one per
        // immediate child), each carrying the child's construct span.
        // Direct writes are the leaf scalar writes themselves.

        // Convert passage-relative line → document-absolute line
        let abs_line = passage_positions
            .get(&(access.file_uri.clone(), access.passage_name.clone()))
            .map(|pos| pos.body_start_line + access.line)
            .unwrap_or(access.line);

        // Convert passage-relative span → document-absolute span
        let abs_span = passage_positions
            .get(&(access.file_uri.clone(), access.passage_name.clone()))
            .map(|pos| {
                access.span.start + pos.body_start_offset..access.span.end + pos.body_start_offset
            });

        refs.push(PassageVarRef {
            variable_name: full_name.to_string(),
            is_write: access.is_write(),
            line: abs_line,
            file_uri: access.file_uri.clone(),
            passage_name: access.passage_name.clone(),
            span: abs_span,
        });
    }

    // Recurse into children — skip freed nodes
    let mut child_id = node.first_child;
    while child_id != NO_NODE {
        let child = match arena.try_get(child_id) {
            Some(c) => c,
            None => break, // Stale pointer — stop traversing
        };
        if child.parent != NO_NODE {
            let child_name = child.name.clone();
            let child_full_name = format!("{}.{}", full_name, child_name);
            collect_refs_from_arena_node(
                arena,
                child_id,
                &child_full_name,
                passage_name,
                passage_positions,
                refs,
            );
        }
        child_id = child.next_sibling;
    }
}

/// Build a shape-aware property map for dot-notation completion.
///
/// For each variable in the tree, infers its structural kind (Scalar, Object,
/// Array, Unknown) from the child node structure, and builds a `PropertyMapEntry`
/// with immediate child property names and element shapes for arrays.
pub fn build_shape_aware_property_map_impl(
    var_tree: &VariableTree,
) -> HashMap<String, PropertyMapEntry> {
    let mut map = HashMap::new();

    for (var_name, var_id) in var_tree.iter() {
        let arena = var_tree.arena();
        let properties: Vec<String> = arena
            .children_of(var_id)
            .map(|child_id| arena.get(child_id).name.clone())
            .collect();

        let kind = infer_property_kind(&properties);

        let children = if kind == PropertyKind::Object {
            properties.clone()
        } else {
            Vec::new()
        };

        let element_shape = if kind == PropertyKind::Array && !properties.is_empty() {
            Some(Box::new(PropertyMapEntry {
                kind: PropertyKind::Object,
                children: properties,
                element_shape: None,
            }))
        } else {
            None
        };

        map.entry(var_name).or_insert(PropertyMapEntry {
            kind,
            children,
            element_shape,
        });
    }

    map
}

/// Infer the structural kind of a variable from its known properties.
pub fn infer_property_kind(properties: &[String]) -> PropertyKind {
    if properties.is_empty() {
        return PropertyKind::Unknown;
    }

    if properties
        .iter()
        .any(|p| p == "length" || p == "push" || p == "pop")
    {
        return PropertyKind::Array;
    }

    PropertyKind::Object
}

/// Build a state variable registry from the variable tree.
///
/// Converts the VariableTree's hierarchical representation into the
/// format-agnostic `StateVariable` type used by the server for
/// variable availability analysis and diagnostics.
///
/// Spans are converted from passage-body-relative to document-absolute
/// using the `passage_positions` map.
pub fn build_state_variable_registry_impl(
    var_tree: &VariableTree,
    passage_positions: &PassagePositionMap,
) -> HashMap<String, StateVariable> {
    let mut registry = HashMap::new();

    for (var_name, var_id) in var_tree.iter() {
        let arena = var_tree.arena();
        let node = arena.get(var_id);

        let dollar_name = if var_name.starts_with('$') || var_name.starts_with('_') {
            var_name.clone()
        } else if node.is_temporary {
            format!("_{}", var_name)
        } else {
            format!("${}", var_name)
        };

        let base_name = if let Some(stripped) = var_name
            .strip_prefix('$')
            .or_else(|| var_name.strip_prefix('_'))
        {
            stripped.to_string()
        } else {
            var_name.clone()
        };

        let mut write_locations = Vec::new();
        let mut read_locations = Vec::new();

        // Collect locations from the root node (direct + propagated)
        collect_locations_from_arena_node(
            arena,
            var_id,
            passage_positions,
            &mut write_locations,
            &mut read_locations,
        );

        // Collect property paths from child nodes
        let known_properties = collect_all_property_paths_from_arena_node(arena, var_id);

        registry.insert(
            var_name.clone(),
            StateVariable {
                base_name,
                dollar_name,
                known_properties,
                write_locations,
                read_locations,
                first_available: None,
                seeded_by_special: node.meta.seeded_by_special,
            },
        );
    }

    registry
}

/// Recursively collect write/read locations from an arena node tree.
///
/// Uses `try_get` for child/sibling traversal to gracefully handle freed
/// nodes that might still appear in a live parent's child chain after
/// `free_subtree` + `unlink_child`. Freed nodes (parent == NO_NODE) are
/// skipped to prevent following stale pointers into reused arena slots.
fn collect_locations_from_arena_node(
    arena: &VarArena,
    node_id: NodeId,
    passage_positions: &PassagePositionMap,
    write_locations: &mut Vec<crate::types::VarLocation>,
    read_locations: &mut Vec<crate::types::VarLocation>,
) {
    let node = match arena.try_get(node_id) {
        Some(n) => n,
        None => return, // Freed or invalid node — skip
    };
    for access in &node.meta.refs {
        let kind = if access.propagated {
            if access.is_write() {
                TypesVarAccessKind::PropertyWrite {
                    path: String::new(),
                }
            } else {
                TypesVarAccessKind::PropertyRead {
                    path: String::new(),
                }
            }
        } else if access.is_write() {
            TypesVarAccessKind::Assign
        } else {
            TypesVarAccessKind::Read
        };

        let abs_span = passage_positions
            .get(&(access.file_uri.clone(), access.passage_name.clone()))
            .map(|pos| {
                pos.body_start_offset + access.span.start..pos.body_start_offset + access.span.end
            })
            .unwrap_or_else(|| access.span.clone());

        let location = crate::types::VarLocation {
            passage_name: access.passage_name.clone(),
            file_uri: access.file_uri.clone(),
            span: abs_span,
            kind,
        };

        if access.is_write() {
            write_locations.push(location);
        } else {
            read_locations.push(location);
        }
    }

    // Recurse into children — skip freed nodes to avoid stale pointers
    let mut child_id = node.first_child;
    while child_id != NO_NODE {
        let child = match arena.try_get(child_id) {
            Some(c) => c,
            None => break, // Stale pointer — stop traversing
        };
        // Skip freed children (parent == NO_NODE). After free_subtree
        // clears first_child/next_sibling, this is a no-op for properly
        // freed nodes, but provides defense-in-depth.
        if child.parent != NO_NODE {
            collect_locations_from_arena_node(
                arena,
                child_id,
                passage_positions,
                write_locations,
                read_locations,
            );
        }
        child_id = child.next_sibling;
    }
}

/// Collect all property paths from an arena node's children as immediate child names.
///
/// Uses `try_get` instead of `get` for defense-in-depth against stale
/// pointers in the child chain.
fn collect_all_property_paths_from_arena_node(
    arena: &VarArena,
    node_id: NodeId,
) -> HashSet<String> {
    arena
        .children_of(node_id)
        .filter_map(|child_id| arena.try_get(child_id).map(|n| n.name.clone()))
        .collect()
}
