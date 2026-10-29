//! Document synchronization handlers: did_open, did_change, did_close,
//! did_save, did_change_configuration, did_change_watched_files.

use crate::handlers::helpers;
use crate::state::{ServerState, ServerStateInner};
use lsp_types::*;
use url::Url;

pub(crate) async fn did_open(state: &ServerState, params: DidOpenTextDocumentParams) {
    // Short-circuit if the server is shutting down
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
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
            Some(path) => inner
                .open_documents
                .keys()
                .filter(|k| **k != uri)
                .filter(|k| k.to_file_path().is_ok_and(|p| p == path))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    };
    for key in &stale_keys {
        tracing::debug!(
            "Removing stale URI-equivalent entry: {} (canonical: {})",
            key,
            uri
        );
        inner.open_documents.remove(key);
        inner.format_diagnostics.remove(key);
        inner.semantic_tokens.remove(key);
    }

    inner.editor_open_docs.insert(uri.clone());
    inner.open_documents.insert(uri.clone(), text.clone());

    // If workspace indexing is still in progress, do a lightweight insert
    // only — the indexing pass will parse with the correct format, build
    // the graph, and publish diagnostics once all files are loaded.
    //
    // CRITICAL: We must NOT parse here during indexing because
    // resolve_format() returns Core (no StoryData discovered yet),
    // producing minimal tokens (headers + links only) instead of the
    // rich format-specific tokens (macros, variables, keywords, etc.).
    // This causes a visible highlighting inconsistency: files open
    // during indexing get Core tokens (sparse), then indexing overwrites
    // them with format-specific tokens (rich), producing a visible
    // flash/transition. By deferring ALL parsing to the indexing
    // pipeline, we avoid the Core → SugarCube token mismatch entirely.
    if indexing_in_progress {
        tracing::debug!(
            "did_open: deferring parse — workspace indexing in progress (format not yet resolved)"
        );
        drop(inner);
        return;
    }

    // After indexing, the format is frozen. Parse with the resolved
    // format plugin and cache the results.
    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&mut inner.format_registry, &uri, &text, format, version);

    // Store format diagnostics for this document
    inner.format_diagnostics.insert(
        uri.clone(),
        helpers::diagnostic_groups_to_map(parse_result.diagnostic_groups.clone()),
    );

    // Cache semantic tokens at parse time so semantic_tokens_full
    // never needs to re-parse (critical for avoiding deadlock with
    // FormatPluginMut in Phase 4).
    inner.semantic_tokens.insert(
        uri.clone(),
        helpers::token_groups_to_map(parse_result.token_groups.clone()),
    );

    // Insert the parsed document into the workspace — but ONLY if it's
    // inside the workspace root.
    //
    // Files opened from outside the workspace (e.g. a reference `.twee`
    // file from another project) are still parsed and cached for
    // per-file LSP features (syntax highlighting, hover, completion
    // within that file), but their passages do NOT enter the workspace
    // graph. This prevents an unrelated file from polluting the project
    // with phantom passages, which would cause spurious "duplicate
    // passage name" diagnostics and stale broken-link reports (Bug #4).
    //
    // NOTE: StoryData metadata extraction does NOT belong here. Format
    // identification is the sole responsibility of the indexing pipeline
    // (index_workspace), which scans ALL workspace files for StoryData
    // regardless of whether they are open. did_open simply parses with
    // the already-resolved format and inserts the document. StoryData
    // passages are kept as blocks for AST token highlighting only — any
    // changes to StoryData content should prompt a server restart.
    let in_workspace = helpers::is_uri_in_workspace(&uri, &inner.workspace.root_uri);
    if in_workspace {
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

        // The format is resolved by the indexing pipeline — did_open never
        // changes it. Rebuild the graph with the current (frozen) format.
        //
        // Wrap graph rebuild and analysis in catch_unwind to prevent panics
        // in format plugin code (e.g., stale arena pointers in variable tree
        // traversal) from crashing the server. This mirrors the catch_unwind
        // already applied to parse_with_format_plugin.
        let format = inner.workspace.resolve_format();
        let graph_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format)
        }));
        match &graph_result {
            Ok(graph) => inner.workspace.graph = graph.clone(),
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!("did_open: rebuild_graph panicked for {}: {}", uri, msg);
                // Keep the existing graph — it's stale but better than crashing
            }
        }
    } else {
        tracing::info!(
            file = %uri,
            "did_open: file is outside workspace root — parsed for per-file features only, not added to graph"
        );
    }

    // Release write lock before analysis — same two-phase pattern as did_change
    drop(inner);

    // Read-lock phase: analysis + build LSP diagnostics (sync).
    // Task 3: consolidated — no more separate phase 3 lock for publishing.
    let prebuilt_diagnostics = {
        let inner = state.inner.read().await;
        let diagnostics = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry)
        })) {
            Ok(d) => d,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!(
                    "did_open: analyze_with_format_vars panicked for {}: {}",
                    uri,
                    msg
                );
                Vec::new() // Return empty diagnostics rather than crashing
            }
        };
        let sigils = helpers::compute_sigils(&inner.format_registry, &inner.workspace);
        helpers::stamp_doc_versions(
            &inner,
            helpers::build_all_lsp_diagnostics(
                &diagnostics,
                &inner.format_diagnostics,
                &inner.open_documents,
                &inner.workspace,
                &inner.workspace.config,
                &sigils,
            ),
        )
    }; // ← read lock dropped

    // Send diagnostics to the client (lock-free).
    helpers::send_lsp_diagnostics(&state.client, prebuilt_diagnostics).await;

    // Schedule a debounced semantic token refresh. Format is frozen
    // after indexing — no format switch cascades are possible.
    state.schedule_semantic_token_refresh().await;
}

pub(crate) async fn did_change(state: &ServerState, params: DidChangeTextDocumentParams) {
    // Short-circuit if the server is shutting down
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let version = params.text_document.version;

    tracing::debug!("did_change: {} (v{})", uri, version);

    let mut inner = state.inner.write().await;

    // ── Phase 1: synchronous core — classify, dispatch, cache, graph surgery.
    // Extracted into `did_change_phase1` so tests can call it directly without
    // a `tower_lsp::Client` or async runtime.
    let phase1_result = did_change_phase1(&mut inner, uri.clone(), version, params.content_changes);

    // ── Bug #2: StoryData modification detection ──────────────────────
    //
    // If the edited file contains a StoryData passage, re-parse its JSON
    // body and compare to the cached workspace.metadata. If the format
    // (or other critical fields) changed, we must trigger a full
    // workspace re-index — otherwise the rest of the files are still
    // parsed with the OLD format plugin, producing stale tokens,
    // diagnostics, and graph edges.
    //
    // We do this BEFORE dropping the write lock so we can decide whether
    // to reindex synchronously. The reindex itself needs the write lock
    // and the client handle, so we hand off to the existing
    // `knot_reindex_workspace` path by signalling via a flag.
    //
    // To avoid triggering a full reindex on every keystroke while the
    // user is in the middle of typing the StoryData JSON, we ONLY
    // trigger reindex when:
    //   1. The file contains a parseable StoryData passage, AND
    //   2. The parsed format differs from the cached metadata's format.
    //
    // If StoryData is temporarily malformed (e.g. user is mid-edit and
    // the JSON doesn't parse yet), we silently skip — the next successful
    // parse will pick up the change. This matches the indexing pipeline's
    // behavior (quick_scan_story_data returns None on parse failure).
    let needs_reindex_for_format = check_storydata_format_change(&mut inner, &uri);

    // ── Phase 1 complete: all state mutations are done. ──────────────
    // Release the write lock early so that read-lock handlers
    // (codeAction, documentLink, inlayHint, etc.) are not blocked while
    // we run the (read-only) diagnostic analysis below.  This is the key
    // fix for the "Cannot call write after a stream was destroyed" race:
    // the shorter the write-lock hold time, the less likely a restart
    // will catch in-flight handlers still waiting for the lock.

    drop(inner); // ← release write lock

    // If StoryData's format changed, trigger a full re-index now.
    //
    // This runs AFTER dropping the write lock so we don't hold it across
    // the (potentially slow) reindex. `knot_reindex_workspace` acquires
    // its own write lock internally.
    if needs_reindex_for_format {
        tracing::info!(
            file = %uri,
            "did_change: StoryData format changed — triggering full workspace re-index"
        );
        state
            .client
            .log_message(
                tower_lsp::lsp_types::MessageType::INFO,
                "StoryData format changed — re-indexing workspace…",
            )
            .await;
        let reindex_params = crate::lsp_ext::KnotReindexParams {
            workspace_uri: String::new(),
        };
        let _ = state.knot_reindex_workspace(reindex_params).await;
        // knot_reindex_workspace already publishes diagnostics + tokens,
        // so we skip the normal phase 2/3 path below.
        return;
    }

    // ── Phase 2 + 3: single-flight analysis worker (Fix 2) ────────
    //
    // Full-workspace analysis + publish moved OFF the keystroke path into
    // a long-lived worker fed by a latest-wins `watch` channel. Phase 1
    // (above) still runs synchronously on every keystroke, so hover /
    // completion freshness is untouched. The worker coalesces bursts
    // (trailing edge, 500 ms max lag), stamps every publish with the
    // document's authoritative version (so the client can discard a stale
    // push), and skips documents closed while a cycle was pending. See
    // `run_diagnostics_worker` below for the loop and its guarantees.
    state.ping_diagnostics_worker(&uri);

    // ── Semantic token refresh ──────────────────────────────────────────
    //
    // When the edit took the incremental path (WithinPassage), the token
    // structure is unchanged — only positions shifted. VS Code's built-in
    // delta logic handles this correctly, so a debounced refresh is fine
    // (coalesces bursts of keystrokes into one refresh request).
    //
    // When the edit fell back to FULL re-parse (BoundaryCrossing,
    // IncrementalFallback, WholeDocument), the token structure changed:
    // new passages may have been created, passages may have been renamed
    // or deleted, and passage-relative offsets shifted. VS Code's delta
    // logic CANNOT handle these structural changes — it would show stale
    // tokens (mapped to wrong positions) until the refresh arrives.
    //
    // This is the root cause of the "token coloring fumbling" during
    // char-by-char typing of a new passage header (e.g. `::NewPassage`):
    // each keystroke after the second `:` triggers a full re-parse, but
    // the debounced 150ms refresh delays the new token structure from
    // reaching the client. Between keystrokes, VS Code shows tokens
    // from the PREVIOUS full re-parse, mapped via its delta logic to
    // positions that no longer match the new document structure.
    //
    // Fix: send the refresh IMMEDIATELY (no debounce) when the dispatch
    // was a full re-parse. Keep the debounce for incremental edits
    // (WithinPassage) to avoid spamming the client during normal typing.
    match phase1_result.dispatch {
        DidChangeDispatch::Incremental { .. } | DidChangeDispatch::IncrementalPanic { .. } => {
            // Incremental — token structure unchanged, debounced refresh is fine.
            state.schedule_semantic_token_refresh().await;
        }
        DidChangeDispatch::IncrementalFallback
        | DidChangeDispatch::BoundaryCrossing
        | DidChangeDispatch::WholeDocument => {
            // Full re-parse — token structure changed, refresh immediately.
            state.send_semantic_token_refresh_now().await;
        }
    }
}

/// Check if the file at `uri` contains a StoryData passage whose format
/// field (or other critical metadata) differs from the cached
/// `workspace.metadata`.
///
/// Returns `true` if a full workspace re-index is needed to re-parse all
/// files with the new format plugin.
///
/// This is the Bug #2 fix: previously, editing StoryData's `format` field
/// had no effect — the workspace kept using the old format plugin until
/// the server was restarted.
pub(crate) fn check_storydata_format_change(
    inner: &mut crate::state::ServerStateInner,
    uri: &url::Url,
) -> bool {
    // Only consider files inside the workspace root — out-of-workspace
    // files don't contribute to the project's format.
    if !helpers::is_uri_in_workspace(uri, &inner.workspace.root_uri) {
        return false;
    }

    // Get the post-edit text. If we don't have it cached (shouldn't happen
    // after did_change_phase1, but defensive), skip.
    let text = match inner.open_documents.get(uri) {
        Some(t) => t,
        None => return false,
    };

    // Quick-scan for StoryData + parse JSON. Reuse the indexing pipeline's
    // helper so we get identical semantics to initial format detection.
    let new_metadata = match helpers::quick_scan_story_data_public(text) {
        Some(m) => m,
        None => return false, // No StoryData in this file, or JSON didn't parse
    };

    // Compare to cached metadata. If no cached metadata, this is the first
    // time we see StoryData (e.g. user just added it) — reindex to apply
    // the format.
    let cached = match &inner.workspace.metadata {
        Some(m) => m,
        None => {
            tracing::info!(
                file = %uri,
                new_format = ?new_metadata.format,
                "StoryData detected but no cached metadata — reindexing to apply format"
            );
            return true;
        }
    };
    // Copy the format-version out so the immutable `cached` borrow ends
    // before the in-place IFID update below (NLL ends it at its last use).
    let cached_version = cached.format_version.clone();

    // Compare the format field — this is the primary trigger for reindex
    // because changing format requires re-parsing all files with the new
    // plugin.
    if new_metadata.format != cached.format {
        tracing::info!(
            file = %uri,
            old_format = ?cached.format,
            new_format = ?new_metadata.format,
            "StoryData format changed — reindex required"
        );
        return true;
    }

    // Also reindex if the start passage changed — affects graph reachability
    // analysis (unreachable-passage detection uses start as the root).
    if new_metadata.start_passage != cached.start_passage {
        tracing::info!(
            file = %uri,
            old_start = %cached.start_passage,
            new_start = %new_metadata.start_passage,
            "StoryData start passage changed — reindex required for graph analysis"
        );
        return true;
    }

    // IFID changes don't affect parsing, graph analysis, or the versioned
    // catalog — update in place. Format-version changes DO affect the
    // plugin's versioned catalog now (completions, hover, lifecycle
    // diagnostics), so they require a reindex to refresh every passage's
    // cached tokens + diagnostics under the new version.
    if new_metadata.ifid != cached.ifid {
        tracing::debug!(
            file = %uri,
            "StoryData IFID changed — updating metadata in place (no reindex needed)"
        );
        let new_ifid = new_metadata.ifid.clone();
        if let Some(m) = inner.workspace.metadata.as_mut() {
            m.ifid = new_ifid;
        }
    }
    if new_metadata.format_version != cached_version {
        tracing::info!(
            file = %uri,
            old_version = ?cached_version,
            new_version = ?new_metadata.format_version,
            "StoryData format-version changed — reindex required so version-filtered completions/diagnostics update"
        );
        return true;
    }

    false
}

