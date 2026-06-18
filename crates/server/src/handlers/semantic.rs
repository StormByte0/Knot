//! Semantic handlers: document_symbol, workspace_symbol,
//! semantic_tokens_full, inlay_hint, code_lens.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::AnalysisEngine;
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;

// ---------------------------------------------------------------------------
// Semantic token encoding helpers
// ---------------------------------------------------------------------------

/// Intermediate token used during semantic-token conversion.
struct SemTok {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    token_modifiers: u32,
}

/// Convert passage-relative token groups to the intermediate `SemTok`
/// representation (line/character based).
///
/// Each `PassageTokenGroup` contains tokens with passage-relative byte
/// offsets (0 = the `::` prefix of the passage header). We add
/// `passage_offset` to convert to document-absolute byte offsets, then
/// convert to LSP line/character positions.
///
/// The `length` field from the format plugin is in byte lengths. We convert:
/// - `passage_offset + start` → LSP Position via `byte_offset_to_position`
/// - `length` → UTF-16 code unit count (LSP requires UTF-16, not byte length)
fn convert_semantic_tokens(
    text: &str,
    token_groups: &[fmt_plugin::PassageTokenGroup],
) -> Vec<SemTok> {
    let mut tokens = Vec::new();
    let text_len = text.len();
    let line_count = if text.is_empty() { 0u32 } else { text.lines().count() as u32 };

    for group in token_groups {
        for pt in &group.tokens {
            // Resolve passage-relative offset to document-absolute
            let doc_absolute_start = group.passage_offset + pt.start;

            // Clamp byte offsets to document length to prevent out-of-bounds access
            let safe_start = doc_absolute_start.min(text_len);
            let safe_end = (doc_absolute_start + pt.length).min(text_len);
            
            // Skip zero-length tokens (can happen from incorrect position math)
            if safe_start >= safe_end {
                continue;
            }

            let pos = helpers::byte_offset_to_position(text, safe_start);
            
            // Skip tokens whose line exceeds the document
            if pos.line >= line_count {
                continue;
            }

            let token_type = map_token_type(&pt.token_type);
            let modifiers = map_token_modifier(&pt.modifier);

            // Convert byte length to UTF-16 code unit length for the LSP wire format.
            let token_text = &text[safe_start..safe_end];
            let utf16_length: u32 = token_text.chars()
                .map(|c| if (c as u32) < 0x10000 { 1u32 } else { 2u32 })
                .sum();

            // Clamp start_char to the line length to prevent "end character > model.getLineLength"
            let line_text = text.lines().nth(pos.line as usize).unwrap_or("");
            let line_utf16_len = helpers::utf16_len(line_text);
            let clamped_char = pos.character.min(line_utf16_len);
            let clamped_length = utf16_length.min(line_utf16_len.saturating_sub(clamped_char));
            
            // Skip if the token would have zero or negative length after clamping
            if clamped_length == 0 {
                continue;
            }

            tokens.push(SemTok {
                line: pos.line,
                start_char: clamped_char,
                length: clamped_length,
                token_type,
                token_modifiers: modifiers,
            });
        }
    }

    tokens
}

/// Map a `knot_formats::plugin::SemanticTokenType` to the LSP legend index.
///
/// Delegates to `SemanticTokenType::legend_index()` which derives the index
/// from the single-source-of-truth ordering in `all_types()`. No hardcoded
/// constants needed — adding a new variant to the enum is the only change.
fn map_token_type(tt: &fmt_plugin::SemanticTokenType) -> u32 {
    tt.legend_index()
}

/// Map an optional `SemanticTokenModifier` to the LSP modifier bitset.
///
/// Delegates to `SemanticTokenModifier::bit()` which derives the bit
/// position from the single-source-of-truth ordering in `all_modifiers()`.
fn map_token_modifier(modifier: &Option<fmt_plugin::SemanticTokenModifier>) -> u32 {
    match modifier {
        Some(m) => m.bit(),
        None => 0,
    }
}

