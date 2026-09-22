//! Test battery for the HTML CST builder.
//!
//! Every test asserts **span → source-slice equality** (the strongest form:
//! `&src[range]` must equal the expected text), which pins html5gum's
//! measured span semantics against future upgrades — the module docs list
//! these as facts to re-verify on bump.

use super::parse_html_fragment;
use crate::html::types::{HtmlElementKind, HtmlNodeKind, HtmlRawKind};

/// Collect (kind-discriminant, slice) pairs for a compact overview assert.
fn outline(src: &str) -> Vec<String> {
    let cst = parse_html_fragment(src);
    let mut out = Vec::new();
    for node in &cst.nodes {
        node.walk(&mut |n| {
            let disc = match &n.kind {
                HtmlNodeKind::Element(el) => format!("Element({})", el.name),
                HtmlNodeKind::Text => "Text".to_string(),
                HtmlNodeKind::RawText(_) => "RawText".to_string(),
                HtmlNodeKind::Comment => "Comment".to_string(),
                HtmlNodeKind::Doctype => "Doctype".to_string(),
            };
            out.push(format!("{}: {}", disc, &src[n.range.clone()]));
        });
    }
    out
}

#[test]
fn nested_elements_with_text() {
    let src = r#"<div id="main"><p>Hello <b>world</b>!</p></div>"#;
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty());
    assert_eq!(cst.nodes.len(), 1);
    let div = &cst.nodes[0];
    assert_eq!(&src[div.range.clone()], src);
    let HtmlNodeKind::Element(div_el) = &div.kind else {
        panic!("expected element");
    };
    assert_eq!(div_el.name, "div");
    assert_eq!(&src[div_el.name_range.clone()], "div");
    assert_eq!(div_el.kind, HtmlElementKind::Normal);
    assert_eq!(
        div_el.end_tag_range.as_ref().map(|r| &src[r.clone()]),
        Some("</div>")
    );

    // div > p > [text, b, text]
    let p = &div.children[0];
    assert_eq!(p.element_name(), "p");
    let texts: Vec<&str> = p
        .children
        .iter()
        .filter(|c| matches!(c.kind, HtmlNodeKind::Text))
        .map(|c| &src[c.range.clone()])
        .collect();
    assert_eq!(texts, vec!["Hello ", "!"]);
    let b = &p.children[1];
    assert_eq!(b.element_name(), "b");
    assert_eq!(&src[b.range.clone()], "<b>world</b>");
}

#[test]
fn attributes_quoted_unquoted_valueless() {
    let src = r#"<div a="x y" b='z' c=w d>"#;
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty());
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(el.attrs.len(), 4);

    let a = &el.attrs[0];
    assert_eq!(a.name, "a");
    assert_eq!(&src[a.range.clone()], r#"a="x y""#);
    assert_eq!(&src[a.name_range.clone()], "a");
    assert_eq!(&src[a.value_range.clone().expect("value")], "x y");

    let b = &el.attrs[1];
    assert_eq!(&src[b.range.clone()], "b='z'");
    assert_eq!(&src[b.value_range.clone().expect("value")], "z");

    // Unquoted value: html5gum's raw span includes the terminator byte; the
    // builder corrects it, so `c=w` (not `c=w `).
    let c = &el.attrs[2];
    assert_eq!(&src[c.range.clone()], "c=w");
    assert_eq!(&src[c.value_range.clone().expect("value")], "w");

    // Valueless: range == name range, no value.
    let d = &el.attrs[3];
    assert_eq!(&src[d.range.clone()], "d");
    assert_eq!(d.value_range, None);
}