/// Result of [`did_change_phase1`] — the synchronous core of `did_change`.
///
/// Carries the URI (for phase 2 logging) and a summary of what happened
/// (which dispatch path was taken, whether the incremental path panicked).
/// Tests inspect this to assert dispatch decisions.
#[derive(Debug, Clone)]
pub struct DidChangePhase1Result {
    /// The URI of the document that was changed.
    pub uri: url::Url,
    /// The LSP version of the document after the change.
    pub version: i32,
    /// Which dispatch path was taken.
    pub dispatch: DidChangeDispatch,
}

/// Which dispatch path `did_change_phase1` took for an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DidChangeDispatch {
    /// The edit was classified as `WithinPassage` and the incremental
    /// single-passage re-parse succeeded.
    Incremental { passage_name: String },
    /// The edit was classified as `WithinPassage` but the incremental
    /// re-parse panicked. The panic was contained — a scoped `sc-parse`
    /// diagnostic was emitted for the panicking passage and other passages'
    /// cache entries were left untouched.
    IncrementalPanic { passage_name: String },
    /// The edit was classified as `WithinPassage` but the incremental path
    /// returned `None` (classification failure or no plugin). Fell back to
    /// full-file re-parse.
    IncrementalFallback,
    /// The edit was classified as `BoundaryCrossing` (added/removed a `:: `
    /// header line or spanned multiple passages). Full-file re-parse.
    BoundaryCrossing,
    /// The edit was a full-text replacement (`range == None`) or no
    /// pre-edit document existed. Full-file re-parse.
    WholeDocument,
}

/// Phase 1 of `did_change`: the synchronous core that mutates `ServerStateInner`.
///
/// This function:
/// 1. Updates `doc_versions` with the authoritative LSP version.
/// 2. Snapshots pre-edit state (`text_before`, `doc_before`).
/// 3. Applies rope edits via `apply_document_changes`.
/// 4. Calls `classify_edit` to determine `EditImpact`.
/// 5. Dispatches: `WithinPassage` → incremental single-passage re-parse
///    (with `catch_unwind` panic isolation); `BoundaryCrossing`/`WholeDocument`
///    → full-file re-parse.
/// 6. Updates `format_diagnostics` and `semantic_tokens` caches (surgical
///    merge for incremental, wholesale replace for full re-parse — M3).
/// 7. Computes dynamic navigation edges and calls `apply_document_update`
///    (graph surgery).
///
/// Returns a [`DidChangePhase1Result`] summarizing the dispatch decision.
/// Tests call this directly to assert dispatch behavior without needing a
/// `tower_lsp::Client` or async runtime.
pub fn did_change_phase1(
    inner: &mut crate::state::ServerStateInner,
    uri: url::Url,
    version: i32,
    content_changes: Vec<TextDocumentContentChangeEvent>,
) -> DidChangePhase1Result {
    // Update doc_versions with the authoritative LSP version
    inner.doc_versions.insert(uri.clone(), version);

    // ── Classify the edit's impact (M2) — BEFORE applying (Fix 3, R1) ──
    //
    // `classify_edit` is a pure function of pre-edit state (the document's
    // passages, the cached text, and the raw change events), so it can
    // borrow `inner` directly instead of forcing the caller to snapshot
    // `text_before` / `changes_for_classify` first. `apply_document_changes`
    // mutates only the workspace doc's rope snapshot (never `passages`),
    // so classifying before or after it sees identical inputs — but
    // classifying first lets the borrows die before the mutation begins.
    let impact = {
        let doc_ref = inner.workspace.get_document(&uri);
        match doc_ref {
            Some(doc) => {
                let text_before: &str = inner
                    .open_documents
                    .get(&uri)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                helpers::classify_edit(doc, text_before, &content_changes)
            }
            None => helpers::EditImpact::WholeDocument,
        }
    };

    // ── Pre-edit snapshot (Fix 3, R3) ──
    //
    // The single load-bearing `Document` clone. It is needed ONLY on the
    // WithinPassage path, where `did_change_incremental` splices the
    // re-parsed passage into it (it is passed in by value and consumed —
    // no second clone). The full-reparse paths never look at pre-edit
    // passages, so they skip the clone entirely.
    //
    // Cloned BEFORE `apply_document_changes`: apply mutates the workspace
    // doc's rope snapshot in place, and the splice must start from the
    // PRE-edit rope.
    let doc_before = match &impact {
        helpers::EditImpact::WithinPassage { .. } => inner.workspace.get_document(&uri).cloned(),
        _ => None,
    };

    // The edited passage's pre-edit offset — extracted before the dispatch
    // below moves `doc_before` into `did_change_incremental` (R3). The
    // offset-fix-up block at the end of phase 1 uses it to shift every
    // passage that follows the edited one.
    let edited_offset = match (&impact, doc_before.as_ref()) {
        (helpers::EditImpact::WithinPassage { passage_name, .. }, Some(doc)) => doc
            .passages
            .iter()
            .find(|p| p.name == *passage_name)
            .map(|p| p.passage_offset),
        _ => None,
    };

    // Apply incremental changes to the rope-based snapshot and get the
    // resulting full text for re-parsing. The net byte/line deltas are
    // computed from the very byte ranges that get applied, which is
    // arithmetically identical to `text.len() - text_before.len()` (and
    // the newline-count equivalent) without keeping a pre-edit copy of
    // the text around (Fix 3, R1).
    let (text, byte_delta, line_delta) =
        apply_document_changes(inner, &uri, version, content_changes);

    // Fix 3, R2: the text-cache insert moved to the END of phase 1 (see
    // below). Phase 1 holds the write lock for its entire duration, so no
    // reader can observe the cache mid-phase — every phase-1 consumer
    // (dispatch, fix-up, cache merges, graph surgery) reads workspace/
    // registry state, not `open_documents`. The text stays a borrowed
    // `&text` for both parse paths and is MOVED into the cache at the end,
    // killing the per-keystroke full-text clone.

    let format = inner.workspace.resolve_format();

    // ── Dispatch based on edit impact ──
    // `was_incremental` tracks whether the result came from the incremental
    // path (single-passage `ParseResult`) or the full re-parse path (all
    // passages). M3 uses this to decide between merging the result into the
    // existing cache (incremental) vs replacing the whole cache (full).
    let (
        mut doc,
        mut parse_result,
        was_incremental,
        is_panic_degraded,
        panicked_passage_name,
        dispatch,
    ) = match &impact {
        helpers::EditImpact::WithinPassage {
            passage_name,
            in_passage_range: _,
        } => {
            // Incremental path: re-parse only the touched passage.
            tracing::info!(
                file = %uri,
                version,
                passage = %passage_name,
                "did_change: WithinPassage — incremental re-parse"
            );

            match did_change_incremental(
                inner,
                &uri,
                &format,
                version,
                &text,
                doc_before,
                passage_name,
            ) {
                Some((doc, parse_result, panicked)) => {
                    let dispatch = if panicked {
                        DidChangeDispatch::IncrementalPanic {
                            passage_name: passage_name.clone(),
                        }
                    } else {
                        DidChangeDispatch::Incremental {
                            passage_name: passage_name.clone(),
                        }
                    };
                    let panicked_name = if panicked {
                        Some(passage_name.clone())
                    } else {
                        None
                    };
                    (doc, parse_result, true, panicked, panicked_name, dispatch)
                }
                None => {
                    // Incremental path failed (classification failure, no
                    // plugin, or offset computation issue). Fall back to
                    // full re-parse.
                    tracing::info!(
                        file = %uri,
                        "did_change: incremental path failed, falling back to full re-parse"
                    );
                    let (doc, parse_result) = helpers::parse_with_format_plugin(
                        &mut inner.format_registry,
                        &uri,
                        &text,
                        format.clone(),
                        version,
                    );
                    (
                        doc,
                        parse_result,
                        false,
                        false,
                        None,
                        DidChangeDispatch::IncrementalFallback,
                    )
                }
            }
        }
        helpers::EditImpact::BoundaryCrossing | helpers::EditImpact::WholeDocument => {
            // Full re-parse path (existing behavior).
            let dispatch = if matches!(impact, helpers::EditImpact::BoundaryCrossing) {
                tracing::info!(
                    file = %uri,
                    version,
                    "did_change: BoundaryCrossing — full re-parse"
                );
                DidChangeDispatch::BoundaryCrossing
            } else {
                tracing::info!(
                    file = %uri,
                    version,
                    "did_change: WholeDocument — full re-parse"
                );
                DidChangeDispatch::WholeDocument
            };
            let (doc, parse_result) = helpers::parse_with_format_plugin(
                &mut inner.format_registry,
                &uri,
                &text,
                format.clone(),
                version,
            );
            (doc, parse_result, false, false, None, dispatch)
        }
    };

    // ── Fix up passage_offset for passages after the edited one ──────────
    // When the edit changes the byte count (delta != 0), all passages AFTER
    // the edited passage need their `passage_offset` shifted by delta. Without
    // this, their cached diagnostics and tokens get mapped to wrong document
    // positions because the `passage_offset` on their cached
    // `PassageDiagnosticGroup` / `PassageTokenGroup` entries is stale.
    //
    // The edited passage's own offset doesn't change (the edit is within it,
    // nothing before it changed). But passages after it shift by the net byte
    // delta of the edit.
    //
    // The same applies to the passage header LINE indices (`Passage::line`)
    // when the edit changes the line count (line_delta != 0) — e.g. pasting
    // a multi-line block inside one passage shifts every later passage's
    // header line down. Byte delta and line delta are independent (replacing
    // "\n" with "x" keeps the byte count but shifts lines up; a paste can
    // change both), so they are computed and applied separately.
    //
    // This fix-up applies to:
    // 1. `doc.passages[i].passage_offset` — used by workspace lookups
    //    (find_passage, related-info builders, etc.).
    // 2. `inner.format_diagnostics[uri][name].passage_offset` — used by
    //    `build_all_lsp_diagnostics` to map format diagnostics to
    //    document-absolute ranges.
    // 3. `inner.semantic_tokens[uri][name].passage_offset` — used by
    //    `convert_semantic_tokens` to map tokens to document-absolute
    //    positions.
    // 4. `doc.passages[i].line` — used by the Story Map "open passage"
    //    navigation.
    if was_incremental {
        // `byte_delta` / `line_delta` come straight from
        // `apply_document_changes` (computed from the same byte ranges it
        // applied — Fix 3, R1), and `edited_offset` was extracted from the
        // pre-edit snapshot before the dispatch consumed it (R3).
        if byte_delta != 0 || line_delta != 0 {
            // (logging only — the offset itself was precomputed above)
            let edited_name = match &dispatch {
                DidChangeDispatch::Incremental { passage_name }
                | DidChangeDispatch::IncrementalPanic { passage_name } => passage_name.as_str(),
                _ => "",
            };

            if let Some(edited_offset) = edited_offset {
                // 1. Fix up passage_offset (byte delta) and header line
                //    index (line delta) in a single pass — the membership
                //    test must see the PRE-shift offsets, so both mutations
                //    happen together after the comparison.
                for p in doc.passages.iter_mut() {
                    if p.passage_offset > edited_offset {
                        if byte_delta != 0 {
                            p.passage_offset = ((p.passage_offset as isize) + byte_delta) as usize;
                        }
                        if line_delta != 0 {
                            p.line = ((p.line as isize) + line_delta).max(0) as u32;
                        }
                    }
                }

                // 2. Fix up format_diagnostics cache entries
                if let Some(diag_map) = inner.format_diagnostics.get_mut(&uri) {
                    for group in diag_map.values_mut() {
                        if group.passage_offset > edited_offset {
                            group.passage_offset =
                                ((group.passage_offset as isize) + byte_delta) as usize;
                        }
                    }
                }

                // 3. Fix up semantic_tokens cache entries
                if let Some(token_map) = inner.semantic_tokens.get_mut(&uri) {
                    for group in token_map.values_mut() {
                        if group.passage_offset > edited_offset {
                            group.passage_offset =
                                ((group.passage_offset as isize) + byte_delta) as usize;
                        }
                    }
                }

                tracing::trace!(
                    file = %uri,
                    edited_passage = %edited_name,
                    edited_offset,
                    byte_delta,
                    line_delta,
                    "did_change: fixed up passage_offset for passages after edited passage"
                );
            }
        }
    }

    // Update format diagnostics + semantic tokens caches (M3 surgical
    // invalidation). Fix 3, R5: the two branches are mutually exclusive,
    // so the full-reparse branch MOVES the group collections out of
    // `parse_result` (mem::take) instead of cloning them.
    if was_incremental {
        // Incremental path: merge the single-passage result into the existing
        // cache entry. Other passages' groups are untouched. On
        // panic-degraded mode, the panicked passage's tokens are removed (no
        // tokens emitted for broken JS); other passages' tokens are untouched.
        let existing = inner.format_diagnostics.entry(uri.clone()).or_default();
        helpers::merge_incremental_diagnostics(existing, &parse_result);
        let existing = inner.semantic_tokens.entry(uri.clone()).or_default();
        helpers::merge_incremental_tokens(
            existing,
            &parse_result,
            is_panic_degraded,
            panicked_passage_name.as_deref(),
        );
    } else {
        // Full re-parse path: replace the whole per-URI cache entries.
        inner.format_diagnostics.insert(
            uri.clone(),
            helpers::diagnostic_groups_to_map(std::mem::take(&mut parse_result.diagnostic_groups)),
        );
        inner.semantic_tokens.insert(
            uri.clone(),
            helpers::token_groups_to_map(std::mem::take(&mut parse_result.token_groups)),
        );
    }

    // Compute dynamic navigation edges for the new passages
    // Include the edge_type_hint from ResolvedNavLink so that dynamic
    // navigation edges preserve their semantic type (Jump, Include, etc.)
    // through graph_surgery instead of defaulting to Navigation.
    let extra_edges: Vec<(
        String,
        Option<String>,
        String,
        Option<knot_core::graph::EdgeType>,
    )> = if let Some(plug) = inner.format_registry.get(&format) {
        let var_string_map = plug.build_var_string_map(&inner.workspace);
        doc.passages
            .iter()
            .flat_map(|p| {
                plug.resolve_dynamic_navigation_links(p, &var_string_map)
                    .into_iter()
                    .map(|link| {
                        (
                            p.name.clone(),
                            link.display_text,
                            link.target,
                            link.edge_type_hint,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    };

    // Apply the document update via the centralized method, which handles
    // insert + graph_surgery + recheck_broken_links + rebuild_upstream_edges.
    // NOTE: apply_document_update does NOT extract StoryData metadata —
    // format identification belongs to the indexing pipeline only.
    let update_result = inner
        .workspace
        .apply_document_update(&uri, doc, &extra_edges);
    tracing::trace!(
        "apply_document_update: added={:?} removed={:?} modified={:?}, graph nodes={} edges={}",
        update_result.surgery_result.added,
        update_result.surgery_result.removed,
        update_result.surgery_result.modified,
        inner.workspace.graph.passage_count(),
        inner.workspace.graph.edge_count()
    );

    // ── Fix 3, R2: text cache insert (was at the top of phase 1) ──
    //
    // Inserting here — just before the write lock drops — preserves the
    // original "readers see the latest content once did_change_phase1
    // returns" contract: phase 2, `check_storydata_format_change`, and all
    // read-lock handlers run only after this function returns. `text` is
    // moved, not cloned.
    inner.open_documents.insert(uri.clone(), text);

    DidChangePhase1Result {
        uri,
        version,
        dispatch,
    }
}

/// Phase 2 of `did_change`: the synchronous read-lock analysis core.
///
/// Runs `analyze_with_format_vars` on the workspace (read-only) and builds
/// the complete LSP `Diagnostic` objects (via `build_all_lsp_diagnostics`),
/// each stamped with the document's authoritative LSP version from
/// `doc_versions` (Fix 2), ready for `send_lsp_diagnostics`. Tests call
/// this directly after [`did_change_phase1`] to inspect what would be
/// published.
///
/// **Task 3 (optimization)**: this function now does BOTH the analysis AND
/// the LSP diagnostic building (previously split between phase 2 and phase 3).
/// The LSP `publish_diagnostics` await is now lock-free (done by
/// `send_lsp_diagnostics` in the async caller — since Fix 2, the
/// single-flight diagnostics worker).
///
/// Returns `Vec<(Url, Option<i32>, Vec<Diagnostic>)>` — pre-built,
/// versioned diagnostics grouped by URI, ready to send to the client
/// without any state access.
pub fn did_change_phase2(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
) -> Vec<(url::Url, Option<i32>, Vec<lsp_types::Diagnostic>)> {
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
        let total_passages: usize = inner.workspace.documents().map(|d| d.passages.len()).sum();
        let total_docs = inner.workspace.document_count();
        tracing::debug!(
            total_documents = total_docs,
            total_passages,
            "did_change: workspace summary"
        );
    }

    // Task 3: build LSP diagnostics under the read lock (sync), so the
    // async send can be lock-free. Fix 2: stamp each URI with its
    // authoritative version under the same read lock.
    let sigils = helpers::compute_sigils(&inner.format_registry, &inner.workspace);
    helpers::stamp_doc_versions(
        inner,
        helpers::build_all_lsp_diagnostics(
            &diagnostics,
            &inner.format_diagnostics,
            &inner.open_documents,
            &inner.workspace,
            &inner.workspace.config,
            &sigils,
        ),
    )
}

// ===========================================================================
// Single-flight diagnostics worker (Fix 2, Option C)
// ===========================================================================
//
// Every pre-Fix-2 `did_change` ran Phase 2 (full-workspace analysis, ~19 ms
// on the 500-passage bench fixture — 79% of the per-keystroke pipeline) and
// Phase 3 (publish) synchronously, per keystroke, 100% redundantly during a
// typing burst. The failed gen-1 DebounceTimer (deliberately unwired in
// a680738 after the "token coloring fumbling" bug) established the design
// constraint this implementation follows: NEVER delay or drop the
// synchronous path (Phase 1); coalesce only the client-facing work — the
// same principle as the pull-mode semantic-token refresh
// (`schedule_semantic_token_refresh`) that survived where the push-mode
// timer failed.

/// Pre-built, versioned diagnostics ready for publishing (Fix 2).
pub type VersionedDiagnostics = Vec<(url::Url, Option<i32>, Vec<lsp_types::Diagnostic>)>;

/// Publish target for the single-flight analysis worker.
///
/// Production implements this for `tower_lsp::Client`; the ordering tests
/// record batches to assert coalescing/tail/close/panic behavior without a
/// real client connection.
pub(crate) trait DiagnosticsSink: Send + Sync + 'static {
    fn publish(&self, batch: VersionedDiagnostics) -> impl std::future::Future<Output = ()> + Send;
}

impl DiagnosticsSink for tower_lsp::Client {
    async fn publish(&self, batch: VersionedDiagnostics) {
        for (uri, version, diagnostics) in batch {
            self.publish_diagnostics(uri, diagnostics, version).await;
        }
    }
}

/// Tunables for [`run_diagnostics_worker`] — fields, not consts, so the
/// ordering tests can run with millisecond-scale values (plan §6.4-2).
#[derive(Clone, Copy, Debug)]
pub struct WorkerTunables {
    /// How long the burst must stay quiet before an analysis runs
    /// (trailing edge). 50 ms in production.
    pub quiet_slice: std::time::Duration,
    /// Hard cap on how far analysis may lag behind the newest edit.
    /// 500 ms in production (at most ~2 analyses/sec while typing
    /// continuously; the final state always publishes on pause).
    pub max_lag: std::time::Duration,
}

impl Default for WorkerTunables {
    fn default() -> Self {
        Self {
            quiet_slice: std::time::Duration::from_millis(50),
            max_lag: std::time::Duration::from_millis(500),
        }
    }
}

/// The single-flight analysis worker loop (Fix 2).
///
/// `did_change` pings a `tokio::sync::watch` channel after Phase 1
/// completes and the write lock is dropped. This loop:
///
/// 1. wakes on the first ping after an idle period (and consumes it, so
///    `has_changed` only reports pings arriving during the quiet wait),
/// 2. waits for a `quiet_slice` gap in typing — consuming intermediate
///    pings — but never lags more than `max_lag` behind the newest edit
///    (trailing-edge coalescing with a lag cap),
/// 3. runs `analyze` (production: [`did_change_phase2`]) under the read
///    lock, wrapped in `catch_unwind` — one panicking cycle is logged and
///    dropped, the loop keeps serving the next ping,
/// 4. skips URIs no longer present in `open_documents` (a document closed
///    while the cycle was pending must not have its diagnostics
///    resurrected by a stale cycle), and
/// 5. publishes the versioned batch through `sink` lock-free, so client
///    awaits never block the next keystroke's Phase 1.
///
/// `watch` is latest-wins by construction: only the newest (URI, stamp)
/// pair can be observed, which makes stale-overwrite races structurally
/// impossible — the worker can never wake up to process an old ping.
///
/// The `analyze` parameter makes the analysis injectable so the ordering
/// suite (plan §6.4) can pin burst coalescing, tail delivery, close
/// handling, version ordering, and panic liveness.
pub(crate) async fn run_diagnostics_worker<S, F>(
    sink: S,
    inner: std::sync::Arc<tokio::sync::RwLock<crate::state::ServerStateInner>>,
    mut rx: tokio::sync::watch::Receiver<(url::Url, u64)>,
    tunables: WorkerTunables,
    analyze: F,
) where
    S: DiagnosticsSink,
    F: Fn(&crate::state::ServerStateInner, &url::Url) -> VersionedDiagnostics + Send + 'static,
{
    loop {
        // Wake on the first ping after an idle period. The sender lives as
        // long as `ServerState`; an error means the server shut down.
        if rx.changed().await.is_err() {
            tracing::debug!("diagnostics worker: channel closed — exiting");
            return;
        }
        // Consume the waking ping so `has_changed` below only reports pings
        // arriving DURING the quiet-window wait. The latest trigger URI is
        // used for phase-2 logging (it is not part of the analysis input).
        let (trigger_uri, _stamp) = rx.borrow_and_update().clone();

        // Trailing-edge coalescing with a max-lag cap: wait for a quiet
        // gap; keep consuming pings while typing continues; never lag more
        // than `max_lag` behind the newest edit.
        let deadline = tokio::time::Instant::now() + tunables.max_lag;
        loop {
            tokio::time::sleep(tunables.quiet_slice).await;
            // `has_changed` errors only when the channel closed (server
            // shutdown) — treat that as "quiet" so this last cycle still
            // analyzes; the outer `changed().await` exits on the next lap.
            if !rx.has_changed().unwrap_or(false) {
                break; // quiet period — analyze now
            }
            rx.borrow_and_update(); // consume; typing continues
            if tokio::time::Instant::now() >= deadline {
                break; // lag cap — analyze now even mid-burst
            }
        }

        // Analyze under the read lock; filter out documents closed while
        // this cycle was pending while the lock is still held (no extra
        // lock time); contain analysis panics like `did_open` does.
        let prebuilt: VersionedDiagnostics = {
            let guard = inner.read().await;
            let analyzed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                analyze(&guard, &trigger_uri)
            }));
            match analyzed {
                Ok(mut batch) => {
                    batch.retain(|(uri, _, _)| guard.open_documents.contains_key(uri));
                    batch
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::error!(
                        "diagnostics worker: analysis panicked ({msg}) — \
                         skipping this cycle, worker stays alive"
                    );
                    continue;
                }
            }
        };

        // Publish lock-free — the client awaits do not touch the state, so
        // the next keystroke's Phase 1 is never blocked by the network.
        sink.publish(prebuilt).await;
    }
}

