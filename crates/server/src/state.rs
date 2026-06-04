//! Server state management.
//!
//! The server state holds all mutable workspace data behind an async RwLock
//! so that LSP handlers can concurrently read or exclusively write the state.
//!
//! ## Virtual Document Architecture
//!
//! The server state includes a `VirtualDocManager` from `knot_core` and a
//! format-specific `VirtualDocAdapter`. The manager owns the virtual doc
//! lifecycle (assembly, indexing, JS LSP integration), while the adapter
//! provides format-specific content (what goes in, how to interpret what
//! comes out). This inverts the previous architecture where the format
//! plugin owned everything and core owned nothing.

use knot_core::editing::DebounceTimer;
use knot_core::virtual_doc::{VirtualDocAdapter, VirtualDocManager};
use knot_core::Workspace;
use knot_formats::plugin::{FormatDiagnostic, FormatRegistry, SourceTextProvider};
use lsp_types::Diagnostic;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
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
// CoreSourceTextProvider — newtype wrapper for knot_core's trait
// ---------------------------------------------------------------------------

/// Newtype wrapper that borrows the server's `open_documents` cache to
/// implement `knot_core::virtual_doc::SourceTextProvider`.
///
/// This is the same data source as `DocumentCache` but implements the
/// core crate's `SourceTextProvider` trait instead of the formats crate's
/// version. Both traits have the same signature; they're separate types
/// because they're defined in different crates (Rust's orphan rules).
pub struct CoreDocumentCache<'a>(pub &'a HashMap<Url, String>);

impl<'a> knot_core::virtual_doc::SourceTextProvider for CoreDocumentCache<'a> {
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

    // ── Virtual document architecture (new) ───────────────────────────

    /// Core's virtual document manager. Owns the JS virtual doc lifecycle:
    /// monolithic content, passage entry index, JS LSP integration state.
    ///
    /// The manager is format-agnostic — it drives the pipeline, and the
    /// adapter provides format-specific content.
    pub virtual_doc_manager: VirtualDocManager,

    /// The format-specific virtual doc adapter for the active story format.
    /// This is set when the format is detected (during workspace indexing)
    /// and updated if the format changes.
    ///
    /// Initially `None` until a format is detected. Once set, it persists
    /// across workspace rebuilds (the adapter's `clear_state()` is called
    /// instead of creating a new adapter).
    pub virtual_doc_adapter: Option<Box<dyn VirtualDocAdapter>>,

    /// JS-relayed diagnostics from the client's built-in JS service,
    /// reverse-mapped to .tw source positions. Keyed by .tw file URI.
    ///
    /// These are maintained separately from graph/format diagnostics and
    /// merged when publishing via `textDocument/publishDiagnostics`. This
    /// ensures a single diagnostic source per .tw file — the server —
    /// avoiding conflicts with a client-side DiagnosticCollection.
    pub js_diagnostics: HashMap<Url, Vec<Diagnostic>>,
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
    /// Shutdown guard — set to `true` when `shutdown()` is called so that
    /// in-flight handlers can short-circuit instead of writing to a destroyed
    /// transport stream.  Reset to `false` on `initialize()`.
    pub shutting_down: AtomicBool,
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
            inner: RwLock::new(ServerStateInner {
                workspace,
                format_registry: FormatRegistry::with_defaults(),
                debounce: DebounceTimer::new(),
                editor_open_docs: HashSet::new(),
                open_documents: HashMap::new(),
                format_diagnostics: HashMap::new(),
                doc_versions: HashMap::new(),
                virtual_doc_manager: VirtualDocManager::new(),
                virtual_doc_adapter: None,
                js_diagnostics: HashMap::new(),
            }),
            shutting_down: AtomicBool::new(false),
        }
    }
}
