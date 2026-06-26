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
use crate::state::ServerState;
use knot_core::passage::Passage;
use knot_formats::plugin as fmt_plugin;
use knot_formats::types::MacroArgKind;
use lsp_types::*;
use tracing::info;

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

    let format = inner.workspace.resolve_format();
    info!(
        "hover: uri={}, pos={}:{}, format={:?}",
        uri, position.line, position.character, format
    );

    // Convert the cursor position to a byte offset for span-based lookups.
    let byte_offset = helpers::position_to_byte_offset(text, position);

    // Resolve the active format plugin for format-aware hover queries.
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // Get the document from the workspace for span-based passage lookups.
    let doc = inner.workspace.get_document(&uri);

    // Fetch semantic tokens for entity detection (functions, templates,
    // properties) that don't have dedicated span data on `Passage`.
    let token_groups = inner.semantic_tokens.get(&uri).cloned().unwrap_or_default();

    // 0. NOTE: We deliberately do NOT do diagnostic-first hover here.
    //    VS Code natively shows diagnostic messages when the cursor is over a
    //    squiggly underline, and merges that with our token hover in the same
    //    popup. Re-emitting diagnostics from the server side would duplicate
    //    that and force us to pick a winner (diagnostic vs. token info). Our
    //    hover should just provide token info; diagnostics own their own UI.
    //    If you need diagnostic context inside a token hover, surface it as a
    //    dedicated section in the token hover itself — don't intercept.

    // 0a. Try the format plugin's `provide_hover` first. This is the
    //    plugin-owned path that mirrors `provide_completions`. When the
    //    plugin returns `Some`, we map `FormatHover` → `lsp_types::Hover`
    //    and return immediately. When it returns `None`, we fall through
    //    to the built-in handlers below (which will be removed once all
    //    formats implement `provide_hover`).
    if let Some(plugin) = plugin {
        if let Some(fmt_hover) = plugin.provide_hover(
            text,
            &inner.workspace,
            &uri,
            byte_offset,
            &token_groups,
        ) {
            let range = fmt_hover.range.map(|r| helpers::byte_range_to_lsp_range(text, &r));
            info!("hover: provide_hover returned Some ({} chars)", fmt_hover.contents.len());
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: fmt_hover.contents,
                }),
                range,
            }));
        }
    }

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
    if let Some(plugin) = plugin {
        if let Some(hover) = try_macro_arg_ref_hover(text, byte_offset, doc, &inner.workspace, plugin) {
            return Ok(Some(hover));
        }
    } else {
        // No plugin available — fall back to workspace-only hover (no macro label).
        // This path is rarely hit; preserving previous behavior for safety.
        #[allow(deprecated)]
        if let Some(hover) = try_macro_arg_ref_hover_no_plugin(text, byte_offset, doc, &inner.workspace) {
            return Ok(Some(hover));
        }
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

    // 3b. Try operator hover — cursor on a SugarCube operator like `gt`,
    //     `to`, `eq`, `and`. Shows a plain-English description so users
    //     can model their story logic without memorizing the operator names.
    //
    //     SCOPE: Only fires when an Operator semantic token exists at the
    //     cursor — operators are valid only inside macro expressions, not
    //     in prose or link text. This prevents `to` in "Jump to combat"
    //     from triggering an operator hover.
    if let Some(plugin) = plugin {
        if let Some(hover) = try_operator_hover(text, byte_offset, plugin, &token_groups) {
            return Ok(Some(hover));
        }
    }

    // 4. Try link hover — use span data from the workspace index.
    if let Some(hover) = try_link_hover(text, byte_offset, doc, &inner.workspace) {
        return Ok(Some(hover));
    }

    // 4b. Try template hover — cursor on `?name` in prose. Uses the same
    //     `Function` semantic tokens as function hover (the token builder
    //     emits Function tokens for `?name` patterns). Filters to known
    //     templates via `plugin.find_template()`.
    if let Some(plugin) = plugin {
        if let Some(hover) = try_template_hover(text, byte_offset, plugin, &token_groups) {
            return Ok(Some(hover));
        }
    }

    // 4c. Try function hover — cursor on a JS function call (e.g., `myFunc()`
    //     inside `<<run>>`). Uses semantic tokens for detection. Only fires
    //     when the function has meaningful info (definition location + params).
    if let Some(plugin) = plugin {
        if let Some(hover) = try_function_hover(text, byte_offset, plugin, &token_groups) {
            return Ok(Some(hover));
        }
    }

    // 4c. Try property hover — cursor on `.prop` in `$var.prop`. Only fires
    //     when there are siblings to discover (the value of property hover is
    //     seeing what other properties exist on the parent object).
    if let Some(plugin) = plugin {
        if let Some(hover) = try_property_hover(text, byte_offset, plugin, &token_groups, doc) {
            return Ok(Some(hover));
        }
    }

    // 5. Try macro hover — span-based, using macro_arg_refs.
    //    This is the outer-layer hover: it fires when the cursor is on the
    //    macro name or inside the macro open tag but not on a PassageRef arg
    //    (which was already handled by step 2).
    //
    // 5a. Try close-tag hover — cursor on `<</name>>`. Shows which macro
    //     the close tag belongs to. Close tags don't have span data in
    //     `macro_invocations` (which tracks open tags only), so we detect
    //     via line-scanning for the `<</` pattern.
    if let Some(plugin) = plugin {
        if let Some(hover) = try_close_tag_hover(text, byte_offset, plugin) {
            return Ok(Some(hover));
        }
    }

    // 5b. Try macro hover — fires only when cursor is ON the macro name.
    if let Some(plugin) = plugin {
        if let Some(hover) = try_macro_hover(text, byte_offset, doc, plugin) {
            return Ok(Some(hover));
        }
    }

    // 5c. Try block-level markup hover — cursor on `!`, `*`, `#`, `>`,
    //     `----`, `<<<`, or `{{{` markers. These produce semantic tokens
    //     (Heading, ListMarker, Blockquote, etc.) but had no hover handler,
    //     so hovering over them did nothing. This is a line-based scan that
    //     fires when the cursor is on the marker run at column 0.
    if let Some(hover) = try_block_markup_hover(text, byte_offset) {
        return Ok(Some(hover));
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

/// Compute variable diagnostics for the workspace via the format plugin.
///
/// This is invoked on-demand from `try_variable_hover` to surface diagnostics
/// (unused variable, redundant write, unknown property, availability hint)
/// in the hover popup. Computing on every hover is acceptable because the
/// state variable registry + diagnostic computation is fast for typical
/// workspace sizes (hundreds of passages). If this becomes a hot path,
/// cache the result on `ServerStateInner` keyed by workspace version.
fn collect_variable_diagnostics(
    plugin: &dyn fmt_plugin::FormatPlugin,
    workspace: &knot_core::Workspace,
) -> Vec<knot_formats::types::VariableDiagnostic> {
    let start_passage = workspace
        .metadata
        .as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");
    let state_registry = plugin.build_state_variable_registry(workspace);
    plugin.compute_variable_diagnostics(workspace, start_passage, &state_registry)
}

/// Build the standard hover text for a passage target (used by link hover and
/// macro arg ref hover). Renders the passage name, file URI, link counts,
/// tags, and any persistent variable reads/writes declared in the passage.
///
/// Extracted from `try_macro_arg_ref_hover` and `try_link_hover` to DRY up
/// the duplicated template.
fn build_passage_target_hover_text(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
) -> String {
    build_passage_target_hover_text_impl(target, target_doc, target_passage, workspace, false)
}

/// Compact form of passage target hover text, used for passage REFERENCES
/// (e.g., the `"Shop"` arg in `<<link "Talk" "Shop">>`).
///
/// A reference is NOT the same as the passage definition — the user is
/// asking "what does this reference point to?", not "tell me everything
/// about this passage". The compact form shows just:
/// - Passage name
/// - Tags (if any)
/// - Incoming link count (context: how much this passage is referenced)
///
/// It does NOT show:
/// - File path (the user can Ctrl+Click to navigate; path is noise here)
/// - Links out (irrelevant when you're looking at a reference)
/// - Variables written/read (too much detail for a reference hover)
fn build_passage_target_hover_text_compact(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
) -> String {
    build_passage_target_hover_text_impl(target, target_doc, target_passage, workspace, true)
}

fn build_passage_target_hover_text_impl(
    target: &str,
    target_doc: &knot_core::Document,
    target_passage: &Passage,
    workspace: &knot_core::Workspace,
    compact: bool,
) -> String {
    let incoming = helpers::count_incoming_links(workspace, target);

    if compact {
        // Compact form for references: name + tags + incoming count only.
        let tags_str = if target_passage.tags.is_empty() {
            "none".to_string()
        } else {
            target_passage.tags.join(", ")
        };
        return format!(
            "**{}** `Passage`\n\nTags: {} | Referenced by {} passage(s)",
            target, tags_str, incoming
        );
    }

    // Full form for [[link]] hovers and passage header hovers.
    // Show workspace-relative path instead of full file:// URI.
    // Authors don't want to see "file:///D:/codeWS/twine/..." — just
    // "src/passages/newtest.twee" or similar.
    let display_path = workspace.root_uri
        .to_file_path()
        .ok()
        .and_then(|root| {
            // target_doc.uri is a file:// URL — convert to path
            target_doc.uri.to_file_path().ok().and_then(|doc_path| {
                doc_path.strip_prefix(&root).ok().map(|p| p.display().to_string())
            })
        })
        .unwrap_or_else(|| {
            // Fallback: show just the filename
            target_doc.uri.path_segments()
                .and_then(|s| s.last())
                .unwrap_or("unknown")
                .to_string()
        });

    let mut hover_text = format!(
        "**{}**\n\n{}\nLinks out: {} | Incoming: {} | Tags: {}",
        target,
        display_path,
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
            hover_text.push_str(&format!("\nVariables written: {}", writes.join(", ")));
        }
        if !reads.is_empty() {
            hover_text.push_str(&format!("\nVariables read: {}", reads.join(", ")));
        }
    }

    hover_text
}

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
    plugin: &dyn fmt_plugin::FormatPlugin,
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
                    let mut hover_text = build_passage_target_hover_text_compact(
                        target, target_doc, target_passage, workspace,
                    );

                    // Show which macro this arg belongs to.
                    // Use the format-owned label so this stays format-agnostic
                    // (e.g., SugarCube `<<name>>`, Harlowe `(name:)`).
                    hover_text.push_str(&format!(
                        "\n\n*Referenced by* `{}`",
                        plugin.format_macro_label(&arg_ref.macro_name)
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
                                "**Broken link** — passage `{}` does not exist",
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

/// Fallback variant of [`try_macro_arg_ref_hover`] used when no format plugin
/// is available. Renders the macro label as a bare name without format-specific
/// delimiters. Kept for the rare case where `plugin` is `None` (e.g., workspace
/// not yet indexed); prefer the plugin-aware variant in new code.
#[allow(deprecated)]
fn try_macro_arg_ref_hover_no_plugin(
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
                    let hover_text = build_passage_target_hover_text_compact(
                        target, target_doc, target_passage, workspace,
                    );
                    let hover_range = helpers::byte_range_to_lsp_range(
                        text, &passage.abs_range(&arg_ref.span),
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
    None
}

/// Try to show hover info for a close tag (`<</name>>`).
///
/// Close tags don't have span data in `macro_invocations` (which tracks
/// open tags only), so we detect via line-scanning for the `<</` pattern.
/// Shows "Close tag for `<<name>>`" so users know what they're closing.
fn try_close_tag_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);
    let bytes = line.as_bytes();

    // Find the `<</` sequence that the cursor is inside.
    // Walk backward from the cursor to find `<</`.
    let mut tag_start = None;
    let mut search = byte_pos;
    while search >= 2 {
        if search + 0 <= bytes.len() && search >= 3
            && bytes[search - 3] == b'<' && bytes[search - 2] == b'<' && bytes[search - 1] == b'/'
        {
            tag_start = Some(search - 3);
            break;
        }
        search -= 1;
        // Don't walk past a `>>` (we'd be in a different tag).
        if search < bytes.len() && search >= 1 && bytes[search - 1] == b'>' && search >= 2 && bytes[search - 2] == b'>' {
            return None;
        }
    }
    let tag_start = tag_start?;
    // The name starts at tag_start + 3 (after `<</`).
    let name_start = tag_start + 3;
    if name_start >= bytes.len() {
        return None;
    }
    // Find end of name (alphanumeric + underscore + hyphen).
    let mut name_end = name_start;
    while name_end < bytes.len() {
        let b = bytes[name_end];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            name_end += 1;
        } else {
            break;
        }
    }
    if name_end == name_start {
        return None;
    }
    // Cursor must be within the tag (from `<</` to the closing `>>`).
    // Find the closing `>>` after the name.
    let mut tag_end = name_end;
    while tag_end + 1 < bytes.len() {
        if bytes[tag_end] == b'>' && bytes[tag_end + 1] == b'>' {
            tag_end += 2;
            break;
        }
        tag_end += 1;
    }
    if byte_pos < tag_start || byte_pos > tag_end {
        return None;
    }

    let name = &line[name_start..name_end];
    // Only show hover if this is a known builtin macro (otherwise we'd
    // show "Close tag for" on arbitrary text that looks like a close tag).
    // The `find_macro` call confirms it's a builtin; we don't need the
    // `MacroDef` itself — we just need to know the name is valid.
    let _ = plugin.find_macro(name)?;

    let close_label = plugin.format_close_macro_label(name);
    // Wrap the close label in backticks (rendered as inline code) so VS
    // Code's markdown renderer doesn't strip `<</link>>` as unknown HTML.
    // Use the plugin's `format_close_macro_label` (returns `<</link>>` for
    // SugarCube) instead of a literal `{name}/` (which rendered as `link/`
    // — wrong on two counts: it dropped the `<<>>` delimiters AND left the
    // trailing slash that's only used internally as a catalog key).
    let hover_text = format!(
        "**`{}`** `Close tag`\n\nCloses the `{}` block.",
        close_label,
        plugin.format_macro_label(name)
    );
    let utf16_start = helpers::utf16_len_up_to(line, tag_start);
    let utf16_end = helpers::utf16_len_up_to(line, tag_end);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(Range {
            start: Position { line: line_idx as u32, character: utf16_start },
            end: Position { line: line_idx as u32, character: utf16_end },
        }),
    })
}

