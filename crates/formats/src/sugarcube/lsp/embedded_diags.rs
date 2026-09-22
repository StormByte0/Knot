//! Embedded HTML/CSS diagnostics — the plan.md Phase 2.5 wiring.
//!
//! A post-parse walk over a passage's unified AST that surfaces the
//! authoring mistakes the tolerant parsers deliberately keep silent:
//!
//! | Source | Diagnostic | Severity | Why |
//! |---|---|---|---|
//! | `HtmlTag` with `kind: Normal` and no `close_span` | `html-unclosed-tag` | Error | Upstream renders an error box ("cannot find a closing tag"); Knot's parser consumes to EOF with live content and stays silent — the diagnostic restores upstream's verdict |
//! | Tokenizer errors inside a consumed start tag (`duplicate-attribute`, `missing-attribute-value`) | `html-duplicate-attribute` / `html-missing-attr-value` | Warning | Browsers tolerate both (first wins / valueless), but they are bug smells |
//! | An unterminated tag at EOF (`<div class="x`, never closed) | `html-unterminated-tag` | Warning | Upstream degrades to literal text, so the story still runs — a lint, not an error |
//! | An evaluation directive on `data-setter` (`@data-setter="…"`, `sc-eval:data-setter="…"`) | `html-directive-data-setter` | Error | Upstream throws before evaluating (engine `processAttributeDirectives`: `'evaluation directive is not allowed on the data-setter attribute'`) and renders an error box |
//! | A lone directive sigil (`@` / `sc-eval:` with no target name) | `html-directive-missing-target` | Error | Upstream strips to an empty name, reaches `setAttribute("")`, and throws the 'cannot transform' error — an error box either way |
//! | CSS parse errors + the lints in `style="…"` values | `css-parse` / `css-unknown-property` | Error / Warning | The three CSS-bearing surfaces (`<style>` bodies, `<<style>>`/`<<css>>` blocks, `style` attribute values) already *token* through `analyze_css`; this walk relays the diagnostics the token path dropped |
//! | Selector mistakes in CSS bodies (`:hvoer`, `::befor`, `::before:hover`) | `css-unknown-pseudo-class` / `css-unknown-pseudo-element` / `css-pseudo-element-placement` | Warning | Browsers silently drop the whole rule — the highest-value typo class CSS has; relayed with the other CSS diagnostics on every stylesheet surface |
//!
//! ## What stays silent (deliberate)
//!
//! - **Stray end tags** (`</div>` with no open element): upstream's htmlTag
//!   rule matches start tags only — the spelling renders as literal text.
//!   Parity wins over pedantry.
//! - **Prose `<`** (`5 < 6`, `a <3`): legal Twee source. The walk whitelists
//!   `eof-in-tag` only — never a blanket relay of tokenizer errors.
//! - **Literal regions**: `<nowiki>` bodies (Verbatim), `{{{ }}}` code
//!   blocks, raw `<script>`/`<style>` bodies, and `<<script>>` macro bodies
//!   are skipped — JS/CSS own those bytes, and literal content is not HTML
//!   to be judged.
//! - **Directive values** (`@style="…"`, `sc-eval:style="…"`): TwineScript,
//!   owned by the JS token families — same policy as the zoning engine.
//!   (The directive NAME is judged here — see the two `html-directive-*`
//!   rows — but the value's expression is not.)
//! - **Case-variant directive spellings** (`@DATA-SETTER`, `SC-EVAL:x`):
//!   upstream matches the prefixes case-sensitively
//!   (`"@" === name[0]`, `name.startsWith("sc-eval:")`), so these are
//!   ordinary attributes there — Knot mirrors and stays silent.
//!
//! ## Coordinate system
//!
//! All AST spans and `body_text` offsets are **body-relative**; every
//! emitted diagnostic is shifted by `body_offset_in_passage` so ranges are
//! passage-relative — the same contract `build_diagnostics` (macro
//! diagnostics) already uses. The pipeline then applies `passage_offset` at
//! the LSP boundary.
//!
//! ## Costs
//!
//! The walk re-parses the CSS fragments (small — style attributes and
//! style bodies are typically tens of bytes) and re-scans consumed start
//! tags for their leading tokenizer errors (bounded by the tag's own
//! extent). Both re-uses mirror the arithmetic of the token builder's
//! emission sites so spans stay in lockstep.

