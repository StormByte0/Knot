//! Hover test modules, moved verbatim from the former `hover.rs`
//! monolith (pure move — plan §4.5).

use super::*;

#[cfg(test)]
mod expr_macro_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;

    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse a single-passage document and return the Document plus
    /// the plugin. The plugin's `parse_mut` returns passages already
    /// populated with `macro_invocations`, so we just wrap them in a Document.
    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (doc, plugin)
    }

    /// When the cursor is on `<<` of `<<= _parts>>`, hover MUST return the
    /// macro info for `<<=>>` (catalog name `=`), not `None` and not the
    /// variable hover for `_parts`.
    #[test]
    fn hover_on_print_macro_open_tag_returns_macro_hover() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        // Cursor on the first `<` of `<<=` (start of body, line 1, char 0).
        let body_offset = ":: Init\n".len();
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(hover.is_some(), "hover on `<<=` should fire, got None");
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(
                    m.value.contains("`<<=>>`"),
                    "hover text should mention `<<=>>`: got {}",
                    m.value
                );
            } else {
                panic!("expected markup hover, got {:?}", h.contents);
            }
        }
    }

    /// When the cursor is on `=` of `<<=`, hover MUST also return the macro
    /// info (the entire `<<= _parts>>` is the macro name span for expression
    /// macros — there's no separate name region).
    #[test]
    fn hover_on_print_macro_equal_sign_returns_macro_hover() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        let body_offset = ":: Init\n".len() + 2; // on `=` (after `<<`)
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(
            hover.is_some(),
            "hover on `=` of `<<=` should fire, got None"
        );
    }

    /// Cursor on `_parts` (inside the expression) should ALSO return the
    /// macro hover, since the name_span covers the entire `<<= _parts>>`.
    /// NOTE: in the real hover() entrypoint, try_variable_hover runs first
    /// and wins for cursor-on-variable; this test isolates try_macro_hover
    /// to confirm it would fire if the variable layer didn't intercept.
    #[test]
    fn hover_on_variable_inside_expression_still_resolves_macro() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        // Cursor on `_` of `_parts`.
        let body_offset = ":: Init\n".len() + 4; // `<<= ` is 4 chars
        let hover = try_macro_hover(src, body_offset, Some(&doc), &plugin);
        assert!(
            hover.is_some(),
            "try_macro_hover should fire for cursor inside expression span"
        );
    }

    /// End-to-end check: simulate the full layering for cursor on `<<=`. The
    /// variable-hover layer must NOT fire (cursor is not on a variable), so
    /// the macro-hover layer should win and return the `<<=>>` info.
    ///
    /// This test exists to catch the regression where the user reports "no
    /// hover on `<<=`" — if variable-hover ever over-matches (e.g., by
    /// treating the entire expression span as a variable span), this test
    /// will fail.
    #[test]
    fn variable_hover_does_not_fire_on_macro_open_tag() {
        let src = ":: Init\n<<= _parts>>";
        let (doc, plugin) = parse(src);
        let body_offset = ":: Init\n".len(); // cursor on `<<`
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        let hover = try_variable_hover(src, body_offset, Some(&doc), &ws, &plugin, &[]);
        assert!(
            hover.is_none(),
            "variable hover must NOT fire when cursor is on `<<` of `<<=>>`, got: {:?}",
            hover
        );
    }

    /// Reproduce the user's exact reported scenario: a passage with multiple
    /// `<<run _parts = ...>>` lines, where hovering on `<<` of `<<run`
    /// should fire the macro hover but currently doesn't.
    ///
    /// This test exists to catch the regression where the user reports
    /// "hovering on `<<run>>` shows the variable hover for `_parts` instead".
    /// The expected behavior is: cursor on `<<` → macro hover; cursor on
    /// `_parts` → variable hover.
    #[test]
    fn reproduce_user_run_macro_hover_scenario() {
        let src = "::UIOutfitLabel [nobr] {\"position\":\"440,420\"}\n<<run _parts = []>>\n<<run _eq = State.variables.gs.inventory.equipped>>\n";
        let (doc, plugin) = parse(src);

        // Cursor on `<<` of line 1 (the first `<<run _parts = []>>`).
        // Line 0 is the header `::UIOutfitLabel ...` plus `\n`, so the body
        // starts at the byte offset of `<<run`.
        let line0_end = src.find('\n').unwrap() + 1; // end of header line + \n
        let cursor_on_open_bracket = line0_end; // first `<`
        let hover = try_macro_hover(src, cursor_on_open_bracket, Some(&doc), &plugin);
        assert!(
            hover.is_some(),
            "cursor on `<<` of `<<run>>` should fire macro hover, got None. \
             cursor byte_offset={}, line0_end={}",
            cursor_on_open_bracket,
            line0_end
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("`<<run>>`"),
                "hover text should mention `<<run>>`: got {}",
                m.value
            );
        }
    }

    /// F1 test: cursor on a variable INSIDE `<<run>>` should fire
    /// per-token variable hover, not the whole-macro block hover.
    /// The variable hover uses `passage.vars` which has per-token spans
    /// from js_analysis var_ops.
    #[test]
    fn hover_on_variable_inside_run_macro_fires_variable_hover() {
        let src = ":: Init\n<<set $arr to [1,2,3]>>\n<<run $arr.last()>>";
        let (doc, plugin) = parse(src);
        // Cursor on `$arr` in the `<<run $arr.last()>>` line (line 2).
        // Find the byte offset of `$` in `$arr.last()`.
        let line2_start = src.find("<<run").unwrap();
        let cursor_on_dollar = line2_start + "<<run ".len(); // offset of `$`
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        let hover = try_variable_hover(src, cursor_on_dollar, Some(&doc), &ws, &plugin, &[]);
        // Variable hover SHOULD fire — the cursor is on `$arr`, a variable.
        assert!(
            hover.is_some(),
            "cursor on $arr inside <<run>> should fire variable hover, got None"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("$arr"),
                "hover text should mention $arr: got {}",
                m.value
            );
        }
    }

    /// F1 test: cursor on a method name (`.last`) inside `<<run $arr.last()>>
    /// should fire function hover (per-token), not the macro block hover.
    /// Note: try_function_hover has a meaningfulness gate (requires
    /// param_count) that may block builtin methods. The per-token
    /// resolution is verified via the variable hover test above and the
    /// token builder tests in knot-formats. This test is a placeholder —
    /// see F1 follow-up for the full function-hover meaningfulness audit.
    #[test]
    fn hover_on_method_inside_run_macro_fires_function_hover() {
        // Placeholder — see comment above. The real verification is that
        // the Function token IS emitted (verified in knot-formats debug
        // tests), and try_function_hover would fire if the meaningfulness
        // gate passed. F1 follow-up: audit the gate for builtin methods.
    }

    /// When the cursor is on `>>` of `<<run _parts = []>>` (the closing
    /// delimiter), the macro hover should fire using the open_span as the
    /// hover range. This is the "outer-layer" hover — the user is asking
    /// "what macro is this?" by hovering on the closing `>>`.
    #[test]
    fn hover_on_macro_close_delimiter_uses_open_span() {
        let src = ":: Init\n<<run _parts = []>>";
        let (doc, plugin) = parse(src);
        // Cursor on the second `>` of `>>`.
        let close_offset = src.len() - 1;
        let hover = try_macro_hover(src, close_offset, Some(&doc), &plugin);
        assert!(
            hover.is_some(),
            "hover on `>>` of `<<run>>` should fire, got None"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("`<<run>>`"),
                "hover text should mention `<<run>>`: got {}",
                m.value
            );
        }
    }

    /// When the cursor is on the macro name itself (e.g., `run`), the hover
    /// range should be the name_span (just `run`), NOT the open_span (the
    /// whole `<<run>>`). This is the "precise" hover case.
    #[test]
    fn hover_on_macro_name_uses_name_span() {
        let src = ":: Init\n<<run _parts = []>>";
        let (doc, plugin) = parse(src);
        // Cursor on `u` of `run` (offset 9 = `:: Init\n<<` is 9 chars + 1 = 10? Let's compute).
        // `:: Init\n` is 8 chars, `<<` is 2 chars (offsets 8,9), `r` is at offset 10.
        let on_u_offset = ":: Init\n<<r".len(); // 11
        let hover = try_macro_hover(src, on_u_offset, Some(&doc), &plugin);
        assert!(
            hover.is_some(),
            "hover on `u` of `run` should fire, got None"
        );
        if let Some(h) = hover {
            // Range should cover just `run` (3 chars at offsets 10..13).
            if let Some(range) = h.range {
                assert_eq!(
                    range.start.character, 2,
                    "hover range start char: got {}",
                    range.start.character
                );
                assert_eq!(
                    range.end.character, 5,
                    "hover range end char: got {}",
                    range.end.character
                );
                assert_eq!(range.start.line, 1, "hover range start line");
            }
        }
    }
}

