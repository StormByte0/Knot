//! Structured pipeline logging for the SugarCube parse pipeline.
//!
//! This module provides standardized trace events for the 3-phase parse
//! pipeline (structural parse → JS annotation → registry population)
//! and for LSP handler entry/exit points. Using structured events with
//! consistent field names makes it easy to filter and search logs in
//! the VS Code output panel.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::sugarcube::lsp::pipeline_log;
//!
//! // Handler entry/exit
//! pipeline_log::handler_enter("did_open", &uri);
//! // ... do work ...
//! pipeline_log::handler_exit("did_open", &uri);
//!
//! // Pipeline phases
//! pipeline_log::parse_phase1_enter(passage_name, file_uri);
//! pipeline_log::parse_phase2_enter(passage_name, node_count);
//! pipeline_log::parse_phase3_enter(passage_name, var_op_count);
//! ```
//!
//! ## Log levels
//!
//! - `TRACE`: Phase-level details (phase enter/exit for each passage)
//! - `DEBUG`: Handler-level entry/exit, passage counts
//! - `INFO`:  High-level pipeline summary (files parsed, total time)

/// Log a handler entry point.
///
/// Emits a `DEBUG`-level event with the handler name and target URI.
/// Use at the top of every LSP notification/request handler.
#[inline]
pub fn handler_enter(handler: &str, uri: &url::Url) {
    tracing::debug!(
        handler = handler,
        uri = %uri,
        "handler: enter"
    );
}

/// Log a handler exit point.
///
/// Emits a `DEBUG`-level event with the handler name, target URI,
/// and optional elapsed time in milliseconds. Use at the bottom of
/// every LSP notification/request handler, or just before the early
/// return.
#[inline]
pub fn handler_exit(handler: &str, uri: &url::Url) {
    tracing::debug!(
        handler = handler,
        uri = %uri,
        "handler: exit"
    );
}

/// Log a handler exit with an elapsed time measurement.
#[inline]
pub fn handler_exit_elapsed(handler: &str, uri: &url::Url, elapsed_ms: u64) {
    tracing::debug!(
        handler = handler,
        uri = %uri,
        elapsed_ms = elapsed_ms,
        "handler: exit"
    );
}

/// Log Phase 1 entry: structural parse.
///
/// Emits a `TRACE`-level event when the SugarCube parser begins
/// processing a passage body.
#[inline]
pub fn parse_phase1_enter(passage_name: &str, file_uri: &str) {
    tracing::trace!(
        phase = 1,
        passage = passage_name,
        file = file_uri,
        "pipeline: structural parse enter"
    );
}

/// Log Phase 1 exit with the number of AST nodes produced.
#[inline]
pub fn parse_phase1_exit(passage_name: &str, node_count: usize) {
    tracing::trace!(
        phase = 1,
        passage = passage_name,
        node_count = node_count,
        "pipeline: structural parse exit"
    );
}

/// Log Phase 2 entry: JS annotation pass.
///
/// Emits a `TRACE`-level event when `js_annotate::annotate_js()`
/// begins walking the AST for nodes with JS content.
#[inline]
pub fn parse_phase2_enter(passage_name: &str, js_node_count: usize) {
    tracing::trace!(
        phase = 2,
        passage = passage_name,
        js_nodes = js_node_count,
        "pipeline: JS annotation enter"
    );
}

/// Log Phase 2 exit with the number of var_ops produced.
#[inline]
pub fn parse_phase2_exit(passage_name: &str, total_var_ops: usize) {
    tracing::trace!(
        phase = 2,
        passage = passage_name,
        var_ops = total_var_ops,
        "pipeline: JS annotation exit"
    );
}

/// Log Phase 3 entry: unified registry population.
///
/// Emits a `TRACE`-level event when `populate_registries_from_unified_ast()`
/// begins its walk over the enriched AST.
#[inline]
pub fn parse_phase3_enter(passage_name: &str, file_uri: &str) {
    tracing::trace!(
        phase = 3,
        passage = passage_name,
        file = file_uri,
        "pipeline: registry populate enter"
    );
}

/// Log Phase 3 exit with counts of variables, macros, functions, and templates
/// that were registered.
#[inline]
pub fn parse_phase3_exit(
    passage_name: &str,
    var_count: usize,
    macro_count: usize,
    function_count: usize,
    template_count: usize,
) {
    tracing::trace!(
        phase = 3,
        passage = passage_name,
        vars = var_count,
        macros = macro_count,
        functions = function_count,
        templates = template_count,
        "pipeline: registry populate exit"
    );
}

/// Log a full pipeline summary for a file parse.
///
/// Emits an `INFO`-level event after `parse_full()` completes,
/// with aggregate metrics for the entire file.
#[inline]
pub fn parse_full_summary(file_uri: &str, passage_count: usize, token_count: usize, diagnostic_count: usize) {
    tracing::info!(
        file = file_uri,
        passages = passage_count,
        tokens = token_count,
        diagnostics = diagnostic_count,
        "pipeline: parse_full complete"
    );
}

/// Log a semantic token cache operation.
///
/// Emits a `DEBUG`-level event when tokens are stored or retrieved
/// from the server's semantic token cache.
#[inline]
pub fn token_cache_store(uri: &url::Url, token_count: usize) {
    tracing::debug!(
        uri = %uri,
        token_count = token_count,
        "cache: semantic tokens stored"
    );
}

/// Log a semantic token cache hit/miss.
#[inline]
pub fn token_cache_lookup(uri: &url::Url, hit: bool) {
    tracing::debug!(
        uri = %uri,
        hit = hit,
        "cache: semantic tokens lookup"
    );
}

/// Log a graph surgery operation.
///
/// Emits a `DEBUG`-level event with the results of incremental
/// graph surgery (added, removed, modified passages).
#[inline]
pub fn graph_surgery_result(added: usize, removed: usize, modified: usize, total_nodes: usize, total_edges: usize) {
    tracing::debug!(
        added = added,
        removed = removed,
        modified = modified,
        total_nodes = total_nodes,
        total_edges = total_edges,
        "graph: surgery result"
    );
}

/// Log a format switch cascade event.
///
/// Emits a `INFO`-level event when a format switch cascade begins
/// or completes.
#[inline]
pub fn format_switch(event: &str, format: &str, document_count: usize) {
    tracing::info!(
        event = event,
        format = format,
        documents = document_count,
        "format_switch"
    );
}

/// Log a debounced refresh event.
///
/// Emits a `DEBUG`-level event when a debounced semantic token
/// refresh is scheduled or skipped (coalesced).
#[inline]
pub fn debounced_refresh(action: &str) {
    tracing::debug!(
        action = action,
        "refresh: debounced semantic tokens"
    );
}
