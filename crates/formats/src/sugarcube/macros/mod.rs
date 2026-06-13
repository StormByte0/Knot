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
mod completion_forms;
mod passages;
mod globals;
mod snippets;
mod operators;
mod lookup;

// Re-export all public items to preserve the external API
pub use catalog::*;
pub use classifiers::*;
pub use completion_forms::*;
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
    use crate::types::BodyRequirement;

    #[test]
    fn test_builtin_count() {
        // Should have at least 45+ macros (the master branch has 60+ including deprecated)
        assert!(builtin_macros().len() >= 45, "Expected at least 45 macros, got {}", builtin_macros().len());
    }

    #[test]
    fn test_body_macros() {
        let blocks = body_macro_names();
        assert!(blocks.contains("if"));
        assert!(blocks.contains("for"));
        assert!(blocks.contains("link"));
        assert!(blocks.contains("widget"));
        // Structural modifiers should NOT be in body_macro_names
        assert!(!blocks.contains("else"));
        assert!(!blocks.contains("elseif"));
        assert!(!blocks.contains("case"));
        assert!(!blocks.contains("default"));
        // Previously missing from the old hardcoded list
        assert!(blocks.contains("timed"));
        assert!(blocks.contains("repeat"));
        assert!(blocks.contains("css"));
        assert!(blocks.contains("createplaylist"));
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
        let set_snippet = build_macro_snippet("set", BodyRequirement::Never);
        assert!(set_snippet.contains("set"));

        // Generic block fallback
        let custom_block = build_macro_snippet("customblock", BodyRequirement::Required);
        assert!(custom_block.contains("<</customblock"));

        // Generic inline fallback
        let custom_inline = build_macro_snippet("custominline", BodyRequirement::Never);
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
    fn test_body_macro_names() {
        let blocks = body_macro_names();
        assert!(blocks.contains("if"));
        assert!(blocks.contains("for"));
        assert!(blocks.contains("link"));
        assert!(!blocks.contains("set"));
        assert!(!blocks.contains("goto"));
        // Structural modifiers are NOT body macros
        assert!(!blocks.contains("else"));
        assert!(!blocks.contains("case"));
    }

    #[test]
    fn test_body_requirement() {
        // Required: always block macros
        let if_def = builtin_macros().iter().find(|m| m.name == "if").unwrap();
        assert_eq!(if_def.body, BodyRequirement::Required);

        // Never: always inline macros
        let set_def = builtin_macros().iter().find(|m| m.name == "set").unwrap();
        assert_eq!(set_def.body, BodyRequirement::Never);

        // Optional: polymorphic macros
        let link_def = builtin_macros().iter().find(|m| m.name == "link").unwrap();
        assert_eq!(link_def.body, BodyRequirement::Optional);

        let button_def = builtin_macros().iter().find(|m| m.name == "button").unwrap();
        assert_eq!(button_def.body, BodyRequirement::Optional);
    }

    #[test]
    fn test_inline_js_macro_names() {
        let js_macros = inline_js_macro_names();
        // Control-flow macros with undeclared but always-JS args
        assert!(js_macros.contains("if"));
        assert!(js_macros.contains("elseif"));
        assert!(js_macros.contains("for"));
        assert!(js_macros.contains("switch"));
        // Macros with Expression args in the catalog
        assert!(js_macros.contains("run"));
        assert!(js_macros.contains("print"));
        assert!(js_macros.contains("set"));   // has Variable arg
        assert!(js_macros.contains("capture")); // has Variable arg
        assert!(js_macros.contains("unset"));   // has Variable arg
        // Navigation macros with passage-name args are NOT inline JS
        // (their args are just strings, not JS expressions)
        assert!(!js_macros.contains("goto"));
        assert!(!js_macros.contains("include"));
        // Widget is not JS (just a name identifier)
        assert!(!js_macros.contains("widget"));
    }

    #[test]
    fn test_dynamic_navigation_macros_derived() {
        let nav = dynamic_navigation_macros();
        // Macros with passage-ref args in the catalog
        assert!(nav.contains("goto"));
        assert!(nav.contains("include"));
        assert!(nav.contains("link"));
        assert!(nav.contains("button"));
        // back/return are added manually (no passage arg but navigate dynamically)
        assert!(nav.contains("back"));
        assert!(nav.contains("return"));
        // replace/append/prepend have selector args, not passage refs
        // (unless the catalog says otherwise)
    }

    #[test]
    fn test_completion_forms_link() {
        // <<link>> should have 5 forms
        let forms = macro_completion_forms("link");
        assert!(forms.is_some(), "link should have multi-form completions");
        let forms = forms.unwrap();
        assert_eq!(forms.len(), 5, "link should have 5 completion forms, got {}", forms.len());
        // First form should be the 2-arg navigation (most common)
        assert!(forms[0].label.contains("passage"));
        assert_eq!(forms[0].sort_priority, 0);
    }

    #[test]
    fn test_completion_forms_button() {
        // <<button>> should have 5 forms (same pattern as link)
        let forms = macro_completion_forms("button");
        assert!(forms.is_some());
        assert_eq!(forms.unwrap().len(), 5);
    }

    #[test]
    fn test_completion_forms_set() {
        // <<set>> should have multiple forms (to, ++, +=, --, -=)
        let forms = macro_completion_forms("set");
        assert!(forms.is_some());
        let forms = forms.unwrap();
        assert!(forms.len() >= 3, "set should have at least 3 forms, got {}", forms.len());
    }

    #[test]
    fn test_completion_forms_for() {
        // <<for>> should have multiple loop variants
        let forms = macro_completion_forms("for");
        assert!(forms.is_some());
        let forms = forms.unwrap();
        assert!(forms.len() >= 2, "for should have at least 2 forms, got {}", forms.len());
    }

    #[test]
    fn test_completion_forms_if() {
        // <<if>> should have multiple forms (simple, else, elseif)
        let forms = macro_completion_forms("if");
        assert!(forms.is_some());
        assert!(forms.unwrap().len() >= 2);
    }

    #[test]
    fn test_completion_forms_single_form() {
        // Macros without explicit forms should return None
        assert!(macro_completion_forms("print").is_none());
        assert!(macro_completion_forms("run").is_none());
        assert!(macro_completion_forms("unset").is_none());
        assert!(macro_completion_forms("nonexistent").is_none());
    }

    #[test]
    fn test_completion_form_snippet_conversion() {
        // Ensure snippets with \n get properly converted
        let forms = macro_completion_forms("link").unwrap();
        let block_form = &forms[1]; // "…<</link>>" form
        let converted = convert_snippet_newlines(block_form.snippet);
        assert!(converted.contains('\n'), "Block snippet should contain actual newlines after conversion");
        assert!(!converted.contains("\\n"), "Block snippet should not contain literal \\n after conversion");
    }

    #[test]
    fn test_known_macro_names_derived() {
        let known = known_macro_names();
        // Should have all catalog macros
        assert!(known.contains("if"));
        assert!(known.contains("set"));
        assert!(known.contains("widget"));
        assert!(known.contains("audio"));
        assert!(known.contains("click")); // deprecated but still known
        // Count should match catalog
        assert_eq!(known.len(), builtin_macros().len());
    }

    #[test]
    fn test_deprecated_macros_derived() {
        let deprecated = deprecated_macros();
        // Should match exactly the catalog's deprecated entries
        let catalog_deprecated: Vec<_> = builtin_macros()
            .iter()
            .filter(|m| m.deprecated)
            .collect();
        assert_eq!(deprecated.len(), catalog_deprecated.len());
        assert!(deprecated.contains_key("click"));
        assert!(deprecated.contains_key("display"));
        assert!(deprecated.contains_key("remember"));
        assert!(deprecated.contains_key("forget"));
        assert!(deprecated.contains_key("setcss"));
        assert!(deprecated.contains_key("settitle"));
        // Verify messages come from catalog
        assert!(deprecated["click"].contains("<<link>>"));
    }
}
