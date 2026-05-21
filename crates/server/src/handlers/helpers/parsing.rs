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
/// Falls back to the default format if the requested format plugin is not
/// available.
pub(crate) fn parse_with_format_plugin(
    registry: &fmt_plugin::FormatRegistry,
    uri: &Url,
    text: &str,
    format: StoryFormat,
    version: i32,
) -> (Document, fmt_plugin::ParseResult) {
    let plugin = registry
        .get(&format)
        .or_else(|| {
            // Try the default format
            let default = StoryFormat::default_format();
            registry.get(&default)
        });

    if let Some(plugin) = plugin {
        let result = plugin.parse(uri, text);
        let mut doc = Document::new(uri.clone(), format);
        doc.version = version;
        doc.passages = result.passages.clone();
        doc.set_snapshot_from_text(text);
        (doc, result)
    } else {
        // No plugin available — create an empty document
        tracing::warn!("No format plugin available for {:?}", format);
        let mut doc = Document::new(uri.clone(), format);
        doc.set_snapshot_from_text(text);
        let result = fmt_plugin::ParseResult {
            passages: Vec::new(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
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
        let body_text = extract_passage_body(text, story_data.span.start);

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
pub(crate) fn parse_story_data_json(body: &str) -> Option<StoryMetadata> {
    // Find the first `{` in the body — skip any leading whitespace or tags
    let json_start = body.find('{')?;
    let json_text = &body[json_start..];

    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;

    let format_str = value
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("SugarCube"); // Fallback when JSON has no format field
    let format = format_str
        .parse::<StoryFormat>()
        .unwrap_or_else(|_| StoryFormat::default_format());

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
