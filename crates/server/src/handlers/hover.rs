//! Hover handler: macro, variable, link, passage, and global object hover.
//!
//! Provides rich hover information for format-specific constructs by delegating
//! to the active format plugin:
//! - Macro hover with signature, description, and deprecation warnings
//! - Variable hover (format-specific sigils) with write/read tracking
//! - Link hover with passage info
//! - Passage header hover with metadata
//! - Global object hover (e.g., State, Engine, Story for SugarCube)

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_formats::plugin as fmt_plugin;
use knot_formats::types::MacroArgKind;
use lsp_types::*;

pub(crate) async fn hover(
    state: &ServerState,
    params: HoverParams,
) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;
    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let line_idx = position.line as usize;
    let char_pos = position.character as usize;
    let line = text.lines().nth(line_idx).unwrap_or("");

    // Resolve the active format plugin for format-aware hover queries.
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // 1. Try macro hover — check if cursor is inside <<...>>
    if let Some(plugin) = plugin {
        if let Some(hover) = try_macro_hover(line, line_idx, char_pos, plugin) {
            return Ok(Some(hover));
        }
    }

    // 2. Try variable hover — check if cursor is on $variable or _variable
    if let Some(plugin) = plugin {
        if let Some(hover) = try_variable_hover(line, line_idx, char_pos, &inner.workspace, plugin) {
            return Ok(Some(hover));
        }
    }

    // 3. Try global object hover — check if cursor is on a format-specific global
    if let Some(plugin) = plugin {
        if let Some(hover) = try_global_hover(line, line_idx, char_pos, plugin) {
            return Ok(Some(hover));
        }
    }

    // 4. Try link hover — check if cursor is inside [[...]]
    if let Some(hover) = try_link_hover(line, line_idx, char_pos, &inner.workspace) {
        return Ok(Some(hover));
    }

    // 5. Try passage hover — check if cursor is on a passage header
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

        let incoming = helpers::count_incoming_links(&inner.workspace, &passage_name);
        let incoming_sources = helpers::incoming_link_sources(&inner.workspace, &passage_name);

        // Check for special passage info
        let special_info = if passage.is_special {
            if let Some(ref def) = passage.special_def {
                let behavior = match &def.behavior {
                    knot_core::passage::SpecialPassageBehavior::Startup => "Startup",
                    knot_core::passage::SpecialPassageBehavior::PassageReady => "PassageReady",
                    knot_core::passage::SpecialPassageBehavior::Chrome => "Chrome",
                    knot_core::passage::SpecialPassageBehavior::Metadata => "Metadata",
                    knot_core::passage::SpecialPassageBehavior::Custom(s) => s,
                };
                format!("\n**Special passage** — {}", behavior)
            } else {
                "\n**Special passage**".to_string()
            }
        } else {
            String::new()
        };

        let incoming_detail = if incoming <= 5 && !incoming_sources.is_empty() {
            format!("{} ({})", incoming, incoming_sources.join(", "))
        } else {
            incoming.to_string()
        };

        let hover_text = format!(
            "**{}**{}\n\nLinks: {} | Variables: {} | Tags: {} | Incoming: {}",
            passage.name, special_info, links_count, vars_count, tags, incoming_detail
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

// ===========================================================================
// Private hover helpers
// ===========================================================================

/// Try to show hover info for a macro when cursor is inside `<<...>>`.
fn try_macro_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let mut search_from = 0;
    while let Some(rel_start) = line[search_from..].find("<<") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line[abs_start..].find(">>") {
            let abs_end = abs_start + rel_end + 2;

            // Convert byte-based abs_start/abs_end to UTF-16 code unit offsets
            // for the LSP range.  char_pos arrives as UTF-16 from the client,
            // so we must compare against UTF-16 offsets as well.
            let utf16_start = helpers::utf16_len_up_to(line, abs_start);
            let utf16_end = helpers::utf16_len_up_to(line, abs_end);
            let utf16_pos = char_pos; // already UTF-16 from the client

            if utf16_pos >= utf16_start as usize && utf16_pos <= utf16_end as usize {
                let content = &line[abs_start + 2..abs_end - 2];
                let macro_name = content.split_whitespace().next().unwrap_or(content).trim();

                if let Some(mdef) = plugin.find_macro(macro_name) {
                    let mut hover_text = format!(
                        "**Macro** `<<{}>>`\n\n{}",
                        mdef.name, mdef.description
                    );

                    // Add deprecation warning
                    if mdef.deprecated {
                        if let Some(msg) = mdef.deprecation_message {
                            hover_text.push_str(&format!("\n\n⚠ **Deprecated**: {}", msg));
                        }
                    }

                    // Add parameter info
                    if let Some(args) = mdef.args {
                        if !args.is_empty() {
                            hover_text.push_str("\n\n**Parameters:**\n");
                            for arg in args {
                                let req = if arg.is_required { " (required)" } else { " (optional)" };
                                let kind_str = match arg.kind {
                                    MacroArgKind::Expression => "expression",
                                    MacroArgKind::String => "string",
                                    MacroArgKind::Selector => "selector",
                                    MacroArgKind::Variable => "variable",
                                };
                                let flags = if arg.is_passage_ref { " 🔗" } else { "" };
                                hover_text.push_str(&format!(
                                    "- `{}{}`: {}{}\n",
                                    arg.label, req, kind_str, flags
                                ));
                            }
                        }
                    }

                    // Add container constraint info
                    if let Some(parent) = mdef.container {
                        hover_text.push_str(&format!("\nMust be inside `<<{}>>`.", parent));
                    }
                    if let Some(parents) = mdef.container_any_of {
                        hover_text.push_str(&format!(
                            "\nMust be inside one of: {}.",
                            parents.iter().map(|p| format!("`<<{}>>`", p)).collect::<Vec<_>>().join(", ")
                        ));
                    }

                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(Range {
                            start: Position {
                                line: line_idx as u32,
                                character: utf16_start,
                            },
                            end: Position {
                                line: line_idx as u32,
                                character: utf16_end,
                            },
                        }),
                    });
                }
            }
            search_from = abs_end;
        } else {
            break;
        }
    }
    None
}

