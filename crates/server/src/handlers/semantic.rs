//! Semantic handlers: document_symbol, workspace_symbol,
//! semantic_tokens_full, inlay_hint, code_lens.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::AnalysisEngine;
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;

// ---------------------------------------------------------------------------
// Semantic token constants (used by encoding functions)
// ---------------------------------------------------------------------------

/// Token-type indices — must match the order in the legend we advertise.
pub(crate) const ST_PASSAGE_HEADER: u32 = 0;
pub(crate) const ST_LINK: u32 = 1;
pub(crate) const ST_MACRO: u32 = 2;
pub(crate) const ST_VARIABLE: u32 = 3;
pub(crate) const ST_STRING: u32 = 4;
pub(crate) const ST_NUMBER: u32 = 5;
pub(crate) const ST_COMMENT: u32 = 6;
pub(crate) const ST_TAG: u32 = 7;
pub(crate) const ST_KEYWORD: u32 = 8;
pub(crate) const ST_BOOLEAN: u32 = 9;

/// Token-modifier indices.
pub(crate) const SM_DEFINITION: u32 = 1 << 0;
pub(crate) const SM_READONLY: u32 = 1 << 1;
pub(crate) const SM_DEPRECATED: u32 = 1 << 2;
pub(crate) const SM_CONTROLFLOW: u32 = 1 << 3;

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

/// Convert format-plugin semantic tokens (byte-offset based) to the
/// intermediate `SemTok` representation (line/character based).
///
/// The `start` and `length` fields from the format plugin are in **byte**
/// offsets and byte lengths respectively. We convert:
/// - `start` → LSP Position via `byte_offset_to_position` (UTF-16 character)
/// - `length` → UTF-16 code unit count (LSP requires UTF-16, not byte length)
fn convert_semantic_tokens(
    text: &str,
    plugin_tokens: &[fmt_plugin::SemanticToken],
) -> Vec<SemTok> {
    let mut tokens = Vec::new();

    for pt in plugin_tokens {
        let pos = helpers::byte_offset_to_position(text, pt.start);
        let token_type = map_token_type(&pt.token_type);
        let modifiers = map_token_modifier(&pt.modifier);

        // Convert byte length to UTF-16 code unit length for the LSP wire format.
        let safe_start = pt.start.min(text.len());
        let safe_end = (safe_start + pt.length).min(text.len());
        let token_text = &text[safe_start..safe_end];
        let utf16_length: u32 = token_text.chars()
            .map(|c| if (c as u32) < 0x10000 { 1u32 } else { 2u32 })
            .sum();

        tokens.push(SemTok {
            line: pos.line,
            start_char: pos.character,
            length: utf16_length,
            token_type,
            token_modifiers: modifiers,
        });
    }

    tokens
}

/// Map a `knot_formats::plugin::SemanticTokenType` to the LSP legend index.
fn map_token_type(tt: &fmt_plugin::SemanticTokenType) -> u32 {
    match tt {
        fmt_plugin::SemanticTokenType::PassageHeader => ST_PASSAGE_HEADER,
        fmt_plugin::SemanticTokenType::Link => ST_LINK,
        fmt_plugin::SemanticTokenType::Macro => ST_MACRO,
        fmt_plugin::SemanticTokenType::Variable => ST_VARIABLE,
        fmt_plugin::SemanticTokenType::String => ST_STRING,
        fmt_plugin::SemanticTokenType::Number => ST_NUMBER,
        fmt_plugin::SemanticTokenType::Boolean => ST_BOOLEAN,
        fmt_plugin::SemanticTokenType::Comment => ST_COMMENT,
        fmt_plugin::SemanticTokenType::Tag => ST_TAG,
        fmt_plugin::SemanticTokenType::Keyword => ST_KEYWORD,
    }
}

