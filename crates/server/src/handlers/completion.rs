//! Completion handlers: completion, completion_resolve.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use knot_core::passage::StoryFormat;
use lsp_types::*;
use std::collections::HashMap;
pub(crate) async fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let uri = &params.text_document_position.text_document.uri;
    let _position = params.text_document_position.position;

    // Determine the trigger character
    let trigger = params.context.as_ref().and_then(|ctx| ctx.trigger_character.clone());

    let _text = match inner.open_documents.get(uri) {
        Some(t) => t,
        None => return Ok(None),
    };

    let format = inner.workspace.resolve_format();
    let mut items: Vec<CompletionItem> = Vec::new();

    match trigger.as_deref() {
        Some("[") => {
            // Passage link completion — offer snippet [[${1:passage}]]
            let names = inner.workspace.all_passage_names();
            for (i, name) in names.iter().enumerate() {
                // Find which passages link here for detail
                let written_in = helpers::find_passages_linking_to(&inner.workspace, name);
                let detail_str = if written_in.is_empty() {
                    "Passage".to_string()
                } else if written_in.len() <= 3 {
                    format!("Passage — linked from {}", written_in.join(", "))
                } else {
                    format!("Passage — linked from {} passages", written_in.len())
                };
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(detail_str),
                    sort_text: Some(format!("0_{:06}", i)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(format!("[[{}]]", name)),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    insert_text_mode: Some(InsertTextMode::ADJUST_INDENTATION),
                    commit_characters: Some(vec!["]".to_string()]),
                    preselect: if name == "Start" { Some(true) } else { None },
                    data: Some(serde_json::json!({"type": "passage", "name": name})),
                    ..Default::default()
                });
            }
        }
        Some("$") => {
            // Variable completion from workspace
            let mut var_info: HashMap<String, Vec<String>> = HashMap::new();
            for doc in inner.workspace.documents() {
                for passage in &doc.passages {
                    for var in &passage.vars {
                        if var.is_temporary { continue; }
                        var_info
                            .entry(var.name.clone())
                            .or_default()
                            .push(passage.name.clone());
                    }
                }
            }
            let mut sorted_vars: Vec<_> = var_info.iter().collect();
            sorted_vars.sort_by(|a, b| a.0.cmp(b.0));
            for (i, (var_name, passages)) in sorted_vars.iter().enumerate() {
                let detail_str = if passages.len() <= 3 {
                    format!("Variable — {}", passages.join(", "))
                } else {
                    format!("Variable — {} passages", passages.len())
                };
                items.push(CompletionItem {
                    label: (*var_name).clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(detail_str),
                    sort_text: Some(format!("1_{:06}", i)),
                    filter_text: Some(var_name.trim_start_matches('$').to_string()),
                    insert_text: Some(var_name.to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    commit_characters: Some(vec![" ".to_string(), "\n".to_string()]),
                    data: Some(serde_json::json!({"type": "variable", "name": var_name})),
                    ..Default::default()
                });
            }
        }
        Some("<") => {
            // SugarCube macro completion
            if matches!(format, StoryFormat::SugarCube) {
                let macros_list = macros::sugarcube_macro_signatures();
                for (i, m) in macros_list.iter().enumerate() {
                    items.push(CompletionItem {
                        label: m.name.to_string(),
                        kind: Some(CompletionItemKind::FUNCTION),
                        detail: Some(format!("<<{} {}>>", m.name, m.signature)),
                        sort_text: Some(format!("2_{:06}", i)),
                        filter_text: Some(m.name.to_string()),
                        insert_text: Some(format!("<<{}{}>>", m.name, m.insert_snippet())),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        commit_characters: Some(vec![">".to_string()]),
                        tags: if m.deprecated { Some(vec![CompletionItemTag::DEPRECATED]) } else { None },
                        deprecated: if m.deprecated { Some(true) } else { None },
                        data: Some(serde_json::json!({"type": "macro", "name": m.name})),
                        ..Default::default()
                    });
                }
            }
        }
        _ => {
            // Default: just passage names
            let names = inner.workspace.all_passage_names();
            for (i, name) in names.iter().enumerate() {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some("Passage".to_string()),
                    sort_text: Some(format!("0_{:06}", i)),
                    preselect: if name == "Start" { Some(true) } else { None },
                    data: Some(serde_json::json!({"type": "passage", "name": name})),
                    ..Default::default()
                });
            }
        }
    }

    if items.is_empty() {
        Ok(None)
    } else {
        Ok(Some(CompletionResponse::Array(items)))
    }
}

pub(crate) async fn completion_resolve(
    state: &ServerState,
    params: CompletionItem,
) -> Result<CompletionItem, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;

    if let Some(data) = &params.data {
        let comp_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");

        match comp_type {
            "passage" => {
                if let Some((doc, passage)) = inner.workspace.find_passage(name) {
                    let links_count = passage.links.len();
                    let incoming = helpers::count_incoming_links(&inner.workspace, name);
                    let doc_markdown = format!(
                        "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                        name,
                        doc.uri.as_str(),
                        links_count,
                        incoming,
                        if passage.tags.is_empty() { "none".to_string() } else { passage.tags.join(", ") }
                    );
                    return Ok(CompletionItem {
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc_markdown,
                        })),
                        ..params
                    });
                }
            }
            "variable" => {
                let doc_markdown = format!("**{}**\n\nStory variable (persistent across passages)", name);
                return Ok(CompletionItem {
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc_markdown,
                    })),
                    ..params
                });
            }
            "macro" => {
                if let Some(sig) = macros::sugarcube_macro_signatures().iter().find(|m| m.name == name) {
                    let doc_markdown = format!(
                        "**<<{} {}>>**\n\n{}",
                        sig.name, sig.signature, sig.description
                    );
                    return Ok(CompletionItem {
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc_markdown,
                        })),
                        ..params
                    });
                }
            }
            _ => {}
        }
    }

    Ok(params)
}
