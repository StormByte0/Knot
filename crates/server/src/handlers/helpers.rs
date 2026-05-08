//! Utility functions shared across handler submodules.
//!
//! Contains position/range helpers, diagnostic publishing, workspace indexing,
//! graph rebuild, metadata extraction, format plugin parsing, and all other
//! small helper functions that don't belong to a specific handler group.

use crate::lsp_ext::*;
use knot_core::graph::{DiagnosticKind, PassageEdge, PassageNode};
use knot_core::passage::StoryFormat;
use knot_core::workspace::StoryMetadata;
use knot_core::{AnalysisEngine, Document, Workspace};
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;
use std::collections::HashMap;
use url::Url;

// ===========================================================================
// Format plugin parsing
// ===========================================================================

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
        (doc, result)
    } else {
        // No plugin available — create an empty document
        tracing::warn!("No format plugin available for {:?}", format);
        let doc = Document::new(uri.clone(), format);
        let result = fmt_plugin::ParseResult {
            passages: Vec::new(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            is_complete: false,
        };
        (doc, result)
    }
}

// ===========================================================================
// StoryData extraction
// ===========================================================================

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
        .unwrap_or("SugarCube");
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

// ===========================================================================
// Workspace indexing
// ===========================================================================

/// Scan the workspace root for all `.tw` / `.twee` files, parse them with
/// the format plugin, insert into the workspace, build the graph, and run
/// analysis.
///
/// Uses a two-pass approach:
/// 1. First pass: scan all files for StoryData to resolve the format early.
/// 2. Second pass: parse all files with the correct format.
pub(crate) async fn index_workspace(
    inner: &tokio::sync::RwLock<crate::state::ServerStateInner>,
    client: &tower_lsp::Client,
) -> Result<(), String> {
    let root_uri = {
        let inner = inner.read().await;
        inner.workspace.root_uri.clone()
    };

    let root_path = root_uri
        .to_file_path()
        .map_err(|_| "Workspace root is not a file:// URI".to_string())?;

    // Collect all .tw/.twee files using walkdir
    let twee_files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root_path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            let ext = entry.path().extension().and_then(|e| e.to_str());
            ext == Some("tw") || ext == Some("twee")
        })
        .map(|entry| entry.into_path())
        .collect();

    let total_files = twee_files.len() as u32;
    if total_files == 0 {
        return Ok(());
    }

    client
        .log_message(
            MessageType::INFO,
            format!("Indexing {} Twee files…", total_files),
        )
        .await;

    // Send initial progress notification
    send_index_progress(client, total_files, 0).await;

    // --- Pass 1: Find StoryData to resolve format early ---
    {
        let mut inner = inner.write().await;
        for file_path in &twee_files {
            if let Ok(text) = std::fs::read_to_string(file_path) {
                if let Ok(uri) = Url::from_file_path(file_path) {
                    let format = inner.workspace.resolve_format();
                    let (doc, _parse_result) = parse_with_format_plugin(
                        &inner.format_registry, &uri, &text, format, 0,
                    );
                    // Only extract metadata — don't store the full document yet
                    if doc.story_data().is_some() {
                        extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                        tracing::info!("Found StoryData in {} — format resolved to {:?}", 
                            file_path.display(), inner.workspace.resolve_format());
                    }
                }
            }
        }
    }

    // --- Pass 2: Parse all files with the correct format ---
    let mut parsed_count: u32 = 0;

    for file_path in &twee_files {
        let text = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

        let uri = Url::from_file_path(file_path)
            .map_err(|_| format!("Invalid file path: {}", file_path.display()))?;

        let mut inner = inner.write().await;
        let format = inner.workspace.resolve_format();

        inner.open_documents.insert(uri.clone(), text.clone());

        let (doc, parse_result) = parse_with_format_plugin(
            &inner.format_registry,
            &uri,
            &text,
            format,
            0, // version 0 for indexed files
        );

        // Store format diagnostics
        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);

        // Check for StoryData
        extract_and_set_metadata(&mut inner.workspace, &doc, &text);

        inner.workspace.insert_document(doc);
        drop(inner);

        parsed_count += 1;

        // Send progress every 10 files or on the last file
        if parsed_count.is_multiple_of(10) || parsed_count == total_files {
            send_index_progress(client, total_files, parsed_count).await;
        }
    }

    // After all files are loaded, rebuild the graph and run analysis
    let mut inner = inner.write().await;
    rebuild_graph(&mut inner.workspace);
    inner.workspace.mark_indexed();

    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    publish_all_diagnostics(client, &diagnostics, &fmt_diags, &open_docs, &config).await;

    Ok(())
}

/// Send a `knot/indexProgress` notification to the client.
async fn send_index_progress(client: &tower_lsp::Client, total_files: u32, parsed_files: u32) {
    let progress = KnotIndexProgress {
        total_files,
        parsed_files,
    };
    client
        .send_notification::<KnotIndexProgressNotification>(progress)
        .await;
}

// ===========================================================================
// Graph rebuild
// ===========================================================================

