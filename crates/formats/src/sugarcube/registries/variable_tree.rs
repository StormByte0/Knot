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
//! $player          (root VarArenaNode)
//!   └─ hp          (child VarArenaNode)
//!       └─ max     (child VarArenaNode)
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
//! ## Synchronization
//!
//! The tree is a plain data structure with no internal locks. Synchronization
//! is the server's responsibility: the server's `tokio::sync::RwLock<ServerStateInner>`
//! is the sole synchronization mechanism. Read access (`&self`) is available under
//! the server's read lock, and write access (`&mut self`) is available under the
//! server's write lock.

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
    /// The span of the full construct (e.g., `{...}` or the entire `<<set>>`
    /// macro) when this access is part of an object literal assignment or a
    /// <<set>> macro. Used for focus-level granularity: when the user is
    /// focused on a parent variable, the construct span covers the entire
    /// expression rather than just the individual property key. Also used for
    /// deduplication — child property writes that share the same construct
    /// span as a direct block write on the parent should not create redundant
    /// propagated accesses.
    ///
    /// `None` for reads and non-set writes (e.g., `$foo.bar` in prose text).
    pub construct_span: Option<Range<usize>>,
    /// Per-segment spans for each path component, enabling precision
    /// highlighting. For `$player.hp.max`: `[($player span), (.hp span),
    /// (.max span)]`. Empty when per-segment data is unavailable.
    pub segment_spans: Vec<Range<usize>>,
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

    /// Create a propagated copy of this access, with an optional focus-level
    /// span override.
    ///
    /// Propagated copies inherit all fields except `propagated` which is set
    /// to `true`, and `span` which is overridden with `focus_span` if provided.
    /// When a child access is part of an object literal (has a `construct_span`),
    /// the propagated copy uses `construct_span` as its span — this implements
    /// focus-level granularity so the parent sees a single write covering the
    /// full `{...}` expression rather than a tiny property key token.
    ///
    /// The `construct_span` is preserved on the propagated copy so that
    /// downstream deduplication can check whether a parent already has an
    /// access covering the same construct.
    fn as_propagated(&self) -> VarAccess {
        // When this access is part of a construct (object literal or full
        // <<set>> macro), the propagated copy should use the construct span
        // as its span. This gives focus-level granularity: the parent sees
        // the full construct span, not the child's individual key token span.
        let span = self.construct_span.clone().unwrap_or_else(|| self.span.clone());

        VarAccess {
            passage_name: self.passage_name.clone(),
            file_uri: self.file_uri.clone(),
            span,
            kind: self.kind,
            is_temporary: self.is_temporary,
            line: self.line,
            graph_order: self.graph_order,
            propagated: true,
            construct_span: self.construct_span.clone(),
            segment_spans: self.segment_spans.clone(),
        }
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

// ===========================================================================
// Arena-allocated variable tree (new implementation)
// ===========================================================================
//
// This section defines the arena-based tree that replaces the old
// HashMap-based VariableTree.
//
// ## Design
//
// All nodes live in a single `Vec<VarArenaNode>` (the "arena"). Tree
// structure is encoded via `first_child` / `next_sibling` / `parent`
// indices (u32 NodeId values). This gives:
//
// - One allocation for the entire variable registry (~160KB for 2000 nodes)
// - Cache-friendly child traversal (sibling list in contiguous memory)
// - Natural subtree operations (walk from first_child)
// - Deterministic insertion order
//
// A `path_index: HashMap<String, NodeId>` provides O(1) exact lookup
// for LSP operations (hover, go-to-def, find-all-refs).

/// Index into the arena `Vec<VarArenaNode>`.
///
/// Using `u32` instead of `usize` halves the size of `Option<NodeId>`
/// (4 bytes vs 8 on 64-bit) and supports ~4 billion nodes — far more
/// than any Twine story will ever have.
pub type NodeId = u32;

/// A sentinel value indicating no node (instead of `Option<NodeId>` for
/// compactness in the arena). Used in `parent`, `first_child`, `next_sibling`.
pub const NO_NODE: NodeId = u32::MAX;

// ---------------------------------------------------------------------------
// VarScope — persistent vs temporary variable classification
// ---------------------------------------------------------------------------

/// The scope of a variable node.
///
/// SugarCube has two distinct variable scopes with different lifetimes:
/// - **Persistent** (`$var`): Cross-passage, game-wide, stored in
///   `State.variables`. Survives for the entire game session.
/// - **Temporary** (`_var`): Single-passage scope, stored in
///   `State.temporary`. Exists only during the passage's execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarScope {
    /// Persistent state variable (`$var`), lives in `State.variables`.
    Persistent,
    /// Temporary variable (`_var`), lives in `State.temporary`.
    /// The `passage_id` identifies which passage this temp root belongs to.
    /// We use a hash of the passage name as the ID — good enough for
    /// a workspace-local identifier.
    Temporary { passage_id: u32 },
}

// ---------------------------------------------------------------------------
// InferredType — type inference for variable nodes
// ---------------------------------------------------------------------------

/// The inferred type of a variable or property, derived from usage patterns.
///
/// SugarCube has no type declarations — types are inferred from how
/// variables are used. This information powers hover tooltips and
/// completion detail text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InferredType {
    /// A scalar value (number, string, boolean, null).
    Scalar,
    /// An object with named child properties.
    Object,
    /// An array with indexed elements.
    Array,
    /// A function value.
    Function,
    /// Type could not be determined from usage.
    Unknown,
}

// ---------------------------------------------------------------------------
// SourceLocation — a source position for LSP navigation
// ---------------------------------------------------------------------------

/// A source location used for LSP navigation (go-to-def, find-all-refs).
///
/// This is similar to `VarAccess` but lighter-weight — it only carries
/// the information needed for LSP navigation, not the full access record.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    /// The passage name where this location occurs.
    pub passage_name: String,
    /// The file URI where this location occurs.
    pub file_uri: String,
    /// Byte range in the passage body (passage-body-relative).
    pub span: Range<usize>,
    /// The 0-based line number relative to passage body start.
    pub line: u32,
}

// ---------------------------------------------------------------------------
// NavIndex — LSP navigation indexes per node
// ---------------------------------------------------------------------------

/// Navigation indexes for LSP operations, attached to each arena node.
///
/// Each node in the variable tree carries its own `NavIndex`, providing
/// O(1) access to the information needed for LSP queries without
/// having to traverse the tree or compute anything on the fly.
#[derive(Debug, Clone)]
pub struct NavIndex {
    /// Source locations for "Go to Definition" — the places where this
    /// variable/property path is first assigned or defined.
    pub def_sites: Vec<SourceLocation>,
    /// Source locations for "Find All References" — every place where
    /// this exact path is referenced (read or written).
    pub ref_sites: Vec<SourceLocation>,
    /// The inferred type of this variable/property, derived from
    /// usage patterns (object literal shapes, method calls, etc.).
    pub inferred_type: Option<InferredType>,
    /// Whether this variable/property is safe to rename.
    /// `false` if any ancestor or this node has `has_dynamic_access: true`,
    /// because dynamic access (`$ITEMS[expr]`) means we can't guarantee
    /// finding all references.
    pub rename_safe: bool,
    /// Number of known children, for autocomplete display
    /// (e.g., "42 more properties...").
    pub known_child_count: u16,
}

impl Default for NavIndex {
    fn default() -> Self {
        Self {
            def_sites: Vec::new(),
            ref_sites: Vec::new(),
            inferred_type: None,
            rename_safe: true,
            known_child_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// VarMeta — metadata block per arena node
// ---------------------------------------------------------------------------

/// Metadata block attached to each arena node.
///
/// This is the "rich" part of a variable node — all the information
/// beyond the tree structure itself. Each node is self-contained:
/// consumers can answer queries about a variable/property path by
/// looking at a single node's metadata, without traversing ancestors
/// or consulting separate data structures.
#[derive(Debug, Clone)]
pub struct VarMeta {
    /// All observed references to this exact path (direct accesses).
    /// Propagated accesses (from descendants) are NOT stored here —
    /// propagation flags (`has_read_descendant`, `has_write_descendant`)
    /// summarize that information for O(1) queries.
    pub refs: Vec<VarAccess>,

    // ── Propagation flags (computed bottom-up) ──
    /// Whether any descendant node has been read. Computed by `propagate()`.
    pub has_read_descendant: bool,
    /// Whether any descendant node has been written. Computed by `propagate()`.
    pub has_write_descendant: bool,
    /// Whether any descendant (or this node) was accessed via computed
    /// member expression (`$ITEMS[expr]`). When true, the LSP cannot
    /// assume it knows all children, and rename is unsafe.
    pub has_dynamic_access: bool,

    // ── Scope and seed info ──
    /// The scope of this variable (persistent or temporary).
    pub scope: VarScope,
    /// Whether this variable is seeded by a special passage
    /// (StoryInit, [script]). Only meaningful on root nodes.
    pub seeded_by_special: bool,

    // ── LSP navigation ──
    /// Navigation indexes for LSP operations.
    pub nav: NavIndex,
}

impl Default for VarMeta {
    fn default() -> Self {
        Self {
            refs: Vec::new(),
            has_read_descendant: false,
            has_write_descendant: false,
            has_dynamic_access: false,
            scope: VarScope::Persistent,
            seeded_by_special: false,
            nav: NavIndex::default(),
        }
    }
}

impl VarMeta {
    /// Create metadata for a new node.
    fn new(scope: VarScope) -> Self {
        Self {
            scope,
            ..Self::default()
        }
    }

    /// Record a direct access on this node.
    fn record_access(&mut self, access: VarAccess) {
        if access.kind.is_write() {
            self.has_write_descendant = true;
        }
        if access.kind.is_read() {
            self.has_read_descendant = true;
        }
        self.refs.push(access);
    }

    /// Record a propagated access on this node (from a descendant operation).
    ///
    /// Implements construct-span deduplication (same as `VarNode::record_propagated`):
    /// if the incoming access has a `construct_span`, and this node already has
    /// an access with the same `construct_span` and kind, the propagation is skipped.
    ///
    /// Note: The arena `record_var` now inlines per-segment propagation logic
    /// directly, but this method is kept for potential future use.
    #[allow(dead_code)]
    fn record_propagated(&mut self, access: &VarAccess) {
        if access.kind.is_write() {
            self.has_write_descendant = true;
        }
        if access.kind.is_read() {
            self.has_read_descendant = true;
        }
        // Construct-span dedup
        if let Some(ref cs) = access.construct_span {
            let already_covered = self.refs.iter().any(|existing| {
                existing.kind == access.kind
                    && existing.construct_span.as_ref() == Some(cs)
            });
            if already_covered {
                return;
            }
        }
        self.refs.push(access.as_propagated());
    }

    /// Get all direct (non-propagated) write accesses.
    pub fn direct_writes(&self) -> Vec<&VarAccess> {
        self.refs.iter().filter(|a| a.is_write() && !a.propagated).collect()
    }

    /// Get all direct (non-propagated) read accesses.
    pub fn direct_reads(&self) -> Vec<&VarAccess> {
        self.refs.iter().filter(|a| a.is_read() && !a.propagated).collect()
    }

    /// Get all write accesses (direct + propagated).
    pub fn all_writes(&self) -> Vec<&VarAccess> {
        self.refs.iter().filter(|a| a.is_write()).collect()
    }

    /// Get all read accesses (direct + propagated).
    pub fn all_reads(&self) -> Vec<&VarAccess> {
        self.refs.iter().filter(|a| a.is_read()).collect()
    }
}

// ---------------------------------------------------------------------------
// VarArenaNode — a single node in the arena
// ---------------------------------------------------------------------------

/// A node in the arena-allocated variable property tree.
///
/// Each `VarArenaNode` represents one segment of a dot-path. The root
/// node for `$player` has the name "$player" and may have child nodes
/// like "hp", "name", etc. A child node for `$player.hp` has the
/// name "hp" and may itself have children.
///
/// Tree structure is encoded via `first_child` / `next_sibling` /
/// `parent` indices. Children of a node form a singly-linked list:
/// `first_child` points to the first child, and each child's
/// `next_sibling` points to the next child. This is the classic
/// "first-child / next-sibling" representation, stored in a flat
/// arena (Vec) for cache-friendly traversal.
#[derive(Debug, Clone)]
pub struct VarArenaNode {
    /// The single segment name of this node (e.g., "hp" for `$player.hp`).
    /// Root nodes include the sigil (e.g., "$player").
    pub name: String,
    /// Whether this is a temporary variable (`_` sigil). Only meaningful
    /// on root nodes.
    pub is_temporary: bool,
    /// Index of the parent node in the arena. `NO_NODE` for root nodes.
    pub parent: NodeId,
    /// Index of the first child node. `NO_NODE` if no children.
    pub first_child: NodeId,
    /// Index of the next sibling node. `NO_NODE` if last child.
    pub next_sibling: NodeId,
    /// Metadata block: refs, propagation flags, scope, navigation.
    pub meta: VarMeta,
}

impl VarArenaNode {
    /// Create a new arena node with the given name and parent.
    fn new(name: String, is_temporary: bool, parent: NodeId, scope: VarScope) -> Self {
        Self {
            name,
            is_temporary,
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            meta: VarMeta::new(scope),
        }
    }

    /// Whether this node has any children.
    pub fn has_children(&self) -> bool {
        self.first_child != NO_NODE
    }
}

// ---------------------------------------------------------------------------
// VarArena — the arena allocator and tree structure manager
// ---------------------------------------------------------------------------

/// An arena-allocated variable tree with first-child / next-sibling linking.
///
/// All nodes live in a single `Vec<VarArenaNode>`. Tree structure is
/// encoded via `first_child`, `next_sibling`, and `parent` indices.
/// This gives:
///
/// - **One allocation** for the entire tree (the Vec's buffer)
/// - **Cache-friendly traversal** when iterating children (sibling
///   list walks within the same buffer)
/// - **Natural subtree operations** (walk from `first_child`)
/// - **Deterministic insertion order** (unlike HashMap)
///
/// The arena does NOT manage variable scopes — it only manages the
/// tree structure. Scope (persistent vs temporary) is managed by
/// the `VariableTree` wrapper which owns the arena.
#[derive(Debug, Clone)]
pub struct VarArena {
    /// The node storage. Index `i` corresponds to `NodeId = i as u32`.
    nodes: Vec<VarArenaNode>,
    /// Reusable node slots (from removed subtrees). Nodes in the
    /// free list have their `parent` set to `NO_NODE` as a sentinel.
    free_list: Vec<NodeId>,
}

impl Default for VarArena {
    fn default() -> Self {
        Self::new()
    }
}

impl VarArena {
    /// Create an empty arena.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free_list: Vec::new(),
        }
    }

    /// Allocate a new node in the arena and return its NodeId.
    ///
    /// Reuses free list slots when available, otherwise pushes to the end.
    /// Returns an error if the arena exceeds `u32::MAX - 1` nodes (impossible in practice).
    pub fn alloc(&mut self, node: VarArenaNode) -> NodeId {
        if let Some(id) = self.free_list.pop() {
            self.nodes[id as usize] = node;
            id
        } else {
            let id = self.nodes.len() as NodeId;
            assert!(id < NO_NODE, "VarArena::alloc: exceeded maximum node count ({}) — this indicates an unbounded variable tree growth or a logic bug", NO_NODE);
            self.nodes.push(node);
            id
        }
    }

    /// Get an immutable reference to a node by its NodeId.
    ///
    /// # Panics
    /// Panics with a descriptive message if `id` is `NO_NODE` or out of bounds.
    /// Use [`try_get`](Self::try_get) for a non-panicking version.
    pub fn get(&self, id: NodeId) -> &VarArenaNode {
        assert!(id != NO_NODE, "VarArena::get: called with NO_NODE sentinel — this indicates a logic bug where a missing-node handle was used without checking");
        &self.nodes[id as usize]
    }

    /// Get a mutable reference to a node by its NodeId.
    ///
    /// # Panics
    /// Panics with a descriptive message if `id` is `NO_NODE` or out of bounds.
    /// Use [`try_get_mut`](Self::try_get_mut) for a non-panicking version.
    pub fn get_mut(&mut self, id: NodeId) -> &mut VarArenaNode {
        assert!(id != NO_NODE, "VarArena::get_mut: called with NO_NODE sentinel — this indicates a logic bug where a missing-node handle was used without checking");
        &mut self.nodes[id as usize]
    }

    /// Try to get an immutable reference to a node by its NodeId.
    ///
    /// Returns `None` if `id` is `NO_NODE` or out of bounds.
    /// This is the non-panicking alternative to [`get`](Self::get).
    pub fn try_get(&self, id: NodeId) -> Option<&VarArenaNode> {
        if id == NO_NODE || (id as usize) >= self.nodes.len() {
            return None;
        }
        Some(&self.nodes[id as usize])
    }

    /// Try to get a mutable reference to a node by its NodeId.
    ///
    /// Returns `None` if `id` is `NO_NODE` or out of bounds.
    /// This is the non-panicking alternative to [`get_mut`](Self::get_mut).
    pub fn try_get_mut(&mut self, id: NodeId) -> Option<&mut VarArenaNode> {
        if id == NO_NODE || (id as usize) >= self.nodes.len() {
            return None;
        }
        Some(&mut self.nodes[id as usize])
    }

    /// Get the number of nodes currently in the arena (including free slots).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the arena has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    // ── Child traversal ────────────────────────────────────────────────

    /// Iterate over the NodeIds of all children of a node.
    ///
    /// Walks the `first_child` → `next_sibling` chain. Returns an
    /// iterator that yields NodeIds in insertion order.
    pub fn children_of(&self, parent_id: NodeId) -> ArenaChildIter<'_> {
        let first = self.get(parent_id).first_child;
        ArenaChildIter {
            arena: self,
            current: if first == NO_NODE { None } else { Some(first) },
        }
    }

