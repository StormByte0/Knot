//! Diagnostic publishing, severity mapping, format-delegated variable analysis,
//! related-information construction, and special passage seed supplementation.

use knot_core::graph::DiagnosticKind;
use knot_core::passage::StoryFormat;
use knot_core::{AnalysisEngine, Workspace};
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;
use std::collections::HashMap;
use url::Url;

use super::code_actions::extract_variable_name;
use super::position::{
    byte_range_to_lsp_range, find_passage_header_range, parse_passage_name_from_header, utf16_len,
    utf16_len_up_to,
};

// ===========================================================================
// Incoming link counting
// ===========================================================================

/// Count the number of incoming links to a passage from other passages.
/// Deduplicates by passage name to avoid double-counting when the same
/// passage name appears in multiple documents (e.g., during race conditions).
pub(crate) fn count_incoming_links(workspace: &Workspace, passage_name: &str) -> usize {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                seen.insert(passage.name.clone());
            }
        }
    }
    // Also count graph edges (handles dynamic navigation links)
    for source in workspace.graph.incoming_neighbors(passage_name) {
        seen.insert(source);
    }
    seen.len()
}

/// Get the list of passage names that link to a given passage.
/// Deduplicates by source passage name.
pub(crate) fn incoming_link_sources(workspace: &Workspace, passage_name: &str) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                seen.insert(passage.name.clone());
            }
        }
    }
    for source in workspace.graph.incoming_neighbors(passage_name) {
        seen.insert(source);
    }
    let mut sources: Vec<String> = seen.into_iter().collect();
    sources.sort();
    sources
}

// ===========================================================================
// Diagnostic publishing
// ===========================================================================

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
    workspace: &Workspace,
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
                    open_documents, workspace, &gd.kind, &gd.passage_name, &gd.message,
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
// Diagnostic severity mapping
// ===========================================================================

/// Map a `DiagnosticKind` to its default LSP `DiagnosticSeverity`.
pub(crate) fn diagnostic_kind_to_severity(kind: &DiagnosticKind) -> DiagnosticSeverity {
    match kind {
        DiagnosticKind::BrokenLink => DiagnosticSeverity::WARNING,
        DiagnosticKind::UnreachablePassage => DiagnosticSeverity::HINT,
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
        // Format-delegated variable diagnostics — all HINTs because
        // SugarCube variables are persistent game state; other formats
    // have their own persistence models.
        DiagnosticKind::VariableAvailabilityHint => DiagnosticSeverity::HINT,
        DiagnosticKind::UnusedVariableHint => DiagnosticSeverity::HINT,
        DiagnosticKind::RedundantWriteHint => DiagnosticSeverity::HINT,
        DiagnosticKind::UnknownPropertyHint => DiagnosticSeverity::HINT,
    }
}

// ===========================================================================
// Format-delegated variable analysis
// ===========================================================================

/// Compute format-delegated variable diagnostics using the active format plugin.
///
/// This is the preferred path for variable analysis. It delegates to the
/// format plugin's `build_state_variable_registry()` and
/// `compute_variable_diagnostics()` methods, which understand the
/// format-specific variable semantics (e.g., SugarCube's `State.variables`
/// persistence model).
///
/// Returns a list of `FormatVariableDiagnostic` that can be passed to
/// `AnalysisEngine::analyze_with_format_diagnostics()`.
pub(crate) fn compute_format_variable_diagnostics(
    workspace: &Workspace,
    registry: &fmt_plugin::FormatRegistry,
    format: StoryFormat,
) -> Vec<knot_core::FormatVariableDiagnostic> {
    use knot_core::graph::DiagnosticKind;
    use knot_formats::types::VariableDiagnosticKind;

    let start_passage = workspace
        .metadata
        .as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");

    let Some(plugin) = registry.get(&format) else {
        return Vec::new();
    };

    let state_registry = plugin.build_state_variable_registry(workspace);
    let var_diagnostics = plugin.compute_variable_diagnostics(workspace, start_passage, &state_registry);

    // Convert format-specific VariableDiagnostic → core FormatVariableDiagnostic
    var_diagnostics
        .into_iter()
        .map(|vd| {
            let kind = match vd.kind {
                VariableDiagnosticKind::VariableAvailabilityHint => {
                    DiagnosticKind::VariableAvailabilityHint
                }
                VariableDiagnosticKind::UnusedVariableHint => {
                    DiagnosticKind::UnusedVariableHint
                }
                VariableDiagnosticKind::RedundantWriteHint => {
                    DiagnosticKind::RedundantWriteHint
                }
                VariableDiagnosticKind::UnknownPropertyHint => {
                    DiagnosticKind::UnknownPropertyHint
                }
            };
            knot_core::FormatVariableDiagnostic {
                passage_name: vd.passage_name,
                file_uri: vd.file_uri,
                kind,
                message: vd.message,
            }
        })
        .collect()
}