/// Try to show hover info for a variable when cursor is on `$var` or `_var`.
fn try_variable_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    workspace: &knot_core::Workspace,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let chars: Vec<char> = line.chars().collect();
    let mut pos = 0;
    while pos < chars.len() {
        // Check for $variable or _variable (but not _ inside identifiers like foo_bar)
        let is_var_start = (chars[pos] == '$' || chars[pos] == '_')
            && pos + 1 < chars.len()
            && (chars[pos + 1].is_alphabetic() || chars[pos + 1] == '_');

        if !is_var_start {
            pos += 1;
            continue;
        }

        // For _ variables, check that the preceding char is not alphanumeric
        // (to avoid matching _bar inside foo_bar)
        if chars[pos] == '_' && pos > 0 && chars[pos - 1].is_alphanumeric() {
            pos += 1;
            continue;
        }

        let sigil = chars[pos];
        let start = pos;
        pos += 1;
        while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
            pos += 1;
        }
        let var_name: String = chars[start..pos].iter().collect();
        let byte_start: usize = chars[..start].iter().map(|c| c.len_utf8()).sum();
        let byte_end: usize = chars[..pos].iter().map(|c| c.len_utf8()).sum();

        // Convert byte positions to UTF-16 code unit offsets for LSP.
        // char_pos arrives as UTF-16 from the client, so we must compare
        // against UTF-16 offsets as well.
        let utf16_start = helpers::utf16_len_up_to(line, byte_start);
        let utf16_end = helpers::utf16_len_up_to(line, byte_end);
        let utf16_pos = char_pos; // already UTF-16 from the client

        if utf16_pos >= utf16_start as usize && utf16_pos <= utf16_end as usize {
            // Find where this variable is written and read across the workspace
            let mut write_locations: Vec<String> = Vec::new();
            let mut read_count = 0;
            for doc in workspace.documents() {
                for passage in &doc.passages {
                    for var in &passage.vars {
                        if var.name == var_name {
                            match var.kind {
                                knot_core::passage::VarKind::Init => {
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

            let sigil_desc = plugin.describe_variable_sigil(sigil)
                .unwrap_or("Unknown variable type");

            let var_type = plugin.resolve_variable_sigil(sigil)
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if sigil == '_' { "temporary".to_string() } else { "story (persistent)".to_string() }
                });

            let hover_text = format!(
                "**{}** `{}`\n\n{}\nRead in {} location(s)\n\n---\n\n{}",
                var_name, var_type, write_info, read_count, sigil_desc
            );

            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover_text,
                }),
                range: Some(Range {
                    start: Position {
                        line: line_idx as u32,
                        character: utf16_start,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: utf16_end,
                    },
                }),
            });
        }
    }
    None
}