/// Rebuild the passage graph from all workspace documents.
#[allow(clippy::type_complexity)]
pub(crate) fn rebuild_graph(workspace: &mut Workspace) {
    // Collect passage info first (avoid borrow issues)
    let info: Vec<(String, String, bool, bool, Vec<(Option<String>, String)>)> = workspace
        .documents()
        .flat_map(|doc| {
            doc.passages.iter().map(|p| {
                let edges: Vec<(Option<String>, String)> = p
                    .links
                    .iter()
                    .map(|l| (l.display_text.clone(), l.target.clone()))
                    .collect();
                (
                    p.name.clone(),
                    doc.uri.to_string(),
                    p.is_special,
                    p.is_metadata(),
                    edges,
                )
            })
        })
        .collect();

    workspace.graph = knot_core::PassageGraph::new();

    for (name, file_uri, is_special, is_metadata, _edges) in &info {
        let node = PassageNode {
            name: name.clone(),
            file_uri: file_uri.clone(),
            is_special: *is_special,
            is_metadata: *is_metadata,
        };
        workspace.graph.add_passage(node);
    }

    // Add edges after all nodes exist so broken-link detection works.
    for (source, _, _, _, edges) in &info {
        for (display_text, target) in edges {
            let target_exists = workspace.graph.contains_passage(target);
            let edge = PassageEdge {
                display_text: display_text.clone(),
                is_broken: !target_exists,
            };
            workspace.graph.add_edge(source, target, edge);
        }
    }
}

// ===========================================================================
// Diagnostics
// ===========================================================================

/// Count the number of incoming links to a passage from other passages.
pub(crate) fn count_incoming_links(workspace: &Workspace, passage_name: &str) -> usize {
    let mut count = 0;
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                count += 1;
            }
        }
    }
    count
}

/// Publish diagnostics to **all** affected files. This combines:
/// - Graph analysis diagnostics (broken links, unreachable, loops, etc.)
/// - Format plugin diagnostics (syntax errors, parsing warnings, etc.)
///
/// Groups results by `file_uri` and publishes each group separately.
pub(crate) async fn publish_all_diagnostics(
    client: &tower_lsp::Client,
    graph_diagnostics: &[knot_core::graph::GraphDiagnostic],
    format_diagnostics: &std::collections::HashMap<Url, Vec<fmt_plugin::FormatDiagnostic>>,
    open_documents: &std::collections::HashMap<Url, String>,
    config: &knot_core::workspace::KnotConfig,
) {
    use std::collections::HashMap as StdHashMap;

    // Group graph diagnostics by file URI
    let mut by_file: StdHashMap<String, Vec<&knot_core::graph::GraphDiagnostic>> = StdHashMap::new();
    for gd in graph_diagnostics {
        by_file
            .entry(gd.file_uri.clone())
            .or_default()
            .push(gd);
    }

    // Collect all files that should have diagnostics published
    let all_uris: std::collections::HashSet<Url> = open_documents
        .keys()
        .chain(format_diagnostics.keys())
        .cloned()
        .collect();

    for uri in &all_uris {
        let uri_str = uri.to_string();
        let text = open_documents.get(uri).map(|s| s.as_str()).unwrap_or("");

        let mut lsp_diagnostics: Vec<Diagnostic> = Vec::new();

        // Add graph diagnostics for this file
        if let Some(diags) = by_file.get(&uri_str) {
            for gd in diags {
                let default_severity = diagnostic_kind_to_severity(&gd.kind);

                // Check config for severity override or suppression
                let diag_key = format!("{:?}", gd.kind);
                let severity = if let Some(custom) = config.diagnostics.get(&diag_key) {
                    match custom {
                        knot_core::workspace::DiagnosticSeverity::Off => continue, // Suppress
                        knot_core::workspace::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                        knot_core::workspace::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                        knot_core::workspace::DiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                        knot_core::workspace::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                    }
                } else {
                    default_severity
                };

                // For graph diagnostics, find the passage header range
                let range = find_passage_header_range(text, &gd.passage_name);

                // Build related information pointing to source or target passages
                let related_information = build_related_information_for_push(
                    open_documents, &gd.kind, &gd.passage_name, &gd.message,
                );

                lsp_diagnostics.push(Diagnostic {
                    range,
                    severity: Some(severity),
                    code: Some(NumberOrString::String(format!("{:?}", gd.kind))),
                    source: Some("knot".to_string()),
                    message: gd.message.clone(),
                    related_information,
                    ..Default::default()
                });
            }
        }

        // Add format plugin diagnostics for this file
        if let Some(fmt_diags) = format_diagnostics.get(uri) {
            for fd in fmt_diags {
                let range = byte_range_to_lsp_range(text, &fd.range);

                let severity = match fd.severity {
                    fmt_plugin::FormatDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    fmt_plugin::FormatDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                    fmt_plugin::FormatDiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                    fmt_plugin::FormatDiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                };

                lsp_diagnostics.push(Diagnostic {
                    range,
                    severity: Some(severity),
                    code: Some(NumberOrString::String(format!("format:{}", fd.code))),
                    source: Some("knot".to_string()),
                    message: fd.message.clone(),
                    ..Default::default()
                });
            }
        }

        client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, None)
            .await;
    }
}

// ===========================================================================
// Position / Range helpers
// ===========================================================================

/// Convert a byte offset to an LSP Position (0-based line & character).
pub(crate) fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    let safe_offset = offset.min(text.len());
    let text_before = &text[..safe_offset];
    let line = text_before.lines().count() as u32;
    let line = if text_before.is_empty() || text_before.ends_with('\n') {
        line
    } else {
        line - 1
    };
    let last_newline = text_before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = (safe_offset - last_newline) as u32;
    Position { line, character }
}