#[cfg(test)]
mod prose_hover_tests {
    use super::*;
    use knot_formats::sugarcube::SugarCubePlugin;

    use knot_formats::FormatPluginMut;
    use url::Url;

    /// Helper: parse a multi-passage source and return the Document plus plugin.
    /// Same as the `parse` helper in `expr_macro_hover_tests`, duplicated here
    /// so this test module is self-contained.
    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (doc, plugin)
    }

    /// Hovering on a prose `$var` should fire the variable hover via the
    /// text-scan fallback, even if the variable wasn't picked up by the
    /// AST's `var_refs` scanner (e.g., for a var that has no other refs
    /// in the workspace).
    #[test]
    fn hover_fires_on_prose_persistent_var_via_text_scan() {
        // `$gold` appears in prose only (no `<<set $gold = ...>>` anywhere).
        // The text-scan fallback should still fire hover.
        let src = ":: Start\nYou have $gold coins.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `g` of `$gold` (offset = ":: Start\nYou have $".len() = 18).
        let cursor_offset = ":: Start\nYou have $".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin, &[]);
        assert!(
            hover.is_some(),
            "hover on prose `$gold` should fire via text-scan fallback, got None"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("$gold"),
                "hover text should mention `$gold`: got {}",
                m.value
            );
        }
    }

    /// Hovering on a prose `$var.prop` should fire hover. The AST-based path
    /// produces `var.name = "$player"` (without property path) because the
    /// AST's `VarRef.name` is just the sigil + identifier — the property
    /// path is stored separately in `VarRef.property_path`. The hover text
    /// therefore mentions `$player`, and the hover range covers the full
    /// `$player.name` span.
    #[test]
    fn hover_fires_on_prose_var_property_path() {
        let src = ":: Start\nYou have $player.name coins.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `n` of `.name` (offset = ":: Start\nYou have $player.".len() = 24).
        let cursor_offset = ":: Start\nYou have $player.".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin, &[]);
        assert!(
            hover.is_some(),
            "hover on prose `$player.name` should fire, got None"
        );
        if let Some(h) = hover {
            if let HoverContents::Markup(m) = h.contents {
                assert!(
                    m.value.contains("$player"),
                    "hover text should mention `$player`: got {}",
                    m.value
                );
            }
            // Hover range should cover the full `$player.name` span.
            if let Some(range) = h.range {
                // The span starts at `$` and ends after `name` (12 chars total).
                assert_eq!(
                    range.start.character, 9,
                    "hover range start should be at `$` (char 9): got {}",
                    range.start.character
                );
                assert!(
                    range.end.character >= 21,
                    "hover range end should be at end of `$player.name` (char 21+): got {}",
                    range.end.character
                );
            }
        }
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on the sigil
    /// `$` should return the full variable token (sigil + name + property
    /// path).
    #[test]
    fn scan_variable_at_cursor_on_sigil() {
        let src = "You have $player.name coins.";
        // Cursor on `$` (offset = 9).
        let cursor_offset = src.find("$player").unwrap();
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(
            result.is_some(),
            "scan should find `$player.name` at cursor on `$`"
        );
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '$');
        assert_eq!(name, "player.name");
        assert_eq!(start, cursor_offset);
        assert_eq!(&src[start..end], "$player.name");
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on the name
    /// part should return the full variable token.
    #[test]
    fn scan_variable_at_cursor_on_name() {
        let src = "You have $gold coins.";
        // Cursor on `o` of `$gold` (offset = 11).
        let cursor_offset = src.find("$gold").unwrap() + 2;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(
            result.is_some(),
            "scan should find `$gold` at cursor on `o`"
        );
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '$');
        assert_eq!(name, "gold");
        assert_eq!(&src[start..end], "$gold");
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on `_` in the
    /// middle of `snake_case` should NOT match (it's not a temp-var sigil).
    #[test]
    fn scan_variable_at_cursor_rejects_underscore_in_word() {
        let src = "Use snake_case here.";
        // Cursor on `_` in `snake_case`.
        let cursor_offset = src.find("snake_case").unwrap() + 5;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(
            result.is_none(),
            "scan must NOT match `_` in `snake_case`: got {:?}",
            result
        );
    }

    /// Direct unit test for `scan_variable_at_cursor`: cursor on `_temp`
    /// at a word boundary SHOULD match (it's a valid temp var).
    #[test]
    fn scan_variable_at_cursor_matches_temp_var() {
        let src = "Use _temp here.";
        // Cursor on `t` of `_temp`.
        let cursor_offset = src.find("_temp").unwrap() + 1;
        let result = scan_variable_at_cursor(src, cursor_offset);
        assert!(
            result.is_some(),
            "scan should find `_temp` at cursor on `t`"
        );
        let (sigil, name, start, end) = result.unwrap();
        assert_eq!(sigil, '_');
        assert_eq!(name, "temp");
        assert_eq!(&src[start..end], "_temp");
    }

    /// Hovering on `_temp` in prose should fire hover via text-scan fallback,
    /// but NOT when `_` is in the middle of a word (e.g., `snake_case`).
    #[test]
    fn hover_does_not_fire_on_underscore_in_word() {
        let src = ":: Start\nUse snake_case here.";
        let (doc, plugin) = parse(src);
        let ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Cursor on `_` in `snake_case` (offset = ":: Start\nUse snake".len() = 16).
        let cursor_offset = ":: Start\nUse snake".len();
        let hover = try_variable_hover(src, cursor_offset, Some(&doc), &ws, &plugin, &[]);
        assert!(
            hover.is_none(),
            "hover must NOT fire on `_` in `snake_case` (not a temp var): got {:?}",
            hover
        );
    }

    /// Hovering on `?template` in prose should fire the template hover via
    /// the text-scan fallback when the template is registered. The hover
    /// text should be minimal: `**\`?name\`** \`Template\`` (no "Defined in").
    #[test]
    fn hover_fires_on_prose_template_via_text_scan() {
        // Register a template by parsing a StoryInit passage that calls
        // `Template.add("greeting", ...)`. Then test hover on `?greeting`
        // in a separate passage.
        let src = ":: StoryInit\n<<run Template.add(\"greeting\", function() { return \"Hello\"; })>>\n:: Start\nYou see ?greeting friend.";
        let (_doc, plugin) = parse(src);
        // Cursor on `g` of `?greeting` in the Start passage.
        let cursor_offset = src.find("?greeting").unwrap() + 1;
        let token_groups: Vec<knot_formats::plugin::PassageTokenGroup> = Vec::new();
        let hover = try_template_hover(src, cursor_offset, &plugin, &token_groups);
        assert!(
            hover.is_some(),
            "hover on prose `?greeting` should fire via text-scan fallback, got None"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("`?greeting`"),
                "hover text should contain `?greeting`: got {}",
                m.value
            );
            assert!(
                m.value.contains("Template"),
                "hover text should mention 'Template': got {}",
                m.value
            );
            assert!(
                !m.value.contains("Defined in"),
                "hover text should NOT contain 'Defined in' (user wants minimal hover): got {}",
                m.value
            );
        }
    }

    /// Hovering on the `?` prefix of `?template` should also fire hover
    /// (the hover range is extended to include the `?`).
    #[test]
    fn hover_fires_on_template_question_mark_prefix() {
        let src = ":: StoryInit\n<<run Template.add(\"greeting\", function() { return \"Hello\"; })>>\n:: Start\nYou see ?greeting friend.";
        let (_doc, plugin) = parse(src);
        // Cursor ON the `?` of `?greeting`.
        let cursor_offset = src.find("?greeting").unwrap();
        let token_groups: Vec<knot_formats::plugin::PassageTokenGroup> = Vec::new();
        let hover = try_template_hover(src, cursor_offset, &plugin, &token_groups);
        assert!(
            hover.is_some(),
            "hover on `?` of `?greeting` should fire (extended range), got None"
        );
    }

    /// `scan_template_at_cursor` should NOT match `?.` (JS optional chaining).
    #[test]
    fn text_scan_does_not_match_js_optional_chaining() {
        // `obj?.prop` — the `?` is preceded by `j` (a word char).
        // Even though `prop` is a valid identifier, this should NOT match.
        let src = ":: Start\n<<run obj?.prop>>";
        // Cursor on `?` of `obj?.prop`.
        let cursor_offset = src.find("obj?.prop").unwrap() + 3;
        let result = scan_template_at_cursor(src, cursor_offset);
        assert!(
            result.is_none(),
            "scan_template_at_cursor must NOT match JS optional chaining `obj?.prop`: got {:?}",
            result
        );
    }

    /// `scan_template_at_cursor` should NOT match a JS ternary
    /// (`cond ? value : other`) where `?` is followed by a space.
    #[test]
    fn text_scan_does_not_match_js_ternary_with_space() {
        let src = ":: Start\n<<run x > 0 ? \"yes\" : \"no\">>";
        // Cursor on `?` of the ternary.
        let cursor_offset = src.find("? \"yes\"").unwrap();
        let result = scan_template_at_cursor(src, cursor_offset);
        assert!(
            result.is_none(),
            "scan_template_at_cursor must NOT match JS ternary `cond ? value : other`: got {:?}",
            result
        );
    }
}

