//! Completion handlers: completion, completion_resolve.
//!
//! Provides context-aware completions for format-specific macros (with snippets),
//! passage names, story/temporary variables, and close-tag completion.
//!
//! All format-specific logic is delegated to the active format plugin obtained
//! from the `FormatRegistry`. No format-specific data is imported directly.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use knot_formats::plugin::FormatPlugin;
use knot_formats::types::MacroArgKind;
use knot_formats::GlobalProperty;
use lsp_types::*;
use std::collections::HashMap;

pub(crate) async fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;

    // Determine the trigger character
    let trigger = params.context.as_ref().and_then(|ctx| ctx.trigger_character.clone());

    let text = match inner.open_documents.get(&uri) {
        Some(t) => t,
        None => return Ok(None),
    };

    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);
    let mut items: Vec<CompletionItem> = Vec::new();

    // Get line text for context-aware completion
    let line_idx = position.line as usize;
    let line_text = text.lines().nth(line_idx).unwrap_or("");
    // position.character is UTF-16; convert to byte offset for string slicing
    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);
    let before_cursor = &line_text[..byte_pos.min(line_text.len())];

    // ── Close-tag context: <</ ... ──────────────────────────────────────
    if let Some(plugin) = plugin {
        if !plugin.body_macro_names().is_empty() {
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
                    filter_text: Some(var_name.trim_start_matches(|c: char| c == '$' || c == '_').to_string()),
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
                    // Check if we're inside a macro-open context using the format plugin.
                    // The format plugin knows its own opening delimiter patterns.
                    let in_macro_context = plugin.find_macro_at_position(line_text, byte_pos).is_some()
                        || before_cursor.ends_with("<") // partial open delimiter
                        || plugin.detect_close_tag_context(before_cursor).is_some();

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

        // ── "." trigger: global object property completion + variable dot-notation ─
        Some(".") => {
            if let Some(plugin) = plugin {
                // Try variable dot-notation completion first (e.g., $item.)
                if let Some(var_items) = try_variable_dot_completion(before_cursor, &inner.workspace, plugin) {
                    return Ok(Some(CompletionResponse::Array(var_items)));
                }

                // Then try global object property completion (e.g., State.)
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
                // Use the plugin to describe the variable sigil if available.
                // Fall back to format-agnostic labels instead of assuming
                // SugarCube's $/_ sigils.
                let format_desc = plugin
                    .and_then(|p| {
                        let sigils = p.variable_sigils();
                        // Find the sigil matching the is_temp flag:
                        // _ sigil → temporary, any other → persistent
                        sigils.iter().find(|s| (s.sigil == '_') == is_temp)
                            .map(|s| s.description)
                            .or_else(|| sigils.first().map(|s| s.description))
                    })
                    .unwrap_or(if is_temp { "temporary variable" } else { "variable" });

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
                        let kind = macros::classify(mdef.name, mdef, plugin);
                        let mut doc_markdown = format!(
                            "**{}** `{}`\n\n{}",
                            macros::hover_kind_label(kind),
                            plugin.format_macro_label(mdef.name),
                            mdef.description
                        );
                        if mdef.deprecated {
                            if let Some(msg) = mdef.deprecation_message {
                                doc_markdown.push_str(&format!("\n\n⚠ **Deprecated**: {}", msg));
                            }
                        }
                        // Add kind-specific note (e.g., "Close with <</if>>")
                        if let Some(note) = macros::hover_kind_note(kind, mdef.name, plugin) {
                            doc_markdown.push_str(&format!("\n\n{}", note));
                        }
                        // Add arg info
                        if let Some(args) = mdef.args {
                            if !args.is_empty() {
                                doc_markdown.push_str("\n\n**Parameters:**\n");
                                for arg in args {
                                    let req = if arg.is_required { " (required)" } else { "" };
                                    let kind_str = match arg.kind {
                                        MacroArgKind::Expression => "expr",
                                        MacroArgKind::String => "string",
                                        MacroArgKind::Selector => "selector",
                                        MacroArgKind::Variable => "variable",
                                    };
                                    let flags = if arg.is_passage_ref { " 🔗passage" } else { "" };
                                    doc_markdown.push_str(&format!(
                                        "- `{}{}`: {}{}\n",
                                        arg.label, req, kind_str, flags
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
///
/// Uses the `macros` handler's classification system to determine:
/// - `CompletionItemKind` (KEYWORD for operator macros, SNIPPET for
///   blocks/control-flow, FUNCTION for statements/identifiers)
/// - Sort priority (control-flow and blocks first, then keywords, then
///   statements)
/// - Category label with kind annotation
fn build_macro_completions(plugin: &dyn FormatPlugin) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    for mdef in plugin.builtin_macros() {
        let kind = macros::classify(mdef.name, mdef, plugin);
        let snippet = plugin.build_macro_snippet(mdef.name, mdef.body);
        let category = mdef.category.to_string();

        items.push(CompletionItem {
            label: plugin.format_macro_label(mdef.name),
            kind: Some(macros::completion_item_kind(kind)),
            detail: Some(format!("[{}] {} — {}", category, kind, mdef.description)),
            sort_text: Some(macros::sort_text(kind, mdef.name)),
            filter_text: Some(mdef.name.to_string()),
            insert_text: Some(snippet),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            commit_characters: None,
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

/// Try close-tag completion when the user types a format-specific close delimiter.
///
/// Uses the format plugin to detect close-tag context and scan for macro
/// events, so no hardcoded SugarCube `<<>>` patterns are used.
fn try_close_tag_completion(
    before_cursor: &str,
    _workspace: &knot_core::Workspace,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    // Use the format plugin to detect close-tag context
    let partial = plugin.detect_close_tag_context(before_cursor)?;

    // Collect macro block events from the text before cursor to determine
    // the stack of unclosed block macros
    let block_names = plugin.body_macro_names();

    // Only proceed if there are block macros
    if block_names.is_empty() {
        return None;
    }

    // Build open/close event history by scanning lines
    let lines: Vec<&str> = before_cursor.lines().collect();
    let mut events: Vec<(String, bool)> = Vec::new(); // (name, is_open)

    // We need to scan each line for macro events using the plugin
    for (line_idx, line) in lines.iter().enumerate() {
        for event in plugin.scan_line_for_macro_events(line, line_idx as u32) {
            events.push((event.name, event.is_open));
        }
    }

    // Build the stack of unclosed open tags
    let mut open_stack: Vec<String> = Vec::new();
    for (name, is_open) in &events {
        if *is_open {
            open_stack.push(name.clone());
        } else {
            for i in (0..open_stack.len()).rev() {
                if open_stack[i] == *name {
                    open_stack.remove(i);
                    break;
                }
            }
        }
    }

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Offer close tags for unclosed macros (innermost first)
    for (depth, name) in open_stack.iter().rev().enumerate() {
        if seen.contains(name.as_str()) || (!partial.is_empty() && !name.starts_with(&partial)) {
            continue;
        }
        seen.insert(name.clone());
        items.push(CompletionItem {
            label: plugin.format_close_macro_label(name),
            filter_text: Some(name.clone()),
            insert_text: Some(plugin.format_close_macro_label(name)),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("Close {}", plugin.format_macro_label(name))),
            sort_text: Some(format!("0_{:04}_{}", depth, name)),
            ..Default::default()
        });
    }

    // If no unclosed macros found, offer all block macro close tags as fallback
    if items.is_empty() {
        for name in &block_names {
            if !partial.is_empty() && !name.starts_with(&partial) {
                continue;
            }
            items.push(CompletionItem {
                label: plugin.format_close_macro_label(name),
                filter_text: Some(name.to_string()),
                insert_text: Some(plugin.format_close_macro_label(name)),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("Close {}", plugin.format_macro_label(name))),
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
/// Detects format-specific macro contexts using the format plugin instead of
/// hardcoding SugarCube `<<>>` delimiters. Works with SugarCube `<<goto "...`,
/// Harlowe `(goto: "...`, and other format-specific syntax.
fn try_passage_in_quote_completion(
    before_cursor: &str,
    workspace: &knot_core::Workspace,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    // Only proceed if the format has passage-arg macros
    let passage_arg_names = plugin.passage_arg_macro_names();
    if passage_arg_names.is_empty() {
        return None;
    }

    // Find the most recent macro open context by looking for the format's
    // open delimiter pattern. We try each known passage-arg macro name.
    let mut best_match: Option<(&str, usize)> = None;
    for &macro_name in &passage_arg_names {
        // Build the opening pattern for this format
        let open_pattern = plugin.format_macro_label(macro_name);
        // Strip the closing delimiter to get the open prefix
        // e.g., "<<goto>>" -> "<<goto", "(goto:)" -> "(goto:"
        let open_prefix = open_pattern
            .trim_end_matches('>')
            .trim_end_matches(')')
            .trim_end_matches(']')
            .trim_end_matches('}');

        if let Some(pos) = before_cursor.rfind(open_prefix) {
            match best_match {
                None => best_match = Some((macro_name, pos)),
                Some((_, prev_pos)) if pos > prev_pos => best_match = Some((macro_name, pos)),
                _ => {}
            }
        }
    }

    let (macro_name, open_pos) = best_match?;
    let after_open = &before_cursor[open_pos..];

    // Must not contain the closing delimiter for the active format.
    // Instead of checking all possible close delimiters across all formats,
    // use the format plugin's macro label to build the expected close pattern.
    let macro_label = plugin.format_macro_label(macro_name);
    // Derive the close delimiter from the macro label format:
    //   <<name>> → close is >>
    //   (name:)  → close is )
    //   [name]   → close is ]
    //   {{name}} → close is }}
    let close_delim = if macro_label.starts_with("<<") {
        ">>"
    } else if macro_label.starts_with('(') {
        ")"
    } else if macro_label.starts_with('[') {
        "]"
    } else if macro_label.starts_with("{{") {
        "}}"
    } else {
        "" // no known close delimiter
    };
    if !close_delim.is_empty() && after_open.contains(close_delim) { return None; }

    // Count the number of quoted strings so far to determine which arg we're in
    let arg_count = after_open.matches('"').count() / 2;
    let is_in_quote = after_open.matches('"').count() % 2 == 1;

    if !is_in_quote {
        return None; // Cursor isn't inside a quoted string
    }

    // Check if the current arg position is the passage ref position
    let current_arg = arg_count;
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
/// Queries the format plugin's `builtin_globals()` for property lists,
/// falling back to an empty list if no properties are defined.
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

    // Look up the global definition and its properties
    let global_def = plugin.builtin_globals().iter().find(|g| g.name == ident)?;

    let properties = match global_def.properties {
        Some(props) => props,
        None => return None,
    };

    let items: Vec<CompletionItem> = properties
        .iter()
        .map(|prop| global_property_completion(prop))
        .collect();

    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// Create a completion item from a GlobalProperty.
fn global_property_completion(prop: &GlobalProperty) -> CompletionItem {
    CompletionItem {
        label: prop.name.to_string(),
        kind: Some(if prop.is_method {
            CompletionItemKind::METHOD
        } else {
            CompletionItemKind::PROPERTY
        }),
        detail: Some(prop.description.to_string()),
        insert_text: Some(prop.name.to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    }
}

/// Try variable dot-notation completion after a dot.
///
/// Supports deep nesting (e.g., `$player.state.`) and array-index
/// completions (e.g., `$items[0].`). Uses the format plugin's
/// shape-aware property map to distinguish Object from Array variables,
/// offering array methods (`.length`, `.push`) for arrays and
/// child properties for objects.
fn try_variable_dot_completion(
    before_cursor: &str,
    workspace: &knot_core::Workspace,
    plugin: &dyn FormatPlugin,
) -> Option<Vec<CompletionItem>> {
    use knot_formats::types::PropertyKind;

    // Find the text before the dot
    let before_dot = before_cursor.trim_end_matches('.');

    // Extract the variable path: could be "$item" or "$player.state" or "$items[0]"
    // We need to find the full dollar-prefixed path before the final "."
    let var_path = before_dot
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '$' && c != '.' && c != '[' && c != ']')
        .next()?;

    // Must start with a variable sigil
    let sigils: Vec<char> = plugin.variable_sigils().iter().map(|s| s.sigil).collect();
    if sigils.is_empty() || !sigils.iter().any(|s| var_path.starts_with(*s)) {
        return None;
    }

    // Build the shape-aware property map
    let shape_map = plugin.build_shape_aware_property_map(workspace);

    // Look up the variable path in the property map.
    // For "$player.state", we need to walk the map:
    //   "$player" → { children: {"state"}, kind: Object }
    //   "$player.state" → { children: {"stress"}, kind: Object }
    let entry = shape_map.get(var_path)?;

    let mut items: Vec<CompletionItem> = Vec::new();

    match entry.kind {
        PropertyKind::Array => {
            // For arrays, offer array methods/properties instead of element properties.
            // Element properties are accessed via `$items[0].prop`.
            let array_props = [".length", ".push()", ".pop()", ".shift()", ".unshift()", ".includes()", ".indexOf()", ".splice()"];
            for (i, prop) in array_props.iter().enumerate() {
                let method_name = prop.trim_start_matches('.');
                let is_method = prop.ends_with("()");
                items.push(CompletionItem {
                    label: method_name.to_string(),
                    kind: Some(if is_method { CompletionItemKind::METHOD } else { CompletionItemKind::PROPERTY }),
                    detail: Some(format!("Array {} of {}", if is_method { "method" } else { "property" }, var_path)),
                    sort_text: Some(format!("0_{:06}_{}", i, prop)),
                    insert_text: Some(method_name.to_string()),
                    insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }

            // Also offer element properties if element_shape is available
            if let Some(ref element_shape) = entry.element_shape {
                for (i, child) in element_shape.children.iter().enumerate() {
                    items.push(CompletionItem {
                        label: format!("[0].{}", child),
                        kind: Some(CompletionItemKind::PROPERTY),
                        detail: Some(format!("Element property of {}", var_path)),
                        sort_text: Some(format!("1_{:06}_{}", i, child)),
                        insert_text: Some(format!("[0].{}", child)),
                        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                        ..Default::default()
                    });
                }
            }
        }
        PropertyKind::Object | PropertyKind::Unknown => {
            // For objects (and unknowns with children), offer child properties
            for (i, child) in entry.children.iter().enumerate() {
                // Check if this child is itself an object or array (for detail text)
                let child_path = format!("{}.{}", var_path, child);
                let child_kind = shape_map.get(&child_path).map(|e| e.kind.clone()).unwrap_or(PropertyKind::Unknown);
                let detail = match child_kind {
                    PropertyKind::Object => format!("Object property of {}", var_path),
                    PropertyKind::Array => format!("Array property of {}", var_path),
                    PropertyKind::Scalar => format!("Property of {}", var_path),
                    PropertyKind::Unknown => format!("Property of {}", var_path),
                };
                items.push(CompletionItem {
                    label: child.clone(),
                    kind: Some(CompletionItemKind::PROPERTY),
                    detail: Some(detail),
                    sort_text: Some(format!("0_{:06}_{}", i, child)),
                    insert_text: Some(child.clone()),
                    insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                    ..Default::default()
                });
            }
        }
        PropertyKind::Scalar => {
            // Scalars don't have properties — no completions
        }
    }

    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}
