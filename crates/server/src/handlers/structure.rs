//! Structure handlers: folding_range, document_link, selection_range,
//! signature_help.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn folding_range(
    state: &ServerState,
    params: FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;
    Ok(folding_range_inner(&inner, &uri))
}

/// Inner synchronous implementation of `folding_range`.
///
/// Extracted so tests can call it directly without constructing a full
/// `ServerState`.
fn folding_range_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
) -> Option<Vec<FoldingRange>> {
    let text = inner.open_documents.get(uri)?;

    let mut ranges = Vec::new();

    // ── Passage folding (span-based) ───────────────────────────────
    if let Some(doc) = inner.workspace.get_document(uri) {
        let passages = &doc.passages;
        for (i, passage) in passages.iter().enumerate() {
            let span_start = passage.abs_offset(passage.span.start).min(text.len());

            // End of passage: start of next passage or end of document
            let passage_end_offset = if i + 1 < passages.len() {
                passages[i + 1]
                    .abs_offset(passages[i + 1].span.start)
                    .min(text.len())
            } else {
                text.len()
            };

            // Fold starts from the passage header line (::) — includes
            // both the header and the body in the fold.
            let start_pos = helpers::byte_offset_to_position(text, span_start);
            let end_pos = helpers::byte_offset_to_position(text, passage_end_offset);

            if end_pos.line > start_pos.line {
                ranges.push(FoldingRange {
                    start_line: start_pos.line,
                    start_character: None,
                    end_line: end_pos.line.saturating_sub(1),
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: Some(passage.name.clone()),
                });
            }
        }
    }

    // ── Macro block folding (zone-map based) ──────────────────────
    //
    // Phase 5 migration: replaced the `plugin.scan_line_for_macro_events`
    // line-by-line scan with `passage.zones.iter_bodies()`. Each `MacroBody`
    // in the zone map represents a foldable region (open tag → close tag).
    //
    // The old code scanned every line for `<<name>>`/`<</name>>` events and
    // paired them with a stack. The zone map already has this pairing
    // (established by the parser's tree builder), so we just iterate the
    // bodies and convert their spans to line numbers.
    //
    // We use `name_span` (the macro name in the open tag) for the start line
    // and `close_span` (the full `<</name>>` tag) for the end line. If the
    // body is unclosed (`close_span == None`), we skip it (nothing to fold).
    if let Some(doc) = inner.workspace.get_document(uri) {
        for passage in &doc.passages {
            for body in passage.zones.iter_bodies() {
                // Only fold bodies that have a close tag (unclosed bodies
                // have no foldable end).
                let Some(close_span) = &body.close_span else {
                    continue;
                };

                // Convert passage-relative spans to document-absolute.
                let open_abs = passage.abs_offset(body.name_span.start);
                let close_abs = passage.abs_offset(close_span.start);

                // Convert to line numbers.
                let start_pos = helpers::byte_offset_to_position(text, open_abs.min(text.len()));
                let end_pos = helpers::byte_offset_to_position(text, close_abs.min(text.len()));

                // Fold only if the close tag is on a later line than the open
                // tag (matches old behavior: `end_line > start_line + 1`).
                // The old code used `end_line > start_line + 1` because the
                // close tag line itself shouldn't be the fold start. We use
                // `end_pos.line > start_pos.line` to match the visual intent
                // (fold spans at least 2 lines).
                if end_pos.line > start_pos.line {
                    ranges.push(FoldingRange {
                        start_line: start_pos.line,
                        start_character: None,
                        end_line: end_pos.line,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
            }
        }
    }

    if ranges.is_empty() {
        None
    } else {
        Some(ranges)
    }
}

pub(crate) async fn document_link(
    state: &ServerState,
    params: DocumentLinkParams,
) -> Result<Option<Vec<DocumentLink>>, tower_lsp::jsonrpc::Error> {
    // Short-circuit if the server is shutting down
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Ok(None);
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut links = Vec::new();

    // Use workspace passage/link data for span-based resolution.
    for passage in &doc.passages {
        for link in &passage.links {
            let target = link.target.trim();
            if !target.is_empty()
                && let Some(target_uri) = inner.workspace.find_passage_file_uri(target)
            {
                links.push(DocumentLink {
                    range: helpers::byte_range_to_lsp_range(text, &passage.abs_range(&link.span)),
                    target: Some(target_uri),
                    tooltip: Some(format!("Go to {}", target)),
                    data: None,
                });
            }
        }
    }

    if links.is_empty() {
        Ok(None)
    } else {
        Ok(Some(links))
    }
}

pub(crate) async fn selection_range(
    state: &ServerState,
    params: SelectionRangeParams,
) -> Result<Option<Vec<SelectionRange>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut results = Vec::new();
    let passages = &doc.passages;

    for position in &params.positions {
        let mut range_chain: Vec<Range> = Vec::new();
        let byte_offset = helpers::position_to_byte_offset(text, *position);

        // Level 1: Link text (if inside a [[...]])
        'link_search: for passage in passages.iter() {
            for link in &passage.links {
                if passage.span_contains_abs_offset(&link.span, byte_offset) {
                    // Found the link containing the cursor.
                    // Extract the link content to find the target portion.
                    let abs_link_span = passage.abs_range(&link.span);
                    let link_text = &text
                        [abs_link_span.start.min(text.len())..abs_link_span.end.min(text.len())];
                    let content = &link_text[2..link_text.len().saturating_sub(2)];

                    // Compute the target range within the link
                    let target_start_offset = if let Some(arrow) = content.find("->") {
                        abs_link_span.start + 2 + arrow + 2
                    } else if let Some(pipe) = content.find('|') {
                        abs_link_span.start + 2 + pipe + 1
                    } else {
                        abs_link_span.start + 2
                    };
                    let target_end_offset = abs_link_span.end.saturating_sub(2);

                    // Link text range (just the target portion)
                    range_chain.push(helpers::byte_range_to_lsp_range(
                        text,
                        &(target_start_offset..target_end_offset),
                    ));

                    // Full link range (entire [[...]])
                    range_chain.push(helpers::byte_range_to_lsp_range(
                        text,
                        &passage.abs_range(&link.span),
                    ));

                    break 'link_search;
                }
            }
        }

        // Level 2: Passage range (if cursor is within a passage)
        for (i, passage) in passages.iter().enumerate() {
            let span_start = passage.abs_offset(passage.span.start).min(text.len());
            let effective_end = if i + 1 < passages.len() {
                passages[i + 1]
                    .abs_offset(passages[i + 1].span.start)
                    .min(text.len())
            } else {
                text.len()
            };

            if byte_offset >= span_start && byte_offset < effective_end {
                let header_end = text[span_start..]
                    .find('\n')
                    .map(|n| span_start + n)
                    .unwrap_or(effective_end);

                let body_start_pos =
                    helpers::byte_offset_to_position(text, (header_end + 1).min(text.len()));
                let body_end_pos = helpers::byte_offset_to_position(text, effective_end);
                let header_start_pos = helpers::byte_offset_to_position(text, span_start);

                // Passage body range (from after header to end of passage)
                range_chain.push(Range {
                    start: body_start_pos,
                    end: body_end_pos,
                });

                // Passage header + body range
                range_chain.push(Range {
                    start: header_start_pos,
                    end: body_end_pos,
                });

                break;
            }
        }

        // Build the linked SelectionRange list (innermost first)
        let sel_range =
            range_chain
                .into_iter()
                .rev()
                .fold(None::<SelectionRange>, |parent, range| {
                    Some(SelectionRange {
                        range,
                        parent: parent.map(Box::new),
                    })
                });

        results.push(sel_range.unwrap_or(SelectionRange {
            range: Range {
                start: *position,
                end: Position {
                    line: position.line,
                    character: position.character + 1,
                },
            },
            parent: None,
        }));
    }

    Ok(Some(results))
}

pub(crate) async fn signature_help(
    state: &ServerState,
    params: SignatureHelpParams,
) -> Result<Option<SignatureHelp>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document_position_params.text_document.uri);
    let position = params.text_document_position_params.position;
    let inner = state.inner.read().await;
    Ok(signature_help_inner(&inner, &uri, position))
}

