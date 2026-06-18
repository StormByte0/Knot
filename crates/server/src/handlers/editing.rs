//! Editing handlers: formatting, range_formatting, on_type_formatting,
//! linked_editing_range, prepare_rename, rename.
//!
//! Uses span-based resolution via the workspace index for passage and link
//! lookups instead of re-scanning source text.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn formatting(
    state: &ServerState,
    params: DocumentFormattingParams,
) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
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
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
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
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;
    let ch = &params.ch;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
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

    // Auto-close << with >> (format-specific)
    // Only applies when the detected format uses `<<>>` macro delimiters.
    // SugarCube uses `<<>>`, Harlowe uses `()`, Chapbook uses `[]`, Snowman uses `<% %>`.
    if ch == ">" && byte_pos >= 2 {
        let before = &line_text[..byte_pos];
        if before.ends_with("<<") {
            let format = inner.workspace.resolve_format();
            let plugin = inner.format_registry.get(&format);
            // Check if the format plugin uses angle-bracket delimiters
            // by testing if its macro label contains `<<`
            let uses_angle_brackets = plugin
                .map(|p| p.format_macro_label("if").starts_with("<<"))
                .unwrap_or(false);
            if uses_angle_brackets {
                let insert_pos = Position { line: position.line, character: position.character };
                return Ok(Some(vec![TextEdit {
                    range: Range { start: insert_pos, end: insert_pos },
                    new_text: ">>".to_string(),
                }]));
            }
        }
    }

    Ok(None)
}

pub(crate) async fn linked_editing_range(
    state: &ServerState,
    params: LinkedEditingRangeParams,
) -> Result<Option<LinkedEditingRanges>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    // If cursor is on a passage header name, find all [[link]] references
    if let Some(name) = helpers::find_passage_at_position_span_based(
        text, &inner.workspace, &uri, position,
    ) {
        // Use header_name_span for the primary range when available;
        // fall back to computing from the header line using the header parser.
        let primary_range = if let Some((_, passage)) = inner.workspace.find_passage(&name) {
            if let Some(ref name_span) = passage.header_name_span {
                helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
            } else {
                helpers::compute_passage_name_range_fallback(text, &passage.abs_range(&passage.span))
            }
        } else {
            // Passage not found in workspace — use line-based fallback
            let line_text = text.lines().nth(position.line as usize).unwrap_or("");
            let name_start = line_text.find(&name).unwrap_or(2);
            Range {
                start: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, name_start) },
                end: Position { line: position.line, character: helpers::utf16_len_up_to(line_text, name_start + name.len()) },
            }
        };

        let mut ranges = vec![primary_range];

        // Find all link ranges for this target using workspace data
        if let Some(doc) = inner.workspace.get_document(&uri) {
            for passage in &doc.passages {
                for link in &passage.links {
                    if link.target.trim() == name {
                        ranges.push(helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span)));
                    }
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
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let position = params.position;

    let inner = state.inner.read().await;

    // Delegate to the shared `rename_range_at_cursor` helper. This handles
    // all renamable target types (passages, custom macros, functions,
    // templates) with the failsafe: if the definition can't be confirmed,
    // rename is not allowed (returns None -> F2 does nothing).
    let Some((range, placeholder)) = crate::handlers::navigation::rename_range_at_cursor(&inner, &uri, position) else {
        return Ok(None);
    };

    Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
        range,
        placeholder,
    }))
}

pub(crate) async fn rename(
    state: &ServerState,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    let inner = state.inner.read().await;

    // Resolve the target at the cursor. This is the same resolution used by
    // goto-definition and find-references -- format-isolated, goes through
    // `&dyn FormatPlugin` trait methods.
    let Some(target) = crate::handlers::navigation::resolve_target_at_cursor(&inner, &uri, position) else {
        return Ok(None);
    };

    // **Failsafe**: re-confirm the definition is reachable before producing
    // any edits. This is critical -- without it, a stale registry could cause
    // rename to change all call sites while leaving the definition unchanged,
    // silently breaking the project. The check runs again here (not just in
    // `prepare_rename`) because the document may have changed between when
    // the user pressed F2 and when they pressed Enter.
    if !crate::handlers::navigation::definition_confirmed(&target, &inner) {
        return Ok(None);
    }

    // Collect all rename edits (definition + all call sites) across the
    // workspace. This is the rename equivalent of `references_inner` -- it
    // finds every location that needs to change and produces `TextEdit`s
    // with the new name.
    let changes = crate::handlers::navigation::collect_rename_edits(&target, &new_name, &inner);

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
