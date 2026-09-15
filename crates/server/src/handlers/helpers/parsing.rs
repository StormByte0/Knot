//! Format plugin parsing and StoryData extraction.

use knot_core::passage::StoryFormat;
use knot_core::workspace::StoryMetadata;
use knot_core::{Document, Workspace};
use knot_formats::plugin as fmt_plugin;
use url::Url;

/// Parse a document using the format plugin system.
///
/// Returns both the constructed `Document` and the `ParseResult` (which
/// includes format-specific diagnostics and semantic tokens).
///
/// Falls back to the Core format plugin if the requested format plugin is not
/// available. The Core plugin provides base Twine engine behavior (passage
/// headers, links, core special passages) with no format-specific features.
///
/// ## `.js` files
///
/// When the URI ends with `.js`, the document is parsed as a standalone
/// script file via `SugarCubePlugin::parse_script_file()` (only when
/// `format == StoryFormat::SugarCube`). This matches Tweego's behavior of
/// bundling `.js` files from the source directory as `<script>` tags. For
/// non-SugarCube formats, `.js` files are not parsed by Knot (VS Code's
/// built-in JS language features handle them).
///
/// ## Panic safety
///
/// The format plugin's `parse_mut()` method is wrapped in `std::panic::catch_unwind`
/// to prevent a panic in any format parser from killing the entire server
/// process. If a panic occurs, an empty document with a diagnostic warning
/// is returned instead, and the error is logged.
pub(crate) fn parse_with_format_plugin(
    registry: &mut fmt_plugin::FormatRegistry,
    uri: &Url,
    text: &str,
    format: StoryFormat,
    version: i32,
) -> (Document, fmt_plugin::ParseResult) {
    // ── .js file dispatch ────────────────────────────────────────────
    //
    // Standalone .js files are parsed as synthetic script passages by
    // the active format plugin. The plugin's `parse_script_file_mut`
    // method handles the JS analysis (annotate, validate, registry
    // populate). If the active format doesn't support standalone script
    // files (returns None), the file is stored as an empty document.
    if is_javascript_file(uri) {
        return parse_js_file(registry, uri, text, format, version);
    }

    let plugin = match registry.get_mut(&format) {
        Some(p) => Some(p),
        None => {
            let default = StoryFormat::default_format();
            registry.get_mut(&default)
        }
    };

    if let Some(plugin) = plugin {
        // Wrap the parse call in catch_unwind to prevent panics in format
        // parsers from crashing the server. This is the primary defense
        // against EPIPE errors caused by the server process dying.
        let parse_result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.parse_mut(uri, text)));

        match parse_result {
            Ok(result) => {
                let mut doc = Document::new(uri.clone(), format);
                doc.version = version;
                doc.passages = result.passages.clone();
                doc.set_snapshot_from_text(text);
                (doc, result)
            }
            Err(panic_payload) => {
                // Log the panic without crashing
                let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!(
                    "Format plugin {:?} panicked while parsing {}: {}",
                    format,
                    uri,
                    panic_msg
                );

                // Return an empty document with a diagnostic warning
                let mut doc = Document::new(uri.clone(), format);
                doc.version = version;
                doc.set_snapshot_from_text(text);

                let result = fmt_plugin::ParseResult {
                    passages: Vec::new(),
                    token_groups: Vec::new(),
                    diagnostic_groups: vec![fmt_plugin::PassageDiagnosticGroup {
                        passage_name: String::new(),
                        passage_offset: 0,
                        diagnostics: vec![fmt_plugin::FormatDiagnostic {
                            range: 0..text.len().min(1),
                            message: format!("Internal error: parser panicked — {}", panic_msg),
                            severity: fmt_plugin::FormatDiagnosticSeverity::Error,
                            code: "knot-panic".to_string(),
                        }],
                    }],
                    is_complete: false,
                };
                (doc, result)
            }
        }
    } else {
        // No plugin available — create an empty document
        tracing::warn!("No format plugin available for {:?}", format);
        let mut doc = Document::new(uri.clone(), format);
        doc.set_snapshot_from_text(text);
        let result = fmt_plugin::ParseResult {
            passages: Vec::new(),
            token_groups: Vec::new(),
            diagnostic_groups: Vec::new(),
            is_complete: false,
        };
        (doc, result)
    }
}

