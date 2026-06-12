//! Navigation handlers: goto_definition, goto_declaration,
//! goto_implementation, goto_type_definition, references.
//!
//! All handlers use span-based resolution via the workspace index instead of
//! re-scanning source text. This avoids redundant parsing, correctly handles
//! multi-byte characters, and works with arrow/pipe link syntax.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn goto_definition(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    if let Some(text) = inner.open_documents.get(&uri) {
        // ── 1. Try link target (e.g., [[Target]], <<goto "Target">>) ──────
        if let Some(target_name) = helpers::find_link_target_at_position_span_based(
            text, &inner.workspace, &uri, position,
        ) {
            if let Some((doc, _passage)) = inner.workspace.find_passage(&target_name) {
                let target_uri = doc.uri.clone();
                let target_text = inner.open_documents.get(&target_uri);
                let range = if let Some(t) = target_text {
                    helpers::find_passage_header_range_span_based(t, &inner.workspace, &target_name)
                } else {
                    Range::default()
                };

                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: target_uri,
                    range,
                })));
            }
        }

        // ── 2. Try variable definition (e.g., $var → <<set $var>>) ───────
        {
            let format = inner.workspace.resolve_format();
            let plugin = inner.format_registry.get(&format);
            let sigils: Vec<char> = plugin.as_ref()
                .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
                .unwrap_or_default();

            if let Some(var_name) = helpers::find_variable_at_position_span_based(
                text, &inner.workspace, &uri, position, &sigils,
            ) {
                // Find the first passage that initializes this variable.
                // Prefer non-temporary initializations (<<set $var to ...>>) over
                // reads, and prefer passages that appear earlier in the document
                // order to give the most likely "definition" site.
                //
                // Note: var.span is passage-relative; use passage.abs_range()
                // to convert to document-absolute at the LSP boundary.
                let mut best: Option<(url::Url, Range)> = None;
                for doc in inner.workspace.documents() {
                    if let Some(doc_text) = inner.open_documents.get(&doc.uri) {
                        for passage in &doc.passages {
                            for var in &passage.vars {
                                if var.name == var_name && !var.is_temporary {
                                    if matches!(var.kind, knot_core::passage::VarKind::Init) {
                                        // Found an initialization — convert the
                                        // passage-relative span to document-absolute.
                                        let range = helpers::byte_range_to_lsp_range(doc_text, &passage.abs_range(&var.span));
                                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                                            uri: doc.uri.clone(),
                                            range,
                                        })));
                                    } else if best.is_none() {
                                        // Keep the first read as a fallback
                                        let range = helpers::byte_range_to_lsp_range(doc_text, &passage.abs_range(&var.span));
                                        best = Some((doc.uri.clone(), range));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((uri, range)) = best {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location { uri, range })));
                }
            }
        }
    }

    Ok(None)
}

pub(crate) async fn goto_declaration(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    // Declaration — same as definition for Twine (links to passage header)
    goto_definition(state, params).await
}

pub(crate) async fn goto_implementation(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    // Determine the target passage using span-based resolution
    let target_passage = if let Some(text) = inner.open_documents.get(&uri) {
        helpers::find_passage_at_position_span_based(text, &inner.workspace, &uri, position)
            .or_else(|| helpers::find_link_target_at_position_span_based(text, &inner.workspace, &uri, position))
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all passages that link TO this passage using workspace data.
    // Iterate workspace.documents().passages[].links[] where
    // link.target == target_name, using link.span for the location range.
    let mut locations = Vec::new();
    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };
        for passage in &doc.passages {
            for link in &passage.links {
                if link.target.trim() == target_name {
                    let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));
                    locations.push(Location {
                        uri: doc.uri.clone(),
                        range,
                    });
                }
            }
        }
    }

    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDefinitionResponse::Array(locations)))
    }
}

pub(crate) async fn goto_type_definition(
    state: &ServerState,
    _params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;

    // Find the StoryData passage in the workspace
    if let Some((doc, _passage)) = inner.workspace.find_passage("StoryData") {
        let target_uri = doc.uri.clone();
        let target_text = inner.open_documents.get(&target_uri);
        let range = if let Some(t) = target_text {
            helpers::find_passage_header_range_span_based(t, &inner.workspace, "StoryData")
        } else {
            Range::default()
        };
        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range,
        })));
    }

    Ok(None)
}

pub(crate) async fn references(
    state: &ServerState,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;

    let inner = state.inner.read().await;

    // First, determine what the user is on: a passage header or a link
    let target_passage = if let Some(text) = inner.open_documents.get(&uri) {
        // Check if cursor is on a passage header
        if let Some(name) = helpers::find_passage_at_position_span_based(
            text, &inner.workspace, &uri, position,
        ) {
            Some(name)
        } else {
            helpers::find_link_target_at_position_span_based(
                text, &inner.workspace, &uri, position,
            )
        }
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all locations that reference this passage using workspace data:
    // - Header references: passages where passage.name == target_name → use
    //   header_name_span (or passage.span as fallback)
    // - Link references: passages where any link.target == target_name → use
    //   link.span
    let mut locations = Vec::new();

    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };
        for passage in &doc.passages {
            // Header definition reference
            if passage.name == target_name {
                let range = if let Some(ref name_span) = passage.header_name_span {
                    helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
                } else {
                    // Fallback: compute the full header line range
                    let span_start = passage.abs_offset(passage.span.start).min(text.len());
                    let header_end = text[span_start..]
                        .find('\n')
                        .map(|n| span_start + n)
                        .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));
                    helpers::byte_range_to_lsp_range(text, &(span_start..header_end))
                };
                locations.push(Location {
                    uri: doc.uri.clone(),
                    range,
                });
            }

            // Link references
            for link in &passage.links {
                if link.target.trim() == target_name {
                    let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));
                    locations.push(Location {
                        uri: doc.uri.clone(),
                        range,
                    });
                }
            }
        }
    }

    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(locations))
    }
}
