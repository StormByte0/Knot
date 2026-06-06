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
//! ## Hierarchical tree structure
//!
//! Variables form a tree that mirrors the runtime state hierarchy. For SugarCube,
//! `$player.hp.max` is represented as:
//!
//! ```text
//! $player          (root VarEntry)
//!   └─ hp          (child VarNode)
//!       └─ max     (child VarNode)
//! ```
//!
//! Each node in the tree has its own `accesses` list. When an operation targets
//! a leaf node (e.g., `$player.hp.max = 100`), the access is recorded at the
//! **actual target node** and **propagated** up to all ancestor nodes. This means
//! a write to `$player.hp.max` also counts as a write to `$player.hp` and
//! `$player` — because modifying a property inherently modifies the parent object.
//!
//! Propagated accesses are marked with `propagated: true` on the `VarAccess`
//! record so consumers can distinguish direct vs inferred accesses when needed.
//!
//! ## Normalized keys
//!
//! All variables are stored using their **normalized key**: `$foo` for story
//! variables, `_bar` for temporary variables. Both the SugarCube shorthand
//! (`$foo`) and the JavaScript API form (`State.variables.foo`) are unified
//! to the same normalized key.
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
//! ## Thread safety
//!
//! The tree is wrapped in a `RwLock` inside `SugarCubePlugin`. Multiple readers
//! (completion, hover, references) can access it concurrently. Only the parse
//! pipeline needs a write lock.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::plugin::SourceTextProvider;
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
    /// Byte range of the variable reference, **relative to the passage body start**.
    ///
    /// This is the offset within the passage's body text (the content after the
    /// `:: PassageName` header line), not within the full document. At the LSP
    /// output boundary, `PassagePosition::body_start_offset` is added to convert
    /// to document-absolute positions.
    pub span: Range<usize>,
    /// The kind of access: Read, Write, CompoundWrite, PostfixModify, Capture, Unset.
    pub kind: VarAccessKind,
    /// Whether this is a temporary variable (`_` sigil).
    pub is_temporary: bool,
    /// The 0-based line number **relative to the passage body start**.
    ///
    /// Line 0 is the first line of passage body content (the line immediately
    /// after the `:: PassageName` header). At the LSP output boundary,
    /// `PassagePosition::body_start_line` is added to convert to a
    /// document-absolute line number.
    pub line: u32,
    /// The graph-order index of the passage where this access occurs.
    /// `None` until computed by `compute_graph_order()`.
    pub graph_order: Option<u32>,
    /// Whether this access was propagated from a descendant node.
    ///
    /// When `$player.hp.max = 100` is written, the access is recorded directly
    /// on the `max` node and **propagated** up to `hp` and `player`. Propagated
    /// accesses carry the same passage/span/line info but are marked so consumers
    /// can distinguish "this variable was directly written" from "a child property
    /// of this variable was written".
    pub propagated: bool,
}

impl VarAccess {
    /// Whether this access is any kind of write.
    pub fn is_write(&self) -> bool {
        self.kind.is_write()
    }

    /// Whether this access is any kind of read.
    pub fn is_read(&self) -> bool {
        self.kind.is_read()
    }

    /// Create a propagated copy of this access.
    ///
    /// Propagated copies inherit all fields except `propagated` which is set
    /// to `true`. They represent the same source-level operation but are
    /// attached to an ancestor node to indicate that the ancestor was also
    /// affected by the operation.
    fn as_propagated(&self) -> VarAccess {
        VarAccess {
            passage_name: self.passage_name.clone(),
            file_uri: self.file_uri.clone(),
            span: self.span.clone(),
            kind: self.kind,
            is_temporary: self.is_temporary,
            line: self.line,
            graph_order: self.graph_order,
            propagated: true,
        }
    }
}

// ---------------------------------------------------------------------------
// VarNode — a node in the variable property tree
// ---------------------------------------------------------------------------

