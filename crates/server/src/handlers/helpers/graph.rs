//! Passage graph rebuild, implicit special edge construction, and graph metrics.

use knot_core::Workspace;
use knot_core::graph::{PassageEdge, PassageNode};
use knot_core::passage::{
    PassageCategory, SpecialPassageBehavior, SpecialPassageLayer, StoryFormat,
};
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
    let info: Vec<(
        String,
        String,
        bool,
        bool,
        Option<SpecialPassageLayer>,
        PassageCategory,
        Option<SpecialPassageBehavior>,
        Vec<(Option<String>, String, Option<knot_core::graph::EdgeType>)>,
    )> = workspace
        .documents()
        .flat_map(|doc| {
            doc.passages.iter().map(|p| {
                let mut edges: Vec<(Option<String>, String, Option<knot_core::graph::EdgeType>)> =
                    p.links
                        .iter()
                        .map(|l| (l.display_text.clone(), l.target.clone(), l.edge_type_hint))
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
                        .map(|link| (link.display_text, link.target, link.edge_type_hint)),
                );

                (
                    p.name.clone(),
                    doc.uri.to_string(),
                    p.is_special,
                    p.is_metadata(),
                    p.special_def.as_ref().map(|d| d.layer),
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
            layer: *layer,
            category: *category,
            behavior: behavior.clone(),
        };
        graph.add_passage(node);
    }

    // Add edges after all nodes exist so broken-link detection works.
    // Use the format plugin's classify_edge() for format-aware edge typing.
    // Priority: edge_type_hint from link extraction > classify_edge() > Navigation.
    for (source, _, _, _, _, _, _, edges) in &info {
        for (display_text, target, hint) in edges {
            // Skip links with empty targets — these are dynamic navigation
            // macros (<<return "Display">>, <<back "Display">>) where the
            // target is determined at runtime via browser history. Creating
            // a graph edge with an empty target would produce a false
            // "BrokenLink" diagnostic. The link IS kept in `passage.links`
            // (for dead-end detection), but we don't add a graph edge for it.
            if target.is_empty() {
                continue;
            }
            let target_exists = graph.contains_passage(target);
            // Determine the edge type using a priority chain:
            // 1. Broken links always win (target doesn't exist)
            // 2. The format plugin's extraction hint (set during link
            //    extraction, e.g., <<goto>> → Jump, <<include>> → Include)
            // 3. The format plugin's classify_edge() method (for cases
            //    that need full passage context, like widget invocations)
            // 4. Default Navigation
            //
            // When the target doesn't exist, we save the "would-be" type
            // in pre_broken_type so that recheck_broken_links() can
            // restore the correct type (e.g., Jump) when the target is
            // later created, instead of defaulting to Navigation.
            let (edge_type, pre_broken_type) = if !target_exists {
                // Compute what the type WOULD be if the target existed
                let would_be_type = if let Some(hint_type) = hint {
                    *hint_type
                } else if let Some(plug) = plugin.as_ref() {
                    let source_passage = workspace.find_passage(source).map(|(_, p)| p.clone());
                    if let Some(sp) = source_passage {
                        plug.classify_edge(&sp, display_text.as_deref(), target)
                            .unwrap_or(knot_core::graph::EdgeType::Navigation)
                    } else {
                        knot_core::graph::EdgeType::Navigation
                    }
                } else {
                    knot_core::graph::EdgeType::Navigation
                };
                (knot_core::graph::EdgeType::Broken, Some(would_be_type))
            } else if let Some(hint_type) = hint {
                // Use the extraction-time hint directly — it's more reliable
                // than re-scanning the passage body in classify_edge().
                (*hint_type, None)
            } else if let Some(plug) = plugin.as_ref() {
                // Fall back to classify_edge() for cases where the hint
                // wasn't set during extraction (e.g., [[links]] that might
                // be widget invocations, or dynamic variable links).
                let source_passage = workspace.find_passage(source).map(|(_, p)| p.clone());
                if let Some(sp) = source_passage {
                    let classified = plug
                        .classify_edge(&sp, display_text.as_deref(), target)
                        .unwrap_or(knot_core::graph::EdgeType::Navigation);
                    (classified, None)
                } else {
                    (knot_core::graph::EdgeType::Navigation, None)
                }
            } else {
                (knot_core::graph::EdgeType::Navigation, None)
            };
            let edge = PassageEdge {
                display_text: display_text.clone(),
                edge_type,
                pre_broken_type,
            };

            // All edges go source → target (the passage containing the reference
            // → the referenced passage). Both Navigation and Include edges follow
            // this direction. The edge TYPE distinguishes them, not the direction.
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
            let already_exists = graph
                .outgoing_neighbors(script_name)
                .iter()
                .any(|n| n == startup_name);
            if !already_exists {
                graph.add_edge(
                    script_name,
                    startup_name,
                    PassageEdge {
                        display_text: Some(format!(
                            "(upstream: {} → {})",
                            script_name, startup_name
                        )),
                        edge_type: knot_core::graph::EdgeType::Upstream,
                        pre_broken_type: None,
                    },
                );
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

            let already_exists = graph
                .outgoing_neighbors(bridge_source)
                .iter()
                .any(|n| n == start_passage_name);
            if !already_exists {
                graph.add_edge(
                    bridge_source,
                    start_passage_name,
                    PassageEdge {
                        display_text: Some(format!(
                            "(upstream: {} → {})",
                            bridge_source, start_passage_name
                        )),
                        edge_type: knot_core::graph::EdgeType::Upstream,
                        pre_broken_type: None,
                    },
                );
            }
        }
    }
}

// ===========================================================================
// Graph metric computation
// ===========================================================================

/// Compute the maximum depth from the start passage using BFS.
pub(crate) fn compute_max_depth(workspace: &Workspace, start_passage: &str) -> u32 {
    let mut depths: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    if workspace.graph.contains_passage(start_passage) {
        depths.insert(start_passage.to_string(), 0);
        queue.push_back(start_passage.to_string());
    }

    while let Some(name) = queue.pop_front() {
        let current_depth = *depths.get(&name).unwrap_or(&0);
        for neighbor in workspace.graph.outgoing_neighbors(&name) {
            if !depths.contains_key(&neighbor) {
                let new_depth = current_depth + 1;
                depths.insert(neighbor.clone(), new_depth);
                queue.push_back(neighbor);
            }
        }
    }

    depths.values().copied().max().unwrap_or(0)
}

/// Compute the number of weakly connected components in the passage graph.
pub(crate) fn compute_connected_components(workspace: &Workspace) -> u32 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    if passage_names.is_empty() {
        return 0;
    }

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut component_count: u32 = 0;

    for name in &passage_names {
        if visited.contains(name) {
            continue;
        }

        // BFS considering both directions (weakly connected)
        component_count += 1;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(name.clone());
        visited.insert(name.clone());

        while let Some(current) = queue.pop_front() {
            for neighbor in workspace.graph.outgoing_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
            for neighbor in workspace.graph.incoming_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    component_count
}

/// Compute a simplified average clustering coefficient.
///
/// For each passage, count how many of its outgoing neighbors also link
/// to each other (forming triangles), divided by the maximum possible
/// number of such connections. Returns the average across all passages
/// with at least 2 outgoing links.
pub(crate) fn compute_avg_clustering(workspace: &Workspace) -> f64 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    let mut coefficients: Vec<f64> = Vec::new();

    for name in &passage_names {
        let out_neighbors: Vec<String> = workspace.graph.outgoing_neighbors(name);
        let k = out_neighbors.len();

        if k < 2 {
            continue;
        }

        let neighbor_set: std::collections::HashSet<String> =
            out_neighbors.iter().cloned().collect();

        let mut triangle_count: u32 = 0;
        for neighbor in &out_neighbors {
            let their_neighbors = workspace.graph.outgoing_neighbors(neighbor);
            for their_target in &their_neighbors {
                if neighbor_set.contains(their_target) {
                    triangle_count += 1;
                }
            }
        }

        let max_possible = (k * (k - 1)) as f64;
        let local_coeff = triangle_count as f64 / max_possible;
        coefficients.push(local_coeff);
    }

    if coefficients.is_empty() {
        0.0
    } else {
        coefficients.iter().sum::<f64>() / coefficients.len() as f64
    }
}
