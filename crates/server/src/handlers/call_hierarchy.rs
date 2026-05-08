//! Call hierarchy handlers: prepare_call_hierarchy, incoming_calls,
//! outgoing_calls.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn prepare_call_hierarchy(
    state: &ServerState,
    params: CallHierarchyPrepareParams,
) -> Result<Option<Vec<CallHierarchyItem>>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    let target_passage = helpers::find_passage_at_position(text, position)
        .or_else(|| helpers::find_link_target_at_position(text, position));

    let Some(name) = target_passage else {
        return Ok(None);
    };

    // Find the passage definition location
    if let Some((doc, _passage)) = inner.workspace.find_passage(&name) {
        let target_uri = doc.uri.clone();
        let target_text = inner.open_documents.get(&target_uri);
        let range = if let Some(t) = target_text {
            helpers::find_passage_header_range(t, &name)
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

    // Find all passages that link TO this passage
    for (doc_uri, text) in &inner.open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let source_name = helpers::parse_passage_name_from_header(&line[2..]);
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

                        if link_target.trim() == *name {
                            let source_range = helpers::find_passage_header_range(text, &source_name);
                            calls.push(CallHierarchyIncomingCall {
                                from: CallHierarchyItem {
                                    name: source_name,
                                    kind: SymbolKind::MODULE,
                                    tags: None,
                                    detail: None,
                                    uri: doc_uri.clone(),
                                    range: source_range,
                                    selection_range: source_range,
                                    data: None,
                                },
                                from_ranges: vec![Range {
                                    start: Position { line: line_idx as u32, character: content_start as u32 },
                                    end: Position { line: line_idx as u32, character: content_end as u32 },
                                }],
                            });
                            break; // One match per passage is enough
                        }
                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }
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
                    helpers::find_passage_header_range(t, &link.target)
                } else {
                    Range::default()
                };

                // Find the link range in the source
                let from_ranges = if let Some(t) = text {
                    helpers::find_link_ranges_for_target(t, &link.target)
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