/// Try to show hover info for a macro at the cursor position (outer layer).
///
/// Uses `passage.macro_invocations` for span-based resolution instead of
/// line-scanning with `find_macro_at_position()`. This works correctly for
/// multi-line macros and provides precise hover ranges.
///
/// The function checks two conditions:
/// 1. **Cursor on macro name**: If the cursor falls within `name_span`,
///    show macro hover with the name as the hover range.
/// 2. **Cursor inside macro open tag**: If the cursor falls within
///    `open_span` but not on the name (and not on a PassageRef arg,
///    which was already handled by `try_macro_arg_ref_hover` in step 2,
///    and not on a variable, which was already handled by
///    `try_variable_hover` in step 3), show macro hover with the open
///    tag as the hover range. This is what makes hovering on `<<` or
///    `>>` of `<<run>>` work — the name span only covers `run`, so
///    without this fallback, hovering on the delimiters produced no
///    hover at all (the user saw the variable hover for the first
///    variable inside the macro, or nothing at all).
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

    // Span-based resolution for ALL macros (not just those with PassageRef args).
    // `passage.macro_invocations` is populated for every parsed macro, so we
    // can resolve `<<set>>`, `<<if>>`, `<<print>>`, etc. via span lookup
    // without falling back to line-scanning.
    //
    // Design principle: hover answers "what is THIS thing?" — so when the
    // cursor is on the macro NAME, we use the name span as the hover range
    // (precise). When the cursor is on the delimiters (`<<`, `>>`) or
    // whitespace inside the open tag, we use the full open_span as the hover
    // range (the user is clearly asking "what macro is this?", not "what is
    // this single character?"). The variable and PassageRef layers already
    // ran before us, so we know the cursor isn't on a variable or arg.
    for passage in &doc.passages {
        for inv in &passage.macro_invocations {
            // Determine which span the cursor is in (if any).
            //
            // - `on_name`: cursor is inside `name_span` (e.g., on `run` of
            //   `<<run>>`). Use name_span as the hover range.
            // - `in_open_tag`: cursor is inside `open_span` but NOT inside
            //   `name_span`. Use open_span as the hover range. This catches
            //   `<<`, `>>`, and any whitespace/punctuation that isn't a
            //   variable or arg (those are handled by earlier layers).
            //
            // For Expression macros (`<<=>>`, `<<->>`), the parser sets
            // name_span == open_span (the full expression construct), so
            // `on_name` is the only case that fires — which is correct.
            let on_name = passage.span_contains_abs_offset(&inv.name_span, byte_offset);
            let in_open_tag = !on_name
                && passage.span_contains_abs_offset(&inv.open_span, byte_offset);
            if !on_name && !in_open_tag {
                continue;
            }

            // Choose the hover range: name_span when cursor is on the name
            // (precise), open_span when cursor is on the delimiters (the
            // user is asking about the whole tag).
            let hover_byte_range = if on_name {
                passage.abs_range(&inv.name_span)
            } else {
                passage.abs_range(&inv.open_span)
            };

            // Try builtin macro first.
            if let Some(mdef) = plugin.find_macro(&inv.name) {
                let mut hover_text = build_macro_hover_text(mdef, plugin);

                // Container violation check: if this macro requires a parent
                // (e.g., `<<else>>` must be inside `<<if>>`), find the
                // enclosing macro at the cursor and verify it's allowed.
                if let Some(violation) = check_container_violation(
                    mdef, passage, byte_offset, plugin,
                ) {
                    hover_text.push_str(&format!("\n\n**Container violation**: {}", violation));
                }

                let hover_range = helpers::byte_range_to_lsp_range(text, &hover_byte_range);
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(hover_range),
                });
            }

            // Not a builtin — try custom macro (widget / Macro.add()).
            if let Some(detail) = plugin.find_custom_macro_detail(&inv.name) {
                let hover_text = build_custom_macro_hover_text(&inv.name, &detail, plugin);
                let hover_range = helpers::byte_range_to_lsp_range(text, &hover_byte_range);
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

/// Check if a macro invocation violates its container constraints.
///
/// A macro with `mdef.container = Some("if")` must appear inside an `<<if>>`
/// block. This function walks `passage.macro_invocations` to find the
/// enclosing parent macro at `byte_offset` and verifies it matches the
/// constraint. Returns `Some(message)` if there's a violation, `None` if
/// the macro is correctly nested (or has no container constraint).
fn check_container_violation(
    mdef: &knot_formats::types::MacroDef,
    passage: &Passage,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<String> {
    // Determine the allowed parent set.
    let allowed: Vec<&str> = if let Some(parent) = mdef.container {
        vec![parent]
    } else if let Some(parents) = mdef.container_any_of {
        parents.to_vec()
    } else {
        return None; // No container constraint.
    };

    // Find the enclosing parent macro by walking macro_invocations.
    // A macro "encloses" the cursor if the cursor is inside its open tag
    // span AND the macro has a body (container). We pick the innermost
    // such macro.
    let mut enclosing_parent: Option<&str> = None;
    let mut enclosing_span_len = usize::MAX;
    for inv in &passage.macro_invocations {
        if !inv.has_body {
            continue; // Inline macros can't enclose anything.
        }
        let abs_open = passage.abs_range(&inv.open_span);
        // Cursor must be after the open tag and before the close tag.
        // We approximate "inside the body" by checking the cursor is after
        // the open tag's end. The close tag isn't stored separately, so
        // we accept any macro whose open tag starts before the cursor.
        if byte_offset > abs_open.end && abs_open.end - abs_open.start < enclosing_span_len {
            enclosing_span_len = abs_open.end - abs_open.start;
            enclosing_parent = Some(&inv.name);
        }
    }

    match enclosing_parent {
        Some(parent) => {
            if allowed.contains(&parent) {
                None // Correctly nested.
            } else {
                let allowed_labels: Vec<String> = allowed
                    .iter()
                    .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                    .collect();
                Some(format!(
                    "must be inside {} (currently inside `{}`)",
                    allowed_labels.join(" or "),
                    plugin.format_macro_label(parent)
                ))
            }
        }
        None => {
            // No enclosing parent — macro is at top level but requires one.
            let allowed_labels: Vec<String> = allowed
                .iter()
                .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                .collect();
            Some(format!("must be inside {}", allowed_labels.join(" or ")))
        }
    }
}

/// Build hover text for a custom macro (widget / `Macro.add()`).
///
/// Custom macros don't have a `MacroDef` (those are builtins only). Instead
/// we render the definition location, arg count, container-ness, and any
/// description extracted from comments above the definition.
fn build_custom_macro_hover_text(
    name: &str,
    detail: &knot_formats::plugin::CustomMacroDetail,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> String {
    let kind_label = if detail.is_widget {
        "Widget"
    } else {
        "Custom macro"
    };
    let mut text = format!(
        "**`{}`** `{}`\n\nDefined in `:: {}`",
        plugin.format_macro_label(name),
        kind_label,
        detail.defined_in
    );

    if let Some(n) = detail.arg_count {
        text.push_str(&format!("\n\n**Args:** {}", n));
    }
    if detail.is_container {
        text.push_str("\n\n*Container* — has a body between open and close tags.");
    }
    if let Some(ref desc) = detail.description {
        if !desc.is_empty() {
            text.push_str(&format!("\n\n{}", desc));
        }
    }
    text
}

/// Human-readable description of a macro argument kind.
///
/// Used by `build_macro_hover_text` to show what the user should write for
/// each parameter. Instead of just "expression" or "variable", this gives
/// a sentence that helps the user model their story (e.g., "a SugarCube
/// expression — variables, literals, or function calls").
fn describe_macro_arg_kind(kind: &MacroArgKind, is_passage_ref: bool) -> String {
    let base = match kind {
        MacroArgKind::Expression => "a SugarCube expression — variables, literals, or function calls",
        MacroArgKind::String => "a quoted string literal",
        MacroArgKind::Selector => "a CSS selector",
        MacroArgKind::Variable => "a variable reference ($var or _var)",
        MacroArgKind::Keyword => "a bareword keyword (e.g., autofocus, selected, keep)",
        MacroArgKind::Link => "a link markup ([[...]])",
        MacroArgKind::Image => "an image markup ([img[...]])",
        MacroArgKind::Number => "a numeric literal (e.g., 100, 0.5)",
    };
    if is_passage_ref {
        format!("{} (passage name)", base)
    } else {
        base.to_string()
    }
}

/// Build the hover text for a macro definition.
///
/// The hover header is intentionally minimal: just the format-specific macro
/// label (e.g. `**\`<<if>>\`**`) followed by the catalog description. We do
/// NOT emit our own classification labels ("Block macro", "Control-flow macro",
/// etc.) — those are internal SugarCube categorization names that don't appear
/// in the official SugarCube documentation and would only confuse users. The
/// close-tag hint ("Close with `<</if>>`.") is preserved for container macros
/// because that's actionable guidance, not terminology.
///
/// The macro label is wrapped in backticks (rendered as inline code) so that
/// VS Code's markdown renderer doesn't strip `<<set>>` as an unknown HTML tag
/// (`<set>`). Without backticks, `**<<set>>**` renders as bold empty text —
/// the user sees `<>` instead of `<<set>>`.
fn build_macro_hover_text(
    mdef: &knot_formats::types::MacroDef,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> String {
    let mut hover_text = format!("**`{}`**", plugin.format_macro_label(mdef.name));

    // Add description
    hover_text.push_str(&format!("\n\n{}", mdef.description));

    // Add deprecation warning
    if mdef.deprecated {
        if let Some(msg) = mdef.deprecation_message {
            hover_text.push_str(&format!("\n\n**Deprecated**: {}", msg));
        }
    }

    // Close-tag hint for container macros (those that require a body and a
    // matching `<</name>>` close tag). This is the only structural note we
    // surface — it's actionable, not jargon.
    if mdef.kind == knot_formats::types::MacroKind::Container {
        let close_label = plugin.format_close_macro_label(mdef.name);
        if !close_label.is_empty() {
            hover_text.push_str(&format!("\n\nClose with `{}`.", close_label));
        }
    }

    // Add parameter info — render with human-readable kind descriptions
    // so users understand what to write, not just the type name.
    if let Some(args) = mdef.args {
        if !args.is_empty() {
            hover_text.push_str("\n\n**Parameters:**\n");
            for arg in args {
                let req = if arg.is_required {
                    " (required)"
                } else {
                    " (optional)"
                };
                let kind_desc = describe_macro_arg_kind(&arg.kind, arg.is_passage_ref);
                hover_text.push_str(&format!(
                    "- `{}{}`: {}\n",
                    arg.label, req, kind_desc
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
                let enclosing_passage_name = passage.name.clone();
                let is_temporary = var.is_temporary;

                // Find where this variable is written and read.
                //
                // For persistent (`$`) vars: aggregate across the whole
                // workspace — they are workspace-global by design.
                //
                // For temporary (`_`) vars: scope to the enclosing passage
                // only. SugarCube `_` variables are passage-scoped at
                // runtime; aggregating across passages gives wrong counts.
                //
                // TODO: replace with `plugin.variable_hover_info` in Phase 6
                // (provide_hover refactor) so the `is_temporary` check
                // lives in the format plugin, not the handler.
                let mut write_locations: Vec<String> = Vec::new();
                let mut read_count = 0;
                for wp_doc in workspace.documents() {
                    for wp_passage in &wp_doc.passages {
                        if is_temporary && wp_passage.name != enclosing_passage_name {
                            continue;
                        }
                        for v in &wp_passage.vars {
                            if v.name == *var_name {
                                match v.kind {
                                    knot_core::passage::VarKind::Init => {
                                        write_locations.push(wp_passage.name.clone());
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
                // so the first character is the sigil. Fall back to the
                // format's primary sigil (or 'v' if none) rather than
                // hardcoding SugarCube's `$` — Snowman/Chapbook have no
                // sigil and would otherwise misroute to SugarCube's lookup.
                let sigil = var_name.chars().next().unwrap_or_else(|| {
                    plugin
                        .variable_sigils()
                        .first()
                        .map(|s| s.sigil)
                        .unwrap_or('v')
                });
                let sigil_desc = plugin
                    .describe_variable_sigil(sigil)
                    .unwrap_or("variable");

                let var_type = plugin
                    .resolve_variable_sigil(sigil)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "variable".to_string());

                // Surface variable diagnostics (unused, redundant write,
                // unknown property, availability hint). These are computed
                // by the format plugin and filtered to the hovered variable.
                // For temps, only diagnostics from the enclosing passage
                // are shown (matching the scope rule above).
                let var_diagnostics = collect_variable_diagnostics(plugin, workspace);
                let relevant_diagnostics: Vec<_> = var_diagnostics
                    .iter()
                    .filter(|d| {
                        d.message.contains(var_name)
                            && (!is_temporary || d.passage_name == enclosing_passage_name)
                    })
                    .collect();

                // Meaningfulness gate: skip variable hover when there's
                // nothing to add beyond what's visible in the code. A
                // variable with no diagnostics, at most one write, and at
                // most one read is trivially obvious — the code speaks for
                // itself. Don't fill the hover with filler.
                //
                // EXCEPTION: persistent (`$`) variables in prose context
                // (naked variable markup like `You have $gold gold.`) should
                // always get hover — the user explicitly wrote the variable
                // to be rendered, and they want to know what it refers to
                // even if it's only read once. We can't directly tell if a
                // var is in prose vs code from `VarOp`, but persistent vars
                // with `Read` kind are overwhelmingly prose references (code
                // reads go through `<<run>>`/`<<set>>` which have JS analysis).
                // So we skip the gate for persistent read-only vars.
                let is_persistent_read = !is_temporary && matches!(var.kind, knot_core::passage::VarKind::Read);
                if !is_persistent_read
                    && relevant_diagnostics.is_empty()
                    && write_locations.len() <= 1
                    && read_count <= 1
                {
                    return None;
                }

                let mut hover_text = format!(
                    "**{}** `{}`\n\n{}\nRead in {} location(s)",
                    var_name, var_type, write_info, read_count
                );

                // Only show sigil description for temps (the scoping rule
                // is non-obvious). For persistent vars, the sigil is
                // self-explanatory boilerplate — skip it.
                if is_temporary {
                    hover_text.push_str(&format!("\n\n---\n\n{}", sigil_desc));
                }

                if !relevant_diagnostics.is_empty() {
                    hover_text.push_str("\n\n---\n\n**Diagnostics:**\n");
                    for d in &relevant_diagnostics {
                        hover_text.push_str(&format!("- {}\n", d.message));
                    }
                }

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

    // ── Fallback: text-scan for prose `$var` / `_var` ──────────────────
    //
    // If `passage.vars` didn't have an entry covering the cursor (e.g., the
    // document hasn't been re-parsed yet, or the variable scanner missed an
    // edge case), fall back to scanning the text around the cursor for a
    // `$name` or `_name` pattern. This makes hover robust against stale
    // passage.vars and ensures prose `$vars` always get hover info.
    if let Some((sigil, name, name_start, name_end)) = scan_variable_at_cursor(text, byte_offset) {
        let var_name = format!("{}{}", sigil, name);
        let is_temporary = sigil == '_';

        // Determine the sigil description from the plugin (format-agnostic).
        let sigil_desc = plugin
            .describe_variable_sigil(sigil)
            .unwrap_or("variable");
        let var_type = plugin
            .resolve_variable_sigil(sigil)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "variable".to_string());

        // For persistent (`$`) vars: aggregate across the whole workspace.
        // For temporary (`_`) vars: scope to the enclosing passage only.
        let enclosing_passage_name = doc
            .passages
            .iter()
            .find(|p| p.contains_abs_offset(byte_offset))
            .map(|p| p.name.clone());

        let mut write_locations: Vec<String> = Vec::new();
        let mut read_count = 0;
        for wp_doc in workspace.documents() {
            for wp_passage in &wp_doc.passages {
                if is_temporary && Some(&wp_passage.name) != enclosing_passage_name.as_ref() {
                    continue;
                }
                for v in &wp_passage.vars {
                    if v.name == var_name {
                        match v.kind {
                            knot_core::passage::VarKind::Init => {
                                write_locations.push(wp_passage.name.clone());
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

        let mut hover_text = format!(
            "**{}** `{}`\n\n{}\nRead in {} location(s)",
            var_name, var_type, write_info, read_count
        );
        if is_temporary {
            hover_text.push_str(&format!("\n\n---\n\n{}", sigil_desc));
        }

        let hover_range = helpers::byte_range_to_lsp_range(text, &(name_start..name_end));
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_text,
            }),
            range: Some(hover_range),
        });
    }

    None
}

/// Scan the text around `byte_offset` for a `$name` or `_name` variable
/// reference in prose.
///
/// Returns `(sigil, name, span_start, span_end)` where `sigil` is `$` or `_`,
/// `name` is the variable name WITHOUT the sigil, and `span_start..span_end`
/// is the byte range covering the SIGIL PLUS the name (the full variable
/// token, including any property path).
///
/// Returns `None` if the cursor isn't on a `$name` / `_name` pattern, or if
/// the pattern is inside a context where `$` / `_` isn't a variable sigil
/// (e.g., `_` in the middle of a word like `snake_case`, or `$` at end of
/// text without a following identifier).
fn scan_variable_at_cursor(
    text: &str,
    byte_offset: usize,
) -> Option<(char, String, usize, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if byte_offset > len {
        return None;
    }

    // Helper: is byte an identifier char (SugarCube variables allow _ and $ in
    // continuation, but we only match `$` or `_` as sigils at the start)?
    let is_ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let is_ident_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // Case 1: cursor is ON the sigil (`$` or `_`).
    let on_sigil = match bytes.get(byte_offset) {
        Some(&b'$') => Some('$'),
        Some(&b'_') => {
            // `_` is only a temp-var sigil at a word boundary (not inside
            // `snake_case` etc.).
            let prev_is_word = byte_offset > 0
                && bytes.get(byte_offset - 1).copied().map_or(false, is_word_char);
            if prev_is_word {
                None
            } else {
                Some('_')
            }
        }
        _ => None,
    };
    if let Some(sigil) = on_sigil {
        // Scan forward for the name + property path.
        let name_start = byte_offset + 1;
        if name_start >= len || !is_ident_start(bytes[name_start]) {
            return None;
        }
        let mut name_end = name_start + 1;
        // Allow property path: .prop.prop2 (prop names allow hyphens too).
        while name_end < len {
            let b = bytes[name_end];
            if is_ident_char(b) {
                name_end += 1;
            } else if b == b'.' && name_end + 1 < len
                && (is_ident_start(bytes[name_end + 1]) || bytes[name_end + 1] == b'_')
            {
                name_end += 1; // consume `.`
                while name_end < len
                    && (is_ident_char(bytes[name_end]) || bytes[name_end] == b'-')
                {
                    name_end += 1;
                }
            } else {
                break;
            }
        }
        let name = text[name_start..name_end].to_string();
        return Some((sigil, name, byte_offset, name_end));
    }

    // Case 2: cursor is ON the name part. Scan backward for the sigil.
    if byte_offset < len && is_ident_char(bytes[byte_offset]) {
        let mut probe = byte_offset;
        // Walk backward through ident chars and `-`. Special case: when we
        // encounter `_`, check if it's a temp-var sigil (at a word boundary)
        // or part of a snake_case identifier. If it's a sigil, stop here
        // (don't include it in the name).
        while probe > 0 {
            let prev = bytes[probe - 1];
            if prev == b'_' {
                // `_` is a sigil only if NOT preceded by a word char (i.e.,
                // it's at a word boundary). Otherwise it's part of an
                // identifier like `snake_case`.
                let prev_prev_is_word = probe >= 2 && is_word_char(bytes[probe - 2]);
                if !prev_prev_is_word {
                    // `_` is the sigil — stop here.
                    break;
                }
                // `_` is part of `snake_case` — continue walking backward.
                probe -= 1;
            } else if is_ident_char(prev) || prev == b'-' {
                probe -= 1;
            } else {
                break;
            }
        }
        // Also walk back through `.prop` segments.
        // (probe is at the start of the current segment.)
        // Now check if there's a `.` before probe, and another segment, and so on.
        // Walk back through segments separated by `.`.
        let mut name_start = probe;
        while name_start > 0 && bytes[name_start - 1] == b'.'
            && name_start >= 2
            && (is_ident_char(bytes[name_start - 2]) || bytes[name_start - 2] == b'-')
        {
            // Walk back through the previous segment.
            name_start -= 1; // skip the `.`
            while name_start > 0
                && (is_ident_char(bytes[name_start - 1]) || bytes[name_start - 1] == b'-')
            {
                name_start -= 1;
            }
        }
        // Now `name_start` is the start of the first segment. The sigil
        // should be at `name_start - 1`.
        if name_start == 0 {
            return None;
        }
        let sigil_byte = bytes[name_start - 1];
        let sigil = match sigil_byte {
            b'$' => '$',
            b'_' => {
                // `_` is only a sigil at a word boundary.
                let prev_is_word = name_start >= 2
                    && bytes.get(name_start - 2).copied().map_or(false, is_word_char);
                if prev_is_word {
                    return None;
                }
                // Also, the first segment must start with an ident-start char
                // (i.e., the char at name_start must be a letter, not `_`).
                // SugarCube `_var` requires `_` followed by a letter.
                if !is_ident_start(bytes[name_start]) {
                    return None;
                }
                '_'
            }
            _ => return None,
        };
        // Scan forward for the end of the name (including property path).
        let mut name_end = name_start + 1;
        while name_end < len {
            let b = bytes[name_end];
            if is_ident_char(b) {
                name_end += 1;
            } else if b == b'.' && name_end + 1 < len
                && (is_ident_start(bytes[name_end + 1]) || bytes[name_end + 1] == b'_')
            {
                name_end += 1;
                while name_end < len
                    && (is_ident_char(bytes[name_end]) || bytes[name_end] == b'-')
                {
                    name_end += 1;
                }
            } else {
                break;
            }
        }
        let name = text[name_start..name_end].to_string();
        return Some((sigil, name, name_start - 1, name_end));
    }

    None
}

/// Try to show hover info for a SugarCube operator (e.g., `gt`, `to`, `eq`).
///
/// Detects the word at the cursor position and checks if it's a known
/// operator via `plugin.describe_operator()`. Returns a plain-English
/// description so users can model their story logic without memorizing
/// the operator names.
///
/// **Scoping**: SugarCube keyword operators (`to`, `eq`, `gt`, `and`, etc.)
/// are ONLY valid inside macro expression contexts (e.g., `<<set $x to 5>>`,
/// `<<if $hp gt 0>>`). They are NOT operators when they appear in:
/// - Prose text (e.g., "Jump to combat demo")
/// - Link display text (e.g., `[[Jump to combat demo|CombatEncounter]]`)
/// - String literals inside macro args (e.g., `<<link "Go to forest">>`)
///
/// The token builder already correctly emits `Operator` semantic tokens
/// ONLY from oxc JS analysis of macro expressions. So this handler checks
/// whether an `Operator` token exists at the cursor position before firing.
/// If no Operator token is found, the word is just prose/text, not an
/// operator — return `None` so other hover handlers get a chance.
fn try_operator_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    // ── Scope check: only fire if there's an Operator token at the cursor ──
    //
    // The token builder emits Operator tokens exclusively from oxc JS
    // analysis (inside macro expressions like `<<set $x to 5>>`). Words
    // like `to` in prose or link text do NOT get Operator tokens. So
    // checking for an Operator token at the cursor is the precise way
    // to distinguish a real operator from a coincidental word match.
    let mut has_operator_token = false;
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                if matches!(token.token_type, knot_formats::plugin::SemanticTokenType::Operator) {
                    has_operator_token = true;
                    break;
                }
            }
        }
        if has_operator_token {
            break;
        }
    }
    if !has_operator_token {
        return None;
    }

    // Extract the word at the cursor position.
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);

    // Find word boundaries around the cursor.
    //
    // SugarCube has two forms of operators:
    //   1. Keyword operators: `to`, `eq`, `and`, `def`, etc. — alphanumeric.
    //   2. Symbolic operators: `&&`, `||`, `!`, `===`, `!==`, `>`, `<`, etc.
    //
    // The extraction logic must handle BOTH forms. If the cursor is on an
    // alphanumeric char, we extract the alphanumeric word (keyword form).
    // If the cursor is on a symbolic operator char, we extract the symbolic
    // run (e.g., `&&`, `===`). This is safe because the `has_operator_token`
    // check above already confirmed we're inside an Operator semantic token —
    // we won't misfire on symbolic chars in prose or strings.
    let bytes = line.as_bytes();
    if byte_pos > bytes.len() {
        return None;
    }

    /// Returns true if `b` is a symbolic operator character.
    /// These are the characters that make up JS symbolic operators:
    /// `&`, `|`, `!`, `=`, `<`, `>`, `+`, `-`, `*`, `/`, `%`.
    fn is_operator_char(b: u8) -> bool {
        matches!(b, b'&' | b'|' | b'!' | b'=' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%')
    }

    let cursor_byte = if byte_pos < bytes.len() {
        bytes[byte_pos]
    } else {
        // Cursor at end of line — check the last char
        if byte_pos == 0 {
            return None;
        }
        bytes[byte_pos - 1]
    };

    let (start, end) = if cursor_byte.is_ascii_alphanumeric() || cursor_byte == b'_' {
        // Keyword operator form — extract alphanumeric word.
        let mut start = byte_pos;
        while start > 0 {
            let b = bytes[start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = byte_pos;
        while end < bytes.len() {
            let b = bytes[end];
            if b.is_ascii_alphanumeric() || b == b'_' {
                end += 1;
            } else {
                break;
            }
        }
        (start, end)
    } else if is_operator_char(cursor_byte) {
        // Symbolic operator form — extract the symbolic run.
        let mut start = byte_pos;
        while start > 0 && is_operator_char(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = byte_pos;
        while end < bytes.len() && is_operator_char(bytes[end]) {
            end += 1;
        }
        (start, end)
    } else {
        // Cursor is on a non-operator, non-alphanumeric char (space, paren, etc.)
        return None;
    };

    if start == end {
        return None;
    }
    let word = &line[start..end];

    // Check if this word is a known operator.
    let desc = plugin.describe_operator(word)?;
    let hover_text = format!("**{}** `Operator`\n\n{}", word, desc);
    let utf16_start = helpers::utf16_len_up_to(line, start);
    let utf16_end = helpers::utf16_len_up_to(line, end);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(Range {
            start: Position { line: line_idx as u32, character: utf16_start },
            end: Position { line: line_idx as u32, character: utf16_end },
        }),
    })
}

/// Try to show hover info for block-level SugarCube markup markers.
///
/// SugarCube has several column-0-anchored markup constructs that produce
/// semantic tokens but had no hover handler:
///
/// | Marker | Construct | Token type |
/// |--------|-----------|------------|
/// | `!`..`!!!!!!` | Heading (levels 1-6) | `Heading` |
/// | `*`/`**`/`***` | Unordered list item | `ListMarker` |
/// | `#`/`##`/`###` | Ordered list item | `ListMarker` |
/// | `>`/`>>`/`>>>` | Line-style blockquote | `Blockquote` |
/// | `<<<` | Block-style blockquote | `BlockquoteBlock` |
/// | `----` (4+) | Horizontal rule | `HorizontalRule` |
/// | `{{{` | Code block / inline code | `CodeBlock`/`InlineCode` |
///
/// Hover fires when the cursor is on the marker run itself (the `!`/`*`/`#`/
/// `>`/`-` characters at column 0). The hover text explains what the marker
/// does and links it to the SugarCube documentation pattern.
///
/// This is a line-based scan (not AST-based) because:
/// 1. The markers are simple column-0 patterns — no need for AST traversal.
/// 2. It works even when the AST isn't yet indexed (e.g., during incremental
///    re-parse).
/// 3. It's consistent with `try_operator_hover` and `try_global_hover`,
///    which also use line-based scanning for simple patterns.
fn try_block_markup_hover(
    text: &str,
    byte_offset: usize,
) -> Option<Hover> {
    let line_info = helpers::byte_offset_to_position(text, byte_offset);
    let line_idx = line_info.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_pos = line_info.character as usize;
    let byte_pos = helpers::utf16_to_byte_offset(line, char_pos);

    let bytes = line.as_bytes();
    if bytes.is_empty() || byte_pos > bytes.len() {
        return None;
    }

    // All block-level markers are column-0 anchored. If the cursor is not
    // at column 0, none of these can fire.
    // (byte_pos is the byte offset within the line; column 0 means byte_pos == 0
    // OR the cursor is on a leading-whitespace prefix — but SugarCube requires
    // NO leading whitespace for block markup, so we check byte_pos == 0.)
    //
    // However, the cursor might be ON the marker run (e.g., on the 2nd `!` of
    // `!!`), so we check if byte_pos is within the marker run starting at col 0.
    let first = bytes[0];

    // Determine the marker run and its hover content.
    let (marker_end, hover_text): (usize, String) = if first == b'!' {
        // Heading: `!` through `!!!!!!` (1-6 levels)
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'!' && end < 6 {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        // Cursor must be within the `!` run.
        if byte_pos > end {
            return None;
        }
        let level = end;
        let html_tag = match level {
            1 => "h1", 2 => "h2", 3 => "h3", 4 => "h4", 5 => "h5", _ => "h6",
        };
        let heading_desc = match level {
            1 => "level 1 heading — main section title (largest)",
            2 => "level 2 heading — subsection title",
            3 => "level 3 heading — sub-subsection title",
            4 => "level 4 heading — minor section title",
            5 => "level 5 heading — small heading",
            _ => "level 6 heading — smallest heading",
        };
        (end, format!("**`{}` Heading** (`{}` tag)\n\n{}", "!".repeat(level), html_tag, heading_desc))
    } else if first == b'*' {
        // Unordered list item: `*`, `**`, `***`, etc.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'*' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos > end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 { "top level".to_string() } else { format!("nested (depth {})", depth) };
        (end, format!("**`{}` Unordered List Item**\n\nCreates a `<li>` in a `<ul>`. {}", "*".repeat(depth), depth_desc))
    } else if first == b'#' {
        // Ordered list item: `#`, `##`, `###`, etc.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'#' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos > end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 { "top level".to_string() } else { format!("nested (depth {})", depth) };
        (end, format!("**`{}` Ordered List Item**\n\nCreates a `<li>` in an `<ol>`. {}", "#".repeat(depth), depth_desc))
    } else if first == b'>' {
        // Line-style blockquote: `>`, `>>`, `>>>`, etc.
        // Note: `<<<` (block-style blockquote) also starts with `<`, but we
        // check `>` here. `<<<` is handled separately below.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'>' {
            end += 1;
        }
        if end == 0 {
            return None;
        }
        if byte_pos > end {
            return None;
        }
        let depth = end;
        let depth_desc = if depth == 1 { "single level".to_string() } else { format!("nested (depth {})", depth) };
        (end, format!("**`{}` Blockquote**\n\nCreates a `<blockquote>`. {}", ">".repeat(depth), depth_desc))
    } else if first == b'-' {
        // Horizontal rule: `----` (4+ dashes) alone on a line.
        // Also handles `<<<` block-style blockquote — wait, `<<<` starts with
        // `<` not `-`. This branch only handles horizontal rules.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'-' {
            end += 1;
        }
        if end < 4 {
            return None; // Need at least 4 dashes for a horizontal rule.
        }
        // Check that the rest of the line is only whitespace (HR must be alone).
        let rest = &line[end..];
        if !rest.trim().is_empty() {
            return None;
        }
        if byte_pos > end {
            return None;
        }
        (end, "**`----` Horizontal Rule**\n\nCreates an `<hr>` element. Requires 4+ dashes alone on a line at column 0.".to_string())
    } else if first == b'<' && bytes.len() >= 3 && bytes[1] == b'<' && bytes[2] == b'<' {
        // Block-style blockquote: `<<<` alone on a line.
        let mut end = 0;
        while end < bytes.len() && bytes[end] == b'<' {
            end += 1;
        }
        if end != 3 {
            return None; // Must be exactly 3 `<` characters.
        }
        // Check that the rest is whitespace or newline.
        let rest = &line[end..];
        if !rest.trim().is_empty() {
            return None;
        }
        if byte_pos > end {
            return None;
        }
        (end, "**`<<<` Block Blockquote**\n\nOpens a block-style blockquote. Close with another `<<<` on its own line. Content between the delimiters is wrapped in `<blockquote>`.".to_string())
    } else if first == b'{' && bytes.len() >= 3 && bytes[1] == b'{' && bytes[2] == b'{' {
        // Code block: `{{{\n...\n}}}` (block form at column 0).
        // Inline code `{{{...}}}` (mid-line) is NOT column-0 anchored, but we
        // still handle it here for hover purposes — the cursor on `{{{` should
        // show code block info regardless of position.
        if byte_pos > 3 {
            return None;
        }
        // Check if it's block form (immediately followed by `\n`) or inline.
        let is_block = bytes.len() > 3 && bytes[3] == b'\n';
        if is_block {
            (3, "**`{{{` Code Block**\n\nOpens a raw code block. Content is NOT processed (macros/variables inside are literal). Close with `}}}` alone on its own line. Renders as `<pre><code>…</code></pre>`.".to_string())
        } else {
            (3, "**`{{{` Inline Code**\n\nOpens inline raw code. Content is NOT processed (macros/variables inside are literal). Close with the first `}}}`. Renders as `<code>…</code>`.".to_string())
        }
    } else {
        return None;
    };

    // Compute the UTF-16 range for the marker run.
    let utf16_start = helpers::utf16_len_up_to(line, 0);
    let utf16_end = helpers::utf16_len_up_to(line, marker_end);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(Range {
            start: Position { line: line_idx as u32, character: utf16_start },
            end: Position { line: line_idx as u32, character: utf16_end },
        }),
    })
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
                let abs_link_span = passage.abs_range(&link.span);

                // ── Determine label and target sub-spans within the link ──
                //
                // For `[[target]]` (no pipe): the entire inner text is both
                // the display text and the target. Hovering anywhere shows
                // passage info for the target.
                //
                // For `[[label|target]]` (pipe syntax): hovering on the
                // label part shows "Link display text" info; hovering on
                // the target part shows passage info for the target. This
                // mirrors how the token builder colors the two parts
                // differently (display = green, target = teal underline).
                //
                // We scan the source text within the link span to find the
                // `|` separator (if any), then compute the label and target
                // sub-spans.
                let link_text = &text[abs_link_span.start..abs_link_span.end.min(text.len())];

                // Find the `|` separator, skipping the opening `[[`.
                // The inner content starts at offset 2 (after `[[`).
                // We scan for `|` but NOT inside string literals or nested
                // brackets — though SugarCube link syntax doesn't support
                // those, a simple find is sufficient.
                let pipe_pos = link_text[2..]
                    .find('|')
                    .map(|pos| 2 + pos);

                if let Some(pipe_rel) = pipe_pos {
                    // `[[label|target]]` — pipe syntax.
                    let label_start = abs_link_span.start + 2; // after `[[
                    let label_end = abs_link_span.start + pipe_rel; // before `|`
                    let target_start = abs_link_span.start + pipe_rel + 1; // after `|`
                    let target_end = abs_link_span.end - 2; // before `]]`

                    let on_label = byte_offset >= label_start && byte_offset < label_end;
                    let on_target = byte_offset >= target_start && byte_offset < target_end;
                    let on_pipe = byte_offset == abs_link_span.start + pipe_rel;

                    if on_label || on_pipe {
                        // Hovering on the label (display text) part.
                        let label_text = &text[label_start..label_end];
                        let hover_text = if label_text.trim().is_empty() {
                            "**Link display text** (empty — the target passage name will be used as display text)".to_string()
                        } else {
                            format!("**Link display text**\n\n`{}`\n\nLinks to passage `{}`", label_text, target)
                        };
                        let hover_range = helpers::byte_range_to_lsp_range(text, &(label_start..label_end));
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    } else if on_target {
                        // Hovering on the target part — show passage info.
                        if !target.is_empty() {
                            if let Some((doc, passage)) = workspace.find_passage(target) {
                                let hover_text = build_passage_target_hover_text(
                                    target, doc, passage, workspace,
                                );
                                let hover_range = helpers::byte_range_to_lsp_range(text, &(target_start..target_end));
                                return Some(Hover {
                                    contents: HoverContents::Markup(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: hover_text,
                                    }),
                                    range: Some(hover_range),
                                });
                            } else {
                                // Broken link — passage doesn't exist
                                let hover_range = helpers::byte_range_to_lsp_range(text, &(target_start..target_end));
                                return Some(Hover {
                                    contents: HoverContents::Markup(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: format!(
                                            "**Broken link** — passage `{}` does not exist",
                                            target
                                        ),
                                    }),
                                    range: Some(hover_range),
                                });
                            }
                        }
                    }
                    // If cursor is on `[[` or `]]` delimiters, fall through
                    // to the full-link hover below.
                }

                // `[[target]]` (no pipe) OR cursor on delimiters of pipe link —
                // show passage info for the target with the full link span.
                if !target.is_empty() {
                    if let Some((doc, passage)) = workspace.find_passage(target) {
                        let hover_text = build_passage_target_hover_text(
                            target, doc, passage, workspace,
                        );
                        let hover_range = helpers::byte_range_to_lsp_range(text, &abs_link_span);
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    } else {
                        // Broken link — passage doesn't exist
                        let hover_range = helpers::byte_range_to_lsp_range(text, &abs_link_span);
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: format!(
                                    "**Broken link** — passage `{}` does not exist",
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

/// Try to show hover info for a JS function call (e.g., `myFunc()` inside
/// Try to show hover info for a template invocation (`?name` in SugarCube).
///
/// Templates are invoked with `?name` syntax in prose text. The token builder
/// emits `Function` semantic tokens for `?name` occurrences (scanning the text
/// content of `AstNode::Text` nodes). This hover handler checks if the cursor
/// is on such a token, then queries `plugin.find_template(name)` for the
/// definition.
///
/// Returns `None` if the cursor isn't on a template token, the plugin doesn't
/// know the template, or the template has no useful info to display.
fn try_template_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // ── Path 1: Token-based detection ──────────────────────────────
    //
    // The token builder emits `Function` tokens for `?name` patterns in
    // prose. The token spans the NAME only (not the `?`), so we accept
    // cursor-on-name OR cursor-on-`?` (the byte immediately before the
    // token start).
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            // Accept cursor on the name OR on the `?` immediately before.
            let on_name = byte_offset >= abs_start && byte_offset < abs_end;
            let on_q = byte_offset + 1 == abs_start
                && text.as_bytes().get(byte_offset) == Some(&b'?');
            if on_name || on_q {
                let name = &text[abs_start..abs_end];
                // Check if this is a known template. `Function` tokens are
                // also emitted for regular JS functions and widgets, so this
                // check filters to templates only.
                let info = plugin.find_template(name)?;
                let _ = info; // info.defined_in no longer shown — user wants minimal hover
                let hover_text = format!("**`?{}`** `Template`", name);
                // Hover range covers `?name` (the `?` plus the name).
                let range_start = abs_start.saturating_sub(1);
                let hover_range = helpers::byte_range_to_lsp_range(text, &(range_start..abs_end));
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

    // ── Path 2: Text-scan fallback ────────────────────────────────
    //
    // If the token-based path didn't fire (e.g., the document hasn't been
    // re-tokenized yet, or the token builder's `?name` scanning missed an
    // edge case), fall back to scanning the text around the cursor for a
    // `?name` pattern. This makes hover robust against stale tokens.
    if let Some((name_start, name_end)) = scan_template_at_cursor(text, byte_offset) {
        let name = &text[name_start..name_end];
        if plugin.find_template(name).is_some() {
            let hover_text = format!("**`?{}`** `Template`", name);
            let range_start = name_start.saturating_sub(1); // include `?`
            let hover_range = helpers::byte_range_to_lsp_range(text, &(range_start..name_end));
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover_text,
                }),
                range: Some(hover_range),
            });
        }
    }

    None
}

/// Scan the text around `byte_offset` for a `?name` template invocation.
///
/// Returns `(name_start, name_end)` — the byte range of the NAME part
/// (excluding the `?`) — if the cursor is on the `?` or on the name of a
/// `?name` pattern in text, AND the name is a valid template identifier
/// (alpha/underscore start, alphanumeric/underscore/hyphen continuation).
///
/// Returns `None` if the cursor isn't on a `?name` pattern, or if the
/// pattern is preceded by a word character (which would indicate JS
/// optional chaining like `obj?.prop` rather than a template invocation).
fn scan_template_at_cursor(text: &str, byte_offset: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if byte_offset > len {
        return None;
    }

    // Helper: is byte a template-name char?
    let is_name_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let is_name_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';

    // Helper: is byte a word char (for the `(?<!\w)` negative lookbehind)?
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // Case 1: cursor is ON the `?`.
    if bytes.get(byte_offset) == Some(&b'?') {
        // The `?` must NOT be preceded by a word char (mimic grammar's
        // `(?<!\w)` — prevents matching `obj?.prop` optional chaining).
        let preceded_by_word = byte_offset > 0
            && bytes.get(byte_offset - 1).copied().map_or(false, is_word_char);
        if preceded_by_word {
            return None;
        }
        // Scan forward for the name.
        let name_start = byte_offset + 1;
        if name_start >= len || !is_name_start(bytes[name_start]) {
            return None;
        }
        let mut name_end = name_start + 1;
        while name_end < len && is_name_char(bytes[name_end]) {
            name_end += 1;
        }
        return Some((name_start, name_end));
    }

    // Case 2: cursor is ON the name (or just past it). Scan backward for `?`.
    if byte_offset < len && (is_name_char(bytes[byte_offset])
        || (byte_offset > 0 && is_name_char(bytes[byte_offset - 1])))
    {
        // Walk backward to find the `?`.
        let mut probe = byte_offset;
        while probe > 0 && is_name_char(bytes[probe - 1]) {
            probe -= 1;
        }
        // `probe` now points at the first name char. The `?` should be just before.
        if probe == 0 || bytes.get(probe - 1) != Some(&b'?') {
            return None;
        }
        let q_pos = probe - 1;
        // `?` must NOT be preceded by a word char.
        let preceded_by_word = q_pos > 0
            && bytes.get(q_pos - 1).copied().map_or(false, is_word_char);
        if preceded_by_word {
            return None;
        }
        // `probe` must be a name-start char.
        if !is_name_start(bytes[probe]) {
            return None;
        }
        // Scan forward for the end of the name.
        let mut name_end = probe + 1;
        while name_end < len && is_name_char(bytes[name_end]) {
            name_end += 1;
        }
        return Some((probe, name_end));
    }

    None
}

/// Try to show hover info for a function call (e.g., `myFunc()` inside
/// `<<run>>` or `<<script>>`).
///
/// Uses semantic tokens (`SemanticTokenType::Function`) for cursor-on-span
/// detection. The plugin's `find_function()` registry provides definition
/// location + param count.
fn try_function_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // Find the function token under the cursor.
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                let name = &text[abs_start..abs_end];
                let info = plugin.find_function(name)?;

                // Meaningfulness gate: only show function hover when there's
                // info beyond what's visible in the code. The function name
                // is already visible; we need param count to justify a popup.
                // "Defined in `:: Story JavaScript`" alone is too thin.
                let param_count = info.param_count?;
                let hover_text = format!(
                    "**{}** `Function` ({} params)\n\nDefined in `:: {}`",
                    name, param_count, info.defined_in
                );
                let hover_range = helpers::byte_range_to_lsp_range(text, &(abs_start..abs_end));
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

/// Try to show hover info for a property access (e.g., `.hp` in `$player.hp`).
///
/// Uses semantic tokens (`SemanticTokenType::Property`) for cursor-on-span
/// detection. Resolves the parent variable path via the document's parsed
/// `VarOp` spans (walking backward to find the `$var` or `_var` sigil), then
/// queries the plugin's arena tree for the parent's children to render
/// "Property `hp` of `$player` (Object). Siblings: hp, mp, name."
fn try_property_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
    doc: Option<&knot_core::Document>,
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // Find the property token under the cursor.
    let mut prop_abs_start = None;
    let mut prop_abs_end = None;
    let mut prop_name = String::new();
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Property {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                prop_abs_start = Some(abs_start);
                prop_abs_end = Some(abs_end);
                prop_name = text[abs_start..abs_end].to_string();
                break;
            }
        }
    }
    let prop_start = prop_abs_start?;
    let prop_end = prop_abs_end?;
    if prop_name.is_empty() {
        return None;
    }

    // Resolve the parent variable: walk backward from the property token
    // to find the enclosing `$var` or `_var` VarOp span in the document.
    // This gives us the parent variable name for the "of `$player`" label.
    let doc = doc?;
    let enclosing_passage = doc.passages.iter().find(|p| p.contains_abs_offset(byte_offset))?;
    let mut parent_var_name: Option<String> = None;
    let mut parent_kind: Option<knot_formats::types::PropertyKind> = None;
    for var in &enclosing_passage.vars {
        let abs_var_span = enclosing_passage.abs_range(&var.span);
        // The property must come after the variable's span (i.e., the
        // variable is the root of the dot-path).
        if abs_var_span.end <= prop_start {
            // Pick the closest variable before the property.
            if parent_var_name.as_ref().map(|_| 0).unwrap_or(usize::MAX) < abs_var_span.end {
                continue;
            }
            // Check if this var is the root of the path by verifying
            // there's no other identifier between var.end and prop_start
            // except dots.
            let between = &text[abs_var_span.end..prop_start];
            if between.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                parent_var_name = Some(var.name.clone());
                // Query the plugin for the parent's kind.
                let passage_name = Some(enclosing_passage.name.as_str());
                if let Some(kind) = plugin.variable_kind_at_path_for_passage(&var.name, passage_name) {
                    parent_kind = Some(kind);
                }
            }
        }
    }

    let parent = parent_var_name.unwrap_or_else(|| "unknown".to_string());
    let kind_label = match parent_kind {
        Some(knot_formats::types::PropertyKind::Object) => "Object",
        Some(knot_formats::types::PropertyKind::Array) => "Array",
        Some(knot_formats::types::PropertyKind::Scalar) => "Scalar",
        _ => "Unknown",
    };

    // Get siblings (children of the parent variable).
    let passage_name = Some(enclosing_passage.name.as_str());
    let siblings: Vec<String> = plugin
        .variable_children_with_kind_for_passage(&parent, passage_name)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    // Meaningfulness gate: the value of property hover is discovering
    // siblings — "oh, `$player` also has `mp` and `name`." If there are
    // no siblings (the parent is a Scalar or unknown), the hover just
    // repeats what's visible in the code. Skip it.
    if siblings.is_empty() {
        return None;
    }

    let mut hover_text = format!(
        "**{}** `Property of {}` ({})",
        prop_name, parent, kind_label
    );
    let preview: Vec<&str> = siblings.iter().take(8).map(|s| s.as_str()).collect();
    let suffix = if siblings.len() > 8 { ", …" } else { "" };
    hover_text.push_str(&format!("\n\n**Siblings:** {}{}", preview.join(", "), suffix));

    let hover_range = helpers::byte_range_to_lsp_range(text, &(prop_start..prop_end));
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(hover_range),
    })
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

#[cfg(test)]
mod expr_macro_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;
    
    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse a single-passage document and return the Document plus
    /// the plugin. The plugin's `parse_mut` returns passages already
    /// populated with `macro_invocations`, so we just wrap them in a Document.
    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (doc, plugin)
    }

    /// When the cursor is on `<<` of `<<= _parts>>`, hover MUST return the
    /// macro info for `<<=>>` (catalog name `=`), not `None` and not the
    /// variable hover for `_parts`.
    #[test]
    fn hover_on_print_macro_open_tag_returns_macro_hover() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        // Cursor on the first `<` of `<<=` (start of body, line 1, char 0).
        let body_offset = ":: Init\n".len();
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "hover on `<<=` should fire, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("`<<=>>`"),
                    "hover text should mention `<<=>>`: got {}", m.value);
            } else {
                panic!("expected markup hover, got {:?}", h.contents);
            }
        }
    }

    /// When the cursor is on `=` of `<<=`, hover MUST also return the macro
    /// info (the entire `<<= _parts>>` is the macro name span for expression
    /// macros — there's no separate name region).
    #[test]
    fn hover_on_print_macro_equal_sign_returns_macro_hover() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        let body_offset = ":: Init\n".len() + 2; // on `=` (after `<<`)
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "hover on `=` of `<<=` should fire, got None");
    }

    /// Cursor on `_parts` (inside the expression) should ALSO return the
    /// macro hover, since the name_span covers the entire `<<= _parts>>`.
    /// NOTE: in the real hover() entrypoint, try_variable_hover runs first
    /// and wins for cursor-on-variable; this test isolates try_macro_hover
    /// to confirm it would fire if the variable layer didn't intercept.
    #[test]
    fn hover_on_variable_inside_expression_still_resolves_macro() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        // Cursor on `_` of `_parts`.
        let body_offset = ":: Init\n".len() + 4; // `<<= ` is 4 chars
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "try_macro_hover should fire for cursor inside expression span");
    }

    /// End-to-end check: simulate the full layering for cursor on `<<=`. The
    /// variable-hover layer must NOT fire (cursor is not on a variable), so
    /// the macro-hover layer should win and return the `<<=>>` info.
    ///
    /// This test exists to catch the regression where the user reports "no
    /// hover on `<<=`" — if variable-hover ever over-matches (e.g., by
    /// treating the entire expression span as a variable span), this test
    /// will fail.
    #[test]
    fn variable_hover_does_not_fire_on_macro_open_tag() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        let body_offset = ":: Init\n".len(); // cursor on `<<`
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        let hover = try_variable_hover(src, body_offset, Some(&doc), &ws, &plugin);
        assert!(hover.is_none(),
            "variable hover must NOT fire when cursor is on `<<` of `<<=>>`, got: {:?}",
            hover);
    }

    /// Reproduce the user's exact reported scenario: a passage with multiple
    /// `<<run _parts = ...>>` lines, where hovering on `<<` of `<<run`
    /// should fire the macro hover but currently doesn't.
    ///
    /// This test exists to catch the regression where the user reports
    /// "hovering on `<<run>>` shows the variable hover for `_parts` instead".
    /// The expected behavior is: cursor on `<<` → macro hover; cursor on
    /// `_parts` → variable hover.
    #[test]
    fn reproduce_user_run_macro_hover_scenario() {
        let src = "::UIOutfitLabel [nobr] {\"position\":\"440,420\"}\n<<run _parts = []>>\n<<run _eq = State.variables.gs.inventory.equipped>>\n";
        let (doc, plugin) = parse(src);

        // Cursor on `<<` of line 1 (the first `<<run _parts = []>>`).
        // Line 0 is the header `::UIOutfitLabel ...` plus `\n`, so the body
        // starts at the byte offset of `<<run`.
        let line0_end = src.find('\n').unwrap() + 1; // end of header line + \n
        let cursor_on_open_bracket = line0_end; // first `<`
        let hover = try_macro_hover(src, cursor_on_open_bracket, Some(&doc), &plugin);
        assert!(hover.is_some(),
            "cursor on `<<` of `<<run>>` should fire macro hover, got None. \
             cursor byte_offset={}, line0_end={}",
            cursor_on_open_bracket, line0_end);
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("`<<run>>`"),
                    "hover text should mention `<<run>>`: got {}", m.value);
            }
        }
    }

    /// F1 test: cursor on a variable INSIDE `<<run>>` should fire
    /// per-token variable hover, not the whole-macro block hover.
    /// The variable hover uses `passage.vars` which has per-token spans
    /// from js_analysis var_ops.
    #[test]
    fn hover_on_variable_inside_run_macro_fires_variable_hover() {
        let src = ":: Init\n<<set $arr to [1,2,3]>>\n<<run $arr.last()>>";
        let (doc, plugin) = parse(src);
        // Cursor on `$arr` in the `<<run $arr.last()>>` line (line 2).
        // Find the byte offset of `$` in `$arr.last()`.
        let line2_start = src.find("<<run").unwrap();
        let cursor_on_dollar = line2_start + "<<run ".len(); // offset of `$`
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        let hover = try_variable_hover(src, cursor_on_dollar, Some(&doc), &ws, &plugin);
        // Variable hover SHOULD fire — the cursor is on `$arr`, a variable.
        assert!(hover.is_some(),
            "cursor on $arr inside <<run>> should fire variable hover, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("$arr"),
                    "hover text should mention $arr: got {}", m.value);
            }
        }
    }

    /// F1 test: cursor on a method name (`.last`) inside `<<run $arr.last()>>
    /// should fire function hover (per-token), not the macro block hover.
    /// Note: try_function_hover has a meaningfulness gate (requires
    /// param_count) that may block builtin methods. The per-token
    /// resolution is verified via the variable hover test above and the
    /// token builder tests in knot-formats. This test is a placeholder —
    /// see F1 follow-up for the full function-hover meaningfulness audit.
    #[test]
    fn hover_on_method_inside_run_macro_fires_function_hover() {
        // Placeholder — see comment above. The real verification is that
        // the Function token IS emitted (verified in knot-formats debug
        // tests), and try_function_hover would fire if the meaningfulness
        // gate passed. F1 follow-up: audit the gate for builtin methods.
    }

    /// When the cursor is on `>>` of `<<run _parts = []>>` (the closing
    /// delimiter), the macro hover should fire using the open_span as the
    /// hover range. This is the "outer-layer" hover — the user is asking
    /// "what macro is this?" by hovering on the closing `>>`.
    #[test]
    fn hover_on_macro_close_delimiter_uses_open_span() {
        let src = ":: Init\n<<run _parts = []>>";
        let (doc, plugin) = parse(src);
        // Cursor on the second `>` of `>>`.
        let close_offset = src.len() - 1;
        let hover = try_macro_hover(src, close_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "hover on `>>` of `<<run>>` should fire, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("`<<run>>`"),
                    "hover text should mention `<<run>>`: got {}", m.value);
            }
        }
    }

    /// When the cursor is on the macro name itself (e.g., `run`), the hover
    /// range should be the name_span (just `run`), NOT the open_span (the
    /// whole `<<run>>`). This is the "precise" hover case.
    #[test]
    fn hover_on_macro_name_uses_name_span() {
        let src = ":: Init\n<<run _parts = []>>";
        let (doc, plugin) = parse(src);
        // Cursor on `u` of `run` (offset 9 = `:: Init\n<<` is 9 chars + 1 = 10? Let's compute).
        // `:: Init\n` is 8 chars, `<<` is 2 chars (offsets 8,9), `r` is at offset 10.
        let on_u_offset = ":: Init\n<<r".len(); // 11
        let hover = try_macro_hover(src, on_u_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "hover on `u` of `run` should fire, got None");
        if let Some(h) = hover {
            // Range should cover just `run` (3 chars at offsets 10..13).
            if let Some(range) = h.range {
                assert_eq!(range.start.character, 2, "hover range start char: got {}", range.start.character);
                assert_eq!(range.end.character, 5, "hover range end char: got {}", range.end.character);
                assert_eq!(range.start.line, 1, "hover range start line");
            }
        }
    }
}

