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

    // Pre-compute line start byte offsets for efficient line lookup.
    // line_starts[i] = byte offset of the start of line i.
    let mut line_starts: Vec<usize> = vec![0];
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(offset + 1);
        }
    }

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

            let token_type = map_token_type(&pt.token_type);
            let modifiers = map_token_modifier(&pt.modifier);

            // ── Split multi-line tokens into per-line tokens ──────────
            //
            // LSP semantic tokens are single-line: each token has a (line,
            // start_char, length) where length is on that SAME line. A
            // token that spans multiple lines (e.g., a multi-line /* */
            // comment) must be split into one token per line, each with
            // the same type and modifiers.
            //
            // Without this split, VS Code only colors the first line and
            // silently drops the rest — the token's length gets clamped to
            // the end of line 1.

            let start_pos = helpers::byte_offset_to_position(text, safe_start);
            let end_pos = helpers::byte_offset_to_position(text, safe_end);

            // Skip if the start line exceeds the document
            if start_pos.line >= line_count {
                continue;
            }

            // Iterate over each line the token spans
            let mut current_line = start_pos.line;
            loop {
                if current_line >= line_count {
                    break;
                }

                // Get the byte range of this line
                let line_start_byte = line_starts.get(current_line as usize)
                    .copied()
                    .unwrap_or(text_len);
                let line_end_byte = if (current_line + 1) as usize >= line_starts.len() {
                    // Last line — check if text ends with newline
                    if line_start_byte < text_len && text.as_bytes()[line_start_byte] == b'\n' {
                        line_start_byte + 1 // empty line (just newline)
                    } else {
                        text_len
                    }
                } else {
                    line_starts[(current_line + 1) as usize]
                };

                // The token's portion on this line:
                //   - Start: max(safe_start, line_start_byte)
                //   - End: min(safe_end, line_end_byte_excluding_newline)
                let line_content_end = if line_end_byte > 0 && text.as_bytes().get(line_end_byte - 1) == Some(&b'\n') {
                    line_end_byte - 1 // exclude the newline
                } else {
                    line_end_byte
                };

                let segment_start = safe_start.max(line_start_byte);
                let segment_end = safe_end.min(line_content_end);

                if segment_start < segment_end {
                    // Calculate the UTF-16 start character on this line
                    let line_text = &text[line_start_byte..line_content_end.min(text_len)];
                    let char_offset_in_line = segment_start.saturating_sub(line_start_byte);
                    let line_prefix = &line_text[..char_offset_in_line.min(line_text.len())];
                    let start_char_utf16 = helpers::utf16_len(line_prefix) as u32;

                    // Calculate the UTF-16 length of this segment
                    let segment_text = &text[segment_start..segment_end];
                    let segment_utf16_len: u32 = segment_text.chars()
                        .map(|c| if (c as u32) < 0x10000 { 1u32 } else { 2u32 })
                        .sum();

                    if segment_utf16_len > 0 {
                        tokens.push(SemTok {
                            line: current_line,
                            start_char: start_char_utf16,
                            length: segment_utf16_len,
                            token_type,
                            token_modifiers: modifiers,
                        });
                    }
                }

                // Move to the next line
                current_line += 1;
                if current_line > end_pos.line {
                    break;
                }
            }
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
    Ok(document_symbol_inner(&inner, &uri))
}