#[test]
fn attributes_with_whitespace_around_equals() {
    // WHATWG "before attribute value state" skips whitespace between `=`
    // and the value — browsers parse all four shapes identically, so the
    // CST must too (the value is exactly `b`/`z` in every case; the range
    // keeps the author's spelling).
    for (spelled, value) in [
        (r#"<div a ="b">x</div>"#, "a =\"b\""),
        (r#"<div a= "b">x</div>"#, "a= \"b\""),
        (r#"<div a = "b">x</div>"#, "a = \"b\""),
        (r#"<div a = 'b'>x</div>"#, "a = 'b'"),
    ] {
        let cst = parse_html_fragment(spelled);
        let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
            panic!("expected element in {spelled:?}");
        };
        assert_eq!(el.attrs.len(), 1, "in {spelled:?}");
        let attr = &el.attrs[0];
        assert_eq!(attr.name, "a");
        assert_eq!(
            &spelled[attr.range.clone()],
            value,
            "full range in {spelled:?}"
        );
        assert_eq!(
            &spelled[attr.value_range.clone().expect("value")],
            "b",
            "value in {spelled:?}"
        );
    }

    // Unquoted with whitespace after `=`: the value is `b`, not ` b`.
    let src = r#"<div a = b>x</div>"#;
    let cst = parse_html_fragment(src);
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(&src[el.attrs[0].range.clone()], "a = b");
    assert_eq!(&src[el.attrs[0].value_range.clone().expect("value")], "b");
}

#[test]
fn empty_quoted_value_collapses_to_valueless() {
    // html5gum collapses `a=""` to a name-only span — indistinguishable
    // from valueless. Pinned here (documented limitation in HtmlAttr docs).
    let src = r#"<div a="" b = ''>x</div>"#;
    let cst = parse_html_fragment(src);
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(el.attrs.len(), 2);
    for attr in &el.attrs {
        assert_eq!(attr.value_range, None, "attr {:?}", attr.name);
    }
    assert_eq!(&src[el.attrs[0].range.clone()], "a");
    assert_eq!(&src[el.attrs[1].range.clone()], "b");
}

#[test]
fn missing_attribute_value_is_valueless_with_error() {
    // `a= >` — the spec's missing-attribute-value error: html5gum records
    // the error and still emits the tag, with `a` collapsed to its name.
    let src = "<div a= >x</div>";
    let cst = parse_html_fragment(src);
    assert!(
        cst.errors
            .iter()
            .any(|e| e.code == "missing-attribute-value")
    );
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(el.name, "div");
    assert_eq!(el.attrs.len(), 1);
    assert_eq!(&src[el.attrs[0].range.clone()], "a");
    assert_eq!(el.attrs[0].value_range, None);
    // The content after the tag survives.
    assert_eq!(&src[cst.nodes[0].children[0].range.clone()], "x");
}

#[test]
fn attributes_with_sugarcube_symbols_stay_atomic() {
    // The issue #6 shapes: markup-looking bytes inside attribute values are
    // plain attribute content — they must never leak out as markup spans.
    let src = r#"<span title="a ''b'' c; @x" data-l="{{link}}">body</span>"#;
    let cst = parse_html_fragment(src);
    let span_node = &cst.nodes[0];
    let HtmlNodeKind::Element(el) = &span_node.kind else {
        panic!();
    };
    assert_eq!(el.name, "span");
    assert_eq!(
        &src[el.start_tag_range.clone()],
        r#"<span title="a ''b'' c; @x" data-l="{{link}}">"#
    );
    // Only the element's own content is a Text child.
    assert_eq!(&src[span_node.children[0].range.clone()], "body");
    // The double-quoted title survives quotes-in-quotes scanning.
    let title = &el.attrs[0];
    assert_eq!(title.name, "title");
    assert_eq!(
        &src[title.value_range.clone().expect("value")],
        "a ''b'' c; @x"
    );
}

#[test]
fn character_references_decode_in_value_but_ranges_stay_source_faithful() {
    let src = r#"<div title="a&amp;b c&#65;d">x &lt;y&gt;</div>"#;
    let cst = parse_html_fragment(src);
    let div = &cst.nodes[0];
    let HtmlNodeKind::Element(el) = &div.kind else {
        panic!();
    };
    // Attr VALUE range covers the RAW source bytes (`&amp;` = 5 bytes).
    let title = &el.attrs[0];
    assert_eq!(
        &src[title.value_range.clone().expect("value")],
        "a&amp;b c&#65;d"
    );
    // Text node range also covers raw source.
    let text = &div.children[0];
    assert!(matches!(text.kind, HtmlNodeKind::Text));
    assert_eq!(&src[text.range.clone()], "x &lt;y&gt;");
}

#[test]
fn script_style_bodies_are_raw_text() {
    let src =
        "<style>h1 { color: red; } /* <<not a macro>> */</style><script>var a = 1 < 2; ''</script>";
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty(), "errors: {:?}", cst.errors);
    let style = &cst.nodes[0];
    let HtmlNodeKind::Element(style_el) = &style.kind else {
        panic!();
    };
    assert_eq!(style_el.kind, HtmlElementKind::RawText(HtmlRawKind::Style));
    let HtmlNodeKind::RawText(kind) = &style.children[0].kind else {
        panic!("style body should be RawText");
    };
    assert_eq!(*kind, HtmlRawKind::Style);
    assert_eq!(
        &src[style.children[0].range.clone()],
        "h1 { color: red; } /* <<not a macro>> */"
    );

    let script = &cst.nodes[1];
    let HtmlNodeKind::Element(script_el) = &script.kind else {
        panic!();
    };
    assert_eq!(
        script_el.kind,
        HtmlElementKind::RawText(HtmlRawKind::Script)
    );
    assert_eq!(&src[script.children[0].range.clone()], "var a = 1 < 2; ''");
    // The `<` and `''` inside script did NOT become markup nodes.
    assert_eq!(script.children.len(), 1);
}

#[test]
fn textarea_is_rcdata() {
    let src = "<textarea>&amp;<b></textarea>";
    let cst = parse_html_fragment(src);
    let el = &cst.nodes[0];
    let HtmlNodeKind::Element(el_el) = &el.kind else {
        panic!();
    };
    assert_eq!(el_el.kind, HtmlElementKind::RawText(HtmlRawKind::TextArea));
    // `&amp;<b>` is all body (RCDATA: refs decoded in value, tags not parsed).
    assert_eq!(&src[el.children[0].range.clone()], "&amp;<b>");
}

#[test]
fn case_insensitive_names_ranges_unchanged() {
    let src = r#"<DIV CLASS="x">hi</DIV>"#;
    let cst = parse_html_fragment(src);
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(el.name, "div");
    assert_eq!(&src[el.name_range.clone()], "DIV");
    assert_eq!(el.attrs[0].name, "class");
    assert_eq!(&src[el.attrs[0].name_range.clone()], "CLASS");
    assert_eq!(
        el.end_tag_range.as_ref().map(|r| &src[r.clone()]),
        Some("</DIV>")
    );
}

#[test]
fn void_and_self_closing() {
    // NOTE: `<circle r=2/>` is NOT self-closing per spec — in an unquoted
    // attribute value the `/` is part of the value (`r=2/`), so the tag
    // ends without the self-closing flag. Use `r="2"/>` for self-closing.
    let src = "<p>a<br>b<img src=x>c<circle r=\"2\"/>d</p>";
    let cst = parse_html_fragment(src);
    let p = &cst.nodes[0];
    let kinds: Vec<(&str, HtmlElementKind)> = p
        .children
        .iter()
        .filter_map(|c| match &c.kind {
            HtmlNodeKind::Element(el) => Some((el.name.as_str(), el.kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("br", HtmlElementKind::Void),
            ("img", HtmlElementKind::Void),
            ("circle", HtmlElementKind::SelfClosing),
        ]
    );
    // `<br>` never opens a scope: siblings follow at p's level.
    assert!(
        p.children
            .iter()
            .any(|c| matches!(c.kind, HtmlNodeKind::Text) && &src[c.range.clone()] == "b")
    );
}

#[test]
fn misnesting_and_stray_end_tags() {
    // `<span>` closed implicitly when `</div>` arrives; stray `</span>` and
    // `</p>` ignored; no panic, no error.
    let src = "<div><span></div></span></p>x";
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty());
    assert_eq!(
        outline(src),
        vec![
            "Element(div): <div><span></div>",
            "Element(span): <span>",
            "Text: x",
        ]
    );
    let div = &cst.nodes[0];
    let span = &div.children[0];
    assert_eq!(span.element_name(), "span");
    let HtmlNodeKind::Element(span_el) = &span.kind else {
        panic!();
    };
    // Auto-closed: no end tag of its own.
    assert_eq!(span_el.end_tag_range, None);
}

#[test]
fn unclosed_elements_at_eof_are_auto_closed() {
    let src = "<div><p>tail";
    let cst = parse_html_fragment(src);
    let outline = outline(src);
    assert_eq!(
        outline,
        vec![
            "Element(div): <div><p>tail",
            "Element(p): <p>tail",
            "Text: tail",
        ]
    );
    let div = &cst.nodes[0];
    let HtmlNodeKind::Element(div_el) = &div.kind else {
        panic!();
    };
    assert_eq!(div_el.end_tag_range, None);
}

#[test]
fn unterminated_tag_at_eof_degrades_to_error_only() {
    // Spec: EOF in tag ⇒ error, tag dropped. Nothing panics; the text
    // before the tag survives.
    let src = "ok <div class=\"x";
    let cst = parse_html_fragment(src);
    assert_eq!(outline(src), vec!["Text: ok "]);
    assert!(cst.errors.iter().any(|e| e.code == "eof-in-tag"));
}

#[test]
fn stray_less_than_is_just_text() {
    let src = "a < b and 5<6 and <3 hearts";
    let cst = parse_html_fragment(src);
    assert_eq!(outline(src), vec![format!("Text: {}", src)]);
    // Spec parse errors recorded but that is all (see HtmlDiagnostic docs).
    assert!(
        cst.errors
            .iter()
            .all(|e| e.code == "invalid-first-character-of-tag-name")
    );
}

#[test]
fn comments_and_doctype() {
    let src = "<!DOCTYPE html><!-- c --d--><p>y</p>";
    assert_eq!(
        outline(src),
        vec![
            "Doctype: <!DOCTYPE html>",
            "Comment: <!-- c --d-->",
            "Element(p): <p>y</p>",
            "Text: y",
        ]
    );
}

#[test]
fn duplicate_attributes_keep_first_record_error() {
    let src = r#"<p a="1" a="2"></p>"#;
    let cst = parse_html_fragment(src);
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    assert_eq!(el.attrs.len(), 1);
    assert_eq!(&src[el.attrs[0].value_range.clone().expect("value")], "1");
    assert!(cst.errors.iter().any(|e| e.code == "duplicate-attribute"));
}

#[test]
fn quoted_gt_inside_attribute_does_not_end_tag() {
    let src = r#"<div title="a>b">x</div>"#;
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty());
    let div = &cst.nodes[0];
    let HtmlNodeKind::Element(el) = &div.kind else {
        panic!();
    };
    assert_eq!(&src[el.start_tag_range.clone()], r#"<div title="a>b">"#);
    assert_eq!(div.children.len(), 1);
    assert_eq!(&src[div.children[0].range.clone()], "x");
}

#[test]
fn attrs_are_in_source_order_not_alphabetical() {
    let src = r#"<div z="1" a="2" m="3">"#;
    let cst = parse_html_fragment(src);
    let HtmlNodeKind::Element(el) = &cst.nodes[0].kind else {
        panic!();
    };
    let names: Vec<&str> = el.attrs.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["z", "a", "m"]);
}

#[test]
fn fragment_forest_and_bare_prose() {
    let src = "plain <b>bold</b> tail";
    assert_eq!(
        outline(src),
        vec![
            "Text: plain ",
            "Element(b): <b>bold</b>",
            "Text: bold",
            "Text:  tail",
        ]
    );
}

#[test]
fn empty_and_whitespace_input() {
    assert!(parse_html_fragment("").nodes.is_empty());
    let cst = parse_html_fragment("   \n\t");
    assert_eq!(outline("   \n\t"), vec!["Text:    \n\t".to_string()]);
    assert!(cst.errors.is_empty());
    let none = parse_html_fragment("   \n\t");
    let _ = none; // same result; parse is total on whitespace-only input
}

#[test]
fn svg_fragment_typical_sugarcube_usage() {
    // The issue #6 reporter's shape: SVG with attribute directives.
    let src = r#"<svg viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" @class="hud"/><text x="1" y="2">hp</text></svg>"#;
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty(), "errors: {:?}", cst.errors);
    let svg = &cst.nodes[0];
    assert_eq!(svg.element_name(), "svg");
    let circle = &svg.children[0];
    assert_eq!(circle.element_name(), "circle");
    let HtmlNodeKind::Element(circle_el) = &circle.kind else {
        panic!();
    };
    // The `@class` directive is an ordinary attribute at the CST level —
    // directive *semantics* are the Phase 2.2 stratum's job.
    assert_eq!(circle_el.attrs[3].name, "@class");
    assert_eq!(
        &src[circle_el.attrs[3].value_range.clone().expect("value")],
        "hud"
    );
    assert_eq!(&src[svg.children[1].children[0].range.clone()], "hp");
    let _ = cst;
}

#[test]
fn rawtext_with_nested_lookalike_tags() {
    // `</div>` inside script must not close anything.
    let src = "<div><script>if (a</div) { b }</script></div>";
    let cst = parse_html_fragment(src);
    assert!(cst.errors.is_empty(), "errors: {:?}", cst.errors);
    let div = &cst.nodes[0];
    assert_eq!(div.children.len(), 1);
    let script = &div.children[0];
    assert_eq!(script.element_name(), "script");
    assert_eq!(&src[script.children[0].range.clone()], "if (a</div) { b }");
}

// ---------------------------------------------------------------------------
// scan_leading_tag — the Phase 2.2 format-layer service
// ---------------------------------------------------------------------------

use super::scan_leading_tag;

#[test]
fn scan_open_tag_extents_and_attrs() {
    let src = r#"<div @class="hud" style='color: red;' plain data-x=a>b</div>"#;
    let tag = scan_leading_tag(src).expect("tag");
    assert!(!tag.is_end);
    assert!(!tag.self_closing);
    assert_eq!(tag.name, "div");
    assert_eq!(&src[tag.name_range.clone()], "div");
    assert_eq!(
        &src[tag.range.clone()],
        r#"<div @class="hud" style='color: red;' plain data-x=a>"#
    );
    // Source order, not alphabetical.
    let names: Vec<&str> = tag.attrs.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["@class", "style", "plain", "data-x"]);
    assert_eq!(&src[tag.attrs[0].value_range.clone().expect("v")], "hud");
    assert_eq!(
        &src[tag.attrs[1].value_range.clone().expect("v")],
        "color: red;"
    );
    // Valueless attribute.
    assert!(tag.attrs[2].value_range.is_none());
    assert_eq!(&src[tag.attrs[3].value_range.clone().expect("v")], "a");
}