/// Delta-encode semantic tokens into the LSP wire format.
fn encode_semantic_tokens(tokens: &[SemTok]) -> Vec<lsp_types::SemanticToken> {
    let mut sorted: Vec<&SemTok> = tokens.iter().collect();
    sorted.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.start_char.cmp(&b.start_char)));

    let mut data = Vec::with_capacity(sorted.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for tok in sorted {
        let delta_line = tok.line - prev_line;
        let delta_start = if delta_line == 0 {
            tok.start_char - prev_start
        } else {
            tok.start_char
        };

        data.push(lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length: tok.length,
            token_type: tok.token_type,
            token_modifiers_bitset: tok.token_modifiers,
        });

        prev_line = tok.line;
        prev_start = tok.start_char;
    }

    data
}

// ---------------------------------------------------------------------------
// Handler functions
// ---------------------------------------------------------------------------

pub(crate) async fn document_symbol(
    state: &ServerState,
    params: DocumentSymbolParams,
) -> Result<Option<DocumentSymbolResponse>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    // Use workspace passage data for span-based resolution.
    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut symbols = Vec::new();
    let passages = &doc.passages;

    for (i, passage) in passages.iter().enumerate() {
        let name = passage.name.clone();

        let kind = if name == "StoryData" || name == "StoryTitle" {
            SymbolKind::CONSTANT
        } else {
            SymbolKind::MODULE
        };

        // Extract tags detail directly from passage data
        let detail = if passage.tags.is_empty() {
            None
        } else {
            Some(format!("Tags: {}", passage.tags.join(", ")))
        };

        // Full range: from passage start to just before the next passage
        // (or end of document for the last passage).
        let full_range = {
            let start_offset = passage.abs_offset(passage.span.start).min(text.len());
            let end_offset = if i + 1 < passages.len() {
                passages[i + 1].abs_offset(passages[i + 1].span.start).min(text.len())
            } else {
                text.len()
            };
            helpers::byte_range_to_lsp_range(text, &(start_offset..end_offset))
        };

        // Selection range: just the passage name within the header.
        // Use header_name_span when available; fall back to computing
        // from the header line using the header parser.
        let selection_range = passage
            .header_name_span
            .as_ref()
            .map(|name_span| helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span)))
            .unwrap_or_else(|| {
                let span_start = passage.abs_offset(passage.span.start).min(text.len());
                let line_end = text[span_start..]
                    .find('\n')
                    .map(|n| span_start + n)
                    .unwrap_or(text.len());
                let header_line = &text[span_start..line_end];
                let after_colons = header_line.strip_prefix("::").unwrap_or(header_line);
                if let Some(name_range) = knot_formats::header::passage_name_range_in_header(after_colons) {
                    let prefix_len = helpers::utf16_len(&header_line[..2]);
                    let start_char = prefix_len + helpers::utf16_len(&after_colons[..name_range.start]);
                    let end_char = start_char + helpers::utf16_len(&after_colons[name_range.start..name_range.end]);
                    Range {
                        start: Position { line: full_range.start.line, character: start_char },
                        end: Position { line: full_range.start.line, character: end_char },
                    }
                } else {
                    Range {
                        start: Position { line: full_range.start.line, character: 0 },
                        end: Position { line: full_range.start.line, character: helpers::utf16_len(header_line) },
                    }
                }
            });

        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name,
            detail,
            kind,
            tags: None,
            deprecated: None,
            range: full_range,
            selection_range,
            children: None,
        });
    }

    if symbols.is_empty() {
        Ok(None)
    } else {
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }
}

