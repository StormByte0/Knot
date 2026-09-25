//! Code action handlers.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::passage::SpecialPassageBehavior;
use lsp_types::*;
use std::collections::HashSet;

/// Push the UnreachablePassage quickfixes for `name`.
///
/// Two fixes: "Add link from X" wires the passage into the reachable
/// graph from the nearest reachable neighbor, and "Mark as reachable"
/// writes the manual-entry flag into the passage header metadata — the
/// analyzer override for variable-aliased navigation the static
/// analysis can't see through. Shared by the client-diagnostic loop
/// and the cursor-passage fallback below.
fn push_unreachable_actions(
    inner: &crate::state::ServerStateInner,
    actions: &mut Vec<CodeActionOrCommand>,
    name: &str,
    diagnostic: Option<&Diagnostic>,
) {
    // Find nearest reachable passage
    if let Some(near) = helpers::find_nearest_reachable_passage(&inner.workspace, name) {
        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Add link from '{}' to '{}'", near, name),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: diagnostic.cloned().map(|d| vec![d]),
            edit: Some(helpers::add_link_edit(inner, &near, name)),
            ..Default::default()
        }));
    }

    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Mark '{}' as reachable (manual entry)", name),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: diagnostic.cloned().map(|d| vec![d]),
        edit: Some(helpers::mark_reachable_edit(inner, name)),
        ..Default::default()
    }));
}

pub(crate) async fn code_action(
    state: &ServerState,
    params: CodeActionParams,
) -> Result<Option<CodeActionResponse>, tower_lsp::jsonrpc::Error> {
    // Short-circuit if the server is shutting down — the transport stream
    // may already be destroyed, so any attempt to write a response would
    // trigger "Cannot call write after a stream was destroyed".
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Ok(None);
    }

    let inner = state.inner.read().await;

    // Resolve the startup passage name from the format plugin
    let format = inner.workspace.resolve_format();
    let plugin = inner.format_registry.get(&format);
    let startup_passage_name = plugin
        .as_ref()
        .and_then(|p| {
            p.all_special_passages()
                .into_iter()
                .find(|def| {
                    def.contributes_variables
                        && matches!(def.behavior, SpecialPassageBehavior::Startup)
                })
                .map(|def| def.name)
        })
        .unwrap_or_else(|| "Startup".to_string());

    let sigils: Vec<char> = plugin
        .as_ref()
        .map(|p| p.variable_sigils().iter().map(|s| s.sigil).collect())
        .unwrap_or_default();

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    // Passage names whose unreachable quickfixes were already pushed from
    // the client-provided diagnostics — dedupes against the fallback below
    // so the cursor-on-the-squiggle and cursor-in-the-body flows never
    // double up.
    let mut unreachable_handled: HashSet<String> = HashSet::new();

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
                    unreachable_handled.insert(name.clone());
                    push_unreachable_actions(&inner, &mut actions, &name, Some(diag));
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
                if let Some(var_name) = helpers::extract_variable_name(&diag.message, &sigils) {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Initialize {} in {}", var_name, startup_passage_name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(helpers::initialize_var_in_story_init_edit(
                            &inner, &var_name,
                        )),
                        is_preferred: Some(true),
                        ..Default::default()
                    }));
                }
            }
            _ => {}
        }
    }

    // ── Cursor-passage fallback ────────────────────────────────────────
    // VS Code only includes diagnostics that intersect the cursor range
    // in `context.diagnostics`, and UnreachablePassage squiggles cover
    // just the passage NAME in the header — with the cursor in the
    // passage body the context arrives empty and no quickfix ever
    // surfaced. Derive the passage containing the cursor and offer the
    // same unreachable fixes there (deduped against the loop above).
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    if let Some(text) = inner.open_documents.get(&uri)
        && let Some(name) = helpers::unreachable_passage_under_cursor(
            &inner.workspace,
            text,
            &uri,
            params.range.start,
        )
        && !unreachable_handled.contains(&name)
    {
        push_unreachable_actions(&inner, &mut actions, &name, None);
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
