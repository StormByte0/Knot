//! Completion handlers: completion, completion_resolve.
//!
//! Provides context-aware completions for format-specific macros (with snippets),
//! passage names, story/temporary variables, and close-tag completion.
//!
//! All format-specific logic is delegated to the active format plugin obtained
//! from the `FormatRegistry`. No format-specific data is imported directly.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_formats::plugin::FormatPlugin;
use knot_formats::types::MacroArgKind;
use lsp_types::*;
use std::collections::HashMap;

pub(crate) async fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    // Determine the trigger character
    let trigger = params.context.as_ref().and_then(|ctx| ctx.trigger_character.clone());

    let text = match inner.open_documents.get(uri) {
        Some(t) => t,
        None => return Ok(None),
    };

    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);
    let mut items: Vec<CompletionItem> = Vec::new();

    // Get line text for context-aware completion
    let line_idx = position.line as usize;
    let char_pos = position.character as usize;
    let line_text = text.lines().nth(line_idx).unwrap_or("");
    let before_cursor = &line_text[..char_pos.min(line_text.len())];

    // ── Close-tag context: <</ ... ──────────────────────────────────────
    if let Some(plugin) = plugin {
        if !plugin.block_macro_names().is_empty() {
            if let Some(close_items) = try_close_tag_completion(before_cursor, &inner.workspace, plugin) {
                return Ok(Some(CompletionResponse::Array(close_items)));
            }
        }
    }

    match trigger.as_deref() {
        // ── "[" trigger: passage link completion ─────────────────────────
        Some("[") => {
            let names = inner.workspace.all_passage_names();
            for (i, name) in names.iter().enumerate() {
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

        // ── "$" trigger: variable completion (story + temp) ──────────────
        Some("$") | Some("_") => {
            let is_temp = trigger.as_deref() == Some("_");
            let mut var_info: HashMap<String, Vec<String>> = HashMap::new();
            for doc in inner.workspace.documents() {
                for passage in &doc.passages {
                    for var in &passage.vars {
                        // Filter by temporary flag based on trigger
                        if is_temp && !var.is_temporary {
                            continue;
                        }
                        if !is_temp && var.is_temporary {
                            continue;
                        }
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
                    format!("{} — {}", if is_temp { "Temp variable" } else { "Variable" }, passages.join(", "))
                } else {
                    format!("{} — {} passages", if is_temp { "Temp variable" } else { "Variable" }, passages.len())
                };
                items.push(CompletionItem {
                    label: (*var_name).clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(detail_str),
                    sort_text: Some(format!("1_{:06}", i)),
                    filter_text: Some(var_name.trim_start_matches('$').trim_start_matches('_').to_string()),
                    insert_text: Some(var_name.to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    commit_characters: Some(vec![" ".to_string(), "\n".to_string()]),
                    data: Some(serde_json::json!({"type": "variable", "name": var_name, "is_temp": is_temp})),
                    ..Default::default()
                });
            }
        }

        // ── "<" trigger: macro completion with rich snippets ─────────────
        Some("<") => {
            if let Some(plugin) = plugin {
                if !plugin.builtin_macros().is_empty() {
                    // Check if we're inside a macro-open context
                    let in_macro_context = before_cursor.ends_with("<<")
                        || before_cursor.rfind("<<").map_or(false, |pos| {
                            !before_cursor[pos..].contains(">>")
                        });

                    if in_macro_context {
                        items = build_macro_completions(plugin);
                    }
                }
            }
        }

        // ── "\"" trigger: passage name completion inside quotes ──────────
        Some("\"") => {
            if let Some(plugin) = plugin {
                if !plugin.passage_arg_macro_names().is_empty() {
                    // Check if we're inside a passage-arg macro context
                    if let Some(passage_items) = try_passage_in_quote_completion(before_cursor, &inner.workspace, plugin) {
                        return Ok(Some(CompletionResponse::Array(passage_items)));
                    }
                }

                // Fall through to default
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

        // ── "." trigger: global object property completion ───────────────
        Some(".") => {
            if let Some(plugin) = plugin {
                if !plugin.global_object_names().is_empty() {
                    // Check if preceding text ends with a global object name + "."
                    // e.g., "State." → offer "variables", "temporary", etc.
                    if let Some(dot_items) = try_global_property_completion(before_cursor, plugin) {
                        return Ok(Some(CompletionResponse::Array(dot_items)));
                    }
                }
            }
        }

        // ── Default: passage names ───────────────────────────────────────
        _ => {
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
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

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
                let is_temp = data.get("is_temp").and_then(|v| v.as_bool()).unwrap_or(false);
                // Use the plugin to describe the variable sigil if available
                let format_desc = plugin
                    .and_then(|p| p.describe_variable_sigil(if is_temp { '_' } else { '$' }))
                    .unwrap_or(if is_temp { "temporary" } else { "story" });

                let doc_markdown = format!(
                    "**{}**\n\n{} variable — {}",
                    name,
                    format_desc,
                    if is_temp { "scoped to the current passage" } else { "persists across passages" }
                );
                return Ok(CompletionItem {
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc_markdown,
                    })),
                    ..params
                });
            }
            "macro" => {
                if let Some(plugin) = plugin {
                    if let Some(mdef) = plugin.find_macro(name) {
                        let mut doc_markdown = format!(
                            "**<<{}>>**\n\n{}",
                            mdef.name, mdef.description
                        );
                        if mdef.deprecated {
                            if let Some(msg) = mdef.deprecation_message {
                                doc_markdown.push_str(&format!("\n\n⚠ **Deprecated**: {}", msg));
                            }
                        }
                        // Add arg info
                        if let Some(args) = mdef.args {
                            if !args.is_empty() {
                                doc_markdown.push_str("\n\n**Parameters:**\n");
                                for arg in args {
                                    let req = if arg.is_required { " (required)" } else { "" };
                                    let kind = match arg.kind {
                                        MacroArgKind::Expression => "expr",
                                        MacroArgKind::String => "string",
                                        MacroArgKind::Selector => "selector",
                                        MacroArgKind::Variable => "variable",
                                    };
                                    let flags = if arg.is_passage_ref { " 🔗passage" } else { "" };
                                    doc_markdown.push_str(&format!(
                                        "- `{}{}`: {}{}\n",
                                        arg.label, req, kind, flags
                                    ));
                                }
                            }
                        }
                        return Ok(CompletionItem {
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: doc_markdown,
                            })),
                            ..params
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(params)
}

// ===========================================================================
// Private helpers — context-aware completion builders
// ===========================================================================

/// Build macro completion items using the format plugin's macro catalog.
fn build_macro_completions(plugin: &dyn FormatPlugin) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    for mdef in plugin.builtin_macros() {
        let snippet = plugin.build_macro_snippet(mdef.name, mdef.has_body);
        let category = mdef.category.to_string();

        items.push(CompletionItem {
            label: format!("<<{}>>", mdef.name),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("[{}] {}", category, mdef.description)),
            sort_text: Some(format!("2_{:06}_{}", 0, mdef.name)),
            filter_text: Some(mdef.name.to_string()),
            insert_text: Some(snippet),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            commit_characters: Some(vec![">".to_string()]),
            tags: if mdef.deprecated {
                Some(vec![CompletionItemTag::DEPRECATED])
            } else {
                None
            },
            deprecated: if mdef.deprecated { Some(true) } else { None },
            data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
            ..Default::default()
        });
    }

    items
}

/// Try close-tag completion when the user types `<</`.
///
/// Analyzes the text before the cursor to find unclosed block macros,
/// then offers matching close tags ordered by nesting depth.
fn try_close_tag_completion(
    before_cursor: &str,
    _workspace: &knot_core::Workspace,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    // Check if we're in a close-tag context: `<</`
    let close_match = before_cursor.rfind("<</");
    if close_match.is_none() {
        // Also check for `<< /` pattern
        if !before_cursor.ends_with("<<") {
            return None;
        }
    }

    // Collect open/close macro events to determine the stack
    let block_names = plugin.block_macro_names();
    let mut events: Vec<(usize, &str, bool)> = Vec::new(); // (pos, name, is_open)

    // Open macros: <<name ...>> or <<name>>
    let open_re = regex::Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s[^>]*)?>>").ok()?;
    for caps in open_re.captures_iter(before_cursor) {
        let m = caps.get(0)?;
        let name = caps.get(1)?.as_str();
        if block_names.contains(name) {
            events.push((m.start(), name, true));
        }
    }

    // Close macros: <</name>>
    let close_re = regex::Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").ok()?;
    for caps in close_re.captures_iter(before_cursor) {
        let m = caps.get(0)?;
        let name = caps.get(1)?.as_str();
        events.push((m.start(), name, false));
    }

    // Sort by position
    events.sort_by_key(|(pos, _, _)| *pos);

    // Build the stack of unclosed open tags
    let mut open_stack: Vec<&str> = Vec::new();
    for (_, name, is_open) in &events {
        if *is_open {
            open_stack.push(name);
        } else {
            // Find and remove the matching open tag from the stack (innermost first)
            for i in (0..open_stack.len()).rev() {
                if open_stack[i] == *name {
                    open_stack.remove(i);
                    break;
                }
            }
        }
    }

    // Determine what partial text the user has typed after <</
    let partial = if let Some(pos) = close_match {
        &before_cursor[pos + 3..]
    } else {
        ""
    };

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Offer close tags for unclosed macros (innermost first)
    for (depth, &name) in open_stack.iter().rev().enumerate() {
        if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
            continue;
        }
        seen.insert(name);
        items.push(CompletionItem {
            label: format!("</{}>>", name),
            filter_text: Some(name.to_string()),
            insert_text: Some(format!("{}>>", name)),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("Close <<{}>>", name)),
            sort_text: Some(format!("0_{:04}_{}", depth, name)),
            ..Default::default()
        });
    }

    // If no unclosed macros found, offer all block macro close tags as fallback
    if items.is_empty() {
        for name in &block_names {
            if !partial.is_empty() && !name.starts_with(partial) {
                continue;
            }
            items.push(CompletionItem {
                label: format!("</{}>>", name),
                filter_text: Some(name.to_string()),
                insert_text: Some(format!("{}>>", name)),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("Close <<{}>>", name)),
                sort_text: Some(format!("1_{}", name)),
                ..Default::default()
            });
        }
    }

    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// Try passage name completion inside quotes within a passage-arg macro.
