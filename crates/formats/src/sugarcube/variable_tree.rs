//! Variable side table for the SugarCube format plugin.
//!
//! The `VariableTree` is a maintained side table that tracks all state variables
//! (`$var`) and temporary variables (`_var`) seen across the workspace. It is
//! populated incrementally during the ordered parse pipeline:
//!
//! 1. Script passages → oxc walk → `State.variables.*` writes
//! 2. Widget passages → SugarCube parser → `<<widget>>` variable encounters
//! 3. Normal passages → SugarCube parser → `$var` and `_var` references
//!
//! ## Why a side table?
//!
//! The `Passage.vars` field on each passage only contains the var ops for that
//! one passage. The `VariableTree` aggregates across ALL passages to provide:
//!
//! - Complete variable inventory for workspace-wide completion
//! - First-write location for go-to-definition
//! - Read/write locations for find-all-references
//! - Property shape inference for dot-notation completion
//! - Variable tree UI for the sidebar panel
//!
//! ## Thread safety
//!
//! The tree is wrapped in a `RwLock` inside `SugarCubePlugin`. Multiple readers
//! (completion, hover, references) can access it concurrently. Only the parse
//! pipeline needs a write lock.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::types::{PropertyKind, VariablePropertyNode, VariableTreeNode, VariableUsageLocation};

// ---------------------------------------------------------------------------
// VarAccess — a single variable access record
// ---------------------------------------------------------------------------

/// A recorded access to a variable within a passage.
#[derive(Debug, Clone)]
pub struct VarAccess {
    /// The passage name where this access occurs.
    pub passage_name: String,
    /// The file URI where this access occurs.
    pub file_uri: String,
    /// Byte range of the variable reference in the source text.
    pub span: Range<usize>,
    /// Whether this is a write/assignment (vs a read).
    pub is_write: bool,
    /// Whether this is a temporary variable (`_` sigil).
    pub is_temporary: bool,
    /// The 0-based line number within the file (0 until computed by line mapping).
    pub line: u32,
}

// ---------------------------------------------------------------------------
// VarEntry — all known info about a single variable
// ---------------------------------------------------------------------------

/// All tracked information about a single variable across the workspace.
#[derive(Debug, Clone)]
pub struct VarEntry {
    /// The variable name including sigil (e.g., `$hp`, `_i`).
    pub name: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
    /// All recorded accesses to this variable.
    pub accesses: Vec<VarAccess>,
    /// Known dot-notation property paths (e.g., `{"name", "hp"}` for `$player`).
    pub known_properties: HashSet<String>,
    /// Whether this variable is seeded by a special passage (StoryInit, [script]).
    pub seeded_by_special: bool,
}

impl VarEntry {
    /// Create a new entry for a variable.
    pub fn new(name: String, is_temporary: bool) -> Self {
        Self {
            name,
            is_temporary,
            accesses: Vec::new(),
            known_properties: HashSet::new(),
            seeded_by_special: false,
        }
    }

    /// Get the first write location, if any.
    pub fn first_write(&self) -> Option<&VarAccess> {
        self.accesses.iter().find(|a| a.is_write)
    }

    /// Get all write locations.
    pub fn writes(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| a.is_write).collect()
    }

    /// Get all read locations.
    pub fn reads(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| !a.is_write).collect()
    }

    /// Record a variable access.
    pub fn record_access(&mut self, access: VarAccess) {
        // If property path, add to known_properties
        // (property path is extracted from the name like `$player.name`)
        if let Some(dot_pos) = self.name.find('.') {
            // This shouldn't happen — base names don't have dots
        }
        self.accesses.push(access);
    }

    /// Record a property path seen on this variable.
    pub fn record_property(&mut self, property_path: String) {
        self.known_properties.insert(property_path);
    }
}

// ---------------------------------------------------------------------------
// VariableTree — the main side table
// ---------------------------------------------------------------------------

/// The variable side table for a SugarCube workspace.
///
/// Maintains a map of variable name → `VarEntry` for all `$var` and `_var`
/// references across all parsed passages. Updated incrementally during
/// the parse pipeline.
#[derive(Debug, Clone, Default)]
pub struct VariableTree {
    /// Map of variable name (including sigil) → entry.
    variables: HashMap<String, VarEntry>,
}

impl VariableTree {
    /// Create an empty variable tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a variable access from a passage.
    ///
    /// If the variable hasn't been seen before, a new entry is created.
    /// The `property_path` parameter is the dot-notation path after the
    /// base name (e.g., "name" for `$player.name`). Empty string if no
    /// property access.
    pub fn record_var(
        &mut self,
        name: &str,
        is_temporary: bool,
        is_write: bool,
        passage_name: &str,
        file_uri: &str,
        span: Range<usize>,
        property_path: &str,
    ) {
        let entry = self
            .variables
            .entry(name.to_string())
            .or_insert_with(|| VarEntry::new(name.to_string(), is_temporary));

        entry.record_access(VarAccess {
            passage_name: passage_name.to_string(),
            file_uri: file_uri.to_string(),
            span,
            is_write,
            is_temporary,
            line: 0, // Line computed later by SourceTextProvider
        });

        if !property_path.is_empty() {
            entry.record_property(property_path.to_string());
        }
    }

    /// Mark a variable as seeded by a special passage (StoryInit, [script]).
    pub fn mark_seeded(&mut self, name: &str) {
        if let Some(entry) = self.variables.get_mut(name) {
            entry.seeded_by_special = true;
        }
    }

    /// Get an entry for a variable by name.
    pub fn get_variable(&self, name: &str) -> Option<&VarEntry> {
        self.variables.get(name)
    }

    /// Get a mutable entry for a variable by name.
    pub fn get_variable_mut(&mut self, name: &str) -> Option<&mut VarEntry> {
        self.variables.get_mut(name)
    }