/// Inner synchronous implementation of `signature_help`.
///
/// Extracted so tests can call it directly without constructing a full
/// `ServerState` (which requires a `tower_lsp::Client` handle). Same pattern
/// as `navigation::references_inner`.
///
/// **Phase 5 migration**: the macro detection now uses the zone map
/// (`passage.zones.leaf_at` + `body_stack_at`) instead of the line-based
/// `plugin.find_macro_at_position`. This adds a behavior improvement: if the
/// cursor is inside a macro body (not on the tag line), signature help now
/// fires for the enclosing macro. The old code only fired when the cursor
/// was on the same line as the macro tag.
fn signature_help_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
    position: Position,
) -> Option<SignatureHelp> {
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    // Only provide signature help for formats with macro catalogs
    let plugin = plugin?;
    if plugin.builtin_macros().is_empty() {
        return None;
    }

    let text = inner.open_documents.get(uri)?;
    let line_text = text.lines().nth(position.line as usize)?;

    // Convert UTF-16 position to byte offsets.
    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);
    let doc_byte_offset = helpers::position_to_byte_offset(text, position);

    // ── Phase 5: zone-map based macro detection ──────────────────────
    //
    // Try the zone map first. If the cursor is on a MacroTag (open), use
    // that macro. If the cursor is inside a macro body (prose/markup inside
    // a block macro), walk up the body stack to find the enclosing macro.
    //
    // Falls back to the old line-based `find_macro_at_position` if the zone
    // map doesn't find a macro (e.g., cursor is mid-typing `<<` and the
    // parser hasn't recognized a macro yet).
    if let Some(doc) = inner.workspace.get_document(uri) {
        // Find the passage containing the cursor.
        if let Some(passage) = doc
            .passages
            .iter()
            .find(|p| p.contains_abs_offset(doc_byte_offset))
        {
            let passage_offset = doc_byte_offset.saturating_sub(passage.passage_offset);
            let leaf = passage.zones.leaf_at(passage_offset);

            // Case 1: cursor is on a MacroTag (open or expression).
            if let Some(leaf) = leaf
                && let knot_core::zoning::LeafKind::MacroTag {
                    macro_name: name,
                    part,
                    ..
                } = &leaf.kind
                && matches!(
                    part,
                    knot_core::zoning::TagPart::Open | knot_core::zoning::TagPart::Expression
                )
            {
                // The leaf's span is the open tag (passage-relative).
                // Convert to line-relative for comma counting.
                let open_abs = passage.abs_offset(leaf.span.start);
                let open_line = helpers::byte_offset_to_position(text, open_abs).line;
                if open_line == position.line {
                    // Cursor is on the open tag's line — count commas
                    // after the name to determine active parameter.
                    let line_start = helpers::position_to_byte_offset(
                        text,
                        Position {
                            line: position.line,
                            character: 0,
                        },
                    );
                    let name_end_in_line =
                        open_abs + name.len() - line_start.min(open_abs + name.len());
                    let after_name = &line_text[name_end_in_line.min(line_text.len())..];
                    let active_param = after_name.matches(',').count() as u32;
                    if let Some(help) = build_signature_help(plugin, name, active_param) {
                        return Some(help);
                    }
                }
            }

            // Case 2: cursor is inside a macro body — walk up the stack.
            let stack = passage.zones.body_stack_at(passage_offset);
            if let Some(enclosing) = stack.last() {
                // The cursor is inside a block macro's body. Show signature
                // for the innermost enclosing macro. We can't count commas
                // (the cursor isn't in the args), so active_param = 0.
                if let Some(help) = build_signature_help(plugin, &enclosing.macro_name, 0) {
                    return Some(help);
                }
            }
        }
    }

    // ── Fallback: old line-based detection for mid-typing cases ──────
    //
    // If the zone map didn't find a macro (e.g., cursor is mid-typing `<<`),
    // fall back to the line-based `find_macro_at_position`.
    let macro_info = plugin.find_macro_at_position(line_text, byte_pos)?;
    let after_name = &line_text[macro_info.name_range.end..];
    let active_param = after_name.matches(',').count() as u32;

    // Try builtin first.
    if let Some(help) = build_signature_help(plugin, &macro_info.name, active_param) {
        return Some(help);
    }

    // Try custom macro / widget.
    build_custom_macro_signature_help(plugin, &macro_info.name, line_text, &macro_info.name_range)
}

