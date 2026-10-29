//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).
//! ---------------------------------------------------------------------------//! Temp-variable passage-scope tests//!
//! SugarCube `_` variables are passage-scoped at runtime: `_foo` declared//! in `:: Start` is invisible to `:: Inventory`. These tests verify the//! LSP completion path mirrors that semantics — typing `_` in one passage//! must not surface temps from any other passage.//!
//! Coverage://!   - `find_enclosing_passage_name` (backward scan for `::` headers)//!   - `VariableTree::completion_names_for_passage` (passage-scoped query)//!   - `SugarCubeRegistry::variable_names_for_passage` (registry wrapper)//!   - `build_variable_completions` (signature threads passage context)//!   - `provide_completions` end-to-end (`_` trigger in scoped passage)//!
//! Regression: before the fix, `completion_names()` walked all `temp_roots`//! indiscriminately, leaking temps across passages.//! ---------------------------------------------------------------------------
use super::*;
use crate::sugarcube::registries::variable_tree::VariableTree;
use knot_core::Workspace;

/// `find_enclosing_passage_name` should resolve the nearest preceding
/// `::` header. Cursor on the header line itself returns that
/// passage's name (matches `find_passage_header_at_position`).
#[test]
fn enclosing_passage_resolves_from_body_line() {
    let text = ":: Start\nHello world\n:: Inventory\nYou have items";
    // Line 1 (body of Start) — should resolve to "Start".
    assert_eq!(
        find_enclosing_passage_name(text, 1).as_deref(),
        Some("Start"),
        "body line in :: Start should resolve to Start"
    );
    // Line 3 (body of Inventory) — should resolve to "Inventory".
    assert_eq!(
        find_enclosing_passage_name(text, 3).as_deref(),
        Some("Inventory"),
        "body line in :: Inventory should resolve to Inventory"
    );
}

/// Cursor on the header line itself returns that passage's name.
#[test]
fn enclosing_passage_on_header_line_returns_that_passage() {
    let text = ":: Start\n:: Inventory\n:: End";
    assert_eq!(
        find_enclosing_passage_name(text, 0).as_deref(),
        Some("Start")
    );
    assert_eq!(
        find_enclosing_passage_name(text, 1).as_deref(),
        Some("Inventory")
    );
    assert_eq!(find_enclosing_passage_name(text, 2).as_deref(), Some("End"));
}

/// When the cursor is in the pre-story preamble (no `::` header above),
/// the helper returns `None`. Callers treat `None` as "no passage
/// scope" and degrade to persistent-only completions — never leaking
/// another passage's temps.
#[test]
fn enclosing_passage_returns_none_in_preamble() {
    let text = "/* story stylesheet */\n:: Start\nHello";
    assert_eq!(find_enclosing_passage_name(text, 0), None);
}

/// `VariableTree::completion_names_for_passage` must include the
/// passage's own temps plus all persistent vars, and exclude temps
/// declared in any other passage.
#[test]
fn completion_names_for_passage_isolates_temps() {
    let mut tree = VariableTree::new();

    // Persistent vars (workspace-global — always visible).
    tree.record_var(
        "$gold",
        false,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..20,
        "",
        "",
        &[],
        None,
        &[],
    );

    // Temp vars in Start.
    tree.record_var(
        "_local_to_start",
        true,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..30,
        "",
        "",
        &[],
        None,
        &[],
    );

    // Temp vars in Inventory — must NOT leak into Start's completions.
    tree.record_var(
        "_local_to_inventory",
        true,
        VarAccessKind::Write,
        "Inventory",
        "file:///test.twee",
        10..30,
        "",
        "",
        &[],
        None,
        &[],
    );

    let start_names = tree.completion_names_for_passage(Some("Start"));
    assert!(
        start_names.contains("$gold"),
        "persistent $gold should be visible from Start"
    );
    assert!(
        start_names.contains("_local_to_start"),
        "Start's own temp should be visible from Start"
    );
    assert!(
        !start_names.contains("_local_to_inventory"),
        "Inventory's temp must NOT leak into Start's completions"
    );

    // Symmetric check from Inventory's perspective.
    let inv_names = tree.completion_names_for_passage(Some("Inventory"));
    assert!(inv_names.contains("$gold"));
    assert!(inv_names.contains("_local_to_inventory"));
    assert!(!inv_names.contains("_local_to_start"));
}

