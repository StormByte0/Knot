//! Passage graph rebuild and implicit special edge construction.

use knot_core::graph::{PassageEdge, PassageNode};
use knot_core::passage::{SpecialPassageBehavior, SpecialPassageLayer, StoryFormat};
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
    let info: Vec<(String, String, bool, bool, Option<SpecialPassageLayer>, Vec<(Option<String>, String)>)> = workspace
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
                    edges,
                )
            })
        })
        .collect();

    let mut graph = knot_core::PassageGraph::new();

    for (name, file_uri, is_special, is_metadata, layer, _edges) in &info {
        let node = PassageNode {
            name: name.clone(),
            file_uri: file_uri.clone(),
            is_special: *is_special,
            is_metadata: *is_metadata,
            is_placeholder: false,
            layer: layer.clone(),
        };
        graph.add_passage(node);
    }

    // Add edges after all nodes exist so broken-link detection works.
    for (source, _, _, _, _, edges) in &info {
        for (display_text, target) in edges {
            let target_exists = graph.contains_passage(target);
            let edge = PassageEdge {
                display_text: display_text.clone(),
                is_broken: !target_exists,
                is_upstream: false,
            };
            graph.add_edge(source, target, edge);
        }
    }

    // ── Step 4: Add implicit edges for special passages ──────────────────
    add_implicit_special_edges(&mut graph, workspace, plugin);

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
/// ## Structural vs Explicit Edges
///
/// This function only adds **structural/lifecycle edges** — edges that
/// represent the engine's implicit execution ordering. It does NOT add
/// edges for **explicit references** detected by the format plugin's parser.
/// Those are handled separately:
///
/// - **`data-passage` attributes** in StoryInterface: The SugarCube parser
///   extracts `data-passage="SidebarStats"` as a link, which becomes a
///   graph edge via `rebuild_graph()`. StoryInterface is classified as
///   `StructureTemplate` because it can contain these references.
///
/// - **`Engine.play()`/`Engine.goTo()`** in Story JavaScript: The parser
///   extracts these as links via `extract_implicit_passage_refs()`. These
///   create graph edges from the script passage to the referenced user
///   passages. ScriptInjection passages have `contributes_variables: true`
///   for this reason.
///
/// - **Widget declarations** in Story JavaScript: Widgets that reference
///   passages via `<<include>>`, `<<goto>>`, etc. are detected by the
///   format plugin's `resolve_dynamic_navigation_links()` and create
///   graph edges.
///
/// - **`<<goto>>`/`<<include>>`** in StoryInit or other special passages:
///   Detected by the parser as macro passage refs and create graph edges.
///
/// The key principle: **isolation applies to implicit/structural edges,
/// not to explicit references detected by the format plugin**. If a special
/// passage contains an explicit reference to a user-defined passage, that
/// reference SHOULD create a graph edge to ensure correct reachability
/// analysis and variable flow.
///
/// ## Why No Other Structural Cross-Zone Edges
///
/// - **Chrome passages** (StoryCaption, StoryBanner, etc.) render in the
///   UI chrome area on every navigation. They don't need structural edges
///   because they're always invoked by the engine.
///
/// - **ChromeInterceptor passages** (PassageHeader, PassageFooter) wrap
///   every rendered passage body. They're conceptually connected to ALL
///   user-defined passages but we don't create O(N) edges. Instead, the
///   analysis engine merges their variable context into every passage's
///   entry state during dataflow analysis.
///
/// - **User-defined → Special**: User passages rarely link to special
///   passages directly. If they do (e.g., `<<include "StoryInit">>`),
///   those are detected as passage refs by the format plugin, not as
///   structural graph edges.
///
/// ## Upstream Chain
///
/// The upstream chain reflects execution order:
///
/// ```text
/// Story JavaScript (TwineCore, ScriptInjection, priority -1)
///     ↓ (upstream)
/// StoryInit (StoryFormat, Startup, priority 0)
///     ↓ (upstream bridge)
/// Start passage (user-defined, from StoryData's start attribute)
/// ```
///
/// Edge Types
///
/// Upstream edges are marked with `is_upstream: true` so the story map
/// can render them differently (e.g., dashed lines) from normal
/// navigation edges.
///
/// **Format isolation**: The server never hardcodes format-specific passage
/// names. All special passage definitions come from the merged registry
/// (TwineCore + LegacyCore + StoryFormat via `all_special_passages()`).
pub(crate) fn add_implicit_special_edges(
    graph: &mut knot_core::PassageGraph,
    workspace: &Workspace,
    plugin: Option<&dyn fmt_plugin::FormatPlugin>,
) {
    let Some(plugin) = plugin else {
        return;
    };

    let special_defs = plugin.all_special_passages();

    // ── Build the upstream chain among special passages only ──────────
    //
    // Collect special passages that actually exist in the graph, grouped
    // by their layer and behavior. We only add edges among special
    // passages and the single Startup → Start bridge.

    // Collect TwineCore ScriptInjection passages (e.g., "Story JavaScript")
    let mut twine_core_script: Vec<String> = Vec::new();
    // Collect StoryFormat Startup passages (e.g., "StoryInit")
    let mut story_format_startup: Vec<String> = Vec::new();

    for def in &special_defs {
        if !graph.contains_passage(&def.name) {
            continue;
        }
        match (&def.layer, &def.behavior) {
            (SpecialPassageLayer::TwineCore, SpecialPassageBehavior::ScriptInjection) => {
                twine_core_script.push(def.name.clone());
            }
            (SpecialPassageLayer::LegacyCore, SpecialPassageBehavior::ScriptInjection) => {
                // Legacy "script" passages are also upstream from Startup
                twine_core_script.push(def.name.clone());
            }
            (SpecialPassageLayer::StoryFormat, SpecialPassageBehavior::Startup) => {
                story_format_startup.push(def.name.clone());
            }
            _ => {
                // Chrome, ChromeInterceptor, PassageReady, Metadata,
                // StyleInjection, Custom: no implicit edges.
                //
                // ChromeInterceptor passages (PassageHeader, PassageFooter)
                // wrap every rendered passage but don't get graph edges.
                // The analysis engine merges their variable context into
                // every passage's entry state during dataflow analysis,
                // making O(N) edges unnecessary.
                //
                // Chrome passages render in the UI chrome area on every
                // navigation and are always reachable by engine definition.
                //
                // PassageReady/PassageDone run on every navigation but
                // their variable flow is handled by the format plugin's
                // seed variable system, not graph edges.
            }
        }
    }

    // ── Edge: TwineCore/LegacyCore ScriptInjection → StoryFormat Startup ──
    //
    // This represents the execution order: script injection passages run
    // before startup passages. For example, "Story JavaScript" (TwineCore)
    // runs before "StoryInit" (StoryFormat), so its variables and side
    // effects are available when StoryInit executes.
    for script_name in &twine_core_script {
        for startup_name in &story_format_startup {
            let already_exists = graph.outgoing_neighbors(script_name)
                .iter()
                .any(|n| n == startup_name);
            if !already_exists {
                graph.add_edge(script_name, startup_name, PassageEdge {
                    display_text: Some(format!("(upstream: {} → {})", script_name, startup_name)),
                    is_broken: false,
                    is_upstream: true,
                });
            }
        }
    }

    // ── Bridge Edge: StoryFormat Startup → Start passage ──────────────
    //
    // This is the single edge that crosses from the special passage chain
    // into the user-defined passage graph. It represents the moment the
    // engine finishes running startup passages and begins normal navigation
    // at the start passage. This edge is essential for:
    //
    // 1. Variable flow: Startup passages seed variables that are available
    //    in the start passage's entry state.
    // 2. Reachability: The start passage is reachable from the startup
    //    chain (though it's always reachable by definition since it's the
    //    BFS entry point).
    // 3. Story map visualization: Shows the transition from engine setup
    //    to user content.
    //
    // The start passage name comes from StoryData's `start` attribute,
    // falling back to "Start" if not specified.
    if !story_format_startup.is_empty() {
        let start_passage_name = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        if graph.contains_passage(start_passage_name) {
            // Find the highest-priority Startup passage (lowest priority number)
            // as the source of the bridge edge. Typically this is StoryInit
            // (priority 0), which is the last startup passage to run.
            let bridge_source = story_format_startup.first().unwrap();

            let already_exists = graph.outgoing_neighbors(bridge_source)
                .iter()
                .any(|n| n == start_passage_name);
            if !already_exists {
                graph.add_edge(bridge_source, start_passage_name, PassageEdge {
                    display_text: Some(format!("(upstream: {} → {})", bridge_source, start_passage_name)),
                    is_broken: false,
                    is_upstream: true,
                });
            }
        }
    }

    // NOTE: No other edges are added from special passages to user-defined
    // passages. The isolation principle holds:
    //
    // - Variable flow analysis uses explicit seeding via
    //   `collect_special_passage_initializers()` and
    //   `supplement_seed_with_format_specials()` to propagate
    //   variable state from special passages.
    //
    // - ChromeInterceptor variable contexts is merged into every passage's
    //   entry state during dataflow analysis (not via graph edges).
    //
    // - Reachability analysis skips special passages entirely
    //   (they're always reachable by engine definition).
    //
    // - Orphan detection skips special passages entirely
    //   (they're always referenced by the engine).
    //
    // - The story map can show the upstream chain as a separate
    //   visual cluster, distinct from the user navigation graph.
}
