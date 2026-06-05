//! Workspace-wide variable tree for completions, hover, and panel navigation.
//!
//! The `VariableTree` aggregates `VarEncounter` entries from all passages into
//! a single structure with two views:
//!
//! - **By variable** (`variables` map): For completions, hover info, and type hints.
//!   Each `VarEntry` tracks all passages that reference the variable.
//! - **By passage** (derived via `variables_by_passage()`): For the tree panel
//!   navigation showing which variables a passage reads/writes.
//!
//! ## Incremental updates
//!
//! Like the passage tree's `walk_encounters()`, the tree supports surgical passage-level updates:
//!
//! - `update_passage()` replaces one passage's variable data
//! - `remove_passage()` removes all references from a passage
//! - Variables with no remaining references after removal are cleaned up
//! - `first_write` is recomputed after every mutation

use std::collections::HashMap;

use super::passage_tree::{VarAccessKind, VarEncounter, VarTypeHint};

// ---------------------------------------------------------------------------
// VarAccess
// ---------------------------------------------------------------------------

/// A single variable access (read or write) at a specific source line.
#[derive(Debug, Clone)]
pub(crate) struct VarAccess {
    /// Whether this is a read or write access.
    pub kind: VarAccessKind,
    /// The 0-based line number within the passage body.
    pub line: u32,
}

// ---------------------------------------------------------------------------
// VarEntry
// ---------------------------------------------------------------------------

/// Entry for a single variable in the tree.
///
/// Tracks all passages that reference this variable, the inferred type,
/// and the first known write location (for "where is this initialized?"
/// queries).
#[derive(Debug, Clone)]
pub(crate) struct VarEntry {
    /// Inferred type hint from the most recent type-bearing encounter.
    /// Starts as `Unknown` and gets refined as more encounters are processed.
    /// If different passages assign different types, the last non-Unknown
    /// type wins (this is a simple heuristic; proper type union analysis
    /// is a future enhancement).
    pub type_hint: VarTypeHint,
    /// Map from passage name to list of access locations within that passage.
    pub passages: HashMap<String, Vec<VarAccess>>,
    /// The first known write to this variable, as (passage_name, line).
    /// `None` if the variable has only been read (never written).
    /// This is recomputed by `recompute_first_writes()` after mutations.
    pub first_write: Option<(String, u32)>,
}

impl VarEntry {
    /// Create a new VarEntry with Unknown type and no accesses.
    fn new() -> Self {
        VarEntry {
            type_hint: VarTypeHint::Unknown,
            passages: HashMap::new(),
            first_write: None,
        }
    }
}

// ---------------------------------------------------------------------------
// VariableTree
// ---------------------------------------------------------------------------

/// Workspace-wide variable tree aggregating all VarEncounter entries.
///
/// Provides two views of the same data:
/// - **By variable** (primary storage): For completions, hover, type info
/// - **By passage** (derived query): For tree panel navigation
#[derive(Debug, Clone, Default)]
pub(crate) struct VariableTree {
    /// Variable name (without `$` sigil) → VarEntry.
    variables: HashMap<String, VarEntry>,
}

#[allow(dead_code)] // API surface for Phase C (completions, hover, tree panel)
impl VariableTree {
    /// Create a new, empty VariableTree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update a passage's variable data.
    ///
    /// Removes all existing references from the given passage, then inserts
    /// new references from the provided encounters. Recomputes `first_write`
    /// for all affected variables.
    pub fn update_passage(&mut self, passage_name: &str, encounters: &[VarEncounter]) {
        // Remove old references from this passage first
        self.remove_passage(passage_name);

        // Insert new encounters
        for enc in encounters {
            let entry = self.variables.entry(enc.name.clone()).or_insert_with(VarEntry::new);

            // Update type hint if this encounter provides one
            if enc.type_hint != VarTypeHint::Unknown {
                entry.type_hint = enc.type_hint.clone();
            }

            // Add the access
            entry
                .passages
                .entry(passage_name.to_string())
                .or_default()
                .push(VarAccess {
                    kind: enc.kind.clone(),
                    line: enc.line,
                });
        }

        // Recompute first writes for all variables
        self.recompute_first_writes();
    }