/// Try to show hover info for a format-specific global object.
fn try_global_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    // Extract the word at the cursor position.
    // char_pos is in UTF-16 code units (from LSP). Convert to a
    // Unicode-scalar-value index so we can index into `chars[]`.
    let chars: Vec<char> = line.chars().collect();
    let utf16_to_char_idx = |utf16_offset: usize| -> usize {
        let mut utf16_count = 0usize;
        for (i, ch) in chars.iter().enumerate() {
            if utf16_count >= utf16_offset {
                return i;
            }
            utf16_count += if (*ch as u32) < 0x10000 { 1usize } else { 2usize };
        }
        chars.len()
    };

    let char_idx = utf16_to_char_idx(char_pos);
    if char_idx == 0 || char_idx > chars.len() {
        return None;
    }

    // Find the start of the identifier at the cursor
    let mut end = char_idx;
    let mut start = char_idx;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }

    if start == end {
        return None;
    }

    let word: String = chars[start..end].iter().collect();

    // Gate on known global object names for the active format
    if !plugin.global_object_names().contains(word.as_str()) {
        return None;
    }

    if let Some(hover_text) = plugin.global_hover_text(&word) {
        let byte_start: usize = chars[..start].iter().map(|c| c.len_utf8()).sum();
        let byte_end: usize = chars[..end].iter().map(|c| c.len_utf8()).sum();

        // Convert byte positions to UTF-16 code unit offsets for LSP
        let utf16_start = helpers::utf16_len_up_to(line, byte_start);
        let utf16_end = helpers::utf16_len_up_to(line, byte_end);

        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text.to_string(),
            }),
            range: Some(Range {
                start: Position {
                    line: line_idx as u32,
                    character: utf16_start,
                },
                end: Position {
                    line: line_idx as u32,
                    character: utf16_end,
                },
            }),
        });
    }

    None
}

/// Try to show hover info for a passage link when cursor is inside [[...]].
///
/// All byte offsets from string slicing are converted to UTF-16 code unit
/// offsets for LSP positions, as required by the specification.
fn try_link_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    let mut search_from = 0;
    while let Some(rel_start) = line[search_from..].find("[[") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line[abs_start..].find("]]") {
            let abs_end = abs_start + rel_end + 2;
            let content_start = abs_start + 2;
            let content_end = abs_start + rel_end;

            // Convert byte offsets to UTF-16 code unit offsets for LSP
            let utf16_start = helpers::utf16_len_up_to(line, abs_start);
            let utf16_end = helpers::utf16_len_up_to(line, abs_end);
            let utf16_pos = char_pos; // already UTF-16 from the client

            if utf16_pos >= utf16_start as usize && utf16_pos <= utf16_end as usize {
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
                    if let Some((doc, passage)) = workspace.find_passage(target) {
                        let incoming = helpers::count_incoming_links(workspace, target);
                        let mut hover_text = format!(
                            "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                            target,
                            doc.uri.as_str(),
                            passage.links.len(),
                            incoming,
                            if passage.tags.is_empty() { "none".to_string() } else { passage.tags.join(", ") }
                        );

                        // Add variable info for the target passage
                        if !passage.vars.is_empty() {
                            let writes: Vec<&str> = passage.persistent_variable_inits().map(|v| v.name.as_str()).collect();
                            let reads: Vec<&str> = passage.persistent_variable_reads().map(|v| v.name.as_str()).collect();
                            if !writes.is_empty() {
                                hover_text.push_str(&format!("\nVariables written: {}", writes.join(", ")));
                            }
                            if !reads.is_empty() {
                                hover_text.push_str(&format!("\nVariables read: {}", reads.join(", ")));
                            }
                        }

                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(Range {
                                start: Position {
                                    line: line_idx as u32,
                                    character: utf16_start,
                                },
                                end: Position {
                                    line: line_idx as u32,
                                    character: utf16_end,
                                },
                            }),
                        });
                    } else {
                        // Broken link
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: format!("⚠ **Broken link** — passage `{}` does not exist", target),
                            }),
                            range: Some(Range {
                                start: Position {
                                    line: line_idx as u32,
                                    character: utf16_start,
                                },
                                end: Position {
                                    line: line_idx as u32,
                                    character: utf16_end,
                                },
                            }),
                        });
                    }
                }
            }
            search_from = abs_end;
        } else {
            break;
        }
    }
    None
}
