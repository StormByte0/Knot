//! Hover handler: macro, variable, link, passage hover.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use lsp_types::*;
pub(crate) async fn hover(
    state: &ServerState,
    params: HoverParams,
) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;
    let Some(text) = inner.open_documents.get(uri) else {
        return Ok(None);
    };

    let line_idx = position.line as usize;
    let char_pos = position.character as usize;
    let line = text.lines().nth(line_idx).unwrap_or("");

    // 1. Try macro hover — check if cursor is inside <<...>>
    {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("<<") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find(">>") {
                let abs_end = abs_start + rel_end + 2;
                if char_pos >= abs_start && char_pos <= abs_end {
                    let content = &line[abs_start + 2..abs_end - 2];
                    let macro_name = content.split_whitespace().next().unwrap_or(content).trim();

                    if let Some(sig) =
                        macros::sugarcube_macro_signatures().iter().find(|m| m.name == macro_name)
                    {
                        let hover_text = format!(
                            "**<<{} {}>>**\n\n{}\n\n---\n\nSugarCube macro",
                            sig.name, sig.signature, sig.description
                        );
                        return Ok(Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(Range {
                                start: Position {
                                    line: line_idx as u32,
                                    character: abs_start as u32,
                                },
                                end: Position {
                                    line: line_idx as u32,
                                    character: abs_end as u32,
                                },
                            }),
                        }));
                    }
                }
                search_from = abs_end;
            } else {
                break;
            }
        }
    }

    // 2. Try variable hover — check if cursor is on $variable
    {
        let chars: Vec<char> = line.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos] == '$'
                && pos + 1 < chars.len()
                && (chars[pos + 1].is_alphabetic() || chars[pos + 1] == '_')
            {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                    pos += 1;
                }
                let var_name: String = chars[start..pos].iter().collect();
                let byte_start: usize = chars[..start].iter().map(|c| c.len_utf8()).sum();
                let byte_end: usize = chars[..pos].iter().map(|c| c.len_utf8()).sum();

                if char_pos >= byte_start && char_pos <= byte_end {
                    // Find where this variable is written and read across the workspace
                    let mut write_locations: Vec<String> = Vec::new();
                    let mut read_count = 0;
                    for doc in inner.workspace.documents() {
                        for passage in &doc.passages {
                            for var in &passage.vars {
                                if var.name == var_name && !var.is_temporary {
                                    match var.kind {
                                        knot_core::passage::VarKind::Write => {
                                            write_locations.push(passage.name.clone());
                                        }
                                        knot_core::passage::VarKind::Read => {
                                            read_count += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let write_info = if write_locations.is_empty() {
                        "Never written".to_string()
                    } else if write_locations.len() <= 5 {
                        format!("Written in: {}", write_locations.join(", "))
                    } else {
                        format!("Written in {} passages", write_locations.len())
                    };

                    let hover_text = format!(
                        "**{}**\n\n{}\nRead in {} location(s)\n\n---\n\nStory variable (persistent across passages)",
                        var_name, write_info, read_count
                    );
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(Range {
                            start: Position {
                                line: line_idx as u32,
                                character: byte_start as u32,
                            },
                            end: Position {
                                line: line_idx as u32,
                                character: byte_end as u32,
                            },
                        }),
                    }));
                }
            } else {
                pos += 1;
            }
        }
    }

    // 3. Try link hover — check if cursor is inside [[...]]
    {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("[[") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find("]]") {
                let abs_end = abs_start + rel_end + 2;
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;

                if char_pos >= abs_start && char_pos <= abs_end {
                    let link_text = &line[content_start..content_end];

                    // Extract target: handle arrow (->) and pipe (|) syntax
                    let target = if let Some(arrow) = link_text.find("->") {
                        &link_text[arrow + 2..]
                    } else if let Some(pipe) = link_text.find('|') {
                        &link_text[pipe + 1..]
                    } else {
                        link_text
                    };
                    let target = target.trim();

                    if !target.is_empty() {
                        if let Some((doc, passage)) = inner.workspace.find_passage(target) {
                            let incoming = helpers::count_incoming_links(&inner.workspace, target);
                            let hover_text = format!(
                                "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                                target,
                                doc.uri.as_str(),
                                passage.links.len(),
                                incoming,
                                if passage.tags.is_empty() { "none".to_string() } else { passage.tags.join(", ") }
                            );
                            return Ok(Some(Hover {
                                contents: HoverContents::Markup(MarkupContent {
                                    kind: MarkupKind::Markdown,
                                    value: hover_text,
                                }),
                                range: Some(Range {
                                    start: Position {
                                        line: line_idx as u32,
                                        character: abs_start as u32,
                                    },
                                    end: Position {
                                        line: line_idx as u32,
                                        character: abs_end as u32,
                                    },
                                }),
                            }));
                        }
                    }
                }
                search_from = abs_end;
            } else {
                break;
            }
        }
    }

    // 4. Try passage hover — check if cursor is on a passage header
    if let Some(passage_name) = helpers::find_passage_at_position(text, position)
        && let Some((_, passage)) = inner.workspace.find_passage(&passage_name)
    {
        let links_count = passage.links.len();
        let vars_count = passage.vars.len();
        let tags = if passage.tags.is_empty() {
            "none".to_string()
        } else {
            passage.tags.join(", ")
        };

        // Count incoming links (other passages that link to this one)
        let incoming = helpers::count_incoming_links(&inner.workspace, &passage_name);

        let hover_text = format!(
            "**{}**\n\nLinks: {} | Variables: {} | Tags: {} | Incoming: {}",
            passage.name, links_count, vars_count, tags, incoming
        );
        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text,
            }),
            range: None,
        }));
    }

    Ok(None)
}