    /// Find a child of `parent_id` by name.
    ///
    /// Walks the sibling chain and returns the first child whose
    /// `name` matches. Returns `None` if no child with that name exists.
    /// This is a linear scan — for nodes with many children, the
    /// `path_index` on `VariableTree` provides O(1) lookup.
    pub fn find_child_by_name(&self, parent_id: NodeId, name: &str) -> Option<NodeId> {
        let mut child_id = self.get(parent_id).first_child;
        while child_id != NO_NODE {
            let child = self.get(child_id);
            if child.name == name {
                return Some(child_id);
            }
            child_id = child.next_sibling;
        }
        None
    }

    // ── Child insertion ────────────────────────────────────────────────

    /// Insert a child node into the tree under `parent_id`.
    ///
    /// The child's `parent` is set to `parent_id`, and it is appended
    /// to the end of the sibling chain (to maintain insertion order).
    /// Returns the NodeId of the child.
    pub fn insert_child(&mut self, parent_id: NodeId, child: VarArenaNode) -> NodeId {
        let child_id = self.alloc(child);
        self.get_mut(child_id).parent = parent_id;

        let parent = self.get_mut(parent_id);
        if parent.first_child == NO_NODE {
            // No children yet — this is the first
            parent.first_child = child_id;
        } else {
            // Walk to the last sibling and append
            let mut last_id = parent.first_child;
            loop {
                let last = self.get_mut(last_id);
                if last.next_sibling == NO_NODE {
                    last.next_sibling = child_id;
                    break;
                }
                last_id = last.next_sibling;
            }
        }

        // Update the parent's known_child_count in nav index
        self.get_mut(parent_id).meta.nav.known_child_count =
            self.get(parent_id).meta.nav.known_child_count.saturating_add(1);

        child_id
    }

    /// Ensure a path of child nodes exists under `root_id`, creating
    /// intermediate nodes as needed. Returns the NodeId of the leaf.
    ///
    /// For `path = "hp.max"`, this ensures "hp" exists as a child of
    /// `root_id`, then "max" as a child of "hp", and returns the NodeId
    /// of "max". If any segment already exists, it is reused.
    pub fn ensure_path(
        &mut self,
        root_id: NodeId,
        path: &str,
        is_temporary: bool,
        scope: VarScope,
    ) -> NodeId {
        if path.is_empty() {
            return root_id;
        }

        let mut current_id = root_id;
        for segment in path.split('.') {
            // Check if this segment already exists as a child
            if let Some(existing_id) = self.find_child_by_name(current_id, segment) {
                current_id = existing_id;
            } else {
                let node = VarArenaNode::new(
                    segment.to_string(),
                    is_temporary,
                    current_id, // parent
                    scope,
                );
                let new_id = self.insert_child(current_id, node);
                current_id = new_id;
            }
        }
        current_id
    }

    /// Resolve a dot-separated path from a root node.
    ///
    /// Walks `root_id.name1.name2.name3` by looking up each segment
    /// as a child of the previous. Returns `None` if any segment
    /// doesn't exist.
    pub fn resolve_path(&self, root_id: NodeId, path: &str) -> Option<NodeId> {
        if path.is_empty() {
            return Some(root_id);
        }

        let mut current_id = root_id;
        for segment in path.split('.') {
            current_id = self.find_child_by_name(current_id, segment)?;
        }
        Some(current_id)
    }

    // ── Subtree removal ────────────────────────────────────────────────

    /// Collect all NodeIds in the subtree rooted at `root_id`
    /// (including `root_id` itself), in depth-first order.
    ///
    /// Does NOT unlink the subtree from its parent — the caller
    /// must do that separately.
    pub fn collect_subtree(&self, root_id: NodeId) -> Vec<NodeId> {
        let mut result = Vec::new();
        self.collect_subtree_recursive(root_id, &mut result);
        result
    }

    fn collect_subtree_recursive(&self, id: NodeId, result: &mut Vec<NodeId>) {
        result.push(id);
        let mut child_id = self.get(id).first_child;
        while child_id != NO_NODE {
            self.collect_subtree_recursive(child_id, result);
            child_id = self.get(child_id).next_sibling;
        }
    }

    /// Mark a subtree as freed by adding all its NodeIds to the free list.
    ///
    /// Freed nodes have their `parent`, `first_child`, and `next_sibling`
    /// fields set to `NO_NODE` to prevent stale-pointer traversal. Without
    /// this, code that walks `first_child`/`next_sibling` chains (e.g.,
    /// `arena_node_is_alive`, `collect_locations_from_arena_node`) could
    /// follow dangling pointers into freed/reused nodes, eventually hitting
    /// `arena.get(NO_NODE)` which panics.
    pub fn free_subtree(&mut self, root_id: NodeId) {
        let ids = self.collect_subtree(root_id);
        for id in ids {
            let node = self.get_mut(id);
            // Skip if already freed (parent == NO_NODE sentinel).
            // This prevents duplicate free-list entries if a subtree
            // contains nodes that were already freed by a prior call.
            if node.parent == NO_NODE && id != root_id {
                continue;
            }
            node.parent = NO_NODE;
            // Clear child/sibling pointers to prevent stale-pointer
            // traversal after freeing. If these are left dangling,
            // any code walking first_child/next_sibling chains from
            // a parent node whose child was freed (but not yet
            // unlinked in all cases) would follow stale pointers
            // into freed or reused arena slots.
            node.first_child = NO_NODE;
            node.next_sibling = NO_NODE;
            self.free_list.push(id);
        }
    }

    /// Unlink a child from its parent's sibling chain.
    ///
    /// After this, the child is still in the arena but no longer
    /// reachable from its parent. Returns `true` if the child was
    /// found and unlinked, `false` otherwise.
    pub fn unlink_child(&mut self, parent_id: NodeId, child_id: NodeId) -> bool {
        let parent = self.get(parent_id);

        // Special case: child is the first child
        if parent.first_child == child_id {
            let next = self.get(child_id).next_sibling;
            self.get_mut(parent_id).first_child = next;
            self.get_mut(child_id).next_sibling = NO_NODE;
            // Update child count
            self.get_mut(parent_id).meta.nav.known_child_count =
                self.get_mut(parent_id).meta.nav.known_child_count.saturating_sub(1);
            return true;
        }

        // Walk the sibling chain to find the predecessor
        let mut prev_id = parent.first_child;
        while prev_id != NO_NODE {
            let prev = self.get(prev_id);
            if prev.next_sibling == child_id {
                let next = self.get(child_id).next_sibling;
                self.get_mut(prev_id).next_sibling = next;
                self.get_mut(child_id).next_sibling = NO_NODE;
                // Update child count
                self.get_mut(parent_id).meta.nav.known_child_count =
                    self.get_mut(parent_id).meta.nav.known_child_count.saturating_sub(1);
                return true;
            }
            prev_id = prev.next_sibling;
        }

        false
    }

    // ── Propagation ────────────────────────────────────────────────────

    /// Propagate access flags bottom-up from all leaf nodes to the root.
    ///
    /// After recording accesses, call this to ensure every ancestor
    /// knows whether it has read or write descendants. This is O(n)
    /// in the number of nodes in the tree.
    ///
    /// For a single subtree rooted at `root_id`, use `propagate_node()`
    /// instead.
    pub fn propagate_from(&mut self, root_id: NodeId) {
        self.propagate_node_recursive(root_id);
    }

    fn propagate_node_recursive(&mut self, id: NodeId) -> (bool, bool, bool) {
        // (has_read_desc, has_write_desc, has_dynamic)
        let mut has_read = false;
        let mut has_write = false;
        let mut has_dynamic = false;

        // Recurse into children first (bottom-up)
        let mut child_id = self.get(id).first_child;
        while child_id != NO_NODE {
            let (cr, cw, cd) = self.propagate_node_recursive(child_id);
            has_read |= cr;
            has_write |= cw;
            has_dynamic |= cd;
            child_id = self.get(child_id).next_sibling;
        }

        // Also consider this node's own direct accesses
        let node = self.get(id);
        has_read |= node.meta.refs.iter().any(|a| a.is_read());
        has_write |= node.meta.refs.iter().any(|a| a.is_write());
        has_dynamic |= node.meta.has_dynamic_access;

        // Write back
        let node = self.get_mut(id);
        node.meta.has_read_descendant = has_read;
        node.meta.has_write_descendant = has_write;
        node.meta.has_dynamic_access = has_dynamic;

        // Compute rename_safe: if any ancestor has dynamic access,
        // rename is unsafe. We propagate this up so the root knows.
        // rename_safe = !has_dynamic
        node.meta.nav.rename_safe = !has_dynamic;

        (has_read, has_write, has_dynamic)
    }

