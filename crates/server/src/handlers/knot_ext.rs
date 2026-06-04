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
use url;

/// Convert a single format-agnostic `VariablePropertyNode` to LSP wire type
/// `KnotVariableProperty`. This is a pure mechanical translation with no
/// format-specific logic.
fn convert_property_node(p: knot_formats::types::VariablePropertyNode) -> KnotVariableProperty {
    let kind_str = match p.kind {
        knot_formats::types::PropertyKind::Scalar => "scalar",
        knot_formats::types::PropertyKind::Object => "object",
        knot_formats::types::PropertyKind::Array => "array",
        knot_formats::types::PropertyKind::Unknown => "unknown",
    };
    let element_shape = p.element_shape.map(|shape| {
        Box::new(convert_property_node(*shape))
    });
    KnotVariableProperty {
        name: p.name,
        full_name: p.full_name,
        state_path: p.state_path,
        written_in: p.written_in.into_iter().map(|l| KnotVariableLocation {
            passage_name: l.passage_name,
            file_uri: l.file_uri,
            is_write: l.is_write,
            line: l.line,
        }).collect(),
        read_in: p.read_in.into_iter().map(|l| KnotVariableLocation {
            passage_name: l.passage_name,
            file_uri: l.file_uri,
            is_write: l.is_write,
            line: l.line,
        }).collect(),
        properties: convert_properties(p.properties),
        kind: kind_str.to_string(),
        element_shape,
        coverage: p.coverage.map(|c| c.to_string()),
    }
}

/// Recursively convert format-agnostic `VariablePropertyNode` instances
/// to LSP wire type `KnotVariableProperty`. This is a pure mechanical
/// translation with no format-specific logic.
fn convert_properties(
    props: Vec<knot_formats::types::VariablePropertyNode>,
) -> Vec<KnotVariableProperty> {
    props.into_iter().map(convert_property_node).collect()
}

/// Convert a line/character position in a text document to a byte offset.
///
/// VSCode uses line/character positions (0-based). The server needs byte
/// offsets for the VirtualDocManager's binary search. This function walks
/// the content string line by line, accumulating byte positions.
fn line_char_to_byte_range(
    content: &str,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
) -> std::ops::Range<usize> {
    let mut line_starts: Vec<usize> = Vec::new();
    line_starts.push(0);
    for (i, c) in content.char_indices() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    // Add one past the end for the "virtual" line start
    line_starts.push(content.len());

    let start_byte = if (start_line as usize) < line_starts.len() {
        let line_start = line_starts[start_line as usize];
        // Count characters to find the character offset
        let chars_in_line = content[line_start..]
            .lines()
            .next()
            .map(|l| l.chars().count())
            .unwrap_or(0);
        line_start + content[line_start..]
            .chars()
            .take(start_character as usize)
            .map(|c| c.len_utf8())
            .sum::<usize>()
            .min(chars_in_line)
    } else {
        content.len()
    };

    let end_byte = if (end_line as usize) < line_starts.len() {
        let line_start = line_starts[end_line as usize];
        let chars_in_line = content[line_start..]
            .lines()
            .next()
            .map(|l| l.chars().count())
            .unwrap_or(0);
        line_start + content[line_start..]
            .chars()
            .take(end_character as usize)
            .map(|c| c.len_utf8())
            .sum::<usize>()
            .min(chars_in_line)
    } else {
        content.len()
    };

    start_byte..end_byte.max(start_byte)
}

