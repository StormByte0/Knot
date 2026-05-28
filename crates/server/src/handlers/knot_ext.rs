//! Custom LSP request handlers (knot/graph, knot/build, knot/play, etc.)
//!
//! These are `impl ServerState` methods (not part of the `LanguageServer`
//! trait) registered as custom methods via `LspService::build`.

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_core::{AnalysisEngine, Block};
use lsp_types::{TextEdit as LspTextEdit, WorkspaceEdit};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Variable flow conversion helpers
// ---------------------------------------------------------------------------

/// Strip the `$` or `_` sigil prefix from a variable name.
fn strip_dollar_prefix(name: &str) -> String {
    if name.starts_with('$') || name.starts_with('_') {
        name[1..].to_string()
    } else {
        name.to_string()
    }
}

/// Compute BFS reachability depths for all passages reachable from the
/// start passage and special passages with outgoing edges (same seed logic
/// as `detect_unreachable`). Returns a map of passage name → BFS depth.
fn compute_bfs_depths(workspace: &knot_core::Workspace, start_passage: &str) -> std::collections::HashMap<String, u32> {
    let mut depths: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    // Seed BFS from the start passage
    if workspace.graph.contains_passage(start_passage) {
        depths.insert(start_passage.to_string(), 0);
        queue.push_back(start_passage.to_string());
    }

    // Seed from special passages with outgoing edges to user passages
    // (same logic as detect_unreachable — handles StoryInterface,
    // StoryInit with <<goto>>, PassageHeader/Footer, etc.)
    for name in workspace.graph.passage_names() {
        let is_special_non_meta = workspace
            .graph
            .get_passage(&name)
            .map_or(false, |n| n.is_special && !n.is_metadata);
        if !is_special_non_meta {
            continue;
        }
        // Check if this special passage has outgoing edges to non-special passages
        let has_user_refs = workspace
            .graph
            .outgoing_neighbors(&name)
            .iter()
            .any(|neighbor| {
                workspace
                    .graph
                    .get_passage(neighbor)
                    .map_or(false, |n| !n.is_special)
            });
        if has_user_refs && !depths.contains_key(&name) {
            depths.insert(name.clone(), 0);
            queue.push_back(name);
        }
    }

    // BFS
    while let Some(name) = queue.pop_front() {
        let current_depth = *depths.get(&name).unwrap_or(&0);
        for neighbor in workspace.graph.outgoing_neighbors(&name) {
            if !depths.contains_key(&neighbor) {
                depths.insert(neighbor.clone(), current_depth + 1);
                queue.push_back(neighbor);
            }
        }
    }

    depths
}

/// Compute the set of passage names that participate in any game loop.
fn compute_loop_members(workspace: &knot_core::Workspace) -> std::collections::HashSet<String> {
    let mut var_writes: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            let writes: Vec<String> = passage
                .persistent_variable_inits()
                .map(|v| v.name.clone())
                .collect();
            if !writes.is_empty() {
                var_writes.insert(passage.name.clone(), writes);
            }
        }
    }
    let game_loops = workspace.graph.detect_game_loops_for_export(&var_writes);
    game_loops
        .iter()
        .flat_map(|gl| gl.members.iter().cloned())
        .collect()
}

/// Sort passages by: reachable first (desc), then depth ascending, then
/// passage name ascending.
fn sort_passages(passages: &mut [KnotVariablePassage]) {
    passages.sort_by(|a, b| {
        b.reachable
            .cmp(&a.reachable)
            .then(a.depth.cmp(&b.depth))
            .then(a.passage_name.cmp(&b.passage_name))
    });
}

/// Build `KnotVariablePassage` list from own references (written_in + read_in).
fn build_own_passages(
    written_in: Vec<knot_formats::types::VariableUsageLocation>,
    read_in: Vec<knot_formats::types::VariableUsageLocation>,
    depths: &std::collections::HashMap<String, u32>,
    reachable_set: &std::collections::HashSet<String>,
    loop_members: &std::collections::HashSet<String>,
) -> Vec<KnotVariablePassage> {
    // Combine all references
    let mut all_refs: Vec<(knot_formats::types::VariableUsageLocation, bool)> = Vec::new();
    for loc in written_in {
        all_refs.push((loc, true));
    }
    for loc in read_in {
        all_refs.push((loc, false));
    }

    // Group by passage
    let mut passage_map: std::collections::BTreeMap<String, Vec<KnotVariableLocation>> =
        std::collections::BTreeMap::new();
    for (loc, is_write) in &all_refs {
        passage_map
            .entry(loc.passage_name.clone())
            .or_default()
            .push(KnotVariableLocation {
                is_write: *is_write,
                line: loc.line,
                file_uri: loc.file_uri.clone(),
                is_struct_def: false,
                is_reassign: false,
                type_conflict: false,
            });
    }

    // Build KnotVariablePassage list
    let mut passages: Vec<KnotVariablePassage> = passage_map
        .into_iter()
        .map(|(passage_name, refs)| {
            let depth = depths.get(&passage_name).copied().unwrap_or(u32::MAX);
            let reachable = reachable_set.contains(&passage_name);
            let in_loop = loop_members.contains(&passage_name);
            KnotVariablePassage {
                passage_name,
                depth,
                reachable,
                in_loop,
                total_refs: refs.len() as u32,
                references: refs,
            }
        })
        .collect();

    sort_passages(&mut passages);
    passages
}

/// Mark the first write in StoryInit as `is_struct_def`.
fn mark_struct_def(passages: &mut [KnotVariablePassage]) {
    for passage in passages.iter_mut() {
        if passage.passage_name == "StoryInit" {
            if let Some(first_write) = passage.references.iter_mut().find(|r| r.is_write) {
                first_write.is_struct_def = true;
            }
        }
    }
}