/// A node in the variable property tree.
///
/// Each `VarNode` represents one segment of a dot-path. The root node for
/// `$player` has the name "player" and may contain child nodes like "hp",
/// "name", etc. A child node for `$player.hp` has the name "hp" and may
/// itself contain children (e.g., "max" for `$player.hp.max`).
///
/// Every node carries its own `accesses` list — both direct accesses (where
/// this exact path was referenced in source) and propagated accesses (where
/// a descendant was accessed, which inherently affects this node too).
#[derive(Debug, Clone)]
pub struct VarNode {
    /// The single segment name of this node (e.g., "hp" for `$player.hp`).
    /// The root node's name includes the sigil (e.g., "$player").
    pub name: String,
    /// Whether this is a temporary variable (`_` sigil). Only meaningful on root.
    pub is_temporary: bool,
    /// All recorded accesses at this node (direct + propagated from children).
    pub accesses: Vec<VarAccess>,
    /// Child property nodes, keyed by property segment name.
    pub children: HashMap<String, VarNode>,
}

impl VarNode {
    fn new(name: String, is_temporary: bool) -> Self {
        Self {
            name,
            is_temporary,
            accesses: Vec::new(),
            children: HashMap::new(),
        }
    }

    /// Record a direct access on this node.
    fn record_access(&mut self, access: VarAccess) {
        self.accesses.push(access);
    }

    /// Record a propagated access on this node (from a child operation).
    fn record_propagated(&mut self, access: &VarAccess) {
        self.accesses.push(access.as_propagated());
    }

    /// Get all direct (non-propagated) write locations.
    pub fn direct_writes(&self) -> Vec<&VarAccess> {
        self.accesses
            .iter()
            .filter(|a| a.is_write() && !a.propagated)
            .collect()
    }

    /// Get all direct (non-propagated) read locations.
    pub fn direct_reads(&self) -> Vec<&VarAccess> {
        self.accesses
            .iter()
            .filter(|a| a.is_read() && !a.propagated)
            .collect()
    }

    /// Get all write locations (direct + propagated).
    pub fn all_writes(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| a.is_write()).collect()
    }

    /// Get all read locations (direct + propagated).
    pub fn all_reads(&self) -> Vec<&VarAccess> {
        self.accesses.iter().filter(|a| a.is_read()).collect()
    }

    /// Whether this node or any descendant has a direct write.
    #[allow(dead_code)]
    fn has_any_direct_write(&self) -> bool {
        if self.accesses.iter().any(|a| a.is_write() && !a.propagated) {
            return true;
        }
        self.children.values().any(|c| c.has_any_direct_write())
    }
}

// ---------------------------------------------------------------------------
// VarEntry — public accessor for root-level variable info
// ---------------------------------------------------------------------------

/// All tracked information about a single root variable across the workspace.
///
/// This is a convenience wrapper that provides the same public API as before,
/// backed by the hierarchical `VarNode`. It allows callers to query a root
/// variable by name without needing to understand the tree structure.
#[derive(Debug, Clone)]
pub struct VarEntry {
    /// The variable name in normalized form including sigil (e.g., `$hp`, `_i`).
    pub name: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
    /// Whether this variable is seeded by a special passage (StoryInit, [script]).
    pub seeded_by_special: bool,
    /// The full `State.variables.*` path annotation for this variable.
    pub state_path: String,
    /// The root node of this variable's property tree.
    pub node: VarNode,
}

impl VarEntry {
    /// Create a new entry for a variable.
    pub fn new(name: String, is_temporary: bool) -> Self {
        let state_path = compute_state_path(&name, is_temporary);
        let node = VarNode::new(name.clone(), is_temporary);
        Self {
            name,
            is_temporary,
            seeded_by_special: false,
            state_path,
            node,
        }
    }

    /// Get the first write location, if any, respecting graph order.
    /// Considers both direct and propagated accesses on the root node.
    pub fn first_write(&self) -> Option<&VarAccess> {
        let writes: Vec<&VarAccess> = self.node.accesses.iter().filter(|a| a.is_write()).collect();
        if writes.is_empty() {
            return None;
        }
        writes.iter().min_by(|a, b| {
            let order_a = a.graph_order.unwrap_or(u32::MAX);
            let order_b = b.graph_order.unwrap_or(u32::MAX);
            order_a.cmp(&order_b)
        }).copied()
    }

    /// Get all write locations on the root node (direct + propagated).
    pub fn writes(&self) -> Vec<&VarAccess> {
        self.node.all_writes()
    }

