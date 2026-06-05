//! Document synchronization handlers: did_open, did_change, did_close,
//! did_save, did_change_configuration, did_change_watched_files.

use crate::handlers::helpers;
use crate::state::{ServerState, ServerStateInner};
use knot_core::editing::graph_surgery;
use knot_core::passage::{Passage, StoryFormat};
use lsp_types::*;
use url::Url;

pub(crate) async fn did_open(state: &ServerState, params: DidOpenTextDocumentParams) {
    // Short-circuit if the server is shutting down
    if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let text = params.text_document.text;
    let version = params.text_document.version;

    tracing::info!("did_open: {}", uri);

    let mut inner = state.inner.write().await;

    // Store the LSP version in doc_versions so it survives re-parses
    inner.doc_versions.insert(uri.clone(), version);

    // If workspace indexing is still in progress, do a lightweight insert
    // only — the indexing pass will rebuild the graph and publish
    // diagnostics once all files are loaded.  Without this guard,
    // did_open races with index_workspace: it rebuilds the graph with
    // only the files loaded so far, publishes diagnostics showing
    // passages as orphaned, and those stale diagnostics persist until
    // the next edit triggers a fresh analysis.
    let indexing_in_progress = !inner.workspace.indexed;

    // Clean up any stale URI-equivalent entries from workspace indexing.
    // We collect stale keys first to avoid double mutable borrow issues.
    let stale_keys: Vec<Url> = {
        let canonical_path = uri.to_file_path().ok();
        match canonical_path {
            Some(path) => inner.open_documents.keys()
                .filter(|k| **k != uri)
                .filter(|k| k.to_file_path().map_or(false, |p| p == path))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    };
    for key in &stale_keys {
        tracing::debug!("Removing stale URI-equivalent entry: {} (canonical: {})", key, uri);
        inner.open_documents.remove(key);
        inner.format_diagnostics.remove(key);
    }

    inner.editor_open_docs.insert(uri.clone());
    inner.open_documents.insert(uri.clone(), text.clone());

    // Capture format before StoryData extraction so we can detect changes
    let format_before: Option<StoryFormat> = inner.workspace.metadata.as_ref().map(|m| m.format.clone());

    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format, version);

    // Store format diagnostics for this document
    inner.format_diagnostics.insert(
        uri.clone(),
        parse_result.diagnostics.clone(),
    );

    // Check for StoryData in the newly opened document
    helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);

    inner.workspace.insert_document(doc);
    tracing::info!(
        passage_count = inner.workspace.get_document(&uri)
            .map(|d| d.passages.len()).unwrap_or(0),
        passages = ?inner.workspace.get_document(&uri)
            .map(|d| d.passages.iter().map(|p| format!("{}(links={},vars={},special={})", 
                p.name, p.links.len(), p.vars.len(), p.is_special)).collect::<Vec<_>>())
            .unwrap_or_default(),
        "did_open: passages defined"
    );

    // If workspace indexing is still in progress, skip the graph rebuild
    // and diagnostic publish — index_workspace will do both once all
    // files have been loaded.  Doing it here would race with the
    // indexing loop and produce a graph built from only the files
    // loaded so far, causing false "orphaned passage" diagnostics.
    if indexing_in_progress {
        tracing::debug!("did_open: skipping graph rebuild — workspace indexing in progress");
        drop(inner);
        return;
    }

    let format_after = inner.workspace.resolve_format();
    inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format_after.clone());

    // If the format changed (or was first detected), notify the client
    let should_notify = format_before != Some(format_after.clone());
    let doc_uris: Vec<String> = inner.open_documents.keys().map(|u| u.to_string()).collect();


    // Release write lock before analysis — same two-phase pattern as did_change
    drop(inner);

    // Read-lock phase: analysis (read-only)
    let (diagnostics, open_docs, fmt_diags, config) = {
        let inner = state.inner.read().await;
        let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        (diagnostics, open_docs, fmt_diags, config)
    };

    {
        let inner = state.inner.read().await;
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format);
        let sigils: Vec<char> = plugin.as_ref()
            .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
            .unwrap_or_default();
        helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
    }

    if should_notify {
        helpers::send_format_detected(&state.client, format_after, doc_uris).await;
    }

    // Notify other open documents that their semantic tokens may be stale
    // due to cross-file link resolution changes from this document opening.
    let all_uris: Vec<Url> = {
        let inner = state.inner.read().await;
        inner.open_documents.keys().cloned().collect()
    };
    helpers::send_semantic_token_refresh(
        &state.client,
        &uri,
        &all_uris,
        "document opened — cross-file link resolution may have changed",
    ).await;

    // Notify the client that the virtual document has been updated.
    // The client will re-fetch the virtual doc and route JS diagnostics
    // to .tw source positions.
    helpers::send_virtual_doc_refresh(
        &state.client,
        "document opened — virtual doc map updated",
    ).await;
}

