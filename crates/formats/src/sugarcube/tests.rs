//! Integration tests for the SugarCube format plugin.

use super::*;
use knot_core::passage::VarKind;

#[test]
fn parse_simple_passage() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\nYou are in a room. [[Go north->Forest]]\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 1);
    assert_eq!(result.passages[0].name, "Start");
    assert_eq!(result.passages[0].links.len(), 1);
    assert_eq!(result.passages[0].links[0].target, "Forest");
    assert_eq!(
        result.passages[0].links[0].display_text,
        Some("Go north".into())
    );
    assert!(result.is_complete);
}

#[test]
fn parse_multiple_passages() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 2);
    assert_eq!(result.passages[0].name, "Start");
    assert_eq!(result.passages[1].name, "Forest");
}

#[test]
fn parse_passage_with_tags() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 1);
    assert_eq!(result.passages[0].name, "Dark Room");
    assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
}

#[test]
fn parse_variable_operations() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<set $gold to 10>>You have $gold coins.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 1);
    let vars = &result.passages[0].vars;
    assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init));
    assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
}

#[test]
fn parse_pipe_link() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n[[Go to forest|Forest]]\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages[0].links.len(), 1);
    assert_eq!(result.passages[0].links[0].target, "Forest");
    assert_eq!(
        result.passages[0].links[0].display_text,
        Some("Go to forest".into())
    );
}

#[test]
fn detect_special_passages() {
    let plugin = SugarCubePlugin::new();
    assert!(plugin.is_special_passage("StoryInit"));
    assert!(plugin.is_special_passage("StoryCaption"));
    assert!(!plugin.is_special_passage("MyRoom"));
}

#[test]
fn unclosed_macro_diagnostic() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<set $x to 5\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"));
}

#[test]
fn empty_input_is_ok() {
    let plugin = SugarCubePlugin::new();
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");

    assert!(result.passages.is_empty());
    assert!(result.is_complete);
}

#[test]
fn incremental_reparse() {
    let plugin = SugarCubePlugin::new();
    let passage = plugin.parse_passage("Start", &[], "You have $gold coins.\n");

    assert!(passage.is_some());
    let p = passage.unwrap();
    assert_eq!(p.name, "Start");
    assert!(p.vars.iter().any(|v| v.name == "$gold"));
}

#[test]
fn parse_temporary_variable() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<set _temp to 5>>You see _temp items.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 1);
    let vars = &result.passages[0].vars;

    // Should detect _temp as a temporary init
    assert!(
        vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Init && v.is_temporary),
        "Should detect _temp as a temporary init"
    );

    // Should detect _temp as a temporary read
    assert!(
        vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Read && v.is_temporary),
        "Should detect _temp as a temporary read"
    );
}

#[test]
fn persistent_and_temp_vars_separate() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<set $gold to 10>><<set _temp to 5>>You have $gold and _temp.\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let vars = &result.passages[0].vars;

    // $gold should be persistent
    let gold_inits: Vec<_> = vars
        .iter()
        .filter(|v| v.name == "$gold" && v.kind == VarKind::Init)
        .collect();
    assert_eq!(gold_inits.len(), 1);
    assert!(!gold_inits[0].is_temporary);

    // _temp should be temporary
    let temp_inits: Vec<_> = vars
        .iter()
        .filter(|v| v.name == "_temp" && v.kind == VarKind::Init)
        .collect();
    assert_eq!(temp_inits.len(), 1);
    assert!(temp_inits[0].is_temporary);
}

#[test]
fn structural_validation_else_without_if() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<else>>Some text\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
        "Should detect <<else>> outside <<if>>"
    );
}

#[test]
fn structural_validation_break_without_for() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<break>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
        "Should detect <<break>> outside <<for>>"
    );
}

#[test]
fn structural_validation_else_inside_if_ok() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<if $x>><<else>>OK<</if>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
        "<<else>> inside <<if>> should not trigger structural validation"
    );
}