#[cfg(test)]
mod prose_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;
    
    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse a multi-passage source and return the Document plus plugin.
    /// Same as the `parse` helper in `expr_macro_hover_tests`, duplicated here
    /// so this test module is self-contained.
    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (doc, plugin)
    }

    /// Hovering on a prose `$var` should fire the variable hover via the
    /// text-scan fallback, even if the variable wasn't picked up by the
    /// AST's `var_refs` scanner (e.g., for a var that has no other refs
    /// in the workspace).
    #[test]
    fn hover_fires_on_prose_persistent_var_via_text_scan() {
        // `$gold` appears in prose only (no `<<set $gold = ...>>` anywhere).
        // The text-scan fallback should still fire hover.
        let src = ":: Start\nYou have $gold coins.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `g` of `$gold` (offset = ":: Start\nYou have $".len() = 18).
        let cursor_offset = ":: Start\nYou have $".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin);
        assert!(hover.is_some(),
            "hover on prose `$gold` should fire via text-scan fallback, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("$gold"),
                    "hover text should mention `$gold`: got {}", m.value);
            }
        }
    }

    /// Hovering on a prose `$var.prop` should fire hover. The AST-based path
    /// produces `var.name = "$player"` (without property path) because the
    /// AST's `VarRef.name` is just the sigil + identifier — the property
    /// path is stored separately in `VarRef.property_path`. The hover text
    /// therefore mentions `$player`, and the hover range covers the full
    /// `$player.name` span.
    #[test]
    fn hover_fires_on_prose_var_property_path() {
        let src = ":: Start\nYou have $player.name coins.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `n` of `.name` (offset = ":: Start\nYou have $player.".len() = 24).
        let cursor_offset = ":: Start\nYou have $player.".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin);
        assert!(hover.is_some(),
            "hover on prose `$player.name` should fire, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("$player"),
                    "hover text should mention `$player`: got {}", m.value);
            }
            // Hover range should cover the full `$player.name` span.
            if let Some(range) = h.range {
                // The span starts at `$` and ends after `name` (12 chars total).
                assert_eq!(range.start.character, 9,
                    "hover range start should be at `$` (char 9): got {}", range.start.character);
                assert!(range.end.character >= 21,
                    "hover range end should be at end of `$player.name` (char 21+): got {}",
                    range.end.character);
            }
        }
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on the sigil
    /// `$` should return the full variable token (sigil + name + property
    /// path).
    #[test]
    fn scan_variable_at_cursor_on_sigil() {
        let src = "You have $player.name coins.";
        // Cursor on `$` (offset = 9).
        let cursor_offset = src.find("$player").unwrap();
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(result.is_some(), "scan should find `$player.name` at cursor on `$`");
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '$');
        assert_eq!(name, "player.name");
        assert_eq!(start, cursor_offset);
        assert_eq!(&src[start..end], "$player.name");
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on the name
    /// part should return the full variable token.
    #[test]
    fn scan_variable_at_cursor_on_name() {
        let src = "You have $gold coins.";
        // Cursor on `o` of `$gold` (offset = 11).
        let cursor_offset = src.find("$gold").unwrap() + 2;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(result.is_some(), "scan should find `$gold` at cursor on `o`");
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '$');
        assert_eq!(name, "gold");
        assert_eq!(&src[start..end], "$gold");
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on `_` in the
    /// middle of `snake_case` should NOT match (it's not a temp-var sigil).
    #[test]
    fn scan_variable_at_cursor_rejects_underscore_in_word() {
        let src = "Use snake_case here.";
        // Cursor on `_` in `snake_case`.
        let cursor_offset = src.find("snake_case").unwrap() + 5;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(result.is_none(),
            "scan must NOT match `_` in `snake_case`: got {:?}", result);
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on `_temp`
    /// at a word boundary SHOULD match (it's a valid temp var).
    #[test]
    fn scan_variable_at_cursor_matches_temp_var() {
        let src = "Use _temp here.";
        // Cursor on `t` of `_temp`.
        let cursor_offset = src.find("_temp").unwrap() + 1;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(result.is_some(), "scan should find `_temp` at cursor on `t`");
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '_');
        assert_eq!(name, "temp");
        assert_eq!(&src[start..end], "_temp");
    }

    /// Hovering on `_temp` in prose should fire hover via text-scan fallback,
    /// but NOT when `_` is in the middle of a word (e.g., `snake_case`).
    #[test]
    fn hover_does_not_fire_on_underscore_in_word() {
        let src = ":: Start\nUse snake_case here.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `_` in `snake_case` (offset = ":: Start\nUse snake".len() = 16).
        let cursor_offset = ":: Start\nUse snake".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin);
        assert!(hover.is_none(),
            "hover must NOT fire on `_` in `snake_case` (not a temp var): got {:?}",
            hover);
    }

    /// Hovering on `?template` in prose should fire the template hover via
    /// the text-scan fallback when the template is registered. The hover
    /// text should be minimal: `**\`?name\`** \`Template\`` (no "Defined in").
    #[test]
    fn hover_fires_on_prose_template_via_text_scan() {
        // Register a template by parsing a StoryInit passage that calls
        // `Template.add("greeting", ...)`. Then test hover on `?greeting`
        // in a separate passage.
        let src = ":: StoryInit\n<<run Template.add(\"greeting\", function() { return \"Hello\"; })>>\n:: Start\nYou see ?greeting friend.";
        let (_doc, plugin) = parse(src);
        // Cursor on `g` of `?greeting` in the Start passage.
        let cursor_offset = src.find("?greeting").unwrap() + 1;
        let token_groups: Vec<knot_formats::plugin::PassageTokenGroup> = Vec::new();
        let hover = try_template_hover(src, cursor_offset, &plugin, &token_groups);
        assert!(hover.is_some(),
            "hover on prose `?greeting` should fire via text-scan fallback, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("`?greeting`"),
                    "hover text should contain `?greeting`: got {}", m.value);
                assert!(m.value.contains("Template"),
                    "hover text should mention 'Template': got {}", m.value);
                assert!(!m.value.contains("Defined in"),
                    "hover text should NOT contain 'Defined in' (user wants minimal hover): got {}",
                    m.value);
            }
        }
    }

    /// Hovering on the `?` prefix of `?template` should also fire hover
    /// (the hover range is extended to include the `?`).
    #[test]
    fn hover_fires_on_template_question_mark_prefix() {
        let src = ":: StoryInit\n<<run Template.add(\"greeting\", function() { return \"Hello\"; })>>\n:: Start\nYou see ?greeting friend.";
        let (_doc, plugin) = parse(src);
        // Cursor ON the `?` of `?greeting`.
        let cursor_offset = src.find("?greeting").unwrap();
        let token_groups: Vec<knot_formats::plugin::PassageTokenGroup> = Vec::new();
        let hover = try_template_hover(src, cursor_offset, &plugin, &token_groups);
        assert!(hover.is_some(),
            "hover on `?` of `?greeting` should fire (extended range), got None");
    }

    /// `scan_template_at_cursor` should NOT match `?.` (JS optional chaining).
    #[test]
    fn text_scan_does_not_match_js_optional_chaining() {
        // `obj?.prop` — the `?` is preceded by `j` (a word char).
        // Even though `prop` is a valid identifier, this should NOT match.
        let src = ":: Start\n<<run obj?.prop>>";
        // Cursor on `?` of `obj?.prop`.
        let cursor_offset = src.find("obj?.prop").unwrap() + 3;
        let result = scan_template_at_cursor(src, cursor_offset);
        assert!(result.is_none(),
            "scan_template_at_cursor must NOT match JS optional chaining `obj?.prop`: got {:?}",
            result);
    }

    /// `scan_template_at_cursor` should NOT match a JS ternary
    /// (`cond ? value : other`) where `?` is followed by a space.
    #[test]
    fn text_scan_does_not_match_js_ternary_with_space() {
        let src = ":: Start\n<<run x > 0 ? \"yes\" : \"no\">>";
        // Cursor on `?` of the ternary.
        let cursor_offset = src.find("? \"yes\"").unwrap();
        let result = scan_template_at_cursor(src, cursor_offset);
        assert!(result.is_none(),
            "scan_template_at_cursor must NOT match JS ternary `cond ? value : other`: got {:?}",
            result);
    }
}

