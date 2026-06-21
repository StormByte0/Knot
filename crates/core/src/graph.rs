//! Graph Mathematics Engine
//!
//! The workspace is represented as a directed graph where nodes are passages
//! and edges are links between passages. This module provides:
//!
//! - Broken link detection
//! - Game loop detection (Tarjan's SCC)
//! - Unreachable passage detection (BFS from entry point)
//! - Variable flow analysis support

use crate::passage::{PassageCategory, SpecialPassageBehavior, SpecialPassageLayer};
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Node data stored in the passage graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageNode {
    /// The passage name.
    pub name: String,
    /// The URI of the document containing this passage.
    pub file_uri: String,
    /// Whether this passage is special (format-defined).
    pub is_special: bool,
    /// Whether this passage is metadata (StoryData/StoryTitle).
    pub is_metadata: bool,
    /// Whether this is a placeholder node created for a link target that
    /// doesn't exist yet. Placeholder nodes have empty `file_uri` and
    /// should be excluded from reachability analysis and graph exports.
    #[serde(default)]
    pub is_placeholder: bool,
    /// The ownership layer of this passage, if it is a special passage.
    ///
    /// This enables graph isolation: special passages (TwineCore, LegacyCore,
    /// StoryFormat) form their own upstream chain separate from user-defined
    /// passages. The graph analysis uses this to:
    ///
    /// - Skip special passages in reachability/orphan analysis
    /// - Build the upstream chain: TwineCore → StoryFormat (no edges to
    ///   user-defined passages)
    /// - Distinguish edge types in the story map visualization
    ///
    /// Returns `None` for regular (non-special) user-defined passages.
    #[serde(default)]
    pub layer: Option<SpecialPassageLayer>,
    /// The classification category of this passage.
    ///
    /// This is the preferred way to determine a passage's classification
    /// tier in the priority hierarchy. It provides more granular information
    /// than `is_special` (which is just a boolean). Handlers should prefer
    /// `category` over `is_special` for conditional logic.
    #[serde(default)]
    pub category: PassageCategory,
    /// The behavior of this special passage, if it is one.
    ///
    /// This enables the graph to construct implicit lifecycle edges without
    /// re-scanning workspace passages or the format plugin's definitions.
    /// The graph becomes self-sufficient for special passage queries.
    ///
    /// Returns `None` for regular (non-special) user-defined passages.
    #[serde(default)]
    pub behavior: Option<SpecialPassageBehavior>,
}

/// Pre-classified bundle of special passages, organized by behavior.
///
/// This bundle is maintained incrementally as nodes are added/removed from
/// the graph, so handlers and graph construction code never need to re-scan
/// all workspace passages to find special passages by behavior. The graph
/// is self-sufficient for special passage queries.
///
/// ## Usage
///
/// Instead of iterating all workspace documents and checking
/// `passage.special_def.behavior`, query the bundle:
///
/// ```ignore
/// // OLD: iterate workspace passages
/// for doc in workspace.documents() {
///     for passage in &doc.passages {
///         if matches!(passage.special_def, Some(def) if matches!(def.behavior, ScriptInjection)) {
///             script_passages.push(passage.name.clone());
///         }
///     }
/// }
///
/// // NEW: query the bundle
/// let script_passages = &graph.special_bundle.script_injection;
/// ```
///
/// ## Maintenance
///
/// The bundle is updated automatically by `PassageGraph::add_passage()` and
/// `PassageGraph::remove_passage()`. Callers should never modify the bundle
/// directly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecialPassageBundle {
    /// ScriptInjection passages (TwineCore/LegacyCore [script] tagged,
    /// or named "script"). These run before startup passages and
    /// contribute variables. Example: "Story JavaScript".
    #[serde(default)]
    pub script_injection: Vec<String>,
    /// Startup passages (StoryInit, [init] tagged). These run after
    /// script injection but before the start passage.
    #[serde(default)]
    pub startup: Vec<String>,
    /// Chrome passages (StoryCaption, StoryBanner, StoryMenu, etc.).
    /// These render in the UI chrome area on every navigation.
    #[serde(default)]
    pub chrome: Vec<String>,
    /// ChromeInterceptor passages (PassageHeader, PassageFooter).
    /// These wrap every rendered passage body.
    #[serde(default)]
    pub chrome_interceptor: Vec<String>,
    /// StructureTemplate passages (StoryInterface).
    /// These define the HTML shell for the entire story.
    #[serde(default)]
    pub structure_template: Vec<String>,
    /// Metadata passages (StoryData, StoryTitle).
    /// These provide metadata only, no content rendering.
    #[serde(default)]
    pub metadata: Vec<String>,
    /// PassageReady passages (PassageReady, PassageDone).
    /// These run on every navigation.
    #[serde(default)]
    pub passage_ready: Vec<String>,
    /// All special passage names (union of the above lists).
    /// Useful for "is this special?" checks without matching behavior.
    #[serde(default)]
    pub all_special_names: HashSet<String>,
}

