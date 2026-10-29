//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a passage link when the cursor is within a
/// link's span.
///
/// Uses `passage.links[].span` from the workspace index for precise
/// byte-range matching instead of manually scanning the line for `[[`/`]]`
/// patterns. The link's `target` field is used directly, avoiding the need
/// to parse arrow (`->`) or pipe (`|`) syntax.
pub(super) fn try_link_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    workspace: &knot_core::Workspace,
) -> Option<Hover> {
    let doc = doc?;

    // Iterate over all passages in the document and check if the cursor
    // byte offset falls within any link's span. Link spans are absolute
    // byte offsets in the document text.
    for passage in &doc.passages {
        for link in &passage.links {
            if passage.span_contains_abs_offset(&link.span, byte_offset) {
                let target = link.target.trim();
                let abs_link_span = passage.abs_range(&link.span);

                // ── Determine label and target sub-spans within the link ──
                //
                // For `[[target]]` (no pipe): the entire inner text is both
                // the display text and the target. Hovering anywhere shows
                // passage info for the target.
                //
                // For `[[label|target]]` (pipe syntax): hovering on the
                // label part shows "Link display text" info; hovering on
                // the target part shows passage info for the target. This
                // mirrors how the token builder colors the two parts
                // differently (display = green, target = teal underline).
                //
                // We scan the source text within the link span to find the
                // `|` separator (if any), then compute the label and target
                // sub-spans.
                let link_text = &text[abs_link_span.start..abs_link_span.end.min(text.len())];

                // Find the `|` separator, skipping the opening `[[`.
                // The inner content starts at offset 2 (after `[[`).
                // We scan for `|` but NOT inside string literals or nested
                // brackets — though SugarCube link syntax doesn't support
                // those, a simple find is sufficient.
                let pipe_pos = link_text[2..].find('|').map(|pos| 2 + pos);

                if let Some(pipe_rel) = pipe_pos {
                    // `[[label|target]]` — pipe syntax.
                    let label_start = abs_link_span.start + 2; // after `[[
                    let label_end = abs_link_span.start + pipe_rel; // before `|`
                    let target_start = abs_link_span.start + pipe_rel + 1; // after `|`
                    let target_end = abs_link_span.end - 2; // before `]]`

                    let on_label = byte_offset >= label_start && byte_offset < label_end;
                    let on_target = byte_offset >= target_start && byte_offset < target_end;
                    let on_pipe = byte_offset == abs_link_span.start + pipe_rel;

                    if on_label || on_pipe {
                        // Hovering on the label (display text) part.
                        let label_text = &text[label_start..label_end];
                        let hover_text = if label_text.trim().is_empty() {
                            "**Link display text** (empty — the target passage name will be used as display text)".to_string()
                        } else {
                            format!(
                                "**Link display text**\n\n`{}`\n\nLinks to passage `{}`",
                                label_text, target
                            )
                        };
                        let hover_range =
                            helpers::byte_range_to_lsp_range(text, &(label_start..label_end));
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    } else if on_target {
                        // Hovering on the target part — show passage info.
                        if !target.is_empty() {
                            if let Some((doc, passage)) = workspace.find_passage(target) {
                                let hover_text = build_passage_target_hover_text(
                                    target, doc, passage, workspace,
                                );
                                let hover_range = helpers::byte_range_to_lsp_range(
                                    text,
                                    &(target_start..target_end),
                                );
                                return Some(Hover {
                                    contents: HoverContents::Markup(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: hover_text,
                                    }),
                                    range: Some(hover_range),
                                });
                            } else {
                                // Broken link — passage doesn't exist
                                let hover_range = helpers::byte_range_to_lsp_range(
                                    text,
                                    &(target_start..target_end),
                                );
                                return Some(Hover {
                                    contents: HoverContents::Markup(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: format!(
                                            "**Broken link** — passage `{}` does not exist",
                                            target
                                        ),
                                    }),
                                    range: Some(hover_range),
                                });
                            }
                        }
                    }
                    // If cursor is on `[[` or `]]` delimiters, fall through
                    // to the full-link hover below.
                }

                // `[[target]]` (no pipe) OR cursor on delimiters of pipe link —
                // show passage info for the target with the full link span.
                if !target.is_empty() {
                    if let Some((doc, passage)) = workspace.find_passage(target) {
                        let hover_text =
                            build_passage_target_hover_text(target, doc, passage, workspace);
                        let hover_range = helpers::byte_range_to_lsp_range(text, &abs_link_span);
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    } else {
                        // Broken link — passage doesn't exist
                        let hover_range = helpers::byte_range_to_lsp_range(text, &abs_link_span);
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: format!(
                                    "**Broken link** — passage `{}` does not exist",
                                    target
                                ),
                            }),
                            range: Some(hover_range),
                        });
                    }
                }
            }
        }
    }

    None
}
