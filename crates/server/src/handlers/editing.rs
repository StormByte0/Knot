//! Editing handlers: formatting, range_formatting, on_type_formatting,
//! linked_editing_range, prepare_rename, rename.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;
use std::collections::HashMap;
use url::Url;

pub(crate) async fn formatting(
    state: &ServerState,
    params: DocumentFormattingParams,
) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document.uri;
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    let edits = helpers::format_twee_text(text);
    if edits.is_empty() {
        Ok(None)
    } else {
        Ok(Some(edits))
    }
}

pub(crate) async fn range_formatting(
    state: &ServerState,
    params: DocumentRangeFormattingParams,
) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document.uri;
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    let all_edits = helpers::format_twee_text(text);
    // Filter edits to those within the requested range
    let range = params.range;
    let filtered: Vec<TextEdit> = all_edits
        .into_iter()
        .filter(|edit| {
            edit.range.start.line >= range.start.line
                && edit.range.end.line <= range.end.line
        })
        .collect();

    if filtered.is_empty() {
        Ok(None)
    } else {
        Ok(Some(filtered))
    }
}

pub(crate) async fn on_type_formatting(
    state: &ServerState,
    params: DocumentOnTypeFormattingParams,
) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let ch = &params.ch;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    let line_text = text.lines().nth(position.line as usize).unwrap_or("");
    // position.character is UTF-16; convert to byte offset for string slicing
    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);

    // Auto-close [[ with ]]
    if ch == "]" && byte_pos >= 2 {
        let before = &line_text[..byte_pos];
        if before.ends_with("[[") {
            let insert_pos = Position { line: position.line, character: position.character };
            return Ok(Some(vec![TextEdit {
                range: Range { start: insert_pos, end: insert_pos },
                new_text: "]]".to_string(),
            }]));
        }
    }

    // Auto-close << with >>
    if ch == ">" && byte_pos >= 2 {
        let before = &line_text[..byte_pos];
        if before.ends_with("<<") {
            let insert_pos = Position { line: position.line, character: position.character };
            return Ok(Some(vec![TextEdit {
                range: Range { start: insert_pos, end: insert_pos },
                new_text: ">>".to_string(),
            }]));
        }
    }

    Ok(None)
}

pub(crate) async fn linked_editing_range(
    state: &ServerState,
    params: LinkedEditingRangeParams,
) -> Result<Option<LinkedEditingRanges>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    // If cursor is on a passage header name, find all [[link]] references
    if let Some(name) = helpers::find_passage_at_position(text, position) {
        let line_text = text.lines().nth(position.line as usize).unwrap_or("");
        let name_start = line_text.find(&name).unwrap_or(2);

        let mut ranges = vec![Range {
            start: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, name_start) },
            end: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, name_start + name.len()) },
        }];

        // Find all [[name]] links in the document
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

                    if link_target.trim() == name {
                        let target_start = content_start + (link_text.len() - link_target.len());
                        ranges.push(Range {
                            start: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, target_start) },
                            end: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, target_start + name.len()) },
                        });
                    }

                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }

        return Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: None,
        }));
    }

    Ok(None)
}

pub(crate) async fn prepare_rename(
    state: &ServerState,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document.uri;
    let position = params.position;

    let inner = state.inner.read().await;

    if let Some(text) = inner.open_documents.get(uri) {
        // Check if cursor is on a passage header
        if let Some(name) = helpers::find_passage_at_position(text, position) {
            let line_text = text.lines().nth(position.line as usize).unwrap_or("");
            let name_start_byte = line_text.find(&name).unwrap_or(2);
            let name_start_utf16 = helpers::utf16_len_up_to(line_text, name_start_byte);
            let name_end_utf16 = helpers::utf16_len_up_to(line_text, name_start_byte + name.len());
            return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: Range {
                    start: Position { line: position.line, character: name_start_utf16 },
                    end: Position { line: position.line, character: name_end_utf16 },
                },
                placeholder: name,
            }));
        }

        // Check if cursor is on a link target
        if let Some(target_name) = helpers::find_link_target_at_position(text, position) {
            let line_text = text.lines().nth(position.line as usize).unwrap_or("");
            // Find the [[...]] that contains the cursor
            let mut search_from = 0;
            while let Some(rel_start) = line_text[search_from..].find("[[") {
                let abs_start = search_from + rel_start;
                if let Some(rel_end) = line_text[abs_start..].find("]]") {
                    let content_start = abs_start + 2;
                    let content_end = abs_start + rel_end;
                    // Compare UTF-16 cursor position with UTF-16 offsets of link content
                    let utf16_content_start = helpers::utf16_len_up_to(line_text, content_start);
                    let utf16_content_end = helpers::utf16_len_up_to(line_text, content_end);
                    if position.character >= utf16_content_start && position.character <= utf16_content_end {
                        return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                            range: Range {
                                start: Position { line: position.line, character: utf16_content_start },
                                end: Position { line: position.line, character: utf16_content_end },
                            },
                            placeholder: target_name,
                        }));
                    }
                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }
    }

    Ok(None)
}

pub(crate) async fn rename(
    state: &ServerState,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    let inner = state.inner.read().await;

    // Determine what the user is renaming
    let target_passage = if let Some(text) = inner.open_documents.get(uri) {
        helpers::find_passage_at_position(text, position)
            .or_else(|| helpers::find_link_target_at_position(text, position))
    } else {
        None
    };

    let Some(old_name) = target_passage else {
        return Ok(None);
    };

    // Collect all edits across all documents
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    for (doc_uri, text) in &inner.open_documents {
        let mut doc_edits = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            // Rename passage header
            if line.starts_with("::") {
                let name = helpers::parse_passage_name_from_header(&line[2..]);
                if name == old_name {
                    let name_start = line.find(&name).unwrap_or(2);
                    doc_edits.push(TextEdit {
                        range: Range {
                            start: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, name_start) },
                            end: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, name_start + name.len()) },
                        },
                        new_text: new_name.clone(),
                    });
                }
            }

            // Rename links
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

                    if link_target.trim() == old_name {
                        // Find the exact position of the target name in the link
                        let target_start = content_start + (link_text.len() - link_target.len());
                        let target_end = target_start + link_target.trim().len();
                        doc_edits.push(TextEdit {
                            range: Range {
                                start: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, target_start) },
                                end: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, target_end) },
                            },
                            new_text: new_name.clone(),
                        });
                    }

                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }

        if !doc_edits.is_empty() {
            changes.insert(doc_uri.clone(), doc_edits);
        }
    }

    if changes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }
}
