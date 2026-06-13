//! Hover handler: macro, variable, link, passage, and global object hover.
//!
//! Provides rich hover information for format-specific constructs by delegating
//! to the active format plugin:
//! - Macro arg ref hover (inner layer: passage-ref args inside macros)
//! - Macro hover with signature, description, and deprecation warnings
//! - Variable hover (format-specific sigils) with write/read tracking
//! - Link hover with passage info
//! - Passage header hover with metadata
//! - Global object hover (e.g., State, Engine, Story for SugarCube)
//!
//! ## Span-Based Resolution
//!
//! All hover types (except global object hover, which has no stored spans)
//! use span data from the workspace index for precise byte-range matching
//! instead of re-scanning the line text. This avoids false negatives from
//! manual char scanning and correctly handles multi-byte characters, arrow/pipe
//! link syntax, and passage names with spaces.
//!
//! ## Layered Hover
//!
//! Macro arg refs enable **layered hover**: when the cursor is on a
//! `PassageRef` arg inside a macro (e.g., `"Shop"` in `<<link "Talk" "Shop">>`),
//! the passage hover for the arg target takes priority over the macro hover.
//! This provides context-appropriate hover at each position within a macro.
//!
//! ## Format Isolation
//!
//! Macro detection is delegated to `FormatPlugin::find_macro_at_position()`,
//! which returns format-agnostic byte ranges. The handler converts these to
//! UTF-16 LSP positions. No hardcoded delimiters (`<<>>`, `(:)`, etc.) appear
//! in this file — all syntax-specific logic lives in the format plugin.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use knot_core::passage::Passage;
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

    // Convert the cursor position to a byte offset for span-based lookups.
    let byte_offset = helpers::position_to_byte_offset(text, position);

    // Resolve the active format plugin for format-aware hover queries.
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // Get the document from the workspace for span-based passage lookups.
    let doc = inner.workspace.get_document(&uri);

    // Find the passage containing the cursor by checking passage.span containment.
    let current_passage = doc.as_ref().and_then(|d| {
        d.passages
            .iter()
            .find(|p| p.contains_abs_offset(byte_offset))
    });

    // 1. Try passage header hover FIRST — if the cursor is on a :: header
    //    line, always show passage info. This prevents global object names
    //    (e.g., "Story" in "Story Stylesheet [stylesheet]") from matching
    //    the global hover before the passage hover gets a chance.
    if let Some(passage) = current_passage {
        if let Some(hover) =
            try_passage_header_hover(text, byte_offset, passage, &inner.workspace)
        {
            return Ok(Some(hover));
        }
    }
    // Fallback: if span-based passage lookup didn't find a passage (e.g., the
    // cursor is on a header line but the passage span hasn't been updated yet,
    // or the format only spans the header line), try the line-based check.
    if current_passage.is_none() {
        if let Some(passage_name) = helpers::find_passage_at_position_span_based(text, &inner.workspace, &uri, position)
            && let Some((_, passage)) = inner.workspace.find_passage(&passage_name)
        {
            if let Some(hover) =
                try_passage_header_hover(text, byte_offset, passage, &inner.workspace)
            {
                return Ok(Some(hover));
            }
        }
    }

    // 2. Try macro arg ref hover (inner layer) — if the cursor is on a
    //    PassageRef arg inside a macro, show passage info for the target.
    //    This takes priority over the outer macro hover (step 5).
    if let Some(hover) = try_macro_arg_ref_hover(text, byte_offset, doc, &inner.workspace) {
        return Ok(Some(hover));
    }

    // 3. Try variable hover — use span data from the workspace index.
    if let Some(plugin) = plugin {
        if let Some(hover) = try_variable_hover(
            text,
            byte_offset,
            doc,
            &inner.workspace,
            plugin,
        ) {
            return Ok(Some(hover));
        }
    }

    // 4. Try link hover — use span data from the workspace index.
    if let Some(hover) = try_link_hover(text, byte_offset, doc, &inner.workspace) {
        return Ok(Some(hover));
    }

    // 5. Try macro hover — span-based, using macro_arg_refs.
    //    This is the outer-layer hover: it fires when the cursor is on the
    //    macro name or inside the macro open tag but not on a PassageRef arg
    //    (which was already handled by step 2).
    if let Some(plugin) = plugin {
        if let Some(hover) = try_macro_hover(text, byte_offset, doc, plugin) {
            return Ok(Some(hover));
        }
    }

    // 6. Try global object hover — check if cursor is on a format-specific global.
    //    No stored span data exists for global object occurrences, so this
    //    uses line-based scanning as a fallback.
    if let Some(plugin) = plugin {
        let line_idx = position.line as usize;
        let char_pos = position.character as usize;
        let line = text.lines().nth(line_idx).unwrap_or("");
        if let Some(hover) = try_global_hover(line, line_idx, char_pos, plugin) {
            return Ok(Some(hover));
        }
    }

    Ok(None)
}

