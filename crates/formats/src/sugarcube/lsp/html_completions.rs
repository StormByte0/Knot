//! HTML attribute-name completions inside tag interiors — including
//! SugarCube's evaluation attribute directives (`@attr="expr"` /
//! `sc-eval:attr="expr"`, SugarCube ≥2.21.0).
//!
//! ## Context
//!
//! The completion zone guard in `SugarCubePlugin::provide_completions`
//! routes `CompletionZone::TagInterior { directive_value: false }`
//! here (raw `<style>` bodies and directive VALUES keep their existing
//! policies). This module answers one question: given the attribute
//! word in progress at the cursor, which attribute names should be
//! offered?
//!
//! The word's prefix decides the family:
//!
//! | Word in progress | Offers | Examples |
//! |---|---|---|
//! | `@…` | `@` + curated attribute names (directive shorthand) | `@class`, `@data-passage` |
//! | `sc-eval:…` | `sc-eval:` + curated attribute names | `sc-eval:class` |
//! | anything else | plain curated attribute names + the `sc-eval:` spelling itself | `class`, `data-passage`, `sc-eval:` |
//!
//! `data-setter` is deliberately EXCLUDED from the two directive
//! families — the engine throws on `@data-setter`/`sc-eval:data-setter`
//! (see `embedded_diags`); offering it would be a completion that
//! immediately produces an error. It stays in the plain family, where
//! it is valid (`data-setter="$x to 1"` next to `data-passage`).
//!
//! ## Scope decisions
//!
//! - The attribute list is CURATED, not exhaustive: the global
//!   attributes authors actually use in passages plus the SugarCube
//!   special attributes. VS Code's HTML extension covers the long tail
//!   for `.html` files; Knot covers the Twee-authoring surface.
//! - Completions fire on ordinary word typing (VS Code's default
//!   letter triggers); `@` and `:` are NOT registered trigger
//!   characters, so prose typing is unaffected.
//! - Value positions (inside `="…"`) offer nothing — the plugin's zone
//!   guard only routes NAME positions here (the AST walk behind
//!   `CompletionZone::TagInterior` classifies value positions
//!   separately).

use crate::types::{FormatCompletionItem, FormatCompletionKind};

/// A curated HTML attribute: its name and the one-line detail shown in
/// the completion popup.
struct KnownAttr {
    name: &'static str,
    detail: &'static str,
}

/// Curated attribute names offered in tag interiors.
///
/// Global attributes, the common element-specific ones passage authors
/// reach for, and SugarCube's special attributes (`data-passage`,
/// `data-setter` — docs §markup-html-svg-attribute). Keep the list
/// short enough to scan at a glance; this is a curation, not a registry.
const KNOWN_ATTRS: &[KnownAttr] = &[
    KnownAttr {
        name: "class",
        detail: "Space-separated class names (SugarCube: also targeted by CSS/selector helpers)",
    },
    KnownAttr {
        name: "id",
        detail: "Element id",
    },
    KnownAttr {
        name: "style",
        detail: "Inline CSS declarations (colons separate prop: value pairs)",
    },
    KnownAttr {
        name: "title",
        detail: "Hover tooltip text",
    },
    KnownAttr {
        name: "hidden",
        detail: "Hide the element until scripts show it",
    },
    KnownAttr {
        name: "lang",
        detail: "Language code (BCP 47)",
    },
    KnownAttr {
        name: "dir",
        detail: "Text direction: ltr, rtl, or auto",
    },
    KnownAttr {
        name: "tabindex",
        detail: "Focus order (0 = document order, -1 = programmatic)",
    },
    KnownAttr {
        name: "draggable",
        detail: "Allow dragging the element",
    },
    KnownAttr {
        name: "spellcheck",
        detail: "Override spell checking (true/false)",
    },
    KnownAttr {
        name: "src",
        detail: "Resource URL (img, audio, video, iframe, script)",
    },
    KnownAttr {
        name: "href",
        detail: "Link target URL (a, area)",
    },
    KnownAttr {
        name: "target",
        detail: "Navigation target (_blank, _self, …)",
    },
    KnownAttr {
        name: "rel",
        detail: "Link relationship (noopener, …)",
    },
    KnownAttr {
        name: "alt",
        detail: "Alternative text (img, area, input[type=image])",
    },
    KnownAttr {
        name: "width",
        detail: "Element width (px or %)",
    },
    KnownAttr {
        name: "height",
        detail: "Element height (px or %)",
    },
    KnownAttr {
        name: "type",
        detail: "Element type (input, button, ol, script, …)",
    },
    KnownAttr {
        name: "value",
        detail: "Element value (input, option, button)",
    },
    KnownAttr {
        name: "name",
        detail: "Element name (form controls, meta)",
    },
    KnownAttr {
        name: "placeholder",
        detail: "Placeholder text (input, textarea)",
    },
    KnownAttr {
        name: "checked",
        detail: "Initial checked state (checkbox/radio)",
    },
    KnownAttr {
        name: "disabled",
        detail: "Disable the control",
    },
    KnownAttr {
        name: "readonly",
        detail: "Make the control read-only",
    },
    KnownAttr {
        name: "required",
        detail: "Require the control before submission",
    },
    KnownAttr {
        name: "maxlength",
        detail: "Maximum input length",
    },
    KnownAttr {
        name: "min",
        detail: "Minimum value",
    },
    KnownAttr {
        name: "max",
        detail: "Maximum value",
    },
    KnownAttr {
        name: "step",
        detail: "Value step for numeric inputs",
    },
    KnownAttr {
        name: "rows",
        detail: "Visible rows (textarea)",
    },
    KnownAttr {
        name: "cols",
        detail: "Visible columns (textarea)",
    },
    KnownAttr {
        name: "colspan",
        detail: "Cells spanned by a table cell",
    },
    KnownAttr {
        name: "rowspan",
        detail: "Rows spanned by a table cell",
    },
    KnownAttr {
        name: "srcset",
        detail: "Responsive image candidate list",
    },
    KnownAttr {
        name: "loading",
        detail: "Loading behavior: lazy or eager",
    },
    KnownAttr {
        name: "role",
        detail: "ARIA role",
    },
    KnownAttr {
        name: "aria-label",
        detail: "ARIA accessible label",
    },
    KnownAttr {
        name: "aria-hidden",
        detail: "Hide from the accessibility tree",
    },
    KnownAttr {
        name: "aria-live",
        detail: "Announce changes: polite or assertive",
    },
    // ── SugarCube special attributes ─────────────────────────────────
    KnownAttr {
        name: "data-passage",
        detail: "SugarCube: passage link/media source (a, button, img, audio, video, source)",
    },
    KnownAttr {
        name: "data-setter",
        detail: "SugarCube: TwineScript setter run on activation (pairs with data-passage)",
    },
];