/// Incremental re-parse path for `WithinPassage` edits (M2).
///
/// This function:
/// 1. Finds the edited passage in `doc_before` to get its pre-edit offset and tags.
/// 2. Computes the post-edit passage offset (adjusted for any byte delta from
///    edits before this passage).
/// 3. Extracts the post-edit passage text from `text_after`.
/// 4. Calls `parse_passage_incremental` (which wraps `parse_passage_mut` in
///    `catch_unwind`).
/// 5. On success: builds a `Document` by splicing the new passage into
///    the caller's pre-edit snapshot (`doc_before`, passed by value —
///    Fix 3, R3: the caller's single `Document` clone IS the new document;
///    no second clone), fixing up `passage_offset` for subsequent passages.
/// 6. On panic: calls `replace_passage_with_error` to emit a scoped diagnostic.
/// 7. On `ClassificationFailed` or `NoPlugin`: returns `None` to signal the
///    caller to fall back to full re-parse.
///
/// Note (Fix 3, R4): the returned `ParseResult` carries `passages: Vec::new()`
/// on both success and panic paths — on the incremental path only
/// `diagnostic_groups` / `token_groups` are read (by the caller's
/// `merge_incremental_*`); the spliced `Document` carries the passages.
///
/// Returns `Some((Document, ParseResult, panicked))` on success or
/// panic-contained failure, or `None` if the caller should fall back to
/// full re-parse. The `panicked` flag is `true` when the result came from
/// the panic-error fallback path (M3 uses this to decide whether to remove
/// the panicked passage's token group from the cache).
fn did_change_incremental(
    inner: &mut crate::state::ServerStateInner,
    uri: &url::Url,
    format: &knot_core::passage::StoryFormat,
    version: i32,
    text_after: &str,
    doc_before: Option<knot_core::Document>,
    passage_name: &str,
) -> Option<(knot_core::Document, knot_formats::plugin::ParseResult, bool)> {
    let doc_before = doc_before?;

    // Find the edited passage in the pre-edit document.
    let old_passage_idx = doc_before
        .passages
        .iter()
        .position(|p| p.name == passage_name)?;
    let old_passage = &doc_before.passages[old_passage_idx];
    let old_passage_offset = old_passage.passage_offset;
    let passage_tags = old_passage.tags.clone();

    // Compute the post-edit passage offset. For single-passage edits, the
    // passage offset doesn't change (the edit is WITHIN the passage, so
    // nothing before it changed). For multi-change batches confined to the
    // same passage, we'd need to account for deltas from changes before
    // this passage — but classify_edit only returns WithinPassage when all
    // changes are in the same passage, so no changes are before it.
    // Therefore: passage_offset_new == old_passage_offset.
    let passage_offset_new = old_passage_offset;

    // ── Compute the actual end of this passage in `text_after` ──────────
    //
    // We CANNOT use `doc_before.passages[old_passage_idx + 1].passage_offset`
    // for this, because that offset is from the PRE-edit parse. If the
    // WithinPassage edit changed the passage's size (e.g. "1 + 1" → "1 +",
    // shrinking by 2 bytes), the next passage's actual offset has shifted
    // by the size delta. Using the stale offset would include bytes from
    // the next passage's `::` header in this passage's text — which then
    // breaks link extraction, span computation, and our header-stability
    // check below.
    //
    // Instead, we scan `text_after` starting just after THIS passage's
    // header line, looking for the next line that starts with `::`. That's
    // the actual end of this passage. If no such line is found, the
    // passage extends to EOF.
    //
    // This matches what the SugarCube lexer's `split_passages` does on a
    // full parse, ensuring the incremental path sees the same passage
    // boundaries the full parse would.
    if passage_offset_new > text_after.len() {
        tracing::warn!(
            file = %uri,
            passage = %passage_name,
            "did_change_incremental: passage_offset beyond text_after length, falling back"
        );
        return None;
    }
    let after_head = &text_after[passage_offset_new..];
    let header_line_end_in_slice = after_head.find('\n').unwrap_or(after_head.len());
    // Body starts after the first newline (or at end of slice if no newline).
    let body_start_in_text = passage_offset_new + header_line_end_in_slice + 1;
    let body_start_in_text = body_start_in_text.min(text_after.len());

    // Scan the body for a line starting with `::`. We look for `\n::`
    // (a newline followed by `::`) — that's the next passage header.
    // The +1 below skips the `\n` so the header offset points at `::`.
    let next_header_offset = if body_start_in_text < text_after.len() {
        text_after[body_start_in_text..]
            .find("\n::")
            .map(|p| body_start_in_text + p + 1)
    } else {
        None
    };
    let passage_end = next_header_offset.unwrap_or(text_after.len());

    let passage_text = &text_after[passage_offset_new..passage_end];

    // ── Header stability check ──────────────────────────────────────────
    //
    // The incremental path is given the OLD passage name (from `doc_before`)
    // and the post-edit passage text. The plugin's `parse_passage_mut` uses
    // the passed name verbatim — it does NOT re-parse the header. So if the
    // user edited the header (renaming, or completing a new `::` header
    // typed character-by-character), the workspace would keep the stale
    // name forever (Bug #1, #3, #5-rename).
    //
    // We must also detect when a NEW `::` header has appeared anywhere in
    // the document, even outside this passage's text. This happens when
    // the user types a new passage header character-by-character: each
    // keystroke is a single char (no `::` in the inserted text), so
    // classify_edit's `contains_passage_header_line` check on the inserted
    // text doesn't fire. But by the time we get here, `text_after` has
    // one more `::` line than `doc_before` did. The incremental path
    // would never create the new passage in the workspace (Bug #1, #3).
    //
    // Two checks:
    //   (a) Re-parse THIS passage's header line. If the name differs from
    //       `passage_name`, fall back (rename case).
    //   (b) Count `::` header lines in `text_after`. If the count differs
    //       from `doc_before.passages.len()`, fall back (new passage
    //       created, or existing passage header deleted).
    let header_line_end = passage_text.find('\n').unwrap_or(passage_text.len());
    let header_line = &passage_text[..header_line_end];
    // Parsed once for BOTH uses: the stability check below and (on success)
    // restoring the header tooling metadata onto the spliced passage.
    let parsed_header = knot_formats::header::parse_twee_header(header_line, 0);
    match &parsed_header {
        Some(parsed) if parsed.name != passage_name => {
            tracing::info!(
                file = %uri,
                old_passage = %passage_name,
                new_passage = %parsed.name,
                "did_change_incremental: passage header renamed — falling back to full re-parse"
            );
            return None;
        }
        None => {
            // Header line no longer parses as a passage header. This could
            // mean the user deleted the `::` prefix or made the name empty.
            // Fall back to full re-parse.
            tracing::info!(
                file = %uri,
                passage = %passage_name,
                "did_change_incremental: passage header no longer parses — falling back to full re-parse"
            );
            return None;
        }
        _ => {}
    }

    // (b) Count `::` header lines in the full post-edit text. If the count
    // differs from the pre-edit passage count, the document structure has
    // changed — fall back to full re-parse so the workspace graph is
    // rebuilt correctly.
    let post_edit_header_count = text_after
        .lines()
        .filter(|line| line.starts_with("::"))
        .count();
    let pre_edit_passage_count = doc_before.passages.len();
    if post_edit_header_count != pre_edit_passage_count {
        tracing::info!(
            file = %uri,
            passage = %passage_name,
            pre_edit_passages = pre_edit_passage_count,
            post_edit_headers = post_edit_header_count,
            "did_change_incremental: passage header count changed — falling back to full re-parse"
        );
        return None;
    }

    // Call the incremental parser.
    let incremental_result = helpers::parse_passage_incremental(
        &mut inner.format_registry,
        uri,
        format.clone(),
        version,
        passage_name,
        &passage_tags,
        passage_text,
        passage_offset_new,
    );

    match incremental_result {
        Ok(single_result) => {
            // Success: splice the new passage into the caller's pre-edit
            // snapshot (Fix 3, R3 — consumed by value; no clone).
            let mut new_doc = doc_before;
            // Replace the passage by name. The incremental plugin path only
            // receives name + tags, so restore the header tooling metadata
            // (position/group/color/size — an edit on the header line can
            // change them) and derive the line index from the passage
            // offset here, uniformly for every format plugin.
            if let Some(mut new_passage) = single_result.passages.into_iter().next()
                && let Some(slot) = new_doc.passages.iter_mut().find(|p| p.name == passage_name)
            {
                if let Some(hdr) = parsed_header.as_ref() {
                    knot_formats::header::apply_header_metadata(&mut new_passage, hdr);
                }
                new_passage.line = text_after[..passage_offset_new]
                    .bytes()
                    .filter(|&b| b == b'\n')
                    .count() as u32;
                *slot = new_passage;
            }
            // The passage_offset values don't need fix-up because the edit
            // is within a single passage (no boundary crossing, no size
            // change to other passages). The edited passage's offset is
            // already set correctly by parse_passage_incremental.
            //
            // Note: the edited passage's TEXT may have grown or shrunk, but
            // since we extracted text up to the NEXT passage's offset
            // (which didn't change), the edited passage's span now covers
            // the right range. The next passage's offset is still correct.

            // Build a full ParseResult from the single-passage result.
            // The diagnostic_groups and token_groups from the incremental
            // parse replace the old ones for this passage; other passages'
            // groups are preserved from doc_before's cached state (the
            // caller handles the cache merge via merge_incremental_*).
            //
            // Fix 3, R4: `passages` is left EMPTY — on the incremental path
            // nothing reads it (the caller's merge helpers touch only the
            // group fields; the spliced `new_doc` carries the passages into
            // the workspace). Populating it used to deep-copy every passage
            // on every keystroke for no reader.
            let full_result = knot_formats::plugin::ParseResult {
                passages: Vec::new(),
                token_groups: single_result.token_groups,
                diagnostic_groups: single_result.diagnostic_groups,
                is_complete: true,
            };

            // Set the snapshot on the new doc.
            //
            // Task 2 (optimization): instead of rebuilding the rope from
            // `text_after` via `set_snapshot_from_text` (which calls
            // `Rope::from_str` — a full rope build), we steal the
            // already-mutated rope from the workspace's document.
            // `apply_document_changes` (called earlier in
            // `did_change_phase1`) mutated `inner.workspace.documents[uri]
            // .snapshot.rope` in place via `apply_incremental_change`.
            // That rope IS the post-edit rope. We clone it (ropey's clone
            // is O(log n) tree copy, much cheaper than `Rope::from_str`)
            // and install it on `new_doc`.
            //
            // We also update `passage_names` on the snapshot to match
            // `new_doc.passages` (in case the edit renamed a passage —
            // rare for WithinPassage but defensive).
            new_doc.version = version;
            if let Some(workspace_doc) = inner.workspace.get_document(uri) {
                if let Some(ref ws_snapshot) = workspace_doc.snapshot {
                    let mut snapshot = ws_snapshot.clone();
                    snapshot.version = version;
                    snapshot.passage_names =
                        new_doc.passages.iter().map(|p| p.name.clone()).collect();
                    new_doc.snapshot = Some(snapshot);
                } else {
                    // No snapshot on the workspace doc — fall back to
                    // rebuilding from text (rare; only if the workspace
                    // doc lost its snapshot somehow).
                    new_doc.set_snapshot_from_text(text_after);
                }
            } else {
                // No workspace doc — fall back to rebuilding from text.
                new_doc.set_snapshot_from_text(text_after);
            }

            Some((new_doc, full_result, /* panicked = */ false))
        }
        Err(helpers::PassageParseError::Panic(msg)) => {
            // Panic contained: emit a scoped diagnostic for this passage,
            // leave other passages' tokens/diagnostics intact from the
            // previous parse. M3's surgical cache invalidation makes this
            // truly scoped — only the panicked passage's cache entry is
            // touched.
            tracing::warn!(
                file = %uri,
                passage = %passage_name,
                msg = %msg,
                "did_change_incremental: passage parse panicked — scoped error emitted"
            );

            // Fix 3, R3: borrow for the error group BEFORE taking ownership
            // of the snapshot (the old code cloned the whole document just
            // to keep a borrow alive past the move).
            let error_group = helpers::replace_passage_with_error(&doc_before, passage_name, &msg);
            let mut new_doc = doc_before;
            new_doc.version = version;
            // Task 2 (optimization): steal the mutated rope from the
            // workspace doc instead of rebuilding from String. Same
            // pattern as the success path above.
            if let Some(workspace_doc) = inner.workspace.get_document(uri) {
                if let Some(ref ws_snapshot) = workspace_doc.snapshot {
                    let mut snapshot = ws_snapshot.clone();
                    snapshot.version = version;
                    snapshot.passage_names =
                        new_doc.passages.iter().map(|p| p.name.clone()).collect();
                    new_doc.snapshot = Some(snapshot);
                } else {
                    new_doc.set_snapshot_from_text(text_after);
                }
            } else {
                new_doc.set_snapshot_from_text(text_after);
            }

            // Build a diagnostic group for the panicked passage.
            // (computed BEFORE the move into new_doc — see R3 note above)

            // The ParseResult contains only the panicked passage's
            // diagnostic group (no token groups — the plugin panicked
            // before emitting any). The caller's
            // `merge_incremental_diagnostics` will replace just this
            // passage's entry in `format_diagnostics`; the caller's
            // `merge_incremental_tokens` (with `is_panic_degraded=true`)
            // will REMOVE this passage's entry from `semantic_tokens` so
            // we don't show stale tokens for broken JS. Other passages'
            // cache entries are untouched.
            let full_result = knot_formats::plugin::ParseResult {
                passages: Vec::new(), // Fix 3, R4 — see success path note
                token_groups: Vec::new(),
                diagnostic_groups: vec![error_group.unwrap_or_else(|| {
                    knot_formats::plugin::PassageDiagnosticGroup {
                        passage_name: passage_name.to_string(),
                        passage_offset: passage_offset_new,
                        diagnostics: vec![knot_formats::plugin::FormatDiagnostic {
                            range: 0..1,
                            message: format!("Internal error: passage parse panicked — {}", msg),
                            severity: knot_formats::plugin::FormatDiagnosticSeverity::Error,
                            code: "sc-parse".to_string(),
                        }],
                    }
                })],
                is_complete: false,
            };

            Some((new_doc, full_result, /* panicked = */ true))
        }
        Err(helpers::PassageParseError::ClassificationFailed) => {
            // Plugin returned None — fall back to full re-parse.
            None
        }
        Err(helpers::PassageParseError::NoPlugin) => {
            // No plugin — fall back to full re-parse (which will produce
            // an empty document).
            None
        }
    }
}

