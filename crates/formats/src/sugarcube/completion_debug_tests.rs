//! Test module moved verbatim from the former `sugarcube/mod.rs`
//! monolith (pure move, plan §4.6 — flat sibling so the module
//! path, and therefore any insta snapshot names, are unchanged).

use super::*;
use crate::types::FormatCompletionKind;
use knot_core::Workspace;

/// Test that `provide_completions` returns macro items when cursor is after `<<`.
#[test]
fn test_macro_completions_after_double_angle() {
    let plugin = SugarCubePlugin::new();
    let text = "<<";
    let line = 0u32;
    let character = 2u32; // cursor after "<<"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Test with no trigger (Ctrl+Space)
    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    assert!(
        !items.is_empty(),
        "Expected macro completions after <<, got {} items",
        items.len()
    );

    // Check that at least some items have Function kind (macro completions)
    let macro_items: Vec<_> = items
        .iter()
        .filter(|i| matches!(i.kind, FormatCompletionKind::Function))
        .collect();
    assert!(
        !macro_items.is_empty(),
        "Expected Function-typed macro completions, got none"
    );
}

/// Test that `build_macro_completions` directly returns items.
#[test]
fn test_build_macro_completions_direct() {
    let plugin = SugarCubePlugin::new();
    let text = "<<";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.build_macro_completions(
        &workspace,
        &uri,
        "",
        CursorPos {
            line: 0,
            character: 2,
        },
        text,
        2,
    );
    assert!(
        !items.is_empty(),
        "build_macro_completions returned {} items, expected > 0",
        items.len()
    );
}

/// Test `<` as trigger character when cursor is after `<<`.
#[test]
fn test_macro_completions_with_angle_trigger() {
    let plugin = SugarCubePlugin::new();
    let text = "<<";
    let line = 0u32;
    let character = 2u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('<'), &[]);
    assert!(
        !items.is_empty(),
        "Expected macro completions with < trigger after <<, got {} items",
        items.len()
    );
}

/// Test macro completions in a realistic passage document.
#[test]
fn test_macro_completions_in_passage() {
    let plugin = SugarCubePlugin::new();
    // Simulate a passage with text then <<
    let text = ":: Start\nSome text <<";
    let line = 1u32; // second line (after passage header)
    let character = 12u32; // cursor after "<<"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    assert!(
        !items.is_empty(),
        "Expected macro completions inside passage after <<, got {} items",
        items.len()
    );
    let macro_items: Vec<_> = items
        .iter()
        .filter(|i| matches!(i.kind, FormatCompletionKind::Function))
        .collect();
    assert!(
        !macro_items.is_empty(),
        "Expected Function-typed macro completions inside passage, got none"
    );
}

/// Test that text_edit ranges are valid (not out-of-bounds).
#[test]
fn test_macro_completions_text_edit_ranges_valid() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello <<";
    let line = 1u32;
    let character = 9u32; // cursor after "<<"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    assert!(!items.is_empty(), "Expected completions");

    for item in &items {
        if let Some(te) = &item.text_edit {
            // start should be <= end, and both should be on the same line
            assert!(
                te.start_line == te.end_line,
                "text_edit spans multiple lines: start={}, end={}",
                te.start_line,
                te.end_line
            );
            assert!(
                te.start_character <= te.end_character,
                "text_edit start > end: start={}, end={}",
                te.start_character,
                te.end_character
            );
            // start should not be negative (would underflow to huge number via saturating_sub)
            // but the range should also make sense relative to the line
        }
    }
}

/// Test that text_edit correctly handles auto-closed `>>` after cursor.
#[test]
fn test_macro_completions_auto_close_handling() {
    let plugin = SugarCubePlugin::new();
    // User typed `<<` and auto-close added `>>`, cursor is between them
    let text = ":: Start\nHello <<>>";
    let line = 1u32;
    let character = 8u32; // cursor after "<<", before ">>"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    assert!(!items.is_empty(), "Expected completions with auto-close >>");

    // The text_edit should cover the auto-closed ">>" (end_character = character + 2)
    for item in &items {
        if let Some(te) = &item.text_edit {
            // The textEdit should extend past the cursor to cover ">>"
            assert!(
                te.end_character >= character,
                "textEdit end ({}) should be >= cursor ({}) to consume auto-close >>",
                te.end_character,
                character
            );
            // For auto-close case, the textEdit should replace the ">>"
            // So end should be character + 2
            assert_eq!(
                te.end_character,
                character + 2,
                "textEdit should cover auto-closed >>: end={}, expected={}",
                te.end_character,
                character + 2
            );
        }
    }
}