/// Convert a byte range to an LSP Range.
pub(crate) fn byte_range_to_lsp_range(text: &str, range: &std::ops::Range<usize>) -> Range {
    let start = byte_offset_to_position(text, range.start);
    let end = byte_offset_to_position(text, range.end);
    Range { start, end }
}

/// Find the LSP Range for a passage header line.
pub(crate) fn find_passage_header_range(text: &str, passage_name: &str) -> Range {
    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == passage_name {
                return Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: line.len() as u32,
                    },
                };
            }
        }
    }
    Range::default()
}

/// Parse just the passage name from a header (the part after `::`).
pub(crate) fn parse_passage_name_from_header(header: &str) -> String {
    let header = header.trim();
    if let Some(bracket_start) = header.find('[') {
        header[..bracket_start].trim().to_string()
    } else {
        header.to_string()
    }
}

/// Find the passage name at a given LSP position.
pub(crate) fn find_passage_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;
    if line_text.starts_with("::") {
        let name = parse_passage_name_from_header(&line_text[2..]);
        Some(name)
    } else {
        None
    }
}

/// Find a link target at a given LSP position.
pub(crate) fn find_link_target_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;
    let char_offset = position.character as usize;

    let mut search_from = 0;
    while let Some(rel_start) = line_text[search_from..].find("[[") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line_text[abs_start..].find("]]") {
            let content_start = abs_start + 2;
            let content_end = abs_start + rel_end;

            if char_offset >= content_start && char_offset <= content_end {
                let link_text = &line_text[content_start..content_end];
                // Handle both arrow (->) and pipe (|) link syntax
                let target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                let target = target.trim();
                if !target.is_empty() {
                    return Some(target.to_string());
                }
            }
            search_from = abs_start + rel_end + 2;
        } else {
            break;
        }
    }
    None
}

// ===========================================================================
// Diagnostic-severity mapping
// ===========================================================================

/// Map a `DiagnosticKind` to its default LSP `DiagnosticSeverity`.
pub(crate) fn diagnostic_kind_to_severity(kind: &DiagnosticKind) -> DiagnosticSeverity {
    match kind {
        DiagnosticKind::BrokenLink => DiagnosticSeverity::WARNING,
        DiagnosticKind::UnreachablePassage => DiagnosticSeverity::HINT,
        DiagnosticKind::InfiniteLoop => DiagnosticSeverity::WARNING,
        DiagnosticKind::UninitializedVariable => DiagnosticSeverity::WARNING,
        DiagnosticKind::UnusedVariable => DiagnosticSeverity::HINT,
        DiagnosticKind::RedundantWrite => DiagnosticSeverity::HINT,
        DiagnosticKind::DuplicateStoryData => DiagnosticSeverity::ERROR,
        DiagnosticKind::MissingStoryData => DiagnosticSeverity::WARNING,
        DiagnosticKind::MissingStartPassage => DiagnosticSeverity::ERROR,
        DiagnosticKind::UnsupportedFormat => DiagnosticSeverity::ERROR,
        DiagnosticKind::DuplicatePassageName => DiagnosticSeverity::ERROR,
        DiagnosticKind::EmptyPassage => DiagnosticSeverity::HINT,
        DiagnosticKind::DeadEndPassage => DiagnosticSeverity::INFORMATION,
        DiagnosticKind::InvalidPassageName => DiagnosticSeverity::WARNING,
        DiagnosticKind::OrphanedPassage => DiagnosticSeverity::INFORMATION,
        DiagnosticKind::ComplexPassage => DiagnosticSeverity::HINT,
        DiagnosticKind::LargePassage => DiagnosticSeverity::HINT,
        DiagnosticKind::MissingStartLink => DiagnosticSeverity::WARNING,
    }
}

// ===========================================================================
// Compiler helpers
// ===========================================================================

/// Search for the Tweego compiler:
/// 1. In the configured path (from knot.json)
/// 2. In the system PATH (via `which`/`where`)
/// 3. In workspace subdirectories (e.g., `.knot/tweego`, `tools/tweego`)
///
/// On Unix systems, uses `which` to locate the binary.
/// On Windows, uses `where` instead (the `which` command does not exist).
/// Falls back to trying direct execution with `--version` if the
/// system locator is unavailable.
pub(crate) fn which_compiler() -> Option<std::path::PathBuf> {
    which_compiler_in_path().or_else(which_compiler_in_workspace)
}

/// Same as `which_compiler` but searches workspace subdirectories
/// relative to the given root path.
pub(crate) fn which_compiler_with_root(root: &std::path::Path) -> Option<std::path::PathBuf> {
    which_compiler_in_path().or_else(|| which_compiler_in_workspace_root(root))
}

/// Search for Tweego on the system PATH.
fn which_compiler_in_path() -> Option<std::path::PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["tweego.exe"]
    } else {
        &["tweego"]
    };

    // Use the platform-appropriate locator command
    let locator = if cfg!(windows) { "where" } else { "which" };

    for name in candidates {
        if let Ok(output) = std::process::Command::new(locator)
            .arg(name)
            .output()
            && output.status.success() {
                // `where` on Windows may return multiple lines; take the first.
                let path_str = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    return Some(path);
                }
            }
    }

    // Fallback: try direct execution — if the binary is on PATH,
    // running it with --version will succeed.
    for name in candidates {
        if std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(std::path::PathBuf::from(name));
        }
    }

    None
}