/// Build signature help for a builtin macro.
///
/// Returns `None` if the macro is not in the builtin catalog (caller should
/// try the custom-macro path).
fn build_signature_help(
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    macro_name: &str,
    active_param: u32,
) -> Option<SignatureHelp> {
    let mdef = plugin.find_macro(macro_name)?;

    // Version-aware: signature + docs come from the era-effective
    // descriptor at the story's pinned format version (e.g. `<<script>>`
    // shows no `language` parameter before SugarCube 2.37.0).
    let story_version = plugin
        .story_version()
        .unwrap_or(knot_formats::sugarcube::macros::SUGARCUBE_LATEST);
    let descriptor = mdef
        .descriptor_at(story_version)
        .unwrap_or_else(|| mdef.latest_descriptor());
    let args = descriptor.args;

    let params_list: Vec<ParameterInformation> = if let Some(args) = args {
        args.iter()
            .map(|a| ParameterInformation {
                label: ParameterLabel::Simple(a.label.to_string()),
                documentation: None,
            })
            .collect()
    } else {
        Vec::new()
    };

    let sig_str = if let Some(args) = args {
        args.iter().map(|a| a.label).collect::<Vec<_>>().join(", ")
    } else {
        String::new()
    };

    let has_params = !params_list.is_empty();
    let sig_label = plugin.format_macro_signature_label(mdef.name, &sig_str);

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: sig_label,
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: descriptor.description.to_string(),
            })),
            parameters: if has_params { Some(params_list) } else { None },
            active_parameter: if has_params { Some(active_param) } else { None },
        }],
        active_signature: Some(0),
        active_parameter: if has_params { Some(active_param) } else { None },
    })
}

