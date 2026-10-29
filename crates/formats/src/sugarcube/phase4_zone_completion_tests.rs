//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).
//! Phase 4 tests: `build_macro_completions` migrated to use//! `passage.zones.body_stack_at()` instead of the O(document-length)//! `find_enclosing_block_macros` text scan.//!
//! These tests verify that SubMacro filtering (e.g., `<<else>>` only offered//! inside `<<if>>`) works correctly via the zone map, and that the zone-map//! path produces the same results as the old text-scan path.
use super::*;
use crate::FormatPluginMut;
use knot_core::Workspace;

/// Helper: parse a source document into a workspace + plugin.
fn parse(src: &str) -> (knot_core::Workspace, SugarCubePlugin, url::Url) {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let result = plugin.parse_mut(&uri, src);
    let mut workspace = Workspace::new(uri.clone());
    let mut doc = knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
    doc.passages = result.passages.clone();
    workspace.insert_document(doc);
    (workspace, plugin, uri)
}

/// Helper: get completion labels from `build_macro_completions` at a cursor offset.
fn macro_completion_labels(
    plugin: &SugarCubePlugin,
    workspace: &Workspace,
    uri: &url::Url,
    text: &str,
    byte_offset: usize,
) -> Vec<String> {
    let items = plugin.build_macro_completions(
        workspace,
        uri,
        "",
        CursorPos {
            line: 0,
            character: 0,
        },
        text,
        byte_offset,
    );
    items.iter().map(|i| i.label.clone()).collect()
}

/// `<<else>>` should appear in completions when cursor is inside `<<if>>`'s body.
#[test]
fn else_offered_inside_if_body() {
    let src = ":: Start\n<<if $x>>\n  <<\n<</if>>";
    let (workspace, plugin, uri) = parse(src);
    // Cursor after `<<` on line 2 (inside <<if>>'s body).
    let cursor_offset = src.find("  <<").unwrap() + 4; // after the `<<`
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    // Look for the standalone `<<else>>` completion (not the multi-form
    // `<<if>>…<<else>>…<</if>>` which is always offered for `<<if>>`).
    assert!(
        labels.iter().any(|l| l == "<<else>>"),
        "<<else>> should be offered inside <<if>> body, got: {:?}",
        labels
    );
}

/// `<<else>>` should NOT appear in completions when cursor is at top level
/// (not inside `<<if>>`).
#[test]
fn else_not_offered_at_top_level() {
    let src = ":: Start\n<<";
    let (workspace, plugin, uri) = parse(src);
    // Cursor after `<<` on line 1 (top level).
    let cursor_offset = src.find("<<").unwrap() + 2;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    // The standalone `<<else>>` label should NOT appear.
    assert!(
        !labels.iter().any(|l| l == "<<else>>"),
        "<<else>> should NOT be offered at top level, got: {:?}",
        labels
    );
}

/// `<<case>>` should appear in completions when cursor is inside `<<switch>>`'s body.
#[test]
fn case_offered_inside_switch_body() {
    let src = ":: Start\n<<switch $x>>\n  <<\n<</switch>>";
    let (workspace, plugin, uri) = parse(src);
    let cursor_offset = src.find("  <<").unwrap() + 4;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    assert!(
        labels
            .iter()
            .any(|l| l == "<<case>>" || l.starts_with("<<case ")),
        "<<case>> should be offered inside <<switch>> body, got: {:?}",
        labels
    );
}

/// `<<case>>` should NOT appear in completions when cursor is inside `<<if>>` (wrong parent).
#[test]
fn case_not_offered_inside_if_body() {
    let src = ":: Start\n<<if $x>>\n  <<\n<</if>>";
    let (workspace, plugin, uri) = parse(src);
    let cursor_offset = src.find("  <<").unwrap() + 4;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    assert!(
        !labels
            .iter()
            .any(|l| l == "<<case>>" || l.starts_with("<<case ")),
        "<<case>> should NOT be offered inside <<if>> (wrong parent), got: {:?}",
        labels
    );
}

/// `<<break>>` should appear inside `<<for>>` but NOT inside `<<if>>`.
#[test]
fn break_offered_inside_for_not_inside_if() {
    // Inside <<for>> — <<break>> should be offered.
    let src_for = ":: Start\n<<for _i to 0; _i lt 5; _i++>>\n  <<\n<</for>>";
    let (workspace, plugin, uri) = parse(src_for);
    let cursor_offset = src_for.find("  <<").unwrap() + 4;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src_for, cursor_offset);
    assert!(
        labels.iter().any(|l| l == "<<break>>"),
        "<<break>> should be offered inside <<for>>, got: {:?}",
        labels
    );

    // Inside <<if>> — <<break>> should NOT be offered.
    let src_if = ":: Start\n<<if $x>>\n  <<\n<</if>>";
    let (workspace, plugin, uri) = parse(src_if);
    let cursor_offset = src_if.find("  <<").unwrap() + 4;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src_if, cursor_offset);
    assert!(
        !labels.iter().any(|l| l == "<<break>>"),
        "<<break>> should NOT be offered inside <<if>>, got: {:?}",
        labels
    );
}

/// `<<else>>` should NOT be offered after `<<if>>` has closed (post-close-sibling case).
/// This is the same class of bug as §6.5 — the old text-scan would have
/// incorrectly thought the cursor was still inside `<<if>>`.
#[test]
fn else_not_offered_after_closed_if() {
    let src = ":: Start\n<<if $x>>\n  text\n<</if>>\n<<";
    let (workspace, plugin, uri) = parse(src);
    // Cursor after the final `<<` (top level, after <</if>>).
    let cursor_offset = src.rfind("<<").unwrap() + 2;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    assert!(
        !labels.iter().any(|l| l == "<<else>>"),
        "<<else>> should NOT be offered after <</if>> closed, got: {:?}",
        labels
    );
}

/// Nested context: `<<else>>` inside `<<if>>` inside `<<link>>` should be offered.
/// Tests that the zone map correctly walks up the parent chain.
#[test]
fn else_offered_in_nested_if_inside_link() {
    let src = ":: Start\n<<link \"Go\">>\n  <<if $x>>\n    <<\n  <</if>>\n<</link>>";
    let (workspace, plugin, uri) = parse(src);
    let cursor_offset = src.find("    <<").unwrap() + 6;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    assert!(
        labels.iter().any(|l| l == "<<else>>"),
        "<<else>> should be offered inside <<if>> inside <<link>>, got: {:?}",
        labels
    );
}

/// Verify that common macros are offered at top level.
#[test]
fn top_level_completions_include_common_macros() {
    let src = ":: Start\n<<";
    let (workspace, plugin, uri) = parse(src);
    let cursor_offset = src.find("<<").unwrap() + 2;
    let labels = macro_completion_labels(&plugin, &workspace, &uri, src, cursor_offset);
    // Common macros should be offered at top level. Labels are like
    // `<<set $var to value>>` (snippet form), so we check for `<<set`.
    for expected in &["set", "print", "if", "link", "for"] {
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with(&format!("<<{}", expected))),
            "<<{}>> should be offered at top level, got: {:?}",
            expected,
            labels
        );
    }
}