/// Inner synchronous implementation of `document_symbol`.
///
/// Extracted so tests can call it directly without constructing a full
/// `ServerState` (which requires a `tower_lsp::Client` handle). Same pattern
/// as `structure::signature_help_inner`.
fn document_symbol_inner(
    inner: &crate::state::ServerStateInner,
    uri: &url::Url,
) -> Option<DocumentSymbolResponse> {
    let text = inner.open_documents.get(uri)?;

    // Use workspace passage data for span-based resolution.
    let doc = inner.workspace.get_document(uri)?;

    let mut symbols = Vec::new();

    // `doc.passages` is stored in document source order (the SugarCube
    // parse pipeline re-sorts by `passage_offset` after the
    // processing-priority sort has done its job). This means
    // `passages[i+1]` is always the next passage in the file, so the
    // "full range = this passage start .. next passage start" computation
    // below produces well-formed ranges.
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
            // Defensive: clamp to ensure end >= start. If the workspace's
            // passage data is somehow inconsistent (e.g., stale after a
            // rapid edit), fall back to start..start so VS Code's
            // `selectionRange must be contained in fullRange` validator
            // never sees an inverted range.
            let safe_end = end_offset.max(start_offset);
            helpers::byte_range_to_lsp_range(text, &(start_offset..safe_end))
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

        // Defensive containment guarantee: VS Code's `asDocumentSymbol`
        // converter throws `selectionRange must be contained in fullRange`
        // and rejects the entire `textDocument/documentSymbol` response if
        // ANY symbol violates the constraint. Clamp `selection_range` into
        // `full_range` so we never trigger that — even if upstream span
        // data is momentarily inconsistent (e.g., during a rapid edit when
        // `header_name_span` was computed against a slightly different
        // text version than `open_documents`).
        let selection_range = clamp_range_to(&selection_range, &full_range);

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
        None
    } else {
        Some(DocumentSymbolResponse::Nested(symbols))
    }
}

/// Clamp `inner` so it is fully contained in `outer`.
///
/// The LSP spec requires `DocumentSymbol.selectionRange` to be contained in
/// `DocumentSymbol.range`. VS Code's `asDocumentSymbol` validator enforces
/// this by throwing `selectionRange must be contained in fullRange` and
/// rejecting the entire `textDocument/documentSymbol` response if any
/// symbol violates it.
///
/// This helper performs a defensive clamp so that even when upstream span
/// data is momentarily inconsistent (e.g., during a rapid edit when
/// `header_name_span` was computed against a different text version than
/// `open_documents`), the LSP response is still valid. The clamp is a
/// last-resort safety net — under normal operation the input ranges
/// already satisfy the containment constraint and the clamp is a no-op.
fn clamp_range_to(inner: &Range, outer: &Range) -> Range {
    let start = clamp_position_to(inner.start, outer.start, outer.end);
    let end = clamp_position_to(inner.end, start, outer.end);
    // After clamping `start` to `outer`, `end` must still be >= `start`.
    let end = if end.line < start.line || (end.line == start.line && end.character < start.character) {
        start
    } else {
        end
    };
    Range { start, end }
}

