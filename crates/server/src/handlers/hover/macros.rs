//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a macro at the cursor position (outer layer).
///
/// Uses `passage.macro_invocations` for span-based resolution instead of
/// line-scanning with `find_macro_at_position()`. This works correctly for
/// multi-line macros and provides precise hover ranges.
///
/// The function checks two conditions:
/// 1. **Cursor on macro name**: If the cursor falls within `name_span`,
///    show macro hover with the name as the hover range.
/// 2. **Cursor inside macro open tag**: If the cursor falls within
///    `open_span` but not on the name (and not on a PassageRef arg,
///    which was already handled by `try_macro_arg_ref_hover` in step 2,
///    and not on a variable, which was already handled by
///    `try_variable_hover` in step 3), show macro hover with the open
///    tag as the hover range. This is what makes hovering on `<<` or
///    `>>` of `<<run>>` work — the name span only covers `run`, so
///    without this fallback, hovering on the delimiters produced no
///    hover at all (the user saw the variable hover for the first
///    variable inside the macro, or nothing at all).
///
/// Macros that have no `macro_arg_refs` entries (i.e., no PassageRef args)
/// still get hover via the `macro_open_span` check — this handles macros
/// like `<<set>>`, `<<if>>`, `<<print>>` that don't contain passage references.
pub(super) fn try_macro_hover(
    text: &str,
    byte_offset: usize,
    doc: Option<&knot_core::Document>,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<Hover> {
    let doc = doc?;

    // Span-based resolution for ALL macros (not just those with PassageRef args).
    // `passage.macro_invocations` is populated for every parsed macro, so we
    // can resolve `<<set>>`, `<<if>>`, `<<print>>`, etc. via span lookup
    // without falling back to line-scanning.
    //
    // Design principle: hover answers "what is THIS thing?" — so when the
    // cursor is on the macro NAME, we use the name span as the hover range
    // (precise). When the cursor is on the delimiters (`<<`, `>>`) or
    // whitespace inside the open tag, we use the full open_span as the hover
    // range (the user is clearly asking "what macro is this?", not "what is
    // this single character?"). The variable and PassageRef layers already
    // ran before us, so we know the cursor isn't on a variable or arg.
    for passage in &doc.passages {
        for inv in &passage.macro_invocations {
            // Determine which span the cursor is in (if any).
            //
            // - `on_name`: cursor is inside `name_span` (e.g., on `run` of
            //   `<<run>>`). Use name_span as the hover range.
            // - `in_open_tag`: cursor is inside `open_span` but NOT inside
            //   `name_span`. Use open_span as the hover range. This catches
            //   `<<`, `>>`, and any whitespace/punctuation that isn't a
            //   variable or arg (those are handled by earlier layers).
            //
            // For Expression macros (`<<=>>`, `<<->>`), the parser sets
            // name_span == open_span (the full expression construct), so
            // `on_name` is the only case that fires — which is correct.
            let on_name = passage.span_contains_abs_offset(&inv.name_span, byte_offset);
            let in_open_tag =
                !on_name && passage.span_contains_abs_offset(&inv.open_span, byte_offset);
            if !on_name && !in_open_tag {
                continue;
            }

            // Choose the hover range: name_span when cursor is on the name
            // (precise), open_span when cursor is on the delimiters (the
            // user is asking about the whole tag).
            let hover_byte_range = if on_name {
                passage.abs_range(&inv.name_span)
            } else {
                passage.abs_range(&inv.open_span)
            };

            // Try builtin macro first.
            if let Some(mdef) = plugin.find_macro(&inv.name) {
                let mut hover_text = build_macro_hover_text(mdef, plugin);

                // Container violation check: if this macro requires a parent
                // (e.g., `<<else>>` must be inside `<<if>>`), find the
                // enclosing macro at the cursor and verify it's allowed.
                if let Some(violation) =
                    check_container_violation(mdef, passage, byte_offset, plugin)
                {
                    hover_text.push_str(&format!("\n\n**Container violation**: {}", violation));
                }

                let hover_range = helpers::byte_range_to_lsp_range(text, &hover_byte_range);
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_text,
                    }),
                    range: Some(hover_range),
                });
            }

            // Not a builtin — try custom macro (widget / Macro.add()).
            if let Some(detail) = plugin.find_custom_macro_detail(&inv.name) {
                let hover_text = build_custom_macro_hover_text(&inv.name, &detail, plugin);
                let hover_range = helpers::byte_range_to_lsp_range(text, &hover_byte_range);
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

    None
}

