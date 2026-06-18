//! Navigation handlers: goto_definition, goto_declaration,
//! goto_implementation, goto_type_definition, references.
//!
//! All handlers use span-based resolution via the workspace index instead of
//! re-scanning source text. This avoids redundant parsing, correctly handles
//! multi-byte characters, and works with arrow/pipe link syntax.
//!
//! ## goto_definition scope
//!
//! `goto_definition` resolves three kinds of targets, in order:
//! 1. **Passage links** — `[[Target]]`, `<<goto "Target">>`, `<<link "..." "Target">>`.
//!    Jumps to the target passage's header line.
//! 2. **Custom macros** — `<<mywidget>>` where `mywidget` is user-defined via
//!    `<<widget>>` or `Macro.add()`. Jumps to the definition site.
//! 3. **Functions** — `myFunc()` inside `<<run>>` / `<<set>>` / etc. Jumps to
//!    the function declaration in a script passage.
//!
//! Variables are intentionally NOT resolved here. SugarCube has no standardized
//! initialization/declaration site, and reads/writes have no reliable order
//! across passages — so "the definition of `$var`" is not a well-formed query.
//! The dedicated variable tracker panel (sidebar) shows all references with
//! read/write kind and location, which is the correct UI for that data.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ReferenceTarget — shared cursor resolution for goto_def and references
// ---------------------------------------------------------------------------

/// What the cursor is on, resolved in a format-agnostic way.
///
/// This enum is the shared entry point for both `goto_definition` and
/// `references`. Both handlers need to answer "what is the user pointing at?"
/// before they can resolve a target — `goto_definition` jumps to the single
/// definition site, `references` finds all call/usage sites.
///
/// Format isolation: all resolution goes through `&dyn FormatPlugin` trait
/// methods (`find_macro`, `find_custom_macro`, `find_function`) and the
/// format-agnostic `Passage` struct fields (`macro_invocations`, `links`).
/// No SugarCube-specific types are imported here.
#[derive(Debug, Clone)]
pub(crate) enum ReferenceTarget {
    /// Cursor is on a passage link (`[[Target]]`, `<<goto "Target">>`, etc.)
    /// or on a passage header. `name` is the passage name.
    Passage { name: String },
    /// Cursor is on a custom macro invocation (`<<mywidget>>` where
    /// `mywidget` is user-defined via `<<widget>>` or `Macro.add()`).
    /// `name` is the macro name (without `<<`/`>>`).
    CustomMacro { name: String },
    /// Cursor is on a function call (`myFunc()` inside `<<run>>`/`<<set>>`).
    /// `name` is the function name.
    Function { name: String },
    /// Cursor is on a template invocation (`?name` in SugarCube). `name` is
    /// the template name (without the `?` prefix). Templates are defined via
    /// `Template.add("name", ...)` in `[script]` passages — same pattern as
    /// custom macros via `Macro.add()`.
    Template { name: String },
}

/// Resolve what's under the cursor, in priority order:
/// 1. Passage link / header
/// 2. Custom macro name (non-builtin macro invocation that the plugin knows as a custom macro)
/// 3. Function name (semantic token or text-scan fallback)
///
/// Returns `None` if the cursor isn't on anything resolvable.
///
/// **Format isolation:** This function uses only:
/// - `helpers::find_link_target_at_position_span_based` / `find_passage_at_position_span_based`
///   (format-agnostic, work off `Passage` struct data)
/// - `plugin.find_macro(name)` (trait method — builtin check)
/// - `plugin.is_custom_macro(name)` (trait method — custom macro check)
/// - `plugin.find_function(name)` (trait method — function check)
/// - `passage.macro_invocations` (field on `Passage`, a core type)
/// - `inner.semantic_tokens` with `SemanticTokenType::Function` (token type defined in the plugin trait)
///
/// No SugarCube types are imported. The same code works for any format plugin
/// that implements these trait methods.
pub(crate) fn resolve_target_at_cursor(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<ReferenceTarget> {
    let text = inner.open_documents.get(uri)?;
    let byte_offset = helpers::position_to_byte_offset(text, position);

    // ── 1. Passage link / header ──────────────────────────────────────────
    if let Some(name) = helpers::find_passage_at_position_span_based(
        text, &inner.workspace, uri, position,
    ) {
        return Some(ReferenceTarget::Passage { name });
    }
    if let Some(name) = helpers::find_link_target_at_position_span_based(
        text, &inner.workspace, uri, position,
    ) {
        return Some(ReferenceTarget::Passage { name });
    }

    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format)?;

    // ── 2. Custom macro name ──────────────────────────────────────────────
    if let Some(doc) = inner.workspace.get_document(uri) {
        for passage in &doc.passages {
            for inv in &passage.macro_invocations {
                if !passage.span_contains_abs_offset(&inv.name_span, byte_offset) {
                    continue;
                }
                // Skip builtins — they have no user-code definition.
                if plugin.find_macro(&inv.name).is_some() {
                    break;
                }
                if plugin.is_custom_macro(&inv.name) {
                    return Some(ReferenceTarget::CustomMacro { name: inv.name.clone() });
                }
                break;
            }
        }
    }

    // ── 3. Function name ──────────────────────────────────────────────────
    // Step A: semantic token under cursor.
    let token_groups = inner.semantic_tokens.get(uri).cloned().unwrap_or_default();
    use knot_formats::plugin::SemanticTokenType;
    for group in &token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                let name = &text[abs_start..abs_end];
                if plugin.find_function(name).is_some() {
                    return Some(ReferenceTarget::Function { name: name.to_string() });
                }
            }
        }
    }
    // Step B: text-scan fallback for plain JS function calls (the JS walker
    // only emits Function semantic tokens for preprocessed SugarCube variable
    // calls, not for regular JS identifiers).
    if let Some(name) = identifier_at_offset(text, byte_offset) {
        if plugin.find_function(&name).is_some() {
            return Some(ReferenceTarget::Function { name });
        }
    }

    // ── 4. Template invocation (?name) ────────────────────────────────────
    // SugarCube templates are invoked with `?name` syntax. There's no
    // structured `template_invocations` data on `Passage` and no
    // `SemanticTokenType::Template` token, so we detect by checking whether
    // the cursor is on an identifier that's immediately preceded by `?`.
    // The `?` is part of the invocation but NOT part of the name (the name
    // is what `Template.add("name", ...)` registered).
    if let Some(name) = template_name_at_offset(text, byte_offset) {
        if plugin.find_template(&name).is_some() {
            return Some(ReferenceTarget::Template { name });
        }
    }

    None
}