    // ── Iteration ──────────────────────────────────────────────────────

    /// Iterate over all node IDs in the arena (including free-list slots).
    pub fn iter_ids(&self) -> impl Iterator<Item = NodeId> {
        0..self.nodes.len() as NodeId
    }
}

// ---------------------------------------------------------------------------
// ArenaChildIter — iterator over children of an arena node
// ---------------------------------------------------------------------------

/// Iterator over the children of an arena node, yielding NodeIds.
pub struct ArenaChildIter<'a> {
    arena: &'a VarArena,
    current: Option<NodeId>,
}

impl<'a> Iterator for ArenaChildIter<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.current?;
        self.current = {
            let next = self.arena.get(id).next_sibling;
            if next == NO_NODE { None } else { Some(next) }
        };
        Some(id)
    }
}

// ---------------------------------------------------------------------------
// VariableTree — the arena-based variable registry
// ---------------------------------------------------------------------------

/// The arena-allocated variable registry for a SugarCube workspace.
///
/// This replaces the HashMap-based `VariableTree`. It uses a single
/// `VarArena` to store all nodes (both persistent and temporary
/// variables) and provides the same public API as `VariableTree`.
///
/// ## Dual-scope design
///
/// - **Persistent scope**: All `$var` references are stored as
///   children of `persistent_root`. These accumulate across all
///   passages and survive for the entire game session.
///
/// - **Temporary scope**: All `_var` references for a specific
///   passage are stored under a per-passage root in `temp_roots`.
///   These are scoped to a single passage and can be cleared
///   independently when the passage is re-parsed.
///
/// ## Path index
///
/// A `path_index: HashMap<String, NodeId>` provides O(1) exact
/// lookup for the common LSP operations (hover, go-to-def, find-refs).
/// The path is the fully-qualified dot-path including the sigil
/// (e.g., `"$ITEMS.pencil-skirt-navy.name"`).
///
/// The path index is a **navigation accelerator**, not the source of
/// truth. The arena tree is the source of truth. If memory is a
/// concern, the path index can be dropped and rebuilt from the tree.
#[derive(Debug, Clone)]
pub struct VariableTree {
    /// The arena storing all nodes.
    arena: VarArena,
    /// The root NodeId for persistent (`$`) variables.
    /// This node has name "<persistent>" and is not a real variable.
    persistent_root: NodeId,
    /// Per-passage roots for temporary (`_`) variables.
    /// Keyed by passage name hash (sufficient for workspace-local use).
    temp_roots: HashMap<String, NodeId>,
    /// Fast path lookup: fully-qualified dot-path → NodeId.
    /// E.g., `"$ITEMS.pencil-skirt-navy.name"` → NodeId(42).
    path_index: HashMap<String, NodeId>,
    /// Set of variable names that are seeded by special passages.
    seeded_vars: HashSet<String>,
}

impl Default for VariableTree {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// path_index key namespacing
//
// The `path_index` maps fully-qualified dot-paths (e.g. `"$player.hp.max"`)
// to `NodeId`s for O(1) lookup. To prevent cross-passage collisions between
// temporary variables with the same name (passage A's `_x` vs passage B's
// `_x`), temporary paths are namespaced with a sentinel prefix that cannot
// appear in a real SugarCube variable path.
//
// Persistent paths keep their plain display form (`"$player"`) because:
//   1. Persistent vars are workspace-global — there is no collision to avoid.
//   2. External callers (e.g. `kind_at_path("$player.hp")`) and tests use
//      the display form directly; keeping it stable preserves the API.
//
// Temporary paths become `"\u{0}<passage>:<name>"` (NUL prefix). The NUL
// byte is forbidden in SugarCube source, so no legitimate path can
// collide with the namespaced form. Callers that resolve temp paths
// must supply the enclosing passage so the key can be reconstructed;
// without it, temp lookups return `None` (safe degradation — matches
// the completion behavior in `completion_names_for_passage`).
// ---------------------------------------------------------------------------
fn path_index_key(name: &str, is_temporary: bool, passage_name: Option<&str>) -> String {
    if is_temporary {
        // NUL prefix guarantees no collision with persistent paths
        // (which start with `$` or `_`/alpha) or with display-form
        // temp paths that older code might still pass in.
        match passage_name {
            Some(p) => format!("\u{0}{p}:{name}"),
            // No passage context → no key. Callers get `None` from the
            // lookup, which is the correct behavior for an unresolved
            // temp scope.
            None => format!("\u{0}:{name}"),
        }
    } else {
        // Persistent vars: plain display form, no namespacing.
        name.to_string()
    }
}

impl VariableTree {
    /// Create an empty arena variable tree.
    pub fn new() -> Self {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new(
            "<persistent>".to_string(),
            false,
            NO_NODE,
            VarScope::Persistent,
        );
        let persistent_root = arena.alloc(root);

        Self {
            arena,
            persistent_root,
            temp_roots: HashMap::new(),
            path_index: HashMap::new(),
            seeded_vars: HashSet::new(),
        }
    }

    /// Get the arena (immutable).
    pub fn arena(&self) -> &VarArena {
        &self.arena
    }

    /// Get the arena (mutable).
    pub fn arena_mut(&mut self) -> &mut VarArena {
        &mut self.arena
    }

    /// Get the persistent root NodeId.
    pub fn persistent_root(&self) -> NodeId {
        self.persistent_root
    }

    /// Record a variable access from a passage.
    ///
    /// This is the core recording method, matching the same API as
    /// `VariableTree::record_var()`. The access is recorded at the
    /// exact node specified by `name` + `property_path`, then
    /// propagated up to all ancestor nodes.
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
        segment_spans: &[Range<usize>],
        construct_span: Option<Range<usize>>,
    ) {
        let line = compute_line_from_offset(body_text, span.start);
        let scope = if is_temporary {
            // Use a simple hash of the passage name as the passage_id.
            // This is workspace-local and non-cryptographic — good enough.
            let passage_id = passage_name.chars().fold(0u32, |acc, c| acc.wrapping_add(c as u32));
            VarScope::Temporary { passage_id }
        } else {
            VarScope::Persistent
        };

        let access = VarAccess {
            passage_name: passage_name.to_string(),
            file_uri: file_uri.to_string(),
            span,
            kind,
            is_temporary,
            line,
            graph_order: None,
            propagated: false,
            construct_span: construct_span.clone(),
            segment_spans: segment_spans.to_vec(),
        };

        // Find or create the root variable node
        let root_id = if is_temporary {
            self.ensure_temp_root(passage_name)
        } else {
            self.persistent_root
        };

        // Find or create the variable node under the root
        let var_id = self.ensure_variable_node(root_id, name, is_temporary, scope);

        if property_path.is_empty() {
            // Direct access on the root variable.
            // For object literal block writes (property_path="" with segment_spans),
            // the access.span already covers the full `{...}` expression — use it
            // for the VarAccess record (focus-level span). The segment_spans[0]
            // gives the assignment target span (e.g., `$ITEMS`) for precision
            // nav pointing.
            let def_span = segment_spans.first().cloned().unwrap_or_else(|| access.span.clone());
            // For ref_site, use the root variable token span for precision pointing
            // (e.g., `$foo` in `<<set $foo = {...}>>`), not the full construct span.
            let ref_span = segment_spans.first().cloned().unwrap_or_else(|| access.span.clone());
            let nav_loc = SourceLocation {
                passage_name: passage_name.to_string(),
                file_uri: file_uri.to_string(),
                span: ref_span,
                line,
            };
            let def_nav_loc = SourceLocation {
                passage_name: passage_name.to_string(),
                file_uri: file_uri.to_string(),
                span: def_span,
                line,
            };

            self.arena.get_mut(var_id).meta.record_access(access);
            self.arena.get_mut(var_id).meta.nav.ref_sites.push(nav_loc);
            if kind.is_write() {
                // def_site points to the assignment target (e.g., $ITEMS in
                // `<<set $ITEMS = {...}>>`) for precision "Go to Definition".
                self.arena.get_mut(var_id).meta.nav.def_sites.push(def_nav_loc);
            }
        } else {
            // Walk/create the property path
            let leaf_id = self.arena.ensure_path(var_id, property_path, is_temporary, scope);

            // Build nav SourceLocation for the leaf node.
            // With the segment_spans convention:
            //   segment_spans[0] = root var span (e.g., $foo)
            //   segment_spans[1..] = property key spans (e.g., .bar, .baz)
            // The last segment_span corresponds to the leaf property key.
            let leaf_span = if segment_spans.is_empty() {
                access.span.clone()
            } else {
                segment_spans.last().cloned().unwrap_or_else(|| access.span.clone())
            };
            let nav_loc = SourceLocation {
                passage_name: passage_name.to_string(),
                file_uri: file_uri.to_string(),
                span: leaf_span,
                line,
            };

            // Record the access on the leaf node
            self.arena.get_mut(leaf_id).meta.record_access(access.clone());
            self.arena.get_mut(leaf_id).meta.nav.ref_sites.push(nav_loc.clone());
            if kind.is_write() {
                self.arena.get_mut(leaf_id).meta.nav.def_sites.push(nav_loc);
            }

            // Propagate to ancestors: walk from leaf's parent up to (but not
            // including) the scope root (root_id). This covers all intermediate
            // nodes AND var_id itself, since var_id is a child of root_id.
            //
            // ## Focus-level semantics
            //
            // When a node is "in focus", we show the operations done on it at
            // that level. For a simple write `foo.bar = "value"`, focus on
            // `foo` shows a write whose span covers the full access. For an
            // object literal write `foo: {bar: "value", baz: "value"}`, focus
            // on `foo` shows a single write whose span covers the `{...}`.
            //
            // The propagated VarAccess gets the ORIGINAL access span (which
            // covers the full construct — the `{...}` or the full `$foo.bar`).
            // This represents "the extent of the operation at this focus level."
            //
            // For nav ref_sites (precision pointing), we use segment_spans
            // to point to the exact token at each depth.
            //
            // ## Focus-level semantics & deduplication
            //
            // When a node is "in focus", we show the operations done on it at
            // that level. For `$foo = {bar: 1, baz: 2}`, at `$foo`'s focus
            // level the user sees a single write spanning the full `{...}`.
            // The individual `bar`/`baz` writes are only visible when focusing
            // on those child nodes.
            //
            // To avoid redundant propagated accesses:
            // 1. If an ancestor already has a direct access (e.g., a block
            //    write covering `{...}`) from the same passage, we skip — the
            //    direct access already represents this operation at that level.
            // 2. If an ancestor already has a propagated access from the same
            //    passage whose span OVERLAPS with (contains) this one, we skip
            //    — a previous sibling already propagated a covering span.
            //
            // When a propagation is absorbed (skipped because a covering
            // access exists), we also stop propagating to further ancestors —
            // the covering access's own propagation (from when it was recorded)
            // already handles those.
            let mut ancestor_id = self.arena.get(leaf_id).parent;
            // The focus-level span for propagation: use construct_span (the
            // full `{...}` expression) if available, otherwise fall back to
            // access.span (which covers the full `$foo.bar` token for regular
            // dot-path operations). This is the span the user sees when
            // viewing the ancestor node — the extent of the operation at
            // that focus level.
            let focus_span = construct_span
                .clone()
                .unwrap_or_else(|| access.span.clone());

            let mut absorbed = false;
            while ancestor_id != NO_NODE && ancestor_id != root_id {
                // Always set propagation flags
                if access.kind.is_write() {
                    self.arena.get_mut(ancestor_id).meta.has_write_descendant = true;
                }
                if access.kind.is_read() {
                    self.arena.get_mut(ancestor_id).meta.has_read_descendant = true;
                }

                // If a previous ancestor already absorbed this propagation
                // (had a covering access), don't add redundant propagated
                // accesses to further ancestors — the covering access's own
                // propagation already represents this operation at those levels.
                if absorbed {
                    // Still add a nav ref_site if we have per-segment data
                    // and this ancestor doesn't already have one
                    if !segment_spans.is_empty() {
                        let ancestor_depth = self.depth_from_var(var_id, ancestor_id);
                        if ancestor_depth < segment_spans.len() {
                            let nav_span = segment_spans[ancestor_depth].clone();
                            let nav_ref = SourceLocation {
                                passage_name: passage_name.to_string(),
                                file_uri: file_uri.to_string(),
                                span: nav_span.clone(),
                                line: compute_line_from_offset(body_text, nav_span.start),
                            };
                            let ancestor_node = self.arena.get_mut(ancestor_id);
                            let already_has_ref = ancestor_node.meta.nav.ref_sites.iter().any(|existing| {
                                existing.passage_name == nav_ref.passage_name
                                    && existing.span == nav_ref.span
                            });
                            if !already_has_ref {
                                ancestor_node.meta.nav.ref_sites.push(nav_ref);
                            }
                        }
                    }
                    ancestor_id = self.arena.get(ancestor_id).parent;
                    continue;
                }

                // Check if this ancestor already has an access (direct or
                // propagated) from the same passage that covers this operation.
                // Two strategies:
                // 1. **Construct-span dedup** (preferred): if the incoming
                //    access has a construct_span, check if the ancestor already
                //    has an access with the same construct_span and kind. This
                //    is the strongest dedup — it means a block write (or a
                //    previous sibling's propagated access) already represents
                //    this operation at the ancestor's focus level.
                // 2. **Span containment** (fallback): if no construct_span,
                //    check if the ancestor has a covering span from the same
                //    passage. This handles regular dot-path operations like
                //    `$foo.bar = 1` → `$foo` propagation.
                let should_skip_access = {
                    let ancestor_node = self.arena.get(ancestor_id);
                    if let Some(ref cs) = access.construct_span {
                        // Construct-span dedup: exact match on construct_span + kind
                        ancestor_node.meta.refs.iter().any(|existing| {
                            existing.kind == access.kind
                                && existing.construct_span.as_ref() == Some(cs)
                        })
                    } else {
                        // Span containment dedup: same passage, existing span
                        // contains the focus_span
                        ancestor_node.meta.refs.iter().any(|existing| {
                            existing.passage_name == access.passage_name
                                && existing.span.start <= focus_span.start
                                && existing.span.end >= focus_span.end
                        })
                    }
                };

                if should_skip_access {
                    // This ancestor already has a covering access. Mark as
                    // absorbed so we don't add redundant accesses to further
                    // ancestors either.
                    absorbed = true;
                } else {
                    // Create a propagated access with the focus-level span.
                    // This represents "the extent of the operation at this
                    // focus level" — e.g., the full `{...}` when viewing
                    // `$foo`, not just the `bar` key span.
                    let mut prop_access = access.as_propagated();
                    prop_access.span = focus_span.clone();

                    self.arena.get_mut(ancestor_id).meta.refs.push(prop_access);
                }

                // For nav ref_sites, use segment_spans for precision pointing.
                // segment_spans[depth] gives the exact token span at each depth,
                // enabling "Go to Definition" to navigate to the correct token.
                let nav_span = if !segment_spans.is_empty() {
                    let ancestor_depth = self.depth_from_var(var_id, ancestor_id);
                    if ancestor_depth < segment_spans.len() {
                        segment_spans[ancestor_depth].clone()
                    } else {
                        focus_span.clone()
                    }
                } else {
                    // Fallback: no per-segment data, use the focus-level span
                    focus_span.clone()
                };

                let nav_ref = SourceLocation {
                    passage_name: passage_name.to_string(),
                    file_uri: file_uri.to_string(),
                    span: nav_span.clone(),
                    line: compute_line_from_offset(body_text, nav_span.start),
                };
                let ancestor_node = self.arena.get_mut(ancestor_id);
                let already_has_ref = ancestor_node.meta.nav.ref_sites.iter().any(|existing| {
                    existing.passage_name == nav_ref.passage_name
                        && existing.span == nav_ref.span
                });
                if !already_has_ref {
                    ancestor_node.meta.nav.ref_sites.push(nav_ref);
                }

                ancestor_id = self.arena.get(ancestor_id).parent;
            }

            // Update path_index for all nodes along the path, not just the leaf.
            // This ensures O(1) lookup for intermediate paths like "$player.hp"
            // as well as the full path "$player.hp.max".
            //
            // For temporary variables, the root key is namespaced with the
            // declaring passage (see `path_index_key`); intermediate
            // property paths inherit the namespace by prefixing onto the
            // namespaced root. This prevents `_x.y` in passage A from
            // colliding with `_x.y` in passage B.
            let segments: Vec<&str> = property_path.split('.').collect();
            let mut path_prefix = path_index_key(name, is_temporary, Some(passage_name));
            let mut current_id = var_id;
            for segment in &segments {
                if let Some(child_id) = self.arena.find_child_by_name(current_id, segment) {
                    path_prefix = format!("{}.{}", path_prefix, segment);
                    self.path_index.entry(path_prefix.clone()).or_insert(child_id);
                    current_id = child_id;
                }
            }
        }

        // Update path_index for the root variable.
        // Persistent: plain display name (`"$player"`).
        // Temporary: namespaced with the declaring passage so concurrent
        // `_x` in two passages don't clobber each other (first-write
        // would otherwise win, silently dropping the second).
        let root_key = path_index_key(name, is_temporary, Some(passage_name));
        self.path_index.entry(root_key).or_insert(var_id);
    }

