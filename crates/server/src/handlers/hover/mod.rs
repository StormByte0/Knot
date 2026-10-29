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

// ===========================================================================
// Module split (Fix 5b-1, plan §4.5) — pure move of the former 4,561-line
// hover.rs monolith into per-family modules. Every fn kept its exact
// signature and body; cross-family entry points are pub(super).
// ===========================================================================

mod close_tag;
mod function;
mod global;
mod link;
mod macro_arg;
mod macros;
mod markup;
mod operator;
mod passage;
mod property;
mod template;
mod variable;

// Re-export the family entry points into this module's namespace so the
// dispatcher below (and the test modules via `use super::*`) see the same
// names the monolith exposed.
use close_tag::*;
use function::*;
use global::*;
use link::*;
use macro_arg::*;
use macros::*;
use markup::*;
use operator::*;
use passage::*;
use property::*;
use template::*;
use variable::*;

#[cfg(test)]
mod tests;

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
    //
    // M3: the cache is now HashMap<String, PassageTokenGroup> keyed by
    // passage name. The hover helpers below expect a &[PassageTokenGroup]
    // in source order, so we extract the values and sort by passage_offset.
    let token_groups_map = inner.semantic_tokens.get(&uri).cloned().unwrap_or_default();
    let mut token_groups: Vec<knot_formats::plugin::PassageTokenGroup> =
        token_groups_map.into_values().collect();
    token_groups.sort_by_key(|g| g.passage_offset);

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
    if let Some(plugin) = plugin
        && let Some(fmt_hover) =
            plugin.provide_hover(text, &inner.workspace, &uri, byte_offset, &token_groups)
    {
        let range = fmt_hover
            .range
            .map(|r| helpers::byte_range_to_lsp_range(text, &r));
        info!(
            "hover: provide_hover returned Some ({} chars)",
            fmt_hover.contents.len()
        );
        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: fmt_hover.contents,
            }),
            range,
        }));
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
    if let Some(passage) = current_passage
        && let Some(hover) = try_passage_header_hover(text, byte_offset, passage, &inner.workspace)
    {
        return Ok(Some(hover));
    }
    // Fallback: if span-based passage lookup didn't find a passage (e.g., the
    // cursor is on a header line but the passage span hasn't been updated yet,
    // or the format only spans the header line), try the line-based check.
    if current_passage.is_none()
        && let Some(passage_name) =
            helpers::find_passage_at_position_span_based(text, &inner.workspace, &uri, position)
        && let Some((_, passage)) = inner.workspace.find_passage(&passage_name)
        && let Some(hover) = try_passage_header_hover(text, byte_offset, passage, &inner.workspace)
    {
        return Ok(Some(hover));
    }

    // 2. Try macro arg ref hover (inner layer) — if the cursor is on a
    //    PassageRef arg inside a macro, show passage info for the target.
    //    This takes priority over the outer macro hover (step 5).
    if let Some(plugin) = plugin {
        if let Some(hover) =
            try_macro_arg_ref_hover(text, byte_offset, doc, &inner.workspace, plugin)
        {
            return Ok(Some(hover));
        }
    } else {
        // No plugin available — fall back to workspace-only hover (no macro label).
        // This path is rarely hit; preserving previous behavior for safety.
        #[allow(deprecated)]
        if let Some(hover) =
            try_macro_arg_ref_hover_no_plugin(text, byte_offset, doc, &inner.workspace)
        {
            return Ok(Some(hover));
        }
    }

    // 3. Try variable hover — use span data from the workspace index.
    if let Some(plugin) = plugin
        && let Some(hover) = try_variable_hover(
            text,
            byte_offset,
            doc,
            &inner.workspace,
            plugin,
            &token_groups,
        )
    {
        return Ok(Some(hover));
    }

    // 3b. Try operator hover — cursor on a SugarCube operator like `gt`,
    //     `to`, `eq`, `and`. Shows a plain-English description so users
    //     can model their story logic without memorizing the operator names.
    //
    //     SCOPE: Only fires when an Operator semantic token exists at the
    //     cursor — operators are valid only inside macro expressions, not
    //     in prose or link text. This prevents `to` in "Jump to combat"
    //     from triggering an operator hover.
    if let Some(plugin) = plugin
        && let Some(hover) = try_operator_hover(text, byte_offset, plugin, &token_groups)
    {
        return Ok(Some(hover));
    }

    // 4. Try link hover — use span data from the workspace index.
    if let Some(hover) = try_link_hover(text, byte_offset, doc, &inner.workspace) {
        return Ok(Some(hover));
    }

    // 4b. Try template hover — cursor on `?name` in prose. Uses the same
    //     `Function` semantic tokens as function hover (the token builder
    //     emits Function tokens for `?name` patterns). Filters to known
    //     templates via `plugin.find_template()`.
    if let Some(plugin) = plugin
        && let Some(hover) = try_template_hover(text, byte_offset, plugin, &token_groups)
    {
        return Ok(Some(hover));
    }

    // 4c. Try function hover — cursor on a JS function call (e.g., `myFunc()`
    //     inside `<<run>>`). Uses semantic tokens for detection. Only fires
    //     when the function has meaningful info (definition location + params).
    if let Some(plugin) = plugin
        && let Some(hover) = try_function_hover(text, byte_offset, plugin, &token_groups)
    {
        return Ok(Some(hover));
    }

    // 4c. Try property hover — cursor on `.prop` in `$var.prop`. Only fires
    //     when there are siblings to discover (the value of property hover is
    //     seeing what other properties exist on the parent object).
    if let Some(plugin) = plugin
        && let Some(hover) = try_property_hover(text, byte_offset, plugin, &token_groups, doc)
    {
        return Ok(Some(hover));
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
    if let Some(plugin) = plugin
        && let Some(hover) = try_close_tag_hover(text, byte_offset, current_passage, plugin)
    {
        return Ok(Some(hover));
    }

    // 5b. Try macro hover — fires only when cursor is ON the macro name.
    if let Some(plugin) = plugin
        && let Some(hover) = try_macro_hover(text, byte_offset, doc, plugin)
    {
        return Ok(Some(hover));
    }

    // 5c. Try block-level markup hover — cursor on `!`, `*`, `#`, `>`,
    //     `----`, `<<<`, or `{{{` markers. These produce semantic tokens
    //     (Heading, ListMarker, Blockquote, etc.) but had no hover handler,
    //     so hovering over them did nothing. This is a line-based scan that
    //     fires when the cursor is on the marker run at column 0.
    if let Some(hover) = try_block_markup_hover(text, byte_offset, current_passage) {
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