/// After parsing a document, check if it contains a `StoryData` passage.
/// If so, parse its JSON body and set `workspace.metadata`.
pub(crate) fn extract_and_set_metadata(workspace: &mut Workspace, doc: &Document, text: &str) {
    if let Some(story_data) = doc.story_data() {
        // Extract the body text of the StoryData passage.
        // The passage span covers the entire passage (header + body).
        // We need to find the body portion after the header line.
        let body_text = extract_passage_body(text, story_data.abs_offset(story_data.span.start));

        if let Some(metadata) = parse_story_data_json(&body_text) {
            tracing::info!(
                "Found StoryData: format={:?}, start={}",
                metadata.format,
                metadata.start_passage
            );
            workspace.metadata = Some(metadata);
        }
    }
}

/// Extract the body text of a passage given the byte offset where the
/// passage starts (the `::` header line). The body starts after the first
/// newline following the header.
pub(crate) fn extract_passage_body(full_text: &str, passage_start: usize) -> String {
    let remainder = if passage_start < full_text.len() {
        &full_text[passage_start..]
    } else {
        return String::new();
    };

    // Skip the header line (everything up to and including the first newline)
    if let Some(newline_pos) = remainder.find('\n') {
        remainder[newline_pos + 1..].to_string()
    } else {
        // No body
        String::new()
    }
}

/// Parse the JSON body of a StoryData passage.
///
/// The StoryData body in Twee 3 looks like:
/// ```json
/// {
///   "ifid": "A1B2C3D4-E5F6-7890-1234-567890ABCDEF",
///   "format": "SugarCube",
///   "format-version": "2.36.1",
///   "start": "Prologue"
/// }
/// ```
///
/// If the "format" field is missing, empty, or unrecognized, falls back to
/// `StoryFormat::Core` (base Twine engine, no format-specific features).
pub(crate) fn parse_story_data_json(body: &str) -> Option<StoryMetadata> {
    // Find the first `{` in the body — skip any leading whitespace or tags
    let json_start = body.find('{')?;
    let json_text = &body[json_start..];

    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;

    let format = value
        .get("format")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<StoryFormat>().ok())
        .unwrap_or_else(StoryFormat::default_format);

    let format_version = value
        .get("format-version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let start_passage = value
        .get("start")
        .and_then(|v| v.as_str())
        .unwrap_or("Start")
        .to_string();

    let ifid = value
        .get("ifid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(StoryMetadata {
        format,
        format_version,
        start_passage,
        ifid,
    })
}

// ---------------------------------------------------------------------------
// .js file support
// ---------------------------------------------------------------------------

/// Check if a URI refers to a `.js` file.
fn is_javascript_file(uri: &Url) -> bool {
    uri.path()
        .rsplit('.')
        .next()
        .map(|ext| ext.eq_ignore_ascii_case("js"))
        .unwrap_or(false)
}

