//! Graph Mathematics Engine
//!
//! The workspace is represented as a directed graph where nodes are passages
//! and edges are links between passages. This module provides:
//!
//! - Broken link detection
//! - Infinite traversal loop detection (Tarjan's SCC)
//! - Unreachable passage detection (BFS from entry point)
//! - Variable flow analysis support

use crate::passage::{VarKind, VarOp};
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
}

/// Edge data representing a link between passages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageEdge {
    /// The display text of the link (if any).
    pub display_text: Option<String>,
    /// Whether this link is a broken link (target doesn't exist).
    pub is_broken: bool,
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

/// Kinds of diagnostics the graph engine can produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// A link points to a passage that doesn't exist.
    BrokenLink,
    /// A passage cannot be reached from the entry point.
    UnreachablePassage,
    /// An infinite traversal loop was detected.
    InfiniteLoop,
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
    /// A passage has no incoming links from any other passage (orphaned).
    OrphanedPassage,
    /// A passage has a high cyclomatic complexity (too many links or
    /// conditionals, making it hard to follow or test).
    ComplexPassage,
    /// A passage body exceeds a recommended size threshold.
    LargePassage,
    /// The start passage has no outgoing links (the story immediately
    /// ends with no choices).
    MissingStartLink,
}

/// The passage graph — the core data structure for narrative analysis.
#[derive(Debug, Clone)]
pub struct PassageGraph {
    /// The underlying directed graph.
    graph: DiGraph<PassageNode, PassageEdge>,
    /// Mapping from passage name to node index.
    name_to_idx: HashMap<String, NodeIndex>,
}

impl PassageGraph {
    /// Create an empty passage graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_idx: HashMap::new(),
        }
    }

    /// Add a passage as a node to the graph.
    /// Returns the node index. If a passage with the same name already exists,
    /// it is replaced.
    pub fn add_passage(&mut self, node: PassageNode) -> NodeIndex {
        if let Some(&idx) = self.name_to_idx.get(&node.name) {
            self.graph[idx] = node;
            idx
        } else {
            let idx = self.graph.add_node(node.clone());
            self.name_to_idx.insert(node.name, idx);
            idx
        }
    }

    /// Remove a passage node from the graph by name.
    /// Also removes all edges connected to this node.
    pub fn remove_passage(&mut self, name: &str) -> Option<PassageNode> {
        let idx = self.name_to_idx.remove(name)?;
        // Remove edges first (petgraph handles this when removing the node)
        let node = self.graph.remove_node(idx);
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
            };
            let idx = self.graph.add_node(node.clone());
            self.name_to_idx.insert(node.name, idx);
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

    /// Detect broken links — edges whose target passage doesn't exist in the graph
    /// as a real node (i.e., no document URI).
    pub fn detect_broken_links(&self) -> Vec<GraphDiagnostic> {
        let mut diagnostics = Vec::new();
        for edge_ref in self.graph.edge_references() {
            if edge_ref.weight().is_broken {
                let source_idx = edge_ref.source();
                let source = &self.graph[source_idx];
                diagnostics.push(GraphDiagnostic {
                    passage_name: source.name.clone(),
                    file_uri: source.file_uri.clone(),
                    kind: DiagnosticKind::BrokenLink,
                    message: format!(
                        "Link target '{}' not found in workspace",
                        self.graph[edge_ref.target()].name
                    ),
                });
            }
        }
        diagnostics
    }

    /// Detect unreachable passages using BFS from the start passage.
    /// Returns diagnostics for passages that cannot be reached.
    pub fn detect_unreachable(&self, start_passage: &str) -> Vec<GraphDiagnostic> {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(&start_idx) = self.name_to_idx.get(start_passage) {
            reachable.insert(start_idx);
            queue.push_back(start_idx);
        }

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

        // Also mark passages reached through special passages that contribute_variables
        // (e.g., StoryInit may set variables but isn't linked to directly)
        // For now, metadata passages are excluded from reachability analysis.

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

    /// Detect potential infinite loops using Tarjan's Strongly Connected Components.
    /// A cycle is flagged as an infinite loop if no state mutation (variable write)
    /// occurs within the cycle.
    pub fn detect_infinite_loops(
        &self,
        passage_vars: &HashMap<String, Vec<&VarOp>>,
    ) -> Vec<GraphDiagnostic> {
        let sccs = tarjan_scc(&self.graph);
        let mut diagnostics = Vec::new();

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
                        // Check if there are persistent variable writes in this passage
                        // (temporary variable writes don't persist across passages)
                        let has_mutation = passage_vars
                            .get(&node.name)
                            .map(|ops| ops.iter().any(|v| v.kind == VarKind::Init && !v.is_temporary))
                            .unwrap_or(false);

                        if !has_mutation {
                            diagnostics.push(GraphDiagnostic {
                                passage_name: node.name.clone(),
                                file_uri: node.file_uri.clone(),
                                kind: DiagnosticKind::InfiniteLoop,
                                message: format!(
                                    "Potential infinite loop: passage '{}' links to itself without state mutation",
                                    node.name
                                ),
                            });
                        }
                    }
                continue;
            }

            // Multi-node cycle: check if any passage in the cycle has persistent variable writes
            // (temporary variable writes don't count — they don't persist across passages)
            let has_mutation = scc.iter().any(|&idx| {
                let name = &self.graph[idx].name;
                passage_vars
                    .get(name)
                    .map(|ops| ops.iter().any(|v| v.kind == VarKind::Init && !v.is_temporary))
                    .unwrap_or(false)
            });

            if !has_mutation {
                // Report the first passage in the cycle as the diagnostic location
                let node = &self.graph[scc[0]];
                let cycle_names: Vec<&str> = scc
                    .iter()
                    .map(|&idx| self.graph[idx].name.as_str())
                    .collect();
                diagnostics.push(GraphDiagnostic {
                    passage_name: node.name.clone(),
                    file_uri: node.file_uri.clone(),
                    kind: DiagnosticKind::InfiniteLoop,
                    message: format!(
                        "Potential infinite loop: cycle [{}] has no state mutation",
                        cycle_names.join(" → ")
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
    /// the start passage.
    pub fn export_graph_with_metadata(
        &self,
        passage_tags: &std::collections::HashMap<String, Vec<String>>,
        unreachable: &std::collections::HashSet<String>,
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
                    is_broken: e.weight().is_broken,
                    display_text: e.weight().display_text.clone(),
                }
            })
            .collect();

        GraphExport { nodes, edges }
    }

    /// Export the graph as a serializable structure for the Story Map webview.
    ///
    /// This is a convenience wrapper that calls `export_graph_with_metadata`
    /// with empty tag and unreachable data.
    pub fn export_graph(&self) -> GraphExport {
        self.export_graph_with_metadata(
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        )
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
    /// reflect whether their targets exist.
    pub fn recheck_broken_links(&mut self) {
        // Collect edge updates: (edge_id, new_is_broken)
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
                edge.is_broken = new_broken;
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
}

/// A single edge in the exported graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdgeExport {
    pub source: String,
    pub target: String,
    pub is_broken: bool,
    pub display_text: Option<String>,
}
