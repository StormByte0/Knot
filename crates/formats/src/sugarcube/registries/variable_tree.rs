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
//! ## Normalized keys
//!
//! All variables are stored using their **normalized key**: `$foo` for story
//! variables, `_bar` for temporary variables. Both the SugarCube shorthand
//! (`$foo`) and the JavaScript API form (`State.variables.foo`) are unified
//! to the same normalized key. The `state_path` field on `VarEntry` provides
//! the full `State.variables.foo` annotation for display and hover.
//!
//! ## Read vs Write classification
//!
//! JavaScript does not have a variable initialization concept — variables are
//! simply read or written. The registry classifies each access:
//!
//! - **Writes**: `<<set>>` macros, assignment operators (`=`, `+=`, etc.),
//!   `<<capture>>`, `<<run>>` with assignment expressions, postfix `++`/`--`,
//!   and `State.variables.x = value` in JS.
//! - **Reads**: Condition checks (`<<if>>`, `<<elseif>>`), output macros
//!   (`<<print>>`, `<<=>>`), bare `$var` in text, `State.variables.x` in
//!   read positions in JS, and any variable reference that does not modify
//!   the variable's value.
//!
//! ## Graph-order indexing
//!
//! Reads and writes are indexed in reachability order: special passages first
//! (StoryInit, [script], [init]), then the Start passage, then downstream
//! passages via BFS. This ordering enables correct variable availability
//! analysis — a write in StoryInit makes a variable available everywhere,
//! while a write in a downstream passage only makes it available to passages
//! reachable from that point.
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
// VarAccessKind — read vs write classification
// ---------------------------------------------------------------------------

/// The kind of variable access, distinguishing reads from writes with nuance.
///
/// JavaScript doesn't have a variable initialization concept — variables are
/// simply read or written. This enum captures that distinction precisely:
///
/// - **Write variants**: Any access that modifies the variable's value
/// - **Read variants**: Any access that observes the variable's value without
///   modifying it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarAccessKind {
    /// Variable is being read without modification (e.g., `$hp` in text,
    /// `<<if $alive>>`, `<<print $gold>>`, `State.variables.x` in read position).
    Read,
    /// Variable is being assigned a new value (e.g., `<<set $hp to 100>>`,
    /// `<<set $hp = 100>>`, `State.variables.hp = 100`).
    Write,
    /// Variable is being modified via compound assignment (e.g., `<<set $hp += 10>>`,
    /// `<<set $hp -= 5>>`, `<<set $hp *= 2>>`). These are both a read AND a write —
    /// the current value is read, then a new value is written.
    CompoundWrite,
    /// Variable is being modified via postfix increment/decrement (e.g., `<<set $hp++>>`,
    /// `<<set $hp-->>`). Like `CompoundWrite`, this is both a read and a write.
    PostfixModify,
    /// Variable is being captured (e.g., `<<capture $x>>`). The variable is
    /// read to establish the capture context, and a local binding is written.
    Capture,
    /// Variable is being unset (e.g., `<<unset $hp>>`). Removes the variable
    /// from state — this is semantically a write (it changes state), but the
    /// variable's value is not set to a new value.
    Unset,
}

impl VarAccessKind {
    /// Whether this access kind writes to the variable (any write variant).
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            VarAccessKind::Write
                | VarAccessKind::CompoundWrite
                | VarAccessKind::PostfixModify
                | VarAccessKind::Capture
                | VarAccessKind::Unset
        )
    }

    /// Whether this access kind reads the variable (any read variant, including
    /// compound writes which read the current value before writing).
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            VarAccessKind::Read
                | VarAccessKind::CompoundWrite
                | VarAccessKind::PostfixModify
                | VarAccessKind::Capture
        )
    }
}

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
    /// The kind of access: Read, Write, CompoundWrite, PostfixModify, Capture, Unset.
    ///
    /// This replaces the old `is_write: bool` with a nuanced classification
    /// that captures compound assignments (`+=`), postfix operators (`++`),
    /// captures, and unsets as distinct access kinds.
    pub kind: VarAccessKind,
    /// Whether this is a temporary variable (`_` sigil).
    pub is_temporary: bool,
    /// The 0-based line number within the file (0 until computed by line mapping).
    pub line: u32,
    /// The graph-order index of the passage where this access occurs.
    /// Special passages get index 0, Start gets index 1, and downstream
    /// passages get incrementing indices via BFS. `None` until computed
    /// by `compute_graph_order()`.
    pub graph_order: Option<u32>,
}

