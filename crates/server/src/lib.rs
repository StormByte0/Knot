//! Knot Language Server
//!
//! The Rust-based language server for the Knot IDE. Communicates with VS Code
//! via LSP over stdio using tower-lsp.

pub mod handlers;
pub mod lsp_ext;
pub mod state;

use state::ServerState;
use tower_lsp::{LspService, Server};

/// The Knot Language Server.
pub struct KnotServer;

impl KnotServer {
    /// Create a new server instance.
    pub fn new() -> Self {
        Self
    }

    /// Run the language server over stdio.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let (service, socket) = LspService::build(ServerState::new)
            .custom_method("knot/graph", ServerState::knot_graph)
            .custom_method("knot/build", ServerState::knot_build)
            .custom_method("knot/play", ServerState::knot_play)
            .custom_method("knot/variableFlow", ServerState::knot_variable_flow)
            .custom_method("knot/passageDiagnostics", ServerState::knot_passage_diagnostics)
            .custom_method("knot/profile", ServerState::knot_profile)
            .custom_method("knot/compilerDetect", ServerState::knot_compiler_detect)
            .custom_method("knot/watchVariables", ServerState::knot_watch_variables)
            .custom_method("knot/generateIfid", ServerState::knot_generate_ifid)
            .custom_method("knot/reindexWorkspace", ServerState::knot_reindex_workspace)
            .custom_method("knot/updatePositions", ServerState::knot_update_positions)
            .custom_method("knot/clientReady", ServerState::knot_client_ready)
            .custom_method("knot/formatSwitchComplete", ServerState::knot_format_switch_complete)
            .custom_method("knot/formats/list", ServerState::knot_formats_list)
            .custom_method("knot/formats/refresh", ServerState::knot_formats_refresh)
            .finish();

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        Server::new(stdin, stdout, socket).serve(service).await;

        Ok(())
    }
}

impl Default for KnotServer {
    fn default() -> Self {
        Self::new()
    }
}