/// Classification of an edge's semantic type.
///
/// Replaces the old `is_broken`/`is_upstream` boolean pair with a proper
/// enum that distinguishes navigation, call, include, jump, upstream
/// lifecycle, and broken edges. The story map uses this to render edges
/// with distinct visual styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeType {
    /// A navigation link: `[[Target]]`, `<<link>>`, `<<button>>`, `<<goto>>`.
    /// Any edge where the player navigates (or is redirected) to another passage.
    Navigation,
    /// An upstream lifecycle edge (not a user-navigable link).
    /// Connects special passages in execution order
    /// (TwineCore → StoryFormat → Start). Internal — not user-facing.
    Upstream,
    /// A passage inclusion: `<<include>>`, `data-passage="..."`.
    /// The included passage content is rendered inline, not navigated to.
    Include,
    /// A broken link whose target passage doesn't exist.
    /// Internal diagnostic state — restored to the original type when the
    /// target is created.
    Broken,
}

impl Default for EdgeType {
    fn default() -> Self {
        EdgeType::Navigation
    }
}

/// Edge data representing a link between passages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageEdge {
    /// The display text of the link (if any).
    pub display_text: Option<String>,
    /// The semantic type of this edge (navigation, call, include, jump,
    /// upstream lifecycle, or broken).
    #[serde(default)]
    pub edge_type: EdgeType,
    /// The original edge type before it was overwritten to `EdgeType::Broken`.
    ///
    /// When a link's target passage doesn't exist, `edge_type` is set to
    /// `Broken`. This field preserves the *original* semantic type (Jump,
    /// Call, Include, Navigation) so that `recheck_broken_links()` can
    /// restore it correctly when the target is later created. Without this,
    /// a `<<goto>>` whose target is created would incorrectly become
    /// `Navigation` instead of `Jump`.
    ///
    /// `None` means the edge was never Broken (or was Broken from the start
    /// with no prior type to restore). The field is not serialized to the
    /// wire protocol — it is internal to graph bookkeeping.
    #[serde(default, skip_serializing)]
    pub pre_broken_type: Option<EdgeType>,
}

/// Diagnostic produced by graph analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphDiagnostic {
    /// The passage name this diagnostic is associated with.
    pub passage_name: String,
    /// The URI of the document containing this passage.
    pub file_uri: String,
    /// The diagnostic kind.
    pub kind: DiagnosticKind,
    /// Human-readable message.
    pub message: String,
}

/// Information about a detected game loop (strongly connected component).
///
/// In Twine, cycles are the core interaction pattern (game loops), not bugs.
/// This struct carries the SCC analysis results that the story map uses
/// for loop visualization, replacing the old `DiagnosticKind::InfiniteLoop`
/// which incorrectly treated cycles as warnings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameLoopInfo {
    /// The passages that participate in this cycle.
    pub members: Vec<String>,
    /// The identified loop header passage (dominates the back-edge source).
    /// This is the passage that controls whether the loop repeats.
    /// `None` if dominance analysis couldn't identify a single header.
    pub header: Option<String>,
    /// Whether the cycle contains persistent variable writes.
    /// If false, the loop has no state mutation and will repeat
    /// infinitely (a genuine infinite loop, not a game loop).
    pub has_mutation: bool,
}

/// Kinds of diagnostics the graph engine can produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// A link points to a passage that doesn't exist.
    BrokenLink,
    /// A passage cannot be reached from the entry point.
    UnreachablePassage,

    /// A variable may be used before initialization.
    UninitializedVariable,
    /// A variable is written but never read on any reachable path.
    UnusedVariable,
    /// A variable is written twice without an intervening read within a passage.
    RedundantWrite,
    /// Multiple StoryData passages found.
    DuplicateStoryData,
    /// StoryData passage is missing.
    MissingStoryData,
    /// The declared start passage doesn't exist.
    MissingStartPassage,
    /// An unsupported story format was declared.
    UnsupportedFormat,
    /// A passage has the same name as another passage.
    DuplicatePassageName,
    /// A passage has no content in its body.
    EmptyPassage,
    /// A passage has no outgoing links and is not a known ending.
    DeadEndPassage,
    /// A passage name contains spaces or special characters.
    InvalidPassageName,
    /// A passage has a high cyclomatic complexity (too many links or
    /// conditionals, making it hard to follow or test).
    ComplexPassage,
    /// A passage body exceeds a recommended size threshold.
    LargePassage,
    /// The start passage has no outgoing links (the story immediately
    /// ends with no choices).
    MissingStartLink,
    /// A variable may not be available in a passage (format-delegated hint).
    /// This replaces `UninitializedVariable` for formats with persistent
    /// state variables (where variables survive across passage transitions).
    VariableAvailabilityHint,
    /// A variable is written but never read (format-delegated hint).
    UnusedVariableHint,
    /// A variable is assigned twice without an intervening read (format-delegated hint).
    RedundantWriteHint,
    /// A property path is read but never written (format-delegated hint).
    UnknownPropertyHint,
}