/// Build signature help for a custom macro / widget.
///
/// Returns `None` if the macro is not custom or has no known arg_count.
fn build_custom_macro_signature_help(
    plugin: &dyn knot_formats::plugin::FormatPlugin,
    macro_name: &str,
    line_text: &str,
    name_range: &std::ops::Range<usize>,
) -> Option<SignatureHelp> {
    let detail = plugin.find_custom_macro_detail(macro_name)?;

    // Only fire signature help when we know the macro takes at least 1 arg.
    let n = detail.arg_count.unwrap_or(0);
    if n == 0 {
        return None;
    }

    let after_name = &line_text[name_range.end..];
    let active_param = after_name.matches(',').count() as u32;

    let labels: Vec<String> = (0..n).map(|i| format!("arg{}", i + 1)).collect();
    let params_list: Vec<ParameterInformation> = labels
        .iter()
        .map(|l| ParameterInformation {
            label: ParameterLabel::Simple(l.clone()),
            documentation: None,
        })
        .collect();
    let sig_str = labels.join(", ");

    let type_label = if detail.is_widget {
        if detail.is_container {
            "Container widget"
        } else {
            "Widget"
        }
    } else {
        "Custom macro"
    };
    let doc_text = if let Some(desc) = &detail.description {
        format!("{} — {}", type_label, desc)
    } else {
        format!("{} — defined in `:: {}`", type_label, detail.defined_in)
    };

    let sig_label = plugin.format_macro_signature_label(macro_name, &sig_str);

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: sig_label,
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: doc_text,
            })),
            parameters: Some(params_list),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}