/// `None` passage scope degrades to persistent-only — never leaks.
#[test]
fn completion_names_for_passage_none_returns_persistent_only() {
    let mut tree = VariableTree::new();
    tree.record_var(
        "$gold",
        false,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..20,
        "",
        "",
        &[],
        None,
        &[],
    );
    tree.record_var(
        "_temp",
        true,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..20,
        "",
        "",
        &[],
        None,
        &[],
    );

    let names = tree.completion_names_for_passage(None);
    assert!(
        names.contains("$gold"),
        "persistent vars should be returned even with no passage scope"
    );
    assert!(
        !names.contains("_temp"),
        "temps must NOT be returned when passage scope is None"
    );
}

/// `SugarCubeRegistry::variable_names_for_passage` delegates correctly
/// to the underlying `VariableTree` — same isolation semantics.
#[test]
fn registry_variable_names_for_passage_isolates_temps() {
    let mut plugin = SugarCubePlugin::new();
    {
        let tree = plugin.registry_mut().variables_mut();
        tree.record_var(
            "$shared",
            false,
            VarAccessKind::Write,
            "Start",
            "file:///test.twee",
            10..20,
            "",
            "",
            &[],
            None,
            &[],
        );
        tree.record_var(
            "_a_start",
            true,
            VarAccessKind::Write,
            "Start",
            "file:///test.twee",
            10..20,
            "",
            "",
            &[],
            None,
            &[],
        );
        tree.record_var(
            "_b_other",
            true,
            VarAccessKind::Write,
            "Other",
            "file:///test.twee",
            10..20,
            "",
            "",
            &[],
            None,
            &[],
        );
    }

    let names = plugin.registry().variable_names_for_passage(Some("Start"));
    assert!(names.contains("$shared"));
    assert!(names.contains("_a_start"));
    assert!(!names.contains("_b_other"));
}

/// End-to-end: typing `_` inside `:: Start` triggers temp-var
/// completions. Only `Start`'s own temps should appear; `Inventory`'s
/// temps must not leak through.
///
/// Regression test for the original bug: `completion_names()` walked
/// all `temp_roots` indiscriminately, so `_inv_only` would have
/// appeared while editing `Start`.
#[test]
fn provide_completions_underscore_trigger_scoped_to_passage() {
    let mut plugin = SugarCubePlugin::new();
    {
        let tree = plugin.registry_mut().variables_mut();
        tree.record_var(
            "$shared",
            false,
            VarAccessKind::Write,
            "Start",
            "file:///test.twee",
            10..20,
            "",
            "",
            &[],
            None,
            &[],
        );
        tree.record_var(
            "_start_only",
            true,
            VarAccessKind::Write,
            "Start",
            "file:///test.twee",
            10..30,
            "",
            "",
            &[],
            None,
            &[],
        );
        tree.record_var(
            "_inv_only",
            true,
            VarAccessKind::Write,
            "Inventory",
            "file:///test.twee",
            10..30,
            "",
            "",
            &[],
            None,
            &[],
        );
    }

    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Text: cursor on line 1 (body of Start), right after typing `_`.
    // `:: Start\n_`
    //  0123456789 → char 1 of line 1 is the position right after `_`.
    let text = ":: Start\n_";
    let items = plugin.provide_completions(
        text,
        &workspace,
        &uri,
        1,
        1, // line 1, char 1 (after the `_`)
        Some('_'),
        &[],
    );

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"_start_only"),
        "Start's own temp should be in completions, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"_inv_only"),
        "Inventory's temp must NOT leak into Start's `_` completions, got: {:?}",
        labels
    );
    // Persistent vars are NOT shown when is_temp=true (filter excludes
    // them) — this is by design, the sigil determines the var class.
    assert!(
        !labels.iter().any(|l| l.starts_with('$')),
        "`_` trigger should not surface persistent `$` vars, got: {:?}",
        labels
    );
}

/// Symmetric: typing `_` inside `:: Inventory` shows Inventory's temps
/// and excludes Start's. Confirms the fix isn't accidentally filtering
/// to a single hard-coded passage.
#[test]
fn provide_completions_underscore_trigger_scoped_to_other_passage() {
    let mut plugin = SugarCubePlugin::new();
    {
        let tree = plugin.registry_mut().variables_mut();
        tree.record_var(
            "_start_only",
            true,
            VarAccessKind::Write,
            "Start",
            "file:///test.twee",
            10..30,
            "",
            "",
            &[],
            None,
            &[],
        );
        tree.record_var(
            "_inv_only",
            true,
            VarAccessKind::Write,
            "Inventory",
            "file:///test.twee",
            10..30,
            "",
            "",
            &[],
            None,
            &[],
        );
    }

    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let text = ":: Start\n:: Inventory\n_";
    let items = plugin.provide_completions(
        text,
        &workspace,
        &uri,
        2,
        1, // line 2, char 1 (after `_` in Inventory)
        Some('_'),
        &[],
    );

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"_inv_only"),
        "Inventory's own temp should be in completions, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"_start_only"),
        "Start's temp must NOT leak into Inventory's `_` completions, got: {:?}",
        labels
    );
}