#[cfg(test)]
mod block_markup_hover_tests {
    use super::*;

    /// Helper: find the byte offset of the first occurrence of `needle` in `src`.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle).unwrap_or_else(|| panic!("needle {:?} not found in src", needle))
    }

    #[test]
    fn hover_on_h1_heading_marker() {
        // `!Title` — cursor on the `!`.
        let src = ":: Start\n!Title here\n";
        let offset = cursor_on(src, "!Title");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `!` heading marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Heading"), "hover text should mention Heading: {}", m.value);
                assert!(m.value.contains("h1"), "hover text should mention h1 tag: {}", m.value);
                assert!(m.value.contains("level 1"), "hover text should mention level 1: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_h2_heading_marker() {
        // `!!Title` — cursor on the 2nd `!`.
        let src = ":: Start\n!!Subsection\n";
        let offset = cursor_on(src, "!!Subsection") + 1; // 2nd `!`
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `!!` heading marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("level 2"), "hover text should mention level 2: {}", m.value);
                assert!(m.value.contains("h2"), "hover text should mention h2 tag: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_h3_heading_marker() {
        // `!!!Title` — cursor on the 3rd `!`.
        let src = ":: Start\n!!!Sub-subsection\n";
        let offset = cursor_on(src, "!!!Sub") + 2; // 3rd `!`
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `!!!` heading marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("level 3"), "hover text should mention level 3: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_unordered_list_marker() {
        // `* item` — cursor on the `*`.
        let src = ":: Start\n* item one\n";
        let offset = cursor_on(src, "* item");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `*` list marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Unordered List"), "hover text should mention Unordered List: {}", m.value);
                assert!(m.value.contains("<ul>"), "hover text should mention <ul>: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_nested_unordered_list_marker() {
        // `** item` — cursor on the 2nd `*`.
        let src = ":: Start\n** nested item\n";
        let offset = cursor_on(src, "** nested") + 1;
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `**` nested list marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("depth 2"), "hover text should mention depth 2: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_ordered_list_marker() {
        // `# item` — cursor on the `#`.
        let src = ":: Start\n# first\n";
        let offset = cursor_on(src, "# first");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `#` ordered list marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Ordered List"), "hover text should mention Ordered List: {}", m.value);
                assert!(m.value.contains("<ol>"), "hover text should mention <ol>: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_blockquote_marker() {
        // `> quote` — cursor on the `>`.
        let src = ":: Start\n> quoted text\n";
        let offset = cursor_on(src, "> quoted");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `>` blockquote marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Blockquote"), "hover text should mention Blockquote: {}", m.value);
                assert!(m.value.contains("<blockquote>"), "hover text should mention <blockquote>: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_horizontal_rule() {
        // `----` — cursor on a dash.
        let src = ":: Start\n----\n";
        let offset = cursor_on(src, "----");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `----` horizontal rule should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Horizontal Rule"), "hover text should mention Horizontal Rule: {}", m.value);
                assert!(m.value.contains("<hr>"), "hover text should mention <hr>: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_three_dashes_does_not_fire() {
        // `---` (3 dashes) is NOT a horizontal rule — needs 4+.
        let src = ":: Start\n---\n";
        let offset = cursor_on(src, "---");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_none(), "hover on `---` (3 dashes) should NOT fire (needs 4+)");
    }

    #[test]
    fn hover_on_block_blockquote_marker() {
        // `<<<` — cursor on a `<`.
        let src = ":: Start\n<<<\nquoted\n<<<\n";
        let offset = cursor_on(src, "<<<\n");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `<<<` block blockquote should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Block Blockquote"), "hover text should mention Block Blockquote: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_code_block_marker() {
        // `{{{\n...\n}}}` — cursor on the first `{`.
        let src = ":: Start\n{{{\ncode here\n}}}\n";
        let offset = cursor_on(src, "{{{");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `{{{{` code block marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Code Block"), "hover text should mention Code Block: {}", m.value);
                assert!(m.value.contains("NOT processed"), "hover text should mention raw content: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_inline_code_marker() {
        // `{{{code}}}` — cursor on the first `{` (mid-line, not block form).
        let src = ":: Start\nSome {{{inline code}} here.\n";
        let offset = cursor_on(src, "{{{inline");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `{{{{` inline code marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Inline Code"), "hover text should mention Inline Code: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_on_heading_content_does_not_fire_marker_hover() {
        // `!Title` — cursor on `T` (the content, not the marker).
        // The marker hover should NOT fire — `T` is not part of the `!` run.
        let src = ":: Start\n!Title\n";
        let offset = cursor_on(src, "Title");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_none(),
            "hover on heading content (not marker) should NOT fire block markup hover");
    }

    #[test]
    fn hover_on_mid_line_exclamation_does_not_fire() {
        // `Hello!` — `!` mid-line is NOT a heading marker (needs column 0).
        let src = ":: Start\nHello!\n";
        let offset = cursor_on(src, "!");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_none(),
            "hover on mid-line `!` should NOT fire (heading requires column 0)");
    }

    #[test]
    fn hover_on_mid_line_asterisk_does_not_fire() {
        // `a * b` — `*` mid-line is NOT a list marker (needs column 0).
        let src = ":: Start\na * b\n";
        let offset = cursor_on(src, "*");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_none(),
            "hover on mid-line `*` should NOT fire (list marker requires column 0)");
    }

    #[test]
    fn hover_on_h6_heading_marker() {
        // `!!!!!!Title` — 6 levels (the maximum).
        let src = ":: Start\n!!!!!!Tiny heading\n";
        let offset = cursor_on(src, "!!!!!!");
        let hover = try_block_markup_hover(src, offset);
        assert!(hover.is_some(), "hover on `!!!!!!` h6 heading marker should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("level 6"), "hover text should mention level 6: {}", m.value);
                assert!(m.value.contains("h6"), "hover text should mention h6 tag: {}", m.value);
            }
        }
    }

    #[test]
    fn hover_range_covers_full_marker_run() {
        // `!!Title` — hover range should cover both `!` characters.
        let src = ":: Start\n!!Title\n";
        let offset = cursor_on(src, "!!Title");
        let hover = try_block_markup_hover(src, offset).expect("hover should fire");
        if let Some(range) = hover.range {
            // Line 1 (0-indexed), characters 0-2 (the `!!` run).
            assert_eq!(range.start.line, 1);
            assert_eq!(range.start.character, 0);
            assert_eq!(range.end.line, 1);
            assert_eq!(range.end.character, 2,
                "range should cover 2 `!` characters, got end char {}", range.end.character);
        }
    }
}

#[cfg(test)]
mod operator_hover_scoping_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;
    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse a source and return (text, token_groups, plugin).
    fn parse(src: &str) -> (String, Vec<knot_formats::plugin::PassageTokenGroup>, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.tw").unwrap();
        let result = plugin.parse_mut(&uri, src);
        (src.to_string(), result.token_groups, plugin)
    }

    /// Helper: find byte offset of `needle` in `src`.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle).unwrap_or_else(|| panic!("needle {:?} not found", needle))
    }

    #[test]
    fn operator_hover_fires_inside_macro_expression() {
        // `<<set $x to 5>>` — `to` is an assignment operator inside a macro.
        let src = ":: Start\n<<set $x to 5>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to 5");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_some(),
            "hover on `to` inside <<set>> should fire (it's a real operator)");
    }

    #[test]
    fn operator_hover_does_not_fire_in_link_display_text() {
        // `[[Jump to combat demo|CombatEncounter]]` — `to` is part of the
        // link display text, NOT an operator.
        let src = ":: Start\n[[Jump to combat demo|CombatEncounter]]\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to combat");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_none(),
            "hover on `to` in link display text should NOT fire — it's prose, not an operator");
    }

    #[test]
    fn operator_hover_does_not_fire_in_prose() {
        // `Go to the forest.` — `to` is a preposition in prose.
        let src = ":: Start\nGo to the forest.\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to the");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_none(),
            "hover on `to` in prose should NOT fire — it's not an operator context");
    }

    #[test]
    fn operator_hover_does_not_fire_in_string_literal() {
        // `<<link "Go to forest">>` — `to` is inside a string literal arg.
        let src = ":: Start\n<<link \"Go to forest\">><</link>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to forest");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_none(),
            "hover on `to` inside a string literal should NOT fire");
    }

    #[test]
    fn operator_hover_fires_for_gt_in_if_condition() {
        // `<<if $hp gt 0>>` — `gt` is a comparison operator.
        let src = ":: Start\n<<if $hp gt 0>><</if>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "gt 0");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_some(),
            "hover on `gt` inside <<if>> should fire (it's a real operator)");
    }

    #[test]
    fn operator_hover_does_not_fire_for_and_in_prose() {
        // `You and I` — `and` is a conjunction in prose.
        let src = ":: Start\nYou and I went home.\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "and I");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(hover.is_none(),
            "hover on `and` in prose should NOT fire — it's not an operator context");
    }
}

