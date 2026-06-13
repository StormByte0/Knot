//! Completion handlers: completion, completion_resolve.
//!
//! ## Architecture: format-owned completion building
//!
//! The completion handler is a **thin dispatcher**. All context detection AND
//! completion item construction lives in the active format plugin's
//! `provide_completions()` method. The handler:
//!
//! 1. Calls `plugin.provide_completions(text, workspace, uri, line, char, trigger, tokens)`
//! 2. Receives a list of `FormatCompletionItem` from the format plugin
//! 3. Maps each `FormatCompletionItem` to an `lsp_types::CompletionItem`
//!
//! No format-specific trigger routing, pattern detection, or completion item
//! construction exists in this file. The format plugin owns everything.
//! Adding a new format (Harlowe, Chapbook, Snowman) only requires
//! implementing `provide_completions()` in the format plugin.
//!
//! ## Why FormatCompletionItem instead of CompletionContext?
//!
//! The previous architecture used `CompletionContext` — the plugin detected
//! context and returned an enum variant, then the handler built completion
//! items. This bled format-specific knowledge into the handler and caused
//! bugs (e.g., `$` triggering passage names because the handler's variable
//! builder used stale workspace data instead of the plugin's VariableTree).
//!
//! The new architecture follows the legacy TypeScript adapter pattern where
//! `provideFormatCompletions()` returns `CompletionItem[]` directly. The
//! format plugin builds completions from its own registries (the most
//! accurate data source) and the handler just maps types.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use knot_formats::types::{FormatCompletionKind, FormatInsertTextFormat};
use lsp_types::*;

// ===========================================================================
// Completion handler
// ===========================================================================

pub(crate) async fn completion(
    state: &ServerState,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let uri = helpers::normalize_file_uri(&params.text_document_position.text_document.uri);
    let position = params.text_document_position.position;

    // Determine the trigger character as a single char (if any)
    let trigger = params.context.as_ref()
        .and_then(|ctx| ctx.trigger_character.clone())
        .and_then(|s| s.chars().next());

    let text = match inner.open_documents.get(&uri) {
        Some(t) => t,
        None => return Ok(None),
    };

    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    let token_groups = inner.semantic_tokens.get(&uri).cloned().unwrap_or_default();

    // ── Delegate to format plugin ─────────────────────────────────────
    //
    // The format plugin owns ALL context detection and completion building.
    // It returns FormatCompletionItem values which we map to LSP types.
    let format_items = if let Some(plugin) = plugin {
        plugin.provide_completions(
            text,
            &inner.workspace,
            &uri,
            position.line,
            position.character,
            trigger,
            &token_groups,
        )
    } else {
        Vec::new()
    };

    if format_items.is_empty() {
        return Ok(None);
    }

    // ── Map FormatCompletionItem → lsp_types::CompletionItem ──────────
    let items: Vec<CompletionItem> = format_items
        .into_iter()
        .map(|fi| map_completion_item(fi))
        .collect();

    Ok(Some(CompletionResponse::Array(items)))
}

// ===========================================================================
// Completion resolve handler
// ===========================================================================