#[test]
fn gt_in_condition_else_not_flagged() {
    // The critical bug: <<if _parts.length > 0>> should NOT cause
    // <<else>> to be flagged as a structural error. The `>` in the
    // condition must not break macro delimiter parsing.
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<if _parts.length > 0>>\n  <<= _parts[0] >>\n  <<if _parts.length > 1>> +<<= _parts.length - 1 >><</if>>\n<<else>>\n  &mdash;\n<</if>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
        "<<else>> inside <<if _parts.length > 0>> should NOT be flagged — the > in the condition should not break delimiter parsing"
    );

    // Also verify no unclosed-macro diagnostics from the > in condition
    assert!(
        !result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"),
        "<<if _parts.length > 0>> should not produce unclosed-macro warnings"
    );
}

#[test]
fn deprecated_macro_warning() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<click \"label\" \"target\">>Click<</click>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        result.diagnostics.iter().any(|d| d.code == "sc-deprecated-macro"),
        "Should detect deprecated <<click>> macro"
    );
}

#[test]
fn unknown_macro_hint() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<foobar>>test<</foobar>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert!(
        result.diagnostics.iter().any(|d| d.code == "sc-unknown-macro"),
        "Should detect unknown <<foobar>> macro"
    );
}

#[test]
fn implicit_passage_ref_data_passage() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<a data-passage=\"Forest\">Go</a>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect data-passage implicit reference"
    );
}

#[test]
fn implicit_passage_ref_engine_play() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>Engine.play(\"Forest\");<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect Engine.play() implicit reference"
    );
}

#[test]
fn implicit_passage_ref_story_get() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>var p = Story.get(\"Forest\");<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect Story.get() implicit reference"
    );
}

#[test]
fn macro_passage_ref_goto() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<goto \"Forest\">>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect <<goto>> macro passage reference"
    );
}

#[test]
fn macro_passage_ref_link() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<link \"Click\" \"Forest\">>Go<</link>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect <<link>> macro passage reference"
    );
}

#[test]
fn macro_passage_ref_include() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<include \"Sidebar\">>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Sidebar"),
        "Should detect <<include>> macro passage reference"
    );
}

#[test]
fn special_passage_defs_complete() {
    // SugarCube's own special passage definitions (StoryFormat layer only)
    let sc_defs = special_passages::name_matched_special_passages();
    let sc_names: Vec<&str> = sc_defs.iter().map(|d| d.name.as_str()).collect();

    // StoryFormat-layer passages owned by SugarCube
    assert!(sc_names.contains(&"StoryInit"));
    assert!(sc_names.contains(&"StoryCaption"));
    assert!(sc_names.contains(&"StoryMenu"));
    assert!(sc_names.contains(&"StoryBanner"));
    assert!(sc_names.contains(&"StorySubtitle"));
    assert!(sc_names.contains(&"StoryAuthor"));
    assert!(sc_names.contains(&"StoryDisplayTitle"));
    assert!(sc_names.contains(&"StoryShare"));
    assert!(sc_names.contains(&"StoryInterface"));
    assert!(sc_names.contains(&"PassageReady"));
    assert!(sc_names.contains(&"PassageDone"));
    assert!(sc_names.contains(&"PassageHeader"));
    assert!(sc_names.contains(&"PassageFooter"));

    // TwineCore passages should NOT be in SugarCube's own list
    assert!(!sc_names.contains(&"StoryTitle"), "StoryTitle is TwineCore, not SugarCube");
    assert!(!sc_names.contains(&"StoryData"), "StoryData is TwineCore, not SugarCube");

    // But they should be available through all_special_passages()
    let plugin = SugarCubePlugin::new();
    let all_defs = plugin.all_special_passages();
    let all_names: Vec<&str> = all_defs.iter().map(|d| d.name.as_str()).collect();

    assert!(all_names.contains(&"StoryTitle"), "StoryTitle should be in merged registry");
    assert!(all_names.contains(&"StoryData"), "StoryData should be in merged registry");
    // Core tags are tag-matched: "script", "stylesheet", "style"
    // (not "Story JavaScript" / "Story Stylesheet" as passage names)
    assert!(all_names.contains(&"script"), "script tag should be in merged registry");
    assert!(all_names.contains(&"stylesheet"), "stylesheet tag should be in merged registry");
    assert!(all_names.contains(&"Start"), "Start should be in merged registry");
    assert!(all_names.contains(&"StoryInit"), "StoryInit should be in merged registry");
}

