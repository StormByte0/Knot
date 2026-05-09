//! Structure handlers: folding_range, document_link, selection_range,
//! signature_help.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn folding_range(
    state: &ServerState,
    params: FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let mut ranges = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        if line.starts_with("::") {
            // Find the end of this passage (next :: or end of file)
            let end_line = lines[line_idx + 1..]
                .iter()
                .position(|l| l.starts_with("::"))
                .map(|i| line_idx + 1 + i)
                .unwrap_or(lines.len());

            if end_line > line_idx + 1 {
                ranges.push(FoldingRange {
                    start_line: (line_idx + 1) as u32,
                    start_character: None,
                    end_line: (end_line - 1) as u32,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }
    }

    if ranges.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ranges))
    }
}

pub(crate) async fn document_link(
    state: &ServerState,
    params: DocumentLinkParams,
) -> Result<Option<Vec<DocumentLink>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let mut links = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("[[") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find("]]") {
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;
                let link_text = &line[content_start..content_end];

                let target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                let target = target.trim();

                if !target.is_empty() {
                    // Find the target passage's URI
                    if let Some(target_uri) = inner.workspace.find_passage_file_uri(target) {
                        links.push(DocumentLink {
                            range: Range {
                                start: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, content_start) },
                                end: Position { line: line_idx as u32, character: helpers::utf16_len_up_to(line, content_end) },
                            },
                            target: Some(target_uri),
                            tooltip: Some(format!("Go to {}", target)),
                            data: None,
                        });
                    }
                }

                search_from = abs_start + rel_end + 2;
            } else {
                break;
            }
        }
    }

    if links.is_empty() {
        Ok(None)
    } else {
        Ok(Some(links))
    }
}

pub(crate) async fn selection_range(
    state: &ServerState,
    params: SelectionRangeParams,
) -> Result<Option<Vec<SelectionRange>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let mut results = Vec::new();

    for position in &params.positions {
        let mut range_chain: Vec<Range> = Vec::new();

        // Level 1: Link text (if inside a [[...]])
        if let Some(_target) = helpers::find_link_target_at_position(text, *position) {
            let line_text = text.lines().nth(position.line as usize).unwrap_or("");
            let mut search_from = 0;
            while let Some(rel_start) = line_text[search_from..].find("[[") {
                let abs_start = search_from + rel_start;
                if let Some(rel_end) = line_text[abs_start..].find("]]") {
                    let content_start = abs_start + 2;
                    let content_end = abs_start + rel_end;
                    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);
                    if byte_pos >= content_start && byte_pos <= content_end {
                        // Link text range
                        let target_start = if let Some(arrow) = line_text[content_start..content_end].find("->") {
                            content_start + arrow + 2
                        } else if let Some(pipe) = line_text[content_start..content_end].find('|') {
                            content_start + pipe + 1
                        } else {
                            content_start
                        };
                        range_chain.push(Range {
                            start: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, target_start) },
                            end: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, content_end) },
                        });
                        // Full link range
                        range_chain.push(Range {
                            start: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, abs_start) },
                            end: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, abs_start + rel_end + 2) },
                        });
                        break;
                    }
                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }

        // Level 2: Passage body range
        if let Some(name) = helpers::find_passage_at_position(text, *position) {
            let header_line = position.line;
            let lines: Vec<&str> = text.lines().collect();
            let end_line = lines[(header_line as usize) + 1..]
                .iter()
                .position(|l| l.starts_with("::"))
                .map(|i| header_line + 1 + i as u32)
                .unwrap_or(lines.len() as u32 - 1);

            range_chain.push(Range {
                start: Position { line: header_line + 1, character: 0 },
                end: Position { line: end_line, character: 0 },
            });

            // Level 3: Passage header + body
            range_chain.push(Range {
                start: Position { line: header_line, character: 0 },
                end: Position { line: end_line, character: 0 },
            });

            let _ = name; // used above
        }

        // Build the linked SelectionRange list (innermost first)
        let sel_range = range_chain.into_iter().rev().fold(None::<SelectionRange>, |parent, range| {
            Some(SelectionRange {
                range,
                parent: parent.map(Box::new),
            })
        });

        results.push(sel_range.unwrap_or(SelectionRange {
            range: Range {
                start: *position,
                end: Position { line: position.line, character: position.character + 1 },
            },
            parent: None,
        }));
    }

    Ok(Some(results))
}

