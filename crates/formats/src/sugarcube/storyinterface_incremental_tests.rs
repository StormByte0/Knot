//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).

use super::*;

/// The incremental single-passage re-parse (what every WithinPassage
/// keystroke runs) must serve the SAME StoryInterface body tokens as
/// the full-document parse.
///
/// Regression: `parse_single`'s Interface arm was left as a "token
/// serving still a follow-up" stub when the full parse started
/// serving them, so every incremental edit replaced the StoryInterface
/// token group with header-only tokens — the body highlighting
/// flickered off on each keystroke until the next full re-parse.
#[test]
fn incremental_reparse_serves_storyinterface_body_tokens() {
    let src = concat!(
        ":: StoryInterface\n",
        "<div id=\"hud\"><span>$score</span></div>\n",
        ":: Start\n",
        "[[Go]]\n",
        ":: Go\n",
        "Done.\n",
    );
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///guard/story.twee").unwrap();

    // Full-document parse — the reference token set.
    let full = plugin.parse_mut(&uri, src);
    let full_group = full
        .token_groups
        .iter()
        .find(|g| g.passage_name == "StoryInterface")
        .expect("full parse emits a StoryInterface token group");
    let header_len = ":: StoryInterface\n".len();
    assert!(
        full_group.tokens.iter().any(|t| t.start >= header_len),
        "full parse must serve body tokens, got: {:?}",
        full_group.tokens
    );

    // Incremental re-parse of just that passage — exactly what
    // did_change's WithinPassage path does on every keystroke.
    let inc = plugin
        .parse_passage_mut(
            "StoryInterface",
            &[],
            ":: StoryInterface\n<div id=\"hud\"><span>$score</span></div>\n",
            uri.as_str(),
            0,
        )
        .expect("incremental parse must succeed");
    let inc_group = inc
        .token_groups
        .iter()
        .find(|g| g.passage_name == "StoryInterface")
        .expect("incremental parse emits a StoryInterface token group");

    // Token-for-token identical to the full parse (same offsets,
    // lengths, types, modifiers) — not just "some" tokens.
    let key = |t: &crate::plugin::SemanticToken| (t.start, t.length, t.token_type, t.modifier);
    let full_keys: Vec<_> = full_group.tokens.iter().map(key).collect();
    let inc_keys: Vec<_> = inc_group.tokens.iter().map(key).collect();
    assert_eq!(
        inc_keys, full_keys,
        "incremental StoryInterface tokens must match the full parse"
    );
    assert!(
        inc_keys.iter().any(|(start, ..)| *start >= header_len),
        "body tokens must survive the incremental re-parse, got: {inc_keys:?}"
    );
}
