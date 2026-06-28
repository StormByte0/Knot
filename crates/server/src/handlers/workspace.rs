//! Custom LSP request handlers for workspace management (knot/reindexWorkspace, knot/generateIfid).

use crate::handlers::helpers;
use crate::lsp_ext::*;
use crate::state::ServerState;

impl ServerState {
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
                    params.workspace_uri,
                    root
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
                    params.workspace_uri,
                    root
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