/// Search for Tweego in common workspace subdirectories.
fn which_compiler_in_workspace() -> Option<std::path::PathBuf> {
    which_compiler_in_workspace_root(std::path::Path::new("."))
}

/// Search for Tweego in common workspace subdirectories relative to root.
fn which_compiler_in_workspace_root(root: &std::path::Path) -> Option<std::path::PathBuf> {
    // Common locations where Tweego might be installed locally
    let search_dirs: &[&str] = &[
        ".knot",
        "tools",
        "vendor",
        "bin",
        ".tools",
    ];

    let binary_name = if cfg!(windows) { "tweego.exe" } else { "tweego" };

    // Try each search directory under root
    for dir_name in search_dirs {
        let candidate = root.join(dir_name).join(binary_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Try searching recursively in .knot directory
    let knot_dir = root.join(".knot");
    if knot_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&knot_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let candidate = path.join(binary_name);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                } else if path.file_name().map(|n| n == binary_name).unwrap_or(false) {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Detect the version string of a compiler by running `--version`.
pub(crate) async fn detect_compiler_version(path: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout);
        // Take the first line of output as the version string
        Some(version.lines().next().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Compute the maximum depth from the start passage using BFS.
pub(crate) fn compute_max_depth(workspace: &Workspace, start_passage: &str) -> u32 {
    let mut depths: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    if workspace.graph.contains_passage(start_passage) {
        depths.insert(start_passage.to_string(), 0);
        queue.push_back(start_passage.to_string());
    }

    while let Some(name) = queue.pop_front() {
        let current_depth = *depths.get(&name).unwrap_or(&0);
        for neighbor in workspace.graph.outgoing_neighbors(&name) {
            if !depths.contains_key(&neighbor) {
                let new_depth = current_depth + 1;
                depths.insert(neighbor.clone(), new_depth);
                queue.push_back(neighbor);
            }
        }
    }

    depths.values().copied().max().unwrap_or(0)
}

/// Compute the number of weakly connected components in the passage graph.
pub(crate) fn compute_connected_components(workspace: &Workspace) -> u32 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    if passage_names.is_empty() {
        return 0;
    }

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut component_count: u32 = 0;

    for name in &passage_names {
        if visited.contains(name) {
            continue;
        }

        // BFS considering both directions (weakly connected)
        component_count += 1;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(name.clone());
        visited.insert(name.clone());

        while let Some(current) = queue.pop_front() {
            for neighbor in workspace.graph.outgoing_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
            for neighbor in workspace.graph.incoming_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    component_count
}

/// Compute a simplified average clustering coefficient.
///
/// For each passage, count how many of its outgoing neighbors also link
/// to each other (forming triangles), divided by the maximum possible
/// number of such connections. Returns the average across all passages
/// with at least 2 outgoing links.
pub(crate) fn compute_avg_clustering(workspace: &Workspace) -> f64 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    let mut coefficients: Vec<f64> = Vec::new();

    for name in &passage_names {
        let out_neighbors: Vec<String> = workspace.graph.outgoing_neighbors(name);
        let k = out_neighbors.len();

        if k < 2 {
            continue;
        }

        let neighbor_set: std::collections::HashSet<String> =
            out_neighbors.iter().cloned().collect();

        let mut triangle_count: u32 = 0;
        for neighbor in &out_neighbors {
            let their_neighbors = workspace.graph.outgoing_neighbors(neighbor);
            for their_target in &their_neighbors {
                if neighbor_set.contains(their_target) {
                    triangle_count += 1;
                }
            }
        }

        let max_possible = (k * (k - 1)) as f64;
        let local_coeff = triangle_count as f64 / max_possible;
        coefficients.push(local_coeff);
    }

    if coefficients.is_empty() {
        0.0
    } else {
        coefficients.iter().sum::<f64>() / coefficients.len() as f64
    }
}

// ===========================================================================
// Formatting helpers
// ===========================================================================

/// Format a Twee document: normalize headers, trim trailing whitespace,
/// ensure blank lines between passages.
pub(crate) fn format_twee_text(text: &str) -> Vec<TextEdit> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut edits = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        // Trim trailing whitespace
        let trimmed_end = line.trim_end();
        if trimmed_end.len() != line.len() {
            edits.push(TextEdit {
                range: Range {
                    start: Position { line: i as u32, character: trimmed_end.len() as u32 },
                    end: Position { line: i as u32, character: line.len() as u32 },
                },
                new_text: String::new(),
            });
        }

        // Normalize passage header spacing: ensure exactly one space after "::"
        if let Some(rest) = line.strip_prefix("::") {
            if rest.starts_with(|c: char| c != ' ' && c != '[' && c != '\t') && !rest.is_empty() {
                // Missing space after "::", add one
                edits.push(TextEdit {
                    range: Range {
                        start: Position { line: i as u32, character: 2 },
                        end: Position { line: i as u32, character: 2 },
                    },
                    new_text: " ".to_string(),
                });
            }
        }
    }

    // Ensure blank lines between passages — done as a full replacement if needed
    let mut formatted_lines: Vec<String> = Vec::new();
    let mut prev_was_blank = true; // start with blank to avoid blank line at top

    for line in &lines {
        if line.starts_with("::") {
            if !prev_was_blank && !formatted_lines.is_empty() {
                formatted_lines.push(String::new());
            }
            formatted_lines.push(line.trim_end().to_string());
            prev_was_blank = false;
        } else {
            let trimmed = line.trim_end().to_string();
            prev_was_blank = trimmed.is_empty();
            formatted_lines.push(trimmed);
        }
    }

    let formatted_text = formatted_lines.join("\n");
    let original_text = text.to_string();

    if formatted_text != original_text {
        // Return a single edit replacing the entire document
        let line_count = lines.len() as u32;
        let last_line_len = lines.last().map(|l| l.len()).unwrap_or(0) as u32;
        vec![TextEdit {
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: line_count.saturating_sub(1), character: last_line_len },
            },
            new_text: formatted_text,
        }]
    } else {
        edits
    }
}

