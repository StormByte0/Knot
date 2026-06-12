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
        let parse_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plugin.parse_mut(uri, text)
        }));

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