pub(crate) async fn symbol(
    state: &ServerState,
    params: WorkspaceSymbolParams,
) -> Result<Option<Vec<SymbolInformation>>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let query = params.query.to_lowercase();

    let mut symbols = Vec::new();

    for doc in inner.workspace.documents() {
        let text = match inner.open_documents.get(&doc.uri) {
            Some(t) => t,
            None => continue,
        };

        for passage in &doc.passages {
            // Filter by query (case-insensitive substring match)
            if !query.is_empty() && !passage.name.to_lowercase().contains(&query) {
                continue;
            }

            let kind = if passage.name == "StoryData" || passage.name == "StoryTitle" {
                SymbolKind::CONSTANT
            } else {
                SymbolKind::MODULE
            };

            // Use header_name_span for the location range when available,
            // otherwise compute from the passage span (header line only).
            let range = passage
                .header_name_span
                .as_ref()
                .map(|name_span| helpers::byte_range_to_lsp_range(text, &passage.abs_range(name_span)))
                .unwrap_or_else(|| {
                    let span_start = passage.abs_offset(passage.span.start).min(text.len());
                    let line_end = text[span_start..]
                        .find('\n')
                        .map(|n| span_start + n)
                        .unwrap_or(text.len());
                    helpers::byte_range_to_lsp_range(text, &(span_start..line_end))
                });

            #[allow(deprecated)]
            symbols.push(SymbolInformation {
                name: passage.name.clone(),
                kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri: doc.uri.clone(),
                    range,
                },
                container_name: None,
            });
        }
    }

    if symbols.is_empty() {
        Ok(None)
    } else {
        Ok(Some(symbols))
    }
}

pub(crate) async fn semantic_tokens_full(
    state: &ServerState,
    params: SemanticTokensParams,
) -> Result<Option<SemanticTokensResult>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    // Read tokens from the cache (populated at parse time in did_open,
    // did_change, and indexing). This avoids re-parsing on every request,
    // which is critical for avoiding deadlock when FormatPluginMut (Phase 4)
    // requires the write lock for parsing.
    match inner.semantic_tokens.get(&uri) {
        Some(cached_tokens) => {
            // We need the document text to convert byte-offset tokens to
            // LSP line/character positions. If the text is unavailable
            // (shouldn't happen in normal operation), return empty tokens.
            let text = match inner.open_documents.get(&uri) {
                Some(t) => t.clone(),
                None => {
                    return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                        result_id: None,
                        data: vec![],
                    })));
                }
            };

            let tokens = convert_semantic_tokens(&text, cached_tokens);

            let data = encode_semantic_tokens(&tokens);

            // Add result_id based on document version for delta support
            let result_id = inner.doc_versions.get(&uri).map(|v| v.to_string());

            Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id,
                data,
            })))
        }
        None => {
            // No cached tokens — return JSON null. This tells VS Code
            // "tokens not available yet, re-request after refresh."
            // VS Code will re-request after receiving
            // workspace/semanticTokens/refresh, which is sent by the
            // debounced refresh or the formatDetected cascade.
            // This is an upgrade from Phase 2 (which returned empty
            // SemanticTokens) — returning null ensures VS Code doesn't
            // cache empty tokens and suppress future re-requests.
            Ok(None)
        }
    }
}

pub(crate) async fn code_lens(
    state: &ServerState,
    params: CodeLensParams,
) -> Result<Option<Vec<CodeLens>>, tower_lsp::jsonrpc::Error> {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    let mut lenses = Vec::new();

    for passage in &doc.passages {
        let outgoing = inner.workspace.graph.outgoing_neighbors(&passage.name).len();
        let incoming = helpers::count_incoming_links(&inner.workspace, &passage.name);

        if outgoing > 0 || incoming > 0 {
            // Compute range from the passage header line using span data
            let span_start = passage.abs_offset(passage.span.start).min(text.len());
            let line_end = text[span_start..]
                .find('\n')
                .map(|n| span_start + n)
                .unwrap_or(text.len());
            let range = helpers::byte_range_to_lsp_range(text, &(span_start..line_end));

            lenses.push(CodeLens {
                range,
                command: Some(Command {
                    title: if outgoing > 0 {
                        format!("{} links →", outgoing)
                    } else {
                        format!("{} refs", incoming)
                    },
                    command: String::new(),
                    arguments: None,
                }),
                data: None,
            });
        }
    }

    if lenses.is_empty() {
        Ok(None)
    } else {
        Ok(Some(lenses))
    }
}

