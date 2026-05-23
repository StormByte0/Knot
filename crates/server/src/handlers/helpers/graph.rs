//! Passage graph rebuild and implicit special edge construction.

use knot_core::graph::{PassageEdge, PassageNode};
use knot_core::passage::{PassageCategory, SpecialPassageBehavior, SpecialPassageLayer, StoryFormat};
use knot_core::Workspace;
use knot_formats::plugin as fmt_plugin;

/// Rebuild the passage graph from all workspace documents.
///
/// Delegates format-specific logic (variable string map building and
/// dynamic navigation link resolution) to the active format plugin
/// when available, falling back to no-op defaults otherwise.
///
/// Returns the newly constructed `PassageGraph`. The caller is responsible
/// for assigning it to `workspace.graph`.
#[allow(clippy::type_complexity)]
pub(crate) fn rebuild_graph(
    workspace: &Workspace,
    registry: &fmt_plugin::FormatRegistry,
    format: StoryFormat,
) -> knot_core::PassageGraph {
    let plugin = registry.get(&format);

    // ── Step 1: Build dynamic variable resolution map ───────────────────
    // Delegate to the format plugin so that format-specific assignment
    // syntax (e.g., SugarCube <<set $var to "literal">>) is handled
    // by the appropriate plugin rather than hardcoded regexes.
    let var_string_map = plugin
        .map(|p| p.build_var_string_map(workspace))
        .unwrap_or_default();

    // ── Step 2: Collect passage info ────────────────────────────────────
    let info: Vec<(String, String, bool, bool, Option<SpecialPassageLayer>, PassageCategory, Option<SpecialPassageBehavior>, Vec<(Option<String>, String)>)> = workspace
        .documents()
        .flat_map(|doc| {
            doc.passages.iter().map(|p| {
                let mut edges: Vec<(Option<String>, String)> = p
                    .links
                    .iter()
                    .map(|l| (l.display_text.clone(), l.target.clone()))
                    .collect();

                // ── Dynamic variable resolution for navigation macros ────
                // Delegate to the format plugin so that format-specific
                // navigation patterns (e.g., SugarCube <<goto $var>>) are
                // resolved by the appropriate plugin.
                edges.extend(
                    plugin
                        .map(|plug| plug.resolve_dynamic_navigation_links(p, &var_string_map))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|link| (link.display_text, link.target)),
                );

                (
                    p.name.clone(),
                    doc.uri.to_string(),
                    p.is_special,
                    p.is_metadata(),
                    p.special_def.as_ref().map(|d| d.layer.clone()),
                    p.category(),
                    p.special_def.as_ref().map(|d| d.behavior.clone()),
                    edges,
                )
            })
        })
        .collect();

    let mut graph = knot_core::PassageGraph::new();

    for (name, file_uri, is_special, is_metadata, layer, category, behavior, _edges) in &info {
        let node = PassageNode {
            name: name.clone(),
            file_uri: file_uri.clone(),
            is_special: *is_special,
            is_metadata: *is_metadata,
            is_placeholder: false,
            layer: layer.clone(),
            category: *category,
            behavior: behavior.clone(),
        };
        graph.add_passage(node);
    }

    // Add edges after all nodes exist so broken-link detection works.
    // Use the format plugin's classify_edge() for format-aware edge typing.
    for (source, _, _, _, _, _, _, edges) in &info {
        for (display_text, target) in edges {
            let target_exists = graph.contains_passage(target);
            // Ask the format plugin to classify this edge. If it returns
            // None, fall back to Navigation (or Broken if target missing).
            let edge_type = if !target_exists {
                knot_core::graph::EdgeType::Broken
            } else if let Some(plug) = plugin.as_ref() {
                // Find the source passage to get full context for classification
                let source_passage = workspace.find_passage(source)
                    .map(|(_, p)| p.clone());
                if let Some(sp) = source_passage {
                    plug.classify_edge(&sp, display_text.as_deref(), target)
                        .unwrap_or(knot_core::graph::EdgeType::Navigation)
                } else {
                    knot_core::graph::EdgeType::Navigation
                }
            } else {
                knot_core::graph::EdgeType::Navigation
            };
            let edge = PassageEdge {
                display_text: display_text.clone(),
                edge_type,
            };
            graph.add_edge(source, target, edge);
        }
    }

    // ── Step 4: Add implicit edges for special passages ──────────────────
    // Uses the graph's special_bundle instead of re-scanning workspace
    // passages. The bundle was populated incrementally by add_passage().
    add_implicit_special_edges(&mut graph, workspace);

    graph
}