/// Check if a macro invocation violates its container constraints.
///
/// A macro with `mdef.container = Some("if")` must appear inside an `<<if>>`
/// block. This function uses the unified zone map
/// (`passage.zones.enclosing_body_at`) to find the innermost enclosing macro
/// body at `byte_offset` and verifies it matches the constraint. Returns
/// `Some(message)` if there's a violation, `None` if the macro is correctly
/// nested (or has no container constraint).
///
/// **Phase 3 migration**: replaced the hand-rolled innermost-enclosing-macro
/// loop (which approximated "inside body" with "after open tag end" and got
/// the post-close-sibling case wrong — see plan.md §6.5) with a single
/// `enclosing_body_at` query on the zone map.
pub(super) fn check_container_violation(
    mdef: &knot_formats::types::MacroDef,
    passage: &Passage,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> Option<String> {
    // Determine the allowed parent set.
    let allowed: Vec<&str> = if let Some(parent) = mdef.container {
        vec![parent]
    } else {
        let parents = mdef.container_any_of?; // No container constraint.
        parents.to_vec()
    };

    // Convert document-absolute byte_offset to passage-relative for the
    // zone map query. Zone map spans are passage-relative (0 = passage head).
    let passage_offset = byte_offset.saturating_sub(passage.passage_offset);

    // Find the innermost enclosing macro body via the zone map.
    // For a MacroTag leaf (e.g., `<<else>>`), `enclosing_body_at` returns
    // the body the tag lives IN (its parent body) — which is exactly what
    // we want for container violation checking.
    let enclosing_parent: Option<&str> = passage
        .zones
        .enclosing_body_at(passage_offset)
        .map(|body| body.macro_name.as_str());

    match enclosing_parent {
        Some(parent) => {
            if allowed.contains(&parent) {
                None // Correctly nested.
            } else {
                let allowed_labels: Vec<String> = allowed
                    .iter()
                    .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                    .collect();
                Some(format!(
                    "must be inside {} (currently inside `{}`)",
                    allowed_labels.join(" or "),
                    plugin.format_macro_label(parent)
                ))
            }
        }
        None => {
            // No enclosing parent — macro is at top level but requires one.
            let allowed_labels: Vec<String> = allowed
                .iter()
                .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                .collect();
            Some(format!("must be inside {}", allowed_labels.join(" or ")))
        }
    }
}

/// Build hover text for a custom macro (widget / `Macro.add()`).
///
/// Custom macros don't have a `MacroDef` (those are builtins only). Instead
/// we render the definition location, arg count, container-ness, and any
/// description extracted from comments above the definition.
fn build_custom_macro_hover_text(
    name: &str,
    detail: &knot_formats::plugin::CustomMacroDetail,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> String {
    let kind_label = if detail.is_widget {
        "Widget"
    } else {
        "Custom macro"
    };
    let mut text = format!(
        "**`{}`** `{}`\n\nDefined in `:: {}`",
        plugin.format_macro_label(name),
        kind_label,
        detail.defined_in
    );

    if let Some(n) = detail.arg_count {
        text.push_str(&format!("\n\n**Args:** {}", n));
    }
    if detail.is_container {
        text.push_str("\n\n*Container* — has a body between open and close tags.");
    }
    if let Some(ref desc) = detail.description
        && !desc.is_empty()
    {
        text.push_str(&format!("\n\n{}", desc));
    }
    text
}

/// Build the hover text for a macro definition.
///
/// The hover header is intentionally minimal: just the format-specific macro
/// label (e.g. `**\`<<if>>\`**`) followed by the catalog description. We do
/// NOT emit our own classification labels ("Block macro", "Control-flow macro",
/// etc.) — those are internal SugarCube categorization names that don't appear
/// in the official SugarCube documentation and would only confuse users. The
/// close-tag hint ("Close with `<</if>>`.") is preserved for container macros
/// because that's actionable guidance, not terminology.
///
/// The macro label is wrapped in backticks (rendered as inline code) so that
/// VS Code's markdown renderer doesn't strip `<<set>>` as an unknown HTML tag
/// (`<set>`). Without backticks, `**<<set>>**` renders as bold empty text —
/// the user sees `<>` instead of `<<set>>`.
fn build_macro_hover_text(
    mdef: &knot_formats::types::MacroDef,
    plugin: &dyn fmt_plugin::FormatPlugin,
) -> String {
    use knot_formats::types::MacroStatus;

    let mut hover_text = format!("**`{}`**", plugin.format_macro_label(mdef.name));

    // Version-aware resolution: prefer the era-effective descriptor and
    // the lifecycle AT the story's pinned format version. Fall back to the
    // latest-era descriptor when the story's version is unknown (plugins
    // without version tracking), matching the pre-versioning behavior.
    let story_version = plugin
        .story_version()
        .unwrap_or(knot_formats::sugarcube::macros::SUGARCUBE_LATEST);
    let (description, args, status) = match mdef.descriptor_at(story_version) {
        Some(descriptor) => (
            descriptor.description,
            descriptor.args,
            mdef.status_at(story_version),
        ),
        None => (
            mdef.latest_descriptor().description,
            mdef.latest_descriptor().args,
            // Macro doesn't exist at the story's version — classify which
            // side of history it's on for the tombstone note below.
            mdef.status_at(story_version),
        ),
    };

    // Add description (era-appropriate for the story's format version)
    hover_text.push_str(&format!("\n\n{}", description));

    // Lifecycle notes — replace the old flat "deprecated" flag with the
    // version-resolved status so hovers explain WHEN and what to do.
    match status {
        MacroStatus::Removed => {
            // status_at only reports Removed when removed_in is set.
            let removed = mdef.removed_in.unwrap_or(story_version);
            hover_text.push_str(&format!(
                "\n\n**Removed in SugarCube {}** — it does not exist in this story's version ({}).",
                removed, story_version
            ));
            if let Some(msg) = mdef.deprecation_message {
                hover_text.push_str(&format!(" {}", msg));
            }
            hover_text
                .push_str("\n\nReplace it, or downgrade the story's `format-version` to use it.");
        }
        MacroStatus::Deprecated => {
            if let Some(msg) = mdef.deprecation_message {
                hover_text.push_str(&format!("\n\n**Deprecated**: {}", msg));
            } else {
                hover_text.push_str("\n\n**Deprecated**.");
            }
            if let Some(dep) = mdef.deprecated_in {
                hover_text.push_str(&format!(" (deprecated since SugarCube {})", dep));
            }
        }
        MacroStatus::NotYetAdded => {
            hover_text.push_str(&format!(
                "\n\n**Added in SugarCube {}** — it does not exist in this story's version ({}).",
                mdef.added_in, story_version
            ));
        }
        MacroStatus::Available => {}
    }

    // Close-tag hint for container macros (those that require a body and a
    // matching `<</name>>` close tag). This is the only structural note we
    // surface — it's actionable, not jargon.
    if mdef.kind == knot_formats::types::MacroKind::Container {
        let close_label = plugin.format_close_macro_label(mdef.name);
        if !close_label.is_empty() {
            hover_text.push_str(&format!("\n\nClose with `{}`.", close_label));
        }
    }

    // Add parameter info — render with human-readable kind descriptions
    // so users understand what to write, not just the type name.
    if let Some(args) = args
        && !args.is_empty()
    {
        hover_text.push_str("\n\n**Parameters:**\n");
        for arg in args {
            let req = if arg.is_required {
                " (required)"
            } else {
                " (optional)"
            };
            let kind_desc = describe_macro_arg_kind(&arg.kind, arg.is_passage_ref);
            hover_text.push_str(&format!("- `{}{}`: {}\n", arg.label, req, kind_desc));
        }
    }

    // Add container constraint info — use format-specific labels
    if let Some(parent) = mdef.container {
        hover_text.push_str(&format!(
            "\nMust be inside `{}`.",
            plugin.format_macro_label(parent)
        ));
    }
    if let Some(parents) = mdef.container_any_of {
        hover_text.push_str(&format!(
            "\nMust be inside one of: {}.",
            parents
                .iter()
                .map(|p| format!("`{}`", plugin.format_macro_label(p)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    hover_text
}