pub(crate) async fn did_close(state: &ServerState, params: DidCloseTextDocumentParams) {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    tracing::info!("did_close: {}", uri);

    let mut inner = state.inner.write().await;
    // Remove from editor-open set.
    inner.editor_open_docs.remove(&uri);
    inner.format_diagnostics.remove(&uri);
    inner.semantic_tokens.remove(&uri);
    // Fix 2: capture the last known version BEFORE the entry is removed —
    // the clear below publishes with it, so clients that order by version
    // discard any in-flight worker push built from a pre-close state.
    let last_version = inner.doc_versions.remove(&uri);

    // Decide whether to drop the document from the workspace graph.
    //
    // - Files INSIDE the workspace root stay in the graph even after the
    //   editor tab is closed. They are still relevant project content
    //   (graph diagnostics, broken-link checks, find-references) until
    //   they are actually deleted from disk. When deletion happens,
    //   `did_change_watched_files` with `FileChangeType::DELETED` will
    //   call `remove_document_and_update_graph` and clean up properly.
    //
    // - Files OUTSIDE the workspace root (e.g. a reference `.twee` from
    //   another project the user opened in this window) are dropped
    //   immediately on close. They were only loaded for per-file LSP
    //   features while the tab was open; their passages must not
    //   pollute the project graph after close (Bug #5). They are not
    //   covered by the workspace file watcher, so leaving them in the
    //   graph would leak forever.
    let in_workspace = helpers::is_uri_in_workspace(&uri, &inner.workspace.root_uri);
    if !in_workspace {
        let removed = inner.workspace.remove_document_and_update_graph(&uri);
        if let Some(doc) = &removed {
            tracing::info!(
                file = %uri,
                passage_count = doc.passages.len(),
                "did_close: removed out-of-workspace document from graph"
            );
        }
        // Also drop the cached text — there is no file watcher to
        // invalidate it, and the user has signalled they are done with
        // it by closing the tab.
        inner.open_documents.remove(&uri);
    } else {
        tracing::debug!(
            file = %uri,
            "did_close: in-workspace file — keeping in graph; deletion is handled by did_change_watched_files"
        );
    }

    drop(inner);

    // Clear diagnostics for the closed file, versioned with the document's
    // last known LSP version (Fix 2 — `None` if it was never opened in the
    // editor, matching the pre-Fix-2 behavior).
    state
        .client
        .publish_diagnostics(uri, Vec::new(), last_version)
        .await;
}

pub(crate) async fn did_save(_state: &ServerState, params: DidSaveTextDocumentParams) {
    tracing::info!("did_save: {}", params.text_document.uri);
}

pub(crate) async fn did_change_configuration(
    state: &ServerState,
    _params: DidChangeConfigurationParams,
) {
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
            && let Some(severity_str) = value.as_str()
        {
            let severity = match severity_str {
                "error" => Some(knot_core::workspace::DiagnosticSeverity::Error),
                "warning" => Some(knot_core::workspace::DiagnosticSeverity::Warning),
                "info" => Some(knot_core::workspace::DiagnosticSeverity::Info),
                "hint" => Some(knot_core::workspace::DiagnosticSeverity::Hint),
                "off" => Some(knot_core::workspace::DiagnosticSeverity::Off),
                _ => None,
            };
            if let Some(sev) = severity {
                inner
                    .workspace
                    .config
                    .diagnostics
                    .insert(diag_key.to_string(), sev);
            }
        }
    }

    // Re-run analysis and publish diagnostics with updated config
    // Release write lock before analysis
    drop(inner);

    // Task 3: consolidated read-lock phase — analysis + build LSP diagnostics.
    let prebuilt_diagnostics = {
        let inner = state.inner.read().await;
        let diagnostics =
            helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
        let sigils = helpers::compute_sigils(&inner.format_registry, &inner.workspace);
        helpers::stamp_doc_versions(
            &inner,
            helpers::build_all_lsp_diagnostics(
                &diagnostics,
                &inner.format_diagnostics,
                &inner.open_documents,
                &inner.workspace,
                &inner.workspace.config,
                &sigils,
            ),
        )
    }; // ← read lock dropped

    // Send diagnostics to the client (lock-free).
    helpers::send_lsp_diagnostics(&state.client, prebuilt_diagnostics).await;
}

