---
Task ID: 6
Agent: snowman-impl
Task: Implement full Snowman format plugin

Work Log:
- Read existing Snowman plugin, knot-core passage types, and knot-formats plugin trait
- Read SugarCube plugin as reference for diagnostics and block model patterns
- Rewrote split_passages to use byte-offset tracking via ParsedHeader struct (fixes buggy text.find(line) with duplicate lines)
- Implemented ERB-style template parsing: <%= expr %>, <% code %>, <%- expr %> via parse_template_segments()
- Enhanced variable operations to track reads in <%= %> blocks and writes in <% %> blocks
- Added window.story.state.varName as alias for s.varName (both read and write)
- Added Snowman-specific diagnostics: sm-unclosed-template, sm-unclosed-link, sm-undefined-var
- Replaced single Block::Text with proper block model: Text, Macro (script), Expression, Incomplete
- Added PassageHeader and PassageFooter to special passage registry
- Added header/footer tag detection (passages with [header] or [footer] tags treated as special)
- Built comprehensive test suite: 20 tests covering all required scenarios
- Verified all tests pass (20/20) with no warnings in the build

Stage Summary:
- Full Snowman format plugin rewrite complete at /home/z/my-project/Knot_ver2/crates/knot-formats/src/snowman/mod.rs
- 20 tests all passing: parse_simple_passage, parse_variable_operations, detect_special_passages, empty_input_is_ok, expression_block_variable_read, script_block_variable_write, unescaped_expression_block, unclosed_template_diagnostic, unclosed_link_diagnostic, mixed_text_expression_script_blocks, variable_read_in_text_context, variable_write_in_script_context, multiple_variable_operations, empty_script_block, incomplete_block_from_unclosed_template, passage_header_footer_special, split_passages_byte_offset_tracking, window_story_state_alias, undefined_variable_hint, no_undefined_var_warning_when_written
- Full workspace builds cleanly with no warnings
---
Task ID: 2
Agent: Main
Task: Fix extract_macros sorting bug, add folding_modifier_names(), improve folding range format isolation

Work Log:
- Analyzed the complete SugarCube parsing pipeline: lexer → blocks → body tokens → validation
- Identified critical bug in extract_macros(): macros were NOT sorted by source position
  - Open macros collected first, then close macros, regardless of actual position
  - build_body_blocks() assumed sorted order when interleaving text/macro blocks
  - This caused wrong Block::Macro entries at wrong positions when close tags appeared between open tags (e.g., <</if>> before <<else>>)
- Fixed extract_macros() in blocks.rs: added sort_by_key(|b| span.start) after collecting all macros
- Fixed build_body_blocks() in blocks.rs: simplified to iterate sorted macros directly instead of maintaining separate sorted span list
- Fixed extract_macros() in parsing.rs (dead code): same sorting fix for consistency
- DRY cleanup: removed duplicate RE_MACRO/RE_MACRO_CLOSE from blocks.rs, now re-exports from regexes.rs
  - Updated validation.rs to import from regexes instead of blocks
  - Updated tokens.rs to import from regexes instead of blocks
- Added folding_modifier_names() to FormatPlugin trait with default empty implementation
  - Returns set of block macro names that are structural modifiers (no close tag)
  - e.g., "else", "elseif" inside <<if>>, "case", "default" inside <<switch>>
- Implemented folding_modifier_names() in SugarCube: ["else", "elseif", "case", "default"]
- Updated structure.rs folding range handler to use plugin.folding_modifier_names() instead of hardcoded list
  - Replaced match on StoryFormat::SugarCube with format-isolated trait method call
  - Added std::collections::HashSet import
- Added 4 new tests:
  - extract_macros_sorted_by_position: verifies basic source-order sorting
  - extract_macros_nested_sorted: verifies correct ordering with nested macros
  - gt_in_condition_exhaustive: tests 6 different > in condition patterns
  - body_blocks_correct_order_with_close_tags: verifies body block order

Stage Summary:
- Critical fix: extract_macros now returns blocks in source order
- Format isolation: folding_modifier_names() replaces hardcoded SugarCube-only list
- Code quality: DRY cleanup of regex definitions
- Critical fix: added `pub(crate) mod regexes;` to sugarcube/mod.rs (was missing, would cause compile error)
- 4 new regression tests for macro ordering and > in conditions
---
Task ID: 1
Agent: Main
Task: Fix deprecation warnings + link highlighting + PassageRef tokens + TextMate grammar overhaul

Work Log:
- Added `supports_full_variable_tracking()` and `supports_partial_variable_tracking()` to FormatPlugin trait with default `false` implementations
- Overrode in SugarCube (full=true), Harlowe (partial=true), Snowman (full=true), Chapbook (default false)
- Marked `StoryFormat::supports_full_variable_tracking()` and `supports_partial_variable_tracking()` as `#[deprecated]`
- Updated knot_ext.rs to use plugin methods instead of deprecated StoryFormat methods
- Updated integration_tests.rs to use plugin methods instead of deprecated StoryFormat methods
- Fixed link highlighting in tokens.rs: now only highlights the passage name, not the entire [[...]] brackets
  - [[Target]] → highlights "Target" only
  - [[Display->Target]] → highlights "Target" only
  - [[Display|Target]] → highlights "Target" only
- Added PassageRef semantic token type (index 10, mapped to LSP TYPE) for implicit/macro passage refs
- Added `script_passage_ref_tokens()` for implicit refs (Engine.play, data-passage, etc.) - highlights only the passage name string
- Added `macro_passage_ref_tokens()` for macro passage refs (<<goto "name">>, <<link "label" "name">>) - highlights only the passage name string
- Updated semantic token legend in lifecycle.rs to include index 10 (TYPE) for PassageRef
- Updated map_token_type() in semantic.rs to handle PassageRef variant
- Overhauled TextMate grammar (twee.tmLanguage.json):
  - Proper passage headers with tag and metadata capture groups
  - All 4 SugarCube comment types (/* */, /% %/, <!-- -->, and // inside script blocks via JS grammar)
  - Embedded JS regions: <<script>>...<</script>> gets full JS syntax highlighting
  - Embedded CSS regions: <<style>>...<</style>> gets full CSS syntax highlighting
  - Proper link decomposition: [[Display->Target]] shows brackets, display text, separator, and target with different scopes
  - Fixed _var pattern: now uses word boundary to avoid matching foo_bar
  - SugarCube keywords (to, is, eq, neq, gt, gte, lt, lte, and, or, not, isnot)
  - SugarCube boolean literals (true, false)
  - Implicit passage reference patterns (Engine.play, Story.get, UI.goto, data-passage) with passage name highlighted
  - SugarCube global objects (State, Engine, Story, UI, Setup, settings, etc.)
  - Macro tag decomposition: <<name args>> with separate punctuation, keyword, and parameter scopes
- Updated package.json with embeddedLanguages mapping for JS and CSS

Stage Summary:
- Build: 0 errors, 0 warnings
- Tests: 188 passed, 0 failed
- Key architectural decision: Hybrid approach for embedded language highlighting
  - TextMate grammar provides basic syntax coloring via embedded language regions
  - LSP provides Twine-specific semantic analysis (variables, passage refs, macros)
  - Future: virtual documents for full language service support in embedded regions