#[cfg(test)]
mod signature_help_tests {
    use super::*;
    use url::Url;

    /// Build a ServerStateInner fixture: parse a single twee source file via
    /// the SugarCube plugin, then assemble the inner state. Same pattern as
    /// the navigation tests.
    fn build_state(src: &str) -> (crate::state::ServerStateInner, Url) {
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let mut registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let format = knot_core::passage::StoryFormat::SugarCube;
        let parse_result = {
            let plugin = registry
                .get_mut(&format)
                .expect("SugarCube plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        let workspace = {
            let mut ws = knot_core::Workspace::new(Url::parse("file:///project/").unwrap());
            ws.config.format = Some("SugarCube".to_string());
            let mut doc =
                knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
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
                m.insert(
                    uri.clone(),
                    crate::handlers::helpers::token_groups_to_map(parse_result.token_groups),
                );
                m
            },
            installed_formats: Vec::new(),
            global_storage_path: None,
        };
        (inner, uri)
    }

    /// Helper: build state and call signature_help_inner at the given position.
    fn get_signature_help(src: &str, line: u32, character: u32) -> Option<SignatureHelp> {
        let (inner, uri) = build_state(src);
        signature_help_inner(&inner, &uri, Position { line, character })
    }

    /// Builtin macro: `<<link "Talk" "Shop">>` should show a signature with
    /// the builtin `link` macro's params.
    #[test]
    fn signature_help_fires_for_builtin_macro() {
        let src = ":: Start\n<<link \"Talk\" \"Shop\">>\n";
        // Line 1, char 3 is on `i` of `link` (<< = chars 0-1, link = chars 2-5).
        let help = get_signature_help(src, 1, 3);
        assert!(
            help.is_some(),
            "signature help should fire for builtin <<link>>"
        );
        let help = help.unwrap();
        assert_eq!(help.signatures.len(), 1);
        assert!(
            help.signatures[0].label.contains("link"),
            "signature label should contain 'link': got {}",
            help.signatures[0].label
        );
    }

    /// Custom widget with known arg_count: `<<mywidget>>` invoked after a
    /// widget definition that uses `_args[0]` and `_args[1]` (so arg_count=2).
    /// The signature should show `<<mywidget arg1, arg2>>` with placeholder
    /// param names.
    #[test]
    fn signature_help_fires_for_custom_widget_with_arg_count() {
        let src = ":: Widgets [widget]\n<<widget mywidget>>Args: _args[0], _args[1]<</widget>>\n:: Start\n<<mywidget \"a\", \"b\">>\n";
        // Line 3 is `<<mywidget "a", "b">>`. `<<` = chars 0-1, `mywidget` = chars 2-9.
        // Char 5 is on `i` of `mywidget`.
        let help = get_signature_help(src, 3, 5);
        assert!(
            help.is_some(),
            "signature help should fire for custom widget <<mywidget>>"
        );
        let help = help.unwrap();
        assert_eq!(help.signatures.len(), 1);
        let sig = &help.signatures[0];
        assert!(
            sig.label.contains("mywidget"),
            "signature label should contain 'mywidget': got {}",
            sig.label
        );
        // Should have 2 params (arg1, arg2) since the widget body uses _args[0] and _args[1].
        assert!(sig.parameters.is_some(), "should have parameters");
        let params = sig.parameters.as_ref().unwrap();
        assert_eq!(
            params.len(),
            2,
            "should have 2 params (arg1, arg2): got {:?}",
            params
        );
        // The label should include the param names.
        assert!(
            sig.label.contains("arg1"),
            "signature label should contain 'arg1': got {}",
            sig.label
        );
        assert!(
            sig.label.contains("arg2"),
            "signature label should contain 'arg2': got {}",
            sig.label
        );
        // Documentation should mention "Widget".
        if let Some(Documentation::MarkupContent(m)) = &sig.documentation {
            assert!(
                m.value.contains("Widget"),
                "doc should mention 'Widget': got {}",
                m.value
            );
        }
    }

