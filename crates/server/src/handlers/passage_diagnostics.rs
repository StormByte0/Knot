//! Custom LSP request handler for passage-level diagnostics (knot/passageDiagnostics).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;

impl ServerState {
    /// `knot/passageDiagnostics` — return diagnostic information about a specific passage.
    ///
    /// Returns linter issues, link connections, passage metadata, and
    /// variable references (reads/writes with line numbers) resolved from
    /// the format plugin.
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
                    params.workspace_uri,
                    root
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
                    temporary_variables: Vec::new(),
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
        let is_reachable = !unreachable_diags
            .iter()
            .any(|d| d.passage_name == params.passage_name);

        // Outgoing links — skip dynamic links with empty targets.
        // These include single-arg <<link "Display">> (click handler),
        // zero-arg <<return>>/<<back>> (history-based), and any link
        // with is_dynamic=true and no fixed target. They're not broken
        // links — they just don't have a fixed passage target.
        let outgoing_links: Vec<KnotPassageLink> = passage
            .links
            .iter()
            .filter(|l| !l.target.is_empty())
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

        // ── Variable references (from format plugin) ────────────────────
        // Extract variable accesses filtered for this passage via the format
        // plugin. This gives us exact line numbers for each read/write,
        // enabling "go to reference" from the passage diagnostics panel.
        let variable_references = super::variables::build_passage_variable_references(
            workspace,
            &inner.format_registry,
            &inner.open_documents,
            &params.passage_name,
        );

        // ── Temporary variables (passage-scoped, `_var` in SugarCube) ──
        // These don't belong in the workspace-wide variable tracker
        // (which only sees persistent `$` vars) — they live and die with
        // the passage. We surface them here as a small infographics
        // section: per-name read/write counts plus clickable line refs.
        // Formats without passage-scoped temps return an empty Vec and
        // the client simply hides the section.
        let temporary_variables = super::variables::build_passage_temporary_variables(
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
            temporary_variables,
        })
    }
}
