//! Structure handlers: folding_range, document_link, selection_range,
//! signature_help.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_formats::plugin::MacroBlockEvent;
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

    // ── Passage body folding (span-based) ──────────────────────────
    if let Some(doc) = inner.workspace.get_document(&uri) {
        let passages = &doc.passages;
        for (i, passage) in passages.iter().enumerate() {
            let span_start = passage.abs_offset(passage.span.start).min(text.len());
            let header_end = text[span_start..]
                .find('\n')
                .map(|n| span_start + n)
                .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));

            // End of passage body: start of next passage or end of document
            let body_end_offset = if i + 1 < passages.len() {
                passages[i + 1].abs_offset(passages[i + 1].span.start).min(text.len())
            } else {
                text.len()
            };

            let body_start_pos = helpers::byte_offset_to_position(text, (header_end + 1).min(text.len()));
            let body_end_pos = helpers::byte_offset_to_position(text, body_end_offset);

            if body_end_pos.line > body_start_pos.line {
                ranges.push(FoldingRange {
                    start_line: body_start_pos.line,
                    start_character: None,
                    end_line: body_end_pos.line.saturating_sub(1),
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }
    }

    // ── Macro block folding ──────────────────────────────────────
    // Use the format plugin for format-agnostic macro block detection.
    let format = inner.workspace.resolve_format();
    if let Some(plugin) = inner.format_registry.get(&format) {
        let lines: Vec<&str> = text.lines().collect();
        let mut open_stack: Vec<(String, u32)> = Vec::new(); // (name, start_line)

        // Collect all macro block events from the format plugin
        let mut all_events: Vec<MacroBlockEvent> = Vec::new();
        for (line_idx, line) in lines.iter().enumerate() {
            all_events.extend(plugin.scan_line_for_macro_events(line, line_idx as u32));
        }

        for event in all_events {
            if event.is_open {
                open_stack.push((event.name, event.line));
            } else {
                // Find matching open tag on stack (search backward)
                if let Some(pos) = open_stack.iter().rposition(|(n, _)| n == &event.name) {
                    let (_, start_line) = open_stack.remove(pos);
                    let end_line = event.line;
                    if end_line > start_line + 1 {
                        ranges.push(FoldingRange {
                            start_line,
                            start_character: None,
                            end_line,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
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
    // Short-circuit if the server is shutting down
    if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(None);
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut links = Vec::new();

    // Use workspace passage/link data for span-based resolution.
    for passage in &doc.passages {
        for link in &passage.links {
            let target = link.target.trim();
            if !target.is_empty() {
                if let Some(target_uri) = inner.workspace.find_passage_file_uri(target) {
                    links.push(DocumentLink {
                        range: helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span)),
                        target: Some(target_uri),
                        tooltip: Some(format!("Go to {}", target)),
                        data: None,
                    });
                }
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

    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut results = Vec::new();
    let passages = &doc.passages;

    for position in &params.positions {
        let mut range_chain: Vec<Range> = Vec::new();
        let byte_offset = helpers::position_to_byte_offset(text, *position);

        // Level 1: Link text (if inside a [[...]])
        'link_search: for passage in passages.iter() {
            for link in &passage.links {
                if passage.span_contains_abs_offset(&link.span, byte_offset) {
                    // Found the link containing the cursor.
                    // Extract the link content to find the target portion.
                    let abs_link_span = passage.abs_range(&link.span);
                    let link_text = &text[abs_link_span.start.min(text.len())..abs_link_span.end.min(text.len())];
                    let content = &link_text[2..link_text.len().saturating_sub(2)];

                    // Compute the target range within the link
                    let target_start_offset = if let Some(arrow) = content.find("->") {
                        abs_link_span.start + 2 + arrow + 2
                    } else if let Some(pipe) = content.find('|') {
                        abs_link_span.start + 2 + pipe + 1
                    } else {
                        abs_link_span.start + 2
                    };
                    let target_end_offset = abs_link_span.end.saturating_sub(2);

                    // Link text range (just the target portion)
                    range_chain.push(helpers::byte_range_to_lsp_range(
                        text,
                        &(target_start_offset..target_end_offset),
                    ));

                    // Full link range (entire [[...]])
                    range_chain.push(helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span)));

                    break 'link_search;
                }
            }
        }

        // Level 2: Passage range (if cursor is within a passage)
        for (i, passage) in passages.iter().enumerate() {
            let span_start = passage.abs_offset(passage.span.start).min(text.len());
            let effective_end = if i + 1 < passages.len() {
                passages[i + 1].abs_offset(passages[i + 1].span.start).min(text.len())
            } else {
                text.len()
            };

            if byte_offset >= span_start && byte_offset < effective_end {
                let header_end = text[span_start..]
                    .find('\n')
                    .map(|n| span_start + n)
                    .unwrap_or(effective_end);

                let body_start_pos = helpers::byte_offset_to_position(text, (header_end + 1).min(text.len()));
                let body_end_pos = helpers::byte_offset_to_position(text, effective_end);
                let header_start_pos = helpers::byte_offset_to_position(text, span_start);

                // Passage body range (from after header to end of passage)
                range_chain.push(Range {
                    start: body_start_pos,
                    end: body_end_pos,
                });

                // Passage header + body range
                range_chain.push(Range {
                    start: header_start_pos,
                    end: body_end_pos,
                });

                break;
            }
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

    let line_text = match text.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return Ok(None),
    };

    // Convert UTF-16 position to byte offset for the format plugin
    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);

    // Delegate macro detection to the format plugin
    let macro_info = match plugin.find_macro_at_position(line_text, byte_pos) {
        Some(info) => info,
        None => return Ok(None),
    };

    if let Some(mdef) = plugin.find_macro(&macro_info.name) {
        // Count commas after the macro name to determine active parameter
        let after_name = &line_text[macro_info.name_range.end..];
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

        // Use the format plugin's signature label — no hardcoded <<>>
        let sig_label = plugin.format_macro_signature_label(mdef.name, &sig_str);

        return Ok(Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: sig_label,
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

    Ok(None)
}
