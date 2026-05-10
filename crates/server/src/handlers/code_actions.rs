//! Code action handlers.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::passage::SpecialPassageBehavior;
use lsp_types::*;

pub(crate) async fn code_action(
    state: &ServerState,
    params: CodeActionParams,
) -> Result<Option<CodeActionResponse>, tower_lsp::jsonrpc::Error> {
    let _uri = helpers::normalize_file_uri(&params.text_document.uri);
    let inner = state.inner.read().await;

    // Resolve the startup passage name from the format plugin
    let format = inner.workspace.resolve_format();
    let startup_passage_name = inner.format_registry.get(&format)
        .and_then(|plugin| {
            plugin.special_passages()
                .into_iter()
                .find(|def| {
                    def.contributes_variables
                        && matches!(def.behavior, SpecialPassageBehavior::Startup)
                })
                .map(|def| def.name)
        })
        .unwrap_or_else(|| "StoryInit".to_string());

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
                        title: format!("Initialize {} in {}", var_name, startup_passage_name),
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

// NOTE: The pull-diagnostic handler (`diagnostic`) has been removed.
// The server uses the push model (`publish_diagnostics`) exclusively.
// Using both models simultaneously causes VS Code to display every
// diagnostic twice, which makes errors appear duplicated in hover
// and the Problems panel.