pub(crate) async fn completion_resolve(
    state: &ServerState,
    params: CompletionItem,
) -> Result<CompletionItem, tower_lsp::jsonrpc::Error> {
    let inner = state.inner.read().await;
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);

    if let Some(data) = &params.data {
        let comp_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");

        match comp_type {
            "passage" => {
                if let Some((doc, passage)) = inner.workspace.find_passage(name) {
                    let links_count = passage.links.len();
                    let incoming = helpers::count_incoming_links(&inner.workspace, name);
                    let doc_markdown = format!(
                        "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                        name,
                        doc.uri.as_str(),
                        links_count,
                        incoming,
                        if passage.tags.is_empty() { "none".to_string() } else { passage.tags.join(", ") }
                    );
                    return Ok(CompletionItem {
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc_markdown,
                        })),
                        ..params
                    });
                }
            }
            "variable" => {
                let is_temp = data.get("is_temp").and_then(|v| v.as_bool()).unwrap_or(false);
                let format_desc = plugin
                    .and_then(|p| {
                        let sigils = p.variable_sigils();
                        sigils.iter().find(|s| (s.sigil == '_') == is_temp)
                            .map(|s| s.description)
                            .or_else(|| sigils.first().map(|s| s.description))
                    })
                    .unwrap_or(if is_temp { "temporary variable" } else { "variable" });

                let doc_markdown = format!(
                    "**{}**\n\n{} variable — {}",
                    name,
                    format_desc,
                    if is_temp { "scoped to the current passage" } else { "persists across passages" }
                );
                return Ok(CompletionItem {
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc_markdown,
                    })),
                    ..params
                });
            }
            "macro" => {
                if let Some(plugin) = plugin {
                    if let Some(mdef) = plugin.find_macro(name) {
                        let kind = macros::classify(mdef.name, mdef, plugin);
                        let mut doc_markdown = format!(
                            "**{}** `{}`\n\n{}",
                            macros::hover_kind_label(kind),
                            plugin.format_macro_label(mdef.name),
                            mdef.description
                        );
                        if mdef.deprecated {
                            if let Some(msg) = mdef.deprecation_message {
                                doc_markdown.push_str(&format!("\n\n**Deprecated**: {}", msg));
                            }
                        }
                        if let Some(note) = macros::hover_kind_note(kind, mdef.name, plugin) {
                            doc_markdown.push_str(&format!("\n\n{}", note));
                        }
                        if let Some(args) = mdef.args {
                            if !args.is_empty() {
                                doc_markdown.push_str("\n\n**Parameters:**\n");
                                for arg in args {
                                    let req = if arg.is_required { " (required)" } else { "" };
                                    let kind_str = match arg.kind {
                                        knot_formats::types::MacroArgKind::Expression => "expr",
                                        knot_formats::types::MacroArgKind::String => "string",
                                        knot_formats::types::MacroArgKind::Selector => "selector",
                                        knot_formats::types::MacroArgKind::Variable => "variable",
                                    };
                                    let flags = if arg.is_passage_ref { " passage" } else { "" };
                                    doc_markdown.push_str(&format!(
                                        "- `{}{}`: {}{}\n",
                                        arg.label, req, kind_str, flags
                                    ));
                                }
                            }
                        }
                        return Ok(CompletionItem {
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: doc_markdown,
                            })),
                            ..params
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(params)
}

// ===========================================================================
// Private helpers — FormatCompletionItem → CompletionItem mapping
// ===========================================================================

/// Map a `FormatCompletionItem` to an `lsp_types::CompletionItem`.
///
/// This is the ONLY place where format-agnostic completion data touches
/// LSP types. The mapping is straightforward:
/// - `FormatCompletionKind` → `CompletionItemKind`
/// - `FormatInsertTextFormat` → `InsertTextFormat`
/// - `FormatTextEdit` → `TextEdit` (with Position + Range)
fn map_completion_item(fi: knot_formats::types::FormatCompletionItem) -> CompletionItem {
    CompletionItem {
        label: fi.label,
        kind: Some(map_completion_kind(fi.kind)),
        detail: fi.detail,
        sort_text: fi.sort_text,
        filter_text: fi.filter_text,
        insert_text: fi.insert_text,
        insert_text_format: Some(map_insert_format(fi.insert_text_format)),
        text_edit: fi.text_edit.map(|te| {
            TextEdit::new(
                Range::new(
                    Position::new(te.start_line, te.start_character),
                    Position::new(te.end_line, te.end_character),
                ),
                te.new_text,
            ).into()
        }),
        deprecated: if fi.deprecated { Some(true) } else { None },
        preselect: if fi.preselect { Some(true) } else { None },
        data: fi.data,
        commit_characters: if fi.commit_characters.is_empty() {
            None
        } else {
            Some(fi.commit_characters)
        },
        tags: if fi.deprecated {
            Some(vec![CompletionItemTag::DEPRECATED])
        } else {
            None
        },
        ..Default::default()
    }
}

/// Map `FormatCompletionKind` to `CompletionItemKind`.
fn map_completion_kind(kind: FormatCompletionKind) -> CompletionItemKind {
    match kind {
        FormatCompletionKind::Text => CompletionItemKind::TEXT,
        FormatCompletionKind::Method => CompletionItemKind::METHOD,
        FormatCompletionKind::Function => CompletionItemKind::FUNCTION,
        FormatCompletionKind::Constructor => CompletionItemKind::CONSTRUCTOR,
        FormatCompletionKind::Field => CompletionItemKind::FIELD,
        FormatCompletionKind::Variable => CompletionItemKind::VARIABLE,
        FormatCompletionKind::Class => CompletionItemKind::CLASS,
        FormatCompletionKind::Interface => CompletionItemKind::INTERFACE,
        FormatCompletionKind::Module => CompletionItemKind::MODULE,
        FormatCompletionKind::Property => CompletionItemKind::PROPERTY,
        FormatCompletionKind::Unit => CompletionItemKind::UNIT,
        FormatCompletionKind::Value => CompletionItemKind::VALUE,
        FormatCompletionKind::Enum => CompletionItemKind::ENUM,
        FormatCompletionKind::Keyword => CompletionItemKind::KEYWORD,
        FormatCompletionKind::Snippet => CompletionItemKind::SNIPPET,
        FormatCompletionKind::Color => CompletionItemKind::COLOR,
        FormatCompletionKind::File => CompletionItemKind::FILE,
        FormatCompletionKind::Reference => CompletionItemKind::REFERENCE,
        FormatCompletionKind::Folder => CompletionItemKind::FOLDER,
        FormatCompletionKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
        FormatCompletionKind::Constant => CompletionItemKind::CONSTANT,
        FormatCompletionKind::Struct => CompletionItemKind::STRUCT,
        FormatCompletionKind::Event => CompletionItemKind::EVENT,
        FormatCompletionKind::Operator => CompletionItemKind::OPERATOR,
        FormatCompletionKind::TypeParameter => CompletionItemKind::TYPE_PARAMETER,
    }
}

/// Map `FormatInsertTextFormat` to `InsertTextFormat`.
fn map_insert_format(fmt: FormatInsertTextFormat) -> InsertTextFormat {
    match fmt {
        FormatInsertTextFormat::PlainText => InsertTextFormat::PLAIN_TEXT,
        FormatInsertTextFormat::Snippet => InsertTextFormat::SNIPPET,
    }
}
