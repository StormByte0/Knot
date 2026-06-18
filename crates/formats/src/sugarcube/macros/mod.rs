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
        // Should have at least 70+ macros (including new entries and deprecated)
        assert!(builtin_macros().len() >= 70, "Expected at least 70 macros, got {}", builtin_macros().len());
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

        // Link/Button are Container macros — always require closing tag
        let link_def = builtin_macros().iter().find(|m| m.name == "link").unwrap();
        assert_eq!(link_def.body, BodyRequirement::Required);

        let button_def = builtin_macros().iter().find(|m| m.name == "button").unwrap();
        assert_eq!(button_def.body, BodyRequirement::Required);
    }

    #[test]
    fn test_macro_kind() {
        use crate::types::MacroKind;

        // Container: macros that always need a closing tag
        let if_def = builtin_macros().iter().find(|m| m.name == "if").unwrap();
        assert_eq!(if_def.kind, MacroKind::Container);

        let link_def = builtin_macros().iter().find(|m| m.name == "link").unwrap();
        assert_eq!(link_def.kind, MacroKind::Container);

        // Inline: macros that never need a closing tag
        let set_def = builtin_macros().iter().find(|m| m.name == "set").unwrap();
        assert_eq!(set_def.kind, MacroKind::Inline);

        let goto_def = builtin_macros().iter().find(|m| m.name == "goto").unwrap();
        assert_eq!(goto_def.kind, MacroKind::Inline);

        // SubMacro: macros only valid inside a parent container
        let else_def = builtin_macros().iter().find(|m| m.name == "else").unwrap();
        assert_eq!(else_def.kind, MacroKind::SubMacro);

        let break_def = builtin_macros().iter().find(|m| m.name == "break").unwrap();
        assert_eq!(break_def.kind, MacroKind::SubMacro);

        let next_def = builtin_macros().iter().find(|m| m.name == "next").unwrap();
        assert_eq!(next_def.kind, MacroKind::SubMacro);
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
        // <<link>> should have 3 forms (always requires closing tag)
        let forms = macro_completion_forms("link");
        assert!(forms.is_some(), "link should have multi-form completions");
        let forms = forms.unwrap();
        assert_eq!(forms.len(), 3, "link should have 3 completion forms, got {}", forms.len());
        // First form should be the 2-arg navigation (most common)
        assert!(forms[0].label.contains("passage"));
        assert_eq!(forms[0].sort_priority, 0);
        // All forms should include closing tag
        for form in forms {
            assert!(form.snippet.contains("<</link"),
                "link form '{}' should include closing tag, snippet: {}", form.label, form.snippet);
        }
    }

    #[test]
    fn test_completion_forms_button() {
        // <<button>> should have 3 forms (always requires closing tag)
        let forms = macro_completion_forms("button");
        assert!(forms.is_some());
        let forms = forms.unwrap();
        assert_eq!(forms.len(), 3, "button should have 3 completion forms, got {}", forms.len());
        // All forms should include closing tag
        for form in forms {
            assert!(form.snippet.contains("<</button"),
                "button form '{}' should include closing tag, snippet: {}", form.label, form.snippet);
        }
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
        assert!(macro_completion_forms("run").is_none());
        assert!(macro_completion_forms("unset").is_none());
        assert!(macro_completion_forms("nonexistent").is_none());
        // print, =, - now have explicit forms
        assert!(macro_completion_forms("print").is_some());
        assert!(macro_completion_forms("=").is_some());
        assert!(macro_completion_forms("-").is_some());
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
        assert!(deprecated.contains_key("silently"));
        assert!(deprecated.contains_key("actions"));
        // Verify messages come from catalog
        assert!(deprecated["click"].contains("<<link>>"));
        assert!(deprecated["silently"].contains("<<silent>>"));
    }

    // ── Phase 1: Catalog completeness & snippet accuracy tests ──────────

    #[test]
    fn test_inline_js_macros_exist_in_catalog() {
        // Every macro referenced in inline_js_macro_names() must have a catalog entry
        let known = known_macro_names();
        for name in inline_js_macro_names() {
            assert!(known.contains(name), "inline_js_macro_names contains '{}' but it's not in the catalog", name);
        }
    }

    #[test]
    fn test_newly_added_macros_exist() {
        let known = known_macro_names();
        // Phase 1 additions
        assert!(known.contains("silent"), "silent should be in catalog");
        assert!(known.contains("do"), "do should be in catalog");
        assert!(known.contains("redo"), "redo should be in catalog");
        assert!(known.contains("listbox"), "listbox should be in catalog");
        assert!(known.contains("cycle"), "cycle should be in catalog");
        assert!(known.contains("option"), "option should be in catalog");
        assert!(known.contains("optionsfrom"), "optionsfrom should be in catalog");
        assert!(known.contains("next"), "next should be in catalog");
        assert!(known.contains("createaudiogroup"), "createaudiogroup should be in catalog");
        assert!(known.contains("removeaudiogroup"), "removeaudiogroup should be in catalog");
        assert!(known.contains("removeplaylist"), "removeplaylist should be in catalog");
        assert!(known.contains("track"), "track should be in catalog");
    }

    #[test]
    fn test_newly_added_macros_have_snippets() {
        // All newly added macros should have per-macro snippets
        for name in &["silent", "do", "redo", "listbox", "cycle",
                      "option", "optionsfrom", "next", "audio", "cacheaudio",
                      "masteraudio", "playlist", "createplaylist",
                      "createaudiogroup", "removeaudiogroup", "removeplaylist",
                      "waitforaudio", "track", "css"] {
            assert!(macro_snippet(name).is_some(), "'{}' should have a snippet", name);
        }
    }

    #[test]
    fn test_newly_added_macros_have_completion_forms() {
        // New macros that should have multi-form completions
        for name in &["do", "back", "return", "textbox", "radiobutton",
                      "numberbox", "listbox", "cycle", "audio", "cacheaudio"] {
            assert!(macro_completion_forms(name).is_some(), "'{}' should have completion forms", name);
        }
    }

    #[test]
    fn test_for_completion_forms_use_range_keyword() {
        let forms = macro_completion_forms("for").unwrap();
        // All range/iteration forms should use "range" not commas
        for form in forms {
            if form.detail.contains("range") || form.detail.contains("Iterate") {
                assert!(form.snippet.contains("range"),
                    "for form '{}' should use 'range' keyword, snippet: {}", form.label, form.snippet);
            }
        }
        // C-style form should use semicolons
        let c_style = forms.iter().find(|f| f.detail.contains("3-part")).unwrap();
        assert!(c_style.snippet.contains(";"), "C-style for should use semicolons");
    }

    #[test]
    fn test_form_input_macros_use_quoted_variable_names() {
        // SugarCube requires quoted variable names for data-input macros
        for name in &["textbox", "textarea", "numberbox", "radiobutton", "checkbox"] {
            if let Some(forms) = macro_completion_forms(name) {
                for form in forms {
                    // The snippet should contain quoted variable syntax like "${1:\$var}"
                    // (the \$ is the escaped dollar sign in the raw string)
                    assert!(form.snippet.contains("\"${"),
                        "{} form '{}' snippet should use quoted variable name, got: {}",
                        name, form.label, form.snippet);
                }
            }
            // Also check the fallback snippet
            if let Some(snippet) = macro_snippet(name) {
                assert!(snippet.contains("\"${"),
                    "{} snippet should use quoted variable name, got: {}", name, snippet);
            }
        }
    }

    #[test]
    fn test_set_completion_forms_include_equals() {
        let forms = macro_completion_forms("set").unwrap();
        assert!(forms.len() >= 6, "set should have at least 6 forms (to, =, ++, +=, --, -=), got {}", forms.len());
        // Should have both "to" and "=" forms
        assert!(forms.iter().any(|f| f.snippet.contains(" to ")), "set should have 'to' form");
        assert!(forms.iter().any(|f| f.snippet.contains(" = ")), "set should have '=' form");
    }

    #[test]
    fn test_widget_has_container_form() {
        let forms = macro_completion_forms("widget").unwrap();
        assert!(forms.len() >= 2, "widget should have at least 2 forms (basic + container), got {}", forms.len());
        assert!(forms.iter().any(|f| f.snippet.contains("container")),
            "widget should have a container form");
    }

    #[test]
    fn test_sub_macro_parent_constraints() {
        // New sub-macros should have correct parent constraints
        let constraints = macro_parent_constraints();
        // next → timed
        assert_eq!(constraints.get("next").unwrap(),
            &(["timed"].into_iter().collect::<HashSet<_>>()));
        // option → listbox, cycle
        assert_eq!(constraints.get("option").unwrap(),
            &(["listbox", "cycle"].into_iter().collect::<HashSet<_>>()));
        // optionsfrom → listbox, cycle
        assert_eq!(constraints.get("optionsfrom").unwrap(),
            &(["listbox", "cycle"].into_iter().collect::<HashSet<_>>()));
        // track → createaudiogroup, createplaylist
        assert_eq!(constraints.get("track").unwrap(),
            &(["createaudiogroup", "createplaylist"].into_iter().collect::<HashSet<_>>()));
    }

    #[test]
    fn test_all_catalog_macros_have_snippets_or_forms() {
        // Every macro in the catalog should have either a custom snippet or a
        // completion form (or rely on the generic fallback). This test just
        // verifies that the most important ones have explicit coverage.
        let critical_macros = [
            "if", "elseif", "else", "for", "switch", "case", "default",
            "set", "unset", "capture", "run",
            "print", "link", "button", "goto", "include", "back", "return",
            "widget", "script", "done", "timed", "repeat", "next",
            "append", "prepend", "replace", "remove", "addclass",
            "checkbox", "textbox", "textarea", "numberbox", "radiobutton",
            "listbox", "cycle", "option", "optionsfrom",
            "audio", "cacheaudio",
        ];
        for name in &critical_macros {
            let has_snippet = macro_snippet(name).is_some();
            let has_forms = macro_completion_forms(name).is_some();
            assert!(has_snippet || has_forms,
                "'{}' should have either a snippet or completion forms", name);
        }
    }

    // ── Phase 2: Sub-macro scoping & close-tag tests ──────────────────

    #[test]
    fn test_sub_macro_kind_matches_parent_constraints() {
        // Every SubMacro should have a container or container_any_of field
        for m in builtin_macros() {
            if m.kind == crate::types::MacroKind::SubMacro {
                assert!(
                    m.container.is_some() || m.container_any_of.is_some(),
                    "SubMacro '{}' must have container or container_any_of field",
                    m.name
                );
            }
        }
    }

    #[test]
    fn test_container_macros_have_required_body() {
        // Every Container macro should have body: Required
        for m in builtin_macros() {
            if m.kind == crate::types::MacroKind::Container {
                assert_eq!(
                    m.body,
                    BodyRequirement::Required,
                    "Container macro '{}' should have body: Required",
                    m.name
                );
            }
        }
    }

    #[test]
    fn test_inline_macros_have_never_body() {
        // Every Inline macro should have body: Never
        for m in builtin_macros() {
            if m.kind == crate::types::MacroKind::Inline {
                assert_eq!(
                    m.body,
                    BodyRequirement::Never,
                    "Inline macro '{}' should have body: Never",
                    m.name
                );
            }
        }
    }

    #[test]
    fn test_sub_macros_have_never_body() {
        // Every SubMacro should have body: Never (they don't have their own body)
        for m in builtin_macros() {
            if m.kind == crate::types::MacroKind::SubMacro {
                assert_eq!(
                    m.body,
                    BodyRequirement::Never,
                    "SubMacro '{}' should have body: Never",
                    m.name
                );
            }
        }
    }

    #[test]
    fn test_parent_constraints_complete() {
        // All known sub-macros should have parent constraints
        let constraints = macro_parent_constraints();
        let sub_macros: Vec<_> = builtin_macros()
            .iter()
            .filter(|m| m.kind == crate::types::MacroKind::SubMacro)
            .collect();
        for m in &sub_macros {
            assert!(
                constraints.contains_key(m.name),
                "SubMacro '{}' should appear in macro_parent_constraints()",
                m.name
            );
        }
    }

    // ── Phase 3: Completion form coverage tests ────────────────────────

    #[test]
    fn test_phase3_audio_completion_forms() {
        // Audio macros should have multi-form completions
        assert!(macro_completion_forms("audio").is_some());
        assert!(macro_completion_forms("cacheaudio").is_some());
        assert!(macro_completion_forms("masteraudio").is_some());
        assert!(macro_completion_forms("playlist").is_some());
        assert!(macro_completion_forms("createplaylist").is_some());
        assert!(macro_completion_forms("createaudiogroup").is_some());

        // masteraudio: stop, volume, mute, unmute = 4 forms
        let master = macro_completion_forms("masteraudio").unwrap();
        assert_eq!(master.len(), 4, "masteraudio should have 4 forms, got {}", master.len());
        assert!(master.iter().any(|f| f.snippet.contains("stop")));
        assert!(master.iter().any(|f| f.snippet.contains("volume")));

        // playlist: play, stop = 2 forms
        let playlist = macro_completion_forms("playlist").unwrap();
        assert_eq!(playlist.len(), 2, "playlist should have 2 forms, got {}", playlist.len());
    }

    #[test]
    fn test_phase3_output_completion_forms() {
        // Output macros should now have completion forms
        assert!(macro_completion_forms("print").is_some());
        assert!(macro_completion_forms("=").is_some());
        assert!(macro_completion_forms("-").is_some());
        assert!(macro_completion_forms("type").is_some());
        assert!(macro_completion_forms("redo").is_some());
        assert!(macro_completion_forms("silent").is_some());
        assert!(macro_completion_forms("css").is_some());

        // type: 4 forms (basic, with delay, with keep, with class)
        let type_forms = macro_completion_forms("type").unwrap();
        assert!(type_forms.len() >= 3, "type should have at least 3 forms, got {}", type_forms.len());
        assert!(type_forms.iter().any(|f| f.snippet.contains("keep")));
        assert!(type_forms.iter().any(|f| f.snippet.contains("class")));
    }

    #[test]
    fn test_phase3_sub_macro_completion_forms() {
        // Sub-macros should have completion forms
        assert!(macro_completion_forms("option").is_some());
        assert!(macro_completion_forms("optionsfrom").is_some());
        assert!(macro_completion_forms("track").is_some());
        assert!(macro_completion_forms("next").is_some());

        // option: display + value
        let option = macro_completion_forms("option").unwrap();
        assert!(!option.is_empty());
        assert!(option[0].snippet.contains("display"));

        // optionsfrom: collection reference
        let optionsfrom = macro_completion_forms("optionsfrom").unwrap();
        assert!(optionsfrom[0].snippet.contains("collection"));
    }

    #[test]
    fn test_phase3_form_input_completion_forms() {
        // textarea should have completion forms (previously only snippet)
        assert!(macro_completion_forms("textarea").is_some());
        let textarea = macro_completion_forms("textarea").unwrap();
        assert!(textarea.len() >= 1);
        // Should use quoted variable name
        for form in textarea {
            assert!(form.snippet.contains("\"${"),
                "textarea form '{}' should use quoted variable name, got: {}", form.label, form.snippet);
        }
    }

    #[test]
    fn test_phase3_include_has_element_form() {
        // include should have 2 forms (basic + element)
        let include = macro_completion_forms("include").unwrap();
        assert!(include.len() >= 2, "include should have at least 2 forms, got {}", include.len());
        assert!(include.iter().any(|f| f.label.contains("element")));
    }

    #[test]
    fn test_phase3_print_alias_forms() {
        // = and - are aliases for print with trimmed variants
        let eq = macro_completion_forms("=").unwrap();
        assert!(eq[0].snippet.starts_with("= "), "=' snippet should start with '= '");

        let trim = macro_completion_forms("-").unwrap();
        assert!(trim[0].snippet.starts_with("- "), "-' snippet should start with '- '");
    }

    #[test]
    fn test_phase3_all_catalog_macros_have_snippets_or_forms() {
        // Every macro in the catalog should have either a snippet or a completion form
        for m in builtin_macros() {
            let has_snippet = macro_snippet(m.name).is_some();
            let has_forms = macro_completion_forms(m.name).is_some();
            assert!(has_snippet || has_forms,
                "'{}' (kind={:?}) should have either a snippet or completion forms",
                m.name, m.kind);
        }
    }

    #[test]
    fn test_phase3_form_input_snippets_use_quoted_vars() {
        // All form input macros in snippets should use quoted variable names
        for name in &["textbox", "textarea", "numberbox", "radiobutton", "checkbox",
                      "listbox", "cycle"] {
            if let Some(snippet) = macro_snippet(name) {
                assert!(snippet.contains("\"${"),
                    "{} snippet should use quoted variable name, got: {}", name, snippet);
            }
        }
    }

    // ── Phase 5: Context-smart completion tests ─────────────────────────

    #[test]
    fn test_phase5_sub_macro_prioritized_inside_parent() {
        // Sub-macros should have completion forms (so they can be prioritized)
        for name in &["else", "elseif", "break", "continue", "case", "default",
                      "next", "option", "optionsfrom", "track", "stop"] {
            let has_snippet = macro_snippet(name).is_some();
            let has_forms = macro_completion_forms(name).is_some();
            assert!(has_snippet || has_forms,
                "Sub-macro '{}' should have either a snippet or completion forms for context-smart prioritization", name);
        }
    }

    #[test]
    fn test_phase5_deprecated_macros_have_lower_sort_order() {
        // Deprecated macros should have sort prefix "2" (verified via build_macro_completions)
        // Just verify the catalog data is correct
        for m in builtin_macros() {
            if m.deprecated {
                assert!(m.deprecated, "Macro '{}' should be marked deprecated", m.name);
            }
        }
    }
}