// ── Comment filtering tests ───────────────────────────────────────

#[test]
fn twine_comment_skips_links() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n/% [[HiddenLink]] %/ visible [[RealLink]]\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        !links.iter().any(|l| l.target == "HiddenLink"),
        "Links inside /% %/ comments should be filtered out"
    );
    assert!(
        links.iter().any(|l| l.target == "RealLink"),
        "Links outside /% %/ comments should be detected"
    );
}

#[test]
fn html_comment_skips_links() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<!-- [[HiddenLink]] --> visible [[RealLink]]\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        !links.iter().any(|l| l.target == "HiddenLink"),
        "Links inside <!-- --> comments should be filtered out"
    );
    assert!(
        links.iter().any(|l| l.target == "RealLink"),
        "Links outside <!-- --> comments should be detected"
    );
}

#[test]
fn line_comment_skips_refs_in_script_block() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>\n// Engine.play(\"Hidden\");\nEngine.play(\"Visible\");\n<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        !links.iter().any(|l| l.target == "Hidden"),
        "Engine.play inside // line comment should be filtered out"
    );
    assert!(
        links.iter().any(|l| l.target == "Visible"),
        "Engine.play outside line comment should be detected"
    );
}

#[test]
fn line_comment_in_script_passage() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Story JavaScript [script]\n// Engine.play(\"Hidden\");\nEngine.play(\"Visible\");\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        !links.iter().any(|l| l.target == "Hidden"),
        "Engine.play inside // line comment in script passage should be filtered out"
    );
    assert!(
        links.iter().any(|l| l.target == "Visible"),
        "Engine.play outside line comment in script passage should be detected"
    );
}

#[test]
fn twine_comment_skips_vars() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n/% <<set $hidden to 5>> %/ <<set $visible to 10>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let vars = &result.passages[0].vars;
    assert!(
        !vars.iter().any(|v| v.name == "$hidden"),
        "Variables inside /% %/ comments should be filtered out"
    );
    assert!(
        vars.iter().any(|v| v.name == "$visible"),
        "Variables outside /% %/ comments should be detected"
    );
}

// ── New implicit passage reference tests ──────────────────────────

#[test]
fn implicit_passage_ref_ui_goto() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>UI.goto(\"Forest\");<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect UI.goto() implicit reference"
    );
}

#[test]
fn implicit_passage_ref_ui_include() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>UI.include(\"Sidebar\");<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Sidebar"),
        "Should detect UI.include() implicit reference"
    );
}