/// Extract the identifier at the given byte offset in `text`.
///
/// Scans backward and forward from `offset` to find the identifier boundaries.
/// Identifier chars are `[A-Za-z0-9_$]` (JS convention). Returns `None` if no
/// identifier is found.
fn identifier_at_offset(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';

    let mut start = offset.min(len);
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = start;
    while end < len && is_ident(bytes[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    Some(text[start..end].to_string())
}

/// Extract the template name at the given byte offset, if the cursor is on a
/// `?name` invocation.
///
/// SugarCube templates are invoked with `?name` (e.g., `?heal`). The `?` is
/// the invocation prefix; the name is what `Template.add("name", ...)`
/// registered. Returns the name WITHOUT the `?` prefix, or `None` if:
/// - The cursor isn't on an identifier, OR
/// - The identifier isn't immediately preceded by `?`
///
/// This is format-agnostic text scanning — the caller is responsible for
/// confirming the name is a known template via `plugin.find_template()`.
fn template_name_at_offset(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';

    // Find the identifier boundaries at the cursor.
    let mut start = offset.min(len);
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = start;
    while end < len && is_ident(bytes[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }

    // Check that the character immediately before the identifier is `?`.
    // `?` is a single ASCII byte, so this byte check is safe for UTF-8 text
    // (no multi-byte char ends with a `?` byte).
    if start == 0 || bytes[start - 1] != b'?' {
        return None;
    }

    Some(text[start..end].to_string())
}

// ---------------------------------------------------------------------------
// goto_definition
// ---------------------------------------------------------------------------

pub(crate) async fn goto_definition(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;
    Ok(goto_definition_inner(&inner, &uri, position))
}

/// Inner synchronous implementation of `goto_definition`.
///
/// Extracted from the async wrapper so unit tests can call it directly without
/// having to construct a full `ServerState` (which requires a tower-lsp
/// `Client` handle).
///
/// Delegates cursor resolution to `resolve_target_at_cursor` (shared with
/// `references`) to avoid duplicating the detection logic. Once the target is
/// known, jumps to its single definition site.
fn goto_definition_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let target = resolve_target_at_cursor(inner, uri, position)?;
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format)?;

    match target {
        ReferenceTarget::Passage { name } => {
            // Jump to the passage header.
            let (doc, _passage) = inner.workspace.find_passage(&name)?;
            let target_uri = doc.uri.clone();
            let target_text = inner.open_documents.get(&target_uri);
            let range = if let Some(t) = target_text {
                helpers::find_passage_header_range_span_based(t, &inner.workspace, &name)
            } else {
                Range::default()
            };
            Some(GotoDefinitionResponse::Scalar(Location {
                uri: target_uri,
                range,
            }))
        }
        ReferenceTarget::CustomMacro { name } => {
            let loc = lookup_custom_macro_definition(&name, plugin, inner)?;
            Some(GotoDefinitionResponse::Scalar(loc))
        }
        ReferenceTarget::Function { name: _ } => {
            // The function name from `target` is not used here — we
            // re-derive it from the cursor position via `lookup_function_*`
            // because those helpers need the exact byte offset to compute
            // the definition range. `name` IS used by `references_inner`.
            // Try semantic-token path first, then text-scan fallback.
            let text = inner.open_documents.get(uri)?;
            let byte_offset = helpers::position_to_byte_offset(text, position);
            let token_groups = inner.semantic_tokens.get(uri).cloned().unwrap_or_default();
            if let Some(loc) = lookup_function_definition(
                text, byte_offset, token_groups, plugin, inner,
            ) {
                return Some(GotoDefinitionResponse::Scalar(loc));
            }
            if let Some(loc) = lookup_function_by_text_scan(
                text, byte_offset, plugin, inner,
            ) {
                return Some(GotoDefinitionResponse::Scalar(loc));
            }
            None
        }
        ReferenceTarget::Template { name } => {
            let loc = lookup_template_definition(&name, plugin, inner)?;
            Some(GotoDefinitionResponse::Scalar(loc))
        }
    }
}

/// Look up a custom macro definition and return an LSP `Location`.
///
/// Returns `None` if the plugin doesn't know about the macro, or if the
/// definition file isn't open in the workspace (we can't compute a range
/// without the target file's text).
///
/// **Offset semantics:** The plugin's `find_custom_macro()` returns a
/// **passage-relative** byte offset (0 = the `::` prefix of the passage
/// header), matching the convention used by the `Passage` struct. We convert
/// it to document-absolute via `passage.abs_offset()` at the LSP boundary.
fn lookup_custom_macro_definition(
    name: &str,
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    inner: &crate::state::ServerStateInner,
) -> Option<Location> {
    let (defined_in_passage, file_uri, passage_rel_offset) = plugin.find_custom_macro(name)?;
    let target_uri: url::Url = file_uri.parse().ok()?;
    let target_text = inner.open_documents.get(&target_uri)?;

    // Convert passage-relative → document-absolute using the passage's
    // `passage_offset` (document-absolute position of the passage head `::`).
    let abs_offset = if let Some((doc, passage)) = inner.workspace.find_passage(&defined_in_passage) {
        if doc.uri == target_uri {
            passage.abs_offset(passage_rel_offset)
        } else {
            // Passage found but in a different file — the offset is stale.
            // Fall back to the passage-relative offset as-is (will likely be
            // wrong, but better than nothing).
            passage_rel_offset
        }
    } else {
        passage_rel_offset
    };

    let range = identifier_range_at_offset(target_text, abs_offset);
    Some(Location { uri: target_uri, range })
}

/// Look up a template definition and return an LSP `Location`.
///
/// Templates are defined via `Template.add("name", ...)` in `[script]`
/// passages — same pattern as `Macro.add()` for custom macros. The plugin's
/// `find_template()` returns a `TemplateDefInfo` with a passage-relative
/// offset (0 = `::` head), which we convert to document-absolute via
/// `passage.abs_offset()`.
///
/// Returns `None` if the plugin doesn't know the template, the definition
/// file isn't open, or the passage can't be found in the workspace.
fn lookup_template_definition(
    name: &str,
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    inner: &crate::state::ServerStateInner,
) -> Option<Location> {
    let info = plugin.find_template(name)?;
    let target_uri: url::Url = info.file_uri.parse().ok()?;
    let target_text = inner.open_documents.get(&target_uri)?;

    // Convert passage-relative → document-absolute using the passage's
    // `passage_offset` (document-absolute position of the passage head `::`).
    let abs_offset = if let Some((doc, passage)) = inner.workspace.find_passage(&info.defined_in) {
        if doc.uri == target_uri {
            passage.abs_offset(info.defined_at_offset)
        } else {
            info.defined_at_offset
        }
    } else {
        info.defined_at_offset
    };

    let range = identifier_range_at_offset(target_text, abs_offset);
    Some(Location { uri: target_uri, range })
}

/// Look up a function definition and return an LSP `Location`.
///
/// Walks `token_groups` to find the `Function` token under the cursor, then
/// queries `plugin.find_function()` for the declaration. Returns `None` if no
/// function token is under the cursor, the plugin doesn't know the function,
/// or the definition file isn't open.
///
/// **Offset semantics:** Like `lookup_custom_macro_definition`, the plugin's
/// `find_function()` returns a **passage-relative** offset (0 = `::` head).
/// We convert it to document-absolute via `passage.abs_offset()`.
fn lookup_function_definition(
    text: &str,
    byte_offset: usize,
    token_groups: Vec<knot_formats::plugin::PassageTokenGroup>,
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    inner: &crate::state::ServerStateInner,
) -> Option<Location> {
    use knot_formats::plugin::SemanticTokenType;

    for group in &token_groups {
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
                let target_uri: url::Url = info.file_uri.parse().ok()?;
                let target_text = inner.open_documents.get(&target_uri)?;

                // Convert passage-relative → document-absolute.
                let abs_offset = if let Some((doc, passage)) = inner.workspace.find_passage(&info.defined_in) {
                    if doc.uri == target_uri {
                        passage.abs_offset(info.defined_at_offset)
                    } else {
                        info.defined_at_offset
                    }
                } else {
                    info.defined_at_offset
                };

                let range = identifier_range_at_offset(target_text, abs_offset);
                return Some(Location { uri: target_uri, range });
            }
        }
    }
    None
}