pub(crate) async fn did_change_watched_files(
    state: &ServerState,
    params: DidChangeWatchedFilesParams,
) {
    tracing::info!("did_change_watched_files: {} events", params.changes.len());

    for event in params.changes {
        let uri = helpers::normalize_file_uri(&event.uri);
        let file_type = uri.to_file_path().and_then(|p| {
            p.extension()
                .and_then(|e| e.to_str().map(|s| s.to_string()))
                .ok_or(())
        });

        // Accept .tw, .twee, and .js files. .js files are indexed and
        // analyzed the same way as [script]-tagged passages — see
        // parse_script_file in the SugarCube parse pipeline.
        let is_supported = match file_type.as_deref() {
            Ok("tw") | Ok("twee") | Ok("js") => true,
            _ => false,
        };

        if !is_supported {
            continue;
        }

        match event.typ {
            FileChangeType::CREATED => {
                tracing::info!("File created: {}", uri);
                // Read and index the new file
                if let Ok(path) = uri.to_file_path()
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    let mut inner = state.inner.write().await;

                    let format = inner.workspace.resolve_format();
                    let (doc, parse_result) = helpers::parse_with_format_plugin(
                        &mut inner.format_registry,
                        &uri,
                        &text,
                        format.clone(),
                        0,
                    );

                    inner.open_documents.insert(uri.clone(), text.clone());
                    inner.format_diagnostics.insert(
                        uri.clone(),
                        helpers::diagnostic_groups_to_map(parse_result.diagnostic_groups),
                    );
                    inner.semantic_tokens.insert(
                        uri.clone(),
                        helpers::token_groups_to_map(parse_result.token_groups),
                    );

                    // StoryData parsing is a core-only operation (see: Format Isolation).
                    inner.workspace.insert_document(doc);

                    // Format is frozen after indexing — no dynamic format switches.
                    // Just rebuild the graph with the current (frozen) format.
                    let format_after = inner.workspace.resolve_format();
                    inner.workspace.graph = helpers::rebuild_graph(
                        &inner.workspace,
                        &inner.format_registry,
                        format_after.clone(),
                    );

                    // Release write lock before analysis
                    drop(inner);

                    // Task 3: consolidated read-lock phase — analysis + build LSP diagnostics.
                    let prebuilt_diagnostics = {
                        let inner = state.inner.read().await;
                        let diagnostics = helpers::analyze_with_format_vars(
                            &inner.workspace,
                            &inner.format_registry,
                        );
                        let sigils =
                            helpers::compute_sigils(&inner.format_registry, &inner.workspace);
                        helpers::stamp_doc_versions(
                            &inner,
                            helpers::build_all_lsp_diagnostics(
                                &diagnostics,
                                &inner.format_diagnostics,
                                &inner.open_documents,
                                &inner.workspace,
                                &inner.workspace.config,
                                &sigils,
                            ),
                        )
                    }; // ← read lock dropped

                    // Send diagnostics to the client (lock-free).
                    helpers::send_lsp_diagnostics(&state.client, prebuilt_diagnostics).await;

                    // Schedule debounced semantic token refresh
                    state.schedule_semantic_token_refresh().await;
                }
            }
            FileChangeType::DELETED => {
                tracing::info!("File deleted: {}", uri);
                let mut inner = state.inner.write().await;
                inner.open_documents.remove(&uri);
                inner.editor_open_docs.remove(&uri);
                inner.format_diagnostics.remove(&uri);

                // Clean up format registries for the deleted file.
                // Without this, stale variables/macros/functions/templates
                // from the deleted document persist in completion and hover
                // until a full workspace re-parse.
                let format = inner.workspace.resolve_format();
                if let Some(plug) = inner.format_registry.get_mut(&format) {
                    plug.remove_file_from_registries(uri.as_ref());
                }

                inner.workspace.remove_document_and_update_graph(&uri);

                // Recheck broken links after removal
                inner.workspace.graph.recheck_broken_links();

                // Rebuild upstream lifecycle edges for special passages after
                // removal. When a file containing a Startup or ScriptInjection
                // passage is deleted, all edges connected to that node are
                // removed, which can break the upstream chain for remaining
                // special passages.
                inner.workspace.rebuild_upstream_edges();

                // Release write lock before analysis
                drop(inner);

                // Task 3: consolidated read-lock phase — analysis + build LSP diagnostics.
                let prebuilt_diagnostics = {
                    let inner = state.inner.read().await;
                    let diagnostics =
                        helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
                    let sigils = helpers::compute_sigils(&inner.format_registry, &inner.workspace);
                    helpers::stamp_doc_versions(
                        &inner,
                        helpers::build_all_lsp_diagnostics(
                            &diagnostics,
                            &inner.format_diagnostics,
                            &inner.open_documents,
                            &inner.workspace,
                            &inner.workspace.config,
                            &sigils,
                        ),
                    )
                }; // ← read lock dropped

                // Send diagnostics to the client (lock-free).
                helpers::send_lsp_diagnostics(&state.client, prebuilt_diagnostics).await;

                // Schedule debounced semantic token refresh for remaining
                // documents whose broken link status may have changed.
                state.schedule_semantic_token_refresh().await;

                // Clear diagnostics for the deleted file
                state
                    .client
                    .publish_diagnostics(uri, Vec::new(), None)
                    .await;
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
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    let mut inner = state.inner.write().await;

                    let format = inner.workspace.resolve_format();
                    let (doc, parse_result) = helpers::parse_with_format_plugin(
                        &mut inner.format_registry,
                        &uri,
                        &text,
                        format.clone(),
                        0,
                    );

                    inner.open_documents.insert(uri.clone(), text.clone());
                    inner.format_diagnostics.insert(
                        uri.clone(),
                        helpers::diagnostic_groups_to_map(parse_result.diagnostic_groups),
                    );
                    inner.semantic_tokens.insert(
                        uri.clone(),
                        helpers::token_groups_to_map(parse_result.token_groups),
                    );

                    // StoryData parsing is a core-only operation (see: Format Isolation).
                    inner.workspace.insert_document(doc);

                    let format_after = inner.workspace.resolve_format();
                    inner.workspace.graph = helpers::rebuild_graph(
                        &inner.workspace,
                        &inner.format_registry,
                        format_after.clone(),
                    );

                    // Release write lock before analysis
                    drop(inner);

                    // Task 3: consolidated read-lock phase — analysis + build LSP diagnostics.
                    let prebuilt_diagnostics = {
                        let inner = state.inner.read().await;
                        let diagnostics = helpers::analyze_with_format_vars(
                            &inner.workspace,
                            &inner.format_registry,
                        );
                        let sigils =
                            helpers::compute_sigils(&inner.format_registry, &inner.workspace);
                        helpers::stamp_doc_versions(
                            &inner,
                            helpers::build_all_lsp_diagnostics(
                                &diagnostics,
                                &inner.format_diagnostics,
                                &inner.open_documents,
                                &inner.workspace,
                                &inner.workspace.config,
                                &sigils,
                            ),
                        )
                    }; // ← read lock dropped

                    // Send diagnostics to the client (lock-free).
                    helpers::send_lsp_diagnostics(&state.client, prebuilt_diagnostics).await;

                    // Schedule debounced semantic token refresh
                    state.schedule_semantic_token_refresh().await;
                }
            }
            _ => {}
        }
    }
}

/// Apply a `didChange` batch to the workspace doc's rope snapshot and return
/// the post-edit full text, together with the net **byte delta** and **line
/// delta** of the batch.
///
/// With INCREMENTAL sync, each `TextDocumentContentChangeEvent` contains
/// a `range` (the region being replaced) and `text` (the replacement text).
/// If the range is `None`, the change is a full-text replacement.
///
/// The deltas are computed from the very byte ranges that get applied
/// (`Σ len(inserted) − (range.end − range.start)`, and the newline-count
/// equivalent), which is arithmetically identical to comparing the pre- and
/// post-edit texts — without materializing a pre-edit copy (Fix 3, R1). On
/// the full-replacement / no-snapshot recovery paths the deltas are computed
/// against the cached text length instead (cheap, allocation-free, and only
/// consumed on the incremental path anyway).
///
/// Kept private: `did_change_phase1` is the only caller.
fn apply_document_changes(
    inner: &mut ServerStateInner,
    uri: &Url,
    version: i32,
    content_changes: Vec<TextDocumentContentChangeEvent>,
) -> (String, isize, isize) {
    use crate::handlers::helpers::lsp_range_to_byte_range;

    let newline_count = |t: &str| t.bytes().filter(|&b| b == b'\n').count() as isize;

    // Collect incremental changes as (byte_range, new_text) pairs.
    // We need the current text to convert LSP positions to byte offsets.
    // The current text comes from the rope snapshot (if available) or
    // the open_documents cache.
    let has_snapshot = inner
        .workspace
        .get_document(uri)
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

            let (cached_len, cached_newlines) = inner
                .open_documents
                .get(uri)
                .map(|s| (s.len() as isize, newline_count(s)))
                .unwrap_or((0, 0));
            let byte_delta = text.len() as isize - cached_len;
            let line_delta = newline_count(&text) - cached_newlines;

            tracing::debug!(
                file = %uri,
                version,
                text_len = text.len(),
                "apply_document_changes: full-text replacement"
            );
            (text, byte_delta, line_delta)
        } else {
            // All changes have ranges — apply incrementally.
            //
            // Fix 3, R6: VS Code sends ONE change event per keystroke. The
            // single-change fast path converts its LSP range against a
            // BORROW of the cached text — no O(doc) clone, no
            // evolving-text rebuild. Multi-change batches (paste, format-
            // on-save) keep the evolving-text approach below; their O(n·k)
            // cost is acceptable on those cold paths.

            // Get the current full text for position conversion
            if content_changes.len() == 1 {
                // ── Hot path: exactly one change event ──
                let TextDocumentContentChangeEvent {
                    range,
                    text: new_text,
                    ..
                } = content_changes.into_iter().next().expect("len == 1");
                let range = range.expect("checked above: no full-replace in this branch");

                let byte_delta;
                let line_delta;
                let byte_changes = {
                    // Borrow the cached text ONLY for position conversion;
                    // the borrow dies before the rope mutation below.
                    let cached: &str = inner
                        .open_documents
                        .get(uri)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let byte_range = lsp_range_to_byte_range(cached, &range);
                    byte_delta =
                        new_text.len() as isize - (byte_range.end - byte_range.start) as isize;
                    line_delta = newline_count(&new_text)
                        - newline_count(&cached[byte_range.start..byte_range.end]);
                    vec![(byte_range, new_text)]
                };

                // Apply to the rope snapshot (borrow of open_documents is
                // done; `inner` can be mutably borrowed again).
                if let Some(doc) = inner.workspace.get_document_mut(uri) {
                    match doc.apply_incremental_change(version, &byte_changes) {
                        Some(text) => {
                            tracing::debug!(
                                file = %uri,
                                version,
                                change_count = 1,
                                text_len = text.len(),
                                "apply_document_changes: incremental applied (single change)"
                            );
                            return (text, byte_delta, line_delta);
                        }
                        None => {
                            tracing::warn!(
                                file = %uri,
                                "apply_document_changes: snapshot unexpectedly None, falling back to full text"
                            );
                        }
                    }
                }

                // Fallback (snapshot unexpectedly gone): rebuild the text
                // from the cache + the one change. Rare defensive path.
                let cached = inner.open_documents.get(uri).cloned().unwrap_or_default();
                let (r, t) = &byte_changes[0];
                let mut rebuilt = String::with_capacity(cached.len() - (r.end - r.start) + t.len());
                rebuilt.push_str(&cached[..r.start]);
                rebuilt.push_str(t);
                rebuilt.push_str(&cached[r.end..]);
                (rebuilt, byte_delta, line_delta)
            } else {
                // ── Cold path: multi-change batch (paste / format) ──
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
                // Net byte/line deltas accumulated from the applied ranges
                // (Fix 3, R1 — equivalent to comparing pre/post text lengths).
                let mut byte_delta: isize = 0;
                let mut line_delta: isize = 0;

                for change in &content_changes {
                    if let Some(range) = &change.range {
                        let byte_range = lsp_range_to_byte_range(&evolving_text, range);
                        byte_delta += change.text.len() as isize
                            - (byte_range.end - byte_range.start) as isize;
                        line_delta += newline_count(&change.text)
                            - newline_count(&evolving_text[byte_range.clone()]);
                        byte_changes.push((byte_range.clone(), change.text.clone()));

                        // Update evolving_text to reflect this change so that
                        // subsequent position conversions are correct
                        let mut new_text = String::with_capacity(
                            evolving_text.len() - (byte_range.end - byte_range.start)
                                + change.text.len(),
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
                            return (text, byte_delta, line_delta);
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
                (evolving_text, byte_delta, line_delta)
            }
        }
    } else {
        // No snapshot available.
        //
        // P0-3 (study / plan.md Phase 1.6): the old fallback used the LAST
        // change's text as the whole document. That is only correct for
        // full-text syncs (`range: None`). For an incremental change
        // (`range: Some(_)`), `.text` is just the typed snippet — treating
        // it as the whole document silently replaced the cached document
        // with a fragment, corrupting passages/diagnostics (reachable when
        // a file is opened during indexing and edited before its snapshot
        // exists).
        //
        // Recovery:
        // - Full-text events: take the text as before and rebuild the
        //   snapshot (this also heals the no-snapshot state).
        // - Incremental events: we cannot reconstruct the document from a
        //   snippet — re-read the file from disk; if the read fails, keep
        //   the previously cached text and effectively skip this event. A
        //   later full-text sync or save heals the state. Either way the
        //   cache is never replaced with a bare snippet.
        let has_full_replace = content_changes.iter().any(|c| c.range.is_none());

        if has_full_replace {
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
                "apply_document_changes: no snapshot, full-text replacement"
            );
            let (cached_len, cached_newlines) = inner
                .open_documents
                .get(uri)
                .map(|s| (s.len() as isize, newline_count(s)))
                .unwrap_or((0, 0));
            let byte_delta = text.len() as isize - cached_len;
            let line_delta = newline_count(&text) - cached_newlines;
            (text, byte_delta, line_delta)
        } else {
            match uri
                .to_file_path()
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
            {
                Some(disk_text) => {
                    // Re-read succeeded — treat as a full document and
                    // rebuild the snapshot from it.
                    if let Some(doc) = inner.workspace.get_document_mut(uri) {
                        doc.version = version;
                        doc.set_snapshot_from_text(&disk_text);
                    }
                    let (cached_len, cached_newlines) = inner
                        .open_documents
                        .get(uri)
                        .map(|s| (s.len() as isize, newline_count(s)))
                        .unwrap_or((0, 0));
                    let byte_delta = disk_text.len() as isize - cached_len;
                    let line_delta = newline_count(&disk_text) - cached_newlines;
                    tracing::warn!(
                        file = %uri,
                        version,
                        text_len = disk_text.len(),
                        "apply_document_changes: no snapshot for incremental change, re-read from disk"
                    );
                    (disk_text, byte_delta, line_delta)
                }
                None => {
                    // Cannot reconstruct the document — keep the previously
                    // cached text so the cache is not corrupted by the
                    // snippet. The next full-text sync/save heals the state.
                    // Delta against the cache is zero by construction.
                    let stale = inner.open_documents.get(uri).cloned().unwrap_or_default();
                    tracing::warn!(
                        file = %uri,
                        version,
                        text_len = stale.len(),
                        "apply_document_changes: no snapshot for incremental change and disk re-read failed, keeping stale cache"
                    );
                    (stale, 0, 0)
                }
            }
        }
    }
}