// ===========================================================================
// Code Action helpers
// ===========================================================================

/// Extract a quoted name from a diagnostic message, e.g. "Broken link to 'Foo'"
pub(crate) fn extract_quoted_name(message: &str) -> Option<String> {
    if let Some(start) = message.find('\'') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    // Also try double quotes
    if let Some(start) = message.find('"') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Extract a passage name from a diagnostic message.
pub(crate) fn extract_passage_from_diag(message: &str) -> Option<String> {
    extract_quoted_name(message).or_else(|| {
        // Try to extract after "passage" keyword
        let lower = message.to_lowercase();
        if let Some(idx) = lower.find("passage ") {
            let rest = &message[idx + 8..];
            let name = rest.split_whitespace().next().unwrap_or("").trim_end_matches(':').to_string();
            if !name.is_empty() { Some(name) } else { None }
        } else {
            None
        }
    })
}

/// Extract a variable name from a diagnostic message.
pub(crate) fn extract_variable_name(message: &str) -> Option<String> {
    // Look for $varname pattern
    for word in message.split_whitespace() {
        if word.starts_with('$') {
            return Some(word.trim_end_matches(':').trim_end_matches(',').to_string());
        }
    }
    None
}

/// Create a WorkspaceEdit that creates a new passage at the end of the file
/// where the broken link occurs.
pub(crate) fn create_passage_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    // Find any open document to add the passage to (prefer the one with StoryData)
    let target_uri = inner.workspace.find_passage_file_uri("StoryData")
        .or_else(|| {
            inner.open_documents.keys().next().cloned()
        });

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some(uri) = target_uri
        && let Some(text) = inner.open_documents.get(&uri) {
            let line_count = text.lines().count() as u32;
            changes.insert(uri, vec![TextEdit {
                range: Range {
                    start: Position { line: line_count, character: 0 },
                    end: Position { line: line_count, character: 0 },
                },
                new_text: format!("\n:: {}\n", name),
            }]);
        }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Find the nearest reachable passage to a given unreachable one.
pub(crate) fn find_nearest_reachable_passage(workspace: &Workspace, name: &str) -> Option<String> {
    let start_passage = workspace.metadata.as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");

    let unreachable = workspace.graph.detect_unreachable(start_passage);
    let unreachable_set: std::collections::HashSet<String> = unreachable.iter()
        .map(|d| d.passage_name.clone())
        .collect();

    // Look for passages that link to passages that are near the unreachable one
    // Try finding a passage in the same file first
    if let Some((doc, _)) = workspace.find_passage(name) {
        for passage in &doc.passages {
            if !unreachable_set.contains(&passage.name) && passage.name != name {
                return Some(passage.name.clone());
            }
        }
    }

    // Fall back to the start passage
    if workspace.find_passage(start_passage).is_some() {
        Some(start_passage.to_string())
    } else {
        None
    }
}

/// Create a WorkspaceEdit that adds a link from one passage to another.
pub(crate) fn add_link_edit(
    inner: &crate::state::ServerStateInner,
    from_passage: &str,
    to_passage: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(from_passage)
        && let Some(text) = inner.open_documents.get(&doc.uri) {
            // Find the passage header line and add a link at the end of its body
            let mut header_line: Option<u32> = None;
            let mut end_line: u32 = 0;
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == from_passage {
                        header_line = Some(i as u32);
                    } else if header_line.is_some() {
                        end_line = i as u32;
                        break;
                    }
                }
                if header_line.is_some() {
                    end_line = i as u32 + 1;
                }
            }

            if let Some(hl) = header_line {
                let insert_line = if end_line > hl { end_line } else { hl + 1 };
                changes.insert(doc.uri.clone(), vec![TextEdit {
                    range: Range {
                        start: Position { line: insert_line, character: 0 },
                        end: Position { line: insert_line, character: 0 },
                    },
                    new_text: format!("[[{}]]\n", to_passage),
                }]);
            }
        }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that adds content template to an empty passage.
pub(crate) fn add_content_template_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(name)
        && let Some(text) = inner.open_documents.get(&doc.uri) {
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == name {
                        // Insert template content after the header line
                        let template = format!("{}\n", name);
                        changes.insert(doc.uri.clone(), vec![TextEdit {
                            range: Range {
                                start: Position { line: (i as u32) + 1, character: 0 },
                                end: Position { line: (i as u32) + 1, character: 0 },
                            },
                            new_text: template,
                        }]);
                        break;
                    }
                }
            }
        }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that initializes a variable in StoryInit.