pub(crate) async fn did_change(state: &ServerState, params: DidChangeTextDocumentParams) {
    // Short-circuit if the server is shutting down
    if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let version = params.text_document.version;

    tracing::debug!("did_change: {} (v{})", uri, version);

    let mut inner = state.inner.write().await;

    // Update doc_versions with the authoritative LSP version
    inner.doc_versions.insert(uri.clone(), version);

    // Apply incremental changes to the rope-based snapshot and get the
    // resulting full text for re-parsing.
    let text = apply_document_changes(&mut inner, &uri, version, params.content_changes);

    // Always update the text cache immediately so go-to-definition etc.
    // see the latest content
    inner.open_documents.insert(uri.clone(), text.clone());

    // Record the edit after updating the text cache, but do not skip the
    // parse/analysis pass.
    inner.debounce.record_edit();

    if inner.debounce.needs_flush() {
        inner.debounce.clear_skipped();
    }

    // Parse with format plugin
    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), version);

    // Update format diagnostics
    inner.format_diagnostics.insert(
        uri.clone(),
        parse_result.diagnostics.clone(),
    );

    let old_passages: Vec<Passage> = inner
        .workspace
        .get_document(&uri)
        .map(|d| d.passages.clone())
        .unwrap_or_default();
    tracing::trace!(
        file = %uri,
        old_passage_count = old_passages.len(),
        old_passages = ?old_passages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        "did_change: comparing old vs new passages"
    );
    let new_passages = doc.passages.clone();

    // Compute dynamic navigation edges for the new passages
    let extra_edges: Vec<(String, Option<String>, String)> = if let Some(plug) = inner.format_registry.get(&format) {
        let var_string_map = plug.build_var_string_map(&inner.workspace);
        new_passages.iter()
            .flat_map(|p| {
                plug.resolve_dynamic_navigation_links(p, &var_string_map)
                    .into_iter()
                    .map(|link| (p.name.clone(), link.display_text, link.target))
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    };

    // Check for StoryData changes
    let format_before: Option<StoryFormat> = inner.workspace.metadata.as_ref().map(|m| m.format.clone());
    helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);

    inner.workspace.insert_document(doc);
    let file_uri_str = uri.to_string();
    let surgery_result = graph_surgery(
        &mut inner.workspace.graph,
        &old_passages,
        &new_passages,
        &file_uri_str,
        &extra_edges,
    );
    tracing::trace!(
        "graph_surgery result: added={:?} removed={:?} modified={:?}, graph nodes={} edges={}",
        surgery_result.added,
        surgery_result.removed,
        surgery_result.modified,
        inner.workspace.graph.passage_count(),
        inner.workspace.graph.edge_count()
    );

    // Update broken-link flags on all edges after surgery
    inner.workspace.graph.recheck_broken_links();

    // Re-add upstream lifecycle edges for special passages after surgery.
    // graph_surgery() strips implicit edges during incremental update,
    // so we must re-establish the upstream chain. The graph's special_bundle
    // was populated incrementally by add_passage(), so we can query it
    // directly instead of re-scanning workspace documents.
    let start_passage_name: String = inner.workspace.metadata
        .as_ref()
        .map(|m| m.start_passage.clone())
        .unwrap_or_else(|| "Start".into());

    {
        let graph = &mut inner.workspace.graph;

        let script_injection = graph.special_bundle.script_injection.clone();
        let startup = graph.special_bundle.startup.clone();

        // Upstream edge: ScriptInjection → Startup
        for script_name in &script_injection {
            for startup_name in &startup {
                let exists = graph.outgoing_neighbors(script_name).iter().any(|n| n == startup_name);
                if !exists {
                    graph.add_edge(script_name, startup_name, knot_core::graph::PassageEdge {
                        display_text: Some(format!("(upstream: {} → {})", script_name, startup_name)),
                        edge_type: knot_core::graph::EdgeType::Upstream,
                        pre_broken_type: None,
                    });
                }
            }
        }

        // Bridge edge: Startup → Start passage
        if !startup.is_empty() {
            if graph.contains_passage(&start_passage_name) {
                let bridge_source = &startup[0];
                let exists = graph.outgoing_neighbors(bridge_source).iter().any(|n| *n == start_passage_name);
                if !exists {
                    graph.add_edge(bridge_source, &start_passage_name, knot_core::graph::PassageEdge {
                        display_text: Some(format!("(upstream: {} → {})", bridge_source, start_passage_name)),
                        edge_type: knot_core::graph::EdgeType::Upstream,
                        pre_broken_type: None,
                    });
                }
            }
        }
    }

    // ── Phase 1 complete: all state mutations are done. ──────────────
    // Release the write lock early so that read-lock handlers
    // (codeAction, documentLink, inlayHint, etc.) are not blocked while
    // we run the (read-only) diagnostic analysis below.  This is the key
    // fix for the "Cannot call write after a stream was destroyed" race:
    // the shorter the write-lock hold time, the less likely a restart
    // will catch in-flight handlers still waiting for the lock.

    // Check if format changed after StoryData extraction
    let format_after = inner.workspace.resolve_format();
    let should_notify = format_before != Some(format_after.clone());
    let doc_uris: Vec<String> = inner.open_documents.keys().map(|u| u.to_string()).collect();


    drop(inner); // ← release write lock

    // ── Phase 2: read-lock — analysis (read-only) ──────────────────
    let (diagnostics, open_docs, fmt_diags, config) = {
        let inner = state.inner.read().await;
        let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
        tracing::trace!(
            file = %uri,
            diagnostic_count = diagnostics.len(),
            workspace_total_passages = inner.workspace.passage_count(),
            workspace_total_documents = inner.workspace.document_count(),
            graph_nodes = inner.workspace.graph.passage_count(),
            graph_edges = inner.workspace.graph.edge_count(),
            "did_change: analysis complete"
        );

        // Log passage count summary (not full list — that was too noisy and
        // produced huge debug output on every keystroke)
        {
            let total_passages: usize = inner.workspace.documents()
                .map(|d| d.passages.len())
                .sum();
            let total_docs = inner.workspace.document_count();
            tracing::debug!(
                total_documents = total_docs,
                total_passages,
                "did_change: workspace summary"
            );
        }

        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        (diagnostics, open_docs, fmt_diags, config)
    }; // ← read lock dropped

    // ── Phase 3: publish (needs workspace read lock for variable related info) ──
    {
        let inner = state.inner.read().await;
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format);
        let sigils: Vec<char> = plugin.as_ref()
            .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
            .unwrap_or_default();
        helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
    }

    if should_notify {
        helpers::send_format_detected(&state.client, format_after, doc_uris).await;
    }

    // Notify other open documents that their semantic tokens may be stale
    // due to broken link resolution changes.
    helpers::send_semantic_token_refresh(
        &state.client,
        &uri,
        &open_docs.keys().cloned().collect::<Vec<_>>(),
        "cross-file link resolution may have changed",
    ).await;

    // Notify the client that the virtual document has been updated
    // after processing the text document change.
    helpers::send_virtual_doc_refresh(
        &state.client,
        "document changed — virtual doc map updated",
    ).await;
}