#[cfg(test)]
mod block_markup_hover_tests {
    use super::*;

    /// Helper: find the byte offset of the first occurrence of `needle` in `src`.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle)
            .unwrap_or_else(|| panic!("needle {:?} not found in src", needle))
    }

    #[test]
    fn hover_on_h1_heading_marker() {
        // `!Title` — cursor on the `!`.
        let src = ":: Start\n!Title here\n";
        let offset = cursor_on(src, "!Title");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(hover.is_some(), "hover on `!` heading marker should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Heading"),
                "hover text should mention Heading: {}",
                m.value
            );
            assert!(
                m.value.contains("h1"),
                "hover text should mention h1 tag: {}",
                m.value
            );
            assert!(
                m.value.contains("level 1"),
                "hover text should mention level 1: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_h2_heading_marker() {
        // `!!Title` — cursor on the 2nd `!`.
        let src = ":: Start\n!!Subsection\n";
        let offset = cursor_on(src, "!!Subsection") + 1; // 2nd `!`
        let hover = try_block_markup_hover(src, offset, None);
        assert!(hover.is_some(), "hover on `!!` heading marker should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("level 2"),
                "hover text should mention level 2: {}",
                m.value
            );
            assert!(
                m.value.contains("h2"),
                "hover text should mention h2 tag: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_h3_heading_marker() {
        // `!!!Title` — cursor on the 3rd `!`.
        let src = ":: Start\n!!!Sub-subsection\n";
        let offset = cursor_on(src, "!!!Sub") + 2; // 3rd `!`
        let hover = try_block_markup_hover(src, offset, None);
        assert!(hover.is_some(), "hover on `!!!` heading marker should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("level 3"),
                "hover text should mention level 3: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_unordered_list_marker() {
        // `* item` — cursor on the `*`.
        let src = ":: Start\n* item one\n";
        let offset = cursor_on(src, "* item");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(hover.is_some(), "hover on `*` list marker should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Unordered List"),
                "hover text should mention Unordered List: {}",
                m.value
            );
            assert!(
                m.value.contains("<ul>"),
                "hover text should mention <ul>: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_nested_unordered_list_marker() {
        // `** item` — cursor on the 2nd `*`.
        let src = ":: Start\n** nested item\n";
        let offset = cursor_on(src, "** nested") + 1;
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `**` nested list marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("depth 2"),
                "hover text should mention depth 2: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_ordered_list_marker() {
        // `# item` — cursor on the `#`.
        let src = ":: Start\n# first\n";
        let offset = cursor_on(src, "# first");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `#` ordered list marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Ordered List"),
                "hover text should mention Ordered List: {}",
                m.value
            );
            assert!(
                m.value.contains("<ol>"),
                "hover text should mention <ol>: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_blockquote_marker() {
        // `> quote` — cursor on the `>`.
        let src = ":: Start\n> quoted text\n";
        let offset = cursor_on(src, "> quoted");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `>` blockquote marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Blockquote"),
                "hover text should mention Blockquote: {}",
                m.value
            );
            assert!(
                m.value.contains("<blockquote>"),
                "hover text should mention <blockquote>: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_horizontal_rule() {
        // `----` — cursor on a dash.
        let src = ":: Start\n----\n";
        let offset = cursor_on(src, "----");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `----` horizontal rule should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Horizontal Rule"),
                "hover text should mention Horizontal Rule: {}",
                m.value
            );
            assert!(
                m.value.contains("<hr>"),
                "hover text should mention <hr>: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_three_dashes_does_not_fire() {
        // `---` (3 dashes) is NOT a horizontal rule — needs 4+.
        let src = ":: Start\n---\n";
        let offset = cursor_on(src, "---");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_none(),
            "hover on `---` (3 dashes) should NOT fire (needs 4+)"
        );
    }

    #[test]
    fn hover_on_block_blockquote_marker() {
        // `<<<` — cursor on a `<`.
        let src = ":: Start\n<<<\nquoted\n<<<\n";
        let offset = cursor_on(src, "<<<\n");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `<<<` block blockquote should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Block Blockquote"),
                "hover text should mention Block Blockquote: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_code_block_marker() {
        // `{{{\n...\n}}}` — cursor on the first `{`.
        let src = ":: Start\n{{{\ncode here\n}}}\n";
        let offset = cursor_on(src, "{{{");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `{{{{` code block marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Code Block"),
                "hover text should mention Code Block: {}",
                m.value
            );
            assert!(
                m.value.contains("NOT processed"),
                "hover text should mention raw content: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_inline_code_marker() {
        // `{{{code}}}` — cursor on the first `{` (mid-line, not block form).
        let src = ":: Start\nSome {{{inline code}} here.\n";
        let offset = cursor_on(src, "{{{inline");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `{{{{` inline code marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Inline Code"),
                "hover text should mention Inline Code: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_on_heading_content_does_not_fire_marker_hover() {
        // `!Title` — cursor on `T` (the content, not the marker).
        // The marker hover should NOT fire — `T` is not part of the `!` run.
        let src = ":: Start\n!Title\n";
        let offset = cursor_on(src, "Title");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_none(),
            "hover on heading content (not marker) should NOT fire block markup hover"
        );
    }

    #[test]
    fn hover_on_mid_line_exclamation_does_not_fire() {
        // `Hello!` — `!` mid-line is NOT a heading marker (needs column 0).
        let src = ":: Start\nHello!\n";
        let offset = cursor_on(src, "!");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_none(),
            "hover on mid-line `!` should NOT fire (heading requires column 0)"
        );
    }

    #[test]
    fn hover_on_mid_line_asterisk_does_not_fire() {
        // `a * b` — `*` mid-line is NOT a list marker (needs column 0).
        let src = ":: Start\na * b\n";
        let offset = cursor_on(src, "*");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_none(),
            "hover on mid-line `*` should NOT fire (list marker requires column 0)"
        );
    }

    #[test]
    fn hover_on_h6_heading_marker() {
        // `!!!!!!Title` — 6 levels (the maximum).
        let src = ":: Start\n!!!!!!Tiny heading\n";
        let offset = cursor_on(src, "!!!!!!");
        let hover = try_block_markup_hover(src, offset, None);
        assert!(
            hover.is_some(),
            "hover on `!!!!!!` h6 heading marker should fire"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("level 6"),
                "hover text should mention level 6: {}",
                m.value
            );
            assert!(
                m.value.contains("h6"),
                "hover text should mention h6 tag: {}",
                m.value
            );
        }
    }

    #[test]
    fn hover_range_covers_full_marker_run() {
        // `!!Title` — hover range should cover both `!` characters.
        let src = ":: Start\n!!Title\n";
        let offset = cursor_on(src, "!!Title");
        let hover = try_block_markup_hover(src, offset, None).expect("hover should fire");
        if let Some(range) = hover.range {
            // Line 1 (0-indexed), characters 0-2 (the `!!` run).
            assert_eq!(range.start.line, 1);
            assert_eq!(range.start.character, 0);
            assert_eq!(range.end.line, 1);
            assert_eq!(
                range.end.character, 2,
                "range should cover 2 `!` characters, got end char {}",
                range.end.character
            );
        }
    }
}