    /// Find or create a temp root for the given passage.
    fn ensure_temp_root(&mut self, passage_name: &str) -> NodeId {
        if let Some(&id) = self.temp_roots.get(passage_name) {
            return id;
        }

        let passage_id = passage_name.chars().fold(0u32, |acc, c| acc.wrapping_add(c as u32));
        let node = VarArenaNode::new(
            format!("<temp:{}>", passage_name),
            true,
            NO_NODE,
            VarScope::Temporary { passage_id },
        );
        let id = self.arena.alloc(node);
        self.temp_roots.insert(passage_name.to_string(), id);
        id
    }

    /// Find or create a variable node under the given root.
    fn ensure_variable_node(
        &mut self,
        root_id: NodeId,
        name: &str,
        is_temporary: bool,
        scope: VarScope,
    ) -> NodeId {
        // Check if this variable already exists as a child of root
        if let Some(id) = self.arena.find_child_by_name(root_id, name) {
            return id;
        }

        // Create a new variable node
        let node = VarArenaNode::new(name.to_string(), is_temporary, root_id, scope);
        self.arena.insert_child(root_id, node)
    }

    /// Compute the depth of `ancestor_id` relative to `var_id`.
    ///
    /// Returns 0 if `ancestor_id == var_id`, 1 if `ancestor_id` is the
    /// parent of `var_id`, etc. This is used to find the correct index
    /// into `segment_spans` during propagation, where:
    /// - `segment_spans[0]` = root variable span (var_id's level)
    /// - `segment_spans[1]` = first property span
    /// - etc.
    fn depth_from_var(&self, var_id: NodeId, ancestor_id: NodeId) -> usize {
        // Walk UP from ancestor_id toward var_id, counting steps.
        // "ancestor_id" is named from the perspective of the leaf node
        // (it's an ancestor of the leaf), but it's a DESCENDANT of
        // var_id. So we walk up FROM ancestor_id TO var_id.
        let mut depth = 0;
        let mut current = ancestor_id;
        while current != var_id && current != NO_NODE {
            current = self.arena.get(current).parent;
            depth += 1;
        }
        depth
    }

    /// Mark a variable as seeded by a special passage.
    ///
    /// For persistent (`$`) vars, sets the `seeded_by_special` flag on
    /// the indexed node. For temporary (`_`) vars, only the
    /// `seeded_vars` set is updated — the per-passage `path_index`
    /// key cannot be reconstructed without a passage context, so the
    /// node flag is skipped. (Temps seeded by special passages is
    /// semantically odd anyway — startup passages initialize
    /// persistent state.)
    pub fn mark_seeded(&mut self, name: &str) {
        self.seeded_vars.insert(name.to_string());
        // Skip the path_index lookup for temp vars: keys are now
        // namespaced by passage and `mark_seeded` doesn't take one.
        if name.starts_with('_') {
            return;
        }
        if let Some(&id) = self.path_index.get(name) {
            self.arena.get_mut(id).meta.seeded_by_special = true;
        }
    }

    /// Look up a node by fully-qualified path (via path_index).
    ///
    /// **Warning:** for temporary variable paths (e.g. `"_x.y"`), this
    /// method cannot resolve the declaring passage and will return
    /// `None` because `path_index` keys for temps are namespaced by
    /// passage. Callers in completion/hover contexts that know the
    /// enclosing passage should use [`get_node_by_path_for_passage`]
    /// instead. Persistent (`$`) paths work as before.
    ///
    /// [`get_node_by_path_for_passage`]: Self::get_node_by_path_for_passage
    pub fn get_node_by_path(&self, path: &str) -> Option<(NodeId, &VarArenaNode)> {
        // Persistent paths are stored under their display form — direct
        // lookup works. Temp paths are namespaced; legacy callers that
        // don't know the passage get `None` (safe degradation).
        if path.starts_with('_') {
            return None;
        }
        let id = self.path_index.get(path)?;
        Some((*id, self.arena.get(*id)))
    }

    /// Look up a node by path, scoped to a passage for temp variables.
    ///
    /// For persistent paths (`$player.hp`), `passage_name` is ignored —
    /// persistent vars are workspace-global. For temp paths (`_x.y`),
    /// the lookup is namespaced with `passage_name`; if `passage_name`
    /// is `None` or doesn't match the declaring passage of the temp,
    /// `None` is returned (safe degradation — never returns a
    /// different passage's temp).
    ///
    /// This is the passage-aware variant of [`get_node_by_path`],
    /// intended for completion, hover, and dot-continuation contexts
    /// where the enclosing passage is known.
    ///
    /// [`get_node_by_path`]: Self::get_node_by_path
    pub fn get_node_by_path_for_passage(
        &self,
        path: &str,
        passage_name: Option<&str>,
    ) -> Option<(NodeId, &VarArenaNode)> {
        // Split into root + property suffix so we can namespace only
        // the root segment. E.g. `_x.y.z` → root=`_x`, suffix=`.y.z`.
        let (root, suffix) = match path.find('.') {
            Some(idx) => (&path[..idx], &path[idx..]),
            None => (path, ""),
        };

        let is_temp = root.starts_with('_');
        let key = path_index_key(root, is_temp, passage_name);
        let full_key = format!("{key}{suffix}");
        let id = self.path_index.get(&full_key)?;
        Some((*id, self.arena.get(*id)))
    }

    /// Look up a variable's root node by name (e.g., "$ITEMS").
    ///
    /// Searches both persistent and temporary variable roots.
    pub fn get_variable(&self, name: &str) -> Option<(NodeId, &VarArenaNode)> {
        // First check persistent variables
        if let Some(id) = self.arena.find_child_by_name(self.persistent_root, name) {
            return Some((id, self.arena.get(id)));
        }
        // Then check temp variable roots
        for &temp_root_id in self.temp_roots.values() {
            if let Some(id) = self.arena.find_child_by_name(temp_root_id, name) {
                return Some((id, self.arena.get(id)));
            }
        }
        None
    }

    /// Get all persistent variable names (children of persistent_root).
    pub fn variable_names(&self) -> Vec<String> {
        self.arena
            .children_of(self.persistent_root)
            .map(|id| self.arena.get(id).name.clone())
            .collect()
    }

    /// Get the number of persistent variables.
    pub fn len(&self) -> usize {
        self.arena.get(self.persistent_root).meta.nav.known_child_count as usize
    }

    /// Get the number of entries in the path index.
    ///
    /// This counts all fully-qualified dot-paths (e.g., `$player`,
    /// `$player.hp`, `$player.hp.max`) currently indexed for O(1)
    /// lookup. Used by pipeline logging to report variable counts.
    pub fn path_index_len(&self) -> usize {
        self.path_index.len()
    }

    /// Whether the tree has no persistent variables.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all data (for full re-parse).
    pub fn clear(&mut self) {
        self.arena = VarArena::new();
        let root = VarArenaNode::new(
            "<persistent>".to_string(),
            false,
            NO_NODE,
            VarScope::Persistent,
        );
        self.persistent_root = self.arena.alloc(root);
        self.temp_roots.clear();
        self.path_index.clear();
        self.seeded_vars.clear();
    }

    /// Remove all accesses for a specific file (for incremental re-parse).
    pub fn remove_file(&mut self, file_uri: &str) {
        // Walk all LIVE nodes in the arena, filter accesses.
        // We skip freed slots (parent == NO_NODE) because:
        // 1. Their data is stale and shouldn't be modified
        // 2. Operating on freed nodes can create incorrect refs that
        //    make arena_node_is_alive return true for dead nodes
        let ids: Vec<NodeId> = self.arena.iter_ids()
            .filter(|&id| self.is_live_node(id))
            .collect();
        for id in ids {
            let node = self.arena.get_mut(id);
            node.meta.refs.retain(|a| a.file_uri != file_uri);
            node.meta.nav.def_sites.retain(|s| s.file_uri != file_uri);
            node.meta.nav.ref_sites.retain(|s| s.file_uri != file_uri);
        }
        // Prune dead nodes (no refs, no children with refs)
        self.prune_dead_nodes();
    }

