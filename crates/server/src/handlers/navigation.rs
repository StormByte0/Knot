//! Navigation handlers: goto_definition, goto_declaration,
//! goto_implementation, goto_type_definition, references.

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

    if let Some(text) = inner.open_documents.get(&uri)
        && let Some(target_name) = helpers::find_link_target_at_position(text, position)
            && let Some((doc, passage)) = inner.workspace.find_passage(&target_name) {
                let target_uri = doc.uri.clone();
                // Find the passage header line in the target document.
                let target_text = inner.open_documents.get(&target_uri);
                let range = if let Some(t) = target_text {
                    helpers::find_passage_header_range(t, &passage.name)
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

    let target_passage = if let Some(text) = inner.open_documents.get(&uri) {
        helpers::find_passage_at_position(text, position)
            .or_else(|| helpers::find_link_target_at_position(text, position))
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all passages that link TO this passage
    let mut locations = Vec::new();
    for (doc_uri, text) in &inner.open_documents {
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
                    if link_target.trim() == target_name {
                        locations.push(Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, content_start) },
                                end: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, content_end) },
                            },
                        });
                    }
                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
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
            helpers::find_passage_header_range(t, "StoryData")
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
        if let Some(name) = helpers::find_passage_at_position(text, position) {
            Some(name)
        } else { helpers::find_link_target_at_position(text, position) }
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all locations that reference this passage (links + definition)
    let mut locations = Vec::new();

    for (doc_uri, text) in &inner.open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            // Check for passage header definition
            if line.starts_with("::") {
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if name == target_name {
                    locations.push(Location {
                        uri: doc_uri.clone(),
                        range: Range {
                            start: Position {
                                line: line_idx as u32,
                                character: 0,
                            },
                            end: Position {
                                line: line_idx as u32,
                                character: helpers::utf16_len(line),
                            },
                        },
                    });
                }
            }

            // Check for links to this passage
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

                    if link_target.trim() == target_name {
                        locations.push(Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position {
                                    line: line_idx as u32,
                                    character: helpers::utf16_len_up_to(line, content_start),
                                },
                                end: Position {
                                    line: line_idx as u32,
                                    character: helpers::utf16_len_up_to(line, content_end),
                                },
                            },
                        });
                    }

                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
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
