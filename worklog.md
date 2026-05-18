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