    /// Remove all accesses for a specific passage (for incremental re-parse).
    pub fn remove_passage(&mut self, passage_name: &str) {
        // Walk all LIVE nodes in the arena, filter accesses.
        // Skip freed slots for the same reasons as remove_file.
        let ids: Vec<NodeId> = self.arena.iter_ids()
            .filter(|&id| self.is_live_node(id))
            .collect();
        for id in ids {
            let node = self.arena.get_mut(id);
            node.meta.refs.retain(|a| a.passage_name != passage_name);
            node.meta.nav.def_sites.retain(|s| s.passage_name != passage_name);
            node.meta.nav.ref_sites.retain(|s| s.passage_name != passage_name);
        }
        // Remove the temp root for this passage. We unlink it from its
        // parent (if any) and remove its path_index entries BEFORE calling
        // prune_dead_nodes, which handles all freeing. This avoids the
        // double-free that occurred when free_subtree was called here
        // AND again inside prune_dead_nodes for the same nodes.
        if let Some(temp_root_id) = self.temp_roots.remove(passage_name) {
            // Unlink from parent if it has one (temp roots usually don't,
            // but defensive programming against future changes)
            let parent_id = self.arena.get(temp_root_id).parent;
            if parent_id != NO_NODE {
                self.arena.unlink_child(parent_id, temp_root_id);
            }
            // Remove path_index entries for this temp root and its
            // descendants so prune_dead_nodes doesn't try to process them.
            // (prune_dead_nodes will still free the nodes since they have
            // no refs and no parent.)
            let subtree_ids = self.arena.collect_subtree(temp_root_id);
            let subtree_set: HashSet<NodeId> = subtree_ids.into_iter().collect();
            let paths_to_remove: Vec<String> = self.path_index.iter()
                .filter(|(_, id)| subtree_set.contains(id))
                .map(|(path, _)| path.clone())
                .collect();
            for path in paths_to_remove {
                self.path_index.remove(&path);
            }
            // Do NOT call free_subtree here — let prune_dead_nodes
            // handle all freeing to prevent double-free.
        }
        self.prune_dead_nodes();
    }

    /// Prune property nodes that have no refs and no children with refs.
    /// Root variable nodes are only pruned if they also have no children.
    ///
    /// ## Double-free prevention
    ///
    /// When a parent node is freed via `free_subtree()`, all its descendants
    /// are freed too (added to the free list with `parent = NO_NODE`). If we
    /// naively iterate `dead_paths` and free each one independently, a child
    /// that was already freed as part of its parent's subtree would be freed
    /// again — corrupting the arena with duplicate free-list entries and
    /// potentially traversing stale pointers into live nodes.
    ///
    /// To prevent this, we collect all dead root IDs first, then for each
    /// dead root we compute its full subtree (collect_subtree). All subtree
    /// IDs are recorded in a `freed_set` so that if a descendant appears
    /// later in the dead list, we skip it. All descendant paths are also
    /// removed from `path_index` when the parent is freed, preventing stale
    /// lookups.
    fn prune_dead_nodes(&mut self) {
        // Step 1: Collect dead root node IDs (nodes with no alive descendants).
        let mut dead_roots: Vec<NodeId> = Vec::new();
        for (_, &id) in &self.path_index {
            if !self.arena_node_is_alive(id) {
                dead_roots.push(id);
            }
        }

        // Step 2: Deduplicate — collect the full set of IDs that will be freed
        // (including all descendants of dead roots). This prevents double-free
        // when a parent's subtree includes a child that's also in the dead list.
        let mut freed_set: HashSet<NodeId> = HashSet::new();
        let mut root_subtrees: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for &root_id in &dead_roots {
            if freed_set.contains(&root_id) {
                // Already freed as part of another subtree
                continue;
            }
            let subtree = self.arena.collect_subtree(root_id);
            for &id in &subtree {
                freed_set.insert(id);
            }
            root_subtrees.push((root_id, subtree));
        }

        // Step 3: Remove all freed node paths from path_index (both roots
        // and their descendants). This must happen before freeing to avoid
        // stale lookups.
        let mut paths_to_remove: Vec<String> = Vec::new();
        for (path, &id) in &self.path_index {
            if freed_set.contains(&id) {
                paths_to_remove.push(path.clone());
            }
        }
        for path in paths_to_remove {
            self.path_index.remove(&path);
        }

        // Step 4: Unlink dead roots from their parents and free subtrees.
        // Only unlink/freeze the root of each subtree — descendants are
        // handled by free_subtree automatically.
        for (root_id, _subtree) in &root_subtrees {
            let parent_id = self.arena.get(*root_id).parent;
            if parent_id != NO_NODE {
                self.arena.unlink_child(parent_id, *root_id);
            }
            self.arena.free_subtree(*root_id);
        }

        // Also remove from seeded_vars if the variable is gone.
        //
        // Persistent (`$`) vars: check `path_index` directly — keys are
        // display-form.
        // Temporary (`_`) vars: keys are namespaced per-passage, so a
        // plain `contains_key` would always return false. We retain
        // them unconditionally here; they get cleaned up via
        // `remove_passage` when their declaring passage is removed.
        // (Temps seeded by special passages is a degenerate case
        // anyway — see `mark_seeded`.)
        self.seeded_vars.retain(|name| {
            name.starts_with('_') || self.path_index.contains_key(name)
        });
    }

    /// Check if an arena node ID refers to a live (non-freed) node.
    ///
    /// Freed nodes have `parent == NO_NODE` (set by `free_subtree`).
    /// The persistent root and temp roots are always considered live
    /// regardless of their parent field.
    ///
    /// Uses `try_get` instead of `get` to avoid panicking if a stale
    /// NodeId is passed (e.g., from a `path_index` entry that points
    /// to a freed-and-reused slot).
    fn is_live_node(&self, id: NodeId) -> bool {
        id == self.persistent_root
            || self.arena.try_get(id).map_or(false, |n| n.parent != NO_NODE)
            || self.temp_roots.values().any(|&tr| tr == id)
    }

    /// Check if a node (and its subtree) has any remaining accesses.
    ///
    /// Uses `try_get` instead of `get` for child/sibling traversal to
    /// gracefully handle freed nodes whose `first_child`/`next_sibling`
    /// pointers might be stale. After `free_subtree` clears these fields
    /// to `NO_NODE`, this check is a no-op for properly freed nodes, but
    /// it provides defense-in-depth against any edge cases where a freed
    /// node might still be reachable from a live parent's child chain.
    fn arena_node_is_alive(&self, id: NodeId) -> bool {
        let node = match self.arena.try_get(id) {
            Some(n) => n,
            None => return false, // Freed or invalid — treat as dead
        };
        if !node.meta.refs.is_empty() {
            return true;
        }
        // Check children — skip any that are freed (parent == NO_NODE)
        // to avoid following stale pointers deeper into the arena.
        let mut child_id = node.first_child;
        while child_id != NO_NODE {
            let child = match self.arena.try_get(child_id) {
                Some(c) => c,
                None => break, // Stale pointer — stop traversing
            };
            // Skip freed children (their parent was set to NO_NODE by
            // free_subtree). They may still appear in the sibling chain
            // if unlink_child hasn't been called yet.
            if child.parent == NO_NODE {
                child_id = child.next_sibling;
                continue;
            }
            if self.arena_node_is_alive(child_id) {
                return true;
            }
            child_id = child.next_sibling;
        }
        false
    }

    /// Build a set of variable names for completion.
    ///
    /// Includes both persistent (`$var`) and temporary (`_var`) variable names.
    ///
    /// **Warning:** this method returns temporary variables from *all*
    /// passages. SugarCube `_` variables are passage-scoped at runtime,
    /// so showing temps from other passages is incorrect in completion
    /// contexts. Use [`completion_names_for_passage`] instead when the
    /// caller knows the enclosing passage.
    ///
    /// [`completion_names_for_passage`]: Self::completion_names_for_passage
    pub fn completion_names(&self) -> HashSet<String> {
        let mut names: HashSet<String> = self.arena
            .children_of(self.persistent_root)
            .map(|id| self.arena.get(id).name.clone())
            .collect();

        // Also include temporary variables from all passage roots
        for &temp_root_id in self.temp_roots.values() {
            for child_id in self.arena.children_of(temp_root_id) {
                names.insert(self.arena.get(child_id).name.clone());
            }
        }

        names
    }

    /// Build a set of variable names for completion, scoped to a passage.
    ///
    /// Returns all persistent (`$var`) variable names (these are global
    /// across passages by design) plus only the temporary (`_var`)
    /// variable names belonging to `passage_name`.
    ///
    /// When `passage_name` is `None` (caller cannot determine the
    /// enclosing passage — e.g., cursor is in a non-passage region),
    /// only persistent variables are returned. This is the safe
    /// degradation: it never leaks another passage's temps.
    ///
    /// SugarCube semantics: `_foo` declared in `:: Start` is invisible
    /// to `:: Inventory`. The arena already partitions temps under
    /// per-passage roots (`temp_roots`); this method just refuses to
    /// walk roots that don't belong to the requested passage.
    pub fn completion_names_for_passage(&self, passage_name: Option<&str>) -> HashSet<String> {
        // Persistent vars are workspace-global — always included.
        let mut names: HashSet<String> = self.arena
            .children_of(self.persistent_root)
            .map(|id| self.arena.get(id).name.clone())
            .collect();

        // Temps are passage-scoped — include only the matching root.
        if let Some(passage) = passage_name {
            if let Some(&temp_root_id) = self.temp_roots.get(passage) {
                for child_id in self.arena.children_of(temp_root_id) {
                    names.insert(self.arena.get(child_id).name.clone());
                }
            }
        }

        names
    }

    /// Build a map of variable name → known property paths
    /// (for dot-notation completion).
    pub fn property_map(&self) -> HashMap<String, HashSet<String>> {
        let mut map = HashMap::new();
        for var_id in self.arena.children_of(self.persistent_root) {
            let var_node = self.arena.get(var_id);
            let children: HashSet<String> = self
                .arena
                .children_of(var_id)
                .map(|child_id| self.arena.get(child_id).name.clone())
                .collect();
            if !children.is_empty() {
                map.insert(var_node.name.clone(), children);
            }
        }
        map
    }

    /// Propagate all access flags from leaves to roots.
    pub fn propagate(&mut self) {
        self.arena.propagate_from(self.persistent_root);
        for &temp_root_id in self.temp_roots.values() {
            self.arena.propagate_from(temp_root_id);
        }
    }

    /// Compute graph-order indices for all variable accesses.
    pub fn compute_graph_order(
        &mut self,
        special_passages: &HashSet<String>,
        start_passage: &str,
        bfs_order: &[String],
    ) {
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
        // Collect LIVE IDs only — skip freed slots to avoid modifying stale data
        let ids: Vec<NodeId> = self.arena.iter_ids()
            .filter(|&id| self.is_live_node(id))
            .collect();
        for id in ids {
            let node = self.arena.get_mut(id);
            for access in &mut node.meta.refs {
                access.graph_order = passage_order.get(&access.passage_name).copied();
            }
            // Sort accesses by graph_order for deterministic ordering
            node.meta.refs.sort_by(|a, b| {
                let order_a = a.graph_order.unwrap_or(u32::MAX);
                let order_b = b.graph_order.unwrap_or(u32::MAX);
                order_a.cmp(&order_b).then_with(|| a.span.start.cmp(&b.span.start))
            });
        }
    }

    /// Build the `VariableTreeNode` list for the variable tree UI.
    pub fn build_tree(&self, passage_positions: &PassagePositionMap) -> Vec<VariableTreeNode> {
        let mut nodes = Vec::new();

        for var_id in self.arena.children_of(self.persistent_root) {
            nodes.push(self.build_root_node(var_id, passage_positions));
        }

        // Sort by name for stable output
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        nodes
    }

    /// Build a `VariableTreeNode` from a root variable node.
    fn build_root_node(
        &self,
        var_id: NodeId,
        passage_positions: &PassagePositionMap,
    ) -> VariableTreeNode {
        let var_node = self.arena.get(var_id);
        let name = var_node.name.clone();
        let is_temporary = var_node.is_temporary;
        let state_path = compute_state_path(&name, is_temporary);

        let written_in = self.accesses_to_usage_locations(
            &var_node.meta.all_writes(), passage_positions,
        );
        let read_in = self.accesses_to_usage_locations(
            &var_node.meta.all_reads(), passage_positions,
        );

        let is_unused = !var_node.meta.refs.iter().any(|a| a.is_read() && !a.is_write());
        let initialized_at_start = var_node.meta.seeded_by_special;

        let properties = self.build_property_nodes(var_id, &name, &state_path, passage_positions);
        let kind = self.infer_kind_from_arena_node(var_id);

        VariableTreeNode {
            name,
            state_path,
            is_temporary,
            written_in,
            read_in,
            initialized_at_start,
            is_unused,
            properties,
            kind,
            element_shape: None,
        }
    }

