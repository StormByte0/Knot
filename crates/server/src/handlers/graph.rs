//! Custom LSP request handlers for the Story Map (knot/graph, knot/updatePositions).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use lsp_types::{TextEdit as LspTextEdit, WorkspaceEdit};
use std::collections::HashMap;
use url;

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
                    params.workspace_uri,
                    root
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
                writable.workspace.graph =
                    helpers::rebuild_graph(&writable.workspace, &writable.format_registry, format);
                tracing::info!(
                    "knot/graph: rebuilt empty graph from {} documents / {} passages",
                    document_count,
                    passage_count
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

        // ── Collect Story Map metadata from the Passage model ────────────
        //
        // Everything below (tags, line numbers, positions, groups, colors,
        // sizes) is parsed ONCE by the format plugins' full-document parse
        // (header::apply_header_metadata) and carried on each Passage. The
        // export used to rescan every open document's raw text on every
        // request — this reads the model instead, so it also works for
        // CLOSED files (which used to get line 0) and never fights the
        // parser over what the header metadata means.
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
        let mut passage_sizes: std::collections::HashMap<String, (f64, f64)> =
            std::collections::HashMap::new();

        for doc in inner.workspace.documents() {
            for passage in &doc.passages {
                let name = passage.name.clone();
                passage_tags.insert(name.clone(), passage.tags.clone());
                passage_lines.insert(name.clone(), passage.line);
                if let Some(pos) = passage.position {
                    passage_positions.insert(name.clone(), pos);
                }
                if let Some(group) = passage.group.clone() {
                    passage_groups.insert(name.clone(), group);
                }
                if let Some(color) = passage.color.clone() {
                    passage_colors.insert(name.clone(), color);
                }
                if let Some(size) = passage.size {
                    passage_sizes.insert(name.clone(), size);
                }
            }
        }

        // StoryData fallback: Twine 2 HTML archives can carry per-passage
        // positions inside the StoryData JSON body. Positions run against
        // EVERY open document that actually contains a StoryData passage —
        // the old code only looked at the FIRST open document, silently
        // dropping the data when StoryData lived in a different file.
        // StoryData positions overwrite header metadata (historical
        // precedence).
        for doc in inner.workspace.documents() {
            if doc.passages.iter().any(|p| p.name == "StoryData")
                && let Some(text) = inner.open_documents.get(&doc.uri)
            {
                helpers::extract_positions_from_storydata(text, &mut passage_positions);
            }
        }

        // Determine unreachable passages. Reachability runs from ALL roots:
        // the start passage plus every passage marked `reachable` in
        // its header metadata (manual entry points for variable-aliased
        // navigation that static analysis can't resolve).
        let start_passage = inner
            .workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let unreachable_diags = inner.workspace.detect_unreachable_passages();
        let unreachable_set: std::collections::HashSet<String> = unreachable_diags
            .iter()
            .map(|d| d.passage_name.clone())
            .collect();
        let manual_entries: std::collections::HashSet<String> = inner
            .workspace
            .documents()
            .flat_map(|doc| doc.passages.iter())
            .filter(|p| p.is_manual_entry())
            .map(|p| p.name.clone())
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
            .map(|n| {
                let size = passage_sizes.get(&n.label);
                KnotGraphNode {
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
                    is_entry: manual_entries.contains(&n.label),
                    position_x: n.position.map(|(x, _)| x),
                    position_y: n.position.map(|(_, y)| y),
                    group: n.group,
                    color: n.color,
                    size_w: size.map(|(w, _)| *w),
                    size_h: size.map(|(_, h)| *h),
                    var_writes: n.var_writes,
                    var_reads: n.var_reads,
                }
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
                    params.workspace_uri,
                    root
                );
            }
        }

        let mut changes: HashMap<url::Url, Vec<LspTextEdit>> = HashMap::new();
        let mut updated_count: u32 = 0;
        let mut errors: Vec<String> = Vec::new();

        // ── Resolve each update to its file, then edit each file ONCE ────
        //
        // The old loop rescanned the whole file (`text.lines()`) PER UPDATE,
        // making a "save all positions" O(passages × file_lines). We group
        // the updates by target file here, then do a single header-line
        // pass per file below. Multiple updates for the same passage name
        // are collapsed to the first one (the old code's first-match-wins
        // header scan made duplicates undefined anyway — two edits on the
        // same line, last-apply-wins at the client).
        let mut updates_by_file: HashMap<url::Url, Vec<&KnotPositionUpdate>> = HashMap::new();
        for update in &params.updates {
            match inner.workspace.find_passage_file_uri(&update.passage_name) {
                Some(file_uri) => updates_by_file.entry(file_uri).or_default().push(update),
                None => errors.push(format!(
                    "Passage '{}' not found in workspace",
                    update.passage_name
                )),
            }
        }

        for (file_uri, file_updates) in &updates_by_file {
            let text = match inner.open_documents.get(file_uri) {
                Some(t) => t,
                None => {
                    for update in file_updates {
                        errors.push(format!(
                            "File text not available for passage '{}' ({})",
                            update.passage_name, file_uri
                        ));
                    }
                    continue;
                }
            };

            // passage name → update (first update for a name wins).
            let mut pending: HashMap<&str, &KnotPositionUpdate> = HashMap::new();
            for update in file_updates {
                pending
                    .entry(update.passage_name.as_str())
                    .or_insert(update);
            }
            // Track which names were matched by a header line so the
            // error pass can report unmatched updates in request order.
            let mut found_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            // Single header-line pass over the file: each `::` line is
            // parsed once, and if it belongs to one of the pending updates,
            // its whole-line replacement is queued. Duplicate passage names
            // in one file only match the FIRST header (the old behavior).
            for (line_idx, line) in text.lines().enumerate() {
                if !line.starts_with("::") {
                    continue;
                }
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if !pending.contains_key(name.as_str()) || !found_names.insert(name.clone()) {
                    continue;
                }
                let update = pending[name.as_str()];

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

                updated_count += 1;
            }

            // Report unmatched updates, preserving the request order.
            for update in file_updates {
                if !found_names.contains(&update.passage_name) {
                    errors.push(format!(
                        "Could not find header line for passage '{}' in file {}",
                        update.passage_name, file_uri
                    ));
                }
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