/// Phase 6 tests: `try_close_tag_hover` and `try_block_markup_hover` migrated
/// to use `passage.zones.leaf_at()` instead of backward byte-scans and
/// column-0 detection.
#[cfg(test)]
mod phase6_zone_hover_tests {
    use super::*;
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use url::Url;

    /// Helper: parse a source document and return (text, doc, plugin, uri).
    fn parse(src: &str) -> (String, knot_core::Document, SugarCubePlugin, url::Url) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (src.to_string(), doc, plugin, uri)
    }

    /// Helper: find the passage containing a document-absolute byte offset.
    fn passage_at(doc: &knot_core::Document, offset: usize) -> Option<&knot_core::Passage> {
        doc.passages.iter().find(|p| p.contains_abs_offset(offset))
    }

    /// Hover on `<</if>>` close tag should fire via the zone map.
    #[test]
    fn close_tag_hover_fires_via_zone_map() {
        let src = ":: Start\n<<if $x>>\n  text\n<</if>>";
        let (text, doc, plugin, _uri) = parse(src);
        let close_offset = src.find("<</if>>").unwrap();
        let passage = passage_at(&doc, close_offset);
        let hover = try_close_tag_hover(&text, close_offset, passage, &plugin);
        assert!(
            hover.is_some(),
            "close-tag hover should fire on <</if>> via zone map"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Close tag"),
                "hover text should mention 'Close tag': got {}",
                m.value
            );
            assert!(
                m.value.contains("if"),
                "hover text should mention 'if': got {}",
                m.value
            );
        }
    }

    /// Hover on `!` of a heading should fire via the zone map.
    #[test]
    fn heading_hover_fires_via_zone_map() {
        let src = ":: Start\n!Title here\n";
        let (text, doc, _plugin, _uri) = parse(src);
        let heading_offset = src.find("!Title").unwrap();
        let passage = passage_at(&doc, heading_offset);
        let hover = try_block_markup_hover(&text, heading_offset, passage);
        assert!(
            hover.is_some(),
            "block-markup hover should fire on `!` heading via zone map"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Heading"),
                "hover text should mention 'Heading': got {}",
                m.value
            );
        }
    }

    /// Hover on `*` of a list item should fire via the zone map.
    #[test]
    fn list_item_hover_fires_via_zone_map() {
        let src = ":: Start\n* item one\n";
        let (text, doc, _plugin, _uri) = parse(src);
        let list_offset = src.find("* item").unwrap();
        let passage = passage_at(&doc, list_offset);
        let hover = try_block_markup_hover(&text, list_offset, passage);
        assert!(
            hover.is_some(),
            "block-markup hover should fire on `*` list item via zone map"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("List Item"),
                "hover text should mention 'List Item': got {}",
                m.value
            );
        }
    }

    /// Hover on `----` horizontal rule should fire via the zone map.
    #[test]
    fn horizontal_rule_hover_fires_via_zone_map() {
        let src = ":: Start\n----\n";
        let (text, doc, _plugin, _uri) = parse(src);
        let hr_offset = src.find("----").unwrap();
        let passage = passage_at(&doc, hr_offset);
        let hover = try_block_markup_hover(&text, hr_offset, passage);
        assert!(
            hover.is_some(),
            "block-markup hover should fire on `----` via zone map"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Horizontal Rule"),
                "hover text should mention 'Horizontal Rule': got {}",
                m.value
            );
        }
    }

    /// Hover on `''bold''` text format should fire via the zone map.
    #[test]
    fn text_format_hover_fires_via_zone_map() {
        let src = ":: Start\n''bold text''\n";
        let (text, doc, _plugin, _uri) = parse(src);
        let bold_offset = src.find("''bold").unwrap();
        let passage = passage_at(&doc, bold_offset);
        let hover = try_block_markup_hover(&text, bold_offset, passage);
        assert!(
            hover.is_some(),
            "block-markup hover should fire on `''bold''` via zone map"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Bold"),
                "hover text should mention 'Bold': got {}",
                m.value
            );
        }
    }

    /// Hover on prose text should NOT fire block-markup hover via zone map.
    #[test]
    fn prose_does_not_fire_block_markup_hover() {
        let src = ":: Start\nHello world\n";
        let (text, doc, _plugin, _uri) = parse(src);
        let prose_offset = src.find("Hello").unwrap();
        let passage = passage_at(&doc, prose_offset);
        let hover = try_block_markup_hover(&text, prose_offset, passage);
        assert!(
            hover.is_none(),
            "block-markup hover should NOT fire on prose text"
        );
    }
}