impl VarAccess {
    /// Backward-compatible: whether this access is any kind of write.
    pub fn is_write(&self) -> bool {
        self.kind.is_write()
    }

    /// Backward-compatible: whether this access is any kind of read.
    pub fn is_read(&self) -> bool {
        self.kind.is_read()
    }
}

// ---------------------------------------------------------------------------
// VarEntry — all known info about a single variable
// ---------------------------------------------------------------------------

/// All tracked information about a single variable across the workspace.
#[derive(Debug, Clone)]
pub struct VarEntry {
    /// The variable name in normalized form including sigil (e.g., `$hp`, `_i`).
    /// Both `$foo` references and `State.variables.foo` references are unified
    /// to the same `$foo` normalized key.
    pub name: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
    /// All recorded accesses to this variable.
    pub accesses: Vec<VarAccess>,
    /// Known dot-notation property paths (e.g., `{"name", "hp"}` for `$player`).
    pub known_properties: HashSet<String>,
    /// Whether this variable is seeded by a special passage (StoryInit, [script]).
    pub seeded_by_special: bool,
    /// The full `State.variables.*` path annotation for this variable.
    /// For story variables (`$foo`): `"State.variables.foo"`
    /// For temporary variables (`_bar`): `"State.temporary.bar"`
    /// This is computed from the normalized name for display and hover.
    pub state_path: String,
}

impl VarEntry {
    /// Create a new entry for a variable.
    ///
    /// The `state_path` is computed from the normalized name: `$foo` becomes
    /// `State.variables.foo`, `_bar` becomes `State.temporary.bar`.
    pub fn new(name: String, is_temporary: bool) -> Self {
        let state_path = compute_state_path(&name, is_temporary);
        Self {
            name,
            is_temporary,
            accesses: Vec::new(),
            known_properties: HashSet::new(),
            seeded_by_special: false,
            state_path,
        }
    }

    /// Get the first write location, if any, respecting graph order.
    /// If graph_order has been computed, returns the write with the lowest
    /// graph_order. Otherwise, returns the first write in insertion order.
    pub fn first_write(&self) -> Option<&VarAccess> {
        let writes: Vec<&VarAccess> = self.accesses.iter().filter(|a| a.is_write()).collect();
        if writes.is_empty() {
            return None;
        }
        // If graph_order is available, use it; otherwise fall back to insertion order
        writes.iter().min_by(|a, b| {
            let order_a = a.graph_order.unwrap_or(u32::MAX);
            let order_b = b.graph_order.unwrap_or(u32::MAX);
            order_a.cmp(&order_b)
        }).copied()
    }

    /// Get all write locations.
    pub fn writes(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| a.is_write()).collect()
    }

    /// Get all read locations.
    pub fn reads(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| a.is_read()).collect()
    }

    /// Record a variable access.
    pub fn record_access(&mut self, access: VarAccess) {
        self.accesses.push(access);
    }

    /// Record a property path seen on this variable.
    pub fn record_property(&mut self, property_path: String) {
        self.known_properties.insert(property_path);
    }
}