pub(crate) async fn signature_help(
    state: &ServerState,
    params: SignatureHelpParams,
) -> Result<Option<SignatureHelp>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // Only provide signature help for formats with macro catalogs
    let Some(plugin) = plugin else {
        return Ok(None);
    };
    if plugin.builtin_macros().is_empty() {
        return Ok(None);
    }

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    // Find if cursor is inside a <<macro ...>> construct
    let line_text = match text.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return Ok(None),
    };

    let mut search_from = 0;
    while let Some(rel_start) = line_text[search_from..].find("<<") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line_text[abs_start..].find(">>") {
            let content_start = abs_start + 2;
            let content_end = abs_start + rel_end;
            let char_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);

            if char_pos >= content_start && char_pos <= content_end {
                let macro_content = &line_text[content_start..content_end];
                let macro_name = macro_content.split_whitespace().next().unwrap_or("");

                if let Some(mdef) = plugin.find_macro(macro_name) {
                    let after_name = &macro_content[macro_name.len()..];
                    let active_param = after_name.matches(',').count() as u32;

                    let params_list: Vec<ParameterInformation> = if let Some(args) = mdef.args {
                        args.iter().map(|a| ParameterInformation {
                            label: ParameterLabel::Simple(a.label.to_string()),
                            documentation: None,
                        }).collect()
                    } else {
                        Vec::new()
                    };

                    let sig_str = if let Some(args) = mdef.args {
                        args.iter().map(|a| a.label).collect::<Vec<_>>().join(", ")
                    } else {
                        String::new()
                    };

                    let has_params = !params_list.is_empty();
                    return Ok(Some(SignatureHelp {
                        signatures: vec![SignatureInformation {
                            label: format!("<<{} {}>>", mdef.name, sig_str),
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: mdef.description.to_string(),
                            })),
                            parameters: if has_params { Some(params_list) } else { None },
                            active_parameter: if has_params { Some(active_param) } else { None },
                        }],
                        active_signature: Some(0),
                        active_parameter: if has_params { Some(active_param) } else { None },
                    }));
                }
            }

            search_from = abs_start + rel_end + 2;
        } else {
            // Unclosed macro — cursor might be inside
            let content_start = abs_start + 2;
            let char_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);
            if char_pos >= content_start {
                let macro_content = &line_text[content_start..];
                let macro_name = macro_content.split_whitespace().next().unwrap_or("");

                if let Some(mdef) = plugin.find_macro(macro_name) {
                    let after_name = &macro_content[macro_name.len()..];
                    let active_param = after_name.matches(',').count() as u32;

                    let params_list: Vec<ParameterInformation> = if let Some(args) = mdef.args {
                        args.iter().map(|a| ParameterInformation {
                            label: ParameterLabel::Simple(a.label.to_string()),
                            documentation: None,
                        }).collect()
                    } else {
                        Vec::new()
                    };

                    let sig_str = if let Some(args) = mdef.args {
                        args.iter().map(|a| a.label).collect::<Vec<_>>().join(", ")
                    } else {
                        String::new()
                    };

                    let has_params = !params_list.is_empty();
                    return Ok(Some(SignatureHelp {
                        signatures: vec![SignatureInformation {
                            label: format!("<<{} {}>>", mdef.name, sig_str),
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: mdef.description.to_string(),
                            })),
                            parameters: if has_params { Some(params_list) } else { None },
                            active_parameter: if has_params { Some(active_param) } else { None },
                        }],
                        active_signature: Some(0),
                        active_parameter: if has_params { Some(active_param) } else { None },
                    }));
                }
            }
            break;
        }
    }

    Ok(None)
}