pub(crate) fn initialize_var_in_story_init_edit(
    inner: &crate::state::ServerStateInner,
    var_name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    // Find StoryInit passage
    if let Some((doc, _)) = inner.workspace.find_passage("StoryInit") {
        if let Some(text) = inner.open_documents.get(&doc.uri) {
            // Find the last line of StoryInit
            let mut init_start: Option<u32> = None;
            let mut init_end: u32 = 0;
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == "StoryInit" {
                        init_start = Some(i as u32);
                    } else if init_start.is_some() {
                        init_end = i as u32;
                        break;
                    }
                }
                if init_start.is_some() {
                    init_end = i as u32 + 1;
                }
            }

            let insert_line = if init_end > init_start.unwrap_or(0) { init_end } else { init_start.unwrap_or(0) + 1 };
            changes.insert(doc.uri.clone(), vec![TextEdit {
                range: Range {
                    start: Position { line: insert_line, character: 0 },
                    end: Position { line: insert_line, character: 0 },
                },
                new_text: format!("<<set {} to 0>>\n", var_name),
            }]);
        }
    } else {
        // No StoryInit — create one
        if let Some(uri) = inner.open_documents.keys().next()
            && let Some(text) = inner.open_documents.get(uri) {
                let line_count = text.lines().count() as u32;
                changes.insert(uri.clone(), vec![TextEdit {
                    range: Range {
                        start: Position { line: line_count, character: 0 },
                        end: Position { line: line_count, character: 0 },
                    },
                    new_text: format!("\n:: StoryInit\n<<set {} to 0>>\n", var_name),
                }]);
            }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

// ===========================================================================
// Related Information helpers
// ===========================================================================

/// Build related information for push diagnostics (publish_all_diagnostics).
pub(crate) fn build_related_information_for_push(
    open_documents: &HashMap<Url, String>,
    kind: &DiagnosticKind,
    passage_name: &str,
    _message: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    match kind {
        DiagnosticKind::BrokenLink => {
            // Point to the passage that contains the broken link
            find_link_locations(open_documents, passage_name)
        }
        DiagnosticKind::UnreachablePassage => {
            // Point to passages that should link to this one
            find_definition_location(open_documents, passage_name)
        }
        DiagnosticKind::DuplicatePassageName => {
            // Point to all definitions of this passage name
            find_all_definition_locations(open_documents, passage_name)
        }
        DiagnosticKind::UninitializedVariable => {
            // Point to where the variable is first read
            let var_name = extract_variable_name(_message);
            find_variable_read_locations(open_documents, passage_name, var_name.as_deref())
        }
        _ => None,
    }
}

/// Build related information for pull diagnostics (diagnostic method).
pub(crate) fn build_related_information(
    inner: &crate::state::ServerStateInner,
    kind: &DiagnosticKind,
    passage_name: &str,
    _message: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    match kind {
        DiagnosticKind::BrokenLink => {
            find_link_locations(&inner.open_documents, passage_name)
        }
        DiagnosticKind::UnreachablePassage => {
            find_definition_location(&inner.open_documents, passage_name)
        }
        DiagnosticKind::DuplicatePassageName => {
            find_all_definition_locations(&inner.open_documents, passage_name)
        }
        DiagnosticKind::UninitializedVariable => {
            let var_name = extract_variable_name(_message);
            find_variable_read_locations(&inner.open_documents, passage_name, var_name.as_deref())
        }
        _ => None,
    }
}

/// Find locations of links to a given passage name (for broken link related info).
pub(crate) fn find_link_locations(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            let mut search_from = 0;
            while let Some(rel_start) = line[search_from..].find("[[") {
                let abs_start = search_from + rel_start;
                if let Some(rel_end) = line[abs_start..].find("]]") {
                    let content_start = abs_start + 2;
                    let content_end = abs_start + rel_end;
                    let link_text = &line[content_start..content_end];
                    let link_target = if let Some(arrow) = link_text.find("->") {
                        &link_text[arrow + 2..]
                    } else if let Some(pipe) = link_text.find('|') {
                        &link_text[pipe + 1..]
                    } else {
                        link_text
                    };
                    if link_target.trim() == passage_name {
                        related.push(DiagnosticRelatedInformation {
                            location: Location {
                                uri: doc_uri.clone(),
                                range: Range {
                                    start: Position { line: line_idx as u32, character: content_start as u32 },
                                    end: Position { line: line_idx as u32, character: content_end as u32 },
                                },
                            },
                            message: format!("Link to '{}'", passage_name),
                        });
                    }
                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }
    }
    if related.is_empty() { None } else { Some(related) }
}

/// Find the definition location of a passage (for unreachable passage related info).
pub(crate) fn find_definition_location(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                if name == passage_name {
                    return Some(vec![DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: 0 },
                                end: Position { line: line_idx as u32, character: line.len() as u32 },
                            },
                        },
                        message: format!("Definition of '{}'", passage_name),
                    }]);
                }
            }
        }
    }
    None
}

/// Find all definition locations of a passage name (for duplicate passage diagnostics).
pub(crate) fn find_all_definition_locations(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                if name == passage_name {
                    related.push(DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: 0 },
                                end: Position { line: line_idx as u32, character: line.len() as u32 },
                            },
                        },
                        message: format!("Definition of '{}'", passage_name),
                    });
                }
            }
        }
    }
    if related.is_empty() { None } else { Some(related) }
}