    /// Get all read locations on the root node (direct + propagated).
    pub fn reads(&self) -> Vec<&VarAccess> {
        self.node.all_reads()
    }

    /// Record a variable access on the root node (backward compat).
    pub fn record_access(&mut self, access: VarAccess) {
        self.node.record_access(access);
    }

    /// Record a property path (backward compat — ensures child node exists).
    pub fn record_property(&mut self, property_path: String) {
        self.ensure_path(&property_path);
    }

    /// Ensure a child path exists in the tree, creating intermediate nodes.
    fn ensure_path(&mut self, path: &str) {
        let segments: Vec<&str> = path.split('.').collect();
        let mut current = &mut self.node;
        for segment in &segments {
            current = current
                .children
                .entry(segment.to_string())
                .or_insert_with(|| VarNode::new(segment.to_string(), self.is_temporary));
        }
    }

    /// Backward compat: accessor for root node's access list.
    /// Prefer using `node.accesses` directly for new code.
    pub fn accesses(&self) -> &[VarAccess] {
        &self.node.accesses
    }

    /// Backward compat: accessor for known property paths.
    /// Derived from the node's children — immediate child names only.
    pub fn known_properties(&self) -> HashSet<String> {
        self.node.children.keys().cloned().collect()
    }
}

/// Compute the `State.variables.*` / `State.temporary.*` path from a
/// normalized variable name.
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
/// references across all parsed passages. Each `VarEntry` contains a
/// hierarchical `VarNode` tree that mirrors the runtime state structure.
///
/// Updated incrementally during the parse pipeline.
#[derive(Debug, Clone, Default)]
pub struct VariableTree {
    /// Map of root variable name (including sigil) → entry.
    variables: HashMap<String, VarEntry>,
}

