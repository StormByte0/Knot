//! Hover handler: macro, variable, link, passage, and global object hover.
//!
//! Provides rich hover information for format-specific constructs by delegating
//! to the active format plugin:
//! - Macro hover with signature, description, and deprecation warnings
//! - Variable hover (format-specific sigils) with write/read tracking
//! - Link hover with passage info
//! - Passage header hover with metadata
//! - Global object hover (e.g., State, Engine, Story for SugarCube)
//!
//! ## Format Isolation
//!
//! Macro detection is delegated to `FormatPlugin::find_macro_at_position()`,
//! which returns format-agnostic byte ranges. The handler converts these to
//! UTF-16 LSP positions. No hardcoded delimiters (`<<>>`, `(:)`, etc.) appear
//! in this file — all syntax-specific logic lives in the format plugin.

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

    // 1. Try passage header hover FIRST — if the cursor is on a :: header
    //    line, always show passage info. This prevents global object names
    //    (e.g., "Story" in "Story Stylesheet [stylesheet]") from matching
    //    the global hover before the passage hover gets a chance.
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
                    knot_core::passage::SpecialPassageBehavior::ChromeInterceptor => "Chrome Interceptor",
                    knot_core::passage::SpecialPassageBehavior::StructureTemplate => "Structure Template",
                    knot_core::passage::SpecialPassageBehavior::Metadata => "Metadata",
                    knot_core::passage::SpecialPassageBehavior::ScriptInjection => "Script Injection",
                    knot_core::passage::SpecialPassageBehavior::StyleInjection => "Style Injection",
                    knot_core::passage::SpecialPassageBehavior::Custom(s) => s,
                };
                let layer = match &def.layer {
                    knot_core::passage::SpecialPassageLayer::TwineCore => " (Twine Core)",
                    knot_core::passage::SpecialPassageLayer::LegacyCore => " (Legacy Core)",
                    knot_core::passage::SpecialPassageLayer::StoryFormat => "",
                    knot_core::passage::SpecialPassageLayer::UserDefined => " (User Defined)",
                };
                format!("\n**Special passage** — {}{}", behavior, layer)
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

        // Compute an explicit hover range covering the full passage name.
        // This is critical for passage names with spaces (e.g., "My Passage")
        // because VS Code's default word-boundary detection splits on whitespace,
        // causing the hover to appear for only the first word.
        let hover_range = compute_passage_header_range(text, position);

        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text,
            }),
            range: hover_range,
        }));
    }

    // 2. Try macro hover — delegate syntax detection to the format plugin
    if let Some(plugin) = plugin {
        if let Some(hover) = try_macro_hover(line, line_idx, char_pos, plugin) {
            return Ok(Some(hover));
        }
    }

    // 3. Try variable hover — check if cursor is on $variable or _variable
    if let Some(plugin) = plugin {
        if let Some(hover) = try_variable_hover(line, line_idx, char_pos, &inner.workspace, plugin) {
            return Ok(Some(hover));
        }
    }

    // 4. Try global object hover — check if cursor is on a format-specific global
    if let Some(plugin) = plugin {
        if let Some(hover) = try_global_hover(line, line_idx, char_pos, plugin) {
            return Ok(Some(hover));
        }
    }

    // 5. Try link hover — check if cursor is inside [[...]]
    if let Some(hover) = try_link_hover(line, line_idx, char_pos, &inner.workspace) {
        return Ok(Some(hover));
    }

    Ok(None)
}

// ===========================================================================
// Private hover helpers
// ===========================================================================