/// Convert a byte range within a text document to an LSP Range.
///
/// Walks the source text to find the line and character positions
/// corresponding to the byte offsets.
fn byte_range_to_lsp_range(
    source: &str,
    byte_range: std::ops::Range<usize>,
) -> lsp_types::Range {
    let mut line_starts: Vec<usize> = Vec::new();
    line_starts.push(0);
    for (i, c) in source.char_indices() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }

    let start = byte_range.start;
    let end = byte_range.end.min(source.len());

    // Find start line/character
    let start_line_idx = line_starts
        .iter()
        .position(|&s| s > start)
        .map(|i| i.saturating_sub(1))
        .unwrap_or(line_starts.len().saturating_sub(1));

    let start_line = start_line_idx as u32;
    let line_start_byte = line_starts.get(start_line_idx).copied().unwrap_or(0);
    let start_character = source[line_start_byte..end.max(line_start_byte)]
        .chars()
        .take_while(|_| {
            let current = line_start_byte + source[line_start_byte..start]
                .chars()
                .map(|c| c.len_utf8())
                .sum::<usize>();
            current < start
        })
        .count() as u32;

    // Find end line/character
    let end_line_idx = line_starts
        .iter()
        .position(|&s| s > end)
        .map(|i| i.saturating_sub(1))
        .unwrap_or(line_starts.len().saturating_sub(1));

    let end_line = end_line_idx as u32;
    let end_line_start_byte = line_starts.get(end_line_idx).copied().unwrap_or(0);
    let end_character = source[end_line_start_byte..end.max(end_line_start_byte)]
        .chars()
        .take_while(|_| {
            let current = end_line_start_byte + source[end_line_start_byte..end]
                .chars()
                .map(|c| c.len_utf8())
                .sum::<usize>();
            current < end
        })
        .count() as u32;

    lsp_types::Range {
        start: lsp_types::Position {
            line: start_line,
            character: start_character,
        },
        end: lsp_types::Position {
            line: end_line,
            character: end_character,
        },
    }
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
                is_start: n.label == start_passage,
                position_x: n.position.map(|(x, _)| x),
                position_y: n.position.map(|(_, y)| y),
                group: n.group,
                color: n.color,
                var_writes: n.var_writes,
                var_reads: n.var_reads,
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
        // The server only does a mechanical translation to LSP wire types.
        let tree_nodes = if let Some(p) = plugin {
            let source_text = crate::state::DocumentCache(&inner.open_documents);
            p.build_variable_tree(workspace, &source_text)
        } else {
            Vec::new()
        };

        // Pure mechanical translation: format-agnostic tree → LSP wire types.
        // No VarAccessKind matching, no "State.variables" hardcoding.
        let variables: Vec<KnotVariableInfo> = tree_nodes
            .into_iter()
            .map(|node| {
                let kind_str = match node.kind {
                    knot_formats::types::PropertyKind::Scalar => "scalar",
                    knot_formats::types::PropertyKind::Object => "object",
                    knot_formats::types::PropertyKind::Array => "array",
                    knot_formats::types::PropertyKind::Unknown => "unknown",
                };
                KnotVariableInfo {
                    name: node.name,
                    state_path: node.state_path,
                    is_temporary: node.is_temporary,
                    written_in: node.written_in.into_iter().map(|l| KnotVariableLocation {
                        passage_name: l.passage_name,
                        file_uri: l.file_uri,
                        is_write: l.is_write,
                        line: l.line,
                    }).collect(),
                    read_in: node.read_in.into_iter().map(|l| KnotVariableLocation {
                        passage_name: l.passage_name,
                        file_uri: l.file_uri,
                        is_write: l.is_write,
                        line: l.line,
                    }).collect(),
                    initialized_at_start: node.initialized_at_start,
                    is_unused: node.is_unused,
                    properties: convert_properties(node.properties),
                    kind: kind_str.to_string(),
                    element_shape: node.element_shape.map(|shape| {
                        Box::new(convert_property_node(*shape))
                    }),
                }
            })
            .collect();

        Ok(KnotVariableFlowResponse {
            variables,
        })
    }

    /// `knot/passageDiagnostics` — return diagnostic information about a specific passage.
    ///
    /// Returns linter issues, link connections, passage metadata, and
    /// variable references (reads/writes with line numbers) resolved from
    /// the virtual document.
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
                    variable_references: Vec::new(),
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

        // ── Variable references (from virtual document) ──────────────────
        // Build the virtual document via the format plugin and extract
        // variable accesses filtered for this passage. This gives us exact
        // line numbers for each read/write, enabling "go to reference" from
        // the passage diagnostics panel.
        let variable_references = helpers::build_passage_variable_references(
            workspace,
            &inner.format_registry,
            &inner.open_documents,
            &params.passage_name,
        );

        Ok(KnotPassageDiagnosticsResponse {
            passage_name: params.passage_name,
            file_uri,
            is_reachable,
            is_special: passage.is_special,
            is_metadata: passage.is_metadata(),
            outgoing_links,
            incoming_links,
            diagnostics,
            variable_references,
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
        let mut unreachable_orphan_count: u32 = 0;

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

                    // Unreachable-orphan detection (0 incoming links, not special)
                    // These are a subset of unreachable passages — the most
                    // severe case where nothing links to the passage at all.
                    let in_count = workspace.graph.incoming_neighbors(&passage.name).len();
                    if in_count == 0 && !passage.is_special {
                        unreachable_orphan_count += 1;
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
        let unreachable_ratio = unreachable_orphan_count as f64 / non_meta_count as f64;

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
                unreachable_ratio,
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

    // -------------------------------------------------------------------
    // knot/virtualDoc — get the assembled virtual document content + line map
    // -------------------------------------------------------------------

    /// Handle `knot/virtualDoc` — return the assembled virtual document for
    /// the current workspace, enabling VSCode's native JS/TS validation on
    /// the translated SugarCube code.
    ///
    /// The response includes:
    /// - `content`: The assembled JavaScript (preamble + widgets + passages)
    /// - `line_map`: Per-line mapping back to .tw source positions
    /// - `passage_names`: All passages included in the virtual doc
    ///
    /// ## Architecture
    ///
    /// This handler uses the new `VirtualDocManager` from `knot_core` when
    /// available. The manager owns the monolithic virtual doc content and
    /// passage entry index. During the migration period, the old
    /// FormatPlugin-based path is used as a fallback when the manager
    /// hasn't been populated yet.
    pub async fn knot_virtual_doc(
        &self,
        params: KnotVirtualDocParams,
    ) -> Result<KnotVirtualDocResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Validate workspace_uri matches our workspace
        if !params.workspace_uri.is_empty() {
            let root = &inner.workspace.root_uri;
            if params.workspace_uri != root.to_string() {
                tracing::warn!(
                    "knot/virtualDoc: workspace_uri '{}' doesn't match server root '{}' — using server root",
                    params.workspace_uri, root
                );
            }
        }

        // ── New path: VirtualDocManager ──────────────────────────────
        // If the VirtualDocManager has been populated (via rebuild during
        // indexing), use its content directly. This is the new architecture
        // where core owns the virtual doc lifecycle.
        let vdoc_manager = &inner.virtual_doc_manager;
        if !vdoc_manager.is_empty() {
            let content = vdoc_manager.content().to_string();
            let passage_names = vdoc_manager.passage_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();

            // Build a line map from the passage entries.
            // For each line, use the VirtualDocManager's binary search to
            // find which passage it belongs to, then compute a passage-relative
            // line number for original_line.
            let mut line_map: Vec<KnotVirtualDocLineEntry> = Vec::new();
            let mut byte_offset = 0usize;

            for line in content.lines() {
                let line_end = byte_offset + line.len() + 1; // +1 for \n

                // Find which passage this line's byte offset falls in
                match vdoc_manager.find_passage_for_byte_range(byte_offset..byte_offset) {
                    Some((passage_name, file_uri, relative_range)) => {
                        // Compute the line number within the passage's js_block.
                        // The relative_range.start is the passage-relative byte offset
                        // of this line. Count newlines to get the line index.
                        let passage_entry = vdoc_manager.get_entry(passage_name);
                        let original_line = if let Some(entry) = passage_entry {
                            let passage_content = &content[entry.byte_range.start..entry.byte_range.end];
                            passage_content[..relative_range.start.min(passage_content.len())]
                                .chars()
                                .filter(|&c| c == '\n')
                                .count() as u32
                        } else {
                            relative_range.start as u32 // rough fallback
                        };

                        line_map.push(KnotVirtualDocLineEntry {
                            passage_name: passage_name.to_string(),
                            file_uri: file_uri.to_string(),
                            original_line,
                        });
                    }
                    None => {
                        // Line doesn't belong to any passage (preamble or gap)
                        line_map.push(KnotVirtualDocLineEntry {
                            passage_name: String::new(),
                            file_uri: String::new(),
                            original_line: 0,
                        });
                    }
                }

                byte_offset = line_end;
            }

            return Ok(KnotVirtualDocResponse {
                content,
                line_map,
                passage_names,
            });
        }

        // ── Fallback: old FormatPlugin path ─────────────────────────
        // During the migration period, fall back to the FormatPlugin's
        // virtual_doc_content() and virtual_doc_line_map() methods.
        // This is the path that uses the SugarCube plugin's VirtualDocMap
        // side table directly.
        let workspace = &inner.workspace;
        let format = workspace.resolve_format();
        let plugin = inner.format_registry.get(&format);

        let (content, line_map, passage_names) = if let Some(p) = plugin {
            let vdoc_content = p.virtual_doc_content();
            let vdoc_line_map = p.virtual_doc_line_map();

            match (vdoc_content, vdoc_line_map) {
                (Some(content), Some(line_map)) => {
                    // Extract passage names from the line map (deduplicated,
                    // preserving first-seen order)
                    let mut names = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for entry in &line_map {
                        if !entry.passage_name.is_empty() && seen.insert(entry.passage_name.clone()) {
                            names.push(entry.passage_name.clone());
                        }
                    }
                    (content, line_map, names)
                }
                _ => (String::new(), Vec::new(), Vec::new()),
            }
        } else {
            (String::new(), Vec::new(), Vec::new())
        };

        // Translate format-agnostic VirtualDocLineMapEntry to LSP wire type
        let wire_line_map: Vec<KnotVirtualDocLineEntry> = line_map
            .into_iter()
            .map(|entry| KnotVirtualDocLineEntry {
                passage_name: entry.passage_name,
                file_uri: entry.file_uri,
                original_line: entry.original_line,
            })
            .collect();

        Ok(KnotVirtualDocResponse {
            content,
            line_map: wire_line_map,
            passage_names,
        })
    }

    /// `knot/jsDiagnostics` — relay JS diagnostics from client to server.
    ///
    /// VSCode's built-in JS service validates the virtual doc and the client
    /// relays the diagnostics to the server. The server runs the two-stage
    /// diagnostic relay:
    /// 1. **Stage 1 (Core):** Convert line/char → byte offset, then binary search
    ///    the PassageEntry index to find which passage the diagnostic falls in.
    /// 2. **Stage 2 (Format):** Delegate to the adapter's `resolve_source_location()`
    ///    for precise .tw byte range and `interpret_diagnostic()` for format-specific
    ///    filtering and message rephrasing.
    ///
    /// The resulting .tw diagnostics are stored in `inner.js_diagnostics` and
    /// published via `textDocument/publishDiagnostics`, merged with graph/format
    /// diagnostics from the indexing pipeline.
    pub async fn knot_js_diagnostics(
        &self,
        params: KnotJsDiagnosticsParams,
    ) -> Result<KnotJsDiagnosticsResponse, tower_lsp::jsonrpc::Error> {
        let mut inner = self.inner.write().await;

        // If no adapter or virtual doc is empty, nothing to do
        if inner.virtual_doc_adapter.is_none() || inner.virtual_doc_manager.is_empty() {
            return Ok(KnotJsDiagnosticsResponse { processed: 0 });
        }

        let vdoc_content = inner.virtual_doc_manager.content().to_string();
        let adapter = inner.virtual_doc_adapter.as_ref().unwrap();
        let source_text_cache = crate::state::CoreDocumentCache(&inner.open_documents);

        let mut processed: u32 = 0;
        let mut new_js_diagnostics: HashMap<url::Url, Vec<lsp_types::Diagnostic>> = HashMap::new();

        for js_diag in &params.diagnostics {
            // Convert line/character positions to byte offsets using the virtual doc content
            let byte_range = line_char_to_byte_range(
                &vdoc_content,
                js_diag.start_line,
                js_diag.start_character,
                js_diag.end_line,
                js_diag.end_character,
            );

            // Stage 1: Find which passage this diagnostic falls in
            let (passage_name, file_uri, vdoc_range) = match inner
                .virtual_doc_manager
                .find_passage_for_byte_range(byte_range)
            {
                Some(result) => result,
                None => continue, // Preamble or gap — skip
            };

            // Build the core JsDiagnostic for the adapter
            let core_js_diag = knot_core::virtual_doc::JsDiagnostic {
                byte_range: vdoc_range,
                message: js_diag.message.clone(),
                severity: match js_diag.severity {
                    1 => knot_core::virtual_doc::DiagnosticSeverity::Error,
                    2 => knot_core::virtual_doc::DiagnosticSeverity::Warning,
                    3 => knot_core::virtual_doc::DiagnosticSeverity::Info,
                    _ => knot_core::virtual_doc::DiagnosticSeverity::Hint,
                },
                code: js_diag.code.clone(),
            };

            // Stage 2: Format-specific resolution and interpretation
            let tw_diag = match adapter.interpret_diagnostic(&core_js_diag, passage_name, file_uri) {
                Some(d) => d,
                None => continue, // Filtered out by adapter (e.g., false positive)
            };

            // Resolve source location via the adapter
            let source_location = adapter.resolve_source_location(
                passage_name,
                file_uri,
                tw_diag.byte_range.clone(),
                source_text_cache.get_source_text(file_uri).unwrap_or(""),
            );

            // Convert byte range to LSP Range using the .tw source text
            let tw_source = inner
                .open_documents
                .get(&url::Url::parse(&source_location.file_uri).unwrap_or_else(|_| url::Url::parse("file:/// ").unwrap()))
                .map(|s| s.as_str())
                .unwrap_or("");

            let range = byte_range_to_lsp_range(tw_source, source_location.byte_range.clone());

            let lsp_diag = lsp_types::Diagnostic {
                range,
                severity: Some(match tw_diag.severity {
                    knot_core::virtual_doc::DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
                    knot_core::virtual_doc::DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                    knot_core::virtual_doc::DiagnosticSeverity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
                    knot_core::virtual_doc::DiagnosticSeverity::Hint => lsp_types::DiagnosticSeverity::HINT,
                }),
                code: tw_diag.code.map(lsp_types::NumberOrString::String),
                source: Some("knot (virtual doc)".to_string()),
                message: tw_diag.message,
                ..Default::default()
            };

            if let Ok(uri) = url::Url::parse(&source_location.file_uri) {
                new_js_diagnostics.entry(uri).or_default().push(lsp_diag);
            }

            processed += 1;
        }

        // Update the stored JS diagnostics. This replaces the entire map
        // because each relay batch represents a complete snapshot of the
        // JS service's current diagnostics (the client debounces and sends
        // all current diagnostics each time).
        inner.js_diagnostics = new_js_diagnostics;
        drop(inner);

        // Trigger a full diagnostic re-publish. This ensures JS diagnostics
        // are merged with graph/format diagnostics rather than overwriting
        // them (LSP's publishDiagnostics replaces ALL diagnostics for a URI).
        // We re-compute graph and format diagnostics to get a consistent snapshot.
        //
        // Note: This is slightly expensive but necessary for correctness.
        // The alternative (publishing only JS diagnostics) would wipe out
        // graph/format diagnostics until the next sync cycle.
        {
            let inner = self.inner.read().await;
            let workspace = &inner.workspace;
            let config = &workspace.config;
            let format = workspace.resolve_format();
            let graph_diagnostics = helpers::analyze_with_format_vars(workspace, &inner.format_registry);
            let fmt_diags = inner.format_registry.get(&format)
                .map(|p| p.analyze_workspace(workspace, &crate::state::DocumentCache(&inner.open_documents)))
                .unwrap_or_default();

            helpers::publish_all_diagnostics(
                &self.client,
                &graph_diagnostics,
                &fmt_diags,
                &inner.js_diagnostics,
                &inner.open_documents,
                workspace,
                config,
            ).await;
        }

        Ok(KnotJsDiagnosticsResponse { processed })
    }
}