use crate::plugin::{FormatDiagnostic, FormatDiagnosticSeverity};
use crate::sugarcube::ast::{AstNode, HtmlTagKind};
use crate::sugarcube::css;
use knot_core::zoning::RawLanguage;

/// Walk a passage body's AST and append embedded HTML/CSS diagnostics.
///
/// Callers append the result to the same `diagnostics` list that
/// `token_builder::build_diagnostics` fills (macro diagnostics) — order
/// between the two walks is not meaningful; the server re-sorts by range
/// when rendering if it needs to.
pub fn build_embedded_diagnostics(
    nodes: &[AstNode],
    diagnostics: &mut Vec<FormatDiagnostic>,
    body_offset_in_passage: usize,
    body_text: &str,
) {
    walk_nodes(nodes, diagnostics, body_offset_in_passage, body_text);
}

fn walk_nodes(
    nodes: &[AstNode],
    diagnostics: &mut Vec<FormatDiagnostic>,
    off: usize,
    body_text: &str,
) {
    for node in nodes {
        match node {
            AstNode::HtmlTag {
                name,
                attrs,
                children,
                open_span,
                close_span,
                kind,
                raw_body,
                ..
            } => {
                // ── Unclosed element (upstream renders an error box) ──
                if *kind == HtmlTagKind::Normal && close_span.is_none() {
                    diagnostics.push(FormatDiagnostic {
                        range: off + open_span.start..off + open_span.end,
                        message: format!("Unclosed HTML tag: expected a matching </{name}>"),
                        severity: FormatDiagnosticSeverity::Error,
                        code: "html-unclosed-tag".to_string(),
                    });
                }

                // ── Start-tag tokenizer errors the scan skipped ──
                // The consumed tag's own extent bounds the re-scan, so the
                // leading errors (duplicate-attribute, missing-attribute-
                // value) arrive with slice-relative spans we shift by the
                // tag's start.
                if open_span.end <= body_text.len() {
                    let slice = &body_text[open_span.start..open_span.end];
                    let scan = knot_core::html::scan_leading_tag_detailed(slice);
                    for e in &scan.errors {
                        translate_tag_error(
                            e.code.as_str(),
                            off + open_span.start + e.span.start
                                ..off + open_span.start + e.span.end,
                            diagnostics,
                        );
                    }
                }

                // ── Attribute-directive validation (plan.md Phase 2.5
                // deferral, paid) ──
                // Both checks mirror the engine's
                // `processAttributeDirectives` exactly (verified against the
                // 2.37.3 bundle source): the `data-setter` comparison is on
                // the stripped name and case-SENSITIVE; a lone sigil strips
                // to an empty name and reaches `setAttribute("")` → the
                // 'cannot transform' throw. Both render an upstream error
                // box, hence Error severity here.
                for attr in attrs {
                    if attr
                        .directive
                        .as_ref()
                        .is_some_and(|d| d.target_name == "data-setter")
                    {
                        diagnostics.push(FormatDiagnostic {
                            range: off + attr.name_span.start..off + attr.name_span.end,
                            message: format!(
                                "Evaluation directive is not allowed on the data-setter \
                                 attribute: `{}` — remove the {} prefix",
                                attr.name,
                                match attr.directive.as_ref().unwrap().kind {
                                    crate::sugarcube::ast::HtmlAttrDirectiveKind::Shorthand => "@",
                                    crate::sugarcube::ast::HtmlAttrDirectiveKind::ScEval =>
                                        "sc-eval:",
                                }
                            ),
                            severity: FormatDiagnosticSeverity::Error,
                            code: "html-directive-data-setter".to_string(),
                        });
                    }
                    // Lone sigil: `attr.name` is the tokenizer's LOWERCASED
                    // name, so re-read the raw spelling from the body — a
                    // source `SC-EVAL:` must not fire (upstream's prefix
                    // match is case-sensitive; only the exact `sc-eval:`
                    // strips to an empty name and throws).
                    let raw_name = body_text
                        .get(attr.name_span.start..attr.name_span.end)
                        .unwrap_or(attr.name.as_str());
                    if raw_name == "@" || raw_name == "sc-eval:" {
                        diagnostics.push(FormatDiagnostic {
                            range: off + attr.name_span.start..off + attr.name_span.end,
                            message: format!(
                                "Evaluation directive is missing the attribute name: `{}`",
                                raw_name
                            ),
                            severity: FormatDiagnosticSeverity::Error,
                            code: "html-directive-missing-target".to_string(),
                        });
                    }
                }

                // ── Inline CSS: `style="…"` attribute values ──
                for attr in attrs {
                    if !attr.name.eq_ignore_ascii_case("style") || attr.directive.is_some() {
                        continue;
                    }
                    if let Some(vs) = &attr.value_span
                        && vs.end <= body_text.len()
                    {
                        let analysis = css::analyze_css_declarations(&body_text[vs.start..vs.end]);
                        for d in analysis.diagnostics {
                            diagnostics.push(FormatDiagnostic {
                                range: off + vs.start + d.range.start..off + vs.start + d.range.end,
                                message: d.message.clone(),
                                severity: d.severity,
                                code: css::css_diagnostic_code(&d.message).to_string(),
                            });
                        }
                    }
                }

                // ── Raw CSS body: `<style>` ──
                if *raw_body == Some(RawLanguage::Css) {
                    relay_raw_css_body(children, diagnostics, off);
                }

                walk_nodes(children, diagnostics, off, body_text);
            }

            AstNode::Macro {
                name,
                children: Some(children),
                ..
            } => {
                // ── Raw CSS body: `<<style>>` / `<<css>>` blocks ──
                if name.eq_ignore_ascii_case("style") || name.eq_ignore_ascii_case("css") {
                    relay_raw_css_body(children, diagnostics, off);
                }
                walk_nodes(children, diagnostics, off, body_text);
            }

            AstNode::Text {
                content,
                span,
                is_prose: true,
                ..
            } => {
                // ── Unterminated tag at EOF (prose only) ──
                // Every `<` + letter position inside prose text was already
                // scan-tested by the flat parser and failed — that is why
                // the spelling is text at all. Re-testing distinguishes the
                // authoring mistake (eof-in-tag) from legal prose `<`.
                relay_unterminated_tags(content, span.start, diagnostics, off);
            }

            // Wikified containers — recurse so their prose is linted too.
            AstNode::Heading { children, .. }
            | AstNode::ListItem { children, .. }
            | AstNode::Blockquote { children, .. }
            | AstNode::BlockquoteBlock { children, .. }
            | AstNode::InlineStyle { children, .. }
            | AstNode::TextFormat { children, .. } => {
                walk_nodes(children, diagnostics, off, body_text);
            }

            // Everything else (links, expressions, verbatim, code blocks,
            // errors, non-prose text) carries no embedded HTML/CSS to judge.
            _ => {}
        }
    }
}