/// Path-index collision regression: two passages define `_x` with
/// different property shapes. Before namespacing, the second
/// `path_index.entry("_x").or_insert(...)` was a no-op (first-write
/// wins), so `kind_at_path("_x")` would always return the first
/// passage's shape regardless of which passage was querying. Dot
/// completion (`_x.`) in the second passage would show the wrong
/// properties.
///
/// With namespaced keys, each passage's `_x` lives under its own
/// key and resolves independently.
#[test]
fn path_index_no_collision_between_same_named_temps() {
    let mut tree = VariableTree::new();

    // Start's _x: Object with `hp` property.
    tree.record_var(
        "_x",
        true,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..30,
        "hp",
        "",
        &[],
        None,
        &[],
    );

    // Inventory's _x: Object with `gold` property (different shape).
    tree.record_var(
        "_x",
        true,
        VarAccessKind::Write,
        "Inventory",
        "file:///test.twee",
        10..30,
        "gold",
        "",
        &[],
        None,
        &[],
    );

    // From Start's scope: kind resolves, children are `hp`.
    let kind_start = tree.kind_at_path_for_passage("_x", Some("Start"));
    assert!(
        kind_start.is_some(),
        "Start's _x should resolve from Start's scope"
    );
    let children_start = tree.children_with_kind_for_passage("_x", Some("Start"));
    let child_names_start: Vec<&str> = children_start.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        child_names_start.contains(&"hp"),
        "Start's _x should have `hp` child, got: {:?}",
        child_names_start
    );
    assert!(
        !child_names_start.contains(&"gold"),
        "Start's _x must NOT have Inventory's `gold` child, got: {:?}",
        child_names_start
    );

    // From Inventory's scope: same name, different children.
    let kind_inv = tree.kind_at_path_for_passage("_x", Some("Inventory"));
    assert!(
        kind_inv.is_some(),
        "Inventory's _x should resolve from Inventory's scope"
    );
    let children_inv = tree.children_with_kind_for_passage("_x", Some("Inventory"));
    let child_names_inv: Vec<&str> = children_inv.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        child_names_inv.contains(&"gold"),
        "Inventory's _x should have `gold` child, got: {:?}",
        child_names_inv
    );
    assert!(
        !child_names_inv.contains(&"hp"),
        "Inventory's _x must NOT have Start's `hp` child, got: {:?}",
        child_names_inv
    );
}

/// Path-index lookup without passage scope returns `None` for temp
/// paths — safe degradation that prevents leaking any passage's
/// temp. Persistent paths still resolve as before.
#[test]
fn path_index_temp_lookup_without_passage_returns_none() {
    let mut tree = VariableTree::new();
    tree.record_var(
        "_x",
        true,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..30,
        "hp",
        "",
        &[],
        None,
        &[],
    );
    tree.record_var(
        "$player",
        false,
        VarAccessKind::Write,
        "Start",
        "file:///test.twee",
        10..30,
        "hp",
        "",
        &[],
        None,
        &[],
    );

    // Temp without passage scope → None (safe).
    assert!(
        tree.get_node_by_path("_x").is_none(),
        "temp path without passage scope must return None"
    );
    assert!(
        tree.kind_at_path("_x").is_none(),
        "kind_at_path for temp without passage scope must return None"
    );

    // Persistent still works through the legacy API.
    assert!(
        tree.get_node_by_path("$player").is_some(),
        "persistent path should still resolve through legacy API"
    );
    assert!(
        tree.kind_at_path("$player").is_some(),
        "kind_at_path for persistent should still work"
    );
}

/// Ctrl+Space (no trigger) on a partial temp variable identifier
/// should fire variable completions — not just when the cursor is on
/// a parsed VarOp span. Users type `_name` in prose, in macro args,
/// and anywhere else a variable can appear; restricting completion to
/// parsed VarOp spans models variable completion after passage-name
/// completion, which is incorrect.
///
/// Regression: before this fix, Ctrl+Space mid-name on a temp in
/// prose returned passage names (the fallback path) instead of
/// variable completions.
#[test]
fn provide_completions_ctrl_space_partial_temp_in_prose() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let src = ":: Start\n<<set _startTemp to 1>>\nYou have _startTemp coins.\n:: Inventory\n<<set _invTemp to 2>>\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // Cursor mid-name on _startTemp in prose (line 2, char 13).
    // Line 2: "You have _startTemp coins."
    //          0123456789012345
    // char 13 = 'r' (mid _startTemp)
    let items = plugin.provide_completions(src, &workspace, &uri, 2, 13, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"_startTemp"),
        "Ctrl+Space mid-name in prose should show _startTemp, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"_invTemp"),
        "Ctrl+Space in Start prose must NOT leak _invTemp from Inventory, got: {:?}",
        labels
    );
    // Should NOT show passage names — this is a variable context.
    assert!(
        !labels.contains(&"Start") && !labels.contains(&"Inventory"),
        "Ctrl+Space on partial variable should not fall through to passage names, got: {:?}",
        labels
    );
}