/// Mark reassigns: if the variable has children (is an object) and has a
/// direct write in a non-StoryInit passage, that write is a reassign.
fn mark_reassigns(passages: &mut [KnotVariablePassage], has_children: bool) {
    if !has_children {
        return;
    }
    for passage in passages.iter_mut() {
        if passage.passage_name != "StoryInit" {
            for r in &mut passage.references {
                if r.is_write {
                    r.is_reassign = true;
                }
            }
        }
    }
}

/// Bubble up children's passages into the parent's passages list.
///
/// For parent variables, the passages list includes passages where any child
/// is referenced (bubbled up). `total_refs` for a passage is the sum of own
/// refs + all children's refs in that passage. Individual child references
/// are NOT added to the parent's `references` list.
fn bubble_up_children(
    own_passages: &mut Vec<KnotVariablePassage>,
    children: &[KnotVariableInfo],
) {
    let mut merged: std::collections::HashMap<String, KnotVariablePassage> =
        std::collections::HashMap::new();

    // Add own passages
    for p in own_passages.drain(..) {
        match merged.get_mut(&p.passage_name) {
            Some(existing) => {
                existing.total_refs += p.total_refs;
                existing.references.extend(p.references);
            }
            None => {
                merged.insert(p.passage_name.clone(), p);
            }
        }
    }

    // Add children's passages (bubbled up) — only add total_refs, NOT individual references
    for child in children {
        for child_passage in &child.passages {
            merged
                .entry(child_passage.passage_name.clone())
                .and_modify(|existing| {
                    existing.total_refs += child_passage.total_refs;
                })
                .or_insert(KnotVariablePassage {
                    passage_name: child_passage.passage_name.clone(),
                    depth: child_passage.depth,
                    reachable: child_passage.reachable,
                    in_loop: child_passage.in_loop,
                    total_refs: child_passage.total_refs,
                    references: Vec::new(),
                });
        }
    }

    // Re-sort
    let mut result: Vec<KnotVariablePassage> = merged.into_values().collect();
    sort_passages(&mut result);

    *own_passages = result;
}

/// Compute diagnostic flags for a variable based on its own references.
fn compute_flags(var: &KnotVariableInfo) -> Vec<VariableFlag> {
    let mut flags = Vec::new();

    // Count only OWN references (not bubbled-up children refs)
    let own_writes: u32 = var
        .passages
        .iter()
        .flat_map(|p| p.references.iter())
        .filter(|r| r.is_write)
        .count() as u32;
    let own_reads: u32 = var
        .passages
        .iter()
        .flat_map(|p| p.references.iter())
        .filter(|r| !r.is_write)
        .count() as u32;
    let own_passage_count: u32 = var
        .passages
        .iter()
        .filter(|p| !p.references.is_empty())
        .count() as u32;

    if own_writes > 0 && own_reads == 0 {
        flags.push(VariableFlag {
            flag_type: "write-only".to_string(),
            message: "Written but never read. Consider a temp variable.".to_string(),
        });
    } else if own_writes == 0 && own_reads == 0 && !var.has_children {
        // Only flag as "unused" if the variable has no children that are used.
        // Variables referenced only via children aren't truly unused.
        flags.push(VariableFlag {
            flag_type: "unused".to_string(),
            message: "Defined but never referenced.".to_string(),
        });
    } else if own_passage_count == 1 && own_writes + own_reads > 0 {
        flags.push(VariableFlag {
            flag_type: "single-use".to_string(),
            message: "Only used in one passage. Consider a temp variable if persistence isn't needed."
                .to_string(),
        });
    }

    flags
}

/// Recursively convert a `VariablePropertyNode` to `KnotVariableInfo`.
fn convert_property_node(
    prop: knot_formats::types::VariablePropertyNode,
    depths: &std::collections::HashMap<String, u32>,
    reachable_set: &std::collections::HashSet<String>,
    loop_members: &std::collections::HashSet<String>,
    parent_full_name: &str,
) -> KnotVariableInfo {
    let name = strip_dollar_prefix(&prop.name);
    let full_name = format!("{}.{}", parent_full_name, name);

    // Convert sub-properties recursively
    let children: Vec<KnotVariableInfo> = prop
        .properties
        .into_iter()
        .map(|p| convert_property_node(p, depths, reachable_set, loop_members, &full_name))
        .collect();

    let has_children = !children.is_empty();
    let struct_type = if has_children {
        Some("object".to_string())
    } else {
        None
    };

    // Build own passages from written_in + read_in
    let mut own_passages = build_own_passages(prop.written_in, prop.read_in, depths, reachable_set, loop_members);

    // Mark StoryInit first write as struct_def
    mark_struct_def(&mut own_passages);

    // Bubble up children passages
    bubble_up_children(&mut own_passages, &children);

    let ref_count = own_passages.iter().map(|p| p.total_refs).sum();
    let passage_count = own_passages.len() as u32;

    let mut var = KnotVariableInfo {
        name,
        full_name,
        is_temporary: false, // properties are never temporary
        ref_count,
        passage_count,
        has_children,
        struct_type,
        flags: Vec::new(),
        children,
        passages: own_passages,
    };

    var.flags = compute_flags(&var);
    var
}

/// Recursively convert a `VariableTreeNode` to `KnotVariableInfo`.
fn convert_variable_node(
    node: knot_formats::types::VariableTreeNode,
    depths: &std::collections::HashMap<String, u32>,
    reachable_set: &std::collections::HashSet<String>,
    loop_members: &std::collections::HashSet<String>,
) -> KnotVariableInfo {
    let name = strip_dollar_prefix(&node.name);
    let full_name = name.clone(); // top-level: full_name == stripped name

    // Convert properties to children recursively
    let children: Vec<KnotVariableInfo> = node
        .properties
        .into_iter()
        .map(|p| convert_property_node(p, depths, reachable_set, loop_members, &full_name))
        .collect();

    let has_children = !children.is_empty();
    let struct_type = if has_children {
        Some("object".to_string())
    } else {
        None
    };

    // Build own passages from written_in + read_in
    let mut own_passages = build_own_passages(node.written_in, node.read_in, depths, reachable_set, loop_members);

    // Mark StoryInit first write as struct_def
    mark_struct_def(&mut own_passages);

    // Mark reassigns for object variables with writes outside StoryInit
    mark_reassigns(&mut own_passages, has_children);

    // Bubble up children passages
    bubble_up_children(&mut own_passages, &children);

    let ref_count = own_passages.iter().map(|p| p.total_refs).sum();
    let passage_count = own_passages.len() as u32;

    let mut var = KnotVariableInfo {
        name,
        full_name,
        is_temporary: node.is_temporary,
        ref_count,
        passage_count,
        has_children,
        struct_type,
        flags: Vec::new(),
        children,
        passages: own_passages,
    };

    var.flags = compute_flags(&var);
    var
}