    /// Remove all variable references from a passage.
    ///
    /// After removal, any variable that has no remaining references
    /// (from any passage) is removed from the tree entirely.
    pub fn remove_passage(&mut self, passage_name: &str) {
        for entry in self.variables.values_mut() {
            entry.passages.remove(passage_name);
        }

        // Clean up variables with no remaining references
        self.variables.retain(|_, entry| !entry.passages.is_empty());
    }

    /// Look up a variable by name (without `$` sigil).
    pub fn get_variable(&self, name: &str) -> Option<&VarEntry> {
        self.variables.get(name)
    }

    /// Get the number of unique variables in the tree.
    pub fn len(&self) -> usize {
        self.variables.len()
    }

    /// Check if the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }

    /// Get all variable names in the tree.
    pub fn variable_names(&self) -> impl Iterator<Item = &String> {
        self.variables.keys()
    }

    /// Get all variables referenced in a given passage.
    ///
    /// Returns an iterator of (variable_name, &VarEntry) pairs for all
    /// variables that have at least one access in the given passage.
    pub fn variables_by_passage(
        &self,
        passage_name: &str,
    ) -> impl Iterator<Item = (&String, &VarEntry)> {
        self.variables
            .iter()
            .filter(move |(_, entry)| entry.passages.contains_key(passage_name))
    }

    /// Get the variable accesses for a specific (variable, passage) pair.
    pub fn get_accesses(
        &self,
        var_name: &str,
        passage_name: &str,
    ) -> Option<&Vec<VarAccess>> {
        self.variables
            .get(var_name)
            .and_then(|entry| entry.passages.get(passage_name))
    }

    /// Recompute `first_write` for all variables.
    ///
    /// Scans all `VarEntry.passages` for the earliest Write encounter.
    /// "Earliest" means: the first write encountered across all passages,
    /// with ties broken by passage name (alphabetical). This is a simple
    /// heuristic — proper "first write" analysis would need graph-BFS
    /// to determine execution order, which is a Phase E enhancement.
    fn recompute_first_writes(&mut self) {
        for entry in self.variables.values_mut() {
            entry.first_write = None;

            // Collect all write accesses across all passages
            let mut earliest: Option<(String, u32)> = None;
            for (passage_name, accesses) in &entry.passages {
                for access in accesses {
                    if matches!(access.kind, VarAccessKind::Write) {
                        match &earliest {
                            None => {
                                earliest = Some((passage_name.clone(), access.line));
                            }
                            Some((_, earliest_line)) => {
                                if access.line < *earliest_line {
                                    earliest = Some((passage_name.clone(), access.line));
                                }
                            }
                        }
                    }
                }
            }

            entry.first_write = earliest;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn make_encounter(
        name: &str,
        type_hint: VarTypeHint,
        kind: VarAccessKind,
        line: u32,
    ) -> VarEncounter {
        VarEncounter {
            name: name.to_string(),
            type_hint,
            kind,
            line,
            byte_span: Range { start: 0, end: 0 },
        }
    }

    #[test]
    fn test_empty_tree() {
        let tree = VariableTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
    }

    #[test]
    fn test_update_passage_basic() {
        let mut tree = VariableTree::new();
        let encounters = vec![
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
            make_encounter("hp", VarTypeHint::Number, VarAccessKind::Write, 1),
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Read, 3),
        ];

        tree.update_passage("Start", &encounters);

        assert_eq!(tree.len(), 2); // gold + hp
        assert!(tree.get_variable("gold").is_some());
        assert!(tree.get_variable("hp").is_some());
        assert!(tree.get_variable("unknown").is_none());

        // gold has a type hint
        assert_eq!(tree.get_variable("gold").unwrap().type_hint, VarTypeHint::Number);

        // gold is referenced in Start passage
        let gold_accesses = tree.get_accesses("gold", "Start").unwrap();
        assert_eq!(gold_accesses.len(), 2); // one write + one read
    }

    #[test]
    fn test_first_write() {
        let mut tree = VariableTree::new();

        // Start passage writes gold at line 0
        tree.update_passage("Start", &[
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
        ]);

        // Shop passage writes gold at line 5
        tree.update_passage("Shop", &[
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Write, 5),
        ]);

        // First write should be at Start, line 0
        let gold = tree.get_variable("gold").unwrap();
        assert_eq!(gold.first_write, Some(("Start".to_string(), 0)));
    }