/// The passage graph — the core data structure for narrative analysis.
#[derive(Debug, Clone)]
pub struct PassageGraph {
    /// The underlying directed graph.
    graph: DiGraph<PassageNode, PassageEdge>,
    /// Mapping from passage name to node index.
    name_to_idx: HashMap<String, NodeIndex>,
    /// Pre-classified bundle of special passages, maintained incrementally.
    ///
    /// This enables the graph to answer special passage queries without
    /// re-scanning workspace documents or the format plugin's definitions.
    /// Updated automatically by `add_passage()` and `remove_passage()`.
    pub special_bundle: SpecialPassageBundle,
}

impl PassageGraph {
    /// Create an empty passage graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_idx: HashMap::new(),
            special_bundle: SpecialPassageBundle::default(),
        }
    }

    /// Add a passage as a node to the graph.
    /// Returns the node index. If a passage with the same name already exists,
    /// it is replaced and the special bundle is updated accordingly.
    pub fn add_passage(&mut self, node: PassageNode) -> NodeIndex {
        let node_name = node.name.clone();

        // If replacing an existing node, remove its old bundle entry first.
        // We clone the old node's data into locals before calling
        // remove_from_bundle to avoid borrowing self immutably (via
        // old_node) and mutably (via remove_from_bundle) at the same time.
        if let Some(&idx) = self.name_to_idx.get(&node_name) {
            let (old_name, old_behavior) = {
                let old_node = &self.graph[idx];
                (old_node.name.clone(), old_node.behavior.clone())
            };
            self.remove_from_bundle(&old_name, old_behavior.as_ref());
            self.graph[idx] = node;
        } else {
            let idx = self.graph.add_node(node);
            self.name_to_idx.insert(node_name.clone(), idx);
        }

        // Add the new node to the bundle. We snapshot the node from the
        // graph so the immutable borrow ends before add_to_bundle takes
        // &mut self.
        let idx = self.name_to_idx[&node_name];
        let node_snapshot = self.graph[idx].clone();
        self.add_to_bundle(&node_snapshot);

        idx
    }

    /// Remove a passage node from the graph by name.
    /// Also removes all edges connected to this node and updates the bundle.
    pub fn remove_passage(&mut self, name: &str) -> Option<PassageNode> {
        let idx = self.name_to_idx.remove(name)?;
        let node = self.graph.remove_node(idx);
        if let Some(ref n) = node {
            self.remove_from_bundle(&n.name, n.behavior.as_ref());
        }
        // Rebuild name_to_idx since node indices shift after removal
        self.rebuild_index();
        node
    }

    /// Add an edge (link) from one passage to another.
    /// If either passage doesn't exist, the edge is still added (for broken link detection).
    ///
    /// Deduplicates: if an edge with the same source, target, and display_text
    /// already exists, the new edge is NOT added. This prevents accumulation of
    /// duplicate edges during graph surgery or repeated parse cycles.
    pub fn add_edge(&mut self, from: &str, to: &str, edge: PassageEdge) {
        let from_idx = self.get_or_create_node(from);
        let to_idx = self.get_or_create_node(to);

        // Check for an existing edge with the same source, target, and display_text
        let already_exists = self
            .graph
            .edges_connecting(from_idx, to_idx)
            .any(|existing| {
                existing.weight().display_text == edge.display_text
            });

        if !already_exists {
            self.graph.add_edge(from_idx, to_idx, edge);
        }
    }

    /// Remove all edges originating from a given passage.
    pub fn remove_edges_from(&mut self, name: &str) {
        if let Some(&idx) = self.name_to_idx.get(name) {
            let edges: Vec<_> = self
                .graph
                .edges_directed(idx, petgraph::Direction::Outgoing)
                .map(|e| e.id())
                .collect();
            for edge in edges {
                self.graph.remove_edge(edge);
            }
        }
    }

    /// Get the node index for a passage name, or create a placeholder.
    ///
    /// Placeholder nodes represent link targets that don't exist in the
    /// workspace yet. They are marked with `is_placeholder: true` and
    /// excluded from reachability analysis and graph exports.
    fn get_or_create_node(&mut self, name: &str) -> NodeIndex {
        if let Some(&idx) = self.name_to_idx.get(name) {
            idx
        } else {
            let node = PassageNode {
                name: name.to_string(),
                file_uri: String::new(),
                is_special: false,
                is_metadata: false,
                is_placeholder: true,
                layer: None,
                category: PassageCategory::Regular,
                behavior: None,
            };
            let idx = self.graph.add_node(node);
            self.name_to_idx.insert(name.to_string(), idx);
            // Placeholders are never special, so no bundle update needed
            idx
        }
    }

    /// Rebuild the name-to-index mapping after structural changes.
    fn rebuild_index(&mut self) {
        self.name_to_idx.clear();
        for idx in self.graph.node_indices() {
            let name = self.graph[idx].name.clone();
            self.name_to_idx.insert(name, idx);
        }
    }

    /// Add a node's entry to the special bundle.
    fn add_to_bundle(&mut self, node: &PassageNode) {
        if !node.is_special {
            return;
        }
        self.special_bundle.all_special_names.insert(node.name.clone());
        if let Some(ref behavior) = node.behavior {
            match behavior {
                SpecialPassageBehavior::ScriptInjection => {
                    self.special_bundle.script_injection.push(node.name.clone());
                }
                SpecialPassageBehavior::Startup => {
                    self.special_bundle.startup.push(node.name.clone());
                }
                SpecialPassageBehavior::Chrome => {
                    self.special_bundle.chrome.push(node.name.clone());
                }
                SpecialPassageBehavior::ChromeInterceptor => {
                    self.special_bundle.chrome_interceptor.push(node.name.clone());
                }
                SpecialPassageBehavior::StructureTemplate => {
                    self.special_bundle.structure_template.push(node.name.clone());
                }
                SpecialPassageBehavior::Metadata => {
                    self.special_bundle.metadata.push(node.name.clone());
                }
                SpecialPassageBehavior::PassageReady => {
                    self.special_bundle.passage_ready.push(node.name.clone());
                }
                SpecialPassageBehavior::StyleInjection => {
                    // StyleInjection passages don't need a dedicated list;
                    // they're in all_special_names but have no lifecycle edges.
                }
                SpecialPassageBehavior::Custom(_) => {
                    // Custom behaviors are tracked in all_special_names
                    // but don't have a dedicated bundle list.
                }
            }
        }
    }

    /// Remove a node's entry from the special bundle.
    fn remove_from_bundle(&mut self, name: &str, behavior: Option<&SpecialPassageBehavior>) {
        self.special_bundle.all_special_names.remove(name);
        if let Some(behavior) = behavior {
            match behavior {
                SpecialPassageBehavior::ScriptInjection => {
                    self.special_bundle.script_injection.retain(|n| n != name);
                }
                SpecialPassageBehavior::Startup => {
                    self.special_bundle.startup.retain(|n| n != name);
                }
                SpecialPassageBehavior::Chrome => {
                    self.special_bundle.chrome.retain(|n| n != name);
                }
                SpecialPassageBehavior::ChromeInterceptor => {
                    self.special_bundle.chrome_interceptor.retain(|n| n != name);
                }
                SpecialPassageBehavior::StructureTemplate => {
                    self.special_bundle.structure_template.retain(|n| n != name);
                }
                SpecialPassageBehavior::Metadata => {
                    self.special_bundle.metadata.retain(|n| n != name);
                }
                SpecialPassageBehavior::PassageReady => {
                    self.special_bundle.passage_ready.retain(|n| n != name);
                }
                SpecialPassageBehavior::StyleInjection | SpecialPassageBehavior::Custom(_) => {
                    // No dedicated list to update
                }
            }
        }
    }

    /// Detect broken links — edges whose target passage doesn't exist in the graph
    /// as a real node (i.e., no document URI).
    ///
    /// ## Suppressions
    ///
    /// Broken link diagnostics are suppressed in these cases:
    ///
    /// 1. **ScriptInjection / StyleInjection sources**: Script and stylesheet
    ///    passages contain non-Twine content (JavaScript, CSS). Link extraction
    ///    from these passages is best-effort and prone to false positives
    ///    (e.g., JavaScript string concatenation like `'Use::' + key` being
    ///    misidentified as a passage reference). Suppressing broken link
    ///    diagnostics for these passages avoids noise from inevitable
    ///    extraction inaccuracies.
    ///
    /// 2. **Targets containing `::`**: The `::` sequence is the Twee passage
    ///    header prefix and never appears in passage link targets. Any target
    ///    containing `::` is a JavaScript namespace accessor (e.g., `Use::Item`)
    ///    or string concatenation artifact (e.g., `'Use::' + key`), not a
    ///    real passage name. This is a defense-in-depth filter — the link
    ///    extraction code already filters `::` targets, but this catches any
    ///    that slip through.
    ///
    /// 3. **Upstream edges**: Lifecycle edges (ScriptInjection → Startup,
    ///    Startup → Start) are structural, not user-authored links. They are
    ///    never Broken because they are added only after verifying both
    ///    endpoints exist, so the `edge_type != Broken` filter naturally
    ///    excludes them.
    pub fn detect_broken_links(&self) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();
        for edge_ref in self.graph.edge_references() {
            // Only report diagnostics for Broken edges.
            // Upstream lifecycle edges are never Broken (they are added
            // only after verifying both endpoints exist), so they are
            // naturally excluded by this check.
            if edge_ref.weight().edge_type != EdgeType::Broken {
                continue;
            }

            let source_idx = edge_ref.source();
            let target_idx = edge_ref.target();
            let source = &self.graph[source_idx];
            let target = &self.graph[target_idx];

            // Skip broken link diagnostics for ScriptInjection and
            // StyleInjection source passages.
            if let Some(ref behavior) = source.behavior {
                if matches!(
                    behavior,
                    SpecialPassageBehavior::ScriptInjection
                        | SpecialPassageBehavior::StyleInjection
                ) {
                    continue;
                }
            }

            // Skip targets containing "::".
            if target.name.contains("::") {
                continue;
            }

            diagnostics.push(GraphDiagnostic {
                passage_name: source.name.clone(),
                file_uri: source.file_uri.clone(),
                kind: DiagnosticKind::BrokenLink,
                message: format!(
                    "Link target '{}' not found in workspace",
                    target.name
                ),
            });
        }
        diagnostics
    }

    /// Detect unreachable passages using BFS from the start passage.
    /// Returns diagnostics for passages that cannot be reached.
    ///
    /// ## Special Passage Reachability
    ///
    /// Special passages themselves are always considered reachable (they're
    /// invoked by the engine at specific lifecycle points). However, they
    /// may contain explicit references to user-defined passages (e.g.,
    /// `data-passage` in StoryInterface, `Engine.play()` in Story JavaScript,
    /// `<<goto>>` in StoryInit). These referenced passages must be considered
    /// reachable too, since the engine will navigate to them.
    ///
    /// To handle this, the BFS starts from the start passage AND from all
    /// special passages that have `participates_in_graph: true` or that
    /// are in the upstream chain (ScriptInjection, Startup). Passages
    /// reachable from any of these entry points are considered reachable.
    pub fn detect_unreachable(&self, start_passage: &str) -> Vec<GraphDiagnostic> {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();

        // Seed BFS from the start passage
        if let Some(&start_idx) = self.name_to_idx.get(start_passage) {
            reachable.insert(start_idx);
            queue.push_back(start_idx);
        }

        // Also seed BFS from special passages that have outgoing edges
        // to user-defined passages. These passages are always invoked by
        // the engine, so any passage they reference is reachable.
        //
        // This handles:
        // - StoryInterface with data-passage="SidebarStats" (Include edge: outgoing)
        // - Story JavaScript with Engine.play("Forest") (Navigation edge: outgoing)
        // - StoryInit with <<goto "Somewhere">> (Navigation edge: outgoing)
        // - PassageHeader/PassageFooter with data-passage refs (Include edge: outgoing)
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            if node.is_special && !node.is_metadata && !node.is_placeholder {
                // Check if this special passage has any outgoing edges
                // to non-special passages (both Navigation and Include)
                let has_user_refs = self
                    .graph
                    .neighbors_directed(idx, petgraph::Direction::Outgoing)
                    .any(|neighbor| !self.graph[neighbor].is_special);
                if has_user_refs {
                    if reachable.insert(idx) {
                        queue.push_back(idx);
                    }
                }
            }
        }

        // BFS: follow ALL outgoing edges (both Navigation and Include).
        // All edges are stored as source → target (the passage containing
        // the reference → the referenced passage). For reachability, this
        // means: if passage A references passage B (via [[link]], <<goto>>,
        // <<include>>, or data-passage), then B is reachable from A.
        while let Some(current) = queue.pop_front() {
            for neighbor in self
                .graph
                .neighbors_directed(current, petgraph::Direction::Outgoing)
            {
                if reachable.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        let mut diagnostics = Vec::new();
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            // Skip metadata passages from unreachable diagnostics
            if node.is_metadata {
                continue;
            }
            // Skip special passages (StoryInit, StoryCaption, etc.) — they
            // are not reachable via normal narrative links but are invoked
            // by the story engine at specific lifecycle points.
            if node.is_special {
                continue;
            }
            // Skip placeholder nodes — they represent link targets that
            // don't exist in the workspace, not real passages.
            if node.is_placeholder {
                continue;
            }
            if !reachable.contains(&idx) {
                diagnostics.push(GraphDiagnostic {
                    passage_name: node.name.clone(),
                    file_uri: node.file_uri.clone(),
                    kind: DiagnosticKind::UnreachablePassage,
                    message: format!(
                        "Passage '{}' is unreachable from start passage '{}'",
                        node.name, start_passage
                    ),
                });
            }
        }
        diagnostics
    }



    /// Export the graph as a serializable structure for the Story Map webview.
    ///
    /// The `passage_tags` map provides tag data for each passage name (collected
    /// from the document model, since the graph only stores passage nodes).
    /// The `unreachable` set contains passage names that are unreachable from
    /// the start passage. The `passage_vars` map provides variable write/read
    /// names per passage for the variable summary fields.
    pub fn export_graph_with_metadata(
        &self,
        passage_tags: &std::collections::HashMap<String, Vec<String>>,
        unreachable: &std::collections::HashSet<String>,
        passage_positions: &std::collections::HashMap<String, (f64, f64)>,
    ) -> GraphExport {
        self.export_graph_with_metadata_and_vars(
            passage_tags,
            unreachable,
            passage_positions,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        )
    }

    /// Full graph export with variable summaries and game loop data.
    ///
    /// This is the preferred export method. It includes all data from
    /// `export_graph_with_metadata` plus per-node variable summaries
    /// and game loop detection results.
    pub fn export_graph_with_metadata_and_vars(
        &self,
        passage_tags: &std::collections::HashMap<String, Vec<String>>,
        unreachable: &std::collections::HashSet<String>,
        passage_positions: &std::collections::HashMap<String, (f64, f64)>,
        var_writes: &std::collections::HashMap<String, Vec<String>>,
        var_reads: &std::collections::HashMap<String, Vec<String>>,
        passage_groups: &std::collections::HashMap<String, String>,
        passage_colors: &std::collections::HashMap<String, String>,
    ) -> GraphExport {
        let nodes: Vec<GraphNodeExport> = self
            .graph
            .node_indices()
            .filter(|idx| !self.graph[*idx].is_placeholder)
            .map(|idx| {
                let node = &self.graph[idx];
                let out_degree = self
                    .graph
                    .edges_directed(idx, petgraph::Direction::Outgoing)
                    .count() as u32;
                let in_degree = self
                    .graph
                    .edges_directed(idx, petgraph::Direction::Incoming)
                    .count() as u32;
                GraphNodeExport {
                    id: node.name.clone(),
                    label: node.name.clone(),
                    file: node.file_uri.clone(),
                    tags: passage_tags.get(&node.name).cloned().unwrap_or_default(),
                    out_degree,
                    in_degree,
                    is_special: node.is_special,
                    is_metadata: node.is_metadata,
                    is_unreachable: unreachable.contains(&node.name),
                    position: passage_positions.get(&node.name).copied(),
                    group: passage_groups.get(&node.name).cloned(),
                    color: passage_colors.get(&node.name).cloned(),
                    var_writes: var_writes.get(&node.name).cloned().unwrap_or_default(),
                    var_reads: var_reads.get(&node.name).cloned().unwrap_or_default(),
                }
            })
            .collect();

        let edges: Vec<GraphEdgeExport> = self
            .graph
            .edge_references()
            .map(|e| {
                let source = &self.graph[e.source()];
                let target = &self.graph[e.target()];
                GraphEdgeExport {
                    source: source.name.clone(),
                    target: target.name.clone(),
                    edge_type: e.weight().edge_type,
                    display_text: e.weight().display_text.clone(),
                }
            })
            .collect();

        // Detect game loops for export (SCCs with mutation)
        let game_loops = self.detect_game_loops_for_export(var_writes);

        GraphExport { nodes, edges, game_loops }
    }

    /// Detect game loops (strongly connected components) and return them
    /// as `GameLoopExport` instances for the graph export.
    ///
    /// All non-trivial SCCs (size > 1 or self-loops) are reported as game
    /// loops with `has_mutation` indicating whether the cycle contains
    /// persistent variable writes. The client can use this to distinguish
    /// game loops with mutation (normal interaction patterns) from those
    /// without (potential infinite loops) visually.
    fn detect_game_loops_for_export(
        &self,
        var_writes: &std::collections::HashMap<String, Vec<String>>,
    ) -> Vec<GameLoopExport> {
        let sccs = tarjan_scc(&self.graph);
        let mut game_loops = Vec::new();

        for scc in &sccs {
            // Only interested in non-trivial SCCs (size > 1, or self-loops)
            if scc.len() < 2 {
                // Check for self-loop
                if let Some(&idx) = scc.first()
                    && self
                        .graph
                        .edges_directed(idx, petgraph::Direction::Outgoing)
                        .any(|e| e.target() == idx)
                {
                    let node = &self.graph[idx];
                    let has_mutation = var_writes
                        .get(&node.name)
                        .map(|writes| !writes.is_empty())
                        .unwrap_or(false);

                    game_loops.push(GameLoopExport {
                        members: vec![node.name.clone()],
                        header: Some(node.name.clone()),
                        has_mutation,
                    });
                }
                continue;
            }

            // Multi-node cycle: check for persistent variable writes
            let members: Vec<String> = scc
                .iter()
                .map(|&idx| self.graph[idx].name.clone())
                .collect();
            let has_mutation = members.iter().any(|name| {
                var_writes
                    .get(name)
                    .map(|writes| !writes.is_empty())
                    .unwrap_or(false)
            });

            // Simple header detection: the node with the highest in-degree
            // within the SCC is the most likely loop header (dominance
            // approximation). A proper dominance analysis would require
            // an iterative dominator tree algorithm, but in-degree
            // within the SCC is a good heuristic.
            let header = self.identify_loop_header(scc);

            game_loops.push(GameLoopExport {
                members,
                header,
                has_mutation,
            });
        }

        game_loops
    }

    /// Count the number of game loops (non-trivial SCCs) in the graph.
    ///
    /// This is a convenience method for profiling and statistics. It runs
    /// the same SCC analysis as `detect_game_loops_for_export` but only
    /// returns the count, avoiding the overhead of building full
    /// `GameLoopExport` structs.
    pub fn game_loop_count(&self, var_writes: &std::collections::HashMap<String, Vec<String>>) -> usize {
        self.detect_game_loops_for_export(var_writes).len()
    }

    /// Identify the loop header of an SCC using in-degree heuristics.
    ///
    /// The header is the node that dominates the back-edge's source —
    /// i.e., the node that controls whether the loop repeats. As a
    /// simple heuristic, we pick the node with the highest in-degree
    /// from within the SCC (most internal predecessors), breaking ties
    /// by highest total in-degree.
    fn identify_loop_header(&self, scc: &[NodeIndex]) -> Option<String> {
        let scc_set: HashSet<NodeIndex> = scc.iter().copied().collect();

        let mut best: Option<(NodeIndex, usize, usize)> = None;
        for &idx in scc {
            let internal_in = self
                .graph
                .edges_directed(idx, petgraph::Direction::Incoming)
                .filter(|e| scc_set.contains(&e.source()))
                .count();
            let total_in = self
                .graph
                .edges_directed(idx, petgraph::Direction::Incoming)
                .count();
            match best {
                None => best = Some((idx, internal_in, total_in)),
                Some((_, bi, bt)) if internal_in > bi || (internal_in == bi && total_in > bt) => {
                    best = Some((idx, internal_in, total_in));
                }
                _ => {}
            }
        }

        best.map(|(idx, _, _)| self.graph[idx].name.clone())
    }

    /// Get the number of passages in the graph.
    pub fn passage_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Get the number of links in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Check if a passage exists in the graph.
    pub fn contains_passage(&self, name: &str) -> bool {
        self.name_to_idx.contains_key(name)
    }

    /// Get a mutable reference to an edge's weight by edge ID.
    pub fn edge_weight_mut(&mut self, edge: petgraph::graph::EdgeIndex) -> Option<&mut PassageEdge> {
        self.graph.edge_weight_mut(edge)
    }

    /// Return an iterator over all edge references in the graph.
    pub fn edge_references(&self) -> impl Iterator<Item = petgraph::graph::EdgeReference<'_, PassageEdge>> {
        self.graph.edge_references()
    }

    /// Get the name of the passage at a given node index.
    pub fn node_name(&self, idx: petgraph::graph::NodeIndex) -> Option<&str> {
        self.graph.node_weight(idx).map(|n| n.name.as_str())
    }

    /// Get the passage node data for a given name.
    pub fn get_passage(&self, name: &str) -> Option<&PassageNode> {
        self.name_to_idx
            .get(name)
            .map(|&idx| &self.graph[idx])
    }

    /// Get the names of all passages this passage links to (outgoing neighbors).
    pub fn outgoing_neighbors(&self, name: &str) -> Vec<String> {
        if let Some(&idx) = self.name_to_idx.get(name) {
            self.graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .map(|n| self.graph[n].name.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a passage has any outgoing edges. Faster than
    /// `outgoing_neighbors().is_empty()` — no allocation.
    pub fn has_outgoing(&self, name: &str) -> bool {
        self.name_to_idx.get(name).is_some_and(|&idx| {
            self.graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .next()
                .is_some()
        })
    }

    /// Count outgoing edges. Faster than `outgoing_neighbors().len()` — no allocation.
    pub fn outgoing_count(&self, name: &str) -> usize {
        self.name_to_idx.get(name).map_or(0, |&idx| {
            self.graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count()
        })
    }

    /// Get the names of all passages that link to this passage (incoming neighbors).
    pub fn incoming_neighbors(&self, name: &str) -> Vec<String> {
        if let Some(&idx) = self.name_to_idx.get(name) {
            self.graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .map(|n| self.graph[n].name.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if a passage has any incoming edges. Faster than
    /// `incoming_neighbors().is_empty()` — no allocation.
    pub fn has_incoming(&self, name: &str) -> bool {
        self.name_to_idx.get(name).is_some_and(|&idx| {
            self.graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .next()
                .is_some()
        })
    }

    /// Get all passage names in the graph.
    pub fn passage_names(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .map(|idx| self.graph[idx].name.clone())
            .collect()
    }

    /// Re-check all edges for broken-link status after structural changes.
    ///
    /// This is called after graph surgery or document removal to ensure that
    /// edges that previously pointed to non-existent passages now correctly
    /// reflect whether their targets exist. When an edge's target becomes
    /// real, the original edge type (Jump, Call, Include, Navigation) is
    /// restored from `pre_broken_type`; if none was saved, Navigation is
    /// used as the default.
    pub fn recheck_broken_links(&mut self) {
        // Collect edge updates: (edge_id, target_is_missing)
        let updates: Vec<(petgraph::graph::EdgeIndex, bool)> = self
            .graph
            .edge_references()
            .map(|edge_ref| {
                let target_node = &self.graph[edge_ref.target()];
                // A passage is "real" (not a placeholder) if it has a file_uri
                let target_is_real = !target_node.file_uri.is_empty();
                (edge_ref.id(), !target_is_real)
            })
            .collect();

        // Apply updates
        for (edge_id, new_broken) in updates {
            if let Some(edge) = self.graph.edge_weight_mut(edge_id) {
                // Include and Navigation edges are the only user-facing edge
                // types. Both can be marked Broken when their target doesn't
                // exist. Upstream edges are internal and always have valid
                // targets (added only after verification).
                if new_broken {
                    // Target doesn't exist — mark as Broken, but save the
                    // original type so we can restore it later if the target
                    // is created. Only save if we haven't already (avoid
                    // overwriting a previously saved type on repeated checks).
                    if edge.edge_type != EdgeType::Broken {
                        edge.pre_broken_type = Some(edge.edge_type);
                        edge.edge_type = EdgeType::Broken;
                    }
                } else if edge.edge_type == EdgeType::Broken {
                    // Target now exists — restore the original type if we
                    // saved one. Otherwise fall back to Navigation.
                    edge.edge_type = edge.pre_broken_type
                        .unwrap_or(EdgeType::Navigation);
                    edge.pre_broken_type = None;
                }
                // All other edge types (Upstream, Call, Include, Jump) are
                // preserved when their target exists.
            }
        }
    }
}

impl Default for PassageGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable graph export for the Story Map webview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphExport {
    pub nodes: Vec<GraphNodeExport>,
    pub edges: Vec<GraphEdgeExport>,
    /// Detected game loops (SCCs). The client uses this for loop
    /// visualization (cycle highlighting, loop header indicators).
    /// Each game loop includes `has_mutation` so the client can
    /// visually distinguish game loops with mutation from those
    /// without (potential infinite loops).
    #[serde(default)]
    pub game_loops: Vec<GameLoopExport>,
}

/// A detected game loop, exported for client visualization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameLoopExport {
    /// The passages that participate in this cycle.
    pub members: Vec<String>,
    /// The identified loop header passage (dominates the back-edge source),
    /// or `None` if no single header could be identified.
    pub header: Option<String>,
    /// Whether the cycle contains persistent variable writes.
    pub has_mutation: bool,
}

/// A single node in the exported graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNodeExport {
    pub id: String,
    pub label: String,
    pub file: String,
    pub tags: Vec<String>,
    pub out_degree: u32,
    pub in_degree: u32,
    pub is_special: bool,
    pub is_metadata: bool,
    pub is_unreachable: bool,
    /// The (x, y) position of this passage on the Twine editor canvas.
    /// When present, the graph view should place the node at these
    /// coordinates instead of using an automatic layout.
    #[serde(default)]
    pub position: Option<(f64, f64)>,
    /// Persistent variable names written in this passage.
    /// Data already available from format plugin variable extraction.
    #[serde(default)]
    pub var_writes: Vec<String>,
    /// Persistent variable names read in this passage.
    /// Data already available from format plugin variable extraction.
    #[serde(default)]
    pub var_reads: Vec<String>,
    /// Manual group assignment from passage header metadata `{"group":"..."}`.
    /// Groups are rendered as bounding-box containers in the graph view.
    #[serde(default)]
    pub group: Option<String>,
    /// Node color from passage header metadata `{"color":"..."}`.
    /// Overrides the default category-based color.
    #[serde(default)]
    pub color: Option<String>,
}

/// A single edge in the exported graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdgeExport {
    pub source: String,
    pub target: String,
    /// The semantic type of this edge (navigation, call, include, jump,
    /// upstream lifecycle, or broken).
    #[serde(default)]
    pub edge_type: EdgeType,
    pub display_text: Option<String>,
}