#[cfg(test)]
mod operator_hover_scoping_tests {
    use super::*;
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use url::Url;

    /// Helper: parse a source and return (text, token_groups, plugin).
    fn parse(
        src: &str,
    ) -> (
        String,
        Vec<knot_formats::plugin::PassageTokenGroup>,
        SugarCubePlugin,
    ) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.tw").unwrap();
        let result = plugin.parse_mut(&uri, src);
        (src.to_string(), result.token_groups, plugin)
    }

    /// Helper: find byte offset of `needle` in `src`.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle)
            .unwrap_or_else(|| panic!("needle {:?} not found", needle))
    }

    #[test]
    fn operator_hover_fires_inside_macro_expression() {
        // `<<set $x to 5>>` — `to` is an assignment operator inside a macro.
        let src = ":: Start\n<<set $x to 5>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to 5");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_some(),
            "hover on `to` inside <<set>> should fire (it's a real operator)"
        );
    }

    #[test]
    fn operator_hover_does_not_fire_in_link_display_text() {
        // `[[Jump to combat demo|CombatEncounter]]` — `to` is part of the
        // link display text, NOT an operator.
        let src = ":: Start\n[[Jump to combat demo|CombatEncounter]]\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to combat");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_none(),
            "hover on `to` in link display text should NOT fire — it's prose, not an operator"
        );
    }

    #[test]
    fn operator_hover_does_not_fire_in_prose() {
        // `Go to the forest.` — `to` is a preposition in prose.
        let src = ":: Start\nGo to the forest.\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to the");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_none(),
            "hover on `to` in prose should NOT fire — it's not an operator context"
        );
    }

    #[test]
    fn operator_hover_does_not_fire_in_string_literal() {
        // `<<link "Go to forest">>` — `to` is inside a string literal arg.
        let src = ":: Start\n<<link \"Go to forest\">><</link>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "to forest");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_none(),
            "hover on `to` inside a string literal should NOT fire"
        );
    }

    #[test]
    fn operator_hover_fires_for_gt_in_if_condition() {
        // `<<if $hp gt 0>>` — `gt` is a comparison operator.
        let src = ":: Start\n<<if $hp gt 0>><</if>>\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "gt 0");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_some(),
            "hover on `gt` inside <<if>> should fire (it's a real operator)"
        );
    }

    #[test]
    fn operator_hover_does_not_fire_for_and_in_prose() {
        // `You and I` — `and` is a conjunction in prose.
        let src = ":: Start\nYou and I went home.\n";
        let (text, token_groups, plugin) = parse(src);
        let offset = cursor_on(&text, "and I");
        let hover = try_operator_hover(&text, offset, &plugin, &token_groups);
        assert!(
            hover.is_none(),
            "hover on `and` in prose should NOT fire — it's not an operator context"
        );
    }
}