/// Clamp `pos` so it is within `[lo, hi]` (inclusive on both ends).
fn clamp_position_to(pos: Position, lo: Position, hi: Position) -> Position {
    if pos.lt(&lo) { lo }
    else if pos.gt(&hi) { hi }
    else { pos }
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

            // Note: "uninitialized variable" inlay hints were removed.
            // SugarCube variables are persistent (stored in State.variables
            // for the entire session). The simplistic per-passage flow
            // analysis produced false positives for any variable set in
            // StoryInit or an earlier passage. The format plugin's own
            // `compute_variable_diagnostics()` handles this correctly via
            // the state variable registry + graph BFS — no ghost text needed.
        }
    }

    if hints.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hints))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod document_symbol_tests {
    use super::*;
    use crate::state::ServerStateInner;
    use knot_core::editing::DebounceTimer;
    use knot_core::Document;
    use knot_core::Workspace;
    use knot_formats::plugin::FormatRegistry;
    use url::Url;

    /// Build a `ServerStateInner` fixture from a single twee source file.
    /// Mirrors the helper used in `signature_help_tests` in `structure.rs`.
    fn build_state(src: &str) -> (ServerStateInner, Url) {
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let mut registry = FormatRegistry::with_defaults();
        let format = knot_core::passage::StoryFormat::SugarCube;
        let parse_result = {
            let plugin = registry
                .get_mut(&format)
                .expect("SugarCube plugin must be registered");
            plugin.parse_mut(&uri, src)
        };

        let workspace = {
            let mut ws = Workspace::new(Url::parse("file:///project/").unwrap());
            ws.config.format = Some("SugarCube".to_string());
            let mut doc = Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
            for passage in parse_result.passages {
                doc.passages.push(passage);
            }
            ws.insert_document(doc);
            ws
        };

        let inner = ServerStateInner {
            workspace,
            format_registry: registry,
            debounce: DebounceTimer::new(),
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

    /// Run `document_symbol` against the fixture and return the nested symbols.
    fn get_symbols(src: &str) -> Vec<DocumentSymbol> {
        let (inner, uri) = build_state(src);
        let result = document_symbol_inner(&inner, &uri);
        match result {
            Some(DocumentSymbolResponse::Nested(syms)) => syms,
            Some(DocumentSymbolResponse::Flat(_)) => panic!("expected Nested response, got Flat"),
            None => Vec::new(),
        }
    }

    /// Assert that `inner` is contained in `outer` per the LSP
    /// `selectionRange must be contained in fullRange` rule.
    fn assert_contained(outer: &Range, inner: &Range, label: &str) {
        let start_ok = inner.start.line > outer.start.line
            || (inner.start.line == outer.start.line && inner.start.character >= outer.start.character);
        let end_ok = inner.end.line < outer.end.line
            || (inner.end.line == outer.end.line && inner.end.character <= outer.end.character);
        assert!(
            start_ok && end_ok,
            "{label}: selectionRange {:?} not contained in fullRange {:?}",
            inner,
            outer
        );
    }

    /// Regression test for the `selectionRange must be contained in fullRange`
    /// crash that VS Code reported on the sugarcube-testbed project.
    ///
    /// Root cause: the SugarCube format plugin's `parse_full` ran
    /// `classifier::sort_for_processing` (which reorders passages by
    /// processing priority — scripts/init first, then specials, then
    /// regulars) and stored the passages in that order in `doc.passages`.
    /// The `document_symbol` handler computed each symbol's `full_range`
    /// as `passages[i].start .. passages[i+1].start`, which only works in
    /// source order. When passages were out of order, `passages[i+1].start`
    /// could be smaller than `passages[i].start`, producing an inverted
    /// range that VS Code rejected.
    ///
    /// Fix: `parse_full` now re-sorts `result.passages` by `passage_offset`
    /// (document source order) after the processing-priority sort has done
    /// its job for registry population. The LSP handler trusts this
    /// invariant and no longer needs its own sort.
    ///
    /// This test uses a minimal input that triggers the same out-of-order
    /// condition: a regular passage followed by `StoryData` (which has
    /// higher processing priority and would have been sorted earlier
    /// without the fix). Without the fix, the regular passage's full_range
    /// would be `StoryData.start..end_of_doc` (inverted), crashing the
    /// handler.
    #[test]
    fn regression_document_symbol_passages_out_of_source_order() {
        // `:: StoryData` is a special passage with high processing priority.
        // `:: RegularBody` is a normal passage with low priority.
        // The plugin's `sort_for_processing` reorders `StoryData` BEFORE
        // `RegularBody` during parsing (so its metadata is available to
        // the registry), but `parse_full` then re-sorts the final
        // `result.passages` by `passage_offset` (source order). Without
        // that final re-sort, `RegularBody`'s `full_range` would be
        // `StoryData.start..end_of_doc` (inverted), crashing the handler.
        let src = ":: RegularBody\nSome text.\n\n:: StoryData\n{\"format\":\"SugarCube\"}\n";

        let symbols = get_symbols(src);
        assert!(!symbols.is_empty(), "should produce at least one symbol");

        // Every symbol must satisfy the LSP containment constraint that
        // VS Code's `asDocumentSymbol` validator enforces.
        for sym in &symbols {
            assert_contained(&sym.range, &sym.selection_range, &format!("passage {:?}", sym.name));
        }

        // Sanity: both passages should appear in the outline, ordered by
        // source position (RegularBody first, StoryData second).
        assert_eq!(symbols.len(), 2, "should have 2 symbols: RegularBody + StoryData");
        assert_eq!(symbols[0].name, "RegularBody");
        assert_eq!(symbols[1].name, "StoryData");
        // Source order: RegularBody on line 0, StoryData on line 3.
        assert_eq!(symbols[0].range.start.line, 0);
        assert_eq!(symbols[1].range.start.line, 3);
        // RegularBody's full_range should extend up to StoryData's start
        // (NOT wrap around past the end of the file).
        assert!(
            symbols[0].range.end.line >= 1,
            "RegularBody full_range should cover its body, got end line {}",
            symbols[0].range.end.line
        );
    }

    /// Cross-check: when passages are already in source order (no specials),
    /// the fix must not change the visible outline — full_range for each
    /// passage should still extend from its header to the start of the next.
    #[test]
    fn document_symbol_passages_in_source_order_unchanged() {
        let src = ":: First\nbody 1\n\n:: Second\nbody 2\n\n:: Third\nbody 3\n";
        let symbols = get_symbols(src);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "First");
        assert_eq!(symbols[1].name, "Second");
        assert_eq!(symbols[2].name, "Third");
        for sym in &symbols {
            assert_contained(&sym.range, &sym.selection_range, &format!("passage {:?}", sym.name));
        }
        // First passage extends up to Second's header line.
        assert!(symbols[0].range.end.line >= 1);
        // Last passage extends to end of document.
        assert!(symbols[2].range.end.line >= 5);
    }

    /// Stress test: simulate the exact testbed shape that originally crashed.
    /// Multiple special passages (`StoryData`, `StoryTitle`) interspersed
    /// with regular passages, all out of source order after
    /// `sort_for_processing` runs.
    #[test]
    fn regression_document_symbol_testbed_shape_mixed_specials() {
        let src = "\
:: StoryData
{\"format\":\"SugarCube\",\"start\":\"Start\"}

:: StoryTitle
My Story

:: Start
Welcome.

:: Forest
You enter the forest.
";
        let symbols = get_symbols(src);
        assert_eq!(symbols.len(), 4, "should have 4 passages");
        for sym in &symbols {
            assert_contained(&sym.range, &sym.selection_range, &format!("passage {:?}", sym.name));
        }
        // Source-order check: StoryData on line 0, StoryTitle on line 3,
        // Start on line 6, Forest on line 9.
        let line_of = |name: &str| -> u32 {
            symbols.iter().find(|s| s.name == name).unwrap().range.start.line
        };
        assert_eq!(line_of("StoryData"), 0);
        assert_eq!(line_of("StoryTitle"), 3);
        assert_eq!(line_of("Start"), 6);
        assert_eq!(line_of("Forest"), 9);
    }

    /// Unit test for the `clamp_range_to` defensive helper.
    #[test]
    fn clamp_range_to_inner_already_contained_is_noop() {
        let outer = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 10, character: 0 },
        };
        let inner = Range {
            start: Position { line: 3, character: 5 },
            end: Position { line: 4, character: 2 },
        };
        let clamped = clamp_range_to(&inner, &outer);
        assert_eq!(clamped, inner);
    }

    #[test]
    fn clamp_range_to_inner_extends_below_outer_is_clamped() {
        let outer = Range {
            start: Position { line: 5, character: 0 },
            end: Position { line: 10, character: 0 },
        };
        let inner = Range {
            start: Position { line: 2, character: 4 },
            end: Position { line: 7, character: 0 },
        };
        let clamped = clamp_range_to(&inner, &outer);
        assert_eq!(clamped.start, outer.start);
        assert_eq!(clamped.end, inner.end);
    }

    #[test]
    fn clamp_range_to_inner_extends_above_outer_is_clamped() {
        let outer = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 5, character: 0 },
        };
        let inner = Range {
            start: Position { line: 3, character: 0 },
            end: Position { line: 8, character: 4 },
        };
        let clamped = clamp_range_to(&inner, &outer);
        assert_eq!(clamped.start, inner.start);
        assert_eq!(clamped.end, outer.end);
    }

    #[test]
    fn clamp_range_to_inner_completely_outside_outer_collapses_to_start() {
        // If `inner` is entirely below `outer`, clamping both endpoints to
        // `outer.start` produces an empty range at `outer.start` —
        // `selectionRange` is contained in `fullRange` (vacuously, as an
        // empty range), satisfying the LSP invariant.
        let outer = Range {
            start: Position { line: 10, character: 0 },
            end: Position { line: 20, character: 0 },
        };
        let inner = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 5, character: 0 },
        };
        let clamped = clamp_range_to(&inner, &outer);
        assert_eq!(clamped.start, outer.start);
        assert_eq!(clamped.end, outer.start);
    }
}