/// Run analysis with format-delegated variable diagnostics.
///
/// This is the preferred way to run analysis in the server. It first runs
/// the core analysis (broken links, unreachable passages, etc.), then
/// appends format-specific variable diagnostics computed by the active
/// format plugin.
pub(crate) fn analyze_with_format_vars(
    workspace: &Workspace,
    registry: &fmt_plugin::FormatRegistry,
) -> Vec<knot_core::graph::GraphDiagnostic> {
    let format = workspace.resolve_format();
    let format_var_diags = compute_format_variable_diagnostics(workspace, registry, format);
    AnalysisEngine::analyze_with_format_diagnostics(workspace, format_var_diags)
}

// ===========================================================================
// Related Information helpers
// ===========================================================================

/// Build related information for push diagnostics (publish_all_diagnostics).
pub(crate) fn build_related_information_for_push(
    open_documents: &HashMap<Url, String>,
    workspace: &Workspace,
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
            find_variable_read_locations(workspace, passage_name, var_name.as_deref(), open_documents)
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
                                    start: Position { line: line_idx as u32, character: utf16_len_up_to(line, content_start) },
                                    end: Position { line: line_idx as u32, character: utf16_len_up_to(line, content_end) },
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
                                end: Position { line: line_idx as u32, character: utf16_len(line) },
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
                                end: Position { line: line_idx as u32, character: utf16_len(line) },
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
///
/// Uses the already-parsed `passage.vars` from the workspace instead of
/// re-scanning for `$` patterns, which would be SugarCube-specific. The
/// format plugin populates `passage.vars` during parsing with the correct
/// variable names and locations for whichever format is active.
pub(crate) fn find_variable_read_locations(
    workspace: &Workspace,
    _passage_name: &str,
    variable_name: Option<&str>,
    open_documents: &std::collections::HashMap<Url, String>,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();

    // Search across all passages for read locations of the variable
    for doc in workspace.documents() {
        let text = match open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };

        for passage in &doc.passages {
            for var in &passage.vars {
                if var.kind != knot_core::passage::VarKind::Read {
                    continue;
                }
                if let Some(filter) = variable_name {
                    if var.name != filter {
                        continue;
                    }
                }

                // Compute the line number from the byte span offset
                let line = text[..var.span.start.min(text.len())]
                    .lines()
                    .count()
                    .saturating_sub(1) as u32;

                related.push(DiagnosticRelatedInformation {
                    location: Location {
                        uri: doc.uri.clone(),
                        range: Range {
                            start: Position { line, character: 0 },
                            end: Position { line, character: 0 },
                        },
                    },
                    message: format!("Variable {} is read in passage '{}'", var.name, passage.name),
                });
            }
        }
    }

    if related.is_empty() { None } else { Some(related) }
}

// ===========================================================================
// Special passage seed supplementation
// ===========================================================================

/// Supplement the core seed set with variables initialized by format-specific
/// special passages (e.g., SugarCube's `StoryInit`).
///
/// The core `AnalysisEngine::collect_special_passage_initializers()` only finds
/// variables that are directly assigned in special passages discovered during
/// workspace indexing. However, the format plugin may know about additional
/// variables that are implicitly seeded (e.g., via `State.variables` assignments
/// in special passages that the core dataflow doesn't track as "persistent
/// inits"). This function closes that gap by querying the format plugin's
/// `special_passage_seed_variables()` and merging the results into the seed set.
pub(crate) fn supplement_seed_with_format_specials(
    mut core_seed: knot_core::analysis::InitSet,
    workspace: &Workspace,
    registry: &fmt_plugin::FormatRegistry,
    format: StoryFormat,
) -> knot_core::analysis::InitSet {
    if let Some(plugin) = registry.get(&format) {
        let format_seeds = plugin.special_passage_seed_variables(workspace);
        core_seed.extend(format_seeds);
    }
    core_seed
}