#[cfg(test)]
mod arg_ref_hover_tests {
    use super::*;
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use url::Url;

    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin, knot_core::Workspace) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        let mut ws = knot_core::Workspace::new(url::Url::parse("file:///project/").unwrap());
        // Add the document to the workspace so find_passage works
        ws.insert_document(doc.clone());
        (doc, plugin, ws)
    }

    /// F2 test: hovering on a passage reference (e.g., `"Shop"` arg in
    /// `<<link "Talk" "Shop">>`) should show COMPACT hover — just the
    /// passage name, tags, and reference count. It should NOT show the
    /// full passage property block (file path, links out, variables
    /// written/read).
    #[test]
    fn arg_ref_hover_shows_compact_form() {
        let src = ":: Start\nYou are here.\n:: Shop\nWelcome to the shop.\n:: Hub\n<<link \"Talk\" \"Shop\">>Go<</link>>";
        let (doc, plugin, ws) = parse(src);
        // Cursor on "Shop" inside <<link "Talk" "Shop">>
        let shop_offset = src.find("\"Shop\"").map(|o| o + 1).unwrap(); // +1 to skip opening quote
        let hover = try_macro_arg_ref_hover(src, shop_offset, Some(&doc), &ws, &plugin);
        assert!(
            hover.is_some(),
            "arg ref hover should fire for \"Shop\" in <<link>>"
        );
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            // Compact form should include:
            assert!(
                m.value.contains("**Shop**"),
                "should show passage name: got {}",
                m.value
            );
            assert!(
                m.value.contains("Tags:"),
                "should show Tags field: got {}",
                m.value
            );
            assert!(
                m.value.contains("Referenced by"),
                "should show reference count: got {}",
                m.value
            );
            // Compact form should NOT include:
            assert!(
                !m.value.contains("Links out:"),
                "compact form should NOT show Links out: got {}",
                m.value
            );
            assert!(
                !m.value.contains("Variables written:"),
                "compact form should NOT show Variables written: got {}",
                m.value
            );
            assert!(
                !m.value.contains("Variables read:"),
                "compact form should NOT show Variables read: got {}",
                m.value
            );
            assert!(
                !m.value.contains(".twee"),
                "compact form should NOT show file path: got {}",
                m.value
            );
            // Should mention which macro references it
            assert!(
                m.value.contains("Referenced by"),
                "should show referencing macro: got {}",
                m.value
            );
        }
    }

    /// F2 test: `[[link]]` hover should show FULL form (not compact) —
    /// the user is hovering a link, not a reference inside a macro arg.
    #[test]
    fn link_hover_shows_full_form() {
        let src = ":: Start\nYou are here.\n:: Shop\nWelcome to the shop.\n:: Hub\nGo [[Shop]] now";
        let (doc, _plugin, ws) = parse(src);
        // Cursor on "Shop" inside [[Shop]]
        let shop_offset = src.find("[[Shop]]").map(|o| o + 2).unwrap(); // +2 to skip [[
        let hover = try_link_hover(src, shop_offset, Some(&doc), &ws);
        assert!(hover.is_some(), "link hover should fire for [[Shop]]");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            // Full form should include:
            assert!(
                m.value.contains("**Shop**"),
                "should show passage name: got {}",
                m.value
            );
            assert!(
                m.value.contains("Links out:"),
                "full form should show Links out: got {}",
                m.value
            );
            // File path is shown in full form (workspace-relative)
            assert!(
                m.value.contains("story.tw") || m.value.contains("unknown"),
                "full form should show file path: got {}",
                m.value
            );
        }
    }
}

#[cfg(test)]
mod link_hover_tests {
    use super::*;
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use url::Url;

