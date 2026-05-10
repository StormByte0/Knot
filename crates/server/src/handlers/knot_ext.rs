//! Custom LSP request handlers (knot/graph, knot/build, knot/play, etc.)
//!
//! These are `impl ServerState` methods (not part of the `LanguageServer`
//! trait) registered as custom methods via `LspService::build`.

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_core::AnalysisEngine;
use std::collections::HashMap;

/// Recursively convert format-agnostic `VariablePropertyNode` instances
/// to LSP wire type `KnotVariableProperty`. This is a pure mechanical
/// translation with no format-specific logic.
fn convert_properties(
    props: Vec<knot_formats::types::VariablePropertyNode>,
) -> Vec<KnotVariableProperty> {
    props
        .into_iter()
        .map(|p| KnotVariableProperty {
            name: p.name,
            full_name: p.full_name,
            state_path: p.state_path,
            written_in: p.written_in.into_iter().map(|l| KnotVariableLocation {
                passage_name: l.passage_name,
                file_uri: l.file_uri,
                is_write: l.is_write,
            }).collect(),
            read_in: p.read_in.into_iter().map(|l| KnotVariableLocation {
                passage_name: l.passage_name,
                file_uri: l.file_uri,
                is_write: l.is_write,
            }).collect(),
            properties: convert_properties(p.properties),
        })
        .collect()
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

        tracing::debug!(
            "knot/graph: exporting {} documents / {} passages / {} graph nodes",
            document_count,
            passage_count,
            inner.workspace.graph.passage_count()
        );

        // Collect passage tags from all documents
        let mut passage_tags: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut passage_lines: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        for doc in inner.workspace.documents() {
            for passage in &doc.passages {
                passage_tags.insert(passage.name.clone(), passage.tags.clone());
            }
            // Find passage line numbers from the document text
            if let Some(text) = inner.open_documents.get(&doc.uri) {
                for (line_num, line) in text.lines().enumerate() {
                    if line.starts_with("::") {
                        let name = helpers::parse_passage_name_from_header(&line[2..]);
                        passage_lines.insert(name, line_num as u32);
                    }
                }
            }
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

        let export = inner.workspace.graph.export_graph_with_metadata(
            &passage_tags,
            &unreachable_set,
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
            })
            .collect();

        let edges: Vec<KnotGraphEdge> = export
            .edges
            .into_iter()
            .map(|e| KnotGraphEdge {
                source: e.source,
                target: e.target,
                is_broken: e.is_broken,
                display_text: e.display_text,
            })
            .collect();

        Ok(KnotGraphResponse {
            nodes,
            edges,
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
    /// performs a **pure mechanical translation** to LSP wire types — no
    /// format-specific logic (no `VarAccessKind` matching, no hardcoded
    /// `"State.variables"` strings) lives here.
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
        if !format.supports_full_variable_tracking() && !format.supports_partial_variable_tracking() {
            return Ok(KnotVariableFlowResponse {
                variables: Vec::new(),
            });
        }

        // Delegate tree construction to the format plugin.
        // The plugin returns format-agnostic VariableTreeNode instances.
        // The server only does a mechanical translation to LSP wire types.
        let plugin = inner.format_registry.get(&format);
        let tree_nodes = if let Some(p) = plugin {
            p.build_variable_tree(workspace)
        } else {
            Vec::new()
        };

        // Pure mechanical translation: format-agnostic tree → LSP wire types.
        // No VarAccessKind matching, no "State.variables" hardcoding.
        let variables: Vec<KnotVariableInfo> = tree_nodes
            .into_iter()
            .map(|node| KnotVariableInfo {
                name: node.name,
                state_path: node.state_path,
                is_temporary: node.is_temporary,
                written_in: node.written_in.into_iter().map(|l| KnotVariableLocation {
                    passage_name: l.passage_name,
                    file_uri: l.file_uri,
                    is_write: l.is_write,
                }).collect(),
                read_in: node.read_in.into_iter().map(|l| KnotVariableLocation {
                    passage_name: l.passage_name,
                    file_uri: l.file_uri,
                    is_write: l.is_write,
                }).collect(),
                initialized_at_start: node.initialized_at_start,
                is_unused: node.is_unused,
                properties: convert_properties(node.properties),
            })
            .collect();

        Ok(KnotVariableFlowResponse {
            variables,
        })
    }

    /// `knot/debug` — return debug information about a specific passage.
    pub async fn knot_debug(
        &self,
        params: KnotDebugParams,
    ) -> Result<KnotDebugResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/debug: workspace_uri '{}' doesn't match server root '{}' — using server root",
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
                return Ok(KnotDebugResponse {
                    passage_name: params.passage_name,
                    file_uri: String::new(),
                    is_reachable: false,
                    is_special: false,
                    is_metadata: false,
                    variables_written: Vec::new(),
                    variables_read: Vec::new(),
                    initialized_at_entry: Vec::new(),
                    outgoing_links: Vec::new(),
                    incoming_links: Vec::new(),
                    predecessors: Vec::new(),
                    successors: Vec::new(),
                    in_infinite_loop: false,
                    diagnostics: vec![KnotDebugDiagnostic {
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

        // Variable info
        let variables_written: Vec<KnotDebugVariable> = passage
            .persistent_variable_inits()
            .map(|v| KnotDebugVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
            })
            .collect();

        let variables_read: Vec<KnotDebugVariable> = passage
            .persistent_variable_reads()
            .map(|v| KnotDebugVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
            })
            .collect();

        // Compute initialized-at-entry from dataflow
        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(workspace, start_passage, &passage_data, &seed_init);
        let initialized_at_entry: Vec<String> = flow_states
            .get(&params.passage_name)
            .map(|s| {
                let mut vars: Vec<String> = s.entry.iter().cloned().collect();
                vars.sort();
                vars
            })
            .unwrap_or_default();

        // Outgoing links
        let outgoing_links: Vec<KnotDebugLink> = passage
            .links
            .iter()
            .map(|l| KnotDebugLink {
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
                        incoming_links.push(KnotDebugLink {
                            passage_name: other_passage.name.clone(),
                            display_text: link.display_text.clone(),
                            target_exists: true, // The target is this passage, which we know exists
                        });
                    }
                }
            }
        }

        // Graph neighbors
        let predecessors = workspace.graph.incoming_neighbors(&params.passage_name);
        let successors = workspace.graph.outgoing_neighbors(&params.passage_name);

        // Check if in infinite loop
        let passage_vars: std::collections::HashMap<String, Vec<&knot_core::passage::VarOp>> =
            AnalysisEngine::collect_passage_vars_as_ref(workspace);
        let loop_diags = workspace.graph.detect_infinite_loops(&passage_vars);
        let in_infinite_loop = loop_diags.iter().any(|d| d.passage_name == params.passage_name);

        // Diagnostics for this passage
        let all_diagnostics = AnalysisEngine::analyze(workspace);
        let diagnostics: Vec<KnotDebugDiagnostic> = all_diagnostics
            .iter()
            .filter(|d| d.passage_name == params.passage_name)
            .map(|d| KnotDebugDiagnostic {
                kind: format!("{:?}", d.kind),
                message: d.message.clone(),
            })
            .collect();

        Ok(KnotDebugResponse {
            passage_name: params.passage_name,
            file_uri,
            is_reachable,
            is_special: passage.is_special,
            is_metadata: passage.is_metadata(),
            variables_written,
            variables_read,
            initialized_at_entry,
            outgoing_links,
            incoming_links,
            predecessors,
            successors,
            in_infinite_loop,
            diagnostics,
        })
    }

    /// `knot/trace` — simulate execution from a given passage.
    pub async fn knot_trace(
        &self,
        params: KnotTraceParams,
    ) -> Result<KnotTraceResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/trace: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;

        let mut steps = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut truncated = false;

        // DFS trace with proper cycle detection using enter/exit events.
        enum TraceAction {
            Enter(String, u32),
            Exit(String),
        }

        let mut stack: Vec<TraceAction> = vec![TraceAction::Enter(params.start_passage.clone(), 0)];
        let mut on_stack = std::collections::HashSet::new();

        while let Some(action) = stack.pop() {
            match action {
                TraceAction::Exit(name) => {
                    on_stack.remove(&name);
                }
                TraceAction::Enter(passage_name, depth) => {
                    if depth > params.max_depth {
                        truncated = true;
                        continue;
                    }

                    let is_loop = on_stack.contains(&passage_name);

                    if visited.contains(&passage_name) && !is_loop {
                        continue;
                    }

                    // Find the passage data
                    let (variables_written, available_links) = if let Some((_, passage)) = workspace.find_passage(&passage_name) {
                        let vars: Vec<String> = passage.persistent_variable_inits().map(|v| v.name.clone()).collect();
                        let links: Vec<String> = passage.links.iter().map(|l| l.target.clone()).collect();
                        (vars, links)
                    } else {
                        (Vec::new(), Vec::new())
                    };

                    steps.push(KnotTraceStep {
                        passage_name: passage_name.clone(),
                        depth,
                        variables_written,
                        available_links: available_links.clone(),
                        is_loop,
                    });

                    if is_loop {
                        // Don't recurse into a loop
                        continue;
                    }

                    visited.insert(passage_name.clone());
                    on_stack.insert(passage_name.clone());

                    // Push exit marker FIRST (so it's processed AFTER all successors)
                    stack.push(TraceAction::Exit(passage_name.clone()));

                    // Push successors (in reverse order so first link is processed first)
                    for target in available_links.into_iter().rev() {
                        if !visited.contains(&target) || on_stack.contains(&target) {
                            stack.push(TraceAction::Enter(target, depth + 1));
                        }
                    }
                }
            }
        }

        Ok(KnotTraceResponse {
            steps,
            truncated,
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

        // Graph analysis
        let diagnostics = AnalysisEngine::analyze(workspace);
        let unreachable_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::UnreachablePassage)).count() as u32;
        let broken_link_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::BrokenLink)).count() as u32;
        let infinite_loop_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::InfiniteLoop)).count() as u32;
        let variable_issue_count: u32 = diagnostics.iter().filter(|d| matches!(
            d.kind,
            knot_core::graph::DiagnosticKind::UninitializedVariable
            | knot_core::graph::DiagnosticKind::UnusedVariable
            | knot_core::graph::DiagnosticKind::RedundantWrite
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
            infinite_loop_count,
            total_links,
            avg_out_degree,
            avg_in_degree,
            max_depth,
            dead_end_count,
            variable_count: all_variables.len() as u32,
            variable_issue_count,
            format: format.to_string(),
            format_version,
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

    /// `knot/breakpoints` — manage debug breakpoints on passages.
    pub async fn knot_breakpoints(
        &self,
        params: KnotBreakpointsParams,
    ) -> Result<KnotBreakpointsResponse, tower_lsp::jsonrpc::Error> {
        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let inner = self.inner.read().await;
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/breakpoints: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
            drop(inner);
        }

        // Handle mutations
        if params.clear_all == Some(true) {
            let mut inner = self.inner.write().await;
            inner.breakpoints.clear();
        } else if let Some(new_breakpoints) = params.set_breakpoints {
            let mut inner = self.inner.write().await;
            inner.breakpoints = new_breakpoints;
        }

        // Build response with current breakpoint state
        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        let breakpoints: Vec<KnotBreakpointInfo> = inner
            .breakpoints
            .iter()
            .map(|name| {
                let (passage_exists, file_uri, incoming, outgoing) =
                    if let Some((doc, passage)) = workspace.find_passage(name) {
                        (
                            true,
                            Some(doc.uri.to_string()),
                            workspace.graph.incoming_neighbors(name).len() as u32,
                            passage.links.len() as u32,
                        )
                    } else {
                        (false, None, 0, 0)
                    };

                KnotBreakpointInfo {
                    passage_name: name.clone(),
                    passage_exists,
                    file_uri,
                    incoming_links: incoming,
                    outgoing_links: outgoing,
                }
            })
            .collect();

        Ok(KnotBreakpointsResponse { breakpoints })
    }

    /// `knot/stepOver` — simulate a single step from a passage.
    pub async fn knot_step_over(
        &self,
        params: KnotStepOverParams,
    ) -> Result<KnotStepOverResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/stepOver: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        let workspace = &inner.workspace;

        let (choices, variables_written, variables_read) =
            if let Some((_, passage)) = workspace.find_passage(&params.from_passage) {
                let choices: Vec<KnotStepChoice> = passage
                    .links
                    .iter()
                    .map(|l| KnotStepChoice {
                        passage_name: l.target.clone(),
                        display_text: l.display_text.clone(),
                        target_exists: workspace.find_passage(&l.target).is_some(),
                    })
                    .collect();

                let written: Vec<String> = passage
                    .persistent_variable_inits()
                    .map(|v| v.name.clone())
                    .collect();

                let read: Vec<String> = passage
                    .persistent_variable_reads()
                    .map(|v| v.name.clone())
                    .collect();

                (choices, written, read)
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };

        Ok(KnotStepOverResponse {
            from_passage: params.from_passage,
            choices,
            variables_written,
            variables_read,
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
        if !format.supports_full_variable_tracking() && !format.supports_partial_variable_tracking() {
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
}