// ===========================================================================
// Private hover helpers
// ===========================================================================

/// Try to show hover info for a passage header when the cursor is on the
/// header line.
///
/// Uses `passage.header_name_span` for the hover range when available
/// (SugarCube), falling back to [`compute_passage_header_range`] for formats
/// that don't populate it.
fn try_passage_header_hover(
    text: &str,
    byte_offset: usize,
    passage: &Passage,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    // Check if the cursor is on the header line of this passage.
    // The header line starts at passage.span.start and ends at the first newline.
    let span_start = passage.abs_offset(passage.span.start).min(text.len());
    let header_end = text[span_start..]
        .find('\n')
        .map(|n| span_start + n)
        .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));

    if byte_offset < span_start || byte_offset > header_end {
        return None;
    }

    let passage_name = &passage.name;
    let links_count = passage.links.len();
    let vars_count = passage.vars.len();
    let tags = if passage.tags.is_empty() {
        "none".to_string()
    } else {
        passage.tags.join(", ")
    };

    let incoming = helpers::count_incoming_links(workspace, passage_name);
    let incoming_sources = helpers::incoming_link_sources(workspace, passage_name);

    // Check for special passage info
    let special_info = if passage.is_special {
        if let Some(ref def) = passage.special_def {
            let behavior = match &def.behavior {
                knot_core::passage::SpecialPassageBehavior::Startup => "Startup",
                knot_core::passage::SpecialPassageBehavior::PassageReady => "PassageReady",
                knot_core::passage::SpecialPassageBehavior::Chrome => "Chrome",
                knot_core::passage::SpecialPassageBehavior::ChromeInterceptor => {
                    "Chrome Interceptor"
                }
                knot_core::passage::SpecialPassageBehavior::StructureTemplate => {
                    "Structure Template"
                }
                knot_core::passage::SpecialPassageBehavior::Metadata => "Metadata",
                knot_core::passage::SpecialPassageBehavior::ScriptInjection => {
                    "Script Injection"
                }
                knot_core::passage::SpecialPassageBehavior::StyleInjection => {
                    "Style Injection"
                }
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
        passage_name, special_info, links_count, vars_count, tags, incoming_detail
    );

    // Compute an explicit hover range covering the full passage name.
    // Prefer the span-based header_name_span when available (avoids re-parsing
    // the header line). Fall back to compute_passage_header_range for formats
    // that don't populate header_name_span.
    let hover_range = if let Some(ref name_span) = passage.header_name_span {
        helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
    } else {
        let position = helpers::byte_offset_to_position(text, byte_offset);
        compute_passage_header_range(text, position)?
    };

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(hover_range),
    })
}

