//! Hover family moved verbatim from the former `hover.rs` monolith.
//! Pure move (plan §4.5) — no signature or logic changes.

use super::*;

/// Try to show hover info for a property access (e.g., `.hp` in `$player.hp`).
///
/// Uses semantic tokens (`SemanticTokenType::Property`) for cursor-on-span
/// detection. Resolves the parent variable path via the document's parsed
/// `VarOp` spans (walking backward to find the `$var` or `_var` sigil), then
/// queries the plugin's arena tree for the parent's children to render
/// "Property `hp` of `$player` (Object). Siblings: hp, mp, name."
pub(super) fn try_property_hover(
    text: &str,
    byte_offset: usize,
    plugin: &dyn fmt_plugin::FormatPlugin,
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
    doc: Option<&knot_core::Document>,
) -> Option<Hover> {
    use knot_formats::plugin::SemanticTokenType;

    // Find the property token under the cursor.
    let mut prop_abs_start = None;
    let mut prop_abs_end = None;
    let mut prop_name = String::new();
    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Property {
                continue;
            }
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start && byte_offset < abs_end {
                prop_abs_start = Some(abs_start);
                prop_abs_end = Some(abs_end);
                prop_name = text[abs_start..abs_end].to_string();
                break;
            }
        }
    }
    let prop_start = prop_abs_start?;
    let prop_end = prop_abs_end?;
    if prop_name.is_empty() {
        return None;
    }

    // Guard: if this "property" name is actually a known SugarCube builtin
    // method (e.g., `.last()`, `.pushUnique()`, `.toUpperFirst()`), skip
    // property hover. These should get Function hover instead (which runs
    // before this handler in the layered design). This guard catches cases
    // where a stale Property token overlaps with a Function token, or where
    // the method name happens to match a real property name.
    if plugin.describe_builtin_method(&prop_name).is_some() {
        return None;
    }

    // Resolve the parent variable: walk backward from the property token
    // to find the enclosing `$var` or `_var` VarOp span in the document.
    // This gives us the parent variable name for the "of `$player`" label.
    let doc = doc?;
    let enclosing_passage = doc
        .passages
        .iter()
        .find(|p| p.contains_abs_offset(byte_offset))?;
    let mut parent_var_name: Option<String> = None;
    let mut parent_kind: Option<knot_formats::types::PropertyKind> = None;
    for var in &enclosing_passage.vars {
        let abs_var_span = enclosing_passage.abs_range(&var.span);
        // The property must come after the variable's span (i.e., the
        // variable is the root of the dot-path).
        if abs_var_span.end <= prop_start {
            // Pick the closest variable before the property.
            if parent_var_name.as_ref().map(|_| 0).unwrap_or(usize::MAX) < abs_var_span.end {
                continue;
            }
            // Check if this var is the root of the path by verifying
            // there's no other identifier between var.end and prop_start
            // except dots.
            let between = &text[abs_var_span.end..prop_start];
            if between
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                parent_var_name = Some(var.name.clone());
                // Query the plugin for the parent's kind.
                let passage_name = Some(enclosing_passage.name.as_str());
                if let Some(kind) =
                    plugin.variable_kind_at_path_for_passage(&var.name, passage_name)
                {
                    parent_kind = Some(kind);
                }
            }
        }
    }

    let parent = parent_var_name.unwrap_or_else(|| "unknown".to_string());
    let kind_label = match parent_kind {
        Some(knot_formats::types::PropertyKind::Object) => "Object",
        Some(knot_formats::types::PropertyKind::Array) => "Array",
        Some(knot_formats::types::PropertyKind::Scalar) => "Scalar",
        _ => "Unknown",
    };

    // Get siblings (children of the parent variable).
    let passage_name = Some(enclosing_passage.name.as_str());
    let siblings: Vec<String> = plugin
        .variable_children_with_kind_for_passage(&parent, passage_name)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    // Meaningfulness gate: the value of property hover is discovering
    // siblings — "oh, `$player` also has `mp` and `name`." If there are
    // no siblings (the parent is a Scalar or unknown), the hover just
    // repeats what's visible in the code. Skip it.
    if siblings.is_empty() {
        return None;
    }

    let mut hover_text = format!(
        "**{}** `Property of {}` ({})",
        prop_name, parent, kind_label
    );
    let preview: Vec<&str> = siblings.iter().take(8).map(|s| s.as_str()).collect();
    let suffix = if siblings.len() > 8 { ", …" } else { "" };
    hover_text.push_str(&format!(
        "\n\n**Siblings:** {}{}",
        preview.join(", "),
        suffix
    ));

    let hover_range = helpers::byte_range_to_lsp_range(text, &(prop_start..prop_end));
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover_text,
        }),
        range: Some(hover_range),
    })
}
