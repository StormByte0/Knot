//! Structure handlers: folding_range, document_link, selection_range,
//! signature_help.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_formats::plugin::MacroBlockEvent;
use lsp_types::*;

pub(crate) async fn folding_range(
    state: &ServerState,
    params: FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let mut ranges = Vec::new();

    // ── Passage folding (span-based) ───────────────────────────────
    if let Some(doc) = inner.workspace.get_document(&uri) {
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

    // ── Macro block folding ──────────────────────────────────────
    // Use the format plugin for format-agnostic macro block detection.
    let format = inner.workspace.resolve_format();
    if let Some(plugin) = inner.format_registry.get(&format) {
        let lines: Vec<&str> = text.lines().collect();
        let mut open_stack: Vec<(String, u32)> = Vec::new(); // (name, start_line)

        // Collect all macro block events from the format plugin
        let mut all_events: Vec<MacroBlockEvent> = Vec::new();
        for (line_idx, line) in lines.iter().enumerate() {
            all_events.extend(plugin.scan_line_for_macro_events(line, line_idx as u32));
        }

        for event in all_events {
            if event.is_open {
                open_stack.push((event.name, event.line));
            } else {
                // Find matching open tag on stack (search backward)
                if let Some(pos) = open_stack.iter().rposition(|(n, _)| n == &event.name) {
                    let (_, start_line) = open_stack.remove(pos);
                    let end_line = event.line;
                    if end_line > start_line + 1 {
                        ranges.push(FoldingRange {
                            start_line,
                            start_character: None,
                            end_line,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
            }
        }
    }

    if ranges.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ranges))
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

    // Convert UTF-16 position to byte offset for the format plugin
    let byte_pos = helpers::utf16_to_byte_offset(line_text, position.character as usize);

    // Delegate macro detection to the format plugin
    let macro_info = plugin.find_macro_at_position(line_text, byte_pos)?;

    if let Some(mdef) = plugin.find_macro(&macro_info.name) {
        // Count commas after the macro name to determine active parameter
        let after_name = &line_text[macro_info.name_range.end..];
        let active_param = after_name.matches(',').count() as u32;

        let params_list: Vec<ParameterInformation> = if let Some(args) = mdef.args {
            args.iter()
                .map(|a| ParameterInformation {
                    label: ParameterLabel::Simple(a.label.to_string()),
                    documentation: None,
                })
                .collect()
        } else {
            Vec::new()
        };

        let sig_str = if let Some(args) = mdef.args {
            args.iter().map(|a| a.label).collect::<Vec<_>>().join(", ")
        } else {
            String::new()
        };

        let has_params = !params_list.is_empty();

        // Use the format plugin's signature label — no hardcoded <<>>
        let sig_label = plugin.format_macro_signature_label(mdef.name, &sig_str);

        return Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: sig_label,
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: mdef.description.to_string(),
                })),
                parameters: if has_params { Some(params_list) } else { None },
                active_parameter: if has_params { Some(active_param) } else { None },
            }],
            active_signature: Some(0),
            active_parameter: if has_params { Some(active_param) } else { None },
        });
    }

    // ── Fallback: custom macro / widget ────────────────────────────────
    //
    // If `find_macro()` returned None, the cursor is on a user-defined macro
    // (widget or `Macro.add()` definition). We fall back to
    // `find_custom_macro_detail()` to get the `arg_count` (if known) and
    // synthesize a signature with generic placeholder param names
    // (`arg1`, `arg2`, ...).
    //
    // SugarCube widgets and `Macro.add()` macros don't declare parameter
    // names in their syntax — widgets access args via `_args[0]`, `_args[1]`,
    // etc., and `Macro.add()` functions access them via `this.args[0]`,
    // `this.args[1]`, etc. So we can only show arity, not real param names.
    // The `arg_count` field is populated for widgets (by scanning the body
    // for `_args[N]` references); for `Macro.add()` macros it's `None`.
    //
    // We only fire signature help when `arg_count` is `Some(n)` where `n > 0`.
    // For argless macros (n=0) or unknown arity (None), there's nothing useful
    // to show in a signature popup — the user can still get hover info by
    // hovering over the macro name.
    if let Some(detail) = plugin.find_custom_macro_detail(&macro_info.name) {
        // Only fire signature help when we know the macro takes at least 1 arg.
        // For argless macros (arg_count = Some(0)) or unknown (None), return None.
        let n = detail.arg_count.unwrap_or(0);
        if n == 0 {
            return None;
        }

        let after_name = &line_text[macro_info.name_range.end..];
        let active_param = after_name.matches(',').count() as u32;

        // Synthesize placeholder params from arg_count.
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

        let sig_label = plugin.format_macro_signature_label(&macro_info.name, &sig_str);

        return Some(SignatureHelp {
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
        });
    }

    None
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
                m.insert(uri.clone(), parse_result.token_groups);
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
}