/// Try to show hover info for a passage-ref arg inside a macro (inner layer).
///
/// When the cursor is on a `PassageRef` arg inside a macro (e.g., `"Shop"` in
/// `<<link "Talk" "Shop">>`), this shows passage info for the target. This
/// takes priority over the outer macro hover (step 5) — the inner layer wins.
///
/// Uses `passage.macro_arg_refs[].span` from the workspace index for precise
/// byte-range matching. Each `MacroArgRef` contains only the span of the
/// passage name itself (not the full macro), so hover only triggers when the
/// cursor is actually on the passage reference.
fn try_macro_arg_ref_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    let doc = doc?;

    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                let target = arg_ref.target.trim();
                if target.is_empty() {
                    continue;
                }

                if let Some((target_doc, target_passage)) = workspace.find_passage(target) {
                    let incoming = helpers::count_incoming_links(workspace, target);
                    let mut hover_text = format!(
                        "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                        target,
                        target_doc.uri.as_str(),
                        target_passage.links.len(),
                        incoming,
                        if target_passage.tags.is_empty() {
                            "none".to_string()
                        } else {
                            target_passage.tags.join(", ")
                        }
                    );

                    if !target_passage.vars.is_empty() {
                        let writes: Vec<&str> = target_passage
                            .persistent_variable_inits()
                            .map(|v| v.name.as_str())
                            .collect();
                        let reads: Vec<&str> = target_passage
                            .persistent_variable_reads()
                            .map(|v| v.name.as_str())
                            .collect();
                        if !writes.is_empty() {
                            hover_text.push_str(&format!(
                                "\nVariables written: {}",
                                writes.join(", ")
                            ));
                        }
                        if !reads.is_empty() {
                            hover_text.push_str(&format!(
                                "\nVariables read: {}",
                                reads.join(", ")
                            ));
                        }
                    }

                    // Show which macro this arg belongs to
                    hover_text.push_str(&format!(
                        "\n\n*Referenced by* `<<{}>>`",
                        arg_ref.macro_name
                    ));

                    let hover_range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&arg_ref.span));

                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(hover_range),
                    });
                } else {
                    // Broken ref — passage doesn't exist
                    let hover_range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&arg_ref.span));

                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!(
                                "⚠ **Broken link** — passage `{}` does not exist",
                                target
                            ),
                        }),
                        range: Some(hover_range),
                    });
                }
            }
        }
    }

    None
}