/// Parse a `.js` file as a synthetic script passage via the active format plugin.
///
/// Calls `FormatPluginMut::parse_script_file_mut` on the active format's
/// plugin. The plugin analyzes the file identically to a `[script]`-tagged
/// passage — the only difference is that the entire file text is passed to
/// oxc instead of just the passage body. This means:
///
/// - The SugarCube preprocessor runs (`$var` → `State.variables.var`,
///   keyword operators `to`/`is`/`eq` → JS equivalents)
/// - `Macro.add()`, `Template.add()`, `function` declarations, and
///   `State.variables` writes are registered in the workspace registries
/// - JS tokens are emitted for syntax highlighting
/// - JS diagnostics are produced from oxc
///
/// If the active format doesn't support standalone script files (returns
/// `None` from `parse_script_file_mut`), the file is stored as an empty
/// document — VS Code's built-in JS language features handle it.
///
/// Wrapped in `catch_unwind` for panic safety — same as `parse_mut`.
fn parse_js_file(
    registry: &mut fmt_plugin::FormatRegistry,
    uri: &Url,
    text: &str,
    format: StoryFormat,
    version: i32,
) -> (Document, fmt_plugin::ParseResult) {
    // Use the active format's plugin, falling back to the default format
    // if the requested format isn't registered — same pattern as parse_mut.
    let plugin = match registry.get_mut(&format) {
        Some(p) => p,
        None => {
            let default = StoryFormat::default_format();
            match registry.get_mut(&default) {
                Some(p) => p,
                None => {
                    tracing::warn!("No format plugin available to parse .js file {}", uri);
                    return empty_document(uri, text, format, version);
                }
            }
        }
    };

    // Wrap in catch_unwind — same panic safety as parse_mut.
    let parse_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        plugin.parse_script_file_mut(uri, text)
    }));

    match parse_result {
        Ok(Some(result)) => {
            let mut doc = Document::new(uri.clone(), format);
            doc.version = version;
            doc.passages = result.passages.clone();
            doc.set_snapshot_from_text(text);
            (doc, result)
        }
        Ok(None) => {
            // Plugin doesn't support standalone script files — return empty.
            empty_document(uri, text, format, version)
        }
        Err(panic_payload) => {
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(
                "Format plugin {:?} panicked while parsing .js file {}: {}",
                format,
                uri,
                panic_msg
            );
            empty_document(uri, text, format, version)
        }
    }
}

/// Create an empty document with the given text snapshot.
///
/// Used when a `.js` file is in a non-SugarCube workspace, or when the
/// SugarCube plugin is not available.
fn empty_document(
    uri: &Url,
    text: &str,
    format: StoryFormat,
    version: i32,
) -> (Document, fmt_plugin::ParseResult) {
    let mut doc = Document::new(uri.clone(), format);
    doc.version = version;
    doc.set_snapshot_from_text(text);
    let result = fmt_plugin::ParseResult {
        passages: Vec::new(),
        token_groups: Vec::new(),
        diagnostic_groups: Vec::new(),
        is_complete: false,
    };
    (doc, result)
}

// ===========================================================================
// Incremental single-passage parsing (M2)
// ===========================================================================

/// Convert a `Vec<PassageDiagnosticGroup>` (from a `ParseResult`) into the
/// `HashMap<String, PassageDiagnosticGroup>` shape used by the server's
/// `format_diagnostics` cache (M3).
///
/// Keyed by `passage_name`. If the input Vec contains two groups with the
/// same name (shouldn't happen in practice — passage names are unique within
/// a file), the later one wins.
pub fn diagnostic_groups_to_map(
    groups: Vec<fmt_plugin::PassageDiagnosticGroup>,
) -> std::collections::HashMap<String, fmt_plugin::PassageDiagnosticGroup> {
    groups
        .into_iter()
        .map(|g| (g.passage_name.clone(), g))
        .collect()
}

/// Convert a `Vec<PassageTokenGroup>` (from a `ParseResult`) into the
/// `HashMap<String, PassageTokenGroup>` shape used by the server's
/// `semantic_tokens` cache (M3).
///
/// Keyed by `passage_name`. If the input Vec contains two groups with the
/// same name (shouldn't happen in practice), the later one wins.
pub fn token_groups_to_map(
    groups: Vec<fmt_plugin::PassageTokenGroup>,
) -> std::collections::HashMap<String, fmt_plugin::PassageTokenGroup> {
    groups
        .into_iter()
        .map(|g| (g.passage_name.clone(), g))
        .collect()
}