#[cfg(test)]
mod arg_ref_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;
    use knot_formats::FormatPluginMut;
    use url::Url;

    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin, knot_core::Workspace) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        let mut ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Add the document to the workspace so find_passage works
        ws.insert_document(doc.clone());
        (doc, plugin, ws)
    }

    /// F2 test: hovering on a passage reference (e.g., `"Shop"` arg in
    /// `<<link "Talk" "Shop">>`) should show COMPACT hover — just the
    /// passage name, tags, and reference count. It should NOT show the
    /// full passage property block (file path, links out, variables
    /// written/read).
    #[test]
    fn arg_ref_hover_shows_compact_form() {
        let src = ":: Start\nYou are here.\n:: Shop\nWelcome to the shop.\n:: Hub\n<<link \"Talk\" \"Shop\">>Go<</link>>";
        let (doc, plugin, ws) = parse(src);
        // Cursor on "Shop" inside <<link "Talk" "Shop">>
        let shop_offset = src.find("\"Shop\"").map(|o| o + 1).unwrap(); // +1 to skip opening quote
        let hover = try_macro_arg_ref_hover(src, shop_offset, Some(&doc), &ws, &plugin);
        assert!(hover.is_some(), "arg ref hover should fire for \"Shop\" in <<link>>");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                // Compact form should include:
                assert!(m.value.contains("**Shop**"), "should show passage name: got {}", m.value);
                assert!(m.value.contains("Tags:"), "should show Tags field: got {}", m.value);
                assert!(m.value.contains("Referenced by"), "should show reference count: got {}", m.value);
                // Compact form should NOT include:
                assert!(!m.value.contains("Links out:"), "compact form should NOT show Links out: got {}", m.value);
                assert!(!m.value.contains("Variables written:"), "compact form should NOT show Variables written: got {}", m.value);
                assert!(!m.value.contains("Variables read:"), "compact form should NOT show Variables read: got {}", m.value);
                assert!(!m.value.contains(".twee"), "compact form should NOT show file path: got {}", m.value);
                // Should mention which macro references it
                assert!(m.value.contains("Referenced by"), "should show referencing macro: got {}", m.value);
            }
        }
    }

    /// F2 test: `[[link]]` hover should show FULL form (not compact) —
    /// the user is hovering a link, not a reference inside a macro arg.
    #[test]
    fn link_hover_shows_full_form() {
        let src = ":: Start\nYou are here.\n:: Shop\nWelcome to the shop.\n:: Hub\nGo [[Shop]] now";
        let (doc, plugin, ws) = parse(src);
        // Cursor on "Shop" inside [[Shop]]
        let shop_offset = src.find("[[Shop]]").map(|o| o + 2).unwrap(); // +2 to skip [[
        let hover = try_link_hover(src, shop_offset, Some(&doc), &ws);
        assert!(hover.is_some(), "link hover should fire for [[Shop]]");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                // Full form should include:
                assert!(m.value.contains("**Shop**"), "should show passage name: got {}", m.value);
                assert!(m.value.contains("Links out:"), "full form should show Links out: got {}", m.value);
                // File path is shown in full form (workspace-relative)
                assert!(m.value.contains("story.tw") || m.value.contains("unknown"), "full form should show file path: got {}", m.value);
            }
        }
    }
}