pub(crate) async fn did_close(state: &ServerState, params: DidCloseTextDocumentParams) {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    tracing::info!("did_close: {}", uri);

    let mut inner = state.inner.write().await;
    // Remove from editor-open set only; keep text in open_documents cache
    // so that features like find-references still work for closed files
    inner.editor_open_docs.remove(&uri);
    inner.format_diagnostics.remove(&uri);
    // Clean up the version entry to prevent unbounded memory growth.
    // The version will be re-inserted with the client's authoritative
    // version if the document is re-opened.
    inner.doc_versions.remove(&uri);
    drop(inner);

    // Clear diagnostics for the closed file.
    state.client
        .publish_diagnostics(uri, Vec::new(), None)
        .await;
}

pub(crate) async fn did_save(_state: &ServerState, params: DidSaveTextDocumentParams) {
    tracing::info!("did_save: {}", params.text_document.uri);
}

pub(crate) async fn did_change_configuration(state: &ServerState, _params: DidChangeConfigurationParams) {
    tracing::info!("did_change_configuration");

    // Re-read .vscode/knot.json in case it was changed externally
    {
        let inner = state.inner.read().await;
        let root_uri = &inner.workspace.root_uri;
        if let Ok(root_path) = root_uri.to_file_path() {
            let config_path = root_path.join(".vscode").join("knot.json");
            if config_path.exists() {
                drop(inner);
                let mut inner = state.inner.write().await;
                if let Ok(config_text) = std::fs::read_to_string(&config_path) {
                    if let Err(e) = inner.workspace.load_config(&config_text) {
                        tracing::warn!("Failed to reload knot.json on config change: {}", e);
                    } else {
                        tracing::info!("Reloaded .vscode/knot.json after configuration change");
                    }
                }
            }
        }
    }

    // Fetch VS Code diagnostic settings via workspace/configuration
    let diag_keys: [(&str, &str); 11] = [
        ("BrokenLink", "broken-link"),
        ("UnreachablePassage", "unreachable-passage"),
        ("UninitializedVariable", "uninitialized-variable"),
        ("UnusedVariable", "unused-variable"),
        ("RedundantWrite", "redundant-write"),
        ("DuplicatePassageName", "duplicate-passage-name"),
        ("EmptyPassage", "empty-passage"),
        ("DeadEndPassage", "dead-end-passage"),
        ("InvalidPassageName", "invalid-passage-name"),
        ("ComplexPassage", "complex-passage"),
        ("LargePassage", "large-passage"),
    ];

    let config_items: Vec<ConfigurationItem> = diag_keys
        .iter()
        .map(|(_, setting_name)| ConfigurationItem {
            scope_uri: None,
            section: Some(format!("knot.diagnostics.{}", setting_name)),
        })
        .collect();

    let config_values = state
        .client
        .configuration(config_items)
        .await
        .unwrap_or_default();

    // Apply VS Code diagnostic settings (they override knot.json defaults)
    let mut inner = state.inner.write().await;
    for (i, (diag_key, _)) in diag_keys.iter().enumerate() {
        if let Some(value) = config_values.get(i)
            && let Some(severity_str) = value.as_str() {
                let severity = match severity_str {
                    "error" => Some(knot_core::workspace::DiagnosticSeverity::Error),
                    "warning" => Some(knot_core::workspace::DiagnosticSeverity::Warning),
                    "info" => Some(knot_core::workspace::DiagnosticSeverity::Info),
                    "hint" => Some(knot_core::workspace::DiagnosticSeverity::Hint),
                    "off" => Some(knot_core::workspace::DiagnosticSeverity::Off),
                    _ => None,
                };
                if let Some(sev) = severity {
                    inner.workspace.config.diagnostics.insert(diag_key.to_string(), sev);
                }
            }
    }

    // Re-run analysis and publish diagnostics with updated config
    // Release write lock before analysis
    drop(inner);

    let (diagnostics, open_docs, fmt_diags, config) = {
        let inner = state.inner.read().await;
        let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        (diagnostics, open_docs, fmt_diags, config)
    };

    {
        let inner = state.inner.read().await;
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format);
        let sigils: Vec<char> = plugin.as_ref()
            .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
            .unwrap_or_default();
        helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
    }
}