/// Compute the `State.variables.*` / `State.temporary.*` path from a
/// normalized variable name.
///
/// - `$foo` → `State.variables.foo`
/// - `_bar` → `State.temporary.bar`
fn compute_state_path(name: &str, is_temporary: bool) -> String {
    if is_temporary {
        let bare = name.trim_start_matches('_');
        format!("State.temporary.{}", bare)
    } else {
        let bare = name.trim_start_matches('$');
        format!("State.variables.{}", bare)
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
    ///
    /// The `kind` parameter provides nuanced read/write classification:
    /// - `VarAccessKind::Write` for simple assignments (`<<set $x to 1>>`)
    /// - `VarAccessKind::CompoundWrite` for compound ops (`<<set $x += 1>>`)
    /// - `VarAccessKind::PostfixModify` for postfix (`<<set $x++>>`)
    /// - `VarAccessKind::Capture` for `<<capture $x>>`
    /// - `VarAccessKind::Unset` for `<<unset $x>>`
    /// - `VarAccessKind::Read` for any read without modification
    #[allow(clippy::too_many_arguments)]
    pub fn record_var(
        &mut self,
        name: &str,
        is_temporary: bool,
        kind: VarAccessKind,
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
            kind,
            is_temporary,
            line: 0, // Line computed later by SourceTextProvider
            graph_order: None, // Computed later by compute_graph_order()
        });

        if !property_path.is_empty() {
            entry.record_property(property_path.to_string());
        }
    }

    /// Backward-compatible convenience: record a variable access with a
    /// simple read/write boolean. Maps `is_write=true` → `VarAccessKind::Write`,
    /// `is_write=false` → `VarAccessKind::Read`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_var_simple(
        &mut self,
        name: &str,
        is_temporary: bool,
        is_write: bool,
        passage_name: &str,
        file_uri: &str,
        span: Range<usize>,
        property_path: &str,
    ) {
        let kind = if is_write {
            VarAccessKind::Write
        } else {
            VarAccessKind::Read
        };
        self.record_var(name, is_temporary, kind, passage_name, file_uri, span, property_path);
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

    /// Remove all accesses for a specific passage (for incremental single-passage re-parse).
    ///
    /// Unlike `remove_file()`, this keeps the variable entry alive if it has
    /// accesses in other passages or known properties. The variable will be
    /// re-populated when the passage is re-parsed.
    pub fn remove_passage(&mut self, passage_name: &str) {
        for entry in self.variables.values_mut() {
            entry.accesses.retain(|a| a.passage_name != passage_name);
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

            let is_unused = !entry.accesses.iter().any(|a| a.is_read() && !a.is_write());

            // Build property nodes from known_properties
            let properties: Vec<VariablePropertyNode> = entry
                .known_properties
                .iter()
                .map(|prop| VariablePropertyNode {
                    name: prop.clone(),
                    full_name: format!("{}.{}", entry.name, prop),
                    state_path: format!("{}.{}", entry.state_path, prop),
                    line: 0,
                    written_in: Vec::new(),
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
                state_path: entry.state_path.clone(),
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

    /// Iterate over all variable entries in the tree.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &VarEntry)> {
        self.variables.iter()
    }

    /// Compute graph-order indices for all variable accesses.
    ///
    /// Assigns each access a `graph_order` based on reachability:
    /// - Index 0: Special passages (StoryInit, [script], [init], etc.)
    /// - Index 1: Start passage
    /// - Index 2+: Downstream passages in BFS order from Start
    ///
    /// The `special_passages` set contains passage names that are special
    /// (Startup behavior, [script] tagged, etc.). The `start_passage` is the
    /// story's starting passage (usually "Start"). The `bfs_order` provides
    /// passage names in BFS traversal order from Start (excluding special
    /// passages which always come first).
    ///
    /// This must be called after all passages have been parsed and the graph
    /// has been built. Until this is called, `graph_order` on each `VarAccess`
    /// remains `None`.
    pub fn compute_graph_order(
        &mut self,
        special_passages: &HashSet<String>,
        start_passage: &str,
        bfs_order: &[String],
    ) {
        // Build a passage name → order index map
        let mut passage_order: HashMap<String, u32> = HashMap::new();
        let mut next_order: u32 = 0;

        // Special passages get order 0 (all special passages share order 0
        // since they all execute before the story starts)
        for name in special_passages {
            passage_order.insert(name.clone(), 0);
        }

        // Start passage gets order 1
        if !start_passage.is_empty() {
            next_order = 1;
            passage_order.insert(start_passage.to_string(), next_order);
            next_order += 1;
        }

        // Downstream passages get incrementing order from BFS
        for passage_name in bfs_order {
            if !passage_order.contains_key(passage_name) {
                passage_order.insert(passage_name.clone(), next_order);
                next_order += 1;
            }
        }

        // Assign graph_order to each VarAccess
        for entry in self.variables.values_mut() {
            for access in &mut entry.accesses {
                access.graph_order = passage_order.get(&access.passage_name).copied();
            }

            // Sort accesses by graph_order for deterministic ordering
            entry.accesses.sort_by(|a, b| {
                let order_a = a.graph_order.unwrap_or(u32::MAX);
                let order_b = b.graph_order.unwrap_or(u32::MAX);
                order_a.cmp(&order_b).then_with(|| a.span.start.cmp(&b.span.start))
            });
        }
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
            VarAccessKind::Write,
            "Start",
            "file:///test.tw",
            10..13,
            "",
        );
        tree.record_var(
            "$hp",
            false,
            VarAccessKind::Read,
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
    fn state_path_annotation() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            VarAccessKind::Write,
            "Start",
            "file:///test.tw",
            10..13,
            "",
        );
        tree.record_var(
            "_i",
            true,
            VarAccessKind::Write,
            "Loop",
            "file:///test.tw",
            5..7,
            "",
        );

        let story_entry = tree.get_variable("$hp").unwrap();
        assert_eq!(story_entry.state_path, "State.variables.hp");

        let temp_entry = tree.get_variable("_i").unwrap();
        assert_eq!(temp_entry.state_path, "State.temporary.i");
    }

    #[test]
    fn compound_write_classification() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            VarAccessKind::CompoundWrite,
            "Forest",
            "file:///test.tw",
            10..17,
            "",
        );

        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.accesses.len(), 1);
        // CompoundWrite is both a read and a write
        assert!(entry.accesses[0].is_write());
        assert!(entry.accesses[0].is_read());
        assert_eq!(entry.writes().len(), 1);
        assert_eq!(entry.reads().len(), 1);
    }

    #[test]
    fn property_tracking() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$player",
            false,
            VarAccessKind::Write,
            "Init",
            "file:///test.tw",
            10..17,
            "name",
        );
        tree.record_var(
            "$player",
            false,
            VarAccessKind::Write,
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
            VarAccessKind::Write,
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
            VarAccessKind::Write,
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
            VarAccessKind::Write,
            "Start",
            "file:///test.tw",
            10..13,
            "",
        );
        tree.record_var(
            "_i",
            true,
            VarAccessKind::Write,
            "Loop",
            "file:///test.tw",
            5..7,
            "",
        );

        let nodes = tree.build_tree();
        assert_eq!(nodes.len(), 2);
        // Verify state_path in tree output
        let hp_node = nodes.iter().find(|n| n.name == "$hp").unwrap();
        assert_eq!(hp_node.state_path, "State.variables.hp");
        let i_node = nodes.iter().find(|n| n.name == "_i").unwrap();
        assert_eq!(i_node.state_path, "State.temporary.i");
    }

    #[test]
    fn remove_file() {
        let mut tree = VariableTree::new();
        tree.record_var(
            "$hp",
            false,
            VarAccessKind::Write,
            "Start",
            "file:///a.tw",
            10..13,
            "",
        );
        tree.record_var(
            "$hp",
            false,
            VarAccessKind::Read,
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

    #[test]
    fn graph_order_indexing() {
        let mut tree = VariableTree::new();
        // StoryInit writes $gold (special passage → order 0)
        tree.record_var(
            "$gold",
            false,
            VarAccessKind::Write,
            "StoryInit",
            "file:///test.tw",
            10..15,
            "",
        );
        // Start reads $gold (order 1)
        tree.record_var(
            "$gold",
            false,
            VarAccessKind::Read,
            "Start",
            "file:///test.tw",
            20..25,
            "",
        );
        // Forest writes $gold (BFS order → order 2)
        tree.record_var(
            "$gold",
            false,
            VarAccessKind::CompoundWrite,
            "Forest",
            "file:///test.tw",
            30..35,
            "",
        );

        // Compute graph order
        let special: HashSet<String> = ["StoryInit".to_string()].into_iter().collect();
        tree.compute_graph_order(&special, "Start", &["Forest".to_string()]);

        let entry = tree.get_variable("$gold").unwrap();
        // Accesses should be sorted by graph_order
        assert_eq!(entry.accesses[0].graph_order, Some(0)); // StoryInit
        assert_eq!(entry.accesses[1].graph_order, Some(1)); // Start
        assert_eq!(entry.accesses[2].graph_order, Some(2)); // Forest

        // first_write should return the StoryInit write (order 0)
        assert_eq!(entry.first_write().unwrap().passage_name, "StoryInit");
    }
}