// ===========================================================================
// Tests (M4 — incremental dispatch integration tests)
// ===========================================================================
//
// These tests exercise `did_change_phase1` (the synchronous core of
// `did_change`) directly, without needing a `tower_lsp::Client` or async
// runtime. They construct a `ServerStateInner` with the SugarCube plugin,
// simulate LSP `didChange` content-change events, and assert:
//   - which dispatch path was taken (incremental vs full re-parse),
//   - whether the cache was surgically updated (other passages' entries
//     untouched) or wholesale replaced,
//   - whether diagnostics are correctly scoped to the edited passage.

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::passage::StoryFormat;
    use tower_lsp::lsp_types::{
        Position, Range as LspRange, TextDocumentContentChangeEvent,
        VersionedTextDocumentIdentifier,
    };

    /// Build a `ServerStateInner` pre-populated with a SugarCube-parsed
    /// document. The document has whatever passages the caller provides in
    /// `src`. The state is ready to receive `did_change_phase1` calls.
    fn build_state(src: &str) -> (crate::state::ServerStateInner, url::Url) {
        let uri = url::Url::parse("file:///project/story.tw").unwrap();
        let mut registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let format = StoryFormat::SugarCube;

        // Parse the initial document.
        let parse_result = {
            let plugin = registry
                .get_mut(&format)
                .expect("SugarCube plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        // Build the workspace with the parsed document.
        let mut workspace = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Force SugarCube (no StoryData passage in test fixtures).
        workspace.config.format = Some("SugarCube".to_string());
        let mut doc = knot_core::Document::new(uri.clone(), StoryFormat::SugarCube);
        doc.set_snapshot_from_text(src);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        workspace.insert_document(doc);

        let inner = crate::state::ServerStateInner {
            workspace,
            format_registry: registry,
            editor_open_docs: std::collections::HashSet::new(),
            open_documents: {
                let mut m = std::collections::HashMap::new();
                m.insert(uri.clone(), src.to_string());
                m
            },
            format_diagnostics: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    uri.clone(),
                    helpers::diagnostic_groups_to_map(parse_result.diagnostic_groups),
                );
                m
            },
            doc_versions: std::collections::HashMap::new(),
            semantic_tokens: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    uri.clone(),
                    helpers::token_groups_to_map(parse_result.token_groups),
                );
                m
            },
            installed_formats: Vec::new(),
            global_storage_path: None,
        };
        (inner, uri)
    }

    /// Build a `TextDocumentContentChangeEvent` with the given LSP line range
    /// and replacement text.
    fn change(
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        text: &str,
    ) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(LspRange {
                start: Position::new(start_line, start_char),
                end: Position::new(end_line, end_char),
            }),
            range_length: None,
            text: text.to_string(),
        }
    }

    /// Build a `DidChangeTextDocumentParams` for the given URI, version, and
    /// content changes. Used to construct the LSP notification payload that
    /// `did_change` would receive.
    fn _did_change_params(
        uri: &url::Url,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> DidChangeTextDocumentParams {
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            content_changes: changes,
        }
    }

    // -----------------------------------------------------------------------
    // Header metadata survival through the incremental edit path
    // -----------------------------------------------------------------------

    #[test]
    fn incremental_edit_preserves_metadata_and_shifts_later_lines() {
        // Three passages with header metadata. An edit inside the FIRST
        // passage's body that adds two lines must:
        // - keep the edited passage's metadata (restored from its header),
        // - shift the later passages' line indices by +2,
        // - keep the edited passage's own line stable.
        let src = concat!(
            ":: Start {\"position\":\"10,20\",\"group\":\"Intro\",\"color\":\"#aabbcc\"}\n",
            "line one\n",
            ":: Middle {\"position\":\"30,40\"}\n",
            "mid body\n",
            ":: End {\"position\":\"50,60\"}\n",
            "end body\n"
        );
        let (mut inner, uri) = build_state(src);

        // Insert two lines inside Start's body (before "line one", line 1).
        let changes = vec![change(1, 0, 1, 0, "extra\nlines\n")];
        let _result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        let doc = inner.workspace.get_document(&uri).expect("document");
        let start = doc.passages.iter().find(|p| p.name == "Start").unwrap();
        let middle = doc.passages.iter().find(|p| p.name == "Middle").unwrap();
        let end = doc.passages.iter().find(|p| p.name == "End").unwrap();

        // Metadata restored on the edited passage (the incremental plugin
        // path only receives name + tags; the server re-derives the rest).
        assert_eq!(start.position, Some((10.0, 20.0)));
        assert_eq!(start.group.as_deref(), Some("Intro"));
        assert_eq!(start.color.as_deref(), Some("#aabbcc"));
        // Line numbers: Start stayed at 0; Middle/End shifted by +2.
        assert_eq!(start.line, 0);
        assert_eq!(middle.line, 4, "Middle's header shifted down 2 lines");
        assert_eq!(end.line, 6, "End's header shifted down 2 lines");
        // Untouched passages keep their metadata.
        assert_eq!(middle.position, Some((30.0, 40.0)));
        assert_eq!(end.position, Some((50.0, 60.0)));
    }

    #[test]
    fn incremental_header_line_edit_refreshes_metadata() {
        // An edit ON a passage's header line (same passage, same name, so
        // classify_edit says WithinPassage) must refresh the header metadata
        // instead of silently keeping the pre-edit values forever.
        //   Line 0: `:: Start {"position":"10,20"}` — replace `10` with `99`.
        let src = ":: Start {\"position\":\"10,20\"}\nbody\n:: Second\nbody2\n";
        let (mut inner, uri) = build_state(src);

        // Columns: `:: Start {"position":"10,20"}` — "10" spans 22..24.
        let changes = vec![change(0, 22, 0, 24, "99")];
        let _result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        let doc = inner.workspace.get_document(&uri).expect("document");
        let start = doc.passages.iter().find(|p| p.name == "Start").unwrap();
        assert_eq!(
            start.position,
            Some((99.0, 20.0)),
            "header metadata must refresh from the edited header line"
        );
        // Same-length replacement: no line/offset shifts.
        let second = doc.passages.iter().find(|p| p.name == "Second").unwrap();
        assert_eq!(second.line, 2);
    }

    // -----------------------------------------------------------------------
    // Test 0 (P0-3): incremental change without snapshot must not corrupt
    // -----------------------------------------------------------------------

    #[test]
    fn incremental_change_without_snapshot_does_not_corrupt_cache() {
        // P0-3 (study / plan.md Phase 1.6): when the document has NO rope
        // snapshot (e.g. the file was opened during indexing and edited
        // before the snapshot existed) and an INCREMENTAL change arrives,
        // the old fallback treated the typed snippet as the whole document,
        // silently replacing the cache with a fragment. The cache must keep
        // the previous text instead (disk re-read, or stale until the next
        // full sync / save heals it).
        let src = ":: Start\nHello world\n";
        let (mut inner, uri) = build_state(src);

        // Simulate the missing-snapshot state.
        inner
            .workspace
            .get_document_mut(&uri)
            .expect("document in workspace")
            .snapshot = None;

        // An incremental change replacing "world" with "XY" — the typed
        // snippet "XY" is NOT the whole document.
        let changes = vec![change(1, 6, 1, 11, "XY")];
        let _result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        let cached = inner.open_documents.get(&uri).expect("cached text");
        assert_ne!(
            cached, "XY",
            "the snippet must never replace the whole document"
        );
        assert!(
            cached.contains(":: Start"),
            "document cache should remain usable (stale or disk re-read), got: {cached:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 1: within-passage edit re-parses only that passage
    // -----------------------------------------------------------------------

    #[test]
    fn within_passage_edit_takes_incremental_path() {
        // Two passages: A and B. Edit falls entirely within passage A's body.
        // Text:
        //   Line 0: ":: Start\n"        (0..9, 9 bytes)
        //   Line 1: "Hello world\n"     (9..21, 12 bytes)
        //   Line 2: ":: Second\n"       (21..31, 10 bytes)
        //   Line 3: "Goodbye\n"         (31..39, 8 bytes)
        let src = ":: Start\nHello world\n:: Second\nGoodbye\n";
        let (mut inner, uri) = build_state(src);

        // Replace "world" (line 1, chars 6..11) with "there".
        let changes = vec![change(1, 6, 1, 11, "there")];

        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // The dispatch was Incremental for passage "Start".
        assert_eq!(
            result.dispatch,
            DidChangeDispatch::Incremental {
                passage_name: "Start".to_string()
            }
        );

        // The workspace still has 2 passages (Start + Second).
        let doc = inner.workspace.get_document(&uri).expect("doc exists");
        assert_eq!(doc.passages.len(), 2);
        assert_eq!(doc.passages[0].name, "Start");
        assert_eq!(doc.passages[1].name, "Second");

        // The edited passage's text was updated.
        let new_text = inner.open_documents.get(&uri).unwrap();
        assert!(new_text.contains("Hello there"));
        assert!(!new_text.contains("Hello world"));

        // The cache has entries for both passages (surgical update — B's
        // entry was preserved from the initial parse).
        let diags = inner.format_diagnostics.get(&uri).unwrap();
        assert!(diags.contains_key("Start"));
        assert!(diags.contains_key("Second"));
        let tokens = inner.semantic_tokens.get(&uri).unwrap();
        assert!(tokens.contains_key("Start"));
        assert!(tokens.contains_key("Second"));
    }

    // -----------------------------------------------------------------------
    // Test 2: edit introducing a syntax error is confined to one passage
    // -----------------------------------------------------------------------

    #[test]
    fn edit_introducing_syntax_error_confined_to_one_passage() {
        // Two passages. Edit passage A to introduce invalid JS in a <<run>>
        // macro. The error should be confined to passage A; passage B should
        // have no sc-js diagnostics.
        let src = ":: Start\n<<run 1 + 1>>\n:: Second\nSome text.\n";
        let (mut inner, uri) = build_state(src);

        // Replace "1 + 1" (line 1, chars 7..12) with "1 +" — incomplete
        // expression, will produce an oxc parse error.
        let changes = vec![change(1, 7, 1, 12, "1 +")];

        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // Still took the incremental path for passage "Start".
        assert_eq!(
            result.dispatch,
            DidChangeDispatch::Incremental {
                passage_name: "Start".to_string()
            }
        );

        // After phase 1, run phase 2 to collect the pre-built LSP
        // diagnostics that would be published.
        // Task 3: phase 2 now returns Vec<(Url, Vec<Diagnostic>)> instead
        // of the raw format_diagnostics cache. We inspect Diagnostic.code
        // to find sc-js errors.
        let prebuilt = did_change_phase2(&inner, &uri);

        // Flatten all diagnostics for the edited URI.
        let all_diags: Vec<&lsp_types::Diagnostic> = prebuilt
            .iter()
            .filter(|(u, _, _)| u == &uri)
            .flat_map(|(_, _, diags)| diags.iter())
            .collect();

        // The format diagnostics should contain at least one sc-js error
        // (oxc parse error from the broken expression).
        let has_js_error = all_diags.iter().any(|d| {
            matches!(&d.code, Some(lsp_types::NumberOrString::String(s)) if s == "format:sc-js")
        });
        assert!(
            has_js_error,
            "Expected a format:sc-js diagnostic for passage Start, got: {:?}",
            all_diags
        );

        // No graph diagnostics should mention Second as broken (the edit
        // didn't touch Second's links).
        // (Graph diagnostics are workspace-wide; not asserted in detail here.)
    }

    // -----------------------------------------------------------------------
    // Test 3: boundary-crossing edit falls back to full re-parse
    // -----------------------------------------------------------------------

    #[test]
    fn boundary_crossing_edit_falls_back_to_full_reparse() {
        // Two passages. Insert "\n:: NewPassage\n" mid-body in passage A —
        // the inserted text contains a line starting with `:: `, which makes
        // this a BoundaryCrossing (creates a new passage header).
        let src = ":: Start\nHello world\n:: Second\nGoodbye\n";
        let (mut inner, uri) = build_state(src);

        // Insert "\n:: NewPassage\n" at line 1, char 5 (mid "Hello world").
        // The leading \n ensures ":: NewPassage" starts at column 0 of a new
        // line, so the SugarCube lexer recognizes it as a passage header.
        let changes = vec![change(1, 5, 1, 5, "\n:: NewPassage\n")];

        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // The dispatch was BoundaryCrossing (fell back to full re-parse).
        assert_eq!(result.dispatch, DidChangeDispatch::BoundaryCrossing);

        // After full re-parse, the workspace has 3 passages: Start, NewPassage, Second.
        let doc = inner.workspace.get_document(&uri).expect("doc exists");
        assert_eq!(doc.passages.len(), 3);
        let names: Vec<&str> = doc.passages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Start"));
        assert!(names.contains(&"NewPassage"));
        assert!(names.contains(&"Second"));
    }

    // -----------------------------------------------------------------------
    // Test 4: edit at EOF within last passage
    // -----------------------------------------------------------------------

    #[test]
    fn edit_at_eof_within_last_passage_takes_incremental_path() {
        // Single passage. Edit at EOF (line 2, char 0 — past the last
        // passage's recorded span end, but within the file).
        let src = ":: Start\nHello\n";
        let (mut inner, uri) = build_state(src);

        // Insert "more" at EOF (line 2, char 0).
        let changes = vec![change(2, 0, 2, 0, "more")];

        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // The dispatch was Incremental for passage "Start" (EOF edits are
        // clamped to the last passage).
        assert_eq!(
            result.dispatch,
            DidChangeDispatch::Incremental {
                passage_name: "Start".to_string()
            }
        );

        // The text was updated.
        let new_text = inner.open_documents.get(&uri).unwrap();
        assert!(new_text.ends_with("more"));
    }

    // -----------------------------------------------------------------------
    // Test 5: classification failure falls back to full re-parse
    // -----------------------------------------------------------------------
    //
    // This test simulates a classification failure: the SugarCube plugin's
    // `parse_passage_mut` returns `None` when a passage claims to be special
    // (e.g. `[widget]`) but the classifier can't find a matching def. In
    // practice this is hard to trigger with a simple text edit, so we test
    // the fallback path indirectly: a full-text replacement (range == None)
    // always takes the WholeDocument path, which exercises the same
    // `parse_with_format_plugin` fallback code.

    #[test]
    fn full_text_replacement_takes_whole_document_path() {
        // Single passage. Replace the entire document text.
        let src = ":: Start\nHello\n";
        let (mut inner, uri) = build_state(src);

        // Full-text replacement: range == None.
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: ":: Start\nGoodbye\n:: Second\nNew passage.\n".to_string(),
        }];

        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // The dispatch was WholeDocument (full-text replacement always is).
        assert_eq!(result.dispatch, DidChangeDispatch::WholeDocument);

        // After full re-parse, the workspace has 2 passages.
        let doc = inner.workspace.get_document(&uri).expect("doc exists");
        assert_eq!(doc.passages.len(), 2);
        let names: Vec<&str> = doc.passages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Start"));
        assert!(names.contains(&"Second"));
    }

    // -----------------------------------------------------------------------
    // Test 6: incremental edit preserves other passages' cache entries
    // -----------------------------------------------------------------------
    //
    // This is the M3 acceptance criterion at the integration level: after
    // an incremental edit to passage A, passage B's diagnostic group and
    // token group in the cache are byte-for-byte unchanged.

    #[test]
    fn incremental_edit_preserves_other_passages_cache_entries() {
        let src = ":: Start\n<<run 1 + 1>>\n:: Second\nSome text.\n";
        let (mut inner, uri) = build_state(src);

        // Snapshot the cache entries for passage "Second" before the edit.
        let second_diags_before = inner
            .format_diagnostics
            .get(&uri)
            .and_then(|m| m.get("Second"))
            .cloned()
            .expect("Second has a diagnostic group before the edit");
        let second_tokens_before = inner
            .semantic_tokens
            .get(&uri)
            .and_then(|m| m.get("Second"))
            .cloned()
            .expect("Second has a token group before the edit");

        // Edit passage "Start" — replace "1 + 1" (line 1, chars 7..12) with "2".
        // This shrinks the document by 4 bytes (5 chars → 1 char), so passage
        // "Second"'s passage_offset should shift by -4.
        let changes = vec![change(1, 7, 1, 12, "2")];
        let result = did_change_phase1(&mut inner, uri.clone(), 2, changes);

        // Confirm the incremental path was taken.
        assert_eq!(
            result.dispatch,
            DidChangeDispatch::Incremental {
                passage_name: "Start".to_string()
            }
        );

        // Passage "Second"'s cache entries: the diagnostics/tokens themselves
        // (message, code, severity, passage-relative range) are unchanged, but
        // the `passage_offset` is shifted by the byte delta (-4) because the
        // edit shrank the text before passage "Second".
        let second_diags_after = inner
            .format_diagnostics
            .get(&uri)
            .and_then(|m| m.get("Second"))
            .cloned()
            .expect("Second has a diagnostic group after the edit");
        let second_tokens_after = inner
            .semantic_tokens
            .get(&uri)
            .and_then(|m| m.get("Second"))
            .cloned()
            .expect("Second has a token group after the edit");

        // The diagnostics list itself is unchanged (same messages, codes, etc.)
        assert_eq!(
            second_diags_before.diagnostics, second_diags_after.diagnostics,
            "Second's diagnostics (messages, codes, ranges) must be unchanged"
        );
        assert_eq!(
            second_tokens_before.tokens, second_tokens_after.tokens,
            "Second's tokens (types, spans, modifiers) must be unchanged"
        );

        // The passage_offset is correctly shifted by the byte delta.
        // "1 + 1" (5 chars) → "2" (1 char) = delta of -4.
        let expected_delta: isize = second_diags_after.passage_offset as isize
            - second_diags_before.passage_offset as isize;
        assert_eq!(
            expected_delta, -4,
            "Second's passage_offset should shift by -4 (5 chars replaced with 1). Before: {}, After: {}",
            second_diags_before.passage_offset, second_diags_after.passage_offset
        );
        assert_eq!(
            second_tokens_after.passage_offset, second_diags_after.passage_offset,
            "Token and diagnostic groups should have the same passage_offset"
        );
    }

    // -----------------------------------------------------------------------
    // Fix 3 (hot-path de-cloning) — regression tests
    // -----------------------------------------------------------------------

    use proptest::prelude::*;

    fn newlines(t: &str) -> isize {
        t.bytes().filter(|&b| b == b'\n').count() as isize
    }

    /// Snap a byte offset down to the nearest UTF-8 char boundary.
    /// Used ONLY where this test suite slices `&str` directly (slicing a
    /// mid-sequence offset panics at the language level). It is NOT needed
    /// for `byte_offset_to_position` itself anymore — that converter now
    /// floors internally (round 1.1 / DT3), and
    /// `byte_offset_to_position_never_panics_on_any_offset` below pins it
    /// with unsnapped inputs.
    fn snap_to_boundary(t: &str, mut i: usize) -> usize {
        while i > 0 && !t.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    proptest! {
        /// Round 1.1 / DT3: the converter must accept ANY byte offset —
        /// including mid-UTF-8-sequence ones that real editors never send —
        /// without panicking, and positions must stay monotonic as the
        /// offset grows. Before DT3 this panicked; the delta proptest above
        /// had to snap its offsets to tiptoe around it.
        #[test]
        fn byte_offset_to_position_never_panics_on_any_offset(
            text in "\\PC*",
            offset in any::<usize>(),
        ) {
            let len = text.len();
            let o = offset % (len + 1);
            let p = helpers::byte_offset_to_position(&text, o);
            if o < len {
                let p_next = helpers::byte_offset_to_position(&text, o + 1);
                prop_assert!((p_next.line, p_next.character) >= (p.line, p.character));
            }
        }
    }

    // Fix 3, R1: the byte/line deltas returned by `apply_document_changes`
    // must equal the deltas computed by comparing the pre- and post-edit
    // texts directly. This is the one place the classify-before-apply
    // reorder can silently break, so it gets a property test (plan §5.4).
    //
    // Change batches are constructed stepwise (each change's LSP positions
    // are derived from the text state AFTER the previous changes, matching
    // LSP incremental-sync semantics). Ranges are clamped to the evolving
    // text, which keeps every generated batch valid while still exercising
    // multi-change batches, shrinks, growth, and Unicode.
    proptest! {
        #[test]
        fn apply_document_changes_delta_matches_text_diff(
            base in "\\PC*",
            edits in prop::collection::vec((any::<usize>(), any::<usize>(), "\\PC*"), 0..6),
            full_replace in any::<bool>(),
        ) {
            let (mut inner, uri) = build_state(&base);

            // Stepwise: build change events against the evolving text.
            let mut evolving = base.clone();
            let mut changes: Vec<TextDocumentContentChangeEvent> = Vec::new();
            for (s, e, t) in edits {
                let len = evolving.len();
                let start = snap_to_boundary(&evolving, s % (len + 1));
                let end = snap_to_boundary(&evolving, start + (e % (len + 1 - start)));
                let sp = helpers::byte_offset_to_position(&evolving, start);
                let ep = helpers::byte_offset_to_position(&evolving, end);
                changes.push(TextDocumentContentChangeEvent {
                    range: Some(LspRange::new(sp, ep)),
                    range_length: None,
                    text: t.clone(),
                });
                evolving = format!("{}{}{}", &evolving[..start], t, &evolving[end..]);
            }
            if full_replace && !evolving.is_empty() {
                changes = vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: evolving.clone(),
                }];
            }

            let expected_byte_delta = evolving.len() as isize - base.len() as isize;
            let expected_line_delta = newlines(&evolving) - newlines(&base);

            let (after, byte_delta, line_delta) =
                apply_document_changes(&mut inner, &uri, 2, changes);

            prop_assert_eq!(after, evolving);
            prop_assert_eq!(byte_delta, expected_byte_delta);
            prop_assert_eq!(line_delta, expected_line_delta);
        }
    }

    /// Fix 3, R3+R4: for a WithinPassage edit, the workspace document the
    /// incremental path produces (splice into the caller's pre-edit
    /// snapshot, `ParseResult.passages` left empty) must match the document
    /// a full re-parse of the same final text produces — same passage set,
    /// same offsets, same spans, same header lines, same snapshot names.
    /// This pins the ownership refactor to the pre-existing contract
    /// (plan §5.4).
    #[test]
    fn incremental_splice_equivalence_with_full_reparse() {
        let src = ":: Start\nHello $name, <<set $x to 1>> and [[Next]].\n\n\
                   :: Next\nBody two <<if $x>>yes<<else>>no<</if>> tail.\n\n\
                   :: Third\n[[Next|Start]] the end.\n";
        let (mut inner, uri) = build_state(src);

        // Insert one char at the start of `Next`'s body (line 4, col 0).
        let changes = vec![change(4, 0, 4, 0, "x")];
        let r = did_change_phase1(&mut inner, uri.clone(), 2, changes);
        assert!(
            matches!(r.dispatch, DidChangeDispatch::Incremental { .. }),
            "fixture should take the incremental path, got {:?}",
            r.dispatch
        );
        let inc_doc = inner.workspace.get_document(&uri).unwrap().clone();
        let edited_text = inner.open_documents[&uri].clone();

        // Same final text, delivered as a full-text replacement.
        let (mut inner2, uri2) = build_state(src);
        let full = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: edited_text.clone(),
        }];
        let r2 = did_change_phase1(&mut inner2, uri2.clone(), 2, full);
        assert!(matches!(r2.dispatch, DidChangeDispatch::WholeDocument));
        let full_doc = inner2.workspace.get_document(&uri2).unwrap().clone();

        assert_eq!(
            inc_doc.passages.len(),
            full_doc.passages.len(),
            "passage count must match between incremental and full re-parse"
        );
        for (a, b) in inc_doc.passages.iter().zip(&full_doc.passages) {
            assert_eq!(a.name, b.name, "passage name mismatch");
            assert_eq!(
                a.passage_offset, b.passage_offset,
                "passage_offset mismatch for {:?}",
                a.name
            );
            assert_eq!(a.span, b.span, "span mismatch for {:?}", a.name);
            assert_eq!(a.line, b.line, "header line mismatch for {:?}", a.name);
        }

        // The snapshot's rope must be the post-edit text and its
        // passage_names must match the document's passages (order included).
        let snap = inc_doc
            .snapshot
            .as_ref()
            .expect("incremental doc has snapshot");
        assert_eq!(snap.rope.to_string(), edited_text);
        assert_eq!(
            snap.passage_names,
            inc_doc
                .passages
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// Fix 3, R2: the text-cache insert moved from the top of phase 1 to its
    /// end. This test pins the visibility contract against the reorder:
    /// after `did_change_phase1` returns, `open_documents[uri]` must equal
    /// the post-change text for every dispatch kind the fixture can reach.
    ///
    /// `IncrementalPanic` was not injectable with the default registry
    /// when this test was written; since round 1.1 it IS injectable via a
    /// fault-injecting plugin — see
    /// [`Self::phase1_contains_incremental_panic_and_evicts_panicked_tokens`]
    /// below, which covers the panic arm of this same contract.
    #[test]
    fn phase1_text_cache_visible_after_all_dispatch_kinds() {
        let src = ":: Alpha\nalpha body [[Beta]]\n\n:: Beta\nbeta body\n";
        let cases: &[(&str, u32, u32, u32, u32, &str, &str)] = &[
            // (label, line, char, line, char, inserted, expected dispatch)
            ("incremental", 1, 0, 1, 0, "x", "Incremental"),
            (
                "incremental_fallback",
                0,
                2,
                0,
                2,
                "A",
                "IncrementalFallback",
            ),
            (
                "boundary_crossing",
                2,
                0,
                3,
                0,
                ":: Gamma\n",
                "BoundaryCrossing",
            ),
        ];
        for (label, sl, sc, el, ec, inserted, want) in cases {
            let (mut inner, uri) = build_state(src);
            let before = inner.open_documents[&uri].clone();
            let r = did_change_phase1(
                &mut inner,
                uri.clone(),
                2,
                vec![change(*sl, *sc, *el, *ec, inserted)],
            );
            assert!(
                format!("{:?}", r.dispatch).starts_with(want),
                "{label}: expected {want}, got {:?}",
                r.dispatch
            );
            let after = inner.open_documents[&uri].clone();
            assert_ne!(
                after, before,
                "{label}: cache must reflect the post-change text"
            );
            // The cached text must be the exact expected post-edit text.
            let byte = |line: u32, ch: u32| {
                let text = &before;
                let mut off = 0usize;
                for (i, l) in text.lines().enumerate() {
                    if i as u32 == line {
                        let mut utf16 = 0u32;
                        for c in l.chars() {
                            if utf16 == ch {
                                return off;
                            }
                            utf16 += c.len_utf16() as u32;
                            off += c.len_utf8();
                        }
                        return off;
                    }
                    off += l.len() + 1;
                }
                off
            };
            let s = byte(*sl, *sc);
            let e = byte(*el, *ec);
            let expected = format!("{}{}{}", &before[..s], inserted, &before[e..]);
            assert_eq!(after, expected, "{label}: cached text mismatch");
        }

        // WholeDocument: a full-text replacement.
        let (mut inner, uri) = build_state(src);
        let new_text = ":: Only\nreplaced everything\n".to_string();
        let r = did_change_phase1(
            &mut inner,
            uri.clone(),
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: new_text.clone(),
            }],
        );
        assert!(matches!(r.dispatch, DidChangeDispatch::WholeDocument));
        assert_eq!(inner.open_documents[&uri], new_text);
    }

    // -----------------------------------------------------------------------
    // Round 1.1 / DT2 — IncrementalPanic fault injection
    // -----------------------------------------------------------------------
    //
    // The panic arm of phase 1 (`catch_unwind` in `parse_passage_incremental`
    // → `replace_passage_with_error` → token-group eviction in
    // `merge_incremental_tokens`) was previously "believed correct, not
    // tested correct" — the default registry has no way to make a plugin
    // panic. The wrapper below closes that gap: it forwards every method
    // to a real SugarCube plugin except `parse_passage_mut`, which panics
    // for one armed passage, exercising exactly the containment the
    // production code promises.

    struct PanicInjectingPlugin {
        inner: knot_formats::sugarcube::SugarCubePlugin,
        armed_passage: String,
    }

    impl knot_formats::plugin::FormatPlugin for PanicInjectingPlugin {
        fn format(&self) -> StoryFormat {
            self.inner.format()
        }
        fn special_passages(&self) -> Vec<knot_core::passage::SpecialPassageDef> {
            self.inner.special_passages()
        }
        fn display_name(&self) -> &str {
            self.inner.display_name()
        }
    }

    impl knot_formats::plugin::FormatPluginMut for PanicInjectingPlugin {
        fn parse_mut(&mut self, uri: &url::Url, text: &str) -> knot_formats::plugin::ParseResult {
            self.inner.parse_mut(uri, text)
        }
        fn parse_passage_mut(
            &mut self,
            passage_name: &str,
            passage_tags: &[String],
            passage_text: &str,
            file_uri: &str,
            passage_offset: usize,
        ) -> Option<knot_formats::plugin::ParseResult> {
            if passage_name == self.armed_passage {
                panic!("DT2 fault injection: simulated parser panic in {passage_name:?}");
            }
            self.inner.parse_passage_mut(
                passage_name,
                passage_tags,
                passage_text,
                file_uri,
                passage_offset,
            )
        }
        fn remove_file_from_registries(&mut self, file_uri: &str) {
            self.inner.remove_file_from_registries(file_uri)
        }
        fn remove_passage_from_registries(&mut self, passage_name: &str, file_uri: &str) {
            self.inner
                .remove_passage_from_registries(passage_name, file_uri)
        }
    }

    /// Like [`build_state`], but the registry contains only the
    /// fault-injecting plugin, armed against `armed_passage`. The document
    /// still parses through the real SugarCube code paths (`parse_mut` is
    /// forwarded), so classification sees a normal SugarCube workspace.
    fn build_panicking_state(
        src: &str,
        armed_passage: &str,
    ) -> (crate::state::ServerStateInner, url::Url) {
        let uri = url::Url::parse("file:///project/story.tw").unwrap();
        let mut registry = knot_formats::plugin::FormatRegistry::new();
        registry.register(Box::new(PanicInjectingPlugin {
            inner: knot_formats::sugarcube::SugarCubePlugin::new(),
            armed_passage: armed_passage.to_string(),
        }));
        let format = StoryFormat::SugarCube;

        let parse_result = {
            let plugin = registry
                .get_mut(&format)
                .expect("fault-injecting plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        let mut workspace = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        workspace.config.format = Some("SugarCube".to_string());
        let mut doc = knot_core::Document::new(uri.clone(), StoryFormat::SugarCube);
        doc.set_snapshot_from_text(src);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        workspace.insert_document(doc);

        let inner = crate::state::ServerStateInner {
            workspace,
            format_registry: registry,
            editor_open_docs: std::collections::HashSet::new(),
            open_documents: {
                let mut m = std::collections::HashMap::new();
                m.insert(uri.clone(), src.to_string());
                m
            },
            format_diagnostics: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    uri.clone(),
                    helpers::diagnostic_groups_to_map(parse_result.diagnostic_groups),
                );
                m
            },
            doc_versions: std::collections::HashMap::new(),
            semantic_tokens: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    uri.clone(),
                    helpers::token_groups_to_map(parse_result.token_groups),
                );
                m
            },
            installed_formats: Vec::new(),
            global_storage_path: None,
        };
        (inner, uri)
    }

    #[test]
    fn phase1_contains_incremental_panic_and_evicts_panicked_tokens() {
        let src = ":: Alpha\nalpha body [[Boom]]\n\n:: Boom\nboom body\n\n:: Gamma\ngamma body\n";
        let (mut inner, uri) = build_panicking_state(src, "Boom");

        // Precondition: the cache holds token groups for all three passages.
        {
            let tokens = &inner.semantic_tokens[&uri];
            for name in ["Alpha", "Boom", "Gamma"] {
                assert!(tokens.contains_key(name), "missing token group for {name}");
            }
        }

        // Insert one char at the start of Boom's body (line 4) — a
        // WithinPassage edit that routes into `parse_passage_mut("Boom")`,
        // which the plugin turns into a panic.
        let r = did_change_phase1(&mut inner, uri.clone(), 2, vec![change(4, 0, 4, 0, "x")]);

        // 1. Containment: phase 1 returned, and reports the panic dispatch.
        assert!(
            matches!(&r.dispatch, DidChangeDispatch::IncrementalPanic { passage_name } if passage_name == "Boom"),
            "expected IncrementalPanic(Boom), got {:?}",
            r.dispatch
        );

        // 2. Cache visibility (the gap in the matrix test above): the text
        //    cache is updated even when the passage parse panicked.
        let expected =
            ":: Alpha\nalpha body [[Boom]]\n\n:: Boom\nxboom body\n\n:: Gamma\ngamma body\n";
        assert_eq!(inner.open_documents[&uri], expected);

        // 3. Token eviction: Boom's stale tokens are gone; the untouched
        //    passages' groups survive.
        {
            let tokens = &inner.semantic_tokens[&uri];
            assert!(
                !tokens.contains_key("Boom"),
                "panicked passage tokens must be evicted"
            );
            assert!(tokens.contains_key("Alpha"));
            assert!(tokens.contains_key("Gamma"));
        }

        // 4. Error splice: the passage still exists in the workspace document
        //    (replace_passage_with_error keeps the structure), and a scoped
        //    diagnostic group for Boom was merged into the diagnostics cache.
        let doc = inner
            .workspace
            .get_document(&uri)
            .expect("document survives");
        assert_eq!(doc.passages.len(), 3, "passage structure is preserved");
        assert!(
            inner.format_diagnostics[&uri].contains_key("Boom"),
            "panicked passage must carry a scoped error diagnostic"
        );
    }

    // -----------------------------------------------------------------------
    // Fix 2 (single-flight diagnostics worker) — ordering suite (plan §6.4)
    // -----------------------------------------------------------------------

    /// Recording sink: captures every published batch so the worker's
    /// client-facing behavior can be asserted without a real LSP client.
    #[derive(Clone, Default)]
    struct RecordingSink(std::sync::Arc<std::sync::Mutex<Vec<VersionedDiagnostics>>>);

    impl RecordingSink {
        fn batches(&self) -> Vec<VersionedDiagnostics> {
            self.0.lock().unwrap().clone()
        }
        fn entries_for(&self, uri: &url::Url) -> Vec<Option<i32>> {
            self.batches()
                .iter()
                .flat_map(|b| b.iter().filter(|(u, _, _)| u == uri).map(|(_, v, _)| *v))
                .collect()
        }
    }

    impl DiagnosticsSink for RecordingSink {
        async fn publish(&self, batch: VersionedDiagnostics) {
            self.0.lock().unwrap().push(batch);
        }
    }

    /// Wait until the sink has at least `n` batches, or time out (fails).
    async fn wait_for_batches(sink: &RecordingSink, n: usize) -> bool {
        for _ in 0..200 {
            if sink.batches().len() >= n {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        sink.batches().len() >= n
    }

    fn fast_tunables() -> WorkerTunables {
        WorkerTunables {
            quiet_slice: std::time::Duration::from_millis(5),
            max_lag: std::time::Duration::from_millis(25),
        }
    }

    /// Shared-state fixture: wraps build_state's inner in the Arc<RwLock>
    /// the worker consumes, and returns the ping sender alongside.
    fn shared_state(
        src: &str,
    ) -> (
        std::sync::Arc<tokio::sync::RwLock<crate::state::ServerStateInner>>,
        url::Url,
        tokio::sync::watch::Sender<(url::Url, u64)>,
    ) {
        let (inner, uri) = build_state(src);
        let shared = std::sync::Arc::new(tokio::sync::RwLock::new(inner));
        let (tx, _rx) = tokio::sync::watch::channel((uri.clone(), 0u64));
        (shared, uri, tx)
    }

    fn ping(tx: &tokio::sync::watch::Sender<(url::Url, u64)>, uri: &url::Url) {
        tx.send_modify(|(pinged, stamp)| {
            *pinged = uri.clone();
            *stamp += 1;
        });
    }

    /// §6.4-1 — burst coalescing: N back-to-back keystrokes produce at least
    /// one and far fewer than N publishes, and the final publish carries the
    /// LAST did_change version with diagnostics for the final text.
    #[tokio::test]
    async fn worker_coalesces_keystroke_bursts() {
        let (shared, uri, tx) =
            shared_state(":: Alpha\nalpha body [[Beta]]\n\n:: Beta\nbeta body\n");
        let rx = tx.subscribe();
        let sink = RecordingSink::default();
        let worker = tokio::spawn(run_diagnostics_worker(
            sink.clone(),
            shared.clone(),
            rx,
            fast_tunables(),
            did_change_phase2,
        ));

        // 20 keystrokes back-to-back (each phase 1 + ping, no awaits).
        for k in 0..20u32 {
            let mut guard = shared.write().await;
            did_change_phase1(
                &mut guard,
                uri.clone(),
                2 + k as i32,
                vec![change(1, 0, 1, 0, "x")],
            );
            ping(&tx, &uri);
        }

        // Wait for the tail publish, then stop the worker.
        assert!(
            wait_for_batches(&sink, 1).await,
            "worker must publish at least once after a burst"
        );
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        worker.abort();

        let batches = sink.batches();
        assert!(
            batches.len() < 20,
            "burst of 20 keystrokes must coalesce (got {} publishes)",
            batches.len()
        );
        let versions = sink.entries_for(&uri);
        assert_eq!(
            versions.last(),
            Some(&Some(21)),
            "final publish must carry the last did_change version"
        );
    }

    /// §6.4-2 — tail guarantee: after the last keystroke, a publish
    /// reflecting the final text arrives within ~2*quiet_slice + slack.
    #[tokio::test]
    async fn worker_publishes_final_state_promptly_after_pause() {
        let (shared, uri, tx) = shared_state(":: Alpha\nalpha body\n\n:: Beta\nbeta body\n");
        let rx = tx.subscribe();
        let sink = RecordingSink::default();
        let worker = tokio::spawn(run_diagnostics_worker(
            sink.clone(),
            shared.clone(),
            rx,
            fast_tunables(),
            did_change_phase2,
        ));

        for k in 0..5u32 {
            let mut guard = shared.write().await;
            did_change_phase1(
                &mut guard,
                uri.clone(),
                2 + k as i32,
                vec![change(1, 0, 1, 0, "y")],
            );
            ping(&tx, &uri);
        }

        // quiet_slice=5ms: the tail publish must land well inside 200 ms.
        let ok = tokio::time::timeout(std::time::Duration::from_millis(200), async {
            loop {
                if sink.entries_for(&uri).contains(&Some(6)) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await;
        worker.abort();
        assert!(
            ok.is_ok(),
            "final state (version 6) must publish within the tail window; got {:?}",
            sink.entries_for(&uri)
        );
    }

    /// §6.4-3 — close during pending: a document removed from
    /// `open_documents` (did_close's out-of-workspace path) while a cycle
    /// is pending must not be resurrected by the worker's publish.
    #[tokio::test]
    async fn worker_skips_document_closed_during_pending_cycle() {
        let (shared, uri, tx) = shared_state(":: Alpha\nalpha body\n\n:: Beta\nbeta body\n");
        let rx = tx.subscribe();
        let sink = RecordingSink::default();
        let worker = tokio::spawn(run_diagnostics_worker(
            sink.clone(),
            shared.clone(),
            rx,
            fast_tunables(),
            did_change_phase2,
        ));

        // Cycle 1: document open — its diagnostics ARE published
        // (non-vacuity precondition for the skip assertion below).
        {
            let mut guard = shared.write().await;
            did_change_phase1(&mut guard, uri.clone(), 2, vec![change(1, 0, 1, 0, "x")]);
        }
        ping(&tx, &uri);
        assert!(
            wait_for_batches(&sink, 1).await,
            "precondition: open document is published"
        );
        assert!(!sink.entries_for(&uri).is_empty());

        // Cycle 2: ping, then close (remove from open_documents) BEFORE the
        // worker's analysis runs — the pending cycle must skip the URI.
        {
            let mut guard = shared.write().await;
            did_change_phase1(&mut guard, uri.clone(), 3, vec![change(1, 0, 1, 0, "z")]);
        }
        ping(&tx, &uri);
        {
            let mut guard = shared.write().await;
            guard.open_documents.remove(&uri);
        }

        assert!(
            wait_for_batches(&sink, 2).await,
            "worker must run the second cycle"
        );
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        worker.abort();

        let after_close: Vec<_> = sink
            .batches()
            .iter()
            .skip(1)
            .flat_map(|b| b.iter().map(|(u, _, _)| u.clone()))
            .collect();
        assert!(
            !after_close.contains(&uri),
            "closed document must not be resurrected by a pending cycle"
        );
    }

    /// §6.4-4 — version ordering: versions published for a URI are
    /// monotonically non-decreasing, and a version bump landing DURING the
    /// coalescing window is reflected in that cycle's publish (the client
    /// never accepts a lower version over a higher one).
    #[tokio::test]
    async fn worker_publishes_are_version_ordered() {
        let (shared, uri, tx) = shared_state(":: Alpha\nalpha body\n\n:: Beta\nbeta body\n");
        let rx = tx.subscribe();
        let sink = RecordingSink::default();
        let worker = tokio::spawn(run_diagnostics_worker(
            sink.clone(),
            shared.clone(),
            rx,
            fast_tunables(),
            did_change_phase2,
        ));

        // Cycle 1 at version 2.
        {
            let mut guard = shared.write().await;
            did_change_phase1(&mut guard, uri.clone(), 2, vec![change(1, 0, 1, 0, "x")]);
        }
        ping(&tx, &uri);
        assert!(wait_for_batches(&sink, 1).await);

        // Cycle 2: ping at v3, then immediately bump to v10 (a second
        // did_change) BEFORE the quiet window elapses — the cycle's publish
        // must carry the NEWEST version (10), never the stale 3.
        {
            let mut guard = shared.write().await;
            did_change_phase1(&mut guard, uri.clone(), 3, vec![change(1, 0, 1, 0, "y")]);
        }
        ping(&tx, &uri);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        {
            let mut guard = shared.write().await;
            did_change_phase1(&mut guard, uri.clone(), 10, vec![change(1, 0, 1, 0, "z")]);
        }

        assert!(
            wait_for_batches(&sink, 2).await,
            "second cycle must publish"
        );
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        worker.abort();

        let versions = sink.entries_for(&uri);
        let mut sorted = versions.clone();
        sorted.sort_by_key(|v| v.unwrap_or(0));
        assert_eq!(
            versions, sorted,
            "published versions must be monotonically non-decreasing"
        );
        assert!(
            versions.contains(&Some(10)),
            "the newest version (10) must be the one published for the coalesced cycle, got {versions:?}"
        );
    }

    /// §6.4-5 — worker liveness after panic: a panicking analysis cycle is
    /// contained; the next ping still publishes.
    #[tokio::test]
    async fn worker_survives_analysis_panic() {
        let (shared, uri, tx) = shared_state(":: Alpha\nalpha body\n\n:: Beta\nbeta body\n");
        let rx = tx.subscribe();
        let sink = RecordingSink::default();

        // Inject an analysis that panics on the first call, then delegates
        // to the real phase 2 (a capturing closure — no manual Fn impls).
        let first = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let first_in_closure = first.clone();
        let analyze =
            move |inner: &crate::state::ServerStateInner, uri: &url::Url| -> VersionedDiagnostics {
                if first_in_closure.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    panic!("injected analysis panic (test)");
                }
                did_change_phase2(inner, uri)
            };

        let worker = tokio::spawn(run_diagnostics_worker(
            sink.clone(),
            shared.clone(),
            rx,
            fast_tunables(),
            analyze,
        ));

        // Ping 1: panics — no publish, worker must survive.
        ping(&tx, &uri);
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert!(
            sink.batches().is_empty(),
            "panicking cycle must not publish"
        );

        // Ping 2: real analysis — must still publish.
        ping(&tx, &uri);
        assert!(
            wait_for_batches(&sink, 1).await,
            "worker must stay alive after a contained analysis panic"
        );
        worker.abort();
    }
}