    /// Get all variable names in the tree.
    pub fn variable_names(&self) -> impl Iterator<Item = &String> {
        self.variables.keys()
    }

    /// Get the number of tracked variables.
    pub fn len(&self) -> usize {
        self.variables.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }

    /// Clear all variable data (for full re-parse).
    pub fn clear(&mut self) {
        self.variables.clear();
    }

    /// Remove all entries for a specific file (for incremental re-parse).
    pub fn remove_file(&mut self, file_uri: &str) {
        for entry in self.variables.values_mut() {
            entry.accesses.retain(|a| a.file_uri != file_uri);
        }
        // Remove variables with no remaining accesses (unless they have known properties)
        self.variables.retain(|_, entry| {
            !entry.accesses.is_empty() || !entry.known_properties.is_empty()
        });
    }

    /// Build a `VariableTreeNode` for the variable tree UI.
    ///
    /// This converts the flat side table into the tree-structured format
    /// that the LSP server and VS Code extension expect.
    pub fn build_tree(&self) -> Vec<VariableTreeNode> {
        let mut nodes = Vec::new();

        for entry in self.variables.values() {
            let written_in: Vec<VariableUsageLocation> = entry
                .writes()
                .into_iter()
                .map(|a| VariableUsageLocation {
                    passage_name: a.passage_name.clone(),
                    file_uri: a.file_uri.clone(),
                    is_write: true,
                    line: a.line,
                })
                .collect();

            let read_in: Vec<VariableUsageLocation> = entry
                .reads()
                .into_iter()
                .map(|a| VariableUsageLocation {
                    passage_name: a.passage_name.clone(),
                    file_uri: a.file_uri.clone(),
                    is_write: false,
                    line: a.line,
                })
                .collect();

            let is_unused = !entry.accesses.iter().any(|a| !a.is_write);

            // Build property nodes from known_properties
            let properties: Vec<VariablePropertyNode> = entry
                .known_properties
                .iter()
                .map(|prop| VariablePropertyNode {
                    name: prop.clone(),
                    full_name: format!("{}.{}", entry.name, prop),
                    state_path: format!(
                        "State.variables.{}",
                        entry.name.trim_start_matches('$').replace('.', ".")
                    ),
                    line: 0,
                    written_in: Vec::new(), // Property-level writes tracked separately
                    read_in: Vec::new(),
                    properties: Vec::new(),
                    kind: PropertyKind::Unknown,
                    element_shape: None,
                    coverage: None,
                })
                .collect();

            // Infer kind from properties
            let kind = if !properties.is_empty() {
                PropertyKind::Object
            } else {
                PropertyKind::Unknown
            };

            nodes.push(VariableTreeNode {
                name: entry.name.clone(),
                state_path: format!(
                    "State.variables.{}",
                    entry.name.trim_start_matches('$')
                ),
                is_temporary: entry.is_temporary,
                written_in,
                read_in,
                initialized_at_start: entry.seeded_by_special,
                is_unused,
                properties,
                kind,
                element_shape: None,
            });
        }

        // Sort by name for stable output
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        nodes
    }

    /// Build a set of variable names for completion.
    pub fn completion_names(&self) -> HashSet<String> {
        self.variables.keys().cloned().collect()
    }

    /// Build a map of variable name → known property paths (for dot-notation completion).
    pub fn property_map(&self) -> HashMap<String, HashSet<String>> {
        let mut map = HashMap::new();
        for (name, entry) in &self.variables {
            if !entry.known_properties.is_empty() {
                map.insert(name.clone(), entry.known_properties.clone());
            }
        }
        map
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_retrieve_variable() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            true,
            "Start",
            "file:///test.tw",
            10..13,
            "",
        );
        tree.record_var(
            "$hp",
            false,
            false,
            "Forest",
            "file:///test.tw",
            50..53,
            "",
        );

        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.accesses.len(), 2);
        assert!(entry.first_write().is_some());
        assert_eq!(entry.first_write().unwrap().passage_name, "Start");
    }

    #[test]
    fn property_tracking() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$player",
            false,
            true,
            "Init",
            "file:///test.tw",
            10..17,
            "name",
        );
        tree.record_var(
            "$player",
            false,
            true,
            "Init",
            "file:///test.tw",
            20..27,
            "hp",
        );

        let entry = tree.get_variable("$player").unwrap();
        assert!(entry.known_properties.contains("name"));
        assert!(entry.known_properties.contains("hp"));
    }

    #[test]
    fn temporary_variable() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "_i",
            true,
            true,
            "Loop",
            "file:///test.tw",
            5..7,
            "",
        );

        let entry = tree.get_variable("_i").unwrap();
        assert!(entry.is_temporary);
    }

    #[test]
    fn seeded_by_special() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$gold",
            false,
            true,
            "StoryInit",
            "file:///test.tw",
            10..15,
            "",
        );
        tree.mark_seeded("$gold");

        let entry = tree.get_variable("$gold").unwrap();
        assert!(entry.seeded_by_special);
    }

    #[test]
    fn build_tree() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            true,
            "Start",
            "file:///test.tw",
            10..13,
            "",
        );
        tree.record_var(
            "_i",
            true,
            true,
            "Loop",
            "file:///test.tw",
            5..7,
            "",
        );

        let nodes = tree.build_tree();
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn remove_file() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            true,
            "Start",
            "file:///a.tw",
            10..13,
            "",
        );
        tree.record_var(
            "$hp",
            false,
            false,
            "Forest",
            "file:///b.tw",
            50..53,
            "",
        );

        tree.remove_file("file:///a.tw");
        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.accesses.len(), 1);
        assert_eq!(entry.accesses[0].passage_name, "Forest");
    }
}