/// Ctrl+Space on a partial temp var identifier respects passage scope.
/// Cursor on `_inv` in Inventory prose shows _invTemp, NOT _startTemp.
#[test]
fn provide_completions_ctrl_space_partial_temp_respects_scope() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let src = ":: Start\n<<set _startTemp to 1>>\n:: Inventory\n<<set _invTemp to 2>>\nYou have _invTemp coins.\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // Line 4 = "You have _invTemp coins." — char 13 = mid _invTemp
    let items = plugin.provide_completions(src, &workspace, &uri, 4, 13, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"_invTemp"),
        "Ctrl+Space in Inventory prose should show _invTemp, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"_startTemp"),
        "Ctrl+Space in Inventory prose must NOT leak _startTemp from Start, got: {:?}",
        labels
    );
}

/// Ctrl+Space at the END of a temp var name (cursor right after last
/// char, before any whitespace) fires variable completions. The
/// parsed VarOp span ends at the last char, so the cursor at position
/// == span.end was previously not considered "on span" and fell
/// through to the passage-name fallback.
#[test]
fn provide_completions_ctrl_space_at_end_of_temp_name() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let src = ":: Start\n<<set _startTemp to 1>>\n:: Inventory\n<<set _invTemp to 2>>\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // Line 1: "<<set _startTemp to 1>>"
    //          0123456789012345678
    // char 16 = 'p' (last char of _startTemp)
    // char 17 = ' ' (space after name)
    let items = plugin.provide_completions(src, &workspace, &uri, 1, 16, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"_startTemp"),
        "Ctrl+Space at end of _startTemp name should show _startTemp, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"_invTemp"),
        "Ctrl+Space at end of _startTemp must NOT leak _invTemp, got: {:?}",
        labels
    );
}

/// Ctrl+Space past the variable name (on whitespace or other text)
/// does NOT fire variable completions — the partial must extend to
/// the cursor. This prevents false positives like `<<set _x to |>>`
/// (cursor after space) firing variable completions.
#[test]
fn provide_completions_ctrl_space_past_name_does_not_fire() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let src = ":: Start\n<<set _startTemp to 1>>\n:: Inventory\n<<set _invTemp to 2>>\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // char 17 = ' ' (space after _startTemp) — cursor on whitespace,
    // partial "_startTemp" doesn't extend to cursor → should NOT fire.
    let items = plugin.provide_completions(src, &workspace, &uri, 1, 17, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        !labels.contains(&"_startTemp"),
        "Ctrl+Space on whitespace after _startTemp should NOT fire variable completion, got: {:?}",
        labels
    );
}

/// Ctrl+Space on a passage header line (`:: _name`) does NOT fire
/// temp-var completion — the `_` in `:: _passage` is a passage name,
/// not a temp var sigil.
#[test]
fn provide_completions_ctrl_space_in_passage_header_not_temp() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    // Cursor on `:: _somePassage` — the _ is part of the passage name
    let src = ":: Start\n<<set _x to 1>>\n:: _somePassage\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // Line 2 = ":: _somePassage", char 4 = 'o' (mid _somePassage)
    let items = plugin.provide_completions(src, &workspace, &uri, 2, 4, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        !labels.contains(&"_x"),
        "Ctrl+Space on passage header `:: _name` should NOT fire temp-var completion, got: {:?}",
        labels
    );
}

/// Same partial-identifier behavior for persistent `$` vars in prose.
#[test]
fn provide_completions_ctrl_space_partial_persistent_in_prose() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let src = ":: Start\n<<set $gold to 10>>\nYou have $gold coins.\n:: Inventory\n<<set $silver to 5>>\n";
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(Url::parse("file:///project/").unwrap());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);

    // Line 2: "You have $gold coins."
    //          0123456789012
    // char 12 = 'l' (mid $gold)
    let items = plugin.provide_completions(src, &workspace, &uri, 2, 12, None, &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"$gold"),
        "Ctrl+Space on $gold in prose should show $gold, got: {:?}",
        labels
    );
    // Persistent vars are workspace-global — $silver SHOULD appear.
    // This is intentional: $ vars survive across passages by design.
    assert!(
        labels.contains(&"$silver"),
        "Persistent $silver from Inventory should be visible from Start (global scope), got: {:?}",
        labels
    );
}
