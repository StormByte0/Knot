//! SugarCube macro catalog, behavioral data, and helper functions.
//!
//! Provides completion, hover, signature-help, and structural-validation data
//! for built-in SugarCube 2 macros. This is the canonical source of truth for
//! all SugarCube-specific format data within the `formats` crate.
//!
//! All items are `pub` so that the SugarCube plugin (which implements
//! `FormatPlugin`) and the LSP server handlers can both access them.

mod catalog;
mod classifiers;
mod passages;
mod globals;
mod snippets;
mod operators;
mod lookup;

// Re-export all public items to preserve the external API
pub use catalog::*;
pub use classifiers::*;
pub use passages::*;
pub use globals::*;
pub use snippets::*;
pub use operators::*;
pub use lookup::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use super::*;

    #[test]
    fn test_builtin_count() {
        // Should have at least 45+ macros (the master branch has 60+ including deprecated)
        assert!(builtin_macros().len() >= 45, "Expected at least 45 macros, got {}", builtin_macros().len());
    }

    #[test]
    fn test_block_macros() {
        let blocks = block_macro_names();
        assert!(blocks.contains("if"));
        assert!(blocks.contains("for"));
        assert!(blocks.contains("link"));
        assert!(blocks.contains("widget"));
    }

    #[test]
    fn test_passage_arg_macros() {
        let pa = passage_arg_macro_names();
        assert!(pa.contains("goto"));
        assert!(pa.contains("include"));
        assert!(pa.contains("link"));
        assert!(pa.contains("button"));
    }

    #[test]
    fn test_parent_constraints() {
        let constraints = macro_parent_constraints();
        assert_eq!(
            constraints.get("elseif").unwrap(),
            &(["if", "elseif"].into_iter().collect::<HashSet<_>>())
        );
        assert_eq!(
            constraints.get("else").unwrap(),
            &(["if"].into_iter().collect::<HashSet<_>>())
        );
        assert_eq!(
            constraints.get("break").unwrap(),
            &(["for"].into_iter().collect::<HashSet<_>>())
        );
        assert_eq!(
            constraints.get("stop").unwrap(),
            &(["timed", "repeat"].into_iter().collect::<HashSet<_>>())
        );
    }

    #[test]
    fn test_snippets() {
        assert!(macro_snippet("set").is_some());
        assert!(macro_snippet("if").is_some());
        assert!(macro_snippet("link").is_some());
        assert!(macro_snippet("goto").is_some());
        assert!(macro_snippet("nonexistent").is_none());
    }

    #[test]
    fn test_build_macro_snippet() {
        // Custom snippet
        let set_snippet = build_macro_snippet("set", false);
        assert!(set_snippet.contains("set"));

        // Generic block fallback
        let custom_block = build_macro_snippet("customblock", true);
        assert!(custom_block.contains("<</customblock"));

        // Generic inline fallback
        let custom_inline = build_macro_snippet("custominline", false);
        assert!(custom_inline.contains("custominline"));
    }

    #[test]
    fn test_global_hover() {
        assert!(global_hover_text("State").is_some());
        assert!(global_hover_text("Engine").is_some());
        assert!(global_hover_text("nonexistent").is_none());
    }

    #[test]
    fn test_variable_sigils() {
        assert_eq!(resolve_variable_sigil('$'), Some("story"));
        assert_eq!(resolve_variable_sigil('_'), Some("temporary"));
        assert_eq!(resolve_variable_sigil('%'), None);
    }

    #[test]
    fn test_find_macro() {
        assert!(find_macro("set").is_some());
        assert!(find_macro("if").is_some());
        assert!(find_macro("click").is_some());
        assert!(find_macro("click").unwrap().deprecated);
        assert!(find_macro("nonexistent").is_none());
    }

    #[test]
    fn test_passage_arg_index() {
        assert_eq!(get_passage_arg_index("goto", 1), 0);
        assert_eq!(get_passage_arg_index("link", 2), 1);  // label+passage
        assert_eq!(get_passage_arg_index("link", 1), 0);  // only passage
        assert_eq!(get_passage_arg_index("set", 1), -1);  // no passage arg
    }

    #[test]
    fn test_special_passage_names() {
        let sp = special_passage_names();
        assert!(sp.contains("StoryInit"));
        assert!(sp.contains("PassageHeader"));
        assert!(!sp.contains("Start"));
    }

    #[test]
    fn test_deprecated_macros_exist() {
        let deprecated: Vec<_> = builtin_macros()
            .iter()
            .filter(|m| m.deprecated)
            .collect();
        assert!(!deprecated.is_empty(), "Should have some deprecated macros");
        assert!(deprecated.iter().any(|m| m.name == "click"));
        assert!(deprecated.iter().any(|m| m.name == "display"));
    }

    #[test]
    fn test_structural_constraints() {
        let constraints = structural_constraints();
        assert_eq!(
            constraints.get("elseif").unwrap(),
            &(["if", "elseif"].into_iter().collect::<HashSet<_>>())
        );
        assert!(constraints.get("if").is_none()); // if has no parent constraint
    }

    #[test]
    fn test_deprecated_macros_map() {
        let deprecated = deprecated_macros();
        assert!(deprecated.contains_key("click"));
        assert!(deprecated.contains_key("display"));
        assert!(deprecated["click"].contains("<<link>>"));
    }

    #[test]
    fn test_known_macro_names() {
        let known = known_macro_names();
        assert!(known.contains("if"));
        assert!(known.contains("set"));
        assert!(known.contains("widget"));
        assert!(known.contains("audio"));
    }

    #[test]
    fn test_is_block_macro() {
        assert!(is_block_macro("if"));
        assert!(is_block_macro("for"));
        assert!(is_block_macro("link"));
        assert!(!is_block_macro("set"));
        assert!(!is_block_macro("goto"));
    }
}