pub(crate) async fn inlay_hint(
    state: &ServerState,
    params: InlayHintParams,
) -> Result<Option<Vec<InlayHint>>, tower_lsp::jsonrpc::Error> {
    // Short-circuit if the server is shutting down — the dataflow analysis
    // below is the single most expensive handler; if the stream is about to
    // be destroyed, there is no point running it.
    if state.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(None);
    }

    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    let Some(text) = inner.open_documents.get(&uri) else {
        return Ok(None);
    };

    let start_passage = inner
        .workspace
        .metadata
        .as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");

    let passage_data = AnalysisEngine::collect_passage_data(&inner.workspace);
    let core_seed = AnalysisEngine::collect_special_passage_initializers(&inner.workspace, &passage_data);
    let format = inner.workspace.resolve_format();
    let seed_init = helpers::supplement_seed_with_format_specials(
        core_seed, &inner.workspace, &inner.format_registry, format
    );
    let flow_states = AnalysisEngine::run_dataflow_from_engine(&inner.workspace, start_passage, &passage_data, &seed_init);

    let mut hints = Vec::new();

    // Use workspace passage data for span-based resolution.
    let Some(doc) = inner.workspace.get_document(&uri) else {
        return Ok(None);
    };

    for passage in &doc.passages {
        if let Some(state) = flow_states.get(&passage.name) {
            // ── "initialized" hint ─────────────────────────────────────
            //
            // Shows which variables are available at passage entry. Truncated
            // to 5 names to avoid overwhelming the header line. Compact format:
            // `// init: $a, $b, $c, …`
            let mut init_vars: Vec<&String> = state.entry.iter().collect();
            init_vars.sort();

            if !init_vars.is_empty() {
                let display_count = init_vars.len().min(5);
                let names: Vec<&str> = init_vars[..display_count]
                    .iter()
                    .map(|v| v.as_str())
                    .collect();
                let suffix = if init_vars.len() > 5 {
                    format!(", … +{}", init_vars.len() - 5)
                } else {
                    String::new()
                };
                let label = format!("// init: {}{}", names.join(", "), suffix);
                // Position the hint at the start of the passage header
                let position = helpers::byte_offset_to_position(text, passage.abs_offset(passage.span.start).min(text.len()));
                hints.push(InlayHint {
                    position,
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(true),
                    padding_right: Some(true),
                    data: None,
                });
            }

            // ── "may be uninitialized" hint ────────────────────────────
            //
            // Shows variables that are read but may not have been initialized
            // at this point in the flow. This is the more actionable hint —
            // it warns about potential bugs. Truncated to 5 names.
            let mut local_init = state.entry.clone();
            let mut uninit_vars = Vec::new();
            for var in passage.vars_sorted_by_span() {
                if var.is_temporary { continue; }
                match var.kind {
                    knot_core::passage::VarKind::Read => {
                        if !local_init.contains(&var.name)
                            && !uninit_vars.contains(&var.name) {
                                uninit_vars.push(var.name.clone());
                            }
                    }
                    knot_core::passage::VarKind::Init => {
                        local_init.insert(var.name.clone());
                    }
                }
            }

            if !uninit_vars.is_empty() {
                let display_count = uninit_vars.len().min(5);
                let names = &uninit_vars[..display_count];
                let suffix = if uninit_vars.len() > 5 {
                    format!(", … +{}", uninit_vars.len() - 5)
                } else {
                    String::new()
                };
                let label = format!("// uninit: {}{}", names.join(", "), suffix);
                let position = helpers::byte_offset_to_position(text, passage.abs_offset(passage.span.start).min(text.len()));
                hints.push(InlayHint {
                    position,
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::PARAMETER),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(true),
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
    }

    if hints.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hints))
    }
}
