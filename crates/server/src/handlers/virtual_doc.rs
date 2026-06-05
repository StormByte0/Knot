//! Custom LSP request handlers for virtual documents (knot/virtualDoc, knot/jsDiagnostics).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_core::virtual_doc::SourceTextProvider as _;
use std::collections::HashMap;
use url;

impl ServerState {
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

        // VirtualDocManager is empty — return empty response.
        // The old FormatPlugin fallback path has been removed; the
        // VirtualDocManager is now the sole source of virtual doc content.
        Ok(KnotVirtualDocResponse {
            content: String::new(),
            line_map: Vec::new(),
            passage_names: Vec::new(),
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
            let byte_range = helpers::line_char_to_byte_range(
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

            let range = helpers::byte_range_to_lsp_range_owned(tw_source, source_location.byte_range.clone());

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
            let graph_diagnostics = helpers::analyze_with_format_vars(workspace, &inner.format_registry);
            let fmt_diags: std::collections::HashMap<url::Url, Vec<knot_formats::plugin::FormatDiagnostic>> = std::collections::HashMap::new();

            let format = workspace.resolve_format();
            let plugin = inner.format_registry.get(&format);
            let sigils: Vec<char> = plugin.as_ref()
                .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
                .unwrap_or_default();

            helpers::publish_all_diagnostics(
                &self.client,
                &graph_diagnostics,
                &fmt_diags,
                &inner.js_diagnostics,
                &inner.open_documents,
                workspace,
                config,
                &sigils,
            ).await;
        }

        Ok(KnotJsDiagnosticsResponse { processed })
    }
}
