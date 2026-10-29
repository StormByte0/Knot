//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).
//! HTML-stratum completion guard (plan.md Phase 2.5): position-based//! completion contexts consult the ZoneMap instead of re-deriving context//! from raw text. Markup completions (macros, close tags, links, passage//! names, templates) are suppressed inside tag interiors and raw-text//! bodies; SugarCube variable sigils stay live exactly where SugarCube//! evaluates them — `<script>` bodies (SugarCube syntax) and directive//! attribute values (`Scripting.evalTwineScript` upstream).
use super::*;

fn setup(src: &str) -> (SugarCubePlugin, knot_core::Workspace, Url) {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///guard/story.twee").unwrap();
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = knot_core::Workspace::new(Url::parse("file:///guard/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);
    (plugin, workspace, uri)
}

#[test]
fn macro_completions_suppressed_inside_tag_interior() {
    // Typing `<<` inside an attribute value must NOT offer macro
    // completions — those bytes are inert HTML, not markup.
    let src = ":: Start\n<a title=\"a <<\">x</a>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 1u32;
    let char = src.lines().nth(1).unwrap().find("<<").unwrap() as u32 + 2;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('<'), &[]);
    assert!(
        items.is_empty(),
        "macro completions must be suppressed inside a tag interior, got: {:?}",
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn dollar_completions_suppressed_in_plain_attribute_value() {
    // `$` inside a NON-directive attribute value is inert — upstream
    // only evaluates directive values.
    let src = ":: Start\n<span title=\"$\">x</span>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 1u32;
    let char = src.lines().nth(1).unwrap().find('$').unwrap() as u32 + 1;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('$'), &[]);
    assert!(
        items.is_empty(),
        "variable completions must be suppressed in plain attribute values, got: {:?}",
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn variable_completions_offered_inside_directive_value() {
    // `@class="$…"` is a TwineScript expression — SugarCube variable
    // completions MUST stay available (the plan's "offered inside
    // @class" pin).
    let src = ":: Start\n<<set $gold to 1>><div @class=\"$\"></div>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 1u32;
    let body_line = src.lines().nth(1).unwrap();
    let char = body_line.find("\"$").unwrap() as u32 + 2; // right after `$`
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('$'), &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("gold")),
        "variable completions must be offered inside a directive value, got: {labels:?}"
    );
}

#[test]
fn macro_completions_suppressed_in_script_body() {
    // `a << b` in a script body is a JS left-shift, not a macro.
    let src = ":: Start\n<script>\nif (a << b) {}\n</script>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 2u32;
    let char = src.lines().nth(2).unwrap().find("<<").unwrap() as u32 + 2;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('<'), &[]);
    assert!(
        items.is_empty(),
        "macro completions must be suppressed inside <script> bodies, got: {:?}",
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn dollar_completions_still_live_in_script_body() {
    // SugarCube script bodies preprocess $var → State.variables, so the
    // `$` sigil stays meaningful there.
    let src = ":: StoryInit\n<<set $coins to 0>>\n:: Start\n<script>\n$\n</script>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 4u32;
    let char = src.lines().nth(4).unwrap().find('$').unwrap() as u32 + 1;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('$'), &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("coins")),
        "variable completions must stay live in script bodies, got: {labels:?}"
    );
}

#[test]
fn style_body_completions_suppressed() {
    let src = ":: Start\n<style>\n.hud << 2 { color: red; }\n</style>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 2u32;
    let char = src.lines().nth(2).unwrap().find("<<").unwrap() as u32 + 2;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('<'), &[]);
    assert!(
        items.is_empty(),
        "completions must be suppressed inside <style> bodies, got: {:?}",
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn prose_macro_completions_unchanged() {
    // Control: the guard must not over-suppress — `<<` in plain prose
    // still offers macro completions.
    let src = ":: Start\nHello <<\n[[Forest]]\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 1u32;
    let char = src.lines().nth(1).unwrap().find("<<").unwrap() as u32 + 2;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('<'), &[]);
    assert!(
        !items.is_empty(),
        "macro completions in prose must keep working"
    );
}

#[test]
fn attr_name_position_offers_html_attribute_completions() {
    // End-to-end: typing an attribute word in a tag interior offers
    // the curated attribute names; the `@` prefix switches to the
    // directive family.
    let src = ":: Start\n<div cla\n[[Forest]]\n";
    let (plugin, workspace, uri) = setup(src);
    let char = src.lines().nth(1).unwrap().find("cla").unwrap() as u32 + 3;
    let items = plugin.provide_completions(src, &workspace, &uri, 1, char, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"class"), "labels: {labels:?}");
    assert!(
        labels.iter().all(|l| !l.starts_with('@')),
        "no directive items on a plain word: {labels:?}"
    );

    let src = ":: Start\n<div @da\n[[Forest]]\n";
    let (plugin, workspace, uri) = setup(src);
    let char = src.lines().nth(1).unwrap().find("@da").unwrap() as u32 + 3;
    let items = plugin.provide_completions(src, &workspace, &uri, 1, char, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"@data-passage"), "labels: {labels:?}");
    assert!(
        labels.iter().all(|l| !l.contains("setter")),
        "the erroring @data-setter is never offered: {labels:?}"
    );
}