/// Merge a single-passage incremental `ParseResult` into an existing
/// `format_diagnostics` cache entry, replacing the edited passage's group
/// in place (M3 surgical update).
///
/// - `existing`: the current per-URI HashMap (will be mutated).
/// - `single_result`: the incremental parse result (contains one passage's
///   diagnostic group, or one for the panic-error fallback).
///
/// After this call, `existing` has the edited passage's group replaced;
/// all other passages' groups are untouched.
pub fn merge_incremental_diagnostics(
    existing: &mut std::collections::HashMap<String, fmt_plugin::PassageDiagnosticGroup>,
    single_result: &fmt_plugin::ParseResult,
) {
    for group in &single_result.diagnostic_groups {
        existing.insert(group.passage_name.clone(), group.clone());
    }
}

/// Merge a single-passage incremental `ParseResult` into an existing
/// `semantic_tokens` cache entry (M3 surgical update).
///
/// - If the incremental parse succeeded, the edited passage's token group
///   is replaced in place.
/// - If the incremental parse panicked (M2's degraded mode), the edited
///   passage's token group is REMOVED (panic = no tokens emitted for that
///   passage). Other passages' token groups are untouched.
pub fn merge_incremental_tokens(
    existing: &mut std::collections::HashMap<String, fmt_plugin::PassageTokenGroup>,
    single_result: &fmt_plugin::ParseResult,
    is_panic_degraded: bool,
    panicked_passage_name: Option<&str>,
) {
    if is_panic_degraded {
        // Panic path: the single_result has no token groups (the plugin
        // panicked before emitting any). Remove the panicked passage's
        // old token group so we don't show stale tokens for broken JS.
        if let Some(name) = panicked_passage_name {
            existing.remove(name);
        }
        return;
    }
    for group in &single_result.token_groups {
        existing.insert(group.passage_name.clone(), group.clone());
    }
}

/// Error from [`parse_passage_incremental`].
#[derive(Debug)]
pub enum PassageParseError {
    /// No format plugin was registered for the requested format (or the
    /// default format). The caller should fall back to the empty-document
    /// path used by `parse_with_format_plugin`.
    NoPlugin,
    /// `parse_passage_mut` returned `None` — the plugin's passage classifier
    /// could not classify the passage (e.g. a `[widget]` passage lost its
    /// tag). The caller should fall back to a full-file re-parse.
    ClassificationFailed,
    /// The format plugin panicked while parsing this passage. The panic is
    /// contained: other passages in the document are unaffected. The caller
    /// should call [`replace_passage_with_error`] to emit a scoped
    /// diagnostic for this passage and leave the rest of the document
    /// intact.
    Panic(String),
}