    /// Build `VariablePropertyNode` children from an arena node's children.
    fn build_property_nodes(
        &self,
        parent_id: NodeId,
        parent_full_name: &str,
        parent_state_path: &str,
        passage_positions: &PassagePositionMap,
    ) -> Vec<VariablePropertyNode> {
        let mut result = Vec::new();

        // Collect and sort children by name for stable output
        let mut children: Vec<(String, NodeId)> = self
            .arena
            .children_of(parent_id)
            .map(|child_id| (self.arena.get(child_id).name.clone(), child_id))
            .collect();
        children.sort_by(|a, b| a.0.cmp(&b.0));

        for (child_name, child_id) in children {
            let child_node = self.arena.get(child_id);
            let full_name = format!("{}.{}", parent_full_name, child_name);
            let state_path = format!("{}.{}", parent_state_path, child_name);

            let written_in = self.accesses_to_usage_locations(
                &child_node.meta.all_writes(), passage_positions,
            );
            let read_in = self.accesses_to_usage_locations(
                &child_node.meta.all_reads(), passage_positions,
            );

            let rel_line = child_node
                .meta
                .direct_writes()
                .first()
                .map(|a| a.line)
                .or_else(|| child_node.meta.refs.first().map(|a| a.line))
                .unwrap_or(0);
            let line = child_node
                .meta
                .refs
                .first()
                .and_then(|a| passage_positions.get(&(a.file_uri.clone(), a.passage_name.clone())))
                .map(|pos| pos.body_start_line + rel_line)
                .unwrap_or(rel_line);

            let properties = self.build_property_nodes(child_id, &full_name, &state_path, passage_positions);
            let kind = self.infer_kind_from_arena_node(child_id);

            result.push(VariablePropertyNode {
                name: child_name,
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

    /// Convert accesses to `VariableUsageLocation` values.
    fn accesses_to_usage_locations(
        &self,
        accesses: &[&VarAccess],
        passage_positions: &PassagePositionMap,
    ) -> Vec<VariableUsageLocation> {
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
                    span: Some(a.span.clone()),
                }
            })
            .collect()
    }

    /// Infer the `PropertyKind` of an arena node from its children.
    fn infer_kind_from_arena_node(&self, id: NodeId) -> PropertyKind {
        let has_children = self.arena.get(id).has_children();
        if !has_children {
            return PropertyKind::Unknown;
        }

        // Heuristic: if any child has name "length", "push", or "pop",
        // it's probably an Array
        for child_id in self.arena.children_of(id) {
            let name = &self.arena.get(child_id).name;
            if name == "length" || name == "push" || name == "pop" {
                return PropertyKind::Array;
            }
        }
        PropertyKind::Object
    }

    /// Iterate over all root variable entries.
    /// Returns (name, NodeId) pairs for persistent variables.
    pub fn iter(&self) -> impl Iterator<Item = (String, NodeId)> {
        self.arena
            .children_of(self.persistent_root)
            .map(|id| (self.arena.get(id).name.clone(), id))
    }

    /// Backward-compatible convenience: record a variable access with a
    /// simple read/write boolean.
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
        self.record_var(name, is_temporary, kind, passage_name, file_uri, span, property_path, body_text, &[], None);
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

    /// Get the set of known property names for a variable (for dot-notation completion).
    ///
    /// Returns immediate child names for the variable identified by `name`
    /// (e.g., `{"name", "hp"}` for `$player`). Searches both persistent
    /// and temporary variable roots.
    pub fn known_properties(&self, name: &str) -> HashSet<String> {
        if let Some((var_id, _)) = self.get_variable(name) {
            self.arena
                .children_of(var_id)
                .map(|child_id| self.arena.get(child_id).name.clone())
                .collect()
        } else {
            HashSet::new()
        }
    }

    /// Collect all unique file URIs from all variable accesses in the tree.
    ///
    /// Walks every node in the arena and collects URIs from access records.
    /// Used by `compute_passage_positions` to determine which source files
    /// need to be scanned.
    pub fn collect_file_uris(&self) -> HashSet<String> {
        let mut uris = HashSet::new();
        // Only iterate live nodes — skip freed slots to avoid
        // reading stale refs from reused slots.
        for id in self.arena.iter_ids() {
            if self.is_live_node(id) {
                let node = self.arena.get(id);
                for access in &node.meta.refs {
                    uris.insert(access.file_uri.clone());
                }
            }
        }
        uris
    }

    /// Get mutable access to a variable node by name.
    ///
    /// Returns `Some(NodeId)` if found, `None` otherwise.
    /// The caller can then use `arena_mut().get_mut(id)` to modify the node.
    pub fn get_variable_mut_id(&mut self, name: &str) -> Option<NodeId> {
        self.path_index.get(name).copied()
    }

    /// Find the variable path at a given body-relative byte offset.
    ///
    /// Scans all `VarAccess` records in the tree for the given `file_uri` and
    /// `passage_name`, checking each access's `segment_spans` to find which
    /// path segment contains the offset. Returns the full variable path up to
    /// (and including) the matching segment.
    ///
    /// This is designed for span-based dot-notation completion: when the user
    /// types `$player.state.` and the cursor is right after the dot, this
    /// method finds the `$player.state` path by matching the offset against
    /// the `segment_spans` of accesses in that passage.
    ///
    /// The offset should be **passage-body-relative** (0 = first byte after
    /// the `:: Name` header line). Returns `None` if no segment spans contain
    /// the offset.
    pub fn path_at_offset(
        &self,
        file_uri: &str,
        passage_name: &str,
        body_offset: usize,
    ) -> Option<String> {
        let mut best_path: Option<String> = None;
        let mut best_end: usize = 0;

        for id in self.arena.iter_ids() {
            if !self.is_live_node(id) {
                continue;
            }
            let node = self.arena.get(id);
            for access in &node.meta.refs {
                // Skip accesses from other files/passages
                if access.file_uri != file_uri || access.passage_name != passage_name {
                    continue;
                }

                // Check segment_spans — each segment corresponds to a path
                // component: [0] = root var span, [1..] = property spans.
                // Build the path incrementally as we check each segment.
                if access.segment_spans.is_empty() {
                    continue;
                }

                // Check if the offset falls within any segment span, or
                // right after the last segment span (cursor after the dot).
                for (seg_idx, seg_span) in access.segment_spans.iter().enumerate() {
                    // Offset is within this segment's span
                    if body_offset >= seg_span.start && body_offset <= seg_span.end {
                        // Reconstruct the path up to this segment
                        let path = self.build_path_for_segment(id, seg_idx, &access.segment_spans);
                        if let Some(ref p) = path {
                            // Prefer longer (more specific) paths
                            if seg_span.end > best_end {
                                best_path = Some(p.clone());
                                best_end = seg_span.end;
                            }
                        }
                    }
                }

                // Also check if offset is right after the last segment span
                // (cursor after the final dot, which isn't in any segment)
                if let Some(last_span) = access.segment_spans.last() {
                    // Allow up to 5 bytes gap for the dot separator
                    if body_offset > last_span.end && body_offset <= last_span.end + 5 {
                        let path = self.build_path_for_segment(
                            id,
                            access.segment_spans.len() - 1,
                            &access.segment_spans,
                        );
                        if let Some(ref p) = path {
                            if last_span.end > best_end {
                                best_path = Some(p.clone());
                                best_end = last_span.end;
                            }
                        }
                    }
                }
            }
        }

        best_path
    }

    /// Build the full variable path for a given segment index.
    ///
    /// Given a node (which may be at any depth in the tree) and a target
    /// segment index, reconstruct the fully-qualified dot-path up to and
    /// including that segment. For example, on the `$item` node with
    /// `seg_idx = 2`, this returns `"$item.work.pen"`.
    ///
    /// ## Strategy
    ///
    /// The old approach walked UP from the node to the root, but this
    /// broke for deep paths: when the node IS the root variable, walking
    /// up hits `persistent_root` immediately and only produces one
    /// component regardless of `seg_idx`.
    ///
    /// The correct approach is:
    /// 1. Walk UP to find the root variable node (child of persistent/temp root).
    /// 2. Walk DOWN from the root variable through children, using
    ///    `segment_spans` to identify which child corresponds to each segment.
    ///    Since `segment_spans[1]` gives the span of the first property key,
    ///    we match it against children's `ref_sites` spans.
    /// 3. If span matching fails (e.g., no ref_sites), fall back to building
    ///    the path from the node we started on by walking UP (which works
    ///    when the node is already at the right depth).
    fn build_path_for_segment(
        &self,
        node_id: NodeId,
        seg_idx: usize,
        segment_spans: &[Range<usize>],
    ) -> Option<String> {
        // Step 1: Walk up to find the root variable node
        let mut root_id = node_id;
        loop {
            let node = self.arena.try_get(root_id)?;
            let parent = node.parent;
            if parent == NO_NODE
                || parent == self.persistent_root
                || self.temp_roots.values().any(|&id| id == parent)
            {
                break;
            }
            root_id = parent;
        }

        let root_node = self.arena.try_get(root_id)?;
        let root_name = root_node.name.clone();

        // Step 2: If seg_idx == 0, the path is just the root variable
        if seg_idx == 0 {
            return Some(root_name);
        }

        // Step 3: Walk DOWN from root through children, using segment_spans
        // to identify which child each segment corresponds to.
        if segment_spans.len() > seg_idx {
            let mut path = root_name.clone();
            let mut current_id = root_id;

            for depth in 1..=seg_idx {
                let target_span = &segment_spans[depth];
                let mut found_child = None;

                for child_id in self.arena.children_of(current_id) {
                    let child = self.arena.get(child_id);
                    // Match child by checking if any of its ref_sites or
                    // def_sites overlap with the target segment span.
                    let span_matches = child
                        .meta
                        .nav
                        .ref_sites
                        .iter()
                        .chain(child.meta.nav.def_sites.iter())
                        .any(|site| {
                            // The site span should overlap with or be
                            // contained within the target segment span.
                            site.span.start >= target_span.start
                                && site.span.end <= target_span.end
                        });

                    if span_matches {
                        path = format!("{}.{}", path, child.name);
                        found_child = Some(child_id);
                        break;
                    }
                }

                if let Some(child_id) = found_child {
                    current_id = child_id;
                } else {
                    // Span matching failed. Fall back to walking UP from
                    // the original node (works when the original node is
                    // already at or below the target depth).
                    return self.build_path_for_segment_fallback(node_id, seg_idx, &root_name);
                }
            }

            // Validate: the built path should exist in path_index
            if self.path_index.contains_key(&path) {
                return Some(path);
            }
        }

        // Step 4: Final fallback — walk up from the original node.
        // This works when the original node is a deep child (e.g., `color`)
        // and seg_idx selects an ancestor path.
        self.build_path_for_segment_fallback(node_id, seg_idx, &root_name)
    }

    /// Fallback for `build_path_for_segment`: walk UP from the given node.
    ///
    /// This works when the node is a deep child and `seg_idx` selects a path
    /// that includes some (but not all) of its ancestors. It works because
    /// walking UP from a deep node collects all ancestor names.
    fn build_path_for_segment_fallback(
        &self,
        node_id: NodeId,
        seg_idx: usize,
        root_name: &str,
    ) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        let mut current_id = node_id;

        for _ in 0..=seg_idx {
            let node = self.arena.try_get(current_id)?;
            parts.push(node.name.clone());
            current_id = node.parent;
            if current_id == NO_NODE
                || current_id == self.persistent_root
                || self.temp_roots.values().any(|&id| id == current_id)
            {
                break;
            }
        }

        parts.reverse();

        if parts.is_empty() {
            None
        } else {
            let path = parts.join(".");
            // Validate against path_index
            if self.path_index.contains_key(&path) {
                Some(path)
            } else {
                // Last resort: if the path starts with root_name but isn't
                // in path_index, return it anyway (might be a partial match)
                if path.starts_with(root_name) {
                    Some(path)
                } else {
                    None
                }
            }
        }
    }

    /// Get the children of a variable path as typed completion data.
    ///
    /// Unlike `known_properties()` which returns just the names, this
    /// method returns each child's name along with its inferred kind
    /// (Object, Array, Scalar, Unknown). This is the span-based
    /// equivalent of `build_shape_aware_property_map()` for a single
    /// path — it queries the tree directly without building a full map.
    ///
    /// Returns an empty Vec if the path doesn't exist in the tree.
    ///
    /// **Warning:** for temporary variable paths, this method returns
    /// an empty Vec because `path_index` keys for temps are namespaced
    /// by passage. Use [`children_with_kind_for_passage`] in
    /// completion contexts where the enclosing passage is known.
    ///
    /// [`children_with_kind_for_passage`]: Self::children_with_kind_for_passage
    pub fn children_with_kind(&self, path: &str) -> Vec<(String, PropertyKind)> {
        let Some((node_id, _)) = self.get_node_by_path(path) else {
            return Vec::new();
        };

        self.arena
            .children_of(node_id)
            .map(|child_id| {
                let child = self.arena.get(child_id);
                let child_path = format!("{}.{}", path, child.name);
                let kind = self.infer_kind_for_node(child_id, &child_path);
                (child.name.clone(), kind)
            })
            .collect()
    }

    /// Passage-aware variant of [`children_with_kind`].
    ///
    /// For temp variable paths (`_x.y`), resolves against the declaring
    /// passage's namespaced `path_index` key. For persistent paths,
    /// behaves identically to [`children_with_kind`].
    ///
    /// Returns an empty Vec if the path doesn't exist in the tree (e.g.
    /// temp path with no matching passage scope).
    ///
    /// [`children_with_kind`]: Self::children_with_kind
    pub fn children_with_kind_for_passage(
        &self,
        path: &str,
        passage_name: Option<&str>,
    ) -> Vec<(String, PropertyKind)> {
        let Some((node_id, _)) = self.get_node_by_path_for_passage(path, passage_name) else {
            return Vec::new();
        };

        self.arena
            .children_of(node_id)
            .map(|child_id| {
                let child = self.arena.get(child_id);
                let child_path = format!("{}.{}", path, child.name);
                let kind = self.infer_kind_for_node(child_id, &child_path);
                (child.name.clone(), kind)
            })
            .collect()
    }

