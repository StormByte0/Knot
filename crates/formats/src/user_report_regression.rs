//! One-shot verification: the user's literal StoryInterface passage must
//! produce ZERO diagnostics and full HTML tokens. Run with:
//! cargo test -p knot-formats user_report_regression -- --nocapture

#![cfg(test)]
#[test]
fn user_report_regression() {
    use knot_core::passage::StoryFormat;
    use url::Url;

    let src = ":: StoryInterface\n<div id=\"app-shell\" class=\"app-shell\">\n        <!-- Region 1: Left sidebar \u{2014} app controls (buttons) + info widgets.\n             Populated by the PanelLeft passage via SugarCube's native\n             data-passage mechanism: re-rendered on every passage display and\n             on every UI.update() call. -->\n        <aside id=\"left-sidebar\" class=\"sidebar sidebar--left\" data-passage=\"PanelLeft\"></aside>\n\n        <!-- Regions 2 + 3: Main area -->\n        <main id=\"main-area\" class=\"main-area\">\n                <!-- Region 2: Scene canvas. Layers are created/managed by the\n                     scene module (50-scene.twee) \u{2014} this element stays empty. -->\n                <div id=\"scene-canvas\" class=\"scene-canvas\"></div>\n\n                <!-- Region 3: Story content (owned by SugarCube's engine) -->\n                <div id=\"passages\" class=\"story-content\"></div>\n        </main>\n\n        <!-- Region 4: Right sidebar \u{2014} populated by the PanelRight passage. -->\n        <aside id=\"right-sidebar\" class=\"sidebar sidebar--right\" data-passage=\"PanelRight\"></aside>\n\n        <!-- Region 5: Floating window (tabbed; inventory, quests, \u{2026}) -->\n        <div id=\"floating-window\" class=\"floating-window\" aria-hidden=\"true\">\n                <header class=\"floating-window__header\">\n                        <nav id=\"fw-tabs\" class=\"floating-window__tabs\"></nav>\n                        <button id=\"fw-close\" class=\"floating-window__close\" type=\"button\" aria-label=\"Close\">&times;</button>\n                </header>\n                <div id=\"fw-content\" class=\"floating-window__content\"></div>\n        </div>\n</div>\n";

    let mut registry = crate::plugin::FormatRegistry::with_defaults();
    let plugin = registry.get_mut(&StoryFormat::SugarCube).unwrap();
    let uri = Url::parse("file:///user-report/01-shell.twee").unwrap();
    let result = plugin.parse_mut(&uri, src);

    let diags: Vec<String> = result
        .diagnostic_groups
        .iter()
        .filter(|g| g.passage_name == "StoryInterface")
        .flat_map(|g| g.diagnostics.iter().map(|d| d.message.clone()))
        .collect();
    println!("diagnostics: {diags:?}");
    assert!(diags.is_empty(), "false diagnostics: {diags:?}");

    let group = result
        .token_groups
        .iter()
        .find(|g| g.passage_name == "StoryInterface")
        .expect("token group");
    let names: Vec<&str> = group
        .tokens
        .iter()
        .map(|t| t.token_type.lsp_name())
        .collect();
    println!("token types: {}", names.join(" "));
    // `&times;` here is element TEXT, not an attribute value — prose-level
    // entities are rendered literally by SugarCube and deliberately left
    // untokened (see token_builder::find_html_entities), so htmlEntity is
    // not expected in this passage.
    for expected in ["htmlTag", "htmlAttribute", "htmlDelimiter", "string"] {
        assert!(
            names.contains(&expected),
            "missing `{expected}` tokens: {names:?}"
        );
    }
    println!("OK: {} tokens, zero false diagnostics", group.tokens.len());
}