/// Incrementally re-parse a single passage with the format plugin.
///
/// This is the per-passage analog of [`parse_with_format_plugin`]. It wraps
/// `plugin.parse_passage_mut` in `catch_unwind` so that a panic in passage A
/// does NOT propagate to passages B/C/D — the headline win of M2.
///
/// ## Arguments
///
/// - `registry`: the format plugin registry (mutably borrowed).
/// - `uri`: the document URI (passed to the plugin for registry bookkeeping).
/// - `format`: the story format to use for parsing.
/// - `_version`: the LSP document version (unused by plugins, but kept for
///   symmetry with `parse_with_format_plugin`).
/// - `passage_name`, `passage_tags`, `passage_text`: the passage to re-parse.
/// - `passage_offset`: the document-absolute byte offset of the passage's
///   `::` header. The plugin sets `passage.passage_offset` from this.
///
/// ## Returns
///
/// - `Ok(ParseResult)` on success — a single-passage `ParseResult` containing
///   the re-parsed passage, its diagnostic group, and its token group. Ready
///   to splice into the document-level caches.
/// - `Err(NoPlugin)` — no plugin registered; caller falls back to empty doc.
/// - `Err(ClassificationFailed)` — plugin returned `None`; caller falls back
///   to full-file re-parse.
/// - `Err(Panic(msg))` — plugin panicked; caller calls
///   [`replace_passage_with_error`] to scope the error to this passage.
#[allow(clippy::too_many_arguments)]
pub fn parse_passage_incremental(
    registry: &mut fmt_plugin::FormatRegistry,
    uri: &Url,
    format: StoryFormat,
    _version: i32,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
    passage_offset: usize,
) -> Result<fmt_plugin::ParseResult, PassageParseError> {
    // Two-step lookup to avoid the borrow checker: first try the requested
    // format, then fall back to the default format. We can't use `or_else`
    // with a closure that re-borrows `registry` because the first borrow
    // is still live.
    let plugin = match registry.get_mut(&format) {
        Some(p) => Some(p),
        None => registry.get_mut(&StoryFormat::default_format()),
    };
    let plugin = plugin.ok_or(PassageParseError::NoPlugin)?;

    let parse_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        plugin.parse_passage_mut(
            passage_name,
            passage_tags,
            passage_text,
            uri.as_str(),
            passage_offset,
        )
    }));

    match parse_result {
        Ok(Some(parse_result)) => {
            // Defensive: ensure passage_offset is set correctly on the
            // returned passage, in case a plugin impl forgets to set it.
            if let Some(passage) = parse_result.passages.first() {
                debug_assert_eq!(
                    passage.passage_offset, passage_offset,
                    "plugin did not set passage_offset correctly"
                );
            }

            // P1-4 (study / plan.md Phase 1.6): the incremental contract
            // (see `FormatPlugin::parse_passage_mut`) requires the result to
            // carry this passage's semantic-token groups. The regex-based
            // formats (Harlowe/Chapbook/Snowman) returned empty groups, so
            // merging the incremental result silently degraded highlighting
            // for the edited passage. Report that as a classification
            // failure so the caller falls back to a full re-parse, which
            // produces the real tokens — an honest "full reparse" for the
            // formats that don't support single-passage tokenization.
            if parse_result.token_groups.is_empty() {
                tracing::debug!(
                    format = ?format,
                    passage = passage_name,
                    "parse_passage_incremental: plugin returned no token groups — falling back to full re-parse"
                );
                return Err(PassageParseError::ClassificationFailed);
            }

            Ok(parse_result)
        }
        Ok(None) => Err(PassageParseError::ClassificationFailed),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(
                "Format plugin {:?} panicked while parsing passage {} in {}: {}",
                format,
                passage_name,
                uri,
                msg
            );
            Err(PassageParseError::Panic(msg))
        }
    }
}

/// Replace a single passage's diagnostics with a scoped `sc-parse` error
/// after a per-passage parse panic.
///
/// This is the per-passage analog of the `knot-panic` fallback in
/// [`parse_with_format_plugin`]. The key difference: only the panicking
/// passage is affected. All other passages' tokens, diagnostics, and graph
/// edges are preserved.
///
/// ## What this does
///
/// 1. Locates the passage in `doc.passages` by name.
/// 2. Returns a `PassageDiagnosticGroup` containing a single `sc-parse`
///    Error diagnostic whose span covers the passage's full span
///    (passage-relative `0..passage.span.end`) with the panic message.
/// 3. Does NOT mutate `doc.passages` — the passage struct (links, vars,
///    blocks, graph edges) is preserved from the previous parse. Only the
///    diagnostic group is replaced.
///
/// ## Returns
///
/// `Some(PassageDiagnosticGroup)` if the passage was found, `None` if not
/// (in which case the caller should fall back to full-file re-parse).
pub fn replace_passage_with_error(
    doc: &Document,
    passage_name: &str,
    panic_msg: &str,
) -> Option<fmt_plugin::PassageDiagnosticGroup> {
    let passage = doc.passages.iter().find(|p| p.name == passage_name)?;
    let passage_span_end = passage.span.end.max(1);

    Some(fmt_plugin::PassageDiagnosticGroup {
        passage_name: passage_name.to_string(),
        passage_offset: passage.passage_offset,
        diagnostics: vec![fmt_plugin::FormatDiagnostic {
            range: 0..passage_span_end,
            message: format!("Internal error: passage parse panicked — {}", panic_msg),
            severity: fmt_plugin::FormatDiagnosticSeverity::Error,
            code: "sc-parse".to_string(),
        }],
    })
}