/// Try to show hover info for a macro at the cursor position (outer layer).
///
/// Uses `passage.macro_arg_refs[]` for span-based resolution instead of
/// line-scanning with `find_macro_at_position()`. This works correctly for
/// multi-line macros and provides precise hover ranges.
///
/// The function checks two conditions:
/// 1. **Cursor on macro name**: If the cursor falls within `macro_name_span`,
///    show macro hover with the name as the hover range.
/// 2. **Cursor inside macro open tag**: If the cursor falls within
///    `macro_open_span` but not on the name or a PassageRef arg (which was
///    already handled by `try_macro_arg_ref_hover`), show macro hover as the
///    outer-layer fallback with the open tag as the hover range.
///
/// Macros that have no `macro_arg_refs` entries (i.e., no PassageRef args)
/// still get hover via the `macro_open_span` check — this handles macros
/// like `<<set>>`, `<<if>>`, `<<print>>` that don't contain passage references.
fn try_macro_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let doc = doc?;

    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            let has_body = Some(arg_ref.has_body);

            // Check if cursor is on the macro name
            if passage.span_contains_abs_offset(&arg_ref.macro_name_span, byte_offset) {
                if let Some(mdef) = plugin.find_macro(&arg_ref.macro_name) {
                    let hover_text = build_macro_hover_text(mdef, plugin, has_body);
                    let hover_range = helpers::byte_range_to_lsp_range(
                        text,
                        &passage.abs_range(&arg_ref.macro_name_span),
                    );
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(hover_range),
                    });
                }
            }

            // Check if cursor is inside the macro open tag but not on the name
            // or a PassageRef arg (those are handled by try_macro_arg_ref_hover).
            // This is the outer-layer fallback for Label args, whitespace, etc.
            if passage.span_contains_abs_offset(&arg_ref.macro_open_span, byte_offset) {
                // Don't show macro hover if the cursor is on a PassageRef arg
                // (already handled by step 2) or on the macro name (handled above).
                if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                    continue; // PassageRef arg — skip, already handled
                }
                if passage.span_contains_abs_offset(&arg_ref.macro_name_span, byte_offset) {
                    continue; // Macro name — already handled above
                }

                if let Some(mdef) = plugin.find_macro(&arg_ref.macro_name) {
                    let hover_text = build_macro_hover_text(mdef, plugin, has_body);
                    let hover_range = helpers::byte_range_to_lsp_range(
                        text,
                        &passage.abs_range(&arg_ref.macro_open_span),
                    );
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(hover_range),
                    });
                }
            }
        }
    }

    // Fallback: for macros that have no macro_arg_refs (no PassageRef args),
    // we can't do span-based resolution yet. Fall back to line-scanning
    // via find_macro_at_position(). This handles macros like <<set>>, <<if>>,
    // <<print>> that don't contain passage references.
    //
    // TODO: Once we store macro name spans for ALL macros (not just those
    // with PassageRef args), this fallback can be removed.
    let line_idx = helpers::byte_offset_to_position(text, byte_offset).line as usize;
    let char_pos = helpers::byte_offset_to_position(text, byte_offset).character as usize;
    let line = text.lines().nth(line_idx).unwrap_or("");
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);
    let macro_info = plugin.find_macro_at_position(line, byte_pos)?;

    if let Some(mdef) = plugin.find_macro(&macro_info.name) {
        let hover_text = build_macro_hover_text(mdef, plugin, None);
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

/// Build the hover text for a macro definition.
///
/// Extracted from the old `try_macro_hover` so it can be shared between
/// the span-based path and the line-scanning fallback.
///
/// `has_body` indicates whether this specific invocation has a body (children
/// between open and close tags). Since all SugarCube macros are now classified
/// as either Container (always needs close tag) or Inline (never has close tag),
/// this parameter is currently unused but kept for API compatibility.
/// Pass `None` when body presence is unknown.
fn build_macro_hover_text(
    mdef: &knot_formats::types::MacroDef,
    plugin: &dyn fmt_plugin::FormatPlugin,
    _has_body: Option<bool>,
) -> String {
    let kind = macros::classify(mdef.name, mdef, plugin);

    let mut hover_text = macros::hover_header(kind, &plugin.format_macro_label(mdef.name));

    // Add description
    hover_text.push_str(&format!("\n\n{}", mdef.description));

    // Add deprecation warning
    if mdef.deprecated {
        if let Some(msg) = mdef.deprecation_message {
            hover_text.push_str(&format!("\n\n⚠ **Deprecated**: {}", msg));
        }
    }

    // Add kind-specific note (e.g., "Close with <</if>>")
    if let Some(note) = macros::hover_kind_note(kind, mdef.name, plugin) {
        hover_text.push_str(&format!("\n\n{}", note));
    }

    // Add parameter info
    if let Some(args) = mdef.args {
        if !args.is_empty() {
            hover_text.push_str("\n\n**Parameters:**\n");
            for arg in args {
                let req = if arg.is_required {
                    " (required)"
                } else {
                    " (optional)"
                };
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
            parents
                .iter()
                .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    hover_text
}

/// Try to show hover info for a variable when the cursor is within a
/// variable's span.
///
/// Uses `passage.vars[].span` from the workspace index for precise byte-range
/// matching instead of manually scanning the line text for format-specific
/// sigils (e.g., `$var` or `_var`). This correctly handles multi-byte
/// characters, variables inside macros, and avoids false negatives from
/// manual char scanning.
fn try_variable_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let doc = doc?;

    // Iterate over all passages in the document and check if the cursor
    // byte offset falls within any variable's span. Variable spans are
    // absolute byte offsets in the document text, so we can match directly.
    for passage in &doc.passages {
        for var in &passage.vars {
            if passage.span_contains_abs_offset(&var.span, byte_offset) {
                let var_name = &var.name;

                // Find where this variable is written and read across the workspace
                let mut write_locations: Vec<String> = Vec::new();
                let mut read_count = 0;
                for doc in workspace.documents() {
                    for passage in &doc.passages {
                        for v in &passage.vars {
                            if v.name == *var_name {
                                match v.kind {
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

                // Determine the sigil character from the variable name.
                // The name includes the sigil (e.g., "$gold" or "_temp"),
                // so the first character is the sigil.
                let sigil = var_name.chars().next().unwrap_or('$');
                let sigil_desc = plugin
                    .describe_variable_sigil(sigil)
                    .unwrap_or("variable");

                let var_type = plugin
                    .resolve_variable_sigil(sigil)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "variable".to_string());

                let hover_text = format!(
                    "**{}** `{}`\n\n{}\nRead in {} location(s)\n\n---\n\n{}",
                    var_name, var_type, write_info, read_count, sigil_desc
                );

                // Convert the variable's byte span to an LSP Range.
                let hover_range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&var.span));

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(hover_range),
                });
            }
        }
    }

    None
}

/// Try to show hover info for a global object.
///
/// **Guard**: This function immediately returns `None` when the cursor
/// is on a passage header line (starts with `::`). Passage header hover
/// already handles these lines in step 1 of the hover handler, but if
/// that check fails for any reason (e.g., the passage isn't indexed yet),
/// we must not fall through to a global object hover that would split
/// multi-word passage names like "Story Stylesheet" — where "Story"
/// is both a passage name component AND a SugarCube global object.
///
/// No stored span data exists for global object occurrences, so this
/// function uses line-based scanning as a fallback.
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
            utf16_count += if (*ch as u32) < 0x10000 {
                1usize
            } else {
                2usize
            };
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

/// Try to show hover info for a passage link when the cursor is within a
/// link's span.
///
/// Uses `passage.links[].span` from the workspace index for precise
/// byte-range matching instead of manually scanning the line for `[[`/`]]`
/// patterns. The link's `target` field is used directly, avoiding the need
/// to parse arrow (`->`) or pipe (`|`) syntax.
fn try_link_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    let doc = doc?;

    // Iterate over all passages in the document and check if the cursor
    // byte offset falls within any link's span. Link spans are absolute
    // byte offsets in the document text.
    for passage in &doc.passages {
        for link in &passage.links {
            if passage.span_contains_abs_offset(&link.span, byte_offset) {
                let target = link.target.trim();

                if !target.is_empty() {
                    if let Some((doc, passage)) = workspace.find_passage(target) {
                        let incoming = helpers::count_incoming_links(workspace, target);
                        let mut hover_text = format!(
                            "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                            target,
                            doc.uri.as_str(),
                            passage.links.len(),
                            incoming,
                            if passage.tags.is_empty() {
                                "none".to_string()
                            } else {
                                passage.tags.join(", ")
                            }
                        );

                        if !passage.vars.is_empty() {
                            let writes: Vec<&str> = passage
                                .persistent_variable_inits()
                                .map(|v| v.name.as_str())
                                .collect();
                            let reads: Vec<&str> = passage
                                .persistent_variable_reads()
                                .map(|v| v.name.as_str())
                                .collect();
                            if !writes.is_empty() {
                                hover_text.push_str(&format!(
                                    "\nVariables written: {}",
                                    writes.join(", ")
                                ));
                            }
                            if !reads.is_empty() {
                                hover_text.push_str(&format!(
                                    "\nVariables read: {}",
                                    reads.join(", ")
                                ));
                            }
                        }

                        // Convert the link's byte span to an LSP Range.
                        let hover_range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));

                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    } else {
                        // Broken link — passage doesn't exist
                        let hover_range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));

                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: format!(
                                    "⚠ **Broken link** — passage `{}` does not exist",
                                    target
                                ),
                            }),
                            range: Some(hover_range),
                        });
                    }
                }
            }
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
///
/// This is used as a fallback when `passage.header_name_span` is not
/// available (e.g., for formats other than SugarCube).
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