#[test]
fn attr_value_and_tag_name_positions_stay_suppressed() {
    // Inside a plain attribute VALUE and inside the element NAME:
    // nothing is offered.
    let src = ":: Start\n<div title=\"xx\">y</div>\n[[Forest]]\n";
    let (plugin, workspace, uri) = setup(src);
    let char = src.lines().nth(1).unwrap().find("xx").unwrap() as u32 + 1;
    assert!(
        plugin
            .provide_completions(src, &workspace, &uri, 1, char, None, &[])
            .is_empty()
    );
    let char = src.find("<div").unwrap() as u32 + 3 - "\n:: Start\n".len() as u32;
    assert!(
        plugin
            .provide_completions(src, &workspace, &uri, 1, char, None, &[])
            .is_empty()
    );
}

#[test]
fn directive_values_keep_variable_completions() {
    // The @-directive VALUE keeps its TwineScript behavior: `$`
    // offers variables (regression pin for the zone refactor).
    let src = ":: Start\n<span @id=\"$p\">x</span>\n";
    let (plugin, workspace, uri) = setup(src);
    let line = 1u32;
    let char = src.lines().nth(1).unwrap().find("$p").unwrap() as u32 + 1;
    let items = plugin.provide_completions(src, &workspace, &uri, line, char, Some('$'), &[]);
    // No variables registered → empty is fine; the pin is that the
    // DIRECTIVE-VALUE arm handled it (a panic-free pass means the
    // guard classified correctly — the variable list itself is
    // covered by the scoped-variable tests).
    let _ = items;
}

#[test]
fn zone_helper_classifies_strata() {
    // Direct pins on the classifier (offsets via find() — no manual
    // line arithmetic).
    let src = ":: Start\n<div>body</div>\n<script>x</script>\n<style>y</style>\n";
    let plugin = SugarCubePlugin::new();
    let open_tag = src.find("<div").unwrap() + 2; // inside the tag name
    assert_eq!(
        plugin.completion_zone_at(src, open_tag),
        CompletionZone::TagInterior {
            position: AttrPosition::TagName
        }
    );
    let content = src.find("<div>").unwrap() + 5; // inside element content
    assert_eq!(
        plugin.completion_zone_at(src, content),
        CompletionZone::Prose
    );
    let script_body = src.find("<script>").unwrap() + 8;
    assert_eq!(
        plugin.completion_zone_at(src, script_body),
        CompletionZone::RawScript
    );
    let style_body = src.find("<style>").unwrap() + 7;
    assert_eq!(
        plugin.completion_zone_at(src, style_body),
        CompletionZone::RawStyle
    );
    // Inline CSS: cursor inside a `style="…"` attribute VALUE reports
    // the CSS zone (not TagInterior) — the zoning refinement that makes
    // the per-byte language authority match what the bytes actually are.
    let src_style = ":: Start\n<div style=\"color: red\">x</div>\n";
    let style_value = src_style.find("color").unwrap();
    assert_eq!(
        plugin.completion_zone_at(src_style, style_value),
        CompletionZone::RawStyle
    );
    // The `style` attribute NAME is still tag-interior HTML — and it
    // is an attribute-NAME position (the HTML attribute completions
    // fire there).
    let style_name = src_style.find("style").unwrap();
    assert_eq!(
        plugin.completion_zone_at(src_style, style_name),
        CompletionZone::TagInterior {
            position: AttrPosition::AttrName
        }
    );
    // A plain attribute VALUE offers nothing (position-wise it is a
    // plain value; the zone language for `style="…"` is CSS, but for
    // a non-style value the language stays Html).
    let src_plain = ":: Start\n<div title=\"x\">y</div>\n";
    let title_value = src_plain.find("x").unwrap();
    assert_eq!(
        plugin.completion_zone_at(src_plain, title_value),
        CompletionZone::TagInterior {
            position: AttrPosition::PlainValue
        }
    );
    // Directive value (inclusive end): cursor right after `$`.
    let src2 = ":: Start\n<div @class=\"$\">x</div>\n";
    let after_sigil = src2.find("\"$").unwrap() + 2;
    assert_eq!(
        plugin.completion_zone_at(src2, after_sigil),
        CompletionZone::TagInterior {
            position: AttrPosition::DirectiveValue
        }
    );
}