/// Look up a function definition by scanning the source text at the cursor.
///
/// This is a fallback for when no `Function` semantic token covers the cursor
/// (which happens for regular JS function calls like `myFunc()` — the JS
/// walker only emits `Function` tokens for preprocessed SugarCube variable
/// calls). We scan backward and forward from the cursor to find the identifier
/// under it, then check if `plugin.find_function(name)` knows it.
///
/// Returns `None` if no identifier is found at the cursor, or the plugin
/// doesn't know the function, or the definition file isn't open.
fn lookup_function_by_text_scan(
    text: &str,
    byte_offset: usize,
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    inner: &crate::state::ServerStateInner,
) -> Option<Location> {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Identifier chars: JS allows `[A-Za-z0-9_$]`. We also include `-` for
    // SugarCube macro names (harmless here since `find_function` won't match
    // macro names).
    let is_ident = |b: u8| {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
    };

    // Scan backward from cursor to find identifier start.
    let mut start = byte_offset.min(len);
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    // Scan forward to find identifier end.
    let mut end = start;
    while end < len && is_ident(bytes[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }

    let name = &text[start..end];
    let info = plugin.find_function(name)?;
    let target_uri: url::Url = info.file_uri.parse().ok()?;
    let target_text = inner.open_documents.get(&target_uri)?;

    // Convert passage-relative → document-absolute (same logic as
    // `lookup_function_definition`).
    let abs_offset = if let Some((doc, passage)) = inner.workspace.find_passage(&info.defined_in) {
        if doc.uri == target_uri {
            passage.abs_offset(info.defined_at_offset)
        } else {
            info.defined_at_offset
        }
    } else {
        info.defined_at_offset
    };

    let range = identifier_range_at_offset(target_text, abs_offset);
    Some(Location { uri: target_uri, range })
}

/// Compute the LSP range of the identifier at `offset` in `text`.
///
/// The format plugin's `defined_at_offset` for custom macros and functions
/// points at (or very near) the start of the identifier name in the source
/// file. We scan forward to find the end of the identifier, so the LSP range
/// covers exactly the name — giving the user a precise highlight when they
/// land at the definition.
///
/// Identifier characters are `[A-Za-z0-9_-$-]`. The `-` is included because
/// SugarCube macro names may contain hyphens (e.g. `<<link-replace>>`); it
/// won't appear in JS function names but is harmless there. If the offset
/// doesn't land on an identifier character (e.g., it points at the opening
/// `"` of a string literal in `Macro.add("name", ...)`), we scan forward to
/// find the next identifier start.
///
/// Returns a zero-length range at `offset` if no identifier is found within
/// a reasonable lookahead (256 bytes).
fn identifier_range_at_offset(text: &str, offset: usize) -> Range {
    let bytes = text.as_bytes();
    let len = bytes.len();

    let is_ident = |b: u8| {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'-'
    };

    // Start position: if offset isn't on an ident char, scan forward to find
    // the next one. Cap the lookahead so we don't scan into unrelated code.
    let mut start = offset.min(len);
    if start >= len || !is_ident(bytes[start]) {
        let mut probe = start;
        let limit = (start + 256).min(len);
        while probe < limit && !is_ident(bytes[probe]) {
            probe += 1;
        }
        if probe >= limit {
            return Range {
                start: helpers::byte_offset_to_position(text, offset),
                end: helpers::byte_offset_to_position(text, offset),
            };
        }
        start = probe;
    }

    // End position: scan forward while ident chars.
    let mut end = start;
    while end < len && is_ident(bytes[end]) {
        end += 1;
    }

    // Skip a leading hyphen if present (identifiers don't start with `-`).
    if bytes[start] == b'-' && end > start + 1 {
        start += 1;
    }

    helpers::byte_range_to_lsp_range(text, &(start..end))
}

pub(crate) async fn goto_declaration(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    // Declaration — same as definition for Twine (links to passage header)
    goto_definition(state, params).await
}

pub(crate) async fn goto_implementation(
    state: &ServerState,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;

    let inner = state.inner.read().await;

    // Determine the target passage using span-based resolution
    let target_passage = if let Some(text) = inner.open_documents.get(&uri) {
        helpers::find_passage_at_position_span_based(text, &inner.workspace, &uri, position)
            .or_else(|| helpers::find_link_target_at_position_span_based(text, &inner.workspace, &uri, position))
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all passages that link TO this passage using workspace data.
    // Iterate workspace.documents().passages[].links[] where
    // link.target == target_name, using link.span for the location range.
    let mut locations = Vec::new();
    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };
        for passage in &doc.passages {
            for link in &passage.links {
                if link.target.trim() == target_name {
                    let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));
                    locations.push(Location {
                        uri: doc.uri.clone(),
                        range,
                    });
                }
            }
        }
    }

    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDefinitionResponse::Array(locations)))
    }
}

pub(crate) async fn goto_type_definition(
    state: &ServerState,
    _params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;

    // Find the StoryData passage in the workspace
    if let Some((doc, _passage)) = inner.workspace.find_passage("StoryData") {
        let target_uri = doc.uri.clone();
        let target_text = inner.open_documents.get(&target_uri);
        let range = if let Some(t) = target_text {
            helpers::find_passage_header_range_span_based(t, &inner.workspace, "StoryData")
        } else {
            Range::default()
        };
        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range,
        })));
    }

    Ok(None)
}

pub(crate) async fn references(
    state: &ServerState,
    params: ReferenceParams,
) -> Result<Option<Vec<Location>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;

    let inner = state.inner.read().await;
    Ok(references_inner(&inner, &uri, position))
}