impl VariableTree {
    /// Create an empty variable tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a variable access from a passage.
    ///
    /// The access is recorded at the **exact node** specified by the
    /// `name` + `property_path` combination, then **propagated** up to
    /// all ancestor nodes. For example, recording a write to `$player`
    /// with `property_path = "hp.max"` will:
    ///
    /// 1. Record the write on the `max` node (direct access)
    /// 2. Propagate a write to the `hp` node (propagated access)
    /// 3. Propagate a write to the `$player` root node (propagated access)
    ///
    /// This means every node in the tree knows about operations that affect
    /// it — either directly or through a descendant.
    ///
    /// The `span` and `body_text` are both relative to the passage body
    /// (the content after the `:: PassageName` header line). The line number
    /// is computed immediately from `body_text` using `span.start` as the
    /// byte offset.
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
        body_text: &str,
    ) {
        let entry = self
            .variables
            .entry(name.to_string())
            .or_insert_with(|| VarEntry::new(name.to_string(), is_temporary));

        let line = compute_line_from_offset(body_text, span.start);

        let access = VarAccess {
            passage_name: passage_name.to_string(),
            file_uri: file_uri.to_string(),
            span,
            kind,
            is_temporary,
            line,
            graph_order: None, // Computed later by compute_graph_order()
            propagated: false,
        };

        if property_path.is_empty() {
            // Direct access on the root variable
            entry.node.record_access(access);
        } else {
            // Walk/create the property path and record at the leaf,
            // then propagate up to all ancestors.
            // Note: child nodes are created automatically by the walk below
            let segments: Vec<&str> = property_path.split('.').collect();

            // First, propagate to the root node
            entry.node.record_propagated(&access);

            // Walk to the leaf, propagating at each intermediate node
            let mut current = &mut entry.node;
            for segment in &segments[..segments.len() - 1] {
                current = current
                    .children
                    .entry(segment.to_string())
                    .or_insert_with(|| VarNode::new(segment.to_string(), is_temporary));
                // Propagate to this intermediate node
                current.record_propagated(&access);
            }

            // The leaf node gets the direct access (ownership)
            let leaf_segment = segments.last().unwrap();
            let leaf = current
                .children
                .entry(leaf_segment.to_string())
                .or_insert_with(|| VarNode::new(leaf_segment.to_string(), is_temporary));
            leaf.record_access(access);
        }
    }

    /// Backward-compatible convenience: record a variable access with a
    /// simple read/write boolean.
    ///
    /// See [`record_var()`] for details on `body_text` and passage-relative
    /// position handling.
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
        body_text: &str,
    ) {
        let kind = if is_write {
            VarAccessKind::Write
        } else {
            VarAccessKind::Read
        };
        self.record_var(name, is_temporary, kind, passage_name, file_uri, span, property_path, body_text);
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
    ///
    /// Removes accesses from all nodes in every variable tree where the
    /// file URI matches. After removal, variable entries with no remaining
    /// accesses (direct or propagated) in any node are removed entirely.
    pub fn remove_file(&mut self, file_uri: &str) {
        for entry in self.variables.values_mut() {
            remove_file_from_node(&mut entry.node, file_uri);
        }
        self.variables.retain(|_, entry| node_is_alive(&entry.node));
    }

    /// Remove all accesses for a specific passage (for incremental single-passage re-parse).
    pub fn remove_passage(&mut self, passage_name: &str) {
        for entry in self.variables.values_mut() {
            remove_passage_from_node(&mut entry.node, passage_name);
        }
        self.variables.retain(|_, entry| node_is_alive(&entry.node));
    }

    /// Resolve line numbers for all variable accesses in the tree using source text.
    ///
    /// **Deprecated**: With passage-relative positioning, line numbers are now
    /// computed at record time from the passage body text. This method is kept
    /// as a no-op for API compatibility but does nothing.
    pub fn resolve_line_numbers(&mut self, _source_text: &dyn SourceTextProvider) {
        // Line numbers are computed at record time from passage body text.
        // No post-hoc resolution needed.
    }

    /// Build the `VariableTreeNode` list for the variable tree UI.
    ///
    /// Converts the hierarchical variable tree into the format-agnostic
    /// `VariableTreeNode` / `VariablePropertyNode` types that the LSP server
    /// and VS Code extension expect.
    ///
    /// Each root variable becomes a `VariableTreeNode`. Its children become
    /// nested `VariablePropertyNode`s. Write/read locations are populated
    /// at every level using both direct and propagated accesses.
    ///
    /// The `passage_positions` map provides the document-absolute start line
    /// for each passage body, keyed by `(file_uri, passage_name)`. This is
    /// used to convert passage-relative line numbers stored in `VarAccess`
    /// to document-absolute line numbers expected by the LSP.
    pub fn build_tree(&self, passage_positions: &PassagePositionMap) -> Vec<VariableTreeNode> {
        let mut nodes = Vec::new();

        for entry in self.variables.values() {
            nodes.push(build_root_node(entry, passage_positions));
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
    ///
    /// Returns immediate child names for each root variable (e.g., `{"name", "hp"}`
    /// for `$player`), matching the format expected by `build_object_property_map`.
    pub fn property_map(&self) -> HashMap<String, HashSet<String>> {
        let mut map = HashMap::new();
        for (name, entry) in &self.variables {
            let children: HashSet<String> = entry.node.children.keys().cloned().collect();
            if !children.is_empty() {
                map.insert(name.clone(), children);
            }
        }
        map
    }

    /// Iterate over all variable entries in the tree.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &VarEntry)> {
        self.variables.iter()
    }

    /// Compute graph-order indices for all variable accesses.
    pub fn compute_graph_order(
        &mut self,
        special_passages: &HashSet<String>,
        start_passage: &str,
        bfs_order: &[String],
    ) {
        // Build a passage name → order index map
        let mut passage_order: HashMap<String, u32> = HashMap::new();
        let mut next_order: u32 = 0;

        for name in special_passages {
            passage_order.insert(name.clone(), 0);
        }

        if !start_passage.is_empty() {
            next_order = 1;
            passage_order.insert(start_passage.to_string(), next_order);
            next_order += 1;
        }

        for passage_name in bfs_order {
            if !passage_order.contains_key(passage_name) {
                passage_order.insert(passage_name.clone(), next_order);
                next_order += 1;
            }
        }

        // Assign graph_order to every access in every node
        for entry in self.variables.values_mut() {
            assign_graph_order_to_node(&mut entry.node, &passage_order);
        }
    }
}

// ---------------------------------------------------------------------------
// Tree traversal helpers
// ---------------------------------------------------------------------------

/// Recursively remove all accesses matching a file URI from a node and its children.
fn remove_file_from_node(node: &mut VarNode, file_uri: &str) {
    node.accesses.retain(|a| a.file_uri != file_uri);
    for child in node.children.values_mut() {
        remove_file_from_node(child, file_uri);
    }
}

/// Recursively remove all accesses matching a passage name from a node and its children.
fn remove_passage_from_node(node: &mut VarNode, passage_name: &str) {
    node.accesses.retain(|a| a.passage_name != passage_name);
    for child in node.children.values_mut() {
        remove_passage_from_node(child, passage_name);
    }
}

/// Check whether a node (and its subtree) has any remaining accesses,
/// meaning the variable is still alive and should not be pruned.
fn node_is_alive(node: &VarNode) -> bool {
    if !node.accesses.is_empty() {
        return true;
    }
    node.children.values().any(|c| node_is_alive(c))
}



/// Recursively assign graph_order to all accesses in a node tree and sort.
fn assign_graph_order_to_node(node: &mut VarNode, passage_order: &HashMap<String, u32>) {
    for access in &mut node.accesses {
        access.graph_order = passage_order.get(&access.passage_name).copied();
    }
    // Sort accesses by graph_order for deterministic ordering
    node.accesses.sort_by(|a, b| {
        let order_a = a.graph_order.unwrap_or(u32::MAX);
        let order_b = b.graph_order.unwrap_or(u32::MAX);
        order_a.cmp(&order_b).then_with(|| a.span.start.cmp(&b.span.start))
    });
    for child in node.children.values_mut() {
        assign_graph_order_to_node(child, passage_order);
    }
}

/// Build a `VariableTreeNode` (root-level) from a `VarEntry`.
fn build_root_node(entry: &VarEntry, passage_positions: &PassagePositionMap) -> VariableTreeNode {
    let written_in = accesses_to_usage_locations(&entry.node.all_writes(), passage_positions);
    let read_in = accesses_to_usage_locations(&entry.node.all_reads(), passage_positions);

    let is_unused = !entry.node.accesses.iter().any(|a| a.is_read() && !a.is_write());

    let properties = build_property_nodes(&entry.node, &entry.name, &entry.state_path, passage_positions);

    let kind = infer_kind_from_children(&entry.node);

    VariableTreeNode {
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
    }
}

/// Recursively build `VariablePropertyNode` children from a `VarNode`'s children.
fn build_property_nodes(
    parent_node: &VarNode,
    parent_full_name: &str,
    parent_state_path: &str,
    passage_positions: &PassagePositionMap,
) -> Vec<VariablePropertyNode> {
    let mut result = Vec::new();

    // Sort children by name for stable output
    let mut sorted_children: Vec<_> = parent_node.children.iter().collect();
    sorted_children.sort_by(|a, b| a.0.cmp(b.0));

    for (child_name, child_node) in sorted_children {
        let full_name = format!("{}.{}", parent_full_name, child_name);
        let state_path = format!("{}.{}", parent_state_path, child_name);

        let written_in = accesses_to_usage_locations(&child_node.all_writes(), passage_positions);
        let read_in = accesses_to_usage_locations(&child_node.all_reads(), passage_positions);

        // Use the line from the first direct write if available, else first access.
        // Convert passage-relative → document-absolute using passage positions.
        let rel_line = child_node
            .direct_writes()
            .first()
            .map(|a| a.line)
            .or_else(|| child_node.accesses.first().map(|a| a.line))
            .unwrap_or(0);
        let line = child_node
            .accesses
            .first()
            .and_then(|a| passage_positions.get(&(a.file_uri.clone(), a.passage_name.clone())))
            .map(|pos| pos.body_start_line + rel_line)
            .unwrap_or(rel_line);

        let properties = build_property_nodes(child_node, &full_name, &state_path, passage_positions);
        let kind = infer_kind_from_children(child_node);

        result.push(VariablePropertyNode {
            name: child_name.clone(),
            full_name,
            state_path,
            line,
            written_in,
            read_in,
            properties,
            kind,
            element_shape: None,
            coverage: None,
        });
    }

    result
}

/// Convert a list of `VarAccess` references to `VariableUsageLocation` values.
///
/// Converts passage-relative line numbers to document-absolute line numbers
/// using the `passage_positions` map.
fn accesses_to_usage_locations(accesses: &[&VarAccess], passage_positions: &PassagePositionMap) -> Vec<VariableUsageLocation> {
    accesses
        .iter()
        .map(|a| {
            let abs_line = passage_positions
                .get(&(a.file_uri.clone(), a.passage_name.clone()))
                .map(|pos| pos.body_start_line + a.line)
                .unwrap_or(a.line);
            VariableUsageLocation {
                passage_name: a.passage_name.clone(),
                file_uri: a.file_uri.clone(),
                is_write: a.is_write(),
                line: abs_line,
            }
        })
        .collect()
}

/// Infer the `PropertyKind` of a node from its children.
fn infer_kind_from_children(node: &VarNode) -> PropertyKind {
    if node.children.is_empty() {
        PropertyKind::Unknown
    } else {
        // Heuristic: if "length" or "push" is a child, it's probably an Array
        if node.children.keys().any(|k| k == "length" || k == "push" || k == "pop") {
            PropertyKind::Array
        } else {
            PropertyKind::Object
        }
    }
}

/// Compute a 0-based line number from a byte offset in source text.
///
/// Counts the number of `\n` characters before `offset`. The result is
/// relative to the start of the given `source` string — if `source` is a
/// passage body, the result is a passage-relative line number.
pub fn compute_line_from_offset(source: &str, offset: usize) -> u32 {
    let pos = offset.min(source.len());
    source[..pos].chars().filter(|&c| c == '\n').count() as u32
}

// ---------------------------------------------------------------------------
// Passage position map — for converting relative → absolute at output boundary
// ---------------------------------------------------------------------------

/// The position of a passage body within a document.
///
/// Used to convert passage-relative line numbers and byte offsets to
/// document-absolute values at the LSP output boundary.
#[derive(Debug, Clone, Copy)]
pub struct PassagePosition {
    /// The 0-based line number in the full document where the passage body
    /// starts (i.e., the line immediately after the `:: PassageName` header).
    pub body_start_line: u32,
    /// The byte offset in the full document where the passage body starts
    /// (i.e., right after the header line's newline character).
    pub body_start_offset: usize,
}

/// A map from `(file_uri, passage_name)` to the passage's position in the document.
///
/// Built by scanning source text for `::` headers. Used by `build_tree()`
/// and `extract_passage_variable_refs()` to convert passage-relative positions
/// to document-absolute positions at the LSP output boundary.
pub type PassagePositionMap = HashMap<(String, String), PassagePosition>;

/// Scan a source text and build a map of passage positions.
///
/// Finds all `:: PassageName` headers in the source text and records
/// the body start line and body start offset for each passage. The key
/// is `(file_uri, passage_name)`.
///
/// This is a simple scanner that handles the common case. It does not
/// handle edge cases like `::` inside passage body content that happens
/// to be at the start of a line — for that, the full lexer would be needed.
pub fn compute_passage_positions(source: &str, file_uri: &str) -> PassagePositionMap {
    let mut positions = PassagePositionMap::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut line_start = 0usize;
    let mut current_line = 0u32;

    while line_start < len {
        // Find end of current line
        let line_end = source[line_start..].find('\n')
            .map_or(len, |pos| line_start + pos);

        // Check if this line is a passage header (:: Name ...)
        let line_text = &source[line_start..line_end];
        if let Some(rest) = line_text.strip_prefix("::") {
            // Extract passage name (up to [ or { or end of line)
            let name_end = rest.find(|c: char| c == '[' || c == '{').unwrap_or(rest.len());
            let name = rest[..name_end].trim();
            if !name.is_empty() {
                let body_start_offset = if line_end < len { line_end + 1 } else { len };
                let body_start_line = current_line + 1; // Next line after header
                positions.insert(
                    (file_uri.to_string(), name.to_string()),
                    PassagePosition {
                        body_start_line,
                        body_start_offset,
                    },
                );
            }
        }

        current_line += 1;
        line_start = if line_end < len { line_end + 1 } else { len };
    }

    positions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: empty body text for tests that don't check line numbers
    const BT: &str = "";

    #[test]
    fn record_and_retrieve_variable() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///test.tw", 50..53, "", BT);

        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.node.accesses.len(), 2);
        assert!(entry.first_write().is_some());
        assert_eq!(entry.first_write().unwrap().passage_name, "Start");
    }

    #[test]
    fn state_path_annotation() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT);
        tree.record_var("_i", true, VarAccessKind::Write, "Loop", "file:///test.tw", 5..7, "", BT);

        let story_entry = tree.get_variable("$hp").unwrap();
        assert_eq!(story_entry.state_path, "State.variables.hp");

        let temp_entry = tree.get_variable("_i").unwrap();
        assert_eq!(temp_entry.state_path, "State.temporary.i");
    }

    #[test]
    fn compound_write_classification() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::CompoundWrite, "Forest", "file:///test.tw", 10..17, "", BT);

        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.node.accesses.len(), 1);
        assert!(entry.node.accesses[0].is_write());
        assert!(entry.node.accesses[0].is_read());
    }

    #[test]
    fn property_tracking_creates_tree() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..17, "name", BT);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 20..27, "hp", BT);

        let entry = tree.get_variable("$player").unwrap();
        assert_eq!(entry.node.children.len(), 2);
        assert!(entry.node.children.contains_key("name"));
        assert!(entry.node.children.contains_key("hp"));
        assert!(entry.known_properties().contains("name"));
        assert!(entry.known_properties().contains("hp"));
    }

    #[test]
    fn property_write_propagates_to_parent() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..25, "hp.max", BT);

        let entry = tree.get_variable("$player").unwrap();

        let root_writes: Vec<_> = entry.node.accesses.iter().filter(|a| a.is_write()).collect();
        assert_eq!(root_writes.len(), 1);
        assert!(root_writes[0].propagated);

        let hp_node = entry.node.children.get("hp").unwrap();
        let hp_writes: Vec<_> = hp_node.accesses.iter().filter(|a| a.is_write()).collect();
        assert_eq!(hp_writes.len(), 1);
        assert!(hp_writes[0].propagated);

        let max_node = hp_node.children.get("max").unwrap();
        let max_writes: Vec<_> = max_node.accesses.iter().filter(|a| a.is_write()).collect();
        assert_eq!(max_writes.len(), 1);
        assert!(!max_writes[0].propagated);
    }

    #[test]
    fn read_propagates_to_parent() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Read, "Fight", "file:///test.tw", 10..20, "hp", BT);

        let entry = tree.get_variable("$player").unwrap();

        let root_reads: Vec<_> = entry.node.accesses.iter().filter(|a| a.is_read()).collect();
        assert_eq!(root_reads.len(), 1);
        assert!(root_reads[0].propagated);

        let hp_node = entry.node.children.get("hp").unwrap();
        let hp_reads: Vec<_> = hp_node.accesses.iter().filter(|a| a.is_read()).collect();
        assert_eq!(hp_reads.len(), 1);
        assert!(!hp_reads[0].propagated);
    }

    #[test]
    fn mixed_direct_and_propagated() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 5..12, "", BT);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 15..25, "hp", BT);

        let entry = tree.get_variable("$player").unwrap();

        let direct_writes: Vec<_> = entry.node.accesses.iter()
            .filter(|a| a.is_write() && !a.propagated)
            .collect();
        let prop_writes: Vec<_> = entry.node.accesses.iter()
            .filter(|a| a.is_write() && a.propagated)
            .collect();
        assert_eq!(direct_writes.len(), 1);
        assert_eq!(prop_writes.len(), 1);
    }

    #[test]
    fn temporary_variable() {
        let mut tree = VariableTree::new();
        tree.record_var("_i", true, VarAccessKind::Write, "Loop", "file:///test.tw", 5..7, "", BT);

        let entry = tree.get_variable("_i").unwrap();
        assert!(entry.is_temporary);
    }

    #[test]
    fn seeded_by_special() {
        let mut tree = VariableTree::new();
        tree.record_var("$gold", false, VarAccessKind::Write, "StoryInit", "file:///test.tw", 10..15, "", BT);
        tree.mark_seeded("$gold");

        let entry = tree.get_variable("$gold").unwrap();
        assert!(entry.seeded_by_special);
    }

    #[test]
    fn build_tree_populates_property_writes() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..25, "hp", BT);
        tree.record_var("$player", false, VarAccessKind::Read, "Fight", "file:///test.tw", 30..40, "hp", BT);

        let nodes = tree.build_tree(&PassagePositionMap::new());
        let player = nodes.iter().find(|n| n.name == "$player").unwrap();

        assert!(!player.written_in.is_empty());
        assert!(!player.read_in.is_empty());

        assert_eq!(player.properties.len(), 1);
        let hp_prop = &player.properties[0];
        assert_eq!(hp_prop.name, "hp");
        assert!(!hp_prop.written_in.is_empty());
        assert!(!hp_prop.read_in.is_empty());
    }

    #[test]
    fn build_tree_deep_nesting() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..30, "hp.max", BT);

        let nodes = tree.build_tree(&PassagePositionMap::new());
        let player = nodes.iter().find(|n| n.name == "$player").unwrap();

        assert_eq!(player.properties.len(), 1);
        let hp = &player.properties[0];
        assert_eq!(hp.name, "hp");
        assert!(!hp.written_in.is_empty());
        assert_eq!(hp.properties.len(), 1);

        let max = &hp.properties[0];
        assert_eq!(max.name, "max");
        assert!(!max.written_in.is_empty());
    }

    #[test]
    fn remove_file() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///a.tw", 10..13, "", BT);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///b.tw", 50..53, "", BT);

        tree.remove_file("file:///a.tw");
        let entry = tree.get_variable("$hp").unwrap();
        assert_eq!(entry.node.accesses.len(), 1);
        assert_eq!(entry.node.accesses[0].passage_name, "Forest");
    }

    #[test]
    fn remove_file_with_properties() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///a.tw", 10..25, "hp", BT);

        tree.remove_file("file:///a.tw");
        assert!(tree.get_variable("$player").is_none());
    }

    #[test]
    fn graph_order_indexing() {
        let mut tree = VariableTree::new();
        tree.record_var("$gold", false, VarAccessKind::Write, "StoryInit", "file:///test.tw", 10..15, "", BT);
        tree.record_var("$gold", false, VarAccessKind::Read, "Start", "file:///test.tw", 20..25, "", BT);
        tree.record_var("$gold", false, VarAccessKind::CompoundWrite, "Forest", "file:///test.tw", 30..35, "", BT);

        let special: HashSet<String> = ["StoryInit".to_string()].into_iter().collect();
        tree.compute_graph_order(&special, "Start", &["Forest".to_string()]);

        let entry = tree.get_variable("$gold").unwrap();
        assert_eq!(entry.node.accesses[0].graph_order, Some(0)); // StoryInit
        assert_eq!(entry.node.accesses[1].graph_order, Some(1)); // Start
        assert_eq!(entry.node.accesses[2].graph_order, Some(2)); // Forest

        assert_eq!(entry.first_write().unwrap().passage_name, "StoryInit");
    }

    #[test]
    fn passage_relative_line_computation() {
        let mut tree = VariableTree::new();
        // Body text with variable on line 2 (0-based)
        let body = "line 0\n<<set $hp to 100>>\nline 2";
        // Span starts at "line 0\n" = 7 bytes, so $hp is at offset 7
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 7..10, "", body);

        let entry = tree.get_variable("$hp").unwrap();
        // Line should be 1 (0-based, after the first \n)
        assert_eq!(entry.node.accesses[0].line, 1);
    }

    #[test]
    fn compute_passage_positions_basic() {
        let source = ":: Start\nHello world\n:: Forest\nDeep woods\n";
        let positions = compute_passage_positions(source, "file:///test.tw");

        let start_pos = positions.get(&("file:///test.tw".to_string(), "Start".to_string())).unwrap();
        assert_eq!(start_pos.body_start_line, 1); // Line after header
        assert_eq!(start_pos.body_start_offset, 9); // After ":: Start\n"

        let forest_pos = positions.get(&("file:///test.tw".to_string(), "Forest".to_string())).unwrap();
        assert_eq!(forest_pos.body_start_line, 3); // Line after "Hello world\n:: Forest\n"
    }
}