#[test]
fn implicit_passage_ref_story_has() {
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<script>>Story.has(\"Forest\");<</script>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    let links = &result.passages[0].links;
    assert!(
        links.iter().any(|l| l.target == "Forest"),
        "Should detect Story.has() implicit reference"
    );
}

// ── Macro block ordering and > in condition tests ────────────────

#[test]
fn extract_macros_sorted_by_position() {
    // Verify that extract_macros returns blocks in source order,
    // not open-then-close order. This is critical for build_body_blocks()
    // which assumes sorted input.
    use super::blocks;
    let body = "<<if $x>>yes<</if>>";
    let macros = blocks::extract_macros(body, 0);

    // Should be: open "if", close "/if" — in source order
    assert_eq!(macros.len(), 2, "Should find 2 macros");
    match &macros[0] {
        knot_core::passage::Block::Macro { name, .. } => {
            assert_eq!(name, "if", "First macro should be open 'if'");
        }
        _ => panic!("Expected Macro block"),
    }
    match &macros[1] {
        knot_core::passage::Block::Macro { name, .. } => {
            assert_eq!(name, "/if", "Second macro should be close '/if'");
        }
        _ => panic!("Expected Macro block"),
    }
}

#[test]
fn extract_macros_nested_sorted() {
    // Nested macros with close tags between open tags — must be sorted
    // Source order: <<if>>, <<if>>, <</if>>, <<else>>, <</if>>
    use super::blocks;
    let body = "<<if $a>><<if $b>>yes<</if>><<else>>no<</if>>";
    let macros = blocks::extract_macros(body, 0);

    assert_eq!(macros.len(), 5, "Should find 5 macros");

    // Verify source order: open if, open if, close if, open else, close if
    let names: Vec<&str> = macros.iter().filter_map(|m| match m {
        knot_core::passage::Block::Macro { name, .. } => Some(name.as_str()),
        _ => None,
    }).collect();
    assert_eq!(names, &["if", "if", "/if", "else", "/if"],
        "Macros must be in source order, not open-then-close order");

    // Verify spans are monotonically increasing
    let spans: Vec<usize> = macros.iter().filter_map(|m| match m {
        knot_core::passage::Block::Macro { span, .. } => Some(span.start),
        _ => None,
    }).collect();
    for i in 1..spans.len() {
        assert!(spans[i] > spans[i - 1],
            "Macro spans must be in increasing order: {:?} at index {}", spans, i);
    }
}

#[test]
fn gt_in_condition_exhaustive() {
    // Exhaustive test for > in macro conditions — multiple patterns
    let plugin = SugarCubePlugin::new();

    let test_cases = vec![
        // (description, source)
        ("simple gt", ":: Start\n<<if $x > 0>>yes<</if>>\n"),
        ("gt with else", ":: Start\n<<if $x > 0>>yes<<else>>no<</if>>\n"),
        ("nested gt", ":: Start\n<<if $a > 1>>\n  <<if $b > 2>>inner<</if>>\n<<else>>\n  outer\n<</if>>\n"),
        ("gt with print shorthand", ":: Start\n<<if _parts.length > 0>>\n  <<= _parts[0] >>\n  <<if _parts.length > 1>> +<<= _parts.length - 1 >><</if>>\n<<else>>\n  &mdash;\n<</if>>\n"),
        ("multiple gt conditions", ":: Start\n<<if $x > 0>><<elseif $x > -1>>zero<<else>>neg<</if>>\n"),
        ("gte operator", ":: Start\n<<if $x >= 0>>yes<</if>>\n"),
    ];

    for (desc, src) in test_cases {
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "[{}] <<else>>/<<elseif>> should NOT be flagged — > in condition should not break delimiter parsing. Diagnostics: {:?}",
            desc,
            result.diagnostics.iter().map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
        );

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"),
            "[{}] > in condition should not produce unclosed-macro warnings. Diagnostics: {:?}",
            desc,
            result.diagnostics.iter().map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
        );
    }
}

#[test]
fn body_blocks_correct_order_with_close_tags() {
    // Verify that body blocks are in correct source order even when
    // close tags appear between open tags. This tests the sorting fix
    // in extract_macros().
    let plugin = SugarCubePlugin::new();
    let src = ":: Start\n<<if $a>>\n  <<if $b>>inner<</if>>\n<<else>>no\n<</if>>\n";
    let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

    assert_eq!(result.passages.len(), 1);
    let passage = &result.passages[0];

    // Collect macro names and their spans from body blocks
    let macro_info: Vec<(&str, usize)> = passage.body.iter().filter_map(|b| match b {
        knot_core::passage::Block::Macro { name, span, .. } => Some((name.as_str(), span.start)),
        _ => None,
    }).collect();

    // Macros should appear in source order:
    // 1. open "if" (outer)
    // 2. open "if" (inner)
    // 3. close "/if" (inner)
    // 4. open "else"
    // 5. close "/if" (outer)
    assert!(macro_info.len() >= 5,
        "Expected at least 5 macro blocks, got {}: {:?}", macro_info.len(), macro_info);

    // Verify spans are in increasing order
    for i in 1..macro_info.len() {
        assert!(macro_info[i].1 > macro_info[i - 1].1,
            "Body blocks must be in source order: {:?}", macro_info);
    }
}

