//! Workspace indexing (two-pass: StoryData discovery + full parse).

use crate::lsp_ext::*;
use knot_core::passage::StoryFormat;
use knot_core::workspace::StoryMetadata;
use lsp_types::*;
use std::collections::HashMap;
use url::Url;

use super::diagnostics::{analyze_with_format_vars, publish_all_diagnostics};
use super::graph::rebuild_graph;
use super::parsing::{extract_and_set_metadata, parse_with_format_plugin, parse_story_data_json};

/// Scan the workspace root for all `.tw` / `.twee` files, parse them with
/// the format plugin, insert into the workspace, build the graph, and run
/// analysis.
///
/// ## Two-pass indexing
///
/// The indexing process uses two passes to ensure correct format resolution:
///
/// 1. **Pass 1 (StoryData discovery)**: Read all files and search for a
///    `StoryData` passage. The first `StoryData` found determines the story
///    format. This pass is lightweight — it only extracts the `format` field
///    from the JSON body, it does not parse the full document.
///
/// 2. **Pass 2 (Full parse)**: Now that the correct format is resolved,
///    parse every file with the appropriate format plugin. This guarantees
///    that Harlowe files are parsed with Harlowe, SugarCube with SugarCube,
///    etc. — even when `StoryData` appears in a later file.
///
/// If no `.tw`/`.twee` files are found, a `knot/noTweeFiles` notification
/// is sent to the client so it can prompt the user to initialize a project.
pub(crate) async fn index_workspace(
    inner: &tokio::sync::RwLock<crate::state::ServerStateInner>,
    client: &tower_lsp::Client,
) -> Result<(), String> {
    let root_uri = {
        let inner = inner.read().await;
        inner.workspace.root_uri.clone()
    };

    let root_path = root_uri
        .to_file_path()
        .map_err(|_| "Workspace root is not a file:// URI".to_string())?;

    // Get ignore patterns from knot.json config
    let ignore_patterns: Vec<String> = {
        let inner = inner.read().await;
        inner.workspace.config.ignore.clone()
    };

    // Collect all .tw/.twee/.js files using walkdir, filtering against ignore patterns.
    //
    // .js files are included because Tweego bundles them from the source
    // directory as <script> tags in the compiled HTML. Knot parses them as
    // synthetic script passages — see `parse_script_file` in parse_pipeline.rs.
    let twee_files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root_path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            let ext = entry.path().extension().and_then(|e| e.to_str());
            ext == Some("tw") || ext == Some("twee") || ext == Some("js")
        })
        .filter(|entry| {
            // Apply knot.json ignore patterns
            if ignore_patterns.is_empty() {
                return true;
            }
            let path_str = entry.path().to_string_lossy();
            // Normalize to forward slashes for consistent matching
            let normalized = path_str.replace('\\', "/");
            let relative = normalized.strip_prefix(&root_path.to_string_lossy().replace('\\', "/"))
                .unwrap_or(&normalized);
            let relative = relative.trim_start_matches('/');
            // Simple glob-style matching: each ignore pattern is checked against
            // the relative path. Supports basic glob patterns:
            // - "node_modules" matches any path component
            // - "*.tmp" matches file extension
            // - "build/**" matches directory and contents
            for pattern in &ignore_patterns {
                if pattern.starts_with('*') {
                    // Extension pattern like "*.tmp"
                    if relative.ends_with(&pattern[1..]) {
                        return false;
                    }
                } else if pattern.ends_with("/**") {
                    // Directory pattern like "build/**"
                    let dir_name = &pattern[..pattern.len() - 3];
                    if relative.starts_with(dir_name) {
                        return false;
                    }
                } else {
                    // Simple name match against any path component
                    for component in relative.split('/') {
                        if component == pattern {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .map(|entry| entry.into_path())
        .collect();

    let total_files = twee_files.len() as u32;
    if total_files == 0 {
        // Notify the client that no Twee files were found, so it can
        // suggest initializing a project skeleton.
        client
            .send_notification::<KnotNoTweeFilesNotification>(KnotNoTweeFiles {
                workspace_uri: root_uri.to_string(),
            })
            .await;
        client
            .log_message(
                MessageType::INFO,
                "No .tw/.twee files found in workspace. Use 'Knot: Initialize Project' to create one.",
            )
            .await;
        return Ok(());
    }

    client
        .log_message(
            MessageType::INFO,
            format!("Indexing {} Twee files…", total_files),
        )
        .await;

    // Send initial progress notification
    send_index_progress(client, total_files, 0).await;

    // ── Pass 1: StoryData discovery ────────────────────────────────────
    // Read all files and look for a StoryData passage to resolve the correct
    // story format BEFORE parsing. This ensures that files are always parsed
    // with the correct format plugin, regardless of what order they appear in
    // the file system.
    client
        .log_message(MessageType::INFO, "Pass 1: Scanning for StoryData…")
        .await;

    let mut discovered_metadata: Option<StoryMetadata> = None;
    let mut file_texts: HashMap<Url, String> = HashMap::new();

    for file_path in &twee_files {
        if let Ok(text) = tokio::fs::read_to_string(file_path).await {
            if let Ok(uri) = Url::from_file_path(file_path) {
                file_texts.insert(uri.clone(), text.clone());

                // Quick scan for StoryData passage in this file
                if discovered_metadata.is_none() {
                    if let Some(meta) = quick_scan_story_data(&text) {
                        tracing::info!(
                            "StoryData found in {}: format={:?}",
                            file_path.display(),
                            meta.format
                        );
                        discovered_metadata = Some(meta);
                    }
                }
            }
        }
    }

    // Apply the discovered format (or keep knot.json override / default)
    {
        let mut inner = inner.write().await;
        if let Some(meta) = discovered_metadata {
            // Always update metadata from freshly discovered StoryData.
            // The knot.json config.format override is handled separately
            // by resolve_format() (Priority 1 = config, Priority 2 = StoryData).
            inner.workspace.metadata = Some(meta);
        }
    }

    let resolved_format = {
        let inner = inner.read().await;
        inner.workspace.resolve_format()
    };

    tracing::info!("Resolved story format: {:?}", resolved_format);
    client
        .log_message(
            MessageType::INFO,
            format!("Pass 1 complete: format = {}", resolved_format),
        )
        .await;

    // ── Pass 2: Full parse with correct format ─────────────────────────
    client
        .log_message(MessageType::INFO, "Pass 2: Parsing files…")
        .await;

    let mut parsed_count: u32 = 0;

    for file_path in &twee_files {
        let uri = match Url::from_file_path(file_path) {
            Ok(u) => u,
            Err(_) => continue,
        };

        let text = match file_texts.get(&uri) {
            Some(t) => t.clone(),
            None => continue,
        };

        let mut inner = inner.write().await;
        // Use the resolved format from Pass 1 for ALL files
        let format = resolved_format.clone();

        inner.open_documents.insert(uri.clone(), text.clone());
        // Store version 0 for indexed files so semantic_tokens_full
        // returns a consistent result_id. Without this, indexed files
        // get result_id=None while did_open files get result_id=Some("N"),
        // which can cause VS Code's delta token caching to behave
        // inconsistently across the indexing → did_open transition.
        inner.doc_versions.insert(uri.clone(), 0);

        let (doc, parse_result) = parse_with_format_plugin(
            &mut inner.format_registry,
            &uri,
            &text,
            format,
            0, // version 0 for indexed files
        );

        // Store format diagnostics
        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostic_groups);

        // Cache semantic tokens at parse time so semantic_tokens_full
        // never needs to re-parse
        inner.semantic_tokens.insert(uri.clone(), parse_result.token_groups);

        // Check for StoryData (may update metadata with start passage, ifid, etc.)
        extract_and_set_metadata(&mut inner.workspace, &doc, &text);

        inner.workspace.insert_document(doc);
        drop(inner);

        // Yield to the tokio runtime between files so other tasks
        // (did_open, did_change, etc.) can acquire the lock.
        tokio::task::yield_now().await;

        parsed_count += 1;

        // Send progress every 10 files or on the last file
        if parsed_count.is_multiple_of(10) || parsed_count == total_files {
            send_index_progress(client, total_files, parsed_count).await;
        }
    }

    // After all files are loaded, rebuild the graph and run analysis
    let format;
    let doc_uris: Vec<String>;
    let diagnostics;
    let open_docs;
    let fmt_diags;
    let config;
    {
        let mut inner_guard = inner.write().await;
        format = inner_guard.workspace.resolve_format();
        inner_guard.workspace.graph = rebuild_graph(&inner_guard.workspace, &inner_guard.format_registry, format.clone());
        inner_guard.workspace.mark_indexed();

        // Notify the client of the detected format so it can switch language IDs
        doc_uris = inner_guard.open_documents.keys().map(|u| u.to_string()).collect();

        diagnostics = analyze_with_format_vars(&inner_guard.workspace, &inner_guard.format_registry);
        open_docs = inner_guard.open_documents.clone();
        fmt_diags = inner_guard.format_diagnostics.clone();
        config = inner_guard.workspace.config.clone();
    }

    // Re-acquire read lock for publish — it needs workspace for variable related info
    {
        let inner_guard = inner.read().await;
        let format = inner_guard.workspace.resolve_format();
        let plugin = inner_guard.format_registry.get(&format);
        let sigils: Vec<char> = plugin.as_ref()
            .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
            .unwrap_or_default();
        publish_all_diagnostics(client, &diagnostics, &fmt_diags, &open_docs, &inner_guard.workspace, &config, &sigils).await;
    }

    // Always send formatDetected after initial indexing so the client
    // can set language IDs even when the format hasn't "changed" (it
    // may be the first time the client hears about it).
    send_format_detected(client, format, doc_uris, root_uri.to_string()).await;

    // After indexing completes, request the client to refresh semantic
    // tokens for all visible editors. This is critical for the scenario
    // where files were already open in VS Code before the extension
    // restarted: the server re-indexed them during the initial pass,
    // but VS Code still holds stale (empty) tokens from before the
    // restart. The standard `workspace/semanticTokens/refresh` request
    // tells VS Code to re-request `textDocument/semanticTokens/full`
    // for every visible document.
    send_workspace_semantic_token_refresh(client).await;

    Ok(())
}

/// Quick-scan a file's text for a StoryData passage and extract the format.
///
/// This is a lightweight scan that only looks for the `:: StoryData` header
/// and parses the JSON body to extract the `format` field. It does NOT
/// perform a full parse with the format plugin — that happens in Pass 2.
///
/// Returns `Some(StoryMetadata)` if a StoryData passage was found, or
/// `None` if the file doesn't contain one.
fn quick_scan_story_data(text: &str) -> Option<StoryMetadata> {
    // Find the StoryData passage header
    let mut story_data_start: Option<usize> = None;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("::") {
            let name = trimmed[2..].trim();
            // Strip tags: "StoryData [tag]" → "StoryData"
            let name = if let Some(bracket) = name.find('[') {
                name[..bracket].trim()
            } else {
                name
            };
            if name == "StoryData" {
                // Body starts after this line
                let header_end = text.lines().take(i + 1).map(|l| l.len() + 1).sum::<usize>();
                story_data_start = Some(header_end);
                break;
            }
        }
    }

    let body_start = story_data_start?;
    let body = &text[body_start.min(text.len())..];

    // Find the next passage header (if any) to limit the body
    let body_end = body.find("\n::").unwrap_or(body.len());
    let body = &body[..body_end];

    parse_story_data_json(body)
}

/// Send a `knot/indexProgress` notification to the client.
async fn send_index_progress(client: &tower_lsp::Client, total_files: u32, parsed_files: u32) {
    let progress = KnotIndexProgress {
        total_files,
        parsed_files,
    };
    client
        .send_notification::<KnotIndexProgressNotification>(progress)
        .await;
}

/// Send a `knot/formatDetected` notification to the client.
///
/// Called when the story format is first detected or changes (e.g., after
/// StoryData is found). The client uses this to switch document language IDs,
/// which activates the correct TextMate grammar for the detected format.
pub(crate) async fn send_format_detected(
    client: &tower_lsp::Client,
    format: StoryFormat,
    document_uris: Vec<String>,
    workspace_uri: String,
) {
    tracing::info!(
        format = %format,
        document_count = document_uris.len(),
        "Sending knot/formatDetected notification"
    );
    client
        .send_notification::<FormatDetectedNotification>(FormatDetectedParams {
            format: format.to_string(),
            document_uris,
            workspace_uri,
        })
        .await;
}


/// Send the standard LSP `workspace/semanticTokens/refresh` request.
///
/// This is the official server-to-client request defined in LSP 3.16+
/// that asks the client to re-request semantic tokens for all visible
/// documents. `vscode-languageclient` handles this automatically — it
/// re-issues `textDocument/semanticTokens/full` for every open editor.
///
/// This is the primary mechanism for forcing a semantic token refresh
/// after server-side state changes that affect highlighting (e.g., after
/// initial workspace indexing completes, or when cross-file link
/// resolution changes).
async fn send_workspace_semantic_token_refresh(client: &tower_lsp::Client) {
    use crate::lsp_ext::WorkspaceSemanticTokensRefreshRequest;

    match client.send_request::<WorkspaceSemanticTokensRefreshRequest>(()).await {
        Ok(()) => {
            tracing::debug!("workspace/semanticTokens/refresh accepted by client");
        }
        Err(e) => {
            // Not fatal — older clients may not support this request.
            // The custom knot/refreshSemanticTokens notification serves
            // as a fallback.
            tracing::debug!(
                "workspace/semanticTokens/refresh failed (client may not support it): {}",
                e
            );
        }
    }
}