pub(crate) async fn did_change_watched_files(state: &ServerState, params: DidChangeWatchedFilesParams) {
    tracing::info!("did_change_watched_files: {} events", params.changes.len());

    for event in params.changes {
        let uri = helpers::normalize_file_uri(&event.uri);
        let file_type = uri.to_file_path().and_then(|p| {
            p.extension()
                .and_then(|e| e.to_str().map(|s| s.to_string()))
                .ok_or(())
        });

        let is_twee = match file_type.as_deref() {
            Ok("tw") | Ok("twee") => true,
            _ => false,
        };

        if !is_twee {
            continue;
        }

        match event.typ {
            FileChangeType::CREATED => {
                tracing::info!("File created: {}", uri);
                // Read and index the new file
                if let Ok(path) = uri.to_file_path()
                    && let Ok(text) = std::fs::read_to_string(&path) {
                        let mut inner = state.inner.write().await;

                        // Remember the current format before inserting the new document
                        let format_before = inner.workspace.resolve_format();

                        let format = inner.workspace.resolve_format();
                        let (doc, parse_result) =
                            helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), 0);

                        inner.open_documents.insert(uri.clone(), text.clone());
                        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                        helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                        inner.workspace.insert_document(doc);

                        // If the new file's StoryData changed the format, re-parse
                        // ALL existing documents with the updated format.
                        let format_after = inner.workspace.resolve_format();
                        if format_before != format_after {
                            tracing::info!(
                                "Format changed from {:?} to {:?} after file creation — re-parsing all documents",
                                format_before, format_after
                            );
                            // Re-parse all documents with the new format
                            let uris: Vec<url::Url> = inner.open_documents.keys().cloned().collect();
                            let texts: Vec<String> = uris.iter()
                                .filter_map(|u| inner.open_documents.get(u).cloned())
                                .collect();

                            for (doc_uri, doc_text) in uris.iter().zip(texts.iter()) {
                                // Skip the newly created file — it's already parsed above
                                if *doc_uri == uri {
                                    continue;
                                }
                                let (re_parsed, re_result) =
                                    helpers::parse_with_format_plugin(&inner.format_registry, doc_uri, doc_text, format_after.clone(), 0);
                                inner.format_diagnostics.insert(doc_uri.clone(), re_result.diagnostics);
                                helpers::extract_and_set_metadata(&mut inner.workspace, &re_parsed, doc_text);
                                inner.workspace.insert_document(re_parsed);
                            }
                        }

                        inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format_after.clone());

                        // Collect doc URIs before dropping the lock
                        let doc_uris: Vec<String> = inner.open_documents.keys().map(|u| u.to_string()).collect();
                        let should_notify = format_before != format_after;

                        // Release write lock before analysis
                        drop(inner);

                        // Read-lock phase: analysis (read-only)
                        let (diagnostics, open_docs, fmt_diags, config) = {
                            let inner = state.inner.read().await;
                            let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
                            let open_docs = inner.open_documents.clone();
                            let fmt_diags = inner.format_diagnostics.clone();
                            let config = inner.workspace.config.clone();
                            (diagnostics, open_docs, fmt_diags, config)
                        };

                        {
                            let inner = state.inner.read().await;
                            let format = inner.workspace.resolve_format();
                            let plugin = inner.format_registry.get(&format);
                            let sigils: Vec<char> = plugin.as_ref()
                                .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
                                .unwrap_or_default();
                            helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
                        }

                        // Notify client if format changed after file creation
                        if should_notify {
                            helpers::send_format_detected(&state.client, format_after, doc_uris).await;
                        }

                        // Notify other open documents that their semantic tokens may be stale
                        // due to a new file being added (affects passage resolution).
                        helpers::send_semantic_token_refresh(
                            &state.client,
                            &uri,
                            &open_docs.keys().cloned().collect::<Vec<_>>(),
                            "file created — passage resolution may have changed",
                        ).await;

                        // Notify the client that the virtual document has been updated
                        // after a new file was created and indexed.
                        helpers::send_virtual_doc_refresh(
                            &state.client,
                            "file created — virtual doc map updated",
                        ).await;
                    }
            }
            FileChangeType::DELETED => {
                tracing::info!("File deleted: {}", uri);
                let mut inner = state.inner.write().await;
                inner.open_documents.remove(&uri);
                inner.editor_open_docs.remove(&uri);
                inner.format_diagnostics.remove(&uri);
                inner.workspace.remove_document_and_update_graph(&uri);

                // Recheck broken links after removal
                inner.workspace.graph.recheck_broken_links();

                // Re-add upstream lifecycle edges for special passages after
                // removal. When a file containing a Startup or ScriptInjection
                // passage is deleted, petgraph removes all edges connected to
                // that node, which can break the upstream chain for remaining
                // special passages. We must re-establish the chain.
                let start_passage_name: String = inner.workspace.metadata
                    .as_ref()
                    .map(|m| m.start_passage.clone())
                    .unwrap_or_else(|| "Start".into());

                {
                    let graph = &mut inner.workspace.graph;

                    let script_injection = graph.special_bundle.script_injection.clone();
                    let startup = graph.special_bundle.startup.clone();

                    // Upstream edge: ScriptInjection → Startup
                    for script_name in &script_injection {
                        for startup_name in &startup {
                            let exists = graph.outgoing_neighbors(script_name).iter().any(|n| n == startup_name);
                            if !exists {
                                graph.add_edge(script_name, startup_name, knot_core::graph::PassageEdge {
                                    display_text: Some(format!("(upstream: {} → {})", script_name, startup_name)),
                                    edge_type: knot_core::graph::EdgeType::Upstream,
                                    pre_broken_type: None,
                                });
                            }
                        }
                    }

                    // Bridge edge: Startup → Start passage
                    if !startup.is_empty() {
                        if graph.contains_passage(&start_passage_name) {
                            let bridge_source = &startup[0];
                            let exists = graph.outgoing_neighbors(bridge_source).iter().any(|n| *n == start_passage_name);
                            if !exists {
                                graph.add_edge(bridge_source, &start_passage_name, knot_core::graph::PassageEdge {
                                    display_text: Some(format!("(upstream: {} → {})", bridge_source, start_passage_name)),
                                    edge_type: knot_core::graph::EdgeType::Upstream,
                                    pre_broken_type: None,
                                });
                            }
                        }
                    }
                }

                // Release write lock before analysis
                drop(inner);

                // Read-lock phase: analysis (read-only)
                let (diagnostics, open_docs, fmt_diags, config) = {
                    let inner = state.inner.read().await;
                    let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
                    let open_docs = inner.open_documents.clone();
                    let fmt_diags = inner.format_diagnostics.clone();
                    let config = inner.workspace.config.clone();
                    (diagnostics, open_docs, fmt_diags, config)
                };

                {
                    let inner = state.inner.read().await;
                    let format = inner.workspace.resolve_format();
                    let plugin = inner.format_registry.get(&format);
                    let sigils: Vec<char> = plugin.as_ref()
                        .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
                        .unwrap_or_default();
                    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
                }

                // Notify remaining open documents that their semantic tokens may be stale
                // due to a file being deleted (broken links may have changed).
                helpers::send_semantic_token_refresh(
                    &state.client,
                    &uri,
                    &open_docs.keys().cloned().collect::<Vec<_>>(),
                    "file deleted — broken link resolution may have changed",
                ).await;

                // Clear diagnostics for the deleted file
                state.client
                    .publish_diagnostics(uri, Vec::new(), None)
                    .await;

                // Notify the client that the virtual document has been updated
                // after a file was deleted.
                helpers::send_virtual_doc_refresh(
                    &state.client,
                    "file deleted — virtual doc map updated",
                ).await;
            }
            FileChangeType::CHANGED => {
                tracing::info!("File changed on disk: {}", uri);
                // Re-read and re-index the file ONLY if it's NOT currently
                // open in the editor. When a file is open, the did_change
                // handler manages updates from the editor buffer.
                let is_editor_open = {
                    let inner = state.inner.read().await;
                    inner.editor_open_docs.contains(&uri)
                };

                if !is_editor_open
                    && let Ok(path) = uri.to_file_path()
                        && let Ok(text) = std::fs::read_to_string(&path) {
                            let mut inner = state.inner.write().await;

                            // Capture format before StoryData extraction
                            let format_before: Option<StoryFormat> = inner.workspace.metadata.as_ref().map(|m| m.format.clone());

                            let format = inner.workspace.resolve_format();
                            let (doc, parse_result) =
                                helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), 0);

                            inner.open_documents.insert(uri.clone(), text.clone());
                            inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                            helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                            inner.workspace.insert_document(doc);

                            let format_after = inner.workspace.resolve_format();
                            inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format_after.clone());

                            // Check if format changed
                            let should_notify = format_before != Some(format_after.clone());
                            let doc_uris: Vec<String> = inner.open_documents.keys().map(|u| u.to_string()).collect();

                            // Release write lock before analysis
                            drop(inner);

                            // Read-lock phase: analysis (read-only)
                            let (diagnostics, open_docs, fmt_diags, config) = {
                                let inner = state.inner.read().await;
                                let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
                                let open_docs = inner.open_documents.clone();
                                let fmt_diags = inner.format_diagnostics.clone();
                                let config = inner.workspace.config.clone();
                                (diagnostics, open_docs, fmt_diags, config)
                            };

                            {
                                let inner = state.inner.read().await;
                                let format = inner.workspace.resolve_format();
                                let plugin = inner.format_registry.get(&format);
                                let sigils: Vec<char> = plugin.as_ref()
                                    .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
                                    .unwrap_or_default();
                                helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &inner.js_diagnostics, &open_docs, &inner.workspace, &config, &sigils).await;
                            }

                            if should_notify {
                                helpers::send_format_detected(&state.client, format_after, doc_uris).await;
                            }

                            // Notify other open documents that their semantic tokens may be stale
                            // due to a file changing on disk (affects passage resolution).
                            helpers::send_semantic_token_refresh(
                                &state.client,
                                &uri,
                                &open_docs.keys().cloned().collect::<Vec<_>>(),
                                "file changed on disk — passage resolution may have changed",
                            ).await;

                            // Notify the client that the virtual document has been updated
                            // after a file changed on disk.
                            helpers::send_virtual_doc_refresh(
                                &state.client,
                                "file changed on disk — virtual doc map updated",
                            ).await;
                        }
            }
            _ => {}
        }
    }
}