#[test]
fn scan_end_tag() {
    let src = "</DIV>\nrest";
    let tag = scan_leading_tag(src).expect("end tag");
    assert!(tag.is_end);
    assert_eq!(tag.name, "div");
    assert_eq!(&src[tag.range.clone()], "</DIV>");
    assert!(tag.attrs.is_empty());
}

#[test]
fn scan_self_closing_and_void() {
    let tag = scan_leading_tag("<circle @class=\"hud\"/>").expect("tag");
    assert!(tag.self_closing);
    assert_eq!(tag.name, "circle");
    let tag = scan_leading_tag("<br>").expect("tag");
    assert!(
        !tag.self_closing,
        "void-ness is the format layer's call (upstream list)"
    );
    assert_eq!(tag.name, "br");
}

#[test]
fn scan_rejects_non_tags() {
    // Prose with a stray `<` (spec: invalid-first-character-of-tag-name).
    assert!(scan_leading_tag("5 < 6").is_none());
    assert!(scan_leading_tag("<3 hearts").is_none());
    // Whitespace after `<`.
    assert!(scan_leading_tag("< div>").is_none());
    // Comment / doctype / processing instruction — not tags for this API.
    assert!(scan_leading_tag("<!-- c -->").is_none());
    assert!(scan_leading_tag("<!DOCTYPE html>").is_none());
    assert!(scan_leading_tag("<?xml version=\"1.0\"?>").is_none());
    // Unterminated tag at EOF: the tokenizer drops it (spec), the scan
    // must agree — upstream's htmlTag regex requires the closing `>`.
    assert!(scan_leading_tag("<div class=\"x").is_none());
    assert!(scan_leading_tag("</div").is_none());
    // Empty input.
    assert!(scan_leading_tag("").is_none());
}