///
/// Detects contexts like `<<goto "...` or `<<link "label" "...` and offers
/// passage name completions.
fn try_passage_in_quote_completion(
    before_cursor: &str,
    workspace: &knot_core::Workspace,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    // Find the most recent `<<` that hasn't been closed with `>>`
    let last_open = before_cursor.rfind("<<")?;
    let after_open = &before_cursor[last_open + 2..];

    // Must not contain >> (already closed)
    if after_open.contains(">>") {
        return None;
    }

    // Extract the macro name
    let macro_name = after_open.split_whitespace().next()?;

    // Check if this macro has passage-ref args
    let passage_arg_names = plugin.passage_arg_macro_names();
    if !passage_arg_names.contains(macro_name) {
        return None;
    }

    // Count the number of quoted strings so far to determine which arg we're in
    let arg_count = after_open.matches('"').count() / 2;
    let is_in_quote = after_open.matches('"').count() % 2 == 1;

    if !is_in_quote {
        return None; // Cursor isn't inside a quoted string
    }

    // Check if the current arg position is the passage ref position
    let current_arg = arg_count; // 0-indexed arg we're completing
    let passage_idx = plugin.get_passage_arg_index(macro_name, current_arg + 1);

    if passage_idx < 0 || passage_idx as usize != current_arg {
        // This might be the label arg, not the passage arg — but we still
        // offer passage names since the user might be typing a passage directly
    }

    let names = workspace.all_passage_names();
    let mut items = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let incoming = helpers::count_incoming_links(workspace, name);
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(format!("Passage — {} incoming", incoming)),
            sort_text: Some(format!("0_{:06}", i)),
            filter_text: Some(name.clone()),
            insert_text: Some(name.clone()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            preselect: if name == "Start" { Some(true) } else { None },
            data: Some(serde_json::json!({"type": "passage", "name": name})),
            ..Default::default()
        });
    }

    Some(items)
}