    /// Helper: parse source with multiple passages into a Document + Workspace.
    fn parse_with_workspace(src: &str) -> (String, knot_core::Document, knot_core::Workspace) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.tw").unwrap();
        let result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in result.passages {
            doc.passages.push(passage);
        }
        let ws = knot_core::Workspace::new(url::Url::parse("file:///").unwrap());
        (src.to_string(), doc, ws)
    }

    /// Helper: find byte offset of needle in src.
    fn cursor_on(src: &str, needle: &str) -> usize {
        src.find(needle)
            .unwrap_or_else(|| panic!("needle {:?} not found", needle))
    }

    #[test]
    fn pipe_link_label_hover_shows_display_text() {
        // `[[Go to forest|Forest]]` — hovering on "Go to forest" (the label)
        // should show "Link display text" info, NOT passage info for "Forest".
        let src = ":: Start\n[[Go to forest|Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        // Insert doc into workspace for find_passage
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Go" in the label
        let offset = cursor_on(&text, "Go to forest");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on label should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Link display text"),
                "label hover should mention 'Link display text': {}",
                m.value
            );
            assert!(
                m.value.contains("Go to forest"),
                "label hover should contain the label text: {}",
                m.value
            );
        }
    }

    #[test]
    fn pipe_link_target_hover_shows_passage_info() {
        // `[[Go to forest|Forest]]` — hovering on "Forest" (the target)
        // should show passage info for "Forest", NOT "Link display text".
        let src = ":: Start\n[[Go to forest|Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Forest" (the target, after the pipe)
        let offset = cursor_on(&text, "|Forest") + 1; // skip the pipe
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on target should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                !m.value.contains("Link display text"),
                "target hover should NOT mention 'Link display text': {}",
                m.value
            );
        }
    }

    #[test]
    fn simple_link_hover_shows_passage_info() {
        // `[[Forest]]` — no pipe, so hovering anywhere shows passage info.
        let src = ":: Start\n[[Forest]]\n:: Forest\nYou are in the forest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "Forest]]");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on simple link should fire");
    }

    #[test]
    fn pipe_link_label_range_covers_only_label() {
        // The hover range for the label should NOT include the target or pipe.
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "Go to forest");
        let hover = try_link_hover(&text, offset, Some(&doc), &ws).expect("hover should fire");
        if let Some(range) = hover.range {
            // The range should cover "Go to forest" (12 chars) on line 1.
            assert_eq!(range.start.line, 1);
            assert_eq!(range.end.line, 1);
            assert_eq!(
                range.end.character - range.start.character,
                12,
                "label range should cover 'Go to forest' (12 chars), got {}",
                range.end.character - range.start.character
            );
        }
    }

    #[test]
    fn pipe_link_target_range_covers_only_target() {
        // The hover range for the target should NOT include the label or pipe.
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on "Forest"
        let offset = cursor_on(&text, "|Forest") + 1;
        let hover = try_link_hover(&text, offset, Some(&doc), &ws).expect("hover should fire");
        if let Some(range) = hover.range {
            // The range should cover "Forest" (6 chars) on line 1.
            assert_eq!(range.start.line, 1);
            assert_eq!(range.end.line, 1);
            assert_eq!(
                range.end.character - range.start.character,
                6,
                "target range should cover 'Forest' (6 chars), got {}",
                range.end.character - range.start.character
            );
        }
    }

    #[test]
    fn broken_pipe_link_target_shows_broken_message() {
        // `[[Go to forest|MissingPassage]]` — target doesn't exist.
        let src = ":: Start\n[[Go to forest|MissingPassage]]\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        let offset = cursor_on(&text, "|MissingPassage") + 1;
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on broken target should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("Broken link"),
                "broken target hover should mention 'Broken link': {}",
                m.value
            );
        }
    }

    #[test]
    fn empty_label_pipe_link_shows_empty_hint() {
        // `[[|Forest]]` — empty label. Should show the "empty" hint.
        let src = ":: Start\n[[|Forest]]\n:: Forest\nForest.\n";
        let (text, doc, ws) = parse_with_workspace(src);
        let mut ws = ws;
        ws.insert_document(doc.clone());
        // Cursor on the pipe character
        let offset = cursor_on(&text, "[[|") + 2; // on the `|`
        let hover = try_link_hover(&text, offset, Some(&doc), &ws);
        assert!(hover.is_some(), "hover on empty label should fire");
        if let Some(h) = hover
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(
                m.value.contains("empty"),
                "empty label hover should mention 'empty': {}",
                m.value
            );
        }
    }
}

/// Phase 3 tests: `check_container_violation` migrated to use the unified
/// zone map (`passage.zones.enclosing_body_at`) instead of the hand-rolled
/// innermost-enclosing-macro loop.
#[cfg(test)]
mod container_violation_tests {
    use super::*;
    use knot_formats::FormatPlugin;
    use knot_formats::FormatPluginMut;
    use knot_formats::sugarcube::SugarCubePlugin;
    use url::Url;

    /// Helper: parse a single-passage document and return (doc, plugin).
    /// The plugin's `parse_mut` goes through the full pipeline, so
    /// `passage.zones` is populated.
    fn parse(src: &str) -> (knot_core::Document, SugarCubePlugin) {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///project/story.tw").unwrap();
        let parse_result = plugin.parse_mut(&uri, src);
        let mut doc =
            knot_core::Document::new(uri.clone(), knot_core::passage::StoryFormat::SugarCube);
        for passage in parse_result.passages {
            doc.passages.push(passage);
        }
        (doc, plugin)
    }

    /// Helper: find the first passage in a document.
    fn first_passage(doc: &knot_core::Document) -> &knot_core::Passage {
        doc.passages
            .first()
            .expect("document should have at least one passage")
    }