/// Map an optional `SemanticTokenModifier` to the LSP modifier bitset.
fn map_token_modifier(modifier: &Option<fmt_plugin::SemanticTokenModifier>) -> u32 {
    match modifier {
        Some(fmt_plugin::SemanticTokenModifier::Definition) => SM_DEFINITION,
        Some(fmt_plugin::SemanticTokenModifier::ReadOnly) => SM_READONLY,
        Some(fmt_plugin::SemanticTokenModifier::Deprecated) => SM_DEPRECATED,
        Some(fmt_plugin::SemanticTokenModifier::ControlFlow) => SM_CONTROLFLOW,
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

    let mut symbols = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = helpers::parse_passage_name_from_header(&line[2..]);

            // Find the end of this passage (next :: or end of file)
            let end_line = text
                .lines()
                .enumerate()
                .skip(line_idx + 1)
                .find(|(_, l)| l.starts_with("::"))
                .map(|(i, _)| i as u32 - 1)
                .unwrap_or_else(|| text.lines().count() as u32 - 1);

            let kind = if name == "StoryData" || name == "StoryTitle" {
                SymbolKind::CONSTANT
            } else {
                SymbolKind::MODULE
            };

            // Extract tags from the header line if present
            let detail = if let Some(bracket_start) = line[2..].find('[') {
                let header = &line[2..];
                if let Some(bracket_end) = header[bracket_start..].find(']') {
                    let tags = &header[bracket_start + 1..bracket_start + bracket_end];
                    Some(format!("Tags: {}", tags))
                } else {
                    None
                }
            } else {
                None
            };

            // lsp_types 0.94 still requires the `deprecated` field in the struct literal
            // even though it was deprecated in LSP 3.16+ in favor of `tags`.
            #[allow(deprecated)]
            symbols.push(DocumentSymbol {
                name,
                detail,
                kind,
                tags: None,
                deprecated: None,
                range: Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 0,
                    },
                    end: Position {
                        line: end_line,
                        character: 0,
                    },
                },
                selection_range: Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 2, // after "::" (always 2 UTF-16 code units for ASCII)
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: helpers::utf16_len(line),
                    },
                },
                children: None,
            });
        }
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

        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = helpers::parse_passage_name_from_header(&line[2..]);

                // Filter by query (case-insensitive substring match)
                if !query.is_empty() && !name.to_lowercase().contains(&query) {
                    continue;
                }

                let kind = if name == "StoryData" || name == "StoryTitle" {
                    SymbolKind::CONSTANT
                } else {
                    SymbolKind::MODULE
                };

                #[allow(deprecated)]
                symbols.push(SymbolInformation {
                    name,
                    kind,
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: doc.uri.clone(),
                        range: Range {
                            start: Position { line: line_idx as u32, character: 0 },
                            end: Position { line: line_idx as u32, character: helpers::utf16_len(line) },
                        },
                    },
                    container_name: None,
                });
            }
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

    if let Some(text) = inner.open_documents.get(&uri) {
        let format = inner.workspace.resolve_format();
        if let Some(plugin) = inner.format_registry.get(&format) {
            let parse_result = plugin.parse(&uri, text);
            let tokens = convert_semantic_tokens(text, &parse_result.tokens);
            let data = encode_semantic_tokens(&tokens);
            return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data,
            })));
        }
    }

    Ok(None)
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

    let mut lenses = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = helpers::parse_passage_name_from_header(&line[2..]);
            let outgoing = inner.workspace.graph.outgoing_neighbors(&name).len();
            let incoming = helpers::count_incoming_links(&inner.workspace, &name);

            if outgoing > 0 || incoming > 0 {
                lenses.push(CodeLens {
                    range: Range {
                        start: Position { line: line_idx as u32, character: 0 },
                        end: Position { line: line_idx as u32, character: helpers::utf16_len(line) },
                    },
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
    let seed_init = AnalysisEngine::collect_special_passage_initializers(&inner.workspace, &passage_data);
    let flow_states = AnalysisEngine::run_dataflow_from_engine(&inner.workspace, start_passage, &passage_data, &seed_init);

    let mut hints = Vec::new();

    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = helpers::parse_passage_name_from_header(&line[2..]);

            if let Some(state) = flow_states.get(&name) {
                let mut init_vars: Vec<&String> = state.entry.iter().collect();
                init_vars.sort();

                if !init_vars.is_empty() {
                    let label = format!("// initialized: {}", init_vars.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(", "));
                    hints.push(InlayHint {
                        position: Position { line: line_idx as u32, character: 0 },
                        label: InlayHintLabel::String(label),
                        kind: Some(InlayHintKind::TYPE),
                        text_edits: None,
                        tooltip: None,
                        padding_left: Some(true),
                        padding_right: Some(true),
                        data: None,
                    });
                }

                // Check for potentially uninitialized variables
                let mut local_init = state.entry.clone();
                let mut uninit_vars = Vec::new();
                if let Some((_, passage)) = inner.workspace.find_passage(&name) {
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
                }

                if !uninit_vars.is_empty() {
                    let label = format!("// may be uninitialized: {}", uninit_vars.join(", "));
                    hints.push(InlayHint {
                        position: Position { line: line_idx as u32, character: 0 },
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
    }

    if hints.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hints))
    }
}