    /// Custom widget with unknown arg_count (no `_args[N]` references in the
    /// body): signature help should return None — there's nothing useful to
    /// show in a popup when we don't know the arity.
    #[test]
    fn signature_help_returns_none_for_custom_widget_without_arg_count() {
        let src =
            ":: Widgets [widget]\n<<widget simple>>Hello world<</widget>>\n:: Start\n<<simple>>\n";
        // Line 3 is `<<simple>>`. `<<` = chars 0-1, `simple` = chars 2-7.
        // Char 4 is on `m` of `simple`.
        let help = get_signature_help(src, 3, 4);
        assert!(
            help.is_none(),
            "signature help should NOT fire for argless widget (arg_count=None): got {:?}",
            help
        );
    }

    /// Macro.add() custom macro: arg_count is always None (the JS walker
    /// doesn't extract it), so signature help should return None.
    #[test]
    fn signature_help_returns_none_for_macro_add_custom_macro() {
        let src = ":: StoryJavaScript [script]\nMacro.add(\"mymacro\", { fn: function() { return this.args[0]; } });\n:: Start\n<<mymacro \"hello\">>\n";
        // Line 3 is `<<mymacro "hello">>`. `<<` = chars 0-1, `mymacro` = chars 2-8.
        // Char 4 is on `m` of `mymacro`.
        let help = get_signature_help(src, 3, 4);
        assert!(
            help.is_none(),
            "signature help should NOT fire for Macro.add() custom macro (arg_count=None): got {:?}",
            help
        );
    }

    /// Cursor NOT on a macro: signature help should return None.
    #[test]
    fn signature_help_returns_none_for_plain_text() {
        let src = ":: Start\nHello world.\n";
        let help = get_signature_help(src, 1, 3);
        assert!(
            help.is_none(),
            "signature help should NOT fire for plain text: got {:?}",
            help
        );
    }

    /// **Phase 5 behavior improvement**: signature help fires when the cursor
    /// is inside `<<link>>`'s body on a different line from `<<link>>`.
    /// The old code only fired when the cursor was on the same line as the
    /// macro tag. The zone-map migration walks up the body stack to find the
    /// enclosing macro.
    #[test]
    fn signature_help_fires_inside_macro_body_different_line() {
        let src = ":: Start\n<<link \"Go\" \"Shop\">>\n  Click here\n<</link>>\n";
        // Line 2 is `  Click here` — inside <<link>>'s body, NOT on the tag line.
        // Char 5 is in the middle of "Click here".
        let help = get_signature_help(src, 2, 5);
        assert!(
            help.is_some(),
            "signature help should fire inside <<link>> body (different line from tag): got {:?}",
            help
        );
        let help = help.unwrap();
        assert!(
            help.signatures[0].label.contains("link"),
            "signature should be for 'link' (the enclosing macro): got {}",
            help.signatures[0].label
        );
    }

    /// Signature help should NOT fire inside a macro body if the enclosing
    /// macro is inline (no body). But inline macros don't create bodies, so
    /// this case is naturally handled — the body stack is empty.
    #[test]
    fn signature_help_does_not_fire_for_inline_macro_body() {
        // `<<set $x to 1>>` is inline (no body). Cursor on line 2 (after the
        // macro) should not fire signature help via the body-context path.
        let src = ":: Start\n<<set $x to 1>>\nSome text\n";
        let help = get_signature_help(src, 2, 3);
        assert!(
            help.is_none(),
            "signature help should NOT fire on line 2 (no enclosing body): got {:?}",
            help
        );
    }
}

#[cfg(test)]
mod folding_range_tests {
    use super::*;
    use url::Url;