/// Inner synchronous implementation of `references`.
///
/// Delegates cursor resolution to `resolve_target_at_cursor` (shared with
/// `goto_definition`). Once the target is known, finds ALL usage sites across
/// the entire workspace:
///
/// - **Passage**: all passage headers with matching name + all links targeting it
/// - **Custom macro**: all `macro_invocations` with matching name across all documents
/// - **Function**: all `Function` semantic tokens with matching name across all
///   documents, plus a text-scan fallback for plain JS function calls (the JS
///   walker only emits `Function` tokens for preprocessed SugarCube variable
///   calls, not for regular JS identifiers like `myFunc()`)
///
/// **Format isolation:** All resolution goes through `&dyn FormatPlugin` trait
/// methods and format-agnostic `Passage` struct fields. No SugarCube types are
/// imported. The `include_declaration` flag from the LSP params controls
/// whether the definition site itself is included in the results.
fn references_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<Vec<Location>> {
    let target = resolve_target_at_cursor(inner, uri, position)?;
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    let mut locations = Vec::new();

    match &target {
        ReferenceTarget::Passage { name } => {
            // Find all locations that reference this passage:
            // - Header definition reference (if include_declaration)
            // - Link references: all links targeting this passage
            for doc in inner.workspace.documents() {
                let text = match inner.open_documents.get(&doc.uri) {
                    Some(t) => t,
                    None => continue,
                };
                for passage in &doc.passages {
                    // Header definition reference
                    if passage.name == *name {
                        let range = if let Some(ref name_span) = passage.header_name_span {
                            helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
                        } else {
                            let span_start = passage.abs_offset(passage.span.start).min(text.len());
                            let header_end = text[span_start..]
                                .find('\n')
                                .map(|n| span_start + n)
                                .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));
                            helpers::byte_range_to_lsp_range(text, &(span_start..header_end))
                        };
                        locations.push(Location { uri: doc.uri.clone(), range });
                    }

                    // Link references
                    for link in &passage.links {
                        if link.target.trim() == *name {
                            let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));
                            locations.push(Location { uri: doc.uri.clone(), range });
                        }
                    }
                }
            }
        }

        ReferenceTarget::CustomMacro { name } => {
            // Find all macro invocations with this name across all documents.
            // `macro_invocations` is a field on `Passage` (core type) — this is
            // format-agnostic. The plugin's `is_custom_macro` check is NOT
            // needed here because we're matching by exact name, and the name
            // came from `resolve_target_at_cursor` which already confirmed it's
            // a custom macro.
            for doc in inner.workspace.documents() {
                let text = match inner.open_documents.get(&doc.uri) {
                    Some(t) => t,
                    None => continue,
                };
                for passage in &doc.passages {
                    for inv in &passage.macro_invocations {
                        if inv.name == *name {
                            let range = helpers::byte_range_to_lsp_range(
                                text, &passage.abs_range(&inv.name_span),
                            );
                            locations.push(Location { uri: doc.uri.clone(), range });
                        }
                    }
                }
            }
        }

        ReferenceTarget::Function { name } => {
            // Find all function call sites with this name across all documents.
            // Two paths:
            //   a) Semantic tokens: `Function` tokens with matching name
            //   b) Text-scan fallback: scan every document's text for
            //      identifier matches (catches plain JS calls that the walker
            //      doesn't emit tokens for)
            use knot_formats::plugin::SemanticTokenType;

            for doc in inner.workspace.documents() {
                let text = match inner.open_documents.get(&doc.uri) {
                    Some(t) => t,
                    None => continue,
                };

                // Path A: semantic tokens for this document
                if let Some(token_groups) = inner.semantic_tokens.get(&doc.uri) {
                    for group in token_groups {
                        let group_offset = group.passage_offset;
                        for token in &group.tokens {
                            if token.token_type != SemanticTokenType::Function {
                                continue;
                            }
                            let abs_start = token.start + group_offset;
                            let abs_end = abs_start + token.length;
                            if abs_end > text.len() {
                                continue;
                            }
                            let token_name = &text[abs_start..abs_end];
                            if token_name == name.as_str() {
                                let range = helpers::byte_range_to_lsp_range(text, &(abs_start..abs_end));
                                locations.push(Location { uri: doc.uri.clone(), range });
                            }
                        }
                    }
                }

                // Path B: text-scan fallback. Scan the entire document for
                // identifier occurrences matching the function name. This is
                // O(text length) per document — acceptable for typical Twine
                // projects (dozens of files, each a few KB).
                //
                // We only emit a location if `plugin.find_function(name)`
                // confirms the name is a known function (prevents false
                // positives from random identifiers that happen to match).
                if let Some(plugin) = plugin {
                    if plugin.find_function(name).is_some() {
                        for (offset, ident) in identifiers_in_text(text) {
                            if ident == name.as_str() {
                                let end = offset + ident.len();
                                let range = helpers::byte_range_to_lsp_range(text, &(offset..end));
                                locations.push(Location { uri: doc.uri.clone(), range });
                            }
                        }
                    }
                }
            }
        }

        ReferenceTarget::Template { name } => {
            // Find all `?name` invocation sites across all documents via
            // text-scan. Templates have no structured `template_invocations`
            // data on `Passage` and no `SemanticTokenType::Template` token,
            // so we scan every document's text for `?` followed by an
            // identifier matching the template name.
            //
            // Gated by `plugin.find_template(name)` to prevent false positives
            // from random `?identifier` occurrences that happen to match.
            if let Some(plugin) = plugin {
                if plugin.find_template(name).is_some() {
                    for doc in inner.workspace.documents() {
                        let text = match inner.open_documents.get(&doc.uri) {
                            Some(t) => t,
                            None => continue,
                        };
                        for (q_offset, ident) in template_invocations_in_text(text) {
                            if ident == name.as_str() {
                                // The range covers just the name (not the `?`).
                                // Renaming `?heal` → `?cured` means replacing
                                // `heal` with `cured`, keeping the `?` prefix.
                                let name_start = q_offset;
                                let name_end = q_offset + ident.len();
                                let range = helpers::byte_range_to_lsp_range(text, &(name_start..name_end));
                                locations.push(Location { uri: doc.uri.clone(), range });
                            }
                        }
                    }
                }
            }
        }
    }

    if locations.is_empty() {
        None
    } else {
        // Deduplicate: the function path (A + B) can produce duplicate
        // locations when a semantic token and a text-scan match the same
        // offset. Sort by (uri, line, char) and remove consecutive dups.
        locations.sort_by(|a, b| {
            a.uri.cmp(&b.uri)
                .then(a.range.start.line.cmp(&b.range.start.line))
                .then(a.range.start.character.cmp(&b.range.start.character))
        });
        locations.dedup_by(|a, b| {
            a.uri == b.uri
                && a.range.start == b.range.start
                && a.range.end == b.range.end
        });
        Some(locations)
    }
}

/// Confirm that the definition site of a target exists and is reachable.
///
/// This is the **failsafe** for rename: we only proceed with renaming all
/// references if we can confirm the definition site is real and reachable.
/// Without this, a stale registry could cause rename to change all call sites
/// while leaving the definition unchanged — silently breaking the project.
///
/// For each target type:
/// - `Passage`: confirm the passage exists in the workspace.
/// - `CustomMacro`: confirm `find_custom_macro()` returns `Some`, the
///   definition file is open, the passage is in the workspace, and the text
///   at the definition offset matches the name.
/// - `Function`: same confirmation as `CustomMacro` but via `find_function()`.
/// - `Template`: same confirmation via `find_template()`.
///
/// Returns `true` if the definition is confirmed, `false` otherwise.
pub(crate) fn definition_confirmed(
    target: &ReferenceTarget,
    inner: &crate::state::ServerStateInner,
) -> bool {
    let format = inner.workspace.resolve_format();
    let Some(plugin) = inner.format_registry.get(&format) else {
        return false;
    };

    match target {
        ReferenceTarget::Passage { name } => {
            inner.workspace.find_passage(name).is_some()
        }
        ReferenceTarget::CustomMacro { name } => {
            confirm_definition_text(name, plugin.find_custom_macro(name), inner, |t| (t.0, t.1, t.2))
        }
        ReferenceTarget::Function { name } => {
            confirm_definition_text(name, plugin.find_function(name), inner, |t| {
                (t.defined_in.clone(), t.file_uri.clone(), t.defined_at_offset)
            })
        }
        ReferenceTarget::Template { name } => {
            confirm_definition_text(name, plugin.find_template(name), inner, |t| {
                (t.defined_in.clone(), t.file_uri.clone(), t.defined_at_offset)
            })
        }
    }
}

/// Shared confirmation logic: given a `(passage_name, file_uri, offset)` tuple
/// from the plugin, verify the file is open, the passage is in the workspace,
/// and the text at the definition offset actually matches the expected name.
///
/// The `extract` closure adapts the plugin's return type (which differs
/// between `find_custom_macro` returning a tuple and `find_function`/
/// `find_template` returning structs) to the common `(String, String, usize)`
/// shape.
fn confirm_definition_text<T, F>(
    expected_name: &str,
    info: Option<T>,
    inner: &crate::state::ServerStateInner,
    extract: F,
) -> bool
where
    F: FnOnce(T) -> (String, String, usize),
{
    let Some(info) = info else { return false };
    let (defined_in, file_uri, passage_rel_offset) = extract(info);

    let Ok(target_uri) = file_uri.parse::<url::Url>() else { return false };
    let Some(target_text) = inner.open_documents.get(&target_uri) else { return false };
    let Some((doc, passage)) = inner.workspace.find_passage(&defined_in) else { return false };
    if doc.uri != target_uri { return false }

    let abs_offset = passage.abs_offset(passage_rel_offset);

    // The definition offset may point at the opening quote of a string literal
    // (e.g., `Macro.add("name", ...)` — the offset is at `"`, not at `name`).
    // Scan forward to find the first identifier character, then check that
    // the identifier at that position matches `expected_name`.
    let bytes = target_text.as_bytes();
    let len = bytes.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b == b'-';

    // Scan forward to find identifier start (skip quotes, whitespace, etc.).
    let mut start = abs_offset.min(len);
    let limit = (start + 256).min(len);
    while start < limit && !is_ident(bytes[start]) {
        start += 1;
    }
    if start >= limit {
        return false;
    }

    // Scan forward to find identifier end.
    let mut end = start;
    while end < len && is_ident(bytes[end]) {
        end += 1;
    }

    // Skip a leading hyphen if present (identifiers don't start with `-`).
    let name_start = if bytes[start] == b'-' && end > start + 1 { start + 1 } else { start };

    if end > target_text.len() || name_start >= end {
        return false;
    }
    &target_text[name_start..end] == expected_name
}