/// Try global object property completion after a dot.
///
/// E.g., after `State.` offers `variables`, `temporary`, `turns`, etc.
///
/// The entry guard checks the format plugin's `global_object_names()` to
/// determine if the identifier is a known global object. The property lists
/// themselves are currently still hardcoded per format (as a temporary measure
/// until the format plugin's global defs include property lists).
fn try_global_property_completion(
    before_cursor: &str,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    // Find the identifier before the dot
    let before_dot = before_cursor.trim_end_matches('.');
    let ident = before_dot
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()?;

    // Check if the ident is a known global object in the current format
    if !plugin.global_object_names().contains(ident) {
        return None;
    }

    // Return property completions for known global objects.
    // TODO: Move these property lists into the format plugin once the
    // GlobalDef type supports property/method definitions.
    let items = match ident {
        "State" => vec![
            simple_property("variables", "Record<string, unknown> — story variables"),
            simple_property("temporary", "Record<string, unknown> — temporary variables"),
            simple_property("turns", "number — turn count"),
            simple_property("passage", "string — current passage name"),
            simple_property("active", "object — active passage info"),
            simple_property("top", "object — top passage info"),
            simple_property("history", "array — passage history"),
            simple_property("has()", "boolean — check if passage visited"),
            simple_property("hasTag()", "boolean — check if tag visited"),
            simple_property("index", "number — current history index"),
            simple_property("size", "number — history size"),
        ],
        "Engine" => vec![
            simple_property("play()", "void — navigate to passage"),
            simple_property("forward()", "void — go forward in history"),
            simple_property("backward()", "void — go backward in history"),
            simple_property("goto()", "void — navigate to passage"),
            simple_property("isIdle()", "boolean — is engine idle"),
            simple_property("isPlaying()", "boolean — is engine playing"),
        ],
        "Story" => vec![
            simple_property("title", "string — story title"),
            simple_property("has()", "boolean — check passage exists"),
            simple_property("get()", "object — get passage data"),
            simple_property("filter()", "array — filter passages"),
        ],
        "Save" => vec![
            simple_property("save()", "void — save game"),
            simple_property("load()", "void — load game"),
            simple_property("delete()", "void — delete save"),
            simple_property("ok()", "boolean — check save exists"),
            simple_property("sizes()", "object — save sizes"),
        ],
        "Config" => vec![
            simple_property("debug", "boolean — debug mode"),
            simple_property("history", "object — history config"),
            simple_property("macros", "object — macro config"),
            simple_property("navigation", "object — navigation config"),
            simple_property("ui", "object — UI config"),
        ],
        "UI" => vec![
            simple_property("alert()", "void — show alert dialog"),
            simple_property("restart()", "void — restart story"),
            simple_property("squash()", "void — squash history"),
            simple_property("goto()", "void — navigate to passage"),
            simple_property("include()", "void — include passage"),
        ],
        _ => return None,
    };

    Some(items)
}

/// Create a simple property completion item.
fn simple_property(name: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: Some(if name.ends_with("()") {
            CompletionItemKind::METHOD
        } else {
            CompletionItemKind::PROPERTY
        }),
        detail: Some(detail.to_string()),
        insert_text: Some(name.to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    }
}