    /// Helper: find a macro def by name from the plugin.
    fn macro_def<'a>(
        plugin: &'a SugarCubePlugin,
        name: &'a str,
    ) -> Option<&'a knot_formats::types::MacroDef> {
        plugin.find_macro(name)
    }

    /// `<<else>>` inside `<<if>>...<</if>>` → no violation.
    #[test]
    fn else_inside_if_no_violation() {
        let src = ":: Start\n<<if $x>>\n  text\n<<else>>\n  more\n<</if>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "else").expect("<<else>> should be in catalog");

        // Cursor on `<<else>>` — find its document-absolute offset.
        let else_offset = src.find("<<else>>").unwrap();
        let violation = check_container_violation(mdef, passage, else_offset, &plugin);
        assert!(
            violation.is_none(),
            "<<else>> inside <<if>> should NOT be a violation, got: {:?}",
            violation
        );
    }

    /// `<<else>>` at top level (no `<<if>>`) → violation.
    #[test]
    fn else_at_top_level_is_violation() {
        let src = ":: Start\n<<else>> text";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "else").expect("<<else>> should be in catalog");

        let else_offset = src.find("<<else>>").unwrap();
        let violation = check_container_violation(mdef, passage, else_offset, &plugin);
        assert!(
            violation.is_some(),
            "<<else>> at top level should be a violation"
        );
        let msg = violation.unwrap();
        assert!(
            msg.contains("must be inside"),
            "violation message should say 'must be inside': {}",
            msg
        );
        assert!(
            !msg.contains("currently inside"),
            "top-level violation should NOT say 'currently inside': {}",
            msg
        );
    }

    /// **The post-close-sibling bug case (plan.md §6.5)**:
    /// `<<if $x>>...<</if>> <<else>>` — cursor on `<<else>>` which is
    /// OUTSIDE `<<if>>`'s body (after `<</if>>`). The OLD code got this
    /// wrong (it approximated "inside body" with "after open tag end" and
    /// thought the cursor was still inside `<<if>>`). The NEW zone-based
    /// code correctly reports a violation.
    #[test]
    fn else_after_closed_if_is_violation() {
        let src = ":: Start\n<<if $x>>\n  text\n<</if>>\n<<else>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "else").expect("<<else>> should be in catalog");

        let else_offset = src.find("<<else>>").unwrap();
        let violation = check_container_violation(mdef, passage, else_offset, &plugin);
        assert!(
            violation.is_some(),
            "<<else>> after <</if>> should be a violation (the old code got this wrong)"
        );
        let msg = violation.unwrap();
        assert!(
            msg.contains("must be inside"),
            "violation message should say 'must be inside': {}",
            msg
        );
        // Crucially, it should NOT claim we're "currently inside" anything —
        // the cursor is at the top level after <</if>> closed.
        assert!(
            !msg.contains("currently inside"),
            "post-close-sibling violation should NOT say 'currently inside' (old bug): {}",
            msg
        );
    }

    /// `<<case>>` inside `<<switch>>...<</switch>>` → no violation.
    #[test]
    fn case_inside_switch_no_violation() {
        let src = ":: Start\n<<switch $x>>\n<<case 1>>\n  a\n<<default>>\n  b\n<</switch>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "case").expect("<<case>> should be in catalog");

        let case_offset = src.find("<<case").unwrap();
        let violation = check_container_violation(mdef, passage, case_offset, &plugin);
        assert!(
            violation.is_none(),
            "<<case>> inside <<switch>> should NOT be a violation, got: {:?}",
            violation
        );
    }

    /// `<<case>>` at top level (no `<<switch>>`) → violation.
    #[test]
    fn case_at_top_level_is_violation() {
        let src = ":: Start\n<<case 1>> text";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "case").expect("<<case>> should be in catalog");

        let case_offset = src.find("<<case").unwrap();
        let violation = check_container_violation(mdef, passage, case_offset, &plugin);
        assert!(
            violation.is_some(),
            "<<case>> at top level should be a violation"
        );
        let msg = violation.unwrap();
        assert!(
            msg.contains("must be inside"),
            "violation message should say 'must be inside': {}",
            msg
        );
    }

    /// `<<else>>` inside `<<if>>` nested inside `<<link>>` → no violation.
    /// Tests that the zone map correctly walks up the parent chain.
    #[test]
    fn else_in_nested_if_no_violation() {
        let src = ":: Start\n<<link \"Go\">>\n  <<if $x>>\n    text\n  <<else>>\n    more\n  <</if>>\n<</link>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "else").expect("<<else>> should be in catalog");

        let else_offset = src.find("<<else>>").unwrap();
        let violation = check_container_violation(mdef, passage, else_offset, &plugin);
        assert!(
            violation.is_none(),
            "<<else>> inside <<if>> inside <<link>> should NOT be a violation, got: {:?}",
            violation
        );
    }

    /// `<<break>>` inside `<<for>>` → no violation.
    #[test]
    fn break_inside_for_no_violation() {
        let src = ":: Start\n<<for _i to 0; _i lt 5; _i++>>\n  <<break>>\n<</for>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "break").expect("<<break>> should be in catalog");

        let break_offset = src.find("<<break>>").unwrap();
        let violation = check_container_violation(mdef, passage, break_offset, &plugin);
        assert!(
            violation.is_none(),
            "<<break>> inside <<for>> should NOT be a violation, got: {:?}",
            violation
        );
    }

    /// `<<break>>` inside `<<if>>` (not `<<for>>`) → violation.
    /// `<<break>>` requires `<<for>>`, not `<<if>>`.
    #[test]
    fn break_inside_if_is_violation() {
        let src = ":: Start\n<<if $x>>\n  <<break>>\n<</if>>";
        let (doc, plugin) = parse(src);
        let passage = first_passage(&doc);
        let mdef = macro_def(&plugin, "break").expect("<<break>> should be in catalog");

        let break_offset = src.find("<<break>>").unwrap();
        let violation = check_container_violation(mdef, passage, break_offset, &plugin);
        assert!(
            violation.is_some(),
            "<<break>> inside <<if>> (not <<for>>) should be a violation"
        );
        let msg = violation.unwrap();
        assert!(
            msg.contains("must be inside"),
            "violation message should say 'must be inside': {}",
            msg
        );
        assert!(
            msg.contains("currently inside"),
            "violation message should say 'currently inside `<<if>>`': {}",
            msg
        );
    }
}