#[test]
fn scan_skips_leading_error_tokens() {
    // html5gum may emit an error token BEFORE the tag it belongs to; the
    // scan's question is "is there a tag here?", so those tags must scan.
    //
    // `a= >` — missing-attribute-value: the tag is still a tag.
    let src = "<div a= >x</div>";
    let tag = scan_leading_tag(src).expect("missing-attribute-value tag");
    assert_eq!(tag.name, "div");
    assert_eq!(&src[tag.range.clone()], "<div a= >");
    assert_eq!(tag.attrs.len(), 1);
    assert_eq!(tag.attrs[0].value_range, None);

    // Duplicate attribute: the error precedes the tag; the FIRST value wins.
    let src = r#"<p a="1" a="2">x</p>"#;
    let tag = scan_leading_tag(src).expect("duplicate-attribute tag");
    assert_eq!(tag.name, "p");
    assert_eq!(&src[tag.range.clone()], r#"<p a="1" a="2">"#);
    assert_eq!(tag.attrs.len(), 1);
    assert_eq!(&src[tag.attrs[0].value_range.clone().expect("value")], "1");
}

#[test]
fn scan_quoted_gt_and_symbols_in_values() {
    // Quoted `>` inside a value must not terminate the tag early.
    let src = r#"<span title="a ''b'' c" data-x="x>>y">t</span>"#;
    let tag = scan_leading_tag(src).expect("tag");
    assert_eq!(
        &src[tag.range.clone()],
        r#"<span title="a ''b'' c" data-x="x>>y">"#
    );
    assert_eq!(
        &src[tag.attrs[0].value_range.clone().expect("v")],
        "a ''b'' c"
    );
    assert_eq!(&src[tag.attrs[1].value_range.clone().expect("v")], "x>>y");
}

// scan_leading_tag_detailed — the reporting variant (Phase 2.5 diagnostics)

#[test]
fn detailed_scan_reports_errors_the_plain_scan_skips() {
    use super::scan_leading_tag_detailed;

    // Missing-attribute-value: error + the tag. Both must arrive.
    let scan = scan_leading_tag_detailed("<div a= >");
    let tag = scan.tag.expect("tag");
    assert_eq!(tag.name, "div");
    assert!(
        scan.errors
            .iter()
            .any(|e| e.code == "missing-attribute-value"),
        "codes: {:?}",
        scan.errors
            .iter()
            .map(|e| e.code.as_str())
            .collect::<Vec<_>>()
    );

    // Duplicate attribute: error + the tag (first value wins).
    let scan = scan_leading_tag_detailed(r#"<p a="1" a="2">"#);
    assert!(scan.tag.is_some());
    assert!(
        scan.errors.iter().any(|e| e.code == "duplicate-attribute"),
        "codes: {:?}",
        scan.errors
            .iter()
            .map(|e| e.code.as_str())
            .collect::<Vec<_>>()
    );

    // Unterminated at EOF: NO tag, eof-in-tag recorded with a non-empty span.
    let scan = scan_leading_tag_detailed("<div class=\"x");
    assert!(scan.tag.is_none());
    let eof = scan
        .errors
        .iter()
        .find(|e| e.code == "eof-in-tag")
        .expect("eof-in-tag");
    // Pinned: html5gum's eof-in-tag span is EMPTY at the EOF position
    // (13..13 for a 13-byte source). Consumers that relay it as a
    // diagnostic must derive their own, richer range (e.g. the whole
    // unterminated spelling) — this pin keeps that contract explicit.
    assert_eq!(eof.span, 13..13, "span: {:?}", eof.span);

    // Prose `<`: no tag, and the errors are the prose class — consumers
    // must whitelist, not blanket-relay (pinned so the contract is explicit).
    let scan = scan_leading_tag_detailed("5 < 6");
    assert!(scan.tag.is_none());
    assert!(
        !scan.errors.iter().any(|e| e.code == "eof-in-tag"),
        "prose must not read as an unterminated tag: {:?}",
        scan.errors
    );
}

#[test]
fn detailed_scan_matches_plain_scan_on_clean_input() {
    use super::{scan_leading_tag, scan_leading_tag_detailed};

    for src in ["<div a=\"1\">", "</div>", "<circle/>", "<br>"] {
        let plain = scan_leading_tag(src);
        let detailed = scan_leading_tag_detailed(src);
        assert_eq!(plain, detailed.tag, "src: {src}");
        assert!(detailed.errors.is_empty(), "src: {src}");
    }
}