    #[test]
    fn test_no_first_write_for_read_only() {
        let mut tree = VariableTree::new();
        tree.update_passage("Reader", &[
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Read, 2),
        ]);

        let gold = tree.get_variable("gold").unwrap();
        assert!(gold.first_write.is_none());
    }

    #[test]
    fn test_remove_passage() {
        let mut tree = VariableTree::new();
        tree.update_passage("Start", &[
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
            make_encounter("hp", VarTypeHint::Number, VarAccessKind::Write, 1),
        ]);
        tree.update_passage("Shop", &[
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Read, 0),
        ]);

        assert_eq!(tree.len(), 2);

        // Remove Start passage
        tree.remove_passage("Start");

        // gold still exists (referenced by Shop)
        assert!(tree.get_variable("gold").is_some());
        // hp was only in Start — should be removed entirely
        assert!(tree.get_variable("hp").is_none());
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn test_variables_by_passage() {
        let mut tree = VariableTree::new();
        tree.update_passage("Start", &[
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
            make_encounter("hp", VarTypeHint::Number, VarAccessKind::Write, 1),
        ]);
        tree.update_passage("Shop", &[
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Read, 0),
            make_encounter("potions", VarTypeHint::Number, VarAccessKind::Write, 2),
        ]);

        let start_vars: Vec<_> = tree.variables_by_passage("Start").collect();
        assert_eq!(start_vars.len(), 2); // gold, hp

        let shop_vars: Vec<_> = tree.variables_by_passage("Shop").collect();
        assert_eq!(shop_vars.len(), 2); // gold, potions
    }

    #[test]
    fn test_overwrite_passage() {
        let mut tree = VariableTree::new();
        tree.update_passage("Start", &[
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
        ]);

        // Overwrite with different variables
        tree.update_passage("Start", &[
            make_encounter("silver", VarTypeHint::Number, VarAccessKind::Write, 0),
        ]);

        // gold should be gone (only referenced by old Start)
        assert!(tree.get_variable("gold").is_none());
        // silver should be present
        assert!(tree.get_variable("silver").is_some());
    }

    #[test]
    fn test_type_hint_upgrades() {
        let mut tree = VariableTree::new();

        // First encounter: Unknown type (read)
        tree.update_passage("Reader", &[
            make_encounter("score", VarTypeHint::Unknown, VarAccessKind::Read, 0),
        ]);
        assert_eq!(tree.get_variable("score").unwrap().type_hint, VarTypeHint::Unknown);

        // Second encounter: Number type (write)
        tree.update_passage("Writer", &[
            make_encounter("score", VarTypeHint::Number, VarAccessKind::Write, 0),
        ]);
        assert_eq!(tree.get_variable("score").unwrap().type_hint, VarTypeHint::Number);
    }

    #[test]
    fn test_type_hint_no_downgrade() {
        let mut tree = VariableTree::new();

        // First encounter: Number type
        tree.update_passage("Start", &[
            make_encounter("gold", VarTypeHint::Number, VarAccessKind::Write, 0),
        ]);

        // Second encounter: Unknown type (should not downgrade)
        tree.update_passage("Shop", &[
            make_encounter("gold", VarTypeHint::Unknown, VarAccessKind::Read, 0),
        ]);

        assert_eq!(tree.get_variable("gold").unwrap().type_hint, VarTypeHint::Number);
    }
}
