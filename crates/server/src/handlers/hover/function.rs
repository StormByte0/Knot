//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a function call (e.g., `myFunc()` inside
/// `<<run>>` or `<<script>>`).
///
/// Uses semantic tokens (`SemanticTokenType::Function`) for cursor-on-span
/// detection. The plugin's `find_function()` registry provides definition
/// location + param count.
pub(super) fn try_function_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // Find the function token under the cursor.
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Function {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                let name = &text[abs_start..abs_end];

                // Path 1: User-defined function (from script passages).
                // Has definition location + param count.
                if let Some(info) = plugin.find_function(name) {
                    // Meaningfulness gate: only show function hover when there's
                    // info beyond what's visible in the code. The function name
                    // is already visible; we need param count to justify a popup.
                    if let Some(param_count) = info.param_count {
                        let hover_text = format!(
                            "**{}** `Function` ({} params)\n\nDefined in `:: {}`",
                            name, param_count, info.defined_in
                        );
                        let hover_range =
                            helpers::byte_range_to_lsp_range(text, &(abs_start..abs_end));
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover_text,
                            }),
                            range: Some(hover_range),
                        });
                    }
                }

                // Path 2: SugarCube builtin method or function.
                // These are runtime extensions (.pushUnique, .toUpperFirst,
                // .clamp, etc.) or standalone builtins (random, either, etc.).
                // No definition location (they're in the SugarCube runtime),
                // but we have descriptions from the docs.
                if let Some(desc) = plugin.describe_builtin_method(name) {
                    let hover_text = format!("**{}** `Method`\n\n{}", name, desc);
                    let hover_range = helpers::byte_range_to_lsp_range(text, &(abs_start..abs_end));
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