/// `sc-eval:` is offered in the PLAIN family so the explicit directive
/// spelling is discoverable (typing `sc` surfaces it); `@` never
/// fuzzy-matches anything useful as an item, so it is not offered —
/// authors learn it from the item detail once `sc-eval:` is used.
const SC_EVAL_ITEM: KnownAttr = KnownAttr {
    name: "sc-eval:",
    detail: "Evaluation attribute directive — the value is evaluated as TwineScript (shorthand: @attr)",
};

/// Which completion family the cursor's word belongs to.
enum AttrFamily {
    /// `@` shorthand — offers `@name` items.
    AtShorthand,
    /// `sc-eval:` explicit form — offers `sc-eval:name` items.
    ScEval,
    /// No directive prefix — offers plain names plus `sc-eval:` itself.
    Plain,
}

/// Extract the attribute word in progress ending at `before_cursor`
/// (the line text up to the cursor) and classify its family.
///
/// The word chars are the attribute-name bytes: ASCII letters/digits,
/// `-`, `_`, `:`, `.` and the `@` sigil (`:` belongs to `sc-eval:` and
/// XML-style names; `.` for completeness). An empty word (cursor right
/// after `<div ` or a separator) is [`AttrFamily::Plain`] with an empty
/// partial — VS Code shows the whole list on Ctrl+Space.
fn attr_completion_family(before_cursor: &str) -> (AttrFamily, String) {
    let bytes = before_cursor.as_bytes();
    let mut start = bytes.len();
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.' | b'@') {
            start -= 1;
        } else {
            break;
        }
    }
    let word = &before_cursor[start..];
    if let Some(rest) = word.strip_prefix("sc-eval:") {
        return (AttrFamily::ScEval, rest.to_string());
    }
    if let Some(rest) = word.strip_prefix('@') {
        return (AttrFamily::AtShorthand, rest.to_string());
    }
    (AttrFamily::Plain, word.to_string())
}

/// Build the attribute-name completion items for a tag interior.
///
/// `before_cursor` is the line text up to the cursor (same slice
/// `provide_completions` already computes). Items are filtered by the
/// word in progress so VS Code's own filtering stays consistent with
/// the prefix families (`@cl` only ever offers `@class`-shaped items).
pub fn build_html_attr_completions(before_cursor: &str) -> Vec<FormatCompletionItem> {
    let (family, partial) = attr_completion_family(before_cursor);
    let lower_partial = partial.to_ascii_lowercase();
    let mut items = Vec::new();
    let keep =
        |name: &str| -> bool { lower_partial.is_empty() || name.starts_with(&lower_partial) };
    match family {
        AttrFamily::AtShorthand => {
            for attr in KNOWN_ATTRS {
                // data-setter + directive is an upstream error — never
                // offer it (see the module docs).
                if attr.name == "data-setter" || !keep(attr.name) {
                    continue;
                }
                items.push(directive_item(attr, "@"));
            }
        }
        AttrFamily::ScEval => {
            for attr in KNOWN_ATTRS {
                if attr.name == "data-setter" || !keep(attr.name) {
                    continue;
                }
                items.push(directive_item(attr, "sc-eval:"));
            }
        }
        AttrFamily::Plain => {
            if keep("sc-eval:") {
                items.push(FormatCompletionItem {
                    label: SC_EVAL_ITEM.name.to_string(),
                    kind: FormatCompletionKind::Keyword,
                    detail: Some(SC_EVAL_ITEM.detail.to_string()),
                    sort_text: Some(format!("z-{}", SC_EVAL_ITEM.name)),
                    filter_text: Some(SC_EVAL_ITEM.name.to_string()),
                    insert_text: Some(format!("{}\"\"", SC_EVAL_ITEM.name)),
                    insert_text_format: crate::types::FormatInsertTextFormat::PlainText,
                    text_edit: None,
                    deprecated: false,
                    preselect: false,
                    data: None,
                    commit_characters: Vec::new(),
                });
            }
            for attr in KNOWN_ATTRS {
                if !keep(attr.name) {
                    continue;
                }
                items.push(plain_item(attr));
            }
        }
    }
    items
}