/// Relay a raw CSS body's diagnostics for `<style>` elements and
/// `<<style>>`/`<<css>>` macro blocks.
///
/// Mirrors the token builder's emission arithmetic exactly (text children
/// concatenated, leading whitespace of the trimmed source re-added) so
/// the diagnostic ranges land on the same bytes the CSS tokens highlight.
fn relay_raw_css_body(children: &[AstNode], diagnostics: &mut Vec<FormatDiagnostic>, off: usize) {
    let mut css_source = String::new();
    let mut body_start = None;
    for child in children {
        if let AstNode::Text { content, span, .. } = child {
            if body_start.is_none() {
                body_start = Some(span.start);
            }
            css_source.push_str(content);
        }
    }
    let Some(start) = body_start else {
        return;
    };
    if css_source.trim().is_empty() {
        return;
    }
    let leading_ws = css_source.len() - css_source.trim_start().len();
    let analysis = css::analyze_css(css_source.trim());
    for d in analysis.diagnostics {
        diagnostics.push(FormatDiagnostic {
            range: off + start + leading_ws + d.range.start..off + start + leading_ws + d.range.end,
            message: d.message.clone(),
            severity: d.severity,
            code: css::css_diagnostic_code(&d.message).to_string(),
        });
    }
}

/// Scan prose text for `<` + letter positions whose tag never terminates
/// (`eof-in-tag`) and flag the whole unterminated spelling.
///
/// `content_offset` is the body-relative start of `content` (the Text
/// node's span). The diagnostic range covers the candidate `<` through the
/// end of the text node — html5gum's own span for `eof-in-tag` is empty (a
/// zero-width position at EOF), which would render as nothing in the
/// editor; the full spelling is both visible and more informative.
fn relay_unterminated_tags(
    content: &str,
    content_offset: usize,
    diagnostics: &mut Vec<FormatDiagnostic>,
    off: usize,
) {
    let bytes = content.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1].is_ascii_alphabetic() {
            let scan = knot_core::html::scan_leading_tag_detailed(&content[i..]);
            if scan.tag.is_none() && scan.errors.iter().any(|e| e.code == "eof-in-tag") {
                diagnostics.push(FormatDiagnostic {
                    range: off + content_offset + i..off + content_offset + bytes.len(),
                    message: "Unterminated HTML tag: the closing `>` is missing".to_string(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "html-unterminated-tag".to_string(),
                });
                // Nothing after this position can form another tag — the
                // unterminated one swallowed the rest of the text.
                return;
            }
        }
        i += 1;
    }
}