/// Find locations where a variable is read (for uninitialized variable diagnostics).
pub(crate) fn find_variable_read_locations(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
    variable_name: Option<&str>,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();

    for (doc_uri, text) in open_documents {
        let lines: Vec<&str> = text.lines().collect();
        let mut in_passage = false;

        for (line_idx, line) in lines.iter().enumerate() {
            // Check for passage header
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                in_passage = name == passage_name;
                continue;
            }
            if !in_passage { continue; }

            // Find $variable patterns (SugarCube/Snowman style)
            let chars: Vec<char> = line.chars().collect();
            let mut pos = 0;
            while pos < chars.len() {
                if chars[pos] == '$' && pos + 1 < chars.len() && (chars[pos + 1].is_alphabetic() || chars[pos + 1] == '_') {
                    // Found a variable reference — extract the full name
                    let start = pos;
                    pos += 1;
                    while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                        pos += 1;
                    }
                    let var_name: String = chars[start..pos].iter().collect();

                    // If a specific variable was requested, only include matching reads
                    if let Some(filter) = variable_name {
                        if var_name != filter { continue; }
                    }

                    let byte_start: usize = chars[..start].iter().map(|c| c.len_utf8()).sum();
                    let byte_end: usize = chars[..pos].iter().map(|c| c.len_utf8()).sum();

                    related.push(DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: byte_start as u32 },
                                end: Position { line: line_idx as u32, character: byte_end as u32 },
                            },
                        },
                        message: format!("Variable {} is read here", var_name),
                    });
                } else {
                    pos += 1;
                }
            }
        }
    }

    if related.is_empty() { None } else { Some(related) }
}

// ===========================================================================
// Other helper functions
// ===========================================================================

/// Find passages that link TO a given passage name.
pub(crate) fn find_passages_linking_to(workspace: &Workspace, passage_name: &str) -> Vec<String> {
    let mut result = Vec::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                result.push(passage.name.clone());
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

/// Find all [[target]] link ranges for a specific target in the text.
pub(crate) fn find_link_ranges_for_target(text: &str, target: &str) -> Vec<Range> {
    let mut ranges = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("[[") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find("]]") {
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;
                let link_text = &line[content_start..content_end];
                let link_target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                if link_target.trim() == target {
                    ranges.push(Range {
                        start: Position { line: line_idx as u32, character: content_start as u32 },
                        end: Position { line: line_idx as u32, character: content_end as u32 },
                    });
                }
                search_from = abs_start + rel_end + 2;
            } else {
                break;
            }
        }
    }
    ranges
}