impl ServerState {
    /// `knot/graph` — export the passage graph for the Story Map webview.
    pub async fn knot_graph(
        &self,
        params: KnotGraphParams,
    ) -> Result<KnotGraphResponse, tower_lsp::jsonrpc::Error> {
        let mut inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/graph: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }
        // If the Story Map is opened while indexing or after an edit path left
        // the canonical graph empty, rebuild from the authoritative document
        // set before exporting. Do the common export path under a read lock,
        // and upgrade to a write lock only for the rare rebuild case.
        let mut document_count = inner.workspace.documents().count();
        let mut passage_count: usize = inner
            .workspace
            .documents()
            .map(|doc| doc.passages.len())
            .sum();
        if inner.workspace.graph.passage_count() == 0 && passage_count > 0 {
            drop(inner);

            let mut writable = self.inner.write().await;
            document_count = writable.workspace.documents().count();
            passage_count = writable
                .workspace
                .documents()
                .map(|doc| doc.passages.len())
                .sum();
            if writable.workspace.graph.passage_count() == 0 && passage_count > 0 {
                let format = writable.workspace.resolve_format();
                writable.workspace.graph = helpers::rebuild_graph(
                    &writable.workspace,
                    &writable.format_registry,
                    format,
                );
                tracing::info!(
                    "knot/graph: rebuilt empty graph from {} documents / {} passages",
                    document_count, passage_count
                );
            }
            drop(writable);

            inner = self.inner.read().await;
            document_count = inner.workspace.documents().count();
            passage_count = inner
                .workspace
                .documents()
                .map(|doc| doc.passages.len())
                .sum();
        }

        tracing::trace!(
            "knot/graph: exporting {} documents / {} passages / {} graph nodes",
            document_count,
            passage_count,
            inner.workspace.graph.passage_count()
        );

        // Collect passage tags and positions from all documents
        let mut passage_tags: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut passage_lines: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut passage_positions: std::collections::HashMap<String, (f64, f64)> =
            std::collections::HashMap::new();
        let mut passage_groups: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut passage_colors: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for doc in inner.workspace.documents() {
            for passage in &doc.passages {
                passage_tags.insert(passage.name.clone(), passage.tags.clone());
            }
            // Find passage line numbers and positions from the document text
            if let Some(text) = inner.open_documents.get(&doc.uri) {
                for (line_num, line) in text.lines().enumerate() {
                    if line.starts_with("::") {
                        let name = helpers::parse_passage_name_from_header(&line[2..]);
                        passage_lines.insert(name.clone(), line_num as u32);
                        // Try to extract metadata from the header line's JSON block
                        // Twee 3 format: :: Passage Name [tags] {"position":"x,y","group":"G","color":"#ff6600"}
                        if let Some(meta) = helpers::parse_passage_metadata_from_header(line) {
                            if let Some(pos) = meta.position {
                                passage_positions.insert(name.clone(), pos);
                            }
                            if let Some(group) = meta.group {
                                passage_groups.insert(name.clone(), group);
                            }
                            if let Some(color) = meta.color {
                                passage_colors.insert(name.clone(), color);
                            }
                        }
                    }
                }
            }
        }

        // Also extract positions from StoryData if available (Twine 2 JSON format
        // stores passage positions in the "position" field of each passage entry)
        if let Some(text) = inner.workspace.documents().find_map(|doc| {
            inner.open_documents.get(&doc.uri)
        }) {
            helpers::extract_positions_from_storydata(text, &mut passage_positions);
        }

        // Determine unreachable passages
        let start_passage = inner
            .workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let unreachable_diags = inner.workspace.graph.detect_unreachable(start_passage);
        let unreachable_set: std::collections::HashSet<String> = unreachable_diags
            .iter()
            .map(|d| d.passage_name.clone())
            .collect();

        // Collect variable write/read summaries per passage for the graph export.
        let mut var_writes: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut var_reads: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for doc in inner.workspace.documents() {
            for passage in &doc.passages {
                let writes: Vec<String> = passage
                    .persistent_variable_inits()
                    .map(|v| v.name.clone())
                    .collect();
                let reads: Vec<String> = passage
                    .persistent_variable_reads()
                    .map(|v| v.name.clone())
                    .collect();
                if !writes.is_empty() {
                    var_writes.insert(passage.name.clone(), writes);
                }
                if !reads.is_empty() {
                    var_reads.insert(passage.name.clone(), reads);
                }
            }
        }

        let export = inner.workspace.graph.export_graph_with_metadata_and_vars(
            &passage_tags,
            &unreachable_set,
            &passage_positions,
            &var_writes,
            &var_reads,
            &passage_groups,
            &passage_colors,
        );

        let nodes: Vec<KnotGraphNode> = export
            .nodes
            .into_iter()
            .map(|n| KnotGraphNode {
                id: n.id,
                label: n.label.clone(),
                file: n.file.clone(),
                line: passage_lines.get(&n.label).copied().unwrap_or(0),
                tags: n.tags,
                out_degree: n.out_degree,
                in_degree: n.in_degree,
                is_special: n.is_special,
                is_metadata: n.is_metadata,
                is_unreachable: n.is_unreachable,
                position_x: n.position.map(|(x, _)| x),
                position_y: n.position.map(|(_, y)| y),
                group: n.group,
                color: n.color,
                var_writes: n.var_writes,
                var_reads: n.var_reads,
                block: n.block,
            })
            .collect();