    /// Build a ServerStateInner fixture: parse a single twee source file via
    /// the SugarCube plugin, then assemble the inner state.
    fn build_state(src: &str) -> (crate::state::ServerStateInner, Url) {
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let mut registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let format = knot_core::passage::StoryFormat::SugarCube;
        let parse_result = {
            let plugin = registry
                .get_mut(&format)
                .expect("SugarCube plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        let workspace = {
            let mut ws = knot_core::Workspace::new(Url::parse("file:///project/").unwrap());
            ws.config.format = Some("SugarCube".to_string());
            let mut doc =
                knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
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
                m.insert(
                    uri.clone(),
                    crate::handlers::helpers::token_groups_to_map(parse_result.token_groups),
                );
                m
            },
            installed_formats: Vec::new(),
            global_storage_path: None,
        };
        (inner, uri)
    }

    /// Helper: get folding ranges for a source file.
    fn get_folding_ranges(src: &str) -> Vec<FoldingRange> {
        let (inner, uri) = build_state(src);
        folding_range_inner(&inner, &uri).unwrap_or_default()
    }

    /// A multi-line `<<if>>...<</if>>` should produce a folding range.
    #[test]
    fn folding_range_for_multiline_if_block() {
        let src = ":: Start\n<<if $x>>\n  text\n  more text\n<</if>>\n";
        let ranges = get_folding_ranges(src);
        // Should have at least one range for the <<if>> block (plus possibly
        // the passage itself).
        let macro_ranges: Vec<_> = ranges
            .iter()
            .filter(|r| r.collapsed_text.is_none())
            .collect();
        assert!(
            !macro_ranges.is_empty(),
            "should have at least one macro folding range for <<if>>: got {:?}",
            ranges
        );
        // The macro range should span from line 1 (<<if>>) to line 4 (<</if>>).
        let if_range = macro_ranges
            .iter()
            .find(|r| r.end_line > r.start_line)
            .expect("should have a multi-line folding range");
        assert_eq!(
            if_range.start_line, 1,
            "fold should start at line 1 (<<if>>)"
        );
        assert_eq!(if_range.end_line, 4, "fold should end at line 4 (<</if>>)");
    }

    /// A single-line `<<if>>...<</if>>` should NOT produce a folding range
    /// (nothing to fold — it's all on one line).
    #[test]
    fn folding_range_not_produced_for_single_line_macro() {
        let src = ":: Start\n<<if $x>>text<</if>>\n";
        let ranges = get_folding_ranges(src);
        let macro_ranges: Vec<_> = ranges
            .iter()
            .filter(|r| r.collapsed_text.is_none())
            .collect();
        assert!(
            macro_ranges.is_empty(),
            "should NOT have a macro folding range for single-line <<if>>: got {:?}",
            macro_ranges
        );
    }

    /// Nested macros should produce multiple folding ranges.
    #[test]
    fn folding_range_for_nested_macros() {
        let src = ":: Start\n<<link \"Go\">>\n  <<if $x>>\n    text\n  <</if>>\n<</link>>\n";
        let ranges = get_folding_ranges(src);
        let macro_ranges: Vec<_> = ranges
            .iter()
            .filter(|r| r.collapsed_text.is_none())
            .collect();
        assert!(
            macro_ranges.len() >= 2,
            "should have at least 2 macro folding ranges for nested <<link>><<if>>: got {:?}",
            macro_ranges
        );
    }

    /// Unclosed block macros should NOT produce a folding range (no close tag
    /// to fold to).
    #[test]
    fn folding_range_not_produced_for_unclosed_macro() {
        let src = ":: Start\n<<if $x>>\n  text\n  more text\n";
        let ranges = get_folding_ranges(src);
        let macro_ranges: Vec<_> = ranges
            .iter()
            .filter(|r| r.collapsed_text.is_none())
            .collect();
        assert!(
            macro_ranges.is_empty(),
            "should NOT have a macro folding range for unclosed <<if>>: got {:?}",
            macro_ranges
        );
    }
}