/// Apply incremental document changes and return the resulting full text.
///
/// With INCREMENTAL sync, each `TextDocumentContentChangeEvent` contains
/// a `range` (the region being replaced) and `text` (the replacement text).
/// If the range is `None`, the change is a full-text replacement.
///
/// This function:
/// 1. Gets the current document from the workspace
/// 2. If the document has a snapshot, converts each LSP range to a byte range
///    and applies changes incrementally to the rope
/// 3. If no snapshot is available, falls back to the full text from the last
///    change event (backward-compatible behavior)
/// 4. Returns the full text after all changes have been applied
fn apply_document_changes(
    inner: &mut ServerStateInner,
    uri: &Url,
    version: i32,
    content_changes: Vec<TextDocumentContentChangeEvent>,
) -> String {
    use crate::handlers::helpers::lsp_range_to_byte_range;

    // Collect incremental changes as (byte_range, new_text) pairs.
    // We need the current text to convert LSP positions to byte offsets.
    // The current text comes from the rope snapshot (if available) or
    // the open_documents cache.
    let has_snapshot = inner.workspace.get_document(uri)
        .map(|d| d.snapshot.is_some())
        .unwrap_or(false);

    if has_snapshot && !content_changes.is_empty() {
        // Check if all changes have ranges (incremental) or if any are
        // full-text replacements (range is None)
        let has_full_replace = content_changes.iter().any(|c| c.range.is_none());

        if has_full_replace {
            // Full-text replacement — use the text from the last change
            // that has no range (or the last change overall)
            let text = content_changes
                .into_iter()
                .rev()
                .find(|c| c.range.is_none())
                .map(|c| c.text)
                .unwrap_or_default();

            // Rebuild the snapshot from the full text
            if let Some(doc) = inner.workspace.get_document_mut(uri) {
                doc.version = version;
                doc.set_snapshot_from_text(&text);
            }

            tracing::debug!(
                file = %uri,
                version,
                text_len = text.len(),
                "apply_document_changes: full-text replacement"
            );
            text
        } else {
            // All changes have ranges — apply incrementally
            // We need to build the list of (byte_range, new_text) pairs.
            // Important: LSP positions in each change refer to the document
            // state *after* all previous changes in the list have been applied.
            // So we must apply them one at a time, converting positions using
            // the current text state each time.

            // Get the current full text for position conversion
            let current_text = inner.open_documents.get(uri).cloned().unwrap_or_default();

            // Apply changes one by one using Document::apply_incremental_change
            // We need to track the evolving text for position conversion
            let mut evolving_text = current_text;
            let mut byte_changes: Vec<(std::ops::Range<usize>, String)> = Vec::new();

            for change in &content_changes {
                if let Some(range) = &change.range {
                    let byte_range = lsp_range_to_byte_range(&evolving_text, range);
                    byte_changes.push((byte_range.clone(), change.text.clone()));

                    // Update evolving_text to reflect this change so that
                    // subsequent position conversions are correct
                    let mut new_text = String::with_capacity(
                        evolving_text.len() - (byte_range.end - byte_range.start) + change.text.len()
                    );
                    new_text.push_str(&evolving_text[..byte_range.start]);
                    new_text.push_str(&change.text);
                    new_text.push_str(&evolving_text[byte_range.end..]);
                    evolving_text = new_text;
                }
            }

            // Now apply all changes to the document's rope snapshot
            if let Some(doc) = inner.workspace.get_document_mut(uri) {
                match doc.apply_incremental_change(version, &byte_changes) {
                    Some(text) => {
                        tracing::debug!(
                            file = %uri,
                            version,
                            change_count = byte_changes.len(),
                            text_len = text.len(),
                            "apply_document_changes: incremental applied"
                        );
                        return text;
                    }
                    None => {
                        // Snapshot wasn't available after all — fall back
                        tracing::warn!(
                            file = %uri,
                            "apply_document_changes: snapshot unexpectedly None, falling back to full text"
                        );
                    }
                }
            }

            // Fallback: return the evolved text we computed manually
            evolving_text
        }
    } else {
        // No snapshot available — fall back to the last change's full text
        // This is the old FULL-sync behavior
        let text = content_changes
            .into_iter()
            .last()
            .map(|c| c.text)
            .unwrap_or_default();

        tracing::debug!(
            file = %uri,
            version,
            text_len = text.len(),
            "apply_document_changes: no snapshot, using last change text"
        );
        text
    }
}
