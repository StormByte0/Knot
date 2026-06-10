//! Server state management.
//!
//! The server state holds all mutable workspace data behind an async RwLock
//! so that LSP handlers can concurrently read or exclusively write the state.

use knot_core::editing::DebounceTimer;
use knot_core::Workspace;
use knot_formats::plugin::{FormatDiagnostic, FormatRegistry, SemanticToken, SourceTextProvider};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tower_lsp::Client;
use url::Url;

// ---------------------------------------------------------------------------
// SourceTextProvider — newtype wrapper for knot_formats' trait
// ---------------------------------------------------------------------------

/// Newtype wrapper that borrows the server's `open_documents` cache so we can
/// implement `SourceTextProvider` (defined in `knot-formats`) for it.
///
/// We cannot implement a foreign trait for a foreign type (`HashMap<Url, String>`),
/// so we wrap a reference in a local newtype. The wrapper is cheap — it only
/// stores a reference and is created on the stack at each call site that needs
/// to pass the document cache as a `&dyn SourceTextProvider`.
pub struct DocumentCache<'a>(pub &'a HashMap<Url, String>);

impl<'a> SourceTextProvider for DocumentCache<'a> {
    fn get_source_text(&self, file_uri: &str) -> Option<&str> {
        if let Ok(uri) = Url::parse(file_uri) {
            self.0.get(&uri).map(|s| s.as_str())
        } else {
            None
        }
    }
}

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
    /// URIs of documents currently open in the VS Code editor.
    /// This tracks ONLY files with an active text editor — used to determine
    /// whether a file change on disk should be ignored (did_change handles it)
    /// or re-read from disk. This is intentionally separate from `open_documents`
    /// which acts as a general text cache for ALL known files.
    pub editor_open_docs: HashSet<Url>,
    /// Cache of document text for ALL known files (URI → current text).
    /// This includes both editor-open files and files read from disk during
    /// workspace indexing. Used for position lookups, hover text, diagnostics, etc.
    pub open_documents: HashMap<Url, String>,
    /// Per-document format plugin diagnostics (URI → diagnostics).
    /// These are separate from graph diagnostics because they are produced
    /// by the format parser during parsing, not by graph analysis.
    pub format_diagnostics: HashMap<Url, Vec<FormatDiagnostic>>,
    /// Per-document version tracking (URI → LSP version number).
    /// The LSP version is monotonically increasing and comes from the client.
    /// This is stored separately from `Document.version` because re-parsing
    /// a document (via `parse_with_format_plugin`) creates a new `Document`
    /// that resets the version. Keeping the version here preserves it across
    /// re-parses so that `did_change` can always use the authoritative client
    /// version.
    pub doc_versions: HashMap<Url, i32>,
    /// Semantic token cache (URI → format-plugin tokens).
    ///
    /// Tokens are stored at parse time so that `semantic_tokens_full` never
    /// needs to re-parse. This is critical for avoiding deadlock when
    /// FormatPluginMut (Phase 4) requires the write lock for parsing — if
    /// `semantic_tokens_full` had to parse, it would need the write lock
    /// while already holding the read lock.
    ///
    /// Tokens are NOT removed on `did_close` — preserving them is important
    /// for the format-switch cascade (Phase 3), where didClose+didOpen pairs
    /// can temporarily remove documents from the cache. Stale tokens are
    /// better than no tokens because VS Code will re-request after a refresh.
    pub semantic_tokens: HashMap<Url, Vec<SemanticToken>>,
    /// Flag indicating that a format switch cascade is in progress.
    ///
    /// When set to `true`, `did_open` and `did_change` should suppress
    /// semantic token refreshes. The cascade completes when the extension
    /// sends `knot/formatSwitchComplete`, at which point ONE unified
    /// refresh is sent. This prevents the O(N²) token request flood that
    /// would otherwise occur when N files have their language IDs switched
    /// in quick succession.
    pub format_switch_in_progress: bool,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Thread-safe server state.
///
/// The `Client` handle is stored outside the lock because it is `Send + Sync`
/// and does not require interior mutability. All other mutable state lives
/// inside `inner`, protected by a `tokio::sync::RwLock` wrapped in `Arc`
/// so that the `initialized` handler can clone it into a spawned task.
pub struct ServerState {
    /// The LSP client handle for sending notifications.
    pub client: Client,
    /// Mutable inner state behind an async read-write lock.
    /// Wrapped in `Arc` so that `tokio::spawn` can clone it into `'static`
    /// tasks (e.g., the indexing task spawned in `initialized`).
    pub inner: Arc<RwLock<ServerStateInner>>,
    /// Shutdown guard — set to `true` when `shutdown()` is called so that
    /// in-flight handlers can short-circuit instead of writing to a destroyed
    /// transport stream.  Reset to `false` on `initialize()`.
    pub shutting_down: AtomicBool,
    /// Notification primitive for the `knot/clientReady` handshake.
    ///
    /// The `initialized` handler spawns an indexing task that waits on this
    /// `Notify` before starting workspace indexing. The extension sends
    /// `knot/clientReady` after all notification handlers are registered,
    /// preventing the race where `formatDetected` arrives before handlers
    /// are ready.
    pub client_ready: Arc<Notify>,
    /// Flag for debouncing `workspace/semanticTokens/refresh` requests.
    ///
    /// The `compare_exchange` on this flag ensures only ONE debounce timer
    /// is active at a time. Subsequent calls within 150ms are coalesced.
    /// This prevents the O(N²) token request flood during format switch
    /// cascades.
    pub semantic_refresh_pending: Arc<AtomicBool>,
}

impl ServerState {
    /// Create a new server state from a tower-lsp client handle.
    pub fn new(client: Client) -> Self {
        let placeholder_uri = Url::parse("file:///").unwrap_or_else(|e| {
            tracing::error!("Failed to parse placeholder URI: {e}");
            // This should never happen with a valid constant, but provide a safe fallback
            Url::parse("file:///").unwrap()
        });
        let workspace = Workspace::new(placeholder_uri);

        Self {
            client,
            inner: Arc::new(RwLock::new(ServerStateInner {
                workspace,
                format_registry: FormatRegistry::with_defaults(),
                debounce: DebounceTimer::new(),
                editor_open_docs: HashSet::new(),
                open_documents: HashMap::new(),
                format_diagnostics: HashMap::new(),
                doc_versions: HashMap::new(),
                semantic_tokens: HashMap::new(),
                format_switch_in_progress: false,
            })),
            shutting_down: AtomicBool::new(false),
            client_ready: Arc::new(Notify::new()),
            semantic_refresh_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Schedule a debounced `workspace/semanticTokens/refresh` request.
    ///
    /// Uses `compare_exchange` to ensure only ONE debounce timer is active
    /// at a time. Subsequent calls within 150ms are coalesced. This
    /// prevents the O(N²) token request flood during format switch cascades.
    pub async fn schedule_semantic_token_refresh(&self) {
        if self.semantic_refresh_pending.compare_exchange(
            false, true, Ordering::Relaxed, Ordering::Relaxed
        ).is_err() {
            return; // refresh already pending
        }

        let pending = self.semantic_refresh_pending.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            pending.store(false, Ordering::Relaxed);
            use crate::lsp_ext::WorkspaceSemanticTokensRefreshRequest;
            let _ = client.send_request::<WorkspaceSemanticTokensRefreshRequest>(()).await;
        });
    }
}