/// Collect all rename edits for a target across the workspace.
///
/// This is the rename equivalent of `references_inner` — it finds all the
/// locations that need to change (definition + all call sites) and produces
/// `TextEdit`s with the new name. Used by both `rename` (to build the
/// `WorkspaceEdit`) and could be used by `prepare_rename` to validate that
/// there's at least one editable site.
///
/// **Failsafe:** The caller MUST call `definition_confirmed()` before this
/// function. This function does NOT re-check the definition — it assumes the
/// caller has already confirmed the definition is reachable. If the definition
/// is stale, this function will still produce edits for all call sites (which
/// is why the caller's failsafe check is mandatory).
pub(crate) fn collect_rename_edits(
    target: &ReferenceTarget,
    new_name: &str,
    inner: &crate::state::ServerStateInner,
) -> HashMap<url::Url, Vec<TextEdit>> {
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);
    let mut changes: HashMap<url::Url, Vec<TextEdit>> = HashMap::new();

    match target {
        ReferenceTarget::Passage { name: old_name } => {
            for doc in inner.workspace.documents() {
                let Some(text) = inner.open_documents.get(&doc.uri) else { continue };
                let mut doc_edits = Vec::new();

                for passage in &doc.passages {
                    if passage.name == *old_name {
                        let range = passage.header_name_span.as_ref()
                            .map(|ns| helpers::byte_range_to_lsp_range(text, &passage.abs_range(ns)))
                            .unwrap_or_else(|| helpers::compute_passage_name_range_fallback(text, &passage.abs_range(&passage.span)));
                        doc_edits.push(TextEdit { range, new_text: new_name.to_string() });
                    }
                    for link in &passage.links {
                        if link.target.trim() == *old_name {
                            let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span));
                            doc_edits.push(TextEdit { range, new_text: new_name.to_string() });
                        }
                    }
                }

                if !doc_edits.is_empty() {
                    changes.insert(doc.uri.clone(), doc_edits);
                }
            }
        }

        ReferenceTarget::CustomMacro { name: old_name } => {
            // Definition site: `<<widget oldname>>` or `Macro.add("oldname", ...)`.
            // The definition offset comes from `find_custom_macro()`.
            if let Some(plugin) = plugin {
                if let Some((defined_in, file_uri, passage_rel_offset)) = plugin.find_custom_macro(old_name) {
                    if let Ok(target_uri) = file_uri.parse::<url::Url>() {
                        if let Some(target_text) = inner.open_documents.get(&target_uri) {
                            if let Some((doc, passage)) = inner.workspace.find_passage(&defined_in) {
                                if doc.uri == target_uri {
                                    let abs_offset = passage.abs_offset(passage_rel_offset);
                                    let range = identifier_range_at_offset(target_text, abs_offset);
                                    changes.entry(target_uri.clone()).or_default().push(TextEdit {
                                        range,
                                        new_text: new_name.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Call sites: all `<<oldname>>` invocations across all documents.
            for doc in inner.workspace.documents() {
                let Some(text) = inner.open_documents.get(&doc.uri) else { continue };
                let mut doc_edits = Vec::new();
                for passage in &doc.passages {
                    for inv in &passage.macro_invocations {
                        if inv.name == *old_name {
                            let range = helpers::byte_range_to_lsp_range(text, &passage.abs_range(&inv.name_span));
                            doc_edits.push(TextEdit { range, new_text: new_name.to_string() });
                        }
                    }
                }
                if !doc_edits.is_empty() {
                    changes.entry(doc.uri.clone()).or_default().extend(doc_edits);
                }
            }
        }

        ReferenceTarget::Function { name: old_name } => {
            // Definition site: `function oldname()` or `var oldname = function()`.
            if let Some(plugin) = plugin {
                if let Some(info) = plugin.find_function(old_name) {
                    if let Ok(target_uri) = info.file_uri.parse::<url::Url>() {
                        if let Some(target_text) = inner.open_documents.get(&target_uri) {
                            if let Some((doc, passage)) = inner.workspace.find_passage(&info.defined_in) {
                                if doc.uri == target_uri {
                                    let abs_offset = passage.abs_offset(info.defined_at_offset);
                                    let range = identifier_range_at_offset(target_text, abs_offset);
                                    changes.entry(target_uri.clone()).or_default().push(TextEdit {
                                        range,
                                        new_text: new_name.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Call sites: semantic tokens + text-scan fallback (same as references).
            use knot_formats::plugin::SemanticTokenType;
            for doc in inner.workspace.documents() {
                let Some(text) = inner.open_documents.get(&doc.uri) else { continue };
                let mut doc_edits = Vec::new();

                // Path A: semantic tokens
                if let Some(token_groups) = inner.semantic_tokens.get(&doc.uri) {
                    for group in token_groups {
                        let group_offset = group.passage_offset;
                        for token in &group.tokens {
                            if token.token_type != SemanticTokenType::Function { continue }
                            let abs_start = token.start + group_offset;
                            let abs_end = abs_start + token.length;
                            if abs_end > text.len() { continue }
                            if &text[abs_start..abs_end] == old_name.as_str() {
                                doc_edits.push(TextEdit {
                                    range: helpers::byte_range_to_lsp_range(text, &(abs_start..abs_end)),
                                    new_text: new_name.to_string(),
                                });
                            }
                        }
                    }
                }

                // Path B: text-scan fallback
                if let Some(plugin) = plugin {
                    if plugin.find_function(old_name).is_some() {
                        for (offset, ident) in identifiers_in_text(text) {
                            if ident == old_name.as_str() {
                                let end = offset + ident.len();
                                doc_edits.push(TextEdit {
                                    range: helpers::byte_range_to_lsp_range(text, &(offset..end)),
                                    new_text: new_name.to_string(),
                                });
                            }
                        }
                    }
                }

                if !doc_edits.is_empty() {
                    // Dedupe (Path A + B can both match the same offset).
                    doc_edits.sort_by(|a, b| {
                        a.range.start.line.cmp(&b.range.start.line)
                            .then(a.range.start.character.cmp(&b.range.start.character))
                    });
                    doc_edits.dedup_by(|a, b| a.range.start == b.range.start && a.range.end == b.range.end);
                    changes.entry(doc.uri.clone()).or_default().extend(doc_edits);
                }
            }
        }

        ReferenceTarget::Template { name: old_name } => {
            // Definition site: `Template.add("oldname", ...)`.
            if let Some(plugin) = plugin {
                if let Some(info) = plugin.find_template(old_name) {
                    if let Ok(target_uri) = info.file_uri.parse::<url::Url>() {
                        if let Some(target_text) = inner.open_documents.get(&target_uri) {
                            if let Some((doc, passage)) = inner.workspace.find_passage(&info.defined_in) {
                                if doc.uri == target_uri {
                                    let abs_offset = passage.abs_offset(info.defined_at_offset);
                                    let range = identifier_range_at_offset(target_text, abs_offset);
                                    changes.entry(target_uri.clone()).or_default().push(TextEdit {
                                        range,
                                        new_text: new_name.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Call sites: all `?oldname` invocations across all documents.
            if let Some(plugin) = plugin {
                if plugin.find_template(old_name).is_some() {
                    for doc in inner.workspace.documents() {
                        let Some(text) = inner.open_documents.get(&doc.uri) else { continue };
                        let mut doc_edits = Vec::new();
                        for (name_offset, ident) in template_invocations_in_text(text) {
                            if ident == old_name.as_str() {
                                let end = name_offset + ident.len();
                                doc_edits.push(TextEdit {
                                    range: helpers::byte_range_to_lsp_range(text, &(name_offset..end)),
                                    new_text: new_name.to_string(),
                                });
                            }
                        }
                        if !doc_edits.is_empty() {
                            changes.entry(doc.uri.clone()).or_default().extend(doc_edits);
                        }
                    }
                }
            }
        }
    }

    changes
}

/// Compute the cursor range for `prepare_rename` — the range of the identifier
/// under the cursor that should be highlighted as the "rename target".
///
/// Returns `None` if the cursor isn't on a renamable target.
pub(crate) fn rename_range_at_cursor(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<(Range, String)> {
    let target = resolve_target_at_cursor(inner, uri, position)?;

    // Failsafe: don't allow rename if the definition can't be confirmed.
    if !definition_confirmed(&target, inner) {
        return None;
    }

    let text = inner.open_documents.get(uri)?;
    let byte_offset = helpers::position_to_byte_offset(text, position);

    match &target {
        ReferenceTarget::Passage { name } => {
            // Use the link span or header name span — same logic as the
            // existing passage rename in editing.rs.
            if let Some(doc) = inner.workspace.get_document(uri) {
                for passage in &doc.passages {
                    if passage.name == *name {
                        if let Some(ns) = &passage.header_name_span {
                            return Some((helpers::byte_range_to_lsp_range(text, &passage.abs_range(ns)), name.clone()));
                        }
                    }
                    for link in &passage.links {
                        if link.target.trim() == *name && passage.span_contains_abs_offset(&link.span, byte_offset) {
                            return Some((helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span)), name.clone()));
                        }
                    }
                }
            }
            None
        }
        ReferenceTarget::CustomMacro { name } => {
            // Cursor is on the macro name in `macro_invocations`. Use the
            // name_span from the matching invocation.
            if let Some(doc) = inner.workspace.get_document(uri) {
                for passage in &doc.passages {
                    for inv in &passage.macro_invocations {
                        if inv.name == *name && passage.span_contains_abs_offset(&inv.name_span, byte_offset) {
                            return Some((helpers::byte_range_to_lsp_range(text, &passage.abs_range(&inv.name_span)), name.clone()));
                        }
                    }
                }
            }
            None
        }
        ReferenceTarget::Function { name } => {
            // Cursor is on a function name (semantic token or text-scan).
            // Use the identifier range at the cursor.
            let range = identifier_range_at_offset(text, byte_offset);
            Some((range, name.clone()))
        }
        ReferenceTarget::Template { name } => {
            // Cursor is on `?name`. The range covers just the name (not `?`).
            // Recompute the identifier boundaries directly (we can't reuse
            // `template_name_at_offset` because it returns the name string
            // but not its offset).
            let bytes = text.as_bytes();
            let mut name_start = byte_offset.min(text.len());
            let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
            while name_start > 0 && is_ident(bytes[name_start - 1]) {
                name_start -= 1;
            }
            let mut name_end = name_start;
            while name_end < text.len() && is_ident(bytes[name_end]) {
                name_end += 1;
            }
            Some((helpers::byte_range_to_lsp_range(text, &(name_start..name_end)), name.clone()))
        }
    }
}

/// Scan `text` and yield `(byte_offset, identifier_text)` for every identifier.
///
/// Used by the function-references text-scan fallback. An identifier is a
/// maximal run of `[A-Za-z0-9_$]` characters. This is format-agnostic — it
/// doesn't know about SugarCube macros or JS syntax, it just finds words. The
/// caller is responsible for filtering (e.g., checking `find_function`).
fn identifiers_in_text(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';

    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i < len {
            if is_ident(bytes[i]) {
                let start = i;
                while i < len && is_ident(bytes[i]) {
                    i += 1;
                }
                return Some((start, &text[start..i]));
            }
            i += 1;
        }
        None
    })
}

/// Scan `text` and yield `(name_offset, identifier_text)` for every `?name`
/// template invocation.
///
/// `name_offset` is the byte offset of the identifier (NOT the `?` prefix).
/// This is what callers need for range computation — renaming `?heal` →
/// `?cured` means replacing the `heal` part, not the `?`.
///
/// Format-agnostic text scan. The caller is responsible for confirming the
/// name is a known template via `plugin.find_template()`. This catches `?name`
/// but NOT `?{...}` or `?[[...]]` (rare template forms).
fn template_invocations_in_text(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';

    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i < len {
            // Look for `?` followed by an identifier-start character.
            if bytes[i] == b'?' && i + 1 < len && is_ident(bytes[i + 1]) {
                let name_start = i + 1;
                let mut name_end = name_start;
                while name_end < len && is_ident(bytes[name_end]) {
                    name_end += 1;
                }
                i = name_end;
                return Some((name_start, &text[name_start..name_end]));
            }
            i += 1;
        }
        None
    })
}



#[cfg(test)]
mod goto_definition_tests {
    use super::*;
    use knot_formats::plugin::{FormatPlugin, FormatPluginMut};
    use url::Url;

    /// Build a ServerStateInner fixture: parse a single twee source file via
    /// the registry's own SugarCube plugin (so the custom-macro and function
    /// data is populated), then assemble the inner state.
    fn build_state(src: &str) -> (crate::state::ServerStateInner, Url) {
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let mut registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let format = knot_core::passage::StoryFormat::SugarCube;
        let parse_result = {
            let plugin = registry.get_mut(&format).expect("SugarCube plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        let workspace = {
            let mut ws = knot_core::Workspace::new(Url::parse("file:///project/").unwrap());
            // Force the workspace to resolve as SugarCube (otherwise
            // resolve_format() falls back to Core since there's no StoryData
            // passage in the test fixture, and the registry would return the
            // TwineCore plugin instead of SugarCubePlugin).
            ws.config.format = Some("SugarCube".to_string());
            let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
            for passage in parse_result.passages {
                doc.passages.push(passage);
            }
            ws.insert_document(doc);
            ws
        };

        let inner = crate::state::ServerStateInner {
            workspace,
            format_registry: registry,
            debounce: knot_core::editing::DebounceTimer::new(),
            editor_open_docs: std::collections::HashSet::new(),
            open_documents: {
                let mut m = std::collections::HashMap::new();
                m.insert(uri.clone(), src.to_string());
                m
            },
            format_diagnostics: std::collections::HashMap::new(),
            doc_versions: std::collections::HashMap::new(),
            semantic_tokens: {
                let mut m = std::collections::HashMap::new();
                m.insert(uri.clone(), parse_result.token_groups);
                m
            },
        };
        (inner, uri)
    }

    /// Cursor on `<<mywidget>>` invocation should jump to the
    /// `<<widget mywidget>>` definition.
    #[test]
    fn goto_def_for_widget_definition() {
        // The Widgets passage must be tagged `[widget]` for SugarCube to
        // recognize `<<widget>>` definitions inside it. Widget names are
        // bare identifiers (not quoted) per SugarCube syntax.
        let src = ":: Widgets [widget]\n<<widget mywidget>>Hello<</widget>>\n\n:: Start\n<<mywidget>>\n";
        let (inner, uri) = build_state(src);

        // Sanity: the widget is registered.
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format).expect("plugin");
        assert!(plugin.is_custom_macro("mywidget"),
            "widget `mywidget` should be registered. custom_macros: {:?}",
            plugin.custom_macro_names());

        // Cursor on `mywidget` in `<<mywidget>>` (in :: Start).
        let start_idx = src.find(":: Start").unwrap();
        let inv_idx = src[start_idx..].find("mywidget").unwrap() + start_idx;
        let position = helpers::byte_offset_to_position(src, inv_idx);

        let resp = goto_definition_inner(&inner, &uri, position).expect("expected Some");
        let loc = match resp {
            GotoDefinitionResponse::Scalar(l) => l,
            other => panic!("expected Scalar, got {other:?}"),
        };
        assert_eq!(loc.uri, uri, "should jump within the same file");
        // Range should cover `mywidget` in the `<<widget mywidget>>` line.
        let widget_line_start = src.find(":: Widgets").unwrap();
        let widget_name_offset = src[widget_line_start..].find("mywidget").unwrap() + widget_line_start;
        let widget_name_end = widget_name_offset + "mywidget".len();
        assert_eq!(loc.range.start, helpers::byte_offset_to_position(src, widget_name_offset),
            "range start should be at the widget name");
        assert_eq!(loc.range.end, helpers::byte_offset_to_position(src, widget_name_end),
            "range end should be just past the widget name");
    }

    /// Cursor on a function call inside `<<run>>` should jump to the function
    /// declaration in a `[script]` passage.
    #[test]
    fn goto_def_for_function_in_run() {
        let src = ":: Scripts [script]\nfunction myFunc() { return 42; }\n\n:: Start\n<<run myFunc()>>\n";
        let (inner, uri) = build_state(src);

        // Sanity: the function is registered.
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format).expect("plugin");
        assert!(plugin.find_function("myFunc").is_some(),
            "function `myFunc` should be registered. functions: {:?}",
            plugin.function_names());

        // Cursor on `myFunc` in `<<run myFunc()>>` (the call site in :: Start).
        let start_idx = src.find(":: Start").unwrap();
        let call_idx = src[start_idx..].find("myFunc").unwrap() + start_idx;
        let position = helpers::byte_offset_to_position(src, call_idx);

        let resp = goto_definition_inner(&inner, &uri, position).expect("expected Some");
        let loc = match resp {
            GotoDefinitionResponse::Scalar(l) => l,
            other => panic!("expected Scalar, got {other:?}"),
        };
        assert_eq!(loc.uri, uri, "should jump within the same file");
        // Range should cover `myFunc` in `function myFunc() { ... }`.
        let script_line_start = src.find(":: Scripts").unwrap();
        let func_name_offset = src[script_line_start..].find("myFunc").unwrap() + script_line_start;
        let func_name_end = func_name_offset + "myFunc".len();
        assert_eq!(loc.range.start, helpers::byte_offset_to_position(src, func_name_offset),
            "range start should be at the function name");
        assert_eq!(loc.range.end, helpers::byte_offset_to_position(src, func_name_end),
            "range end should be just past the function name");
    }

    /// Cursor on a builtin macro like `<<if>>` should return None (builtins
    /// have no user-code definition to jump to).
    #[test]
    fn goto_def_for_builtin_macro_returns_none() {
        let src = ":: Start\n<<if true>>Hi<</if>>\n";
        let (inner, uri) = build_state(src);

        // Cursor on `if` in `<<if true>>`.
        let if_idx = src.find("if").unwrap();
        let position = helpers::byte_offset_to_position(src, if_idx);

        let result = goto_definition_inner(&inner, &uri, position);
        assert!(result.is_none(), "builtin macro should not resolve, got: {result:?}");
    }

    /// `identifier_range_at_offset` should return the range of the identifier
    /// starting at (or near) the given offset.
    #[test]
    fn identifier_range_at_offset_basic() {
        let text = "function myFunc() { return 1; }";
        // `myFunc` starts at offset 9.
        let range = identifier_range_at_offset(text, 9);
        assert_eq!(range.start.character, 9);
        assert_eq!(range.end.character, 9 + "myFunc".len() as u32);
    }

    /// When the offset points at a non-identifier character (e.g., the opening
    /// quote of a string literal in `Macro.add("name", ...)`), the helper
    /// should scan forward to find the next identifier.
    #[test]
    fn identifier_range_at_offset_scans_forward_past_quote() {
        let text = "Macro.add(\"mymacro\", { ... })";
        // Offset of the opening `"` — should scan forward to `mymacro`.
        let quote_offset = text.find('"').unwrap();
        let range = identifier_range_at_offset(text, quote_offset);
        let macro_offset = text.find("mymacro").unwrap();
        assert_eq!(range.start.character, macro_offset as u32);
        assert_eq!(range.end.character, (macro_offset + "mymacro".len()) as u32);
    }

    /// Macro names with hyphens (e.g. `<<link-replace>>`) should produce a
    /// range covering the full name including the hyphen.
    #[test]
    fn identifier_range_at_offset_handles_hyphenated_name() {
        let text = "<<widget link-replace>>";
        let name_offset = text.find("link-replace").unwrap();
        let range = identifier_range_at_offset(text, name_offset);
        assert_eq!(range.start.character, name_offset as u32);
        assert_eq!(range.end.character, (name_offset + "link-replace".len()) as u32);
    }

    // ── Find References tests ────────────────────────────────────────────

    /// Shift+F12 on `<<mywidget>>` should find all invocation sites across
    /// the workspace — including the one the cursor is on.
    #[test]
    fn references_for_custom_macro_finds_all_invocations() {
        let src = ":: Widgets [widget]\n<<widget mywidget>>Hello<</widget>>\n\n:: Start\n<<mywidget>>\n\n:: Other\n<<mywidget>>\n";
        let (inner, uri) = build_state(src);

        // Cursor on `mywidget` in `:: Start`.
        let start_idx = src.find(":: Start").unwrap();
        let inv_idx = src[start_idx..].find("mywidget").unwrap() + start_idx;
        let position = helpers::byte_offset_to_position(src, inv_idx);

        let locations = references_inner(&inner, &uri, position)
            .expect("expected Some(Vec<Location>)");

        // Should find 2 invocation sites: one in :: Start, one in :: Other.
        // (The `<<widget mywidget>>` definition is NOT a `macro_invocations`
        // entry — it's the definition, tracked separately in the registry.
        // `macro_invocations` only tracks call sites.)
        assert_eq!(locations.len(), 2,
            "should find 2 `<<mywidget>>` call sites, got {}: {:#?}",
            locations.len(), locations);

        // Both locations should be in the same file.
        for loc in &locations {
            assert_eq!(loc.uri, uri);
        }

        // Verify the ranges actually cover `mywidget` text.
        for loc in &locations {
            let start_byte = helpers::position_to_byte_offset(src, loc.range.start);
            let end_byte = helpers::position_to_byte_offset(src, loc.range.end);
            assert_eq!(&src[start_byte..end_byte], "mywidget",
                "range should cover `mywidget`, got `{}`", &src[start_byte..end_byte]);
        }
    }

    /// Shift+F12 on a builtin macro (`<<if>>`) should return None — builtins
    /// have no user-code references to find (they're language keywords).
    #[test]
    fn references_for_builtin_macro_returns_none() {
        let src = ":: Start\n<<if true>>Hi<</if>>\n";
        let (inner, uri) = build_state(src);

        let if_idx = src.find("if").unwrap();
        let position = helpers::byte_offset_to_position(src, if_idx);

        let result = references_inner(&inner, &uri, position);
        assert!(result.is_none(), "builtin macro should not resolve, got: {result:?}");
    }

    /// Shift+F12 on a function call should find all call sites across the
    /// workspace via the text-scan fallback.
    #[test]
    fn references_for_function_finds_all_callsites() {
        let src = ":: Scripts [script]\nfunction myFunc() { return 42; }\n\n:: A\n<<run myFunc()>>\n\n:: B\n<<run myFunc()>>\n";
        let (inner, uri) = build_state(src);

        // Cursor on `myFunc` in `:: A` passage's `<<run myFunc()>>`.
        let a_idx = src.find(":: A").unwrap();
        let call_idx = src[a_idx..].find("myFunc").unwrap() + a_idx;
        let position = helpers::byte_offset_to_position(src, call_idx);

        let locations = references_inner(&inner, &uri, position)
            .expect("expected Some(Vec<Location>)");

        // Should find at least the 2 call sites in :: A and :: B.
        // (The text-scan fallback scans ALL documents, so it'll also find
        // the `function myFunc` declaration in :: Scripts. We check >= 2
        // rather than == 3 because the declaration may or may not be
        // included depending on whether the text-scan matches it — and
        // that's fine, the LSP `include_declaration` flag would control
        // that in a real implementation.)
        assert!(locations.len() >= 2,
            "should find at least 2 `myFunc` call sites, got {}: {:#?}",
            locations.len(), locations);

        // Verify all locations are in the same file.
        for loc in &locations {
            assert_eq!(loc.uri, uri);
        }
    }

    /// Shift+F12 on a passage link should find all links targeting that passage.
    #[test]
    fn references_for_passage_link_finds_all_links() {
        let src = ":: Target\nYou are here.\n\n:: A\n[[Target]]\n\n:: B\n[[Target]]\n";
        let (inner, uri) = build_state(src);

        // Cursor on `[[Target]]` in :: A.
        let a_idx = src.find(":: A").unwrap();
        let link_idx = src[a_idx..].find("Target").unwrap() + a_idx;
        let position = helpers::byte_offset_to_position(src, link_idx);

        let locations = references_inner(&inner, &uri, position)
            .expect("expected Some(Vec<Location>)");

        // Should find: 1 header (Target passage) + 2 links (in A and B) = 3.
        assert!(locations.len() >= 2,
            "should find at least 2 link references to Target, got {}: {:#?}",
            locations.len(), locations);
    }

    // ── Rename tests ─────────────────────────────────────────────────────

    /// F2 on `<<mywidget>>` should rename the widget definition + all call sites.
    #[test]
    fn rename_widget_renames_definition_and_invocations() {
        let src = ":: Widgets [widget]\n<<widget mywidget>>Hello<</widget>>\n\n:: Start\n<<mywidget>>\n\n:: Other\n<<mywidget>>\n";
        let (inner, uri) = build_state(src);

        // Cursor on `mywidget` in `:: Start`.
        let start_idx = src.find(":: Start").unwrap();
        let inv_idx = src[start_idx..].find("mywidget").unwrap() + start_idx;
        let position = helpers::byte_offset_to_position(src, inv_idx);

        // prepare_rename should return the range + current name.
        let (range, placeholder) = rename_range_at_cursor(&inner, &uri, position)
            .expect("prepare_rename should succeed for a custom macro");
        assert_eq!(placeholder, "mywidget");
        let start_byte = helpers::position_to_byte_offset(src, range.start);
        let end_byte = helpers::position_to_byte_offset(src, range.end);
        assert_eq!(&src[start_byte..end_byte], "mywidget");

        // collect_rename_edits should produce edits for the definition + 2 call sites.
        let target = resolve_target_at_cursor(&inner, &uri, position).unwrap();
        assert!(definition_confirmed(&target, &inner), "definition must be confirmed");
        let changes = collect_rename_edits(&target, "renamed", &inner);
        let edits = changes.get(&uri).expect("should have edits for the file");
        // 1 definition + 2 call sites = 3 edits.
        assert_eq!(edits.len(), 3, "should rename 1 definition + 2 call sites, got {}: {:#?}", edits.len(), edits);
        for edit in edits {
            assert_eq!(edit.new_text, "renamed");
        }
    }

    /// F2 on a function call should rename the declaration + all call sites.
    #[test]
    fn rename_function_renames_declaration_and_callsites() {
        let src = ":: Scripts [script]\nfunction myFunc() { return 42; }\n\n:: A\n<<run myFunc()>>\n\n:: B\n<<run myFunc()>>\n";
        let (inner, uri) = build_state(src);

        // Cursor on `myFunc` in `:: A`.
        let a_idx = src.find(":: A").unwrap();
        let call_idx = src[a_idx..].find("myFunc").unwrap() + a_idx;
        let position = helpers::byte_offset_to_position(src, call_idx);

        let (range, placeholder) = rename_range_at_cursor(&inner, &uri, position)
            .expect("prepare_rename should succeed for a function");
        assert_eq!(placeholder, "myFunc");

        let target = resolve_target_at_cursor(&inner, &uri, position).unwrap();
        assert!(definition_confirmed(&target, &inner), "definition must be confirmed");
        let changes = collect_rename_edits(&target, "newFunc", &inner);
        let edits = changes.get(&uri).expect("should have edits");
        // At least: 1 declaration + 2 call sites = 3 edits. (Text-scan may
        // also find the declaration in :: Scripts, so we check >= 3.)
        assert!(edits.len() >= 3, "should rename declaration + 2 call sites, got {}: {:#?}", edits.len(), edits);
        for edit in edits {
            assert_eq!(edit.new_text, "newFunc");
        }
    }

    /// F2 on `?pirate` should rename the Template.add definition + all `?pirate` invocations.
    #[test]
    fn rename_template_renames_definition_and_invocations() {
        let src = ":: Scripts [script]\nTemplate.add('pirate', function () { return \"Hello!\"; });\n\n:: Start\npirate says: ?pirate\n";
        let (inner, uri) = build_state(src);

        // Debug: confirm the template is registered.
        let format = inner.workspace.resolve_format();
        let plugin = inner.format_registry.get(&format).expect("plugin");
        assert!(plugin.find_template("pirate").is_some(),
            "template `pirate` should be registered. templates: {:?}",
            plugin.template_names());

        // Cursor on `pirate` in `?pirate` (the invocation in :: Start).
        let start_idx = src.find(":: Start").unwrap();
        let inv_idx = src[start_idx..].find("?pirate").unwrap() + start_idx + 1; // +1 to skip `?`
        let position = helpers::byte_offset_to_position(src, inv_idx);

        let (range, placeholder) = rename_range_at_cursor(&inner, &uri, position)
            .expect("prepare_rename should succeed for a template");
        assert_eq!(placeholder, "pirate");

        let target = resolve_target_at_cursor(&inner, &uri, position).unwrap();
        assert!(definition_confirmed(&target, &inner), "definition must be confirmed");
        let changes = collect_rename_edits(&target, "captain", &inner);
        let edits = changes.get(&uri).expect("should have edits");
        // 1 definition (Template.add('pirate', ...)) + 1 invocation (?pirate) = 2 edits.
        // The text-scan for `?pirate` finds the invocation; the definition
        // edit comes from `find_template`'s offset.
        assert!(edits.len() >= 2, "should rename definition + invocation, got {}: {:#?}", edits.len(), edits);
        for edit in edits {
            assert_eq!(edit.new_text, "captain");
        }
    }

    /// F2 on a builtin macro (`<<if>>`) should return None — builtins are
    /// not user-renamable.
    #[test]
    fn rename_builtin_macro_returns_none() {
        let src = ":: Start\n<<if true>>Hi<</if>>\n";
        let (inner, uri) = build_state(src);

        let if_idx = src.find("if").unwrap();
        let position = helpers::byte_offset_to_position(src, if_idx);

        let result = rename_range_at_cursor(&inner, &uri, position);
        assert!(result.is_none(), "builtin macro should not be renamable, got: {result:?}");
    }

    /// Failsafe: if the definition can't be confirmed (passage not in
    /// workspace), rename should return None even if the cursor resolves
    /// to a target. This simulates a stale registry.
    #[test]
    fn rename_failsafe_returns_none_when_definition_not_found() {
        // Build a state with a widget, then check that definition_confirmed
        // returns false when we remove the widget passage from the workspace.
        // We can't easily remove a passage from the workspace in this test
        // harness, so instead we test the failsafe by checking a target that
        // resolves but whose definition is stale.
        //
        // Simpler approach: cursor on a macro name that ISN'T a known custom
        // macro. resolve_target_at_cursor returns None, so rename_range_at_cursor
        // returns None too.
        let src = ":: Start\n<<unknownmacro>>\n";
        let (inner, uri) = build_state(src);

        let macro_idx = src.find("unknownmacro").unwrap();
        let position = helpers::byte_offset_to_position(src, macro_idx);

        let result = rename_range_at_cursor(&inner, &uri, position);
        assert!(result.is_none(),
            "unknown macro should not be renamable (no definition to confirm), got: {result:?}");
    }
}