/// Translate a WHATWG tokenizer error code from a consumed start tag into
/// a diagnostic, whitelisting the two authoring-relevant codes.
///
/// Everything else (unexpected characters in attribute names, solidus
/// placement, …) is either tolerated by every browser or so exotic that
/// flagging it would be noise — see the module docs' "What stays silent".
fn translate_tag_error(
    code: &str,
    range: std::ops::Range<usize>,
    diagnostics: &mut Vec<FormatDiagnostic>,
) {
    let (message, diag_code) = match code {
        "duplicate-attribute" => (
            "Duplicate attribute: the first occurrence is used".to_string(),
            "html-duplicate-attribute",
        ),
        "missing-attribute-value" => (
            "Missing attribute value: the attribute is treated as valueless".to_string(),
            "html-missing-attr-value",
        ),
        _ => return,
    };
    diagnostics.push(FormatDiagnostic {
        range,
        message,
        severity: FormatDiagnosticSeverity::Warning,
        code: diag_code.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;

    fn diags(body: &str) -> Vec<FormatDiagnostic> {
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let mut out = Vec::new();
        build_embedded_diagnostics(&ast.nodes, &mut out, 0, body);
        out
    }

    fn texts(
        body: &str,
    ) -> Vec<(
        String,
        std::ops::Range<usize>,
        FormatDiagnosticSeverity,
        String,
    )> {
        diags(body)
            .into_iter()
            .map(|d| {
                let start = d.range.start.min(body.len());
                let end = d.range.end.min(body.len());
                (body[start..end].to_string(), d.range, d.severity, d.code)
            })
            .collect()
    }

    #[test]
    fn clean_passage_produces_nothing() {
        let body = "<div class=\"hud\" style=\"color: red;\">hi</div>[[Forest]]";
        assert!(texts(body).is_empty(), "got: {:?}", texts(body));
    }

    #[test]
    fn unclosed_element_is_an_error() {
        let body = "Hello <div class=\"hud\">never closed";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "html-unclosed-tag");
        assert_eq!(found[0].2, FormatDiagnosticSeverity::Error);
        assert!(found[0].0.starts_with("<div"), "range: {:?}", found[0].1);
    }

    #[test]
    fn void_and_self_closing_are_never_unclosed() {
        for body in ["line<br>", "an <img src=\"x.png\"> inline", "<hr/>"] {
            assert!(texts(body).is_empty(), "body: {body} → {:?}", texts(body));
        }
    }

    #[test]
    fn duplicate_attribute_warns() {
        let body = "<p a=\"1\" a=\"2\">x</p>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "html-duplicate-attribute");
        assert_eq!(found[0].2, FormatDiagnosticSeverity::Warning);
    }

    #[test]
    fn missing_attribute_value_warns() {
        let body = "<div a= >x</div>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "html-missing-attr-value");
    }

    #[test]
    fn unterminated_tag_at_eof_warns_over_whole_spelling() {
        let body = "Then it ended <div class=\"x";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "html-unterminated-tag");
        assert_eq!(found[0].2, FormatDiagnosticSeverity::Warning);
        // The range covers the whole unterminated spelling, not a
        // zero-width EOF position.
        assert_eq!(found[0].0, "<div class=\"x");
    }

    #[test]
    fn prose_less_than_stays_silent() {
        for body in ["5 < 6 stays text", "a <3 hearts"] {
            assert!(texts(body).is_empty(), "body: {body} → {:?}", texts(body));
        }
    }

    #[test]
    fn stray_end_tag_stays_silent() {
        // Upstream parity: the htmlTag rule matches start tags only, so a
        // stray closer is literal text — not a diagnostic.
        assert!(texts("oops </div> typo").is_empty());
    }

    #[test]
    fn style_attribute_relay_property_lint_only() {
        // Style-attribute PARSE errors are deliberately suppressed (the
        // declarations-microsyntax policy in core: browsers drop invalid
        // declarations silently). The property LINT still fires — that is
        // the tier this surface gets.
        //
        // A parse error inside a style attribute does NOT relay:
        let body = "<div style=\"color: 'red\">x</div>";
        assert!(
            texts(body).iter().all(|d| d.3 != "css-parse"),
            "parse errors stay suppressed in style attrs: {:?}",
            texts(body)
        );

        // A property typo inside a style attribute (lint tier).
        let body = "<span style=\"colr: red\">x</span>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "css-unknown-property");
        assert_eq!(found[0].0, "colr");
        assert_eq!(found[0].2, FormatDiagnosticSeverity::Warning);
    }

    #[test]
    fn style_body_relay_parse_error() {
        // A genuine CSS parse error (unclosed string) inside a <style>
        // body — this surface relays parse errors, unlike style attrs.
        let body = "<style>\n.hud { color: 'red; }\n</style>";
        let found = texts(body);
        assert!(
            found.iter().any(|d| d.3 == "css-parse"),
            "expected a parse error, got: {found:?}"
        );
    }

    #[test]
    fn style_directive_values_are_not_css() {
        // `@style` is a TwineScript directive — the JS families own it.
        assert!(texts("<div @style=\"$x ? 'a' : 'b'\">y</div>").is_empty());
    }

    #[test]
    fn style_body_relay_is_offset_correct() {
        let body = "<style>\n.hud { colr: red; }\n</style>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "css-unknown-property");
        assert_eq!(found[0].0, "colr");
        assert_eq!(found[0].2, FormatDiagnosticSeverity::Warning);
    }

    #[test]
    fn style_macro_block_relay() {
        let body = "<<style>>\n.hud { colr: red; }\n<</style>>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "css-unknown-property");
        assert_eq!(found[0].0, "colr");
    }

    #[test]
    fn nowiki_and_code_blocks_are_never_linted() {
        let body = "<nowiki><div></nowiki>\n{{{\n<div a= >\n}}}";
        assert!(texts(body).is_empty(), "got: {:?}", texts(body));
    }

    #[test]
    fn script_bodies_are_never_linted() {
        // A `<` + letter inside JS is code, not markup.
        let body = "<script>var a = b < c;</script>";
        assert!(texts(body).is_empty(), "got: {:?}", texts(body));
    }

    #[test]
    fn nested_elements_report_each_unclosed() {
        let body = "<div><span>text</div>";
        let found = texts(body);
        assert_eq!(found.len(), 1, "got: {found:?}");
        assert_eq!(found[0].3, "html-unclosed-tag");
        // The unclosed one is the <span> (the </div> closes the div).
        assert!(
            found[0].0.starts_with("<span"),
            "range covers: {:?}",
            found[0].0
        );
    }

    #[test]
    fn spans_are_passage_relative() {
        // With body_offset_in_passage = 100, every range shifts by it.
        let body = "<div><span>text</div>";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let mut out = Vec::new();
        build_embedded_diagnostics(&ast.nodes, &mut out, 100, body);
        assert_eq!(out.len(), 1);
        assert!(out[0].range.start >= 100, "range: {:?}", out[0].range);
    }

    // ── Attribute-directive diagnostics ───────────────────────────

    #[test]
    fn directive_on_data_setter_is_an_error() {
        // Upstream throws before evaluating (engine processAttribute-
        // Directives); the value expression is still parsed (the JS
        // families own it) but the spelling itself is broken.
        for body in [
            "<a data-passage=\"Go\" @data-setter=\"$x to 1\">go</a>",
            "<a data-passage=\"Go\" sc-eval:data-setter=\"$x to 1\">go</a>",
            // Valueless form — the upstream check fires before the
            // value is ever looked at.
            "<button data-passage=\"Go\" @data-setter>go</button>",
        ] {
            let found = texts(body);
            assert_eq!(found.len(), 1, "body: {body} → {found:?}");
            assert_eq!(found[0].3, "html-directive-data-setter");
            assert_eq!(found[0].2, FormatDiagnosticSeverity::Error);
            assert!(
                found[0].0.contains("data-setter"),
                "covers: {:?}",
                found[0].0
            );
        }
    }

    #[test]
    fn directive_case_variants_stay_silent() {
        // Upstream matches prefixes case-SENSITIVELY and compares the
        // stripped name exactly — `@DATA-SETTER` sets an ordinary
        // attribute instead, and `SC-EVAL:x` is not a directive at all.
        for body in [
            "<a data-passage=\"Go\" @DATA-SETTER=\"$x to 1\">go</a>",
            "<span SC-EVAL:id=\"_id\">x</span>",
            "<span Sc-Eval:id=\"_id\">x</span>",
        ] {
            assert!(texts(body).is_empty(), "body: {body} → {:?}", texts(body));
        }
    }

    #[test]
    fn lone_directive_sigil_is_an_error() {
        // `@`/`sc-eval:` with no target name strips to "" upstream →
        // setAttribute("") → the 'cannot transform' throw → error box.
        for body in [
            "<span @=\"_id\">x</span>",
            "<span @>x</span>",
            "<span sc-eval:=\"_id\">x</span>",
            "<span sc-eval:>x</span>",
        ] {
            let found = texts(body);
            assert_eq!(found.len(), 1, "body: {body} → {found:?}");
            assert_eq!(found[0].3, "html-directive-missing-target");
            assert_eq!(found[0].2, FormatDiagnosticSeverity::Error);
        }
    }

    #[test]
    fn valid_directives_stay_silent() {
        for body in [
            "<span @id=\"_id\">x</span>",
            "<span sc-eval:id=\"'pre-' + _id + '-suf'\">x</span>",
            "<a data-passage=\"Go\" data-setter=\"$x to 1\">go</a>",
            "<div @style=\"$x ? 'a' : 'b'\">y</div>",
        ] {
            assert!(texts(body).is_empty(), "body: {body} → {:?}", texts(body));
        }
    }

    // ── Selector lint relay (embedded surfaces) ───────────────────

    #[test]
    fn style_body_relay_selector_lints() {
        // A <style> body carrying the three selector-lint classes —
        // unknown class, unknown element, misplaced element.
        let body =
            "<style>\na:hvoer { color: red; }\np::befor { }\ndiv::before:hover { }\n</style>";
        let found = texts(body);
        let codes: Vec<&str> = found.iter().map(|d| d.3.as_str()).collect();
        assert!(
            codes.contains(&"css-unknown-pseudo-class"),
            "codes: {codes:?}"
        );
        assert!(
            codes.contains(&"css-unknown-pseudo-element"),
            "codes: {codes:?}"
        );
        assert!(
            codes.contains(&"css-pseudo-element-placement"),
            "codes: {codes:?}"
        );
        // The squiggles land on the selector spellings, not the whole body.
        assert!(found.iter().any(|d| d.0 == ":hvoer"), "covers: {found:?}");
        assert!(found.iter().any(|d| d.0 == "::befor"), "covers: {found:?}");
        assert!(found.iter().any(|d| d.0 == "::before"), "covers: {found:?}");
        assert!(
            found
                .iter()
                .all(|d| d.2 == FormatDiagnosticSeverity::Warning)
        );
    }

    #[test]
    fn valid_selectors_in_style_bodies_stay_silent() {
        let body = "<style>\n.hud:hover { color: red; }\np::before { content: 'x'; }\ninput:placeholder-shown::placeholder { }\n:-moz-any(.a) { }\n</style>";
        assert!(texts(body).is_empty(), "got: {:?}", texts(body));
    }
}