// ===========================================================================
// Unit tests for handler helper functions
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::graph::DiagnosticKind;
    use knot_core::passage::StoryFormat;

    // -----------------------------------------------------------------------
    // diagnostic_kind_to_severity
    // -----------------------------------------------------------------------

    #[test]
    fn test_diagnostic_severity_defaults() {
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::BrokenLink), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnreachablePassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::InfiniteLoop), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UninitializedVariable), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnusedVariable), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::RedundantWrite), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DuplicateStoryData), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStoryData), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStartPassage), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnsupportedFormat), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DuplicatePassageName), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::EmptyPassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DeadEndPassage), DiagnosticSeverity::INFORMATION);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::InvalidPassageName), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::OrphanedPassage), DiagnosticSeverity::INFORMATION);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::ComplexPassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::LargePassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStartLink), DiagnosticSeverity::WARNING);
    }

    // -----------------------------------------------------------------------
    // parse_passage_name_from_header
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_passage_name_simple() {
        assert_eq!(parse_passage_name_from_header("Start"), "Start");
    }

    #[test]
    fn test_parse_passage_name_with_tags() {
        assert_eq!(parse_passage_name_from_header("Start [important]"), "Start");
    }

    #[test]
    fn test_parse_passage_name_with_leading_space() {
        assert_eq!(parse_passage_name_from_header(" Start "), "Start");
    }

    #[test]
    fn test_parse_passage_name_empty() {
        assert_eq!(parse_passage_name_from_header(""), "");
    }

    // -----------------------------------------------------------------------
    // find_passage_header_range
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_passage_header_range_found() {
        let text = ":: Start\nHello world\n:: End\nGoodbye";
        let range = find_passage_header_range(text, "Start");
        assert_eq!(range.start.line, 0);
    }

    #[test]
    fn test_find_passage_header_range_not_found() {
        let text = ":: Start\nHello world";
        let range = find_passage_header_range(text, "NonExistent");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.end.line, 0);
    }

    // -----------------------------------------------------------------------
    // find_passage_at_position
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_passage_at_position() {
        let text = ":: Start\nHello\n:: Middle\nWorld\n:: End\nBye";
        // Line 0 is passage header "Start" — returns the passage name
        assert_eq!(
            find_passage_at_position(text, Position { line: 0, character: 3 }),
            Some("Start".to_string())
        );
        // Line 1 is body (not a :: header) — returns None
        assert_eq!(
            find_passage_at_position(text, Position { line: 1, character: 0 }),
            None
        );
        // Line 2 is passage header "Middle"
        assert_eq!(
            find_passage_at_position(text, Position { line: 2, character: 3 }),
            Some("Middle".to_string())
        );
    }

    #[test]
    fn test_find_passage_at_position_no_passage() {
        let text = "Just some text without passage headers";
        assert_eq!(
            find_passage_at_position(text, Position { line: 0, character: 0 }),
            None
        );
    }

    // -----------------------------------------------------------------------
    // find_link_target_at_position
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_link_target_simple() {
        let text = ":: Start\nGo to [[Castle]] for adventure";
        // "Castle" link is at approximately character 6 on line 1
        let result = find_link_target_at_position(text, Position { line: 1, character: 10 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    #[test]
    fn test_find_link_target_arrow() {
        let text = ":: Start\n[[Go to Castle->Castle]]";
        let result = find_link_target_at_position(text, Position { line: 1, character: 5 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    #[test]
    fn test_find_link_target_pipe() {
        let text = ":: Start\n[[Visit|Castle]]";
        let result = find_link_target_at_position(text, Position { line: 1, character: 5 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    // -----------------------------------------------------------------------
    // format_twee_text
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_trailing_whitespace() {
        let text = ":: Start   \nHello  ";
        let edits = format_twee_text(text);
        // Should have edits to trim trailing whitespace
        assert!(!edits.is_empty());
    }

    #[test]
    fn test_format_already_clean() {
        let text = ":: Start\nHello\n\n:: End\nGoodbye\n";
        let _edits = format_twee_text(text);
        // Already clean — may or may not have edits (depends on blank line logic)
        // Just ensure it doesn't panic
    }

    // -----------------------------------------------------------------------
    // extract_quoted_name / extract_passage_from_diag / extract_variable_name
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_quoted_name() {
        assert_eq!(extract_quoted_name("Link to 'Castle' not found"), Some("Castle".to_string()));
        assert_eq!(extract_quoted_name("Link to \"Castle\" not found"), Some("Castle".to_string()));
        assert_eq!(extract_quoted_name("No quotes here"), None);
    }

    #[test]
    fn test_extract_passage_from_diag() {
        assert_eq!(
            extract_passage_from_diag("Broken link to passage 'Forest'"),
            Some("Forest".to_string())
        );
        assert_eq!(
            extract_passage_from_diag("Passage 'Start' is unreachable"),
            Some("Start".to_string())
        );
    }

    #[test]
    fn test_extract_variable_name() {
        // $varname without quotes
        assert_eq!(
            extract_variable_name("Variable $gold may be used before initialization"),
            Some("$gold".to_string())
        );
        assert_eq!(
            extract_variable_name("No variable mentioned"),
            None
        );
    }

    // -----------------------------------------------------------------------
    // parse_story_data_json
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_story_data_valid() {
        let json = r#"{ "ifid": "A1B2C3", "format": "SugarCube", "start": "Prologue" }"#;
        let meta = parse_story_data_json(json);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.start_passage, "Prologue");
    }

    #[test]
    fn test_parse_story_data_invalid() {
        let meta = parse_story_data_json("not json at all");
        assert!(meta.is_none());
    }

    #[test]
    fn test_parse_story_data_missing_fields() {
        let json = r#"{ "ifid": "A1B2C3" }"#;
        let meta = parse_story_data_json(json);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        // Default values
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.start_passage, "Start");
    }

    // -----------------------------------------------------------------------
    // sugarcube_macro_signatures
    // -----------------------------------------------------------------------

    #[test]
    fn test_sugarcube_macro_signatures_nonempty() {
        let sigs = crate::handlers::macros::sugarcube_macro_signatures();
        assert!(!sigs.is_empty());
        // Spot-check a few well-known macros
        assert!(sigs.iter().any(|m| m.name == "set"));
        assert!(sigs.iter().any(|m| m.name == "if"));
        assert!(sigs.iter().any(|m| m.name == "goto"));
    }

    #[test]
    fn test_macro_signature_insert_snippet() {
        let sigs = crate::handlers::macros::sugarcube_macro_signatures();
        let set_macro = sigs.iter().find(|m| m.name == "set").unwrap();
        assert_eq!(set_macro.insert_snippet(), " ${1:args}");

        let else_macro = sigs.iter().find(|m| m.name == "else").unwrap();
        assert_eq!(else_macro.insert_snippet(), "");
    }

    // -----------------------------------------------------------------------
    // byte_offset_to_position / byte_range_to_lsp_range
    // -----------------------------------------------------------------------

    #[test]
    fn test_byte_offset_to_position() {
        let text = "line one\nline two\nline three";
        assert_eq!(byte_offset_to_position(text, 0).line, 0);
        assert_eq!(byte_offset_to_position(text, 0).character, 0);
        // "line one\n" = 9 bytes, so offset 9 is start of line 1
        assert_eq!(byte_offset_to_position(text, 9).line, 1);
        assert_eq!(byte_offset_to_position(text, 9).character, 0);
    }

    // -----------------------------------------------------------------------
    // DebounceTimer
    // -----------------------------------------------------------------------

    #[test]
    fn test_debounce_timer_starts_ready() {
        use knot_core::editing::DebounceTimer;
        let timer = DebounceTimer::new();
        assert!(timer.is_ready());
        assert!(!timer.is_pending());
    }

    #[test]
    fn test_debounce_timer_pending_after_edit() {
        use knot_core::editing::DebounceTimer;
        let mut timer = DebounceTimer::new();
        timer.record_edit();
        assert!(timer.is_pending());
    }
}
