//! Utility functions shared across handler submodules.
//!
//! Contains position/range helpers, diagnostic publishing, workspace indexing,
//! graph rebuild, metadata extraction, format plugin parsing, and all other
//! small helper functions that don't belong to a specific handler group.

mod code_actions;
mod compiler;
mod diagnostics;
mod formatting;
mod graph;
mod indexing;
mod navigation;
mod parsing;
mod position;
mod uri;

pub(crate) use code_actions::*;
pub(crate) use compiler::*;
pub(crate) use diagnostics::*;
pub(crate) use formatting::*;
pub(crate) use graph::*;
pub(crate) use indexing::*;
pub(crate) use navigation::*;
pub(crate) use parsing::*;
pub(crate) use position::*;
pub(crate) use uri::*;

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::graph::DiagnosticKind;
    use knot_core::passage::StoryFormat;
    use lsp_types::{DiagnosticSeverity, Position};

    // -----------------------------------------------------------------------
    // diagnostic_kind_to_severity
    // -----------------------------------------------------------------------

    #[test]
    fn test_diagnostic_severity_defaults() {
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::BrokenLink), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnreachablePassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::InfiniteLoop), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UninitializedVariable), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnusedVariable), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::RedundantWrite), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DuplicateStoryData), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStoryData), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStartPassage), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::UnsupportedFormat), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DuplicatePassageName), DiagnosticSeverity::ERROR);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::EmptyPassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::DeadEndPassage), DiagnosticSeverity::INFORMATION);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::InvalidPassageName), DiagnosticSeverity::WARNING);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::OrphanedPassage), DiagnosticSeverity::INFORMATION);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::ComplexPassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::LargePassage), DiagnosticSeverity::HINT);
        assert_eq!(diagnostic_kind_to_severity(&DiagnosticKind::MissingStartLink), DiagnosticSeverity::WARNING);
    }

    // -----------------------------------------------------------------------
    // parse_passage_name_from_header
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_passage_name_simple() {
        assert_eq!(parse_passage_name_from_header("Start"), "Start");
    }

    #[test]
    fn test_parse_passage_name_with_tags() {
        assert_eq!(parse_passage_name_from_header("Start [important]"), "Start");
    }

    #[test]
    fn test_parse_passage_name_with_leading_space() {
        assert_eq!(parse_passage_name_from_header(" Start "), "Start");
    }

    #[test]
    fn test_parse_passage_name_empty() {
        assert_eq!(parse_passage_name_from_header(""), "");
    }

    // -----------------------------------------------------------------------
    // find_passage_header_range
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_passage_header_range_found() {
        let text = ":: Start\nHello world\n:: End\nGoodbye";
        let range = find_passage_header_range(text, "Start");
        assert_eq!(range.start.line, 0);
    }

    #[test]
    fn test_find_passage_header_range_not_found() {
        let text = ":: Start\nHello world";
        let range = find_passage_header_range(text, "NonExistent");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.end.line, 0);
    }

    // -----------------------------------------------------------------------
    // find_passage_at_position
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_passage_at_position() {
        let text = ":: Start\nHello\n:: Middle\nWorld\n:: End\nBye";
        // Line 0 is passage header "Start" — returns the passage name
        assert_eq!(
            find_passage_at_position(text, Position { line: 0, character: 3 }),
            Some("Start".to_string())
        );
        // Line 1 is body (not a :: header) — returns None
        assert_eq!(
            find_passage_at_position(text, Position { line: 1, character: 0 }),
            None
        );
        // Line 2 is passage header "Middle"
        assert_eq!(
            find_passage_at_position(text, Position { line: 2, character: 3 }),
            Some("Middle".to_string())
        );
    }

    #[test]
    fn test_find_passage_at_position_no_passage() {
        let text = "Just some text without passage headers";
        assert_eq!(
            find_passage_at_position(text, Position { line: 0, character: 0 }),
            None
        );
    }

    // -----------------------------------------------------------------------
    // find_link_target_at_position
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_link_target_simple() {
        let text = ":: Start\nGo to [[Castle]] for adventure";
        // "Castle" link is at approximately character 6 on line 1
        let result = find_link_target_at_position(text, Position { line: 1, character: 10 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    #[test]
    fn test_find_link_target_arrow() {
        let text = ":: Start\n[[Go to Castle->Castle]]";
        let result = find_link_target_at_position(text, Position { line: 1, character: 5 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    #[test]
    fn test_find_link_target_pipe() {
        let text = ":: Start\n[[Visit|Castle]]";
        let result = find_link_target_at_position(text, Position { line: 1, character: 5 });
        assert_eq!(result, Some("Castle".to_string()));
    }

    // -----------------------------------------------------------------------
    // format_twee_text
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_trailing_whitespace() {
        let text = ":: Start   \nHello  ";
        let edits = format_twee_text(text);
        // Should have edits to trim trailing whitespace
        assert!(!edits.is_empty());
    }

    #[test]
    fn test_format_already_clean() {
        let text = ":: Start\nHello\n\n:: End\nGoodbye\n";
        let _edits = format_twee_text(text);
        // Already clean — may or may not have edits (depends on blank line logic)
        // Just ensure it doesn't panic
    }

    // -----------------------------------------------------------------------
    // extract_quoted_name / extract_passage_from_diag / extract_variable_name
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_quoted_name() {
        assert_eq!(extract_quoted_name("Link to 'Castle' not found"), Some("Castle".to_string()));
        assert_eq!(extract_quoted_name("Link to \"Castle\" not found"), Some("Castle".to_string()));
        assert_eq!(extract_quoted_name("No quotes here"), None);
    }

    #[test]
    fn test_extract_passage_from_diag() {
        assert_eq!(
            extract_passage_from_diag("Broken link to passage 'Forest'"),
            Some("Forest".to_string())
        );
        assert_eq!(
            extract_passage_from_diag("Passage 'Start' is unreachable"),
            Some("Start".to_string())
        );
    }

    #[test]
    fn test_extract_variable_name() {
        // $varname without quotes
        assert_eq!(
            extract_variable_name("Variable $gold may be used before initialization"),
            Some("$gold".to_string())
        );
        assert_eq!(
            extract_variable_name("No variable mentioned"),
            None
        );
    }

    // -----------------------------------------------------------------------
    // parse_story_data_json
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_story_data_valid() {
        let json = r#"{ "ifid": "A1B2C3", "format": "SugarCube", "start": "Prologue" }"#;
        let meta = parse_story_data_json(json);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.start_passage, "Prologue");
    }

    #[test]
    fn test_parse_story_data_invalid() {
        let meta = parse_story_data_json("not json at all");
        assert!(meta.is_none());
    }

    #[test]
    fn test_parse_story_data_missing_fields() {
        let json = r#"{ "ifid": "A1B2C3" }"#;
        let meta = parse_story_data_json(json);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        // Default values
        assert_eq!(meta.format, StoryFormat::SugarCube);
        assert_eq!(meta.start_passage, "Start");
    }

    // -----------------------------------------------------------------------
    // SugarCube macro catalog via format plugin
    // -----------------------------------------------------------------------

    #[test]
    fn test_sugarcube_builtin_macros_nonempty() {
        use knot_core::passage::StoryFormat;

        let registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let plugin = registry.get(&StoryFormat::SugarCube).expect("SugarCube plugin");
        let macros = plugin.builtin_macros();
        assert!(!macros.is_empty(), "SugarCube plugin should have builtin macros");
        // Spot-check a few well-known macros
        assert!(macros.iter().any(|m| m.name == "set"), "should have <<set>>");
        assert!(macros.iter().any(|m| m.name == "if"), "should have <<if>>");
        assert!(macros.iter().any(|m| m.name == "goto"), "should have <<goto>>");
    }

    #[test]
    fn test_macro_find_and_snippet() {
        use knot_core::passage::StoryFormat;

        let registry = knot_formats::plugin::FormatRegistry::with_defaults();
        let plugin = registry.get(&StoryFormat::SugarCube).expect("SugarCube plugin");

        let set_macro = plugin.find_macro("set").expect("should find <<set>>");
        assert!(!set_macro.args.is_none() || set_macro.args.as_ref().map(|a| a.is_empty()).unwrap_or(true) == false,
            "<<set>> should have args");

        let else_macro = plugin.find_macro("else").expect("should find <<else>>");
        // <<else>> is a bare macro with no arguments
        assert!(else_macro.args.is_none() || else_macro.args.as_ref().map(|a| a.is_empty()).unwrap_or(true),
            "<<else>> should have no args");
    }

    // -----------------------------------------------------------------------
    // byte_offset_to_position / byte_range_to_lsp_range
    // -----------------------------------------------------------------------

    #[test]
    fn test_byte_offset_to_position() {
        let text = "line one\nline two\nline three";
        assert_eq!(byte_offset_to_position(text, 0).line, 0);
        assert_eq!(byte_offset_to_position(text, 0).character, 0);
        // "line one\n" = 9 bytes, so offset 9 is start of line 1
        assert_eq!(byte_offset_to_position(text, 9).line, 1);
        assert_eq!(byte_offset_to_position(text, 9).character, 0);
    }

    // -----------------------------------------------------------------------
    // DebounceTimer
    // -----------------------------------------------------------------------

    #[test]
    fn test_debounce_timer_starts_ready() {
        use knot_core::editing::DebounceTimer;
        let timer = DebounceTimer::new();
        assert!(timer.is_ready());
        assert!(!timer.is_pending());
    }

    #[test]
    fn test_debounce_timer_pending_after_edit() {
        use knot_core::editing::DebounceTimer;
        let mut timer = DebounceTimer::new();
        timer.record_edit();
        assert!(timer.is_pending());
    }
}