/// Try to show hover info for a macro at the cursor position.
///
/// Uses `FormatPlugin::find_macro_at_position()` for format-agnostic syntax
/// detection. The plugin returns byte ranges; this function converts them
/// to UTF-16 LSP positions.
fn try_macro_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    // Convert UTF-16 char_pos (from LSP) to byte offset for the plugin
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);

    // Delegate syntax detection to the format plugin
    let macro_info = plugin.find_macro_at_position(line, byte_pos)?;

    if let Some(mdef) = plugin.find_macro(&macro_info.name) {
        // Use the format plugin's label formatting — no hardcoded <<>>
        let mut hover_text = format!(
            "**Macro** `{}`\n\n{}",
            plugin.format_macro_label(mdef.name),
            mdef.description
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

        // Add container constraint info — use format-specific labels
        if let Some(parent) = mdef.container {
            hover_text.push_str(&format!(
                "\nMust be inside `{}`.",
                plugin.format_macro_label(parent)
            ));
        }
        if let Some(parents) = mdef.container_any_of {
            hover_text.push_str(&format!(
                "\nMust be inside one of: {}.",
                parents.iter()
                    .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // Convert byte ranges from the plugin to UTF-16 LSP positions
        let utf16_start = helpers::utf16_len_up_to(line, macro_info.full_range.start);
        let utf16_end = helpers::utf16_len_up_to(line, macro_info.full_range.end);

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
///
/// **Guard**: This function immediately returns `None` when the cursor
/// is on a passage header line (starts with `::`). Passage header hover
/// already handles these lines in step 1 of the hover handler, but if
/// that check fails for any reason (e.g., the passage isn't indexed yet),
/// we must not fall through to a global object hover that would split
/// multi-word passage names like "Story Stylesheet" — where "Story"
/// is both a passage name component AND a SugarCube global object.
fn try_global_hover(
    line: &str,
    line_idx: usize,
    char_pos: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    // Never show global object hover on passage header lines.
    // The passage header hover (step 1) owns these lines. Falling through
    // to global hover would cause split hover behavior for multi-word
    // passage names where a word happens to match a global object
    // (e.g., "Story" in "Story Stylesheet" matches SugarCube's Story API).
    if line.trim_start().starts_with("::") {
        return None;
    }

    // Extract the word at the cursor position.
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

            let utf16_start = helpers::utf16_len_up_to(line, abs_start);
            let utf16_end = helpers::utf16_len_up_to(line, abs_end);
            let utf16_pos = char_pos;

            if utf16_pos >= utf16_start as usize && utf16_pos <= utf16_end as usize {
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

/// Compute the hover range for a passage header at the given position.
///
/// The range covers the full passage name (including spaces) from `::` to
/// the end of the name, so that hovering over `:: My Passage Name` shows
/// the hover popup for the entire name, not just the first word.
///
/// Returns `None` if the position is not on a `::` header line.
fn compute_passage_header_range(text: &str, position: Position) -> Option<Range> {
    let line_text = text.lines().nth(position.line as usize)?;

    if !line_text.starts_with("::") {
        return None;
    }

    // Parse the passage name from the header, accounting for whitespace
    // between `::` and the name.
    let after_colons = &line_text[2..];
    let whitespace_len = after_colons.len() - after_colons.trim_start().len();
    // Trim trailing \r for CRLF robustness — mirrors the format plugins'
    // parse_header_line() CRLF fix.
    let rest = after_colons.trim_start().trim_end_matches('\r');

    // The name extends to the `[` bracket (for tags) or `{` (for JSON metadata)
    // or the end of the line. Strip JSON metadata first (must end with '}'),
    // then tags — matching the format plugins' parse_header_line() order.
    let rest_before_json = if let Some(brace_start) = rest.rfind('{') {
        if rest.ends_with('}') {
            &rest[..brace_start]
        } else {
            rest
        }
    } else {
        rest
    };
    // Use rfind('[') + ends_with(']') to match the lexer's tag detection.
    // This avoids false matches on '[' characters inside passage names.
    let name_end = if let Some(bracket_start) = rest_before_json.rfind('[') {
        if rest_before_json.ends_with(']') {
            bracket_start
        } else {
            rest_before_json.len()
        }
    } else {
        rest_before_json.len()
    };
    let name_text = rest_before_json[..name_end].trim_end();

    // Compute the byte offset where the name starts and ends.
    let name_byte_start = 2 + whitespace_len;
    let name_byte_end = name_byte_start + name_text.len();

    // Convert to UTF-16 for LSP.
    let utf16_start = helpers::utf16_len_up_to(line_text, name_byte_start);
    let utf16_end = helpers::utf16_len_up_to(line_text, name_byte_end);

    Some(Range {
        start: Position {
            line: position.line,
            character: utf16_start,
        },
        end: Position {
            line: position.line,
            character: utf16_end,
        },
    })
}