    /// Infer the PropertyKind for a node based on its children.
    ///
    /// This mirrors the logic in `build_shape_aware_property_map_impl()`
    /// but works on a single node without building the full map.
    fn infer_kind_for_node(&self, node_id: NodeId, _path: &str) -> PropertyKind {
        // Check if the node's NavIndex has an inferred type
        let node = self.arena.get(node_id);
        if let Some(ref inferred) = node.meta.nav.inferred_type {
            return match inferred {
                InferredType::Object => PropertyKind::Object,
                InferredType::Array => PropertyKind::Array,
                InferredType::Scalar => PropertyKind::Scalar,
                InferredType::Function => PropertyKind::Scalar,
                InferredType::Unknown => {
                    // Fall back to child-based inference
                    self.infer_kind_from_children(node_id)
                }
            };
        }

        // Fall back to child-based inference
        self.infer_kind_from_children(node_id)
    }

    /// Infer PropertyKind from the names of a node's children.
    ///
    /// If the node has children named "length", "push", or "pop",
    /// it's likely an Array. Otherwise, it's an Object (or Scalar
    /// if it has no children).
    fn infer_kind_from_children(&self, node_id: NodeId) -> PropertyKind {
        let children: Vec<String> = self
            .arena
            .children_of(node_id)
            .map(|child_id| self.arena.get(child_id).name.clone())
            .collect();

        if children.is_empty() {
            PropertyKind::Scalar
        } else if children.iter().any(|c| c == "length" || c == "push" || c == "pop") {
            PropertyKind::Array
        } else {
            PropertyKind::Object
        }
    }

    /// Get the inferred `PropertyKind` of a variable at a given path.
    ///
    /// This queries the tree directly for the node at the given path and
    /// returns its inferred kind (Object, Array, Scalar, Unknown). This
    /// is more efficient than `build_shape_aware_property_map()` when only
    /// a single path's kind is needed (e.g., in dot-notation completion to
    /// decide whether to offer array methods vs. object properties).
    ///
    /// Returns `None` if the path doesn't exist in the tree.
    ///
    /// **Warning:** for temporary variable paths, this method returns
    /// `None` because `path_index` keys for temps are namespaced by
    /// passage. Use [`kind_at_path_for_passage`] in completion contexts
    /// where the enclosing passage is known.
    ///
    /// [`kind_at_path_for_passage`]: Self::kind_at_path_for_passage
    pub fn kind_at_path(&self, path: &str) -> Option<PropertyKind> {
        let (node_id, _) = self.get_node_by_path(path)?;
        Some(self.infer_kind_for_node(node_id, path))
    }

    /// Passage-aware variant of [`kind_at_path`].
    ///
    /// For temp variable paths (`_x.y`), resolves against the declaring
    /// passage's namespaced `path_index` key. For persistent paths,
    /// behaves identically to [`kind_at_path`].
    ///
    /// [`kind_at_path`]: Self::kind_at_path
    pub fn kind_at_path_for_passage(
        &self,
        path: &str,
        passage_name: Option<&str>,
    ) -> Option<PropertyKind> {
        let (node_id, _) = self.get_node_by_path_for_passage(path, passage_name)?;
        Some(self.infer_kind_for_node(node_id, path))
    }
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
    // Arena-based VariableTree tests
    // =======================================================================

    #[test]
    fn arena_record_and_retrieve_variable() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///test.tw", 50..53, "", BT, &[], None);

        let (id, node) = tree.get_variable("$hp").unwrap();
        assert_eq!(node.name, "$hp");
        assert_eq!(node.meta.refs.len(), 2);
        assert!(!node.is_temporary);

        // Verify via path_index
        let (id2, _) = tree.get_node_by_path("$hp").unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn arena_variable_names() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("$gold", false, VarAccessKind::Write, "Start", "file:///test.tw", 20..25, "", BT, &[], None);

        let names = tree.variable_names();
        assert!(names.contains(&"$hp".to_string()));
        assert!(names.contains(&"$gold".to_string()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn arena_property_tracking_creates_tree() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..17, "name", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 20..27, "hp", BT, &[], None);

        let (var_id, var_node) = tree.get_variable("$player").unwrap();
        assert_eq!(var_node.name, "$player");

        // Children: "name" and "hp"
        let children: Vec<String> = tree.arena().children_of(var_id)
            .map(|id| tree.arena().get(id).name.clone())
            .collect();
        assert!(children.contains(&"name".to_string()));
        assert!(children.contains(&"hp".to_string()));
        assert_eq!(children.len(), 2);

        // Verify known_properties
        let props = tree.known_properties("$player");
        assert!(props.contains("name"));
        assert!(props.contains("hp"));
    }

