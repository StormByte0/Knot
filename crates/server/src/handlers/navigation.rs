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
fn goto_definition_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let text = inner.open_documents.get(uri)?;

    // ── 1. Try link target (e.g., [[Target]], <<goto "Target">>) ──────────
    if let Some(target_name) = helpers::find_link_target_at_position_span_based(
        text, &inner.workspace, uri, position,
    ) {
        if let Some((doc, _passage)) = inner.workspace.find_passage(&target_name) {
            let target_uri = doc.uri.clone();
            let target_text = inner.open_documents.get(&target_uri);
            let range = if let Some(t) = target_text {
                helpers::find_passage_header_range_span_based(t, &inner.workspace, &target_name)
            } else {
                Range::default()
            };

            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: target_uri,
                range,
            }));
        }
    }

    // Convert cursor to byte offset for span-based lookups below.
    let byte_offset = helpers::position_to_byte_offset(text, position);

    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // ── 2. Try custom macro (<<mywidget>> where mywidget is user-defined) ─
    //
    // Walks `passage.macro_invocations` (the same index hover uses). When the
    // cursor is on a macro name, check whether the name is a builtin via
    // `plugin.find_macro()` — if it returns `None`, the name might be a
    // custom macro (widget / Macro.add). Query `plugin.find_custom_macro()`
    // for the definition location.
    //
    // Only fires on the macro NAME span, not the open_span, so cursor on `<<`
    // or `>>` doesn't intercept link/variable resolution. Builtin macros
    // (`<<if>>`, `<<set>>`, etc.) have no definition to jump to, so they fall
    // through to None.
    if let Some(plugin) = plugin {
        if let Some(doc) = inner.workspace.get_document(uri) {
            for passage in &doc.passages {
                for inv in &passage.macro_invocations {
                    if !passage.span_contains_abs_offset(&inv.name_span, byte_offset) {
                        continue;
                    }
                    // Skip builtins — they have no definition site in user code.
                    if plugin.find_macro(&inv.name).is_some() {
                        break;
                    }
                    if let Some(loc) = lookup_custom_macro_definition(
                        &inv.name, plugin, inner,
                    ) {
                        return Some(GotoDefinitionResponse::Scalar(loc));
                    }
                    break;
                }
            }
        }
    }

    // ── 3. Try function (myFunc() inside <<run>> / <<set>> / etc.) ────────
    //
    // Two-step resolution:
    //   a) If there's a `Function` semantic token under the cursor (emitted for
    //      SugarCube variable calls like `_myFunc()` and for function
    //      declarations), use it directly.
    //   b) Fall back to scanning the source text at the cursor for an
    //      identifier, then check if `plugin.find_function(name)` knows it.
    //      This handles regular JS function calls (`myFunc()`) which the JS
    //      walker doesn't emit `Function` tokens for (it only emits them for
    //      preprocessed SugarCube variable calls). This is the common case for
    //      `<<run myFunc()>>`.
    if let Some(plugin) = plugin {
        let token_groups = inner.semantic_tokens.get(uri).cloned().unwrap_or_default();
        if let Some(loc) = lookup_function_definition(
            text, byte_offset, token_groups, plugin, inner,
        ) {
            return Some(GotoDefinitionResponse::Scalar(loc));
        }
        // Fallback: scan source text at cursor for an identifier.
        if let Some(loc) = lookup_function_by_text_scan(
            text, byte_offset, plugin, inner,
        ) {
            return Some(GotoDefinitionResponse::Scalar(loc));
        }
    }

    None
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

    // First, determine what the user is on: a passage header or a link
    let target_passage = if let Some(text) = inner.open_documents.get(&uri) {
        // Check if cursor is on a passage header
        if let Some(name) = helpers::find_passage_at_position_span_based(
            text, &inner.workspace, &uri, position,
        ) {
            Some(name)
        } else {
            helpers::find_link_target_at_position_span_based(
                text, &inner.workspace, &uri, position,
            )
        }
    } else {
        None
    };

    let Some(target_name) = target_passage else {
        return Ok(None);
    };

    // Find all locations that reference this passage using workspace data:
    // - Header references: passages where passage.name == target_name → use
    //   header_name_span (or passage.span as fallback)
    // - Link references: passages where any link.target == target_name → use
    //   link.span
    let mut locations = Vec::new();

    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };
        for passage in &doc.passages {
            // Header definition reference
            if passage.name == target_name {
                let range = if let Some(ref name_span) = passage.header_name_span {
                    helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span))
                } else {
                    // Fallback: compute the full header line range
                    let span_start = passage.abs_offset(passage.span.start).min(text.len());
                    let header_end = text[span_start..]
                        .find('\n')
                        .map(|n| span_start + n)
                        .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));
                    helpers::byte_range_to_lsp_range(text, &(span_start..header_end))
                };
                locations.push(Location {
                    uri: doc.uri.clone(),
                    range,
                });
            }

            // Link references
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
        Ok(Some(locations))
    }
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
}