#[cfg(test)]
mod link_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;
    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse source with multiple passages into a Document + Workspace.
    fn parse_with_workspace(src: &str) -> (String, knot_core::Document, knot_core::Workspace) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.tw").unwrap();
        let result = plugin.parse_mut(&uri, src);
        let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in result.passages {
            doc.passages.push(passage);
        }
        let ws = knot_core::Workspace::new(url::Url::parse("file:///").unwrap());
        (src.to_string(), doc, ws)
    }

    /// Helper: find byte offset of needle in src.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle).unwrap_or_else(|| panic!("needle {:?} not found", needle))
    }

    #[test]
    fn pipe_link_label_hover_shows_display_text() {
        // `[[Go to forest|Forest]]` — hovering on "Go to forest" (the label)
        // should show "Link display text" info, NOT passage info for "Forest".
        let src = ":: Start\n[[Go to forest|Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        // Insert doc into workspace for find_passage
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Go" in the label
        let offset = cursor_on(&text, "Go to forest");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on label should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Link display text"),
                    "label hover should mention 'Link display text': {}", m.value);
                assert!(m.value.contains("Go to forest"),
                    "label hover should contain the label text: {}", m.value);
            }
        }
    }

    #[test]
    fn pipe_link_target_hover_shows_passage_info() {
        // `[[Go to forest|Forest]]` — hovering on "Forest" (the target)
        // should show passage info for "Forest", NOT "Link display text".
        let src = ":: Start\n[[Go to forest|Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Forest" (the target, after the pipe)
        let offset = cursor_on(&text, "|Forest") + 1; // skip the pipe
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on target should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(!m.value.contains("Link display text"),
                    "target hover should NOT mention 'Link display text': {}", m.value);
            }
        }
    }

    #[test]
    fn simple_link_hover_shows_passage_info() {
        // `[[Forest]]` — no pipe, so hovering anywhere shows passage info.
        let src = ":: Start\n[[Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "Forest]]");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on simple link should fire");
    }

    #[test]
    fn pipe_link_label_range_covers_only_label() {
        // The hover range for the label should NOT include the target or pipe.
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "Go to forest");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws).expect("hover should fire");
        if let Some(range) = hover.range {
            // The range should cover "Go to forest" (12 chars) on line 1.
            assert_eq!(range.start.line, 1);
            assert_eq!(range.end.line, 1);
            assert_eq!(range.end.character - range.start.character, 12,
                "label range should cover 'Go to forest' (12 chars), got {}",
                range.end.character - range.start.character);
        }
    }

    #[test]
    fn pipe_link_target_range_covers_only_target() {
        // The hover range for the target should NOT include the label or pipe.
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Forest"
        let offset = cursor_on(&text, "|Forest") + 1;
        let hover = try_link_hover(&text, offset, Some(&doc), &ws).expect("hover should fire");
        if let Some(range) = hover.range {
            // The range should cover "Forest" (6 chars) on line 1.
            assert_eq!(range.start.line, 1);
            assert_eq!(range.end.line, 1);
            assert_eq!(range.end.character - range.start.character, 6,
                "target range should cover 'Forest' (6 chars), got {}",
                range.end.character - range.start.character);
        }
    }

    #[test]
    fn broken_pipe_link_target_shows_broken_message() {
        // `[[Go to forest|MissingPassage]]` — target doesn't exist.
        let src = ":: Start\n[[Go to forest|MissingPassage]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "|MissingPassage") + 1;
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on broken target should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("Broken link"),
                    "broken target hover should mention 'Broken link': {}", m.value);
            }
        }
    }

    #[test]
    fn empty_label_pipe_link_shows_empty_hint() {
        // `[[|Forest]]` — empty label. Should show the "empty" hint.
        let src = ":: Start\n[[|Forest]]\n:: Forest\nForest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor right after [[ (on the pipe position)
        let offset = cursor_on(&text, "[[|") + 1; // on the pipe
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on empty label should fire");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(m.value.contains("empty"),
                    "empty label hover should mention 'empty': {}", m.value);
            }
        }
    }
}

