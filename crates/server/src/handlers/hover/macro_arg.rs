//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a passage-ref arg inside a macro (inner layer).
///
/// When the cursor is on a `PassageRef` arg inside a macro (e.g., `"Shop"` in
/// `<<link "Talk" "Shop">>`), this shows passage info for the target. This
/// takes priority over the outer macro hover (step 5) — the inner layer wins.
///
/// Uses `passage.macro_arg_refs[].span` from the workspace index for precise
/// byte-range matching. Each `MacroArgRef` contains only the span of the
/// passage name itself (not the full macro), so hover only triggers when the
/// cursor is actually on the passage reference.
pub(super) fn try_macro_arg_ref_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let doc = doc?;

    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                let target = arg_ref.target.trim();
                if target.is_empty() {
                    continue;
                }

                if let Some((target_doc, target_passage)) = workspace.find_passage(target) {
                    let mut hover_text = build_passage_target_hover_text_compact(
                        target,
                        target_doc,
                        target_passage,
                        workspace,
                    );

                    // Show which macro this arg belongs to.
                    // Use the format-owned label so this stays format-agnostic
                    // (e.g., SugarCube `<<name>>`, Harlowe `(name:)`).
                    hover_text.push_str(&format!(
                        "\n\n*Referenced by* `{}`",
                        plugin.format_macro_label(&arg_ref.macro_name)
                    ));

                    let hover_range =
                        helpers::byte_range_to_lsp_range(text, &passage.abs_range(&arg_ref.span));

                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(hover_range),
                    });
                } else {
                    // Broken ref — passage doesn't exist
                    let hover_range =
                        helpers::byte_range_to_lsp_range(text, &passage.abs_range(&arg_ref.span));

                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("**Broken link** — passage `{}` does not exist", target),
                        }),
                        range: Some(hover_range),
                    });
                }
            }
        }
    }

    None
}

/// Fallback variant of [`try_macro_arg_ref_hover`] used when no format plugin
/// is available. Renders the macro label as a bare name without format-specific
/// delimiters. Kept for the rare case where `plugin` is `None` (e.g., workspace
/// not yet indexed); prefer the plugin-aware variant in new code.
#[allow(deprecated)]
pub(super) fn try_macro_arg_ref_hover_no_plugin(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    let doc = doc?;
    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                let target = arg_ref.target.trim();
                if target.is_empty() {
                    continue;
                }
                if let Some((target_doc, target_passage)) = workspace.find_passage(target) {
                    let hover_text = build_passage_target_hover_text_compact(
                        target,
                        target_doc,
                        target_passage,
                        workspace,
                    );
                    let hover_range =
                        helpers::byte_range_to_lsp_range(text, &passage.abs_range(&arg_ref.span));
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: Some(hover_range),
                    });
                }
            }
        }
    }
    None
}

/// Human-readable description of a macro argument kind.
///
/// Used by `build_macro_hover_text` to show what the user should write for
/// each parameter. Instead of just "expression" or "variable", this gives
/// a sentence that helps the user model their story (e.g., "a SugarCube
/// expression — variables, literals, or function calls").
pub(super) fn describe_macro_arg_kind(kind: &MacroArgKind, is_passage_ref: bool) -> String {
    let base = match kind {
        MacroArgKind::Expression => {
            "a SugarCube expression — variables, literals, or function calls"
        }
        MacroArgKind::String => "a quoted string literal",
        MacroArgKind::Selector => "a CSS selector",
        MacroArgKind::Variable => "a variable reference ($var or _var)",
        MacroArgKind::Keyword => "a bareword keyword (e.g., autofocus, selected, keep)",
        MacroArgKind::Link => "a link markup ([[...]])",
        MacroArgKind::Image => "an image markup ([img[...]])",
        MacroArgKind::Number => "a numeric literal (e.g., 100, 0.5)",
    };
    if is_passage_ref {
        format!("{} (passage name)", base)
    } else {
        base.to_string()
    }
}
