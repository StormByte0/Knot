//! Code action and pull diagnostic handlers.

use crate::handlers::helpers;
use crate::handlers::macros;
use crate::state::ServerState;
use knot_core::AnalysisEngine;
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;
use tower_lsp::LanguageServer;

pub(crate) async fn code_action(
    state: &ServerState,
    params: CodeActionParams,
) -> Result<Option<CodeActionResponse>, tower_lsp::jsonrpc::Error> {
    let _uri = &params.text_document.uri;
    let inner = state.inner.read().await;

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();

    for diag in &params.context.diagnostics {
        let code = match &diag.code {
            Some(NumberOrString::String(s)) => s.clone(),
            _ => continue,
        };

        match code.as_str() {
            "BrokenLink" => {
                // Extract the broken link target from the message
                if let Some(name) = helpers::extract_quoted_name(&diag.message) {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Create passage '{}'", name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(helpers::create_passage_edit(&inner, &name)),
                        is_preferred: Some(true),
                        ..Default::default()
                    }));
                }
            }
            "UnreachablePassage" => {
                if let Some(name) = helpers::extract_passage_from_diag(&diag.message) {
                    // Find nearest reachable passage
                    let nearest = helpers::find_nearest_reachable_passage(&inner.workspace, &name);
                    if let Some(near) = nearest {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Add link from '{}' to '{}'", near, name),
                            kind: Some(CodeActionKind::QUICKFIX),
                            diagnostics: Some(vec![diag.clone()]),
                            edit: Some(helpers::add_link_edit(&inner, &near, &name)),
                            ..Default::default()
                        }));
                    }
                }
            }
            "DuplicatePassageName" => {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Rename passage".to_string(),
                    kind: Some(CodeActionKind::new("refactor.rename")),
                    diagnostics: Some(vec![diag.clone()]),
                    command: Some(Command {
                        title: "Rename passage".to_string(),
                        command: "editor.action.rename".to_string(),
                        arguments: None,
                    }),
                    ..Default::default()
                }));
            }
            "EmptyPassage" => {
                if let Some(name) = helpers::extract_passage_from_diag(&diag.message) {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add content template to '{}'", name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(helpers::add_content_template_edit(&inner, &name)),
                        ..Default::default()
                    }));
                }
            }
            "UninitializedVariable" => {
                if let Some(var_name) = helpers::extract_variable_name(&diag.message) {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Initialize {} in StoryInit", var_name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(helpers::initialize_var_in_story_init_edit(&inner, &var_name)),
                        is_preferred: Some(true),
                        ..Default::default()
                    }));
                }
            }
            _ => {}
        }
    }

    if actions.is_empty() {
        Ok(None)
    } else {
        Ok(Some(actions))
    }
}

pub(crate) async fn diagnostic(
    state: &ServerState,
    params: DocumentDiagnosticParams,
) -> Result<DocumentDiagnosticReportResult, tower_lsp::jsonrpc::Error> {
    let uri = &params.text_document.uri;
    let inner = state.inner.read().await;

    let text = match inner.open_documents.get(uri) {
        Some(t) => t,
        None => {
            return Ok(DocumentDiagnosticReportResult::Report(
                DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: None,
                        items: vec![],
                    },
                }),
            ));
        }
    };

    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
    let uri_str = uri.to_string();
    let config = &inner.workspace.config;

    let mut items: Vec<Diagnostic> = Vec::new();

    for gd in &diagnostics {
        if gd.file_uri != uri_str { continue; }

        let default_severity = helpers::diagnostic_kind_to_severity(&gd.kind);

        let diag_key = format!("{:?}", gd.kind);
        let severity = if let Some(custom) = config.diagnostics.get(&diag_key) {
            match custom {
                knot_core::workspace::DiagnosticSeverity::Off => continue,
                knot_core::workspace::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                knot_core::workspace::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                knot_core::workspace::DiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                knot_core::workspace::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
            }
        } else {
            default_severity
        };

        let range = helpers::find_passage_header_range(text, &gd.passage_name);

        // Build related information
        let related_information = helpers::build_related_information(
            &inner, &gd.kind, &gd.passage_name, &gd.message,
        );

        items.push(Diagnostic {
            range,
            severity: Some(severity),
            code: Some(NumberOrString::String(diag_key)),
            source: Some("knot".to_string()),
            message: gd.message.clone(),
            related_information,
            ..Default::default()
        });
    }

    // Also add format diagnostics
    if let Some(fmt_diags) = inner.format_diagnostics.get(uri) {
        for fd in fmt_diags {
            let range = helpers::byte_range_to_lsp_range(text, &fd.range);
            let severity = match fd.severity {
                fmt_plugin::FormatDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                fmt_plugin::FormatDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                fmt_plugin::FormatDiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                fmt_plugin::FormatDiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
            };
            items.push(Diagnostic {
                range,
                severity: Some(severity),
                code: Some(NumberOrString::String(format!("format:{}", fd.code))),
                source: Some("knot".to_string()),
                message: fd.message.clone(),
                ..Default::default()
            });
        }
    }

    Ok(DocumentDiagnosticReportResult::Report(
        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: None,
                items,
            },
        }),
    ))
}
