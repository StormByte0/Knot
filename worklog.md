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
- Full Snowman format plugin rewrite complete at crates/formats/src/snowman/mod.rs (repo-relative path)
- 20 tests all passing: parse_simple_passage, parse_variable_operations, detect_special_passages, empty_input_is_ok, expression_block_variable_read, script_block_variable_write, unescaped_expression_block, unclosed_template_diagnostic, unclosed_link_diagnostic, mixed_text_expression_script_blocks, variable_read_in_text_context, variable_write_in_script_context, multiple_variable_operations, empty_script_block, incomplete_block_from_unclosed_template, passage_header_footer_special, split_passages_byte_offset_tracking, window_story_state_alias, undefined_variable_hint, no_undefined_var_warning_when_written
- Full workspace builds cleanly with no warnings

---
Task ID: 7
Agent: main
Task: Fix variable line mapping, special-passage initializer gap, dead diagnostics code, and worklog paths

Work Log:
- Added SourceTextProvider trait to knot-formats/plugin.rs with NoSourceText default impl
- Updated FormatPlugin::build_variable_tree() signature to accept &dyn SourceTextProvider
- Implemented SourceTextProvider for HashMap<Url, String> in server/state.rs
- Rewrote compute_line_from_offset() in vars.rs to use SourceTextProvider instead of returning 0
- Updated all 5 TODO locations in vars.rs (lines 1607, 1639, 1752, 1771, 1836) with real line computation
- Added special_passage_seed_variables() method to FormatPlugin trait
- Added supplement_seed_with_format_specials() helper to server/helpers.rs
- Updated 3 call sites (knot_ext.rs debug, knot_ext.rs watch, semantic.rs) to merge format seed
- Made InitSet public in core/analysis.rs and added to core/lib.rs exports
- Removed dead build_related_information() function (pull diagnostics not used)
- Updated worklog.md to use repo-relative paths instead of absolute paths

Stage Summary:
- Variable usage locations now resolve to exact source lines instead of line 0
- Special-passage initializer gap closed by supplementing core seed with format plugin data
- Dead pull-diagnostics code removed to prevent API drift
- Worklog documentation uses repo-relative paths for portability
