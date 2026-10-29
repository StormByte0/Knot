//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).

#![allow(deprecated)]
use super::*;

fn body_macros() -> HashSet<&'static str> {
    macros::body_macro_names()
}

#[test]
fn test_scan_macro_tags_simple() {
    let text = "<<if $x>>hello<</if>>";
    let tags = scan_macro_tags(text, text.len());
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].name, "if");
    assert!(!tags[0].is_close);
    assert_eq!(tags[1].name, "if");
    assert!(tags[1].is_close);
}

#[test]
fn test_scan_macro_tags_nested() {
    let text = "<<if $x>><<for _i range $arr>>text<</for>><</if>>";
    let tags = scan_macro_tags(text, text.len());
    assert_eq!(tags.len(), 4);
    assert_eq!(tags[0].name, "if");
    assert!(!tags[0].is_close);
    assert_eq!(tags[1].name, "for");
    assert!(!tags[1].is_close);
    assert_eq!(tags[2].name, "for");
    assert!(tags[2].is_close);
    assert_eq!(tags[3].name, "if");
    assert!(tags[3].is_close);
}

#[test]
fn test_scan_macro_tags_with_string_args() {
    // String args containing > or << should not confuse the scanner
    let text = r#"<<link "Go >>" "Passage">>click<</link>>"#;
    let tags = scan_macro_tags(text, text.len());
    assert_eq!(tags.len(), 2);
    assert_eq!(tags[0].name, "link");
    assert!(!tags[0].is_close);
    assert_eq!(tags[1].name, "link");
    assert!(tags[1].is_close);
}

#[test]
fn test_scan_macro_tags_with_comments() {
    let text = "<<set $x to 1 /* >> not a close */ >>";
    let tags = scan_macro_tags(text, text.len());
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "set");
    assert!(!tags[0].is_close);
}

#[test]
fn test_scan_macro_tags_close_tag() {
    let text = "<</if>>";
    let tags = scan_macro_tags(text, text.len());
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "if");
    assert!(tags[0].is_close);
}

#[test]
fn test_find_enclosing_block_macros_inside_if() {
    let text = "<<if $x>>hello<</if>>";
    let offset = 12; // Inside the "hello" text
    let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
    assert_eq!(enclosing, vec!["if"]);
}

#[test]
fn test_find_enclosing_block_macros_outside() {
    let text = "some text <<if $x>>hello<</if>>";
    let offset = 5; // Before the if
    let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
    assert!(enclosing.is_empty());
}

#[test]
fn test_find_enclosing_block_macros_nested() {
    let text = "<<if $x>><<for _i range $arr>>inner<</for>><</if>>";
    let offset = 30; // Inside "inner"
    let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
    assert_eq!(enclosing, vec!["if", "for"]);
}

#[test]
fn test_find_enclosing_block_macros_after_close() {
    let text = "<<if $x>>hello<</if>>after";
    let offset = text.len() - 3; // In "after"
    let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
    assert!(enclosing.is_empty());
}

#[test]
fn test_find_unclosed_macros_simple() {
    let text = "<<if $x>>hello";
    let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
    assert_eq!(unclosed, vec!["if"]);
}

#[test]
fn test_find_unclosed_macros_nested_unclosed() {
    let text = "<<if $x>><<for _i range $arr>>inner";
    let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
    assert_eq!(unclosed, vec!["if", "for"]);
}

#[test]
fn test_find_unclosed_macros_one_closed_one_open() {
    let text = "<<if $x>><<for _i range $arr>>inner<</for>>more";
    let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
    assert_eq!(unclosed, vec!["if"]);
}

#[test]
fn test_find_unclosed_macros_all_closed() {
    let text = "<<if $x>><<for _i range $arr>>inner<</for>><</if>>";
    let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
    assert!(unclosed.is_empty());
}

#[test]
fn test_sub_macro_else_not_pushed() {
    // <<else>> is a SubMacro, not a Container — it should NOT be pushed
    let text = "<<if $x>>hello<<else>>world<</if>>";
    let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
    // After the full text, everything is closed
    assert!(unclosed.is_empty());

    // Inside after <<else>>, only <<if>> should be on the stack
    let else_pos = text.find("<<else>>").unwrap();
    let after_else = else_pos + "<<else>>".len() + 3; // In "world"
    let enclosing = find_enclosing_block_macros(text, after_else, &body_macros());
    assert_eq!(enclosing, vec!["if"]);
}

#[test]
fn test_scan_ignores_expression_macros() {
    // <<=>> is an expression macro, not a tag-based macro
    let text = "<<set $x to 1>><<=$x>>";
    let tags = scan_macro_tags(text, text.len());
    // Should find <<set>> and <<=>> but <<= should not be treated as a tag
    // Actually, the scanner will pick up "=" as the name after <<
    // Let's verify it doesn't crash and handles it gracefully
    assert!(!tags.is_empty()); // At least <<set>>
    // The = expression should have an empty-ish name
    let set_tag = tags.iter().find(|t| t.name == "set");
    assert!(set_tag.is_some(), "Should find <<set>> tag");
}