    #[test]
    fn arena_deep_nesting() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..30, "hp.max", BT, &[], None);

        // Should create: $player -> hp -> max
        let (var_id, _) = tree.get_variable("$player").unwrap();

        // Find "hp" child
        let hp_id = tree.arena().find_child_by_name(var_id, "hp").unwrap();
        let hp_node = tree.arena().get(hp_id);
        assert_eq!(hp_node.name, "hp");

        // Find "max" child of "hp"
        let max_id = tree.arena().find_child_by_name(hp_id, "max").unwrap();
        let max_node = tree.arena().get(max_id);
        assert_eq!(max_node.name, "max");

        // Leaf should have a direct write
        assert!(max_node.meta.refs.iter().any(|a| a.is_write() && !a.propagated));

        // "hp" should have a propagated write
        assert!(hp_node.meta.refs.iter().any(|a| a.is_write() && a.propagated));

        // "$player" should have a propagated write
        let player_node = tree.arena().get(var_id);
        assert!(player_node.meta.refs.iter().any(|a| a.is_write() && a.propagated));
    }

    #[test]
    fn arena_propagation_read_and_write() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..20, "hp", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Read, "Fight", "file:///test.tw", 30..40, "hp", BT, &[], None);

        let (var_id, var_node) = tree.get_variable("$player").unwrap();

        // Root node: 1 propagated write + 1 propagated read
        let writes: Vec<_> = var_node.meta.refs.iter().filter(|a| a.is_write()).collect();
        let reads: Vec<_> = var_node.meta.refs.iter().filter(|a| a.is_read()).collect();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].propagated);
        assert_eq!(reads.len(), 1);
        assert!(reads[0].propagated);

        // "hp" child: 1 direct write + 1 direct read
        let hp_id = tree.arena().find_child_by_name(var_id, "hp").unwrap();
        let hp_node = tree.arena().get(hp_id);
        let hp_direct_writes: Vec<_> = hp_node.meta.refs.iter()
            .filter(|a| a.is_write() && !a.propagated).collect();
        let hp_direct_reads: Vec<_> = hp_node.meta.refs.iter()
            .filter(|a| a.is_read() && !a.propagated).collect();
        assert_eq!(hp_direct_writes.len(), 1);
        assert_eq!(hp_direct_reads.len(), 1);
    }

    #[test]
    fn arena_mixed_direct_and_propagated() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 5..12, "", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 15..25, "hp", BT, &[], None);

        let (_var_id, var_node) = tree.get_variable("$player").unwrap();

        let direct_writes: Vec<_> = var_node.meta.refs.iter()
            .filter(|a| a.is_write() && !a.propagated).collect();
        let prop_writes: Vec<_> = var_node.meta.refs.iter()
            .filter(|a| a.is_write() && a.propagated).collect();
        assert_eq!(direct_writes.len(), 1);
        assert_eq!(prop_writes.len(), 1);
    }

    #[test]
    fn arena_temporary_variable() {
        let mut tree = VariableTree::new();
        tree.record_var("_i", true, VarAccessKind::Write, "Loop", "file:///test.tw", 5..7, "", BT, &[], None);

        let (_id, node) = tree.get_variable("_i").unwrap();
        assert!(node.is_temporary);
        assert_eq!(node.name, "_i");
    }

    #[test]
    fn arena_mark_seeded() {
        let mut tree = VariableTree::new();
        tree.record_var("$gold", false, VarAccessKind::Write, "StoryInit", "file:///test.tw", 10..15, "", BT, &[], None);
        tree.mark_seeded("$gold");

        let (_, node) = tree.get_variable("$gold").unwrap();
        assert!(node.meta.seeded_by_special);

        // Verify via build_tree (initialized_at_start flag)
        let nodes = tree.build_tree(&PassagePositionMap::new());
        let gold = nodes.iter().find(|n| n.name == "$gold").unwrap();
        assert!(gold.initialized_at_start);
    }

    #[test]
    fn arena_completion_names() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("_i", true, VarAccessKind::Write, "Loop", "file:///test.tw", 5..7, "", BT, &[], None);

        let names = tree.completion_names();
        assert!(names.contains("$hp"));
        assert!(names.contains("_i"));
    }

    #[test]
    fn arena_property_map() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..17, "name", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 20..27, "hp", BT, &[], None);
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 30..33, "", BT, &[], None);

        let map = tree.property_map();
        assert!(map.contains_key("$player"));
        assert!(map["$player"].contains("name"));
        assert!(map["$player"].contains("hp"));
        assert!(!map.contains_key("$hp")); // $hp has no children
    }

    #[test]
    fn arena_remove_file() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///a.tw", 10..13, "", BT, &[], None);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///b.tw", 50..53, "", BT, &[], None);

        tree.remove_file("file:///a.tw");
        let (_, node) = tree.get_variable("$hp").unwrap();
        assert_eq!(node.meta.refs.len(), 1);
        assert_eq!(node.meta.refs[0].passage_name, "Forest");
    }

    #[test]
    fn arena_remove_file_with_properties() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///a.tw", 10..25, "hp", BT, &[], None);

        tree.remove_file("file:///a.tw");
        // Variable and all children should be pruned
        assert!(tree.get_variable("$player").is_none());
    }

    #[test]
    fn arena_remove_passage() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///a.tw", 10..13, "", BT, &[], None);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///a.tw", 50..53, "", BT, &[], None);

        tree.remove_passage("Start");
        let (_, node) = tree.get_variable("$hp").unwrap();
        // Only the Read from "Forest" remains
        assert_eq!(node.meta.refs.len(), 1);
        assert_eq!(node.meta.refs[0].passage_name, "Forest");
    }

    #[test]
    fn arena_clear() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("$gold", false, VarAccessKind::Write, "Start", "file:///test.tw", 20..25, "", BT, &[], None);

        tree.clear();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert!(tree.completion_names().is_empty());
    }

    #[test]
    fn arena_compute_graph_order() {
        let mut tree = VariableTree::new();
        tree.record_var("$gold", false, VarAccessKind::Write, "StoryInit", "file:///test.tw", 10..15, "", BT, &[], None);
        tree.record_var("$gold", false, VarAccessKind::Read, "Start", "file:///test.tw", 20..25, "", BT, &[], None);
        tree.record_var("$gold", false, VarAccessKind::CompoundWrite, "Forest", "file:///test.tw", 30..35, "", BT, &[], None);

        let special: HashSet<String> = ["StoryInit".to_string()].into_iter().collect();
        tree.compute_graph_order(&special, "Start", &["Forest".to_string()]);

        let (_, node) = tree.get_variable("$gold").unwrap();
        assert_eq!(node.meta.refs[0].graph_order, Some(0)); // StoryInit
        assert_eq!(node.meta.refs[1].graph_order, Some(1)); // Start
        assert_eq!(node.meta.refs[2].graph_order, Some(2)); // Forest
    }

    #[test]
    fn arena_build_tree() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..25, "hp", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Read, "Fight", "file:///test.tw", 30..40, "hp", BT, &[], None);

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
    fn arena_build_tree_deep_nesting() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..30, "hp.max", BT, &[], None);

        let nodes = tree.build_tree(&PassagePositionMap::new());
        let player = nodes.iter().find(|n| n.name == "$player").unwrap();
        assert_eq!(player.properties.len(), 1);

        let hp = &player.properties[0];
        assert_eq!(hp.name, "hp");
        assert_eq!(hp.properties.len(), 1);

        let max = &hp.properties[0];
        assert_eq!(max.name, "max");
        assert!(!max.written_in.is_empty());
    }

    #[test]
    fn arena_passage_relative_line_computation() {
        let mut tree = VariableTree::new();
        let body = "line 0\n<<set $hp to 100>>\nline 2";
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 7..10, "", body, &[], None);

        let (_, node) = tree.get_variable("$hp").unwrap();
        assert_eq!(node.meta.refs[0].line, 1);
    }

    #[test]
    fn arena_path_index_consistency() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..30, "hp.max", BT, &[], None);

        // All paths should be in path_index
        assert!(tree.get_node_by_path("$player").is_some());
        assert!(tree.get_node_by_path("$player.hp").is_some());
        assert!(tree.get_node_by_path("$player.hp.max").is_some());

        // Verify they point to the correct nodes
        let (root_id, root_node) = tree.get_node_by_path("$player").unwrap();
        assert_eq!(root_node.name, "$player");

        let (hp_id, hp_node) = tree.get_node_by_path("$player.hp").unwrap();
        assert_eq!(hp_node.name, "hp");

        let (max_id, max_node) = tree.get_node_by_path("$player.hp.max").unwrap();
        assert_eq!(max_node.name, "max");

        // Verify parent links
        assert_eq!(tree.arena().get(hp_id).parent, root_id);
        assert_eq!(tree.arena().get(max_id).parent, hp_id);
    }

    #[test]
    fn arena_ensure_path_creates_intermediate_nodes() {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new("<root>".to_string(), false, NO_NODE, VarScope::Persistent);
        let root_id = arena.alloc(root);

        let leaf_id = arena.ensure_path(root_id, "a.b.c", false, VarScope::Persistent);

        // Verify the path was created
        let a_id = arena.find_child_by_name(root_id, "a").unwrap();
        let b_id = arena.find_child_by_name(a_id, "b").unwrap();
        let c_id = arena.find_child_by_name(b_id, "c").unwrap();
        assert_eq!(c_id, leaf_id);

        // Verify parent links
        assert_eq!(arena.get(a_id).parent, root_id);
        assert_eq!(arena.get(b_id).parent, a_id);
        assert_eq!(arena.get(c_id).parent, b_id);
    }

    #[test]
    fn arena_resolve_path_existing() {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new("<root>".to_string(), false, NO_NODE, VarScope::Persistent);
        let root_id = arena.alloc(root);

        arena.ensure_path(root_id, "x.y.z", false, VarScope::Persistent);

        // resolve_path should find existing nodes
        assert_eq!(arena.resolve_path(root_id, "x.y.z"), Some(arena.find_child_by_name(
            arena.find_child_by_name(
                arena.find_child_by_name(root_id, "x").unwrap(), "y"
            ).unwrap(), "z"
        ).unwrap()));
        assert_eq!(arena.resolve_path(root_id, "x.y"), arena.find_child_by_name(
            arena.find_child_by_name(root_id, "x").unwrap(), "y"
        ));
        assert_eq!(arena.resolve_path(root_id, "nonexistent"), None);
    }

    #[test]
    fn arena_children_of_iteration() {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new("<root>".to_string(), false, NO_NODE, VarScope::Persistent);
        let root_id = arena.alloc(root);

        // Add children in order
        for name in &["c", "a", "b"] {
            let node = VarArenaNode::new(name.to_string(), false, root_id, VarScope::Persistent);
            arena.insert_child(root_id, node);
        }

        // Children should iterate in insertion order
        let child_names: Vec<String> = arena.children_of(root_id)
            .map(|id| arena.get(id).name.clone())
            .collect();
        assert_eq!(child_names, vec!["c", "a", "b"]);
    }

    #[test]
    fn arena_unlink_child() {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new("<root>".to_string(), false, NO_NODE, VarScope::Persistent);
        let root_id = arena.alloc(root);

        let node_a = VarArenaNode::new("a".to_string(), false, root_id, VarScope::Persistent);
        let node_b = VarArenaNode::new("b".to_string(), false, root_id, VarScope::Persistent);
        let node_c = VarArenaNode::new("c".to_string(), false, root_id, VarScope::Persistent);
        let _a_id = arena.insert_child(root_id, node_a);
        let b_id = arena.insert_child(root_id, node_b);
        let _c_id = arena.insert_child(root_id, node_c);

        // Unlink "b"
        assert!(arena.unlink_child(root_id, b_id));

        // Children should now be a, c
        let child_names: Vec<String> = arena.children_of(root_id)
            .map(|id| arena.get(id).name.clone())
            .collect();
        assert_eq!(child_names, vec!["a", "c"]);
    }

    #[test]
    fn arena_collect_subtree() {
        let mut arena = VarArena::new();
        let root = VarArenaNode::new("<root>".to_string(), false, NO_NODE, VarScope::Persistent);
        let root_id = arena.alloc(root);

        arena.ensure_path(root_id, "a.b", false, VarScope::Persistent);
        arena.ensure_path(root_id, "c", false, VarScope::Persistent);

        let a_id = arena.find_child_by_name(root_id, "a").unwrap();
        let subtree = arena.collect_subtree(a_id);

        // Should include "a" and "b"
        assert_eq!(subtree.len(), 2);
        let names: Vec<&str> = subtree.iter()
            .map(|id| arena.get(*id).name.as_str())
            .collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn arena_propagate_flags() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..20, "hp.max", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Read, "Fight", "file:///test.tw", 30..40, "hp", BT, &[], None);

        tree.propagate();

        let (var_id, _) = tree.get_variable("$player").unwrap();
        let player = tree.arena().get(var_id);
        assert!(player.meta.has_write_descendant);
        assert!(player.meta.has_read_descendant);

        let hp_id = tree.arena().find_child_by_name(var_id, "hp").unwrap();
        let hp = tree.arena().get(hp_id);
        assert!(hp.meta.has_write_descendant);
        assert!(hp.meta.has_read_descendant);
    }

    #[test]
    fn arena_object_literal_property_paths() {
        let mut tree = VariableTree::new();
        // Simulate what record_var would receive for object literal paths:
        // <<set $ITEMS = { "pencil-skirt-navy": { name: "..." } }>>
        // The JS annotate pass extracts these as separate record_var calls with property_paths
        tree.record_var("$ITEMS", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..15, "pencil-skirt-navy", BT, &[], None);
        tree.record_var("$ITEMS", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..15, "pencil-skirt-navy.name", BT, &[], None);

        let (var_id, _) = tree.get_variable("$ITEMS").unwrap();

        // $ITEMS should have a "pencil-skirt-navy" child
        let psn_id = tree.arena().find_child_by_name(var_id, "pencil-skirt-navy").unwrap();
        assert_eq!(tree.arena().get(psn_id).name, "pencil-skirt-navy");

        // "pencil-skirt-navy" should have a "name" child
        let name_id = tree.arena().find_child_by_name(psn_id, "name").unwrap();
        assert_eq!(tree.arena().get(name_id).name, "name");

        // Path index should have all paths
        assert!(tree.get_node_by_path("$ITEMS").is_some());
        assert!(tree.get_node_by_path("$ITEMS.pencil-skirt-navy").is_some());
        assert!(tree.get_node_by_path("$ITEMS.pencil-skirt-navy.name").is_some());
    }

    #[test]
    fn arena_collect_file_uris() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///a.tw", 10..13, "", BT, &[], None);
        tree.record_var("$hp", false, VarAccessKind::Read, "Forest", "file:///b.tw", 50..53, "", BT, &[], None);

        let uris = tree.collect_file_uris();
        assert!(uris.contains("file:///a.tw"));
        assert!(uris.contains("file:///b.tw"));
    }

    #[test]
    fn arena_record_var_simple() {
        let mut tree = VariableTree::new();
        tree.record_var_simple("$hp", false, true, "Start", "file:///test.tw", 10..13, "", BT);
        tree.record_var_simple("$hp", false, false, "Forest", "file:///test.tw", 50..53, "", BT);

        let (_, node) = tree.get_variable("$hp").unwrap();
        assert_eq!(node.meta.refs.len(), 2);
        assert!(node.meta.refs[0].is_write());
        assert!(node.meta.refs[1].is_read());
    }

    #[test]
    fn arena_iter() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("$gold", false, VarAccessKind::Write, "Start", "file:///test.tw", 20..25, "", BT, &[], None);

        let entries: Vec<_> = tree.iter().collect();
        assert_eq!(entries.len(), 2);
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"$hp"));
        assert!(names.contains(&"$gold"));
    }

    #[test]
    fn arena_len_and_is_empty() {
        let mut tree = VariableTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);

        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);

        tree.record_var("$gold", false, VarAccessKind::Write, "Start", "file:///test.tw", 20..25, "", BT, &[], None);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn arena_known_properties() {
        let mut tree = VariableTree::new();
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..17, "name", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 20..27, "hp.max", BT, &[], None);

        let props = tree.known_properties("$player");
        assert!(props.contains("name"));
        assert!(props.contains("hp")); // "hp" is a direct child
        assert!(!props.contains("max")); // "max" is a child of "hp", not "$player"

        // Non-existent variable
        let empty = tree.known_properties("$nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn arena_dual_scope_temp_and_persistent() {
        let mut tree = VariableTree::new();
        tree.record_var("$hp", false, VarAccessKind::Write, "Start", "file:///test.tw", 10..13, "", BT, &[], None);
        tree.record_var("_i", true, VarAccessKind::Write, "Start", "file:///test.tw", 20..22, "", BT, &[], None);

        // Both should be findable
        let (_, hp_node) = tree.get_variable("$hp").unwrap();
        assert!(!hp_node.is_temporary);

        let (_, i_node) = tree.get_variable("_i").unwrap();
        assert!(i_node.is_temporary);

        // completion_names should include both
        let names = tree.completion_names();
        assert!(names.contains("$hp"));
        assert!(names.contains("_i"));
    }

    /// Regression test: remove_file with deep property paths must not
    /// double-free arena nodes.
    ///
    /// Before the fix, `prune_dead_nodes()` would iterate `dead_paths`
    /// and call `free_subtree()` on each one. When a parent like
    /// `$player` was freed, its descendants (`hp`, `max`) were also
    /// freed. But those descendants' paths were still in `dead_paths`,
    /// causing them to be freed again — corrupting the arena free list
    /// and eventually leading to a server crash when StoryInit passages
    /// with deep property paths were opened.
    #[test]
    fn arena_remove_file_deep_properties_no_double_free() {
        let mut tree = VariableTree::new();

        // Simulate StoryInit writing deep property paths like
        // <<set $player.hp.max to 100>><<set $player.hp.current to 80>>
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 10..30, "hp.max", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 40..65, "hp.current", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 70..85, "name", BT, &[], None);

        // Verify the tree structure
        let (_, player_node) = tree.get_variable("$player").unwrap();
        assert!(player_node.has_children(), "$player should have children");

        // Remove the file — this triggers prune_dead_nodes with the
        // parent and its deep descendants all dead. Without the fix,
        // this causes a double-free.
        tree.remove_file("file:///special.tw");

        // The variable should be completely gone
        assert!(tree.get_variable("$player").is_none(), "$player should be pruned after file removal");

        // Now re-populate with the same file (simulates re-open/re-parse).
        // This exercises the free-list reuse path — if the free list was
        // corrupted by double-frees, alloc() would return stale IDs
        // and the tree would be corrupted.
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 10..30, "hp.max", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 40..65, "hp.current", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "StoryInit", "file:///special.tw", 70..85, "name", BT, &[], None);

        // Verify the tree is valid after re-population
        let (_, player_node) = tree.get_variable("$player").unwrap();
        assert!(player_node.has_children(), "$player should have children after re-population");

        // Verify path_index consistency — no stale entries
        let prop_map = tree.property_map();
        assert!(prop_map.contains_key("$player"));
        assert!(prop_map["$player"].contains("hp"));
        assert!(prop_map["$player"].contains("name"));
    }

    #[test]
    fn arena_children_with_kind_deep_nesting() {
        let mut tree = VariableTree::new();
        // Create: $item -> work -> pen -> color
        tree.record_var("$item", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..40, "work.pen.color", BT, &[], None);

        // Level 0: children of $item
        let root_children = tree.children_with_kind("$item");
        assert_eq!(root_children.len(), 1);
        assert_eq!(root_children[0].0, "work");

        // Level 1: children of $item.work
        let work_children = tree.children_with_kind("$item.work");
        assert_eq!(work_children.len(), 1);
        assert_eq!(work_children[0].0, "pen");

        // Level 2: children of $item.work.pen
        let pen_children = tree.children_with_kind("$item.work.pen");
        assert_eq!(pen_children.len(), 1);
        assert_eq!(pen_children[0].0, "color");

        // Level 3: children of $item.work.pen.color (leaf, no children)
        let color_children = tree.children_with_kind("$item.work.pen.color");
        assert!(color_children.is_empty());

        // Verify kinds at each level
        assert_eq!(tree.kind_at_path("$item"), Some(PropertyKind::Object));
        assert_eq!(tree.kind_at_path("$item.work"), Some(PropertyKind::Object));
        assert_eq!(tree.kind_at_path("$item.work.pen"), Some(PropertyKind::Object));
        assert_eq!(tree.kind_at_path("$item.work.pen.color"), Some(PropertyKind::Scalar));
    }

    #[test]
    fn arena_children_with_kind_multiple_properties() {
        let mut tree = VariableTree::new();
        // $player has hp, name, and address.city
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 10..20, "hp", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 20..30, "name", BT, &[], None);
        tree.record_var("$player", false, VarAccessKind::Write, "Init", "file:///test.tw", 30..50, "address.city", BT, &[], None);

        // Root level: should have 3 children
        let root_children = tree.children_with_kind("$player");
        assert_eq!(root_children.len(), 3);
        let child_names: Vec<&str> = root_children.iter().map(|(n, _)| n.as_str()).collect();
        assert!(child_names.contains(&"hp"));
        assert!(child_names.contains(&"name"));
        assert!(child_names.contains(&"address"));

        // address level: should have city
        let addr_children = tree.children_with_kind("$player.address");
        assert_eq!(addr_children.len(), 1);
        assert_eq!(addr_children[0].0, "city");

        // city level: should be a leaf (scalar)
        let city_children = tree.children_with_kind("$player.address.city");
        assert!(city_children.is_empty());
    }
}
