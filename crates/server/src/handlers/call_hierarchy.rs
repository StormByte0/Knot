//! Call hierarchy handlers: prepare_call_hierarchy, incoming_calls,
//! outgoing_calls.
//!
//! Uses span-based resolution via the workspace index instead of re-scanning
//! source text for `[[`/`]]` links and `::` headers.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn prepare_call_hierarchy(
    state: &ServerState,
    params: CallHierarchyPrepareParams,
) -> Result<Option<Vec<CallHierarchyItem>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let target_passage = helpers::find_passage_at_position_span_based(text, &inner.workspace, &uri, position)
        .or_else(|| helpers::find_link_target_at_position_span_based(text, &inner.workspace, &uri, position));

    let Some(name) = target_passage else {
        return Ok(None);
    };

    // Find the passage definition location
    if let Some((doc, _passage)) = inner.workspace.find_passage(&name) {
        let target_uri = doc.uri.clone();
        let target_text = inner.open_documents.get(&target_uri);
        let range = if let Some(t) = target_text {
            helpers::find_passage_header_range_span_based(t, &inner.workspace, &name)
        } else {
            Range::default()
        };

        return Ok(Some(vec![CallHierarchyItem {
            name,
            kind: SymbolKind::MODULE,
            tags: None,
            detail: None,
            uri: target_uri,
            range,
            selection_range: range,
            data: None,
        }]));
    }

    Ok(None)
}

pub(crate) async fn incoming_calls(
    state: &ServerState,
    params: CallHierarchyIncomingCallsParams,
) -> Result<Option<Vec<CallHierarchyIncomingCall>>, tower_lsp::jsonrpc::Error> {
    let item = &params.item;
    let name = &item.name;

    let inner = state.inner.read().await;

    let mut calls = Vec::new();

    // Find all passages that link TO this passage using workspace data.
    // Iterate workspace.documents().passages[].links[] where
    // link.target == name, using link.span for the from_ranges and
    // passage.header_name_span (or passage.span fallback) for the
    // source passage header range.
    let mut seen_passages: std::collections::HashSet<url::Url> = std::collections::HashSet::new();
    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };
        for passage in &doc.passages {
            let matching_links: Vec<_> = passage.links.iter()
                .filter(|l| l.target.trim() == name)
                .collect();

            if matching_links.is_empty() {
                continue;
            }

            // One call hierarchy entry per source passage
            if seen_passages.contains(&doc.uri) {
                continue;
            }
            seen_passages.insert(doc.uri.clone());

            let source_range = helpers::find_passage_header_range_span_based(
                text, &inner.workspace, &passage.name,
            );

            let from_ranges: Vec<Range> = matching_links.iter()
                .map(|link| helpers::byte_range_to_lsp_range(text, &link.span))
                .collect();

            calls.push(CallHierarchyIncomingCall {
                from: CallHierarchyItem {
                    name: passage.name.clone(),
                    kind: SymbolKind::MODULE,
                    tags: None,
                    detail: None,
                    uri: doc.uri.clone(),
                    range: source_range,
                    selection_range: source_range,
                    data: None,
                },
                from_ranges,
            });
        }
    }

    if calls.is_empty() {
        Ok(None)
    } else {
        Ok(Some(calls))
    }
}

pub(crate) async fn outgoing_calls(
    state: &ServerState,
    params: CallHierarchyOutgoingCallsParams,
) -> Result<Option<Vec<CallHierarchyOutgoingCall>>, tower_lsp::jsonrpc::Error> {
    let item = &params.item;
    let name = &item.name;

    let inner = state.inner.read().await;

    let mut calls = Vec::new();

    // Find the passage and its outgoing links
    if let Some((doc, passage)) = inner.workspace.find_passage(name) {
        let text = inner.open_documents.get(&doc.uri);
        for link in &passage.links {
            if let Some((target_doc, _target_passage)) = inner.workspace.find_passage(&link.target) {
                let target_uri = target_doc.uri.clone();
                let target_text = inner.open_documents.get(&target_uri);
                let target_range = if let Some(t) = target_text {
                    helpers::find_passage_header_range_span_based(t, &inner.workspace, &link.target)
                } else {
                    Range::default()
                };

                // Find the link ranges in the source using span-based resolution
                let from_ranges = if let Some(t) = text {
                    helpers::find_link_ranges_for_target(t, &inner.workspace, &doc.uri, &link.target)
                } else {
                    vec![]
                };

                calls.push(CallHierarchyOutgoingCall {
                    to: CallHierarchyItem {
                        name: link.target.clone(),
                        kind: SymbolKind::MODULE,
                        tags: None,
                        detail: None,
                        uri: target_uri,
                        range: target_range,
                        selection_range: target_range,
                        data: None,
                    },
                    from_ranges,
                });
            }
        }
    }

    if calls.is_empty() {
        Ok(None)
    } else {
        Ok(Some(calls))
    }
}