/// Integration test: Custom macro definitions in [script] passages must
/// be available when translating invocations in normal passages.
///
/// This tests the full `parse()` → virtual doc pipeline, verifying that:
/// 1. `extract_user_callables()` finds Macro.add definitions in script passages
/// 2. The custom macro registry is updated with the definitions
/// 3. `walk_translate()` receives the callable names via `merged_callables`
/// 4. Custom macro invocations in normal passages are translated as
///    function calls, NOT as `/* unknown */` comments
/// 5. The virtual doc map contains the correctly translated JS
/// 6. The assembled virtual doc contains function calls, not unknowns
#[test]
fn custom_macro_invocations_translated_as_function_calls() {
    let plugin = SugarCubePlugin::new();
    let uri = Url::parse("file:///game.tw").unwrap();

    // A .tw file with a script passage defining custom macros and a
    // normal passage that invokes them
    let src = r#":: Macros [script]
Macro.add('addTime', {
handler: function() {
    var hours = this.args[0];
    State.variables.time += hours;
}
});

Macro.add('setSceneLoc', {
handler: function() {
    State.variables.scene = this.args[0];
}
});

Macro.add('earn', {
handler: function() {
    State.variables.gold += this.args[0];
}
});

:: Work
<<setSceneLoc "file-review-bay">>
<<earn 2500>>
<<addTime 25>>
"#;

    let _result = plugin.parse(&uri, src);

    // Verify the virtual doc map was populated
    let docs = plugin.virtual_docs();
    assert!(!docs.is_empty(), "Virtual doc map should not be empty after parse");

    // Get the assembled virtual doc content
    let vdoc = docs.assemble_virtual_doc();

    // The custom macro invocations should be translated as function calls,
    // NOT as /* unknown */ comments
    assert!(
        !vdoc.contains("/* unknown */"),
        "Custom macro invocations should NOT produce /* unknown */ comments.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("setSceneLoc("),
        "setSceneLoc should be translated as a function call.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("earn("),
        "earn should be translated as a function call.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("addTime("),
        "addTime should be translated as a function call.\n\
         Virtual doc content:\n{}", vdoc
    );

    // The script passage should also emit standalone function declarations
    // for the custom macros
    assert!(
        vdoc.contains("function addTime("),
        "Script passage should emit standalone function declaration for addTime.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("function setSceneLoc("),
        "Script passage should emit standalone function declaration for setSceneLoc.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("function earn("),
        "Script passage should emit standalone function declaration for earn.\n\
         Virtual doc content:\n{}", vdoc
    );
}

/// Integration test: Cross-file custom macro definitions are available
/// when translating passages in a different file.
///
/// When file A defines custom macros and file B uses them, the workspace-wide
/// custom macro registry ensures file B's passages can translate invocations
/// as function calls.
#[test]
fn cross_file_custom_macros_available() {
    let plugin = SugarCubePlugin::new();
    let uri_a = Url::parse("file:///macros.tw").unwrap();
    let uri_b = Url::parse("file:///story.tw").unwrap();

    // File A: script passage defining custom macros
    let src_a = r#":: Macros [script]
Macro.add('addTime', {
handler: function() {
    var hours = this.args[0];
    State.variables.time += hours;
}
});
"#;

    // File B: normal passage invoking the custom macro
    let src_b = ":: Work\n<<addTime 25>>\n";

    // Parse file A first (registers custom macros)
    let _result_a = plugin.parse(&uri_a, src_a);

    // Parse file B (should have access to custom macros from A)
    let _result_b = plugin.parse(&uri_b, src_b);

    // Check the virtual doc
    let docs = plugin.virtual_docs();
    let vdoc = docs.assemble_virtual_doc();

    assert!(
        !vdoc.contains("/* unknown */"),
        "Cross-file custom macro invocations should NOT produce /* unknown */.\n\
         Virtual doc content:\n{}", vdoc
    );

    assert!(
        vdoc.contains("addTime("),
        "Cross-file addTime should be translated as a function call.\n\
         Virtual doc content:\n{}", vdoc
    );
}