/// Test that textEdit does NOT cover `<<` (the key fix for VS Code filtering).
#[test]
fn test_macro_text_edit_excludes_angle_brackets() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<";
    let line = 1u32;
    let character = 2u32; // cursor after "<<"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    assert!(!items.is_empty(), "Expected completions");

    // The textEdit start should NOT be before the cursor (i.e., should not cover <<)
    for item in &items {
        if let Some(te) = &item.text_edit {
            assert!(
                te.start_character >= character.saturating_sub(0),
                "textEdit start ({}) should not cover << before cursor ({})",
                te.start_character,
                character
            );
            // For empty prefix, start should be at cursor position (no prefix to replace)
            assert_eq!(
                te.start_character, character,
                "textEdit start should be at cursor position for empty prefix: start={}, cursor={}",
                te.start_character, character
            );
        }
    }
}
/// Test that single `[` (without `[[`) does NOT trigger passage completions.
#[test]
fn test_single_bracket_no_passage_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [";
    let line = 1u32;
    let character = 7u32; // cursor after single "["
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    // Single `[` should NOT trigger passage name completions
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        passage_items.is_empty(),
        "Single [ should not trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that Ctrl+Space in plain prose does NOT show passage names.
///
/// Before the scoping fix, `build_default_completions` included ALL
/// passage names as a fallback when no specific context was detected.
/// This caused passage names to appear when the user hit Ctrl+Space
/// in plain prose — confusing because bare passage names in prose
/// are just text, not references. Passage completions should only
/// appear inside `[[link]]` syntax or macro passage-ref args.
#[test]
fn test_ctrl_space_in_prose_no_passage_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nYou are in a forest.\n";
    let line = 1u32;
    let character = 10u32; // cursor in the middle of prose
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest", "Cave", "Town"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, None, &[]);
    // No passage names should appear in the default completions
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        passage_items.is_empty(),
        "Ctrl+Space in prose should NOT show passage names, got {} passage items: {:?}",
        passage_items.len(),
        passage_items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

/// Test that `[[` DOES trigger passage completions.
#[test]
fn test_double_bracket_triggers_passage_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [[";
    let line = 1u32;
    let character = 8u32; // cursor after "[["
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "[[ should trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that partial passage name after `[[` triggers completions with filter.
#[test]
fn test_partial_passage_name_after_double_bracket() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [[Gar";
    let line = 1u32;
    let character = 11u32; // cursor after "Gar"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    // Should return passage completions filtered by "Gar"
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "[[Gar should trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that pipe-link syntax triggers passage completions for the passage name part.
#[test]
fn test_pipe_link_triggers_passage_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [[Go to|";
    let line = 1u32;
    let character = 14u32; // cursor after "|"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    // Should return passage completions (pipe-link context)
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "[[display| should trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that single `<` (without `<<`) does NOT trigger macro completions.
#[test]
fn test_single_angle_no_macro_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\na < b";
    let line = 1u32;
    let character = 3u32; // cursor after single "<"
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('<'), &[]);
    assert!(
        items.is_empty(),
        "Single < should not trigger macro completions, got {} items",
        items.len()
    );
}

/// Test that `"` inside a passage-ref macro triggers passage completions.
#[test]
fn test_quote_in_passage_ref_macro_triggers_completions() {
    let plugin = SugarCubePlugin::new();
    // <<goto " — the " is the trigger, and <<goto has is_passage_ref arg
    let text = ":: Start\n<<goto \"";
    let line = 1u32;
    let character = 8u32; // cursor after the "
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "\" in <<goto should trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that `"` in normal text does NOT trigger passage completions.
#[test]
fn test_quote_in_normal_text_no_passage_completions() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHe said \"";
    let line = 1u32;
    let character = 10u32; // cursor after the "
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        passage_items.is_empty(),
        "\" in normal text should NOT trigger passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that Ctrl+Space (no trigger) on a Link semantic token returns passage completions.
#[test]
fn test_ctrl_space_on_link_token_returns_passage_completions() {
    let plugin = SugarCubePlugin::new();
    // Text: ":: Start\nHello [[Garden]]"
    // The word "Garden" is at byte offset 17..23 in the document.
    let text = ":: Start\nHello [[Garden]]";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    // Build a semantic token group simulating what the token builder produces
    // for a [[ ]] link. The "Garden" text starts at byte 17 in the document.
    // Passage "Start" is at passage_offset 0, so the token's passage-relative
    // start = 17 (abs) - 0 (offset) = 17.
    let token_groups = vec![crate::plugin::PassageTokenGroup {
        passage_name: "Start".to_string(),
        passage_offset: 0,
        tokens: vec![crate::plugin::SemanticToken {
            start: 17, // passage-relative byte offset of "Garden"
            length: 6, // "Garden".len()
            token_type: crate::plugin::SemanticTokenType::Link,
            modifier: None,
        }],
    }];

    // Cursor at byte 20 (inside "Garden"), line 1, char 12
    let items = plugin.provide_completions(text, &workspace, &uri, 1, 12, None, &token_groups);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Ctrl+Space on Link token should return passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that Ctrl+Space (no trigger) on a PassageRef semantic token returns passage completions.
#[test]
fn test_ctrl_space_on_passageref_token_returns_passage_completions() {
    let plugin = SugarCubePlugin::new();
    // Text: ":: Start\n<<goto \"Forest\">>"
    // The word "Forest" is inside the macro arg quotes.
    let text = ":: Start\n<<goto \"Forest\">>";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest", "Garden"]);

    // Build a semantic token group simulating what the token builder produces
    // for a passage-ref macro arg. "Forest" starts at byte 17 in the document.
    let token_groups = vec![crate::plugin::PassageTokenGroup {
        passage_name: "Start".to_string(),
        passage_offset: 0,
        tokens: vec![crate::plugin::SemanticToken {
            start: 17, // passage-relative byte offset of "Forest"
            length: 6, // "Forest".len()
            token_type: crate::plugin::SemanticTokenType::PassageRef,
            modifier: None,
        }],
    }];

    // Cursor at byte 20 (inside "Forest"), line 1, char 12
    let items = plugin.provide_completions(text, &workspace, &uri, 1, 12, None, &token_groups);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Ctrl+Space on PassageRef token should return passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test that Ctrl+Space on a Link token uses the current name for filtering.
#[test]
fn test_ctrl_space_on_link_token_uses_name_as_filter() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [[Gar]]";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate", "Forest"]);

    let token_groups = vec![crate::plugin::PassageTokenGroup {
        passage_name: "Start".to_string(),
        passage_offset: 0,
        tokens: vec![crate::plugin::SemanticToken {
            start: 17, // passage-relative byte offset of "Gar"
            length: 3, // "Gar".len()
            token_type: crate::plugin::SemanticTokenType::Link,
            modifier: None,
        }],
    }];

    let items = plugin.provide_completions(text, &workspace, &uri, 1, 9, None, &token_groups);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Ctrl+Space on Link token 'Gar' should return passage completions, got {} passage items",
        passage_items.len()
    );
}

/// Test extract_partial_in_quote helper.
#[test]
fn test_extract_partial_in_quote() {
    assert_eq!(extract_partial_in_quote(r#"<<goto "Gar"#), "Gar");
    assert_eq!(extract_partial_in_quote(r#"<<goto ""#), "");
    assert_eq!(extract_partial_in_quote(r#"He said "hello""#), ""); // closed quote
    assert_eq!(extract_partial_in_quote(r#"<<link "Go" "Ga"#), "Ga"); // second open quote
}

// ── Context-aware passage name completion tests ─────────────────────

/// Test that Link context produces "Link target" detail.
#[test]
fn test_link_context_has_link_target_detail() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\nHello [[";
    let line = 1u32;
    let character = 8u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Expected passage completions after [["
    );
    // All passage items in Link context should have "Link target" detail
    for item in &passage_items {
        assert_eq!(
            item.detail.as_ref().unwrap(),
            &"Link target",
            "Link context passage item should have 'Link target' detail, got: {:?}",
            item.detail
        );
    }
}

/// Test that `<<goto "..."` context produces "Navigation target for <<goto>>" detail.
#[test]
fn test_goto_context_has_navigation_target_detail() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<goto \"";
    let line = 1u32;
    let character = 8u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Expected passage completions after <<goto \""
    );
    for item in &passage_items {
        assert_eq!(
            item.detail.as_ref().unwrap(),
            &"Navigation target for <<goto>>",
            "goto context passage item should have 'Navigation target for <<goto>>' detail, got: {:?}",
            item.detail
        );
    }
}

/// Test that `<<include "..."` context produces "Included passage for <<include>>" detail.
#[test]
fn test_include_context_has_included_passage_detail() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<include \"";
    let line = 1u32;
    let character = 11u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Sidebar", "Header"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Expected passage completions after <<include \""
    );
    for item in &passage_items {
        assert_eq!(
            item.detail.as_ref().unwrap(),
            &"Included passage for <<include>>",
            "include context passage item should have 'Included passage for <<include>>' detail, got: {:?}",
            item.detail
        );
    }
}

/// Test that `<<link "label" "..."` context produces "Link target for <<link>>" detail.
#[test]
fn test_link_macro_context_has_link_target_detail() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<link \"Go\" \"";
    let line = 1u32;
    let character = 14u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Expected passage completions after <<link \"Go\" \""
    );
    for item in &passage_items {
        assert_eq!(
            item.detail.as_ref().unwrap(),
            &"Link target for <<link>>",
            "link macro context passage item should have 'Link target for <<link>>' detail, got: {:?}",
            item.detail
        );
    }
}

/// Test that PassageRef semantic token (Ctrl+Space) produces macro-context detail.
#[test]
fn test_passageref_token_has_macro_context_detail() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<goto \"Forest\">>";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest", "Garden"]);

    // Build a PassageRef semantic token for "Forest"
    let token_groups = vec![crate::plugin::PassageTokenGroup {
        passage_name: "Start".to_string(),
        passage_offset: 0,
        tokens: vec![crate::plugin::SemanticToken {
            start: 17,
            length: 6, // "Forest".len()
            token_type: crate::plugin::SemanticTokenType::PassageRef,
            modifier: None,
        }],
    }];

    let items = plugin.provide_completions(text, &workspace, &uri, 1, 12, None, &token_groups);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(
        !passage_items.is_empty(),
        "Expected passage completions for PassageRef token"
    );
    // PassageRef inside <<goto>> should have "Navigation target for <<goto>>" detail,
    // NOT "Link target" (the old bug where PassageRef was conflated with Link).
    for item in &passage_items {
        let detail = item.detail.as_ref().unwrap();
        assert!(
            detail.contains("<<goto>>"),
            "PassageRef in <<goto>> should have macro-context detail, got: {:?}",
            detail
        );
        assert_ne!(
            detail, &"Link target",
            "PassageRef should NOT have Link target detail (was conflated with Link before fix)"
        );
    }
}

/// Test that passage completions in MacroArg context include macro_name in data.
#[test]
fn test_macro_arg_passage_completions_include_macro_context_in_data() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<goto \"";
    let line = 1u32;
    let character = 8u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('"'), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(!passage_items.is_empty());
    // All items should have macro_name in their data payload
    for item in &passage_items {
        let macro_name = item
            .data
            .as_ref()
            .and_then(|d| d.get("macro_name"))
            .and_then(|v| v.as_str());
        assert_eq!(
            macro_name,
            Some("goto"),
            "MacroArg passage completion should include macro_name='goto' in data, got: {:?}",
            macro_name
        );
    }
}

/// Test that Link context does NOT include macro_name in data.
#[test]
fn test_link_passage_completions_do_not_include_macro_context() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n[[";
    let line = 1u32;
    let character = 4u32;
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('['), &[]);
    let passage_items: Vec<_> = items
        .iter()
        .filter(|i| {
            i.data
                .as_ref()
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str())
                == Some("passage")
        })
        .collect();
    assert!(!passage_items.is_empty());
    // Link context should NOT have macro_name in data
    for item in &passage_items {
        let macro_name = item
            .data
            .as_ref()
            .and_then(|d| d.get("macro_name"))
            .and_then(|v| v.as_str());
        assert_eq!(
            macro_name, None,
            "Link passage completion should NOT include macro_name in data, got: {:?}",
            macro_name
        );
    }
}

/// Test that resolve_completion_context returns MacroPassageRef for PassageRef tokens.
#[test]
fn test_resolve_context_passageref_returns_macro_passage_ref() {
    let plugin = SugarCubePlugin::new();
    let text = ":: Start\n<<goto \"Forest\">>";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

    // Build a PassageRef semantic token
    let token_groups = vec![crate::plugin::PassageTokenGroup {
        passage_name: "Start".to_string(),
        passage_offset: 0,
        tokens: vec![crate::plugin::SemanticToken {
            start: 17,
            length: 6,
            token_type: crate::plugin::SemanticTokenType::PassageRef,
            modifier: None,
        }],
    }];

    let ctx = plugin.resolve_completion_context(text, &workspace, &uri, 1, 12, None, &token_groups);
    // Should be MacroPassageRef, NOT Link
    match ctx {
        crate::types::CompletionContext::MacroPassageRef {
            target, macro_name, ..
        } => {
            assert_eq!(
                target, "Forest",
                "PassageRef should resolve target to 'Forest'"
            );
            assert_eq!(
                macro_name, "goto",
                "PassageRef should resolve macro_name to 'goto'"
            );
        }
        other => panic!(
            "PassageRef token should return MacroPassageRef context, got: {:?}",
            other
        ),
    }
}

// ── Variable dot-completion deep nesting tests ────────────────────

/// Test that `find_variable_path_before_dot` correctly resolves
/// deep paths like `$item.work` when cursor is after the dot.
#[test]
fn test_find_variable_path_before_dot_deep() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Populate the arena tree with deep variable structure
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        10..40,
        "work.pen.color",
        "",
        &[],
        None,
        &[],
    );

    // Line text: "<<set $item.work.>>"
    // Positions:  0123456789012345678
    //             <<set $item.work.>>
    // Cursor after second dot = character 17
    let text = ":: Init\n<<set $item.work.>>";
    let line = 1u32;
    let character = 17u32;
    let byte_offset = line_char_to_byte_offset(text, line, character);
    let line_text = text.lines().nth(line as usize).unwrap_or("");
    let byte_pos = char_to_byte_offset(line_text, character as usize);
    let before_cursor = &line_text[..byte_pos.min(line_text.len())];

    // before_cursor should be "<<set $item.work."
    assert!(
        before_cursor.contains("$item.work."),
        "before_cursor should contain '$item.work.', got: '{}'",
        before_cursor
    );

    let result = find_variable_path_before_dot(
        &workspace,
        &uri,
        text,
        byte_offset,
        before_cursor,
        plugin.registry(),
    );

    assert!(
        result.is_some(),
        "find_variable_path_before_dot should resolve $item.work, got None"
    );
    let path = result.unwrap();
    assert_eq!(
        path, "$item.work",
        "Expected path '$item.work', got '{}'",
        path
    );
}

/// Test that `build_variable_dot_completions` returns children at
/// the correct depth for deeply nested variables.
#[test]
fn test_dot_completions_deep_nesting() {
    let mut plugin = SugarCubePlugin::new();

    // Populate the arena tree: $item -> work -> pen -> color
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        10..40,
        "work.pen.color",
        "",
        &[],
        None,
        &[],
    );

    // Level 1: dot completions for $item → should show "work"
    let items_level1 = plugin.build_variable_dot_completions("$item", 0, 7, "", None);
    assert!(
        !items_level1.is_empty(),
        "Level 1 ($item.) should have completions"
    );
    let labels_l1: Vec<&str> = items_level1.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels_l1.contains(&"work"),
        "Level 1 should contain 'work', got: {:?}",
        labels_l1
    );

    // Level 2: dot completions for $item.work → should show "pen"
    let items_level2 = plugin.build_variable_dot_completions("$item.work", 0, 13, "", None);
    assert!(
        !items_level2.is_empty(),
        "Level 2 ($item.work.) should have completions"
    );
    let labels_l2: Vec<&str> = items_level2.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels_l2.contains(&"pen"),
        "Level 2 should contain 'pen', got: {:?}",
        labels_l2
    );

    // Level 3: dot completions for $item.work.pen → should show "color"
    let items_level3 = plugin.build_variable_dot_completions("$item.work.pen", 0, 17, "", None);
    assert!(
        !items_level3.is_empty(),
        "Level 3 ($item.work.pen.) should have completions"
    );
    let labels_l3: Vec<&str> = items_level3.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels_l3.contains(&"color"),
        "Level 3 should contain 'color', got: {:?}",
        labels_l3
    );
}

/// Test the full `provide_completions` pipeline for dot-trigger
/// deep nesting.
#[test]
fn test_provide_completions_dot_trigger_deep() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Populate the arena tree: $item -> work -> pen -> color
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        10..40,
        "work.pen.color",
        "",
        &[],
        None,
        &[],
    );

    // Test: $item. (dot trigger) → should show "work"
    // Line: "<<set $item.>>" — cursor after dot = character 12
    let text_l1 = ":: Init\n<<set $item.>>";
    let items_l1 = plugin.provide_completions(
        text_l1,
        &workspace,
        &uri,
        1,
        12, // cursor after "$item."
        Some('.'),
        &[],
    );
    let labels_l1: Vec<&str> = items_l1.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels_l1.contains(&"work"),
        "Dot trigger after $item. should show 'work', got: {:?}",
        labels_l1
    );

    // Test: $item.work. (dot trigger) → should show "pen", NOT "work"
    // Line: "<<set $item.work.>>" — cursor after second dot = character 17
    let text_l2 = ":: Init\n<<set $item.work.>>";
    let items_l2 = plugin.provide_completions(
        text_l2,
        &workspace,
        &uri,
        1,
        17, // cursor after "$item.work."
        Some('.'),
        &[],
    );
    let labels_l2: Vec<&str> = items_l2.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels_l2.contains(&"pen"),
        "Dot trigger after $item.work. should show 'pen', got: {:?}",
        labels_l2
    );
    assert!(
        !labels_l2.contains(&"work"),
        "Dot trigger after $item.work. should NOT show 'work' (that's a sibling, not a child), got: {:?}",
        labels_l2
    );
}

/// Test the no-trigger (Ctrl+Space) path for deep dot continuation.
/// This simulates the scenario where the user types $item.work. and
/// then hits Ctrl+Space instead of relying on the dot trigger.
#[test]
fn test_no_trigger_deep_dot_continuation() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Populate the arena tree: $item -> work -> pen -> color
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        10..40,
        "work.pen.color",
        "",
        &[],
        None,
        &[],
    );

    // Test: $item.work. with NO trigger → should show "pen"
    // Line: "<<set $item.work.>>" — cursor after second dot = character 17
    let text = ":: Init\n<<set $item.work.>>";
    let items = plugin.provide_completions(
        text,
        &workspace,
        &uri,
        1,
        17,   // cursor after "$item.work."
        None, // NO trigger (Ctrl+Space)
        &[],
    );
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"pen"),
        "No-trigger after $item.work. should show 'pen', got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"work"),
        "No-trigger after $item.work. should NOT show 'work' (sibling), got: {:?}",
        labels
    );
}

/// Test that when a VarOp span covers the cursor, the dot continuation
/// still takes priority for deep nesting (not root variable completions).
#[test]
fn test_dot_continuation_beats_var_span() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let mut workspace = knot_core::Workspace::new(uri.clone());

    // Populate the arena tree: $item -> work -> pen -> color
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        10..40,
        "work.pen.color",
        "",
        &[],
        None,
        &[],
    );

    // Add a document with a VarOp for $item.work
    // This simulates the parser having seen $item.work in the passage
    use knot_core::passage::{StoryFormat, VarKind, VarOp};
    use knot_core::{Document, Passage};
    let mut doc = Document::new(uri.clone(), StoryFormat::SugarCube);
    let passage_offset = 8; // after ":: Init\n"
    let var_span_start = passage_offset + 7; // position of "$" in "<<set $item.work.>>"
    doc.passages.push(Passage {
        name: "Init".to_string(),
        tags: Vec::new(),
        span: passage_offset..(passage_offset + 30),
        header_name_span: None,
        body: Vec::new(),
        links: Vec::new(),
        vars: vec![VarOp {
            name: "$item".to_string(),
            kind: VarKind::Init,
            span: (var_span_start)..(var_span_start + 10), // covers "$item.work"
            is_temporary: false,
        }],
        macro_arg_refs: Vec::new(),
        macro_invocations: Vec::new(),
        is_special: false,
        special_def: None,
        position: None,
        group: None,
        color: None,
        size: None,
        line: 0,
        manual_reachable: None,
        passage_offset,
        zones: knot_core::zoning::ZoneMap::default(),
    });
    workspace.insert_document(doc);

    // Test: no-trigger at position after "$item.work."
    // The VarOp span covers "$item.work" but the cursor is after the trailing dot
    let text = ":: Init\n<<set $item.work.>>";
    let items = plugin.provide_completions(
        text,
        &workspace,
        &uri,
        1,
        17,   // cursor after "$item.work."
        None, // NO trigger
        &[],
    );
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    // Should show dot completions (pen), NOT root variable list ($item)
    assert!(
        labels.contains(&"pen"),
        "No-trigger after $item.work. with VarOp present should show 'pen', got: {:?}",
        labels
    );
}

/// Test that `build_path_for_segment` on the variable tree correctly
/// resolves deep paths (e.g., "$item.work.pen") when segment_spans
/// are provided and the starting node is the root variable.
///
/// This is a regression test for the bug where `build_path_for_segment`
/// walked UP from the root variable node, hit persistent_root
/// immediately, and only returned a single-component path regardless
/// of `seg_idx`.
#[test]
fn test_build_path_for_segment_deep_resolution() {
    use crate::sugarcube::registries::variable_tree::VarAccessKind;
    use crate::sugarcube::registries::variable_tree::VariableTree;

    let mut tree = VariableTree::new();

    // Record a deep variable: $item.work.pen.color
    // With segment_spans that represent each component's byte range.
    tree.record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        7..27, // full span: "$item.work.pen.color"
        "work.pen.color",
        "",
        &[
            7..12,  // segment 0: "$item"
            12..17, // segment 1: ".work"
            17..21, // segment 2: ".pen"
            21..27, // segment 3: ".color"
        ],
        None,
        &[],
    );

    // Test: path_at_offset should resolve correctly at each depth.
    // When cursor is within segment 1 (work), should return "$item.work"
    let path_seg1 = tree.path_at_offset("file:///test.twee", "Init", 14);
    assert_eq!(
        path_seg1.as_deref(),
        Some("$item.work"),
        "path_at_offset at segment 1 (work) should return '$item.work', got {:?}",
        path_seg1
    );

    // When cursor is within segment 2 (pen), should return "$item.work.pen"
    let path_seg2 = tree.path_at_offset("file:///test.twee", "Init", 19);
    assert_eq!(
        path_seg2.as_deref(),
        Some("$item.work.pen"),
        "path_at_offset at segment 2 (pen) should return '$item.work.pen', got {:?}",
        path_seg2
    );

    // When cursor is within segment 3 (color), should return "$item.work.pen.color"
    let path_seg3 = tree.path_at_offset("file:///test.twee", "Init", 24);
    assert_eq!(
        path_seg3.as_deref(),
        Some("$item.work.pen.color"),
        "path_at_offset at segment 3 (color) should return '$item.work.pen.color', got {:?}",
        path_seg3
    );

    // When cursor is within segment 0 ($item), should return "$item"
    let path_seg0 = tree.path_at_offset("file:///test.twee", "Init", 9);
    assert_eq!(
        path_seg0.as_deref(),
        Some("$item"),
        "path_at_offset at segment 0 ($item) should return '$item', got {:?}",
        path_seg0
    );
}

/// Test the dot-trigger end-to-end scenario that the user reported:
/// typing `$item.work.` should show properties of `work` (like `pen`),
/// NOT the root-level children of `$item` (which would be just `work`).
///
/// This simulates the real LSP scenario with segment_spans populated.
#[test]
fn test_dot_trigger_shows_deep_children_not_root_children() {
    let mut plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    // Populate the arena tree with deep variable structure
    // $item -> work -> pen -> color
    // $item -> name (scalar)
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        7..27,
        "work.pen.color",
        "",
        &[
            7..12,  // $item
            12..17, // .work
            17..21, // .pen
            21..27, // .color
        ],
        None,
        &[],
    );
    plugin.registry_mut().variables_mut().record_var(
        "$item",
        false,
        VarAccessKind::Write,
        "Init",
        "file:///test.twee",
        35..40,
        "name",
        "",
        &[
            35..40, // $item
            40..44, // .name
        ],
        None,
        &[],
    );

    // Test: $item.work. (dot trigger) → should show "pen", NOT "work" or "name"
    let text = ":: Init\n<<set $item.work.>>";
    let items = plugin.provide_completions(
        text,
        &workspace,
        &uri,
        1,
        17, // cursor after "$item.work."
        Some('.'),
        &[],
    );
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    // "pen" is a child of $item.work — should be present
    assert!(
        labels.contains(&"pen"),
        "Dot trigger after $item.work. should show 'pen' (child of work), got: {:?}",
        labels
    );

    // "work" is NOT a child of $item.work — it IS $item.work.
    // It should NOT appear as a suggestion.
    assert!(
        !labels.contains(&"work"),
        "Dot trigger after $item.work. should NOT show 'work' (that's the path itself, not a child), got: {:?}",
        labels
    );

    // "name" is a sibling of "work" under $item, not a child of $item.work.
    // It should NOT appear as a suggestion.
    assert!(
        !labels.contains(&"name"),
        "Dot trigger after $item.work. should NOT show 'name' (sibling under $item, not child of work), got: {:?}",
        labels
    );
}

// ── Custom macro completion snippet tests ───────────────────────────
//
// These tests verify that custom macro completions don't insert a
// literal "${1:args}" placeholder when arg_count is unknown, and that
// argless macros get `<<name>>` (no trailing space).

/// Helper: register a widget in the plugin's registry.
fn register_test_widget(
    plugin: &mut SugarCubePlugin,
    name: &str,
    arg_count: Option<usize>,
    is_container: bool,
) {
    let body = if is_container {
        crate::types::BodyRequirement::Required
    } else {
        crate::types::BodyRequirement::Never
    };
    plugin.registry_mut().custom_macros_mut().register_widget(
        name,
        "Widgets",
        "file:///test.twee",
        0,
        arg_count,
        body,
    );
}

/// Helper: register a Macro.add() custom macro (not a widget).
fn register_test_macro_add(plugin: &mut SugarCubePlugin, name: &str, arg_count: Option<usize>) {
    plugin
        .registry_mut()
        .custom_macros_mut()
        .register_macro_add(
            name,
            "Scripts",
            "file:///test.twee",
            0,
            arg_count,
            crate::types::BodyRequirement::Never,
        );
}

/// Widget with known arg_count=2: inline completion should insert
/// `mywidget ${1:arg1} ${2:arg2}>>` (space-separated tabstops, no literal "args").
#[test]
fn custom_widget_completion_with_known_args() {
    let mut plugin = SugarCubePlugin::new();
    register_test_widget(&mut plugin, "mywidget", Some(2), false);
    let text = ":: Start\n<<";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.build_macro_completions(
        &workspace,
        &uri,
        "",
        CursorPos {
            line: 1,
            character: 4,
        },
        text,
        4,
    );
    let inline = items
        .iter()
        .find(|i| i.insert_text.as_deref() == Some("mywidget ${1:arg1} ${2:arg2}>>"))
        .expect("should have inline form with arg1/arg2 tabstops");
    assert!(
        !inline.insert_text.as_ref().unwrap().contains("args"),
        "inline snippet should NOT contain literal 'args': got {}",
        inline.insert_text.as_ref().unwrap()
    );
}

/// Widget with unknown arg_count (None): inline completion should insert
/// `mywidget>>` (no args, no trailing space, no literal "args" placeholder).
#[test]
fn custom_widget_completion_with_unknown_args() {
    let mut plugin = SugarCubePlugin::new();
    register_test_widget(&mut plugin, "mywidget", None, false);
    let text = ":: Start\n<<";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.build_macro_completions(
        &workspace,
        &uri,
        "",
        CursorPos {
            line: 1,
            character: 4,
        },
        text,
        4,
    );
    // Find the inline form (label is `<<mywidget>>` without `...<</mywidget>>`)
    let inline = items
        .iter()
        .find(|i| i.label == "<<mywidget>>" && !i.label.contains("</"))
        .expect("should have inline form for argless widget");
    let snippet = inline.insert_text.as_ref().unwrap();
    assert_eq!(
        snippet, "mywidget>>",
        "argless widget inline snippet should be 'mywidget>>' (no trailing space, no 'args'): got '{}'",
        snippet
    );
    assert!(
        !snippet.contains("args"),
        "snippet should NOT contain literal 'args': got '{}'",
        snippet
    );
}

/// Widget with arg_count=0: same as unknown — `mywidget>>` with no args.
#[test]
fn custom_widget_completion_with_zero_args() {
    let mut plugin = SugarCubePlugin::new();
    register_test_widget(&mut plugin, "nowidget", Some(0), false);
    let text = ":: Start\n<<";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.build_macro_completions(
        &workspace,
        &uri,
        "",
        CursorPos {
            line: 1,
            character: 4,
        },
        text,
        4,
    );
    let inline = items
        .iter()
        .find(|i| i.label == "<<nowidget>>" && !i.label.contains("</"))
        .expect("should have inline form for zero-arg widget");
    let snippet = inline.insert_text.as_ref().unwrap();
    assert_eq!(
        snippet, "nowidget>>",
        "zero-arg widget inline snippet should be 'nowidget>>': got '{}'",
        snippet
    );
}

/// Macro.add() custom macro with unknown arg_count: should insert
/// `mymacro>>` (no literal "args" placeholder).
#[test]
fn macro_add_completion_with_unknown_args() {
    let mut plugin = SugarCubePlugin::new();
    register_test_macro_add(&mut plugin, "mymacro", None);
    let text = ":: Start\n<<";
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.build_macro_completions(
        &workspace,
        &uri,
        "",
        CursorPos {
            line: 1,
            character: 4,
        },
        text,
        4,
    );
    let item = items
        .iter()
        .find(|i| i.label == "<<mymacro>>")
        .expect("should have completion for Macro.add() custom macro");
    let snippet = item.insert_text.as_ref().unwrap();
    assert_eq!(
        snippet, "mymacro>>",
        "Macro.add() snippet should be 'mymacro>>' (no 'args'): got '{}'",
        snippet
    );
    assert!(
        !snippet.contains("args"),
        "snippet should NOT contain literal 'args': got '{}'",
        snippet
    );
}

// ── `?` trigger template completion tests ────────────────────────────
//
// These tests cover the `?` trigger handling in `provide_completions`,
// including the suppression rules that prevent template completions from
// bleeding into JS expression contexts (ternary, optional chaining).

/// Helper: register a single template named `name` in the plugin's
/// template registry, so `build_template_completions` has something to
/// return.
fn register_template(plugin: &mut SugarCubePlugin, name: &str) {
    use crate::sugarcube::registries::template_registry::TemplateKind;
    plugin.registry_mut().templates_mut().register_template(
        name,
        TemplateKind::Function,
        "StoryInit",
        "file:///test.twee",
        0,
    );
}

/// `?` trigger in prose (cursor right after `?`) should fire template
/// completions.
#[test]
fn template_completions_fire_on_q_trigger_in_prose() {
    let mut plugin = SugarCubePlugin::new();
    register_template(&mut plugin, "greeting");
    let text = ":: Start\nHello ?";
    let line = 1u32;
    let character = 7u32; // cursor right after `?`
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('?'), &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"?greeting"),
        "`?` trigger in prose should show ?greeting, got: {:?}",
        labels
    );
}

/// `?` trigger with a partial name (`?gre`) should filter templates.
#[test]
fn template_completions_filter_by_partial() {
    let mut plugin = SugarCubePlugin::new();
    register_template(&mut plugin, "greeting");
    register_template(&mut plugin, "farewell");
    let text = ":: Start\nHello ?gre";
    let line = 1u32;
    let character = 10u32; // cursor after `?gre`
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('?'), &[]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"?greeting"),
        "`?gre` should match ?greeting, got: {:?}",
        labels
    );
    assert!(
        !labels.contains(&"?farewell"),
        "`?gre` should NOT match ?farewell, got: {:?}",
        labels
    );
}

/// `?` trigger preceded by a word char (e.g., `obj?`) should NOT fire
/// template completions — this is JS optional chaining context.
#[test]
fn template_completions_suppressed_when_q_after_word_char() {
    let mut plugin = SugarCubePlugin::new();
    register_template(&mut plugin, "greeting");
    // `obj?` — the `?` is preceded by `j` (a word char). Even though
    // the `?` trigger fires, the suppression rule should return empty.
    let text = ":: Start\nHello obj?";
    let line = 1u32;
    let character = 11u32; // cursor right after `?` (which is after `obj`)
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('?'), &[]);
    assert!(
        items.is_empty(),
        "`?` trigger after word char `obj?` should suppress template completions (JS optional chaining), got {} items: {:?}",
        items.len(),
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

/// `?` trigger followed by a space (e.g., `cond ? value`) should NOT
/// fire template completions — this is JS ternary context.
#[test]
fn template_completions_suppressed_when_q_followed_by_space() {
    let mut plugin = SugarCubePlugin::new();
    register_template(&mut plugin, "greeting");
    // `cond ? ` — the `?` is followed by a space. JS ternary context.
    // Note: the trigger fires when `?` is typed, so before_cursor is
    // `cond ? ` (with the trailing space). Actually, the trigger fires
    // immediately when `?` is typed, so before_cursor is `cond ?` and
    // the space hasn't been typed yet. To simulate the ternary case,
    // we use before_cursor = `cond ?` followed by `"yes"` — but the
    // trigger only sees up to the `?`. The actual test: when the user
    // types a space after `?`, the trigger fires again (no, `?` is
    // the trigger, space isn't). So we need to test the case where
    // `?` is followed by a non-identifier char in before_cursor.
    //
    // The realistic scenario: the user types `?` after `cond ` and
    // then types a `"`. The `"` doesn't trigger anything, but if the
    // user then backspaces and retypes `?`, the trigger fires with
    // before_cursor = `cond ?` (just `?`, no following char). So the
    // suppression based on "char after `?`" only fires when there IS
    // a char after `?` in before_cursor.
    //
    // To test this, we simulate: user typed `?"` (quote after `?`),
    // and the `?` trigger fired (with before_cursor = `?"`).
    let text = ":: Start\nHello ?\"";
    let line = 1u32;
    let character = 8u32; // cursor right after `?"`
    let uri = Url::parse("file:///test.twee").unwrap();
    let workspace = Workspace::new(uri.clone());

    let items = plugin.provide_completions(text, &workspace, &uri, line, character, Some('?'), &[]);
    assert!(
        items.is_empty(),
        "`?` trigger followed by non-identifier char `?\"` should suppress template completions (likely JS ternary), got {} items: {:?}",
        items.len(),
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}