/// One `@name` / `sc-eval:name` completion item.
fn directive_item(attr: &KnownAttr, prefix: &str) -> FormatCompletionItem {
    let label = format!("{prefix}{}", attr.name);
    FormatCompletionItem {
        kind: FormatCompletionKind::Property,
        detail: Some(format!(
            "Evaluation directive — {} (value evaluated as TwineScript)",
            attr.detail
        )),
        sort_text: Some(label.clone()),
        filter_text: Some(label.clone()),
        insert_text: Some(format!("{label}=\"\"")),
        insert_text_format: crate::types::FormatInsertTextFormat::PlainText,
        text_edit: None,
        deprecated: false,
        preselect: false,
        data: None,
        commit_characters: Vec::new(),
        label,
    }
}

/// One plain attribute completion item.
fn plain_item(attr: &KnownAttr) -> FormatCompletionItem {
    FormatCompletionItem {
        label: attr.name.to_string(),
        kind: FormatCompletionKind::Property,
        detail: Some(attr.detail.to_string()),
        sort_text: Some(format!("a-{}", attr.name)),
        filter_text: Some(attr.name.to_string()),
        insert_text: Some(format!("{}=\"\"", attr.name)),
        insert_text_format: crate::types::FormatInsertTextFormat::PlainText,
        text_edit: None,
        deprecated: false,
        preselect: false,
        data: None,
        commit_characters: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(before_cursor: &str) -> Vec<String> {
        build_html_attr_completions(before_cursor)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn plain_word_offers_plain_attrs_and_sc_eval() {
        // Partial word: only matching plain attrs (sc-eval: is filtered out
        // by the partial — correct, VS Code would do the same).
        let got = labels("<div cla");
        assert!(got.iter().all(|l| !l.starts_with('@')), "{got:?}");
        assert!(got.contains(&"class".to_string()), "{got:?}");
        assert!(!got.contains(&"src".to_string()), "{got:?}");
        // Empty word: the full plain list plus sc-eval: (VS Code ranks by
        // sort_text; the Vec itself is insertion-ordered).
        let got = labels("<div ");
        assert!(got.contains(&"sc-eval:".to_string()), "{got:?}");
        assert!(got.contains(&"data-passage".to_string()), "{got:?}");
        assert!(got.len() > 30, "full curated list: {got:?}");
    }

    #[test]
    fn at_word_offers_directive_attrs_only() {
        let labels = labels("<div @cl");
        assert!(!labels.is_empty(), "{labels:?}");
        assert!(labels.contains(&"@class".to_string()), "{labels:?}");
        assert!(
            labels.iter().all(|l| l.starts_with('@')),
            "only @-prefixed items: {labels:?}"
        );
    }

    #[test]
    fn sc_eval_word_offers_sc_eval_attrs() {
        let labels = labels("<span sc-eval:i");
        assert!(labels.contains(&"sc-eval:id".to_string()), "{labels:?}");
        assert!(
            labels
                .iter()
                .all(|l| l.starts_with("sc-eval:") && !l.contains("setter")),
            "no plain items, no data-setter: {labels:?}"
        );
    }

    #[test]
    fn data_setter_is_never_offered_as_a_directive() {
        for word in ["<a @", "<a sc-eval:"] {
            let labels = labels(word);
            assert!(
                labels.iter().all(|l| !l.contains("setter")),
                "word {word:?} → {labels:?}"
            );
        }
        // But the plain family keeps it (valid next to data-passage).
        let labels = labels("<a data-");
        assert!(labels.contains(&"data-setter".to_string()), "{labels:?}");
        assert!(labels.contains(&"data-passage".to_string()), "{labels:?}");
    }

    #[test]
    fn filtering_respects_the_partial() {
        let labels = labels("<img sr");
        assert!(labels.contains(&"src".to_string()), "{labels:?}");
        assert!(labels.contains(&"srcset".to_string()), "{labels:?}");
        assert!(
            labels
                .iter()
                .all(|l| l.starts_with("sr") || l == "sc-eval:"),
            "{labels:?}"
        );
    }

    #[test]
    fn empty_word_offers_the_full_plain_list() {
        let items = build_html_attr_completions("<div ");
        assert!(items.len() > 30, "curated list: {}", items.len());
        assert!(items.iter().all(|i| !i.label.starts_with('@')));
    }
}