        let edges: Vec<KnotGraphEdge> = export
            .edges
            .into_iter()
            .map(|e| KnotGraphEdge {
                source: e.source,
                target: e.target,
                edge_type: format!("{:?}", e.edge_type).to_lowercase(),
                display_text: e.display_text,
            })
            .collect();

        let game_loops: Vec<KnotGameLoop> = export
            .game_loops
            .into_iter()
            .map(|gl| KnotGameLoop {
                members: gl.members,
                header: gl.header,
                has_mutation: gl.has_mutation,
            })
            .collect();

        Ok(KnotGraphResponse {
            nodes,
            edges,
            game_loops,
            layout: Some("dagre".to_string()),
        })
    }

    /// `knot/build` — trigger project compilation.
    pub async fn knot_build(
        &self,
        params: KnotBuildParams,
    ) -> Result<KnotBuildResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/build: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let root_uri = inner.workspace.root_uri.clone();
        let config = inner.workspace.config.clone();
        drop(inner);

        let root_path = match root_uri.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                return Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec!["Workspace root is not a valid file path".to_string()],
                });
            }
        };

        // Resolve compiler path: config override > PATH lookup
        let compiler_path = if let Some(ref path) = config.compiler_path {
            Some(path.clone())
        } else {
            helpers::which_compiler()
        };

        let Some(compiler_path) = compiler_path else {
            return Ok(KnotBuildResponse {
                success: false,
                output_path: None,
                errors: vec![
                    "No Twine compiler found. Install Tweego and ensure it is on PATH, or set compiler_path in .vscode/knot.json".to_string()
                ],
            });
        };

        // Determine output directory
        let output_dir = root_path.join(&config.build.output_dir);
        std::fs::create_dir_all(&output_dir).ok();

        let output_file = output_dir.join("index.html");

        // Build the command arguments
        let mut args: Vec<String> = Vec::new();

        // If a start passage is specified, add --start flag before the output argument
        if let Some(ref start_passage) = params.start_passage {
            args.push("--start".to_string());
            args.push(start_passage.clone());
        }

        args.push("-o".to_string());
        args.push(output_file.to_string_lossy().to_string());
        args.extend(config.build.flags.iter().cloned());
        args.push(root_path.to_string_lossy().to_string());

        tracing::info!("Build command: {} {}", compiler_path.display(), args.join(" "));

        // Run the compiler
        let output = tokio::process::Command::new(&compiler_path)
            .args(&args)
            .current_dir(&root_path)
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Stream build output to the client
                for line in stdout.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: false,
                        })
                        .await;
                }
                for line in stderr.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: true,
                        })
                        .await;
                }

                if output.status.success() {
                    tracing::info!("Build succeeded: {}", output_file.display());
                    Ok(KnotBuildResponse {
                        success: true,
                        output_path: Some(output_file.to_string_lossy().to_string()),
                        errors: Vec::new(),
                    })
                } else {
                    let error_lines: Vec<String> = stderr.lines().map(|l| l.to_string()).collect();
                    tracing::warn!("Build failed: {}", error_lines.join("; "));
                    Ok(KnotBuildResponse {
                        success: false,
                        output_path: None,
                        errors: if error_lines.is_empty() {
                            vec!["Build failed with no error output".to_string()]
                        } else {
                            error_lines
                        },
                    })
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute compiler: {}", e);
                Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec![format!("Failed to execute compiler: {}", e)],
                })
            }
        }
    }

    /// `knot/play` — compile the project and return the HTML path for preview.
    pub async fn knot_play(
        &self,
        params: KnotPlayParams,
    ) -> Result<KnotPlayResponse, tower_lsp::jsonrpc::Error> {
        // Build first
        let build_result = self.knot_build(KnotBuildParams {
            workspace_uri: params.workspace_uri.clone(),
            start_passage: params.start_passage.clone(),
        }).await?;

        if build_result.success {
            Ok(KnotPlayResponse {
                html_path: build_result.output_path,
                error: None,
            })
        } else {
            Ok(KnotPlayResponse {
                html_path: None,
                error: Some(build_result.errors.join("\n")),
            })
        }
    }

    /// `knot/variableFlow` — export variable dataflow information.
    ///
    /// Delegates to the format plugin's `build_variable_tree()` method, which
    /// produces format-agnostic `VariableTreeNode` instances. The server then
    /// converts these to the new recursive `KnotVariableInfo` wire type,
    /// enriching each passage entry with BFS reachability depth, loop
    /// membership, and diagnostic flags.
    pub async fn knot_variable_flow(
        &self,
        params: KnotVariableFlowParams,
    ) -> Result<KnotVariableFlowResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/variableFlow: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable flow for formats that support it
        let plugin = inner.format_registry.get(&format);
        let supports_tracking = plugin.as_ref().map_or(false, |p| {
            p.supports_full_variable_tracking() || p.supports_partial_variable_tracking()
        });
        if !supports_tracking {
            return Ok(KnotVariableFlowResponse {
                variables: Vec::new(),
            });
        }

        // Delegate tree construction to the format plugin.
        // The plugin returns format-agnostic VariableTreeNode instances.
        let tree_nodes = if let Some(p) = plugin {
            let source_text = crate::state::DocumentCache(&inner.open_documents);
            p.build_variable_tree(workspace, &source_text)
        } else {
            Vec::new()
        };

        // Compute BFS reachability depths for all passages
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let depths = compute_bfs_depths(workspace, start_passage);
        let reachable_set: std::collections::HashSet<String> = depths.keys().cloned().collect();

        // Compute which passages are in loops
        let loop_members = compute_loop_members(workspace);

        // Convert tree nodes to new KnotVariableInfo format.
        // Filter out temporary variables — the Variable Tracking panel only
        // shows persistent variables.
        let variables: Vec<KnotVariableInfo> = tree_nodes
            .into_iter()
            .filter(|node| !node.is_temporary)
            .map(|node| convert_variable_node(node, &depths, &reachable_set, &loop_members))
            .collect();

        Ok(KnotVariableFlowResponse { variables })
    }

    /// `knot/passageDiagnostics` — return diagnostic information about a specific passage.
    ///
    /// Returns linter issues, link connections, and passage metadata.
    /// Variable data is available separately via `knot/watchVariables`.
    pub async fn knot_passage_diagnostics(
        &self,
        params: KnotPassageDiagnosticsParams,
    ) -> Result<KnotPassageDiagnosticsResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/passageDiagnostics: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;

        // Find the passage
        let (doc, passage) = match workspace.find_passage(&params.passage_name) {
            Some(result) => result,
            None => {
                let name = params.passage_name.clone();
                return Ok(KnotPassageDiagnosticsResponse {
                    passage_name: params.passage_name,
                    file_uri: String::new(),
                    is_reachable: false,
                    is_special: false,
                    is_metadata: false,
                    outgoing_links: Vec::new(),
                    incoming_links: Vec::new(),
                    diagnostics: vec![KnotPassageDiagnostic {
                        kind: "NotFound".to_string(),
                        message: format!("Passage '{}' not found in workspace", name),
                    }],
                });
            }
        };

        let file_uri = doc.uri.to_string();

        // Determine reachability
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let unreachable_diags = workspace.graph.detect_unreachable(start_passage);
        let is_reachable = !unreachable_diags.iter().any(|d| d.passage_name == params.passage_name);

        // Outgoing links
        let outgoing_links: Vec<KnotPassageLink> = passage
            .links
            .iter()
            .map(|l| KnotPassageLink {
                passage_name: l.target.clone(),
                display_text: l.display_text.clone(),
                target_exists: workspace.find_passage(&l.target).is_some(),
            })
            .collect();

        // Incoming links
        let mut incoming_links = Vec::new();
        for other_doc in workspace.documents() {
            for other_passage in &other_doc.passages {
                for link in &other_passage.links {
                    if link.target == params.passage_name {
                        incoming_links.push(KnotPassageLink {
                            passage_name: other_passage.name.clone(),
                            display_text: link.display_text.clone(),
                            target_exists: true,
                        });
                    }
                }
            }
        }

        // Diagnostics for this passage (use format-delegated analysis)
        let all_diagnostics = helpers::analyze_with_format_vars(workspace, &inner.format_registry);
        let diagnostics: Vec<KnotPassageDiagnostic> = all_diagnostics
            .iter()
            .filter(|d| d.passage_name == params.passage_name)
            .map(|d| KnotPassageDiagnostic {
                kind: format!("{:?}", d.kind),
                message: d.message.clone(),
            })
            .collect();

        Ok(KnotPassageDiagnosticsResponse {
            passage_name: params.passage_name,
            file_uri,
            is_reachable,
            is_special: passage.is_special,
            is_metadata: passage.is_metadata(),
            outgoing_links,
            incoming_links,
            diagnostics,
        })
    }

    /// `knot/profile` — return workspace profiling statistics.
    pub async fn knot_profile(
        &self,
        params: KnotProfileParams,
    ) -> Result<KnotProfileResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/profile: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;

        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        // Basic counts
        let document_count = workspace.document_count() as u32;
        let passage_count = workspace.passage_count() as u32;

        let mut special_passage_count: u32 = 0;
        let mut metadata_passage_count: u32 = 0;
        let mut total_word_count: u32 = 0;
        let mut dead_end_count: u32 = 0;
        let mut orphaned_count: u32 = 0;

        let mut all_variables: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut var_writes_for_loops: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        // Per-passage word counts for complexity metrics
        let mut passage_word_counts: Vec<u32> = Vec::new();
        let mut passage_out_links: Vec<u32> = Vec::new();
        // Per-tag statistics
        let mut tag_data: HashMap<String, (u32, u32, u32)> = HashMap::new(); // tag → (count, total_words, total_out_links)

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    metadata_passage_count += 1;
                }
                if passage.is_special {
                    special_passage_count += 1;
                }

                // Word count (approximate from text blocks)
                let passage_words: u32 = passage
                    .body
                    .iter()
                    .map(|block| match block {
                        knot_core::passage::Block::Text { content, .. } => content.split_whitespace().count() as u32,
                        _ => 0,
                    })
                    .sum();
                total_word_count += passage_words;

                if !passage.is_metadata() {
                    passage_word_counts.push(passage_words);

                    let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                    passage_out_links.push(out_count);

                    // Dead-end detection
                    if !passage.is_special {
                        let has_outgoing = !passage.links.is_empty() || out_count > 0;
                        if !has_outgoing {
                            dead_end_count += 1;
                        }
                    }

                    // Orphan detection (0 incoming links)
                    let in_count = workspace.graph.incoming_neighbors(&passage.name).len();
                    if in_count == 0 && !passage.is_special {
                        orphaned_count += 1;
                    }
                }

                // Per-tag statistics
                for tag in &passage.tags {
                    let entry = tag_data.entry(tag.clone()).or_insert((0, 0, 0));
                    entry.0 += 1;
                    entry.1 += passage_words;
                    entry.2 += workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                }

                // Variable collection
                for var in &passage.vars {
                    if !var.is_temporary {
                        all_variables.insert(var.name.clone());
                    }
                }

                // Collect var writes for game loop detection
                let writes: Vec<String> = passage
                    .persistent_variable_inits()
                    .map(|v| v.name.clone())
                    .collect();
                if !writes.is_empty() {
                    var_writes_for_loops.insert(passage.name.clone(), writes);
                }
            }
        }

        // Build tag stats
        let tag_stats: Vec<KnotTagStat> = tag_data
            .into_iter()
            .map(|(tag, (count, total_words, total_out))| KnotTagStat {
                tag,
                passage_count: count,
                avg_word_count: if count > 0 { total_words as f64 / count as f64 } else { 0.0 },
                total_word_count: total_words,
                avg_out_links: if count > 0 { total_out as f64 / count as f64 } else { 0.0 },
            })
            .collect();

        // Complexity metrics
        let avg_word_count = if !passage_word_counts.is_empty() {
            passage_word_counts.iter().sum::<u32>() as f64 / passage_word_counts.len() as f64
        } else {
            0.0
        };

        let mut sorted_words = passage_word_counts.clone();
        sorted_words.sort();
        let median_word_count = if !sorted_words.is_empty() {
            let mid = sorted_words.len() / 2;
            if sorted_words.len().is_multiple_of(2) {
                (sorted_words[mid - 1] + sorted_words[mid]) as f64 / 2.0
            } else {
                sorted_words[mid] as f64
            }
        } else {
            0.0
        };

        let max_word_count = passage_word_counts.iter().max().copied().unwrap_or(0);
        let min_word_count = passage_word_counts.iter().filter(|&&w| w > 0).min().copied().unwrap_or(0);

        let avg_out_links = if !passage_out_links.is_empty() {
            let sum: u32 = passage_out_links.iter().sum();
            sum as f64 / passage_out_links.len() as f64
        } else {
            0.0
        };

        let out_links_stddev = if passage_out_links.len() > 1 {
            let mean = avg_out_links;
            let variance: f64 = passage_out_links
                .iter()
                .map(|&x| (x as f64 - mean).powi(2))
                .sum::<f64>()
                / (passage_out_links.len() - 1) as f64;
            variance.sqrt()
        } else {
            0.0
        };

        let complex_passage_count = passage_out_links.iter().filter(|&&x| x > 6).count() as u32;

        // Graph analysis (use format-delegated variable diagnostics)
        let diagnostics = helpers::analyze_with_format_vars(workspace, &inner.format_registry);
        let unreachable_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::UnreachablePassage)).count() as u32;
        let broken_link_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::BrokenLink)).count() as u32;
        // Compute game loop count from the graph
        let game_loop_count = workspace.graph.game_loop_count(&var_writes_for_loops) as u32;
        let variable_issue_count: u32 = diagnostics.iter().filter(|d| matches!(
            d.kind,
            knot_core::graph::DiagnosticKind::UninitializedVariable
            | knot_core::graph::DiagnosticKind::UnusedVariable
            | knot_core::graph::DiagnosticKind::RedundantWrite
            | knot_core::graph::DiagnosticKind::VariableAvailabilityHint
            | knot_core::graph::DiagnosticKind::UnusedVariableHint
            | knot_core::graph::DiagnosticKind::RedundantWriteHint
            | knot_core::graph::DiagnosticKind::UnknownPropertyHint
        )).count() as u32;

        let total_links = workspace.graph.edge_count() as u32;
        let graph_passage_count = workspace.graph.passage_count() as u32;

        let avg_out_degree = if graph_passage_count > 0 {
            total_links as f64 / graph_passage_count as f64
        } else {
            0.0
        };

        let avg_in_degree = avg_out_degree;

        // Compute max depth from start passage using BFS
        let max_depth = helpers::compute_max_depth(workspace, start_passage);

        // Link distribution
        let mut zero_links: u32 = 0;
        let mut few_links: u32 = 0;
        let mut moderate_links: u32 = 0;
        let mut many_links: u32 = 0;

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                match out_count {
                    0 => zero_links += 1,
                    1..=2 => few_links += 1,
                    3..=5 => moderate_links += 1,
                    _ => many_links += 1,
                }
            }
        }

        let format = workspace.resolve_format();
        let format_version = workspace.metadata.as_ref().and_then(|m| m.format_version.clone());
        let has_story_data = workspace.metadata.is_some();
        let ifid = workspace.metadata.as_ref().and_then(|m| m.ifid.clone());

        // Extract story name from the StoryTitle passage body.
        // In Twee, the story name is the body text of the special "StoryTitle" passage.
        let story_name = workspace.documents().find_map(|doc| {
            doc.story_title().and_then(|p| {
                // The body is typically a single Text block containing the name.
                p.body.first().and_then(|block| match block {
                    Block::Text { content, .. } => {
                        let trimmed = content.trim();
                        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
                    }
                    _ => None,
                })
            })
        });

        // Structural balance analysis
        let non_meta_count = if passage_count > metadata_passage_count {
            passage_count - metadata_passage_count
        } else {
            1
        };

        let dead_end_ratio = dead_end_count as f64 / non_meta_count as f64;
        let orphaned_ratio = orphaned_count as f64 / non_meta_count as f64;

        // Compute connected components using graph
        let connected_components = helpers::compute_connected_components(workspace);
        let is_well_connected = connected_components <= 1;

        // Compute diameter (longest shortest path from start)
        let diameter = max_depth; // Simplified — uses max depth as approximation

        // Average clustering coefficient
        let avg_clustering = helpers::compute_avg_clustering(workspace);

        Ok(KnotProfileResponse {
            document_count,
            passage_count,
            special_passage_count,
            metadata_passage_count,
            unreachable_passage_count: unreachable_count,
            broken_link_count,
            game_loop_count,
            total_links,
            avg_out_degree,
            avg_in_degree,
            max_depth,
            dead_end_count,
            variable_count: all_variables.len() as u32,
            variable_issue_count,
            story_name,
            format: format.to_string(),
            format_version,
            ifid,
            has_story_data,
            total_word_count,
            link_distribution: KnotLinkDistribution {
                zero_links,
                few_links,
                moderate_links,
                many_links,
            },
            tag_stats,
            complexity_metrics: KnotComplexityMetrics {
                avg_word_count,
                median_word_count,
                max_word_count,
                min_word_count,
                avg_out_links,
                out_links_stddev,
                complex_passage_count,
            },
            structural_balance: KnotStructuralBalance {
                dead_end_ratio,
                orphaned_ratio,
                is_well_connected,
                connected_components,
                diameter,
                avg_clustering,
            },
        })
    }

    /// `knot/compilerDetect` — detect whether a Twine compiler is available.
    pub async fn knot_compiler_detect(
        &self,
        params: KnotCompilerDetectParams,
    ) -> Result<KnotCompilerDetectResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/compilerDetect: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let config = inner.workspace.config.clone();
        drop(inner);

        // Check configured path first
        if let Some(ref path) = config.compiler_path
            && path.exists() {
                return Ok(KnotCompilerDetectResponse {
                    compiler_found: true,
                    compiler_name: Some("tweego".to_string()),
                    compiler_version: helpers::detect_compiler_version(path).await,
                    compiler_path: Some(path.to_string_lossy().to_string()),
                });
            }

        // Check PATH
        if let Some(path) = helpers::which_compiler() {
            return Ok(KnotCompilerDetectResponse {
                compiler_found: true,
                compiler_name: Some("tweego".to_string()),
                compiler_version: helpers::detect_compiler_version(&path).await,
                compiler_path: Some(path.to_string_lossy().to_string()),
            });
        }

        Ok(KnotCompilerDetectResponse {
            compiler_found: false,
            compiler_name: None,
            compiler_version: None,
            compiler_path: None,
        })
    }

    /// `knot/watchVariables` — get variable state at a specific passage.
    pub async fn knot_watch_variables(
        &self,
        params: KnotWatchVariablesParams,
    ) -> Result<KnotWatchVariablesResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/watchVariables: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable watch for formats that support it
        let plugin = inner.format_registry.get(&format);
        let supports_tracking = plugin.as_ref().map_or(false, |p| {
            p.supports_full_variable_tracking() || p.supports_partial_variable_tracking()
        });
        if !supports_tracking {
            return Ok(KnotWatchVariablesResponse {
                at_passage: params.at_passage,
                initialized_at_entry: Vec::new(),
                written_in_passage: Vec::new(),
                read_in_passage: Vec::new(),
                potentially_uninitialized: Vec::new(),
            });
        }

        // Run dataflow analysis
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(workspace, start_passage, &passage_data, &seed_init);

        // Get passage info
        let (doc_uri, passage) = match workspace.find_passage(&params.at_passage) {
            Some((doc, p)) => (doc.uri.to_string(), p),
            None => {
                return Ok(KnotWatchVariablesResponse {
                    at_passage: params.at_passage,
                    initialized_at_entry: Vec::new(),
                    written_in_passage: Vec::new(),
                    read_in_passage: Vec::new(),
                    potentially_uninitialized: Vec::new(),
                });
            }
        };

        let entry_init = flow_states
            .get(&params.at_passage)
            .map(|s| &s.entry)
            .cloned()
            .unwrap_or_default();

        // Apply filter if specified
        let filter_set: Option<std::collections::HashSet<String>> = params
            .filter
            .map(|f| f.into_iter().collect());

        // Build initialized-at-entry list
        let initialized_at_entry: Vec<KnotWatchVariable> = entry_init
            .iter()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(*v))
            })
            .map(|v| KnotWatchVariable {
                name: v.clone(),
                is_temporary: false,
                file_uri: doc_uri.clone(),
                last_written_in: None, // Could be enhanced with backward tracing
            })
            .collect();

        // Build written-in-passage list
        let written_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_inits()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: Some(params.at_passage.clone()),
            })
            .collect();

        // Build read-in-passage list
        let read_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_reads()
            .filter(|v| {
                filter_set.as_ref().is_none_or(|f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: None,
            })
            .collect();

        // Build potentially-uninitialized list
        let mut local_init = entry_init;
        let mut potentially_uninitialized = Vec::new();

        for var in passage.vars_sorted_by_span() {
            if var.is_temporary { continue; }
            if filter_set.as_ref().is_none_or(|f| f.contains(&var.name)) {
                match var.kind {
                    knot_core::passage::VarKind::Read => {
                        if !local_init.contains(&var.name) {
                            potentially_uninitialized.push(KnotWatchVariable {
                                name: var.name.clone(),
                                is_temporary: false,
                                file_uri: doc_uri.clone(),
                                last_written_in: None,
                            });
                        }
                    }
                    knot_core::passage::VarKind::Init => {
                        local_init.insert(var.name.clone());
                    }
                }
            }
        }

        Ok(KnotWatchVariablesResponse {
            at_passage: params.at_passage,
            initialized_at_entry,
            written_in_passage,
            read_in_passage,
            potentially_uninitialized,
        })
    }

    /// `knot/reindexWorkspace` — trigger a full re-index of the workspace.
    /// `knot/generateIfid` — generate a new IFID (Interactive Fiction IDentifier).
    ///
    /// IFIDs are UUIDs in uppercase per the Twine/Twee specification.
    /// This endpoint is accessible at workspace init time so that clients
    /// can generate IFIDs for new project skeletons.
    pub async fn knot_generate_ifid(
        &self,
        params: KnotGenerateIfidParams,
    ) -> Result<KnotGenerateIfidResponse, tower_lsp::jsonrpc::Error> {
        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let inner = self.inner.read().await;
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/generateIfid: workspace_uri '{}' doesn't match server root '{}' — proceeding anyway",
                    params.workspace_uri, root
                );
            }
            drop(inner);
        }

        let ifid = knot_core::Workspace::generate_ifid();
        tracing::info!("Generated IFID: {}", ifid);

        Ok(KnotGenerateIfidResponse { ifid })
    }

    /// `knot/reindexWorkspace` — re-index all workspace files.
    pub async fn knot_reindex_workspace(
        &self,
        params: KnotReindexParams,
    ) -> Result<KnotReindexResponse, tower_lsp::jsonrpc::Error> {
        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let inner = self.inner.read().await;
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/reindexWorkspace: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
            drop(inner);
        }

        tracing::info!("knot/reindexWorkspace: re-indexing workspace");

        // Reset workspace state — create a fresh workspace keeping the root URI and config
        {
            let mut inner = self.inner.write().await;
            let root = inner.workspace.root_uri.clone();
            let config = inner.workspace.config.clone();
            inner.workspace = knot_core::Workspace::new(root);
            inner.workspace.config = config;
            inner.open_documents.clear();
            inner.editor_open_docs.clear();
            inner.format_diagnostics.clear();
        }

        // Run the full indexing pipeline
        match helpers::index_workspace(&self.inner, &self.client).await {
            Ok(()) => {
                let inner = self.inner.read().await;
                let files_indexed = inner.workspace.document_count() as u32;
                Ok(KnotReindexResponse {
                    success: true,
                    files_indexed,
                    error: None,
                })
            }
            Err(e) => {
                tracing::error!("Re-indexing failed: {}", e);
                Ok(KnotReindexResponse {
                    success: false,
                    files_indexed: 0,
                    error: Some(e),
                })
            }
        }
    }

    /// `knot/updatePositions` — update passage position metadata in source files.
    ///
    /// When the user drags nodes in the Story Map graph view, the webview
    /// sends new positions via this request. The server updates the
    /// `{"position":"x,y"}` metadata in the Twee passage headers using the
    /// standard Twee 3 format: `:: PassageName [tags] {"position":"x,y"}`.
    ///
    /// This preserves compatibility with Twine and other Twee editors — no
    /// custom metadata format is introduced.
    pub async fn knot_update_positions(
        &self,
        params: KnotUpdatePositionsParams,
    ) -> Result<KnotUpdatePositionsResponse, tower_lsp::jsonrpc::Error> {
        // Use a WRITE lock to prevent concurrent position updates from
        // reading stale open_documents. The old READ lock allowed multiple
        // updates to see the same stale text, producing duplicate JSON
        // metadata blocks when their edits were applied sequentially.
        let inner = self.inner.write().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/updatePositions: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let mut changes: HashMap<url::Url, Vec<LspTextEdit>> = HashMap::new();
        let mut updated_count: u32 = 0;
        let mut errors: Vec<String> = Vec::new();

        for update in &params.updates {
            // Find the file URI for this passage
            let file_uri = match inner.workspace.find_passage_file_uri(&update.passage_name) {
                Some(uri) => uri,
                None => {
                    errors.push(format!(
                        "Passage '{}' not found in workspace",
                        update.passage_name
                    ));
                    continue;
                }
            };

            // Get the file text
            let text = match inner.open_documents.get(&file_uri) {
                Some(t) => t,
                None => {
                    errors.push(format!(
                        "File text not available for passage '{}' ({})",
                        update.passage_name, file_uri
                    ));
                    continue;
                }
            };

            // Find the passage header line and build the replacement
            let mut found = false;
            for (line_idx, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let name = helpers::parse_passage_name_from_header(&line[2..]);
                    if name == update.passage_name {
                        // Use the full metadata writer when group or color are
                        // provided alongside position. This writes the entire
                        // JSON metadata block (position + group + color), not
                        // just the position field.
                        let new_line = if update.group.is_some() || update.color.is_some() {
                            helpers::update_passage_metadata_in_header(
                                line,
                                Some((update.position_x, update.position_y)),
                                update.group.as_deref().map(Some),
                                update.color.as_deref().map(Some),
                            )
                        } else {
                            helpers::update_passage_position_in_header(
                                line,
                                update.position_x,
                                update.position_y,
                            )
                        };

                        let range = lsp_types::Range {
                            start: lsp_types::Position {
                                line: line_idx as u32,
                                character: 0,
                            },
                            end: lsp_types::Position {
                                line: line_idx as u32,
                                character: helpers::utf16_len(line),
                            },
                        };

                        changes
                            .entry(file_uri.clone())
                            .or_default()
                            .push(LspTextEdit {
                                range,
                                new_text: new_line,
                            });

                        found = true;
                        break;
                    }
                }
            }

            if found {
                updated_count += 1;
            } else {
                errors.push(format!(
                    "Could not find header line for passage '{}' in file {}",
                    update.passage_name, file_uri
                ));
            }
        }

        // ── Apply the edits via the LSP client ────────────────────────────
        // We do NOT optimistically update open_documents here. The LSP client
        // will apply the edits and send did_change notifications, which will
        // update open_documents correctly. Optimistic updates caused the
        // did_change handler to double-apply the same changes, producing
        // duplicate JSON metadata blocks and garbled passage names.
        //
        // To prevent concurrent position updates from reading stale text, we
        // hold the write lock while computing edits and only drop it after
        // the edit list is finalized. The actual apply_edit is async and
        // cannot hold the lock (it would deadlock with did_change), but by
        // the time a second update_positions arrives, the first one's
        // did_change will have already updated open_documents.
        drop(inner); // release write lock before async apply_edit

        // Apply the edits via the LSP client
        if !changes.is_empty() {
            let workspace_edit = WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            };

            let result = self.client.apply_edit(workspace_edit).await;
            if let Err(e) = result {
                tracing::error!("knot/updatePositions: failed to apply edits: {}", e);
                return Ok(KnotUpdatePositionsResponse {
                    success: false,
                    updated_count: 0,
                    errors: vec![format!("Failed to apply workspace edit: {}", e)],
                });
            }
        }

        tracing::info!(
            "knot/updatePositions: updated {} passage positions",
            updated_count
        );

        Ok(KnotUpdatePositionsResponse {
            success: errors.is_empty(),
            updated_count,
            errors,
        })
    }
}

