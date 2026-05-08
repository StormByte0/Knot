//! Server state management.
//!
//! The server state holds all mutable workspace data behind an async RwLock
//! so that LSP handlers can concurrently read or exclusively write the state.

use knot_core::editing::DebounceTimer;
use knot_core::Workspace;
use knot_formats::plugin::{FormatDiagnostic, FormatRegistry};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tower_lsp::Client;
use url::Url;

// ---------------------------------------------------------------------------
// Inner mutable state
// ---------------------------------------------------------------------------

/// The mutable portion of the server state, protected by an async RwLock.
pub struct ServerStateInner {
    /// The workspace (single Twine project).
    pub workspace: Workspace,
    /// The format plugin registry.
    pub format_registry: FormatRegistry,
    /// Debounce timer for edit events.
    pub debounce: DebounceTimer,
    /// Cache of open document text (URI → current text).
    pub open_documents: HashMap<Url, String>,
    /// Per-document format plugin diagnostics (URI → diagnostics).
    /// These are separate from graph diagnostics because they are produced
    /// by the format parser during parsing, not by graph analysis.
    pub format_diagnostics: HashMap<Url, Vec<FormatDiagnostic>>,
    /// Debug breakpoints — set of passage names where breakpoints are active.
    pub breakpoints: Vec<String>,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Thread-safe server state.
///
/// The `Client` handle is stored outside the lock because it is `Send + Sync`
/// and does not require interior mutability. All other mutable state lives
/// inside `inner`, protected by a `tokio::sync::RwLock`.
pub struct ServerState {
    /// The LSP client handle for sending notifications.
    pub client: Client,
    /// Mutable inner state behind an async read-write lock.
    pub inner: RwLock<ServerStateInner>,
}

impl ServerState {
    /// Create a new server state from a tower-lsp client handle.
    pub fn new(client: Client) -> Self {
        let placeholder_uri = Url::parse("file:///").unwrap();
        let workspace = Workspace::new(placeholder_uri);

        Self {
            client,
            inner: RwLock::new(ServerStateInner {
                workspace,
                format_registry: FormatRegistry::with_defaults(),
                debounce: DebounceTimer::new(),
                open_documents: HashMap::new(),
                format_diagnostics: HashMap::new(),
                breakpoints: Vec::new(),
            }),
        }
    }
}