/// Add upstream lifecycle edges among special passages.
///
/// ## Graph Isolation Architecture
///
/// Special passages and user-defined passages form two conceptual zones with
/// a single structural bridge point, but explicit references from the format
/// plugin's parser CAN cross the boundary:
///
/// 1. **Special passage chain** (upstream): TwineCore passages flow into
///    StoryFormat passages, representing engine execution order.
///    `Story JavaScript → StoryInit` means "Story JavaScript runs before
///    StoryInit, so its variables are available when StoryInit executes."
///
/// 2. **Startup → Start bridge**: The edge from the last Startup passage
///    (e.g., StoryInit) to the start passage (from StoryData's `start`
///    attribute). This is the structural bridge from the special chain into
///    the user-defined graph, representing the moment the engine begins
///    normal navigation.
///
/// 3. **User-defined passage graph** (downstream): The start passage
///    connects to all user-defined passages via explicit `[[links]]`.
///    Only user-defined passages need explicit links for context and
///    reachability analysis.
///
/// ## Bundle-Driven Construction
///
/// This function queries the graph's `special_bundle` instead of iterating
/// workspace documents. The bundle was populated incrementally by
/// `add_passage()`, so this function is self-sufficient — it needs only
/// the graph and the workspace metadata (for the start passage name).
///
/// ## Upstream Chain
///
/// ```text
/// Story JavaScript (TwineCore, ScriptInjection, priority -1)
///     ↓ (upstream)
/// StoryInit (StoryFormat, Startup, priority 0)
///     ↓ (upstream bridge)
/// Start passage (user-defined, from StoryData's start attribute)
/// ```
///
/// Upstream edges use `EdgeType::Upstream` so the story map
/// can render them differently (e.g., dashed lines) from normal
/// navigation edges.
pub(crate) fn add_implicit_special_edges(
    graph: &mut knot_core::PassageGraph,
    workspace: &Workspace,
) {
    // Clone bundle lists so the immutable borrow on graph.special_bundle
    // ends before we call graph.add_edge() (which takes &mut self).
    let script_injection = graph.special_bundle.script_injection.clone();
    let startup = graph.special_bundle.startup.clone();

    // ── Edge: ScriptInjection → Startup ──────────────────────────────
    // Script injection passages run before startup passages. For example,
    // "Story JavaScript" (TwineCore) runs before "StoryInit" (StoryFormat),
    // so its variables and side effects are available when StoryInit executes.
    for script_name in &script_injection {
        for startup_name in &startup {
            let already_exists = graph.outgoing_neighbors(script_name)
                .iter()
                .any(|n| n == startup_name);
            if !already_exists {
                graph.add_edge(script_name, startup_name, PassageEdge {
                    display_text: Some(format!("(upstream: {} → {})", script_name, startup_name)),
                    edge_type: knot_core::graph::EdgeType::Upstream,
                });
            }
        }
    }

    // ── Bridge Edge: Startup → Start passage ─────────────────────────
    // This is the single edge from the special chain into the user-defined
    // graph. It represents the moment the engine begins normal navigation.
    if !startup.is_empty() {
        let start_passage_name = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        if graph.contains_passage(start_passage_name) {
            let bridge_source = &startup[0];

            let already_exists = graph.outgoing_neighbors(bridge_source)
                .iter()
                .any(|n| n == start_passage_name);
            if !already_exists {
                graph.add_edge(bridge_source, start_passage_name, PassageEdge {
                    display_text: Some(format!("(upstream: {} → {})", bridge_source, start_passage_name)),
                    edge_type: knot_core::graph::EdgeType::Upstream,
                });
            }
        }
    }
}