// ===========================================================================
// Tests (M3)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::passage::StoryFormat;
    use std::collections::HashMap;
    use url::Url;

    /// Build a `PassageDiagnosticGroup` for testing.
    fn diag_group(name: &str, offset: usize, msg: &str) -> fmt_plugin::PassageDiagnosticGroup {
        fmt_plugin::PassageDiagnosticGroup {
            passage_name: name.to_string(),
            passage_offset: offset,
            diagnostics: vec![fmt_plugin::FormatDiagnostic {
                range: 0..1,
                message: msg.to_string(),
                severity: fmt_plugin::FormatDiagnosticSeverity::Error,
                code: "sc-parse".to_string(),
            }],
        }
    }

    /// Build a `PassageTokenGroup` for testing.
    fn token_group(name: &str, offset: usize) -> fmt_plugin::PassageTokenGroup {
        fmt_plugin::PassageTokenGroup {
            passage_name: name.to_string(),
            passage_offset: offset,
            tokens: vec![fmt_plugin::SemanticToken {
                start: 0,
                length: 1,
                token_type: fmt_plugin::SemanticTokenType::Variable,
                modifier: None,
            }],
        }
    }

    /// Build a minimal `ParseResult` with the given diagnostic + token groups.
    fn parse_result_with(
        diags: Vec<fmt_plugin::PassageDiagnosticGroup>,
        tokens: Vec<fmt_plugin::PassageTokenGroup>,
    ) -> fmt_plugin::ParseResult {
        fmt_plugin::ParseResult {
            passages: Vec::new(),
            token_groups: tokens,
            diagnostic_groups: diags,
            is_complete: true,
        }
    }

    /// M3 acceptance test: `merge_incremental_diagnostics` replaces only the
    /// edited passage's entry; other passages' entries are byte-for-byte
    /// unchanged.
    #[test]
    fn merge_incremental_diagnostics_replaces_only_edited_passage() {
        // Start with two passages' diagnostics cached.
        let mut existing: HashMap<String, fmt_plugin::PassageDiagnosticGroup> = HashMap::new();
        existing.insert("A".to_string(), diag_group("A", 0, "old A"));
        existing.insert("B".to_string(), diag_group("B", 100, "old B"));

        // Snapshot the original B entry — we'll assert it's unchanged.
        let original_b = existing.get("B").cloned().unwrap();

        // Simulate an incremental re-parse of passage A that produces a new
        // diagnostic group for A only.
        let single_result = parse_result_with(vec![diag_group("A", 0, "new A")], Vec::new());

        merge_incremental_diagnostics(&mut existing, &single_result);

        // A was replaced.
        assert_eq!(existing.get("A").unwrap().diagnostics[0].message, "new A");
        // B is byte-for-byte unchanged.
        assert_eq!(existing.get("B").unwrap(), &original_b);
        // Still exactly 2 entries.
        assert_eq!(existing.len(), 2);
    }

    /// M3 acceptance test: `merge_incremental_tokens` on success replaces
    /// only the edited passage's token group; other passages' groups are
    /// untouched.
    #[test]
    fn merge_incremental_tokens_success_replaces_only_edited_passage() {
        let mut existing: HashMap<String, fmt_plugin::PassageTokenGroup> = HashMap::new();
        existing.insert("A".to_string(), token_group("A", 0));
        let original_b = token_group("B", 100);
        existing.insert("B".to_string(), original_b.clone());

        // Incremental re-parse of A produces a new token group for A.
        let new_a = fmt_plugin::PassageTokenGroup {
            passage_name: "A".to_string(),
            passage_offset: 0,
            tokens: vec![fmt_plugin::SemanticToken {
                start: 5,
                length: 3,
                token_type: fmt_plugin::SemanticTokenType::Function,
                modifier: None,
            }],
        };
        let single_result = parse_result_with(Vec::new(), vec![new_a]);

        merge_incremental_tokens(&mut existing, &single_result, false, None);

        // A was replaced with the new token.
        assert_eq!(existing.get("A").unwrap().tokens[0].start, 5);
        assert_eq!(existing.get("A").unwrap().tokens[0].length, 3);
        // B is unchanged.
        assert_eq!(existing.get("B").unwrap(), &original_b);
        assert_eq!(existing.len(), 2);
    }

    /// M3 acceptance test: `merge_incremental_tokens` on panic-degraded mode
    /// REMOVES the panicked passage's token group (no tokens for broken JS);
    /// other passages' groups are untouched.
    #[test]
    fn merge_incremental_tokens_panic_removes_panicked_passage() {
        let mut existing: HashMap<String, fmt_plugin::PassageTokenGroup> = HashMap::new();
        existing.insert("A".to_string(), token_group("A", 0));
        let original_b = token_group("B", 100);
        existing.insert("B".to_string(), original_b.clone());

        // Panic-degraded mode: the plugin panicked, no token groups emitted.
        let single_result = parse_result_with(Vec::new(), Vec::new());

        merge_incremental_tokens(&mut existing, &single_result, true, Some("A"));

        // A was removed (no tokens for broken JS).
        assert!(!existing.contains_key("A"));
        // B is unchanged.
        assert_eq!(existing.get("B").unwrap(), &original_b);
        assert_eq!(existing.len(), 1);
    }

    /// M3 acceptance test: `diagnostic_groups_to_map` produces a HashMap
    /// keyed by passage name from a Vec of groups.
    #[test]
    fn diagnostic_groups_to_map_keys_by_passage_name() {
        let groups = vec![
            diag_group("A", 0, "A error"),
            diag_group("B", 100, "B error"),
        ];
        let map = diagnostic_groups_to_map(groups);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("A"));
        assert!(map.contains_key("B"));
        assert_eq!(map.get("A").unwrap().passage_offset, 0);
        assert_eq!(map.get("B").unwrap().passage_offset, 100);
    }

    /// M3 acceptance test: `token_groups_to_map` produces a HashMap keyed
    /// by passage name from a Vec of groups.
    #[test]
    fn token_groups_to_map_keys_by_passage_name() {
        let groups = vec![token_group("A", 0), token_group("B", 100)];
        let map = token_groups_to_map(groups);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("A"));
        assert!(map.contains_key("B"));
        assert_eq!(map.get("A").unwrap().passage_offset, 0);
        assert_eq!(map.get("B").unwrap().passage_offset, 100);
    }

    /// M3 acceptance test: `replace_passage_with_error` produces a scoped
    /// diagnostic group covering the passage's span.
    #[test]
    fn replace_passage_with_error_scopes_to_passage_span() {
        use knot_core::passage::Passage;

        let mut passage = Passage::new("A".to_string(), 0..50);
        passage.passage_offset = 100;
        let mut doc = Document::new(Url::parse("file:///test.tw").unwrap(), StoryFormat::Core);
        doc.passages.push(passage);

        let group = replace_passage_with_error(&doc, "A", "boom").unwrap();
        assert_eq!(group.passage_name, "A");
        assert_eq!(group.passage_offset, 100);
        assert_eq!(group.diagnostics.len(), 1);
        assert_eq!(
            group.diagnostics[0].severity,
            fmt_plugin::FormatDiagnosticSeverity::Error
        );
        assert_eq!(group.diagnostics[0].code, "sc-parse");
        assert!(group.diagnostics[0].message.contains("boom"));
        // Span covers the passage (passage-relative 0..50).
        assert_eq!(group.diagnostics[0].range, 0..50);
    }

    /// M3 acceptance test: `replace_passage_with_error` returns None when
    /// the passage is not found.
    #[test]
    fn replace_passage_with_error_returns_none_for_missing_passage() {
        let doc = Document::new(Url::parse("file:///test.tw").unwrap(), StoryFormat::Core);
        let result = replace_passage_with_error(&doc, "missing", "boom");
        assert!(result.is_none());
    }
}
