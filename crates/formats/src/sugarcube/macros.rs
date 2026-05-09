//! SugarCube macro catalog, behavioral data, and helper functions.
//!
//! Provides completion, hover, signature-help, and structural-validation data
//! for built-in SugarCube 2 macros. This is the canonical source of truth for
//! all SugarCube-specific format data within the `formats` crate.
//!
//! All items are `pub` so that the SugarCube plugin (which implements
//! `FormatPlugin`) and the LSP server handlers can both access them.

use std::collections::{HashMap, HashSet};

use crate::types::{
    GlobalDef, GlobalProperty, ImplicitPassagePattern, MacroArgDef, MacroArgKind, MacroCategory, MacroDef,
    MacroSignature, OperatorNormalization, VariableSigilInfo,
};

// ---------------------------------------------------------------------------
// Built-in macro definitions — full catalog
// ---------------------------------------------------------------------------

/// The full list of SugarCube builtin macro definitions.
///
/// Contains ~50+ macro definitions covering control flow, variables, output,
/// DOM manipulation, links, forms, navigation, timing, widgets, audio, and
/// deprecated macros.
pub fn builtin_macros() -> &'static [MacroDef] {
    use MacroArgKind::*;

    static BUILTINS: &[MacroDef] = &[
        // ── Control flow ─────────────────────────────────────────────────────
        MacroDef {
            name: "if",
            description: "Conditional block. `<<if $condition>>…<</if>>`",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "elseif",
            description: "Else-if branch within `<<if>>`.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: None,
            container_any_of: Some(&["if", "elseif"]),
        },
        MacroDef {
            name: "else",
            description: "Else branch within `<<if>>`.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: Some("if"),
            container_any_of: None,
        },
        MacroDef {
            name: "for",
            description: "Iteration. `<<for _i, $arr>>…<</for>>`",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "break",
            description: "Break out of the nearest enclosing `<<for>>` loop.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: Some("for"),
            container_any_of: None,
        },
        MacroDef {
            name: "continue",
            description: "Skip to the next iteration of the nearest `<<for>>` loop.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: Some("for"),
            container_any_of: None,
        },
        MacroDef {
            name: "switch",
            description: "Switch on an expression. `<<switch $v>><<case 1>>…<</switch>>`",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "case",
            description: "Case arm within `<<switch>>`.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: Some("switch"),
            container_any_of: None,
        },
        MacroDef {
            name: "default",
            description: "Default arm within `<<switch>>`.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Control,
            container: Some("switch"),
            container_any_of: None,
        },

        // ── Variables ─────────────────────────────────────────────────────────
        MacroDef {
            name: "set",
            description: "Assign a value: `<<set $var to expression>>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: false,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "unset",
            description: "Remove a story variable: `<<unset $var>>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "variable",
                is_passage_ref: false,
                is_selector: false,
                is_variable: true,
                is_required: true,
                kind: Variable,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "capture",
            description: "Capture variables for use in closures.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "variable",
                is_passage_ref: false,
                is_selector: false,
                is_variable: true,
                is_required: true,
                kind: Variable,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "run",
            description: "Execute an expression without producing output: `<<run $arr.push(\"item\")>>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: false,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },

        // ── Output ────────────────────────────────────────────────────────────
        MacroDef {
            name: "print",
            description: "Print the result of an expression.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "=",
            description: "Short alias for `<<print>>`.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "-",
            description: "Print without leading/trailing whitespace.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "expression",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: Expression,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "type",
            description: "Typewriter effect: displays text character by character.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "speed",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "nobr",
            description: "Remove line breaks from enclosed content.",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "silently",
            description: "Execute enclosed code without producing output.",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },

        // ── DOM / Display ─────────────────────────────────────────────────────
        MacroDef {
            name: "append",
            description: "Append content to a selector: `<<append \"#id\">>…<</append>>`",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "selector",
                is_passage_ref: false,
                is_selector: true,
                is_variable: false,
                is_required: true,
                kind: Selector,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "prepend",
            description: "Prepend content to a selector.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "selector",
                is_passage_ref: false,
                is_selector: true,
                is_variable: false,
                is_required: true,
                kind: Selector,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "replace",
            description: "Replace element content.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "selector",
                is_passage_ref: false,
                is_selector: true,
                is_variable: false,
                is_required: true,
                kind: Selector,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "remove",
            description: "Remove matching element(s) from the DOM.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "selector",
                is_passage_ref: false,
                is_selector: true,
                is_variable: false,
                is_required: true,
                kind: Selector,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "copy",
            description: "Copy existing element content into another.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "selector",
                is_passage_ref: false,
                is_selector: true,
                is_variable: false,
                is_required: true,
                kind: Selector,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "addclass",
            description: "Add CSS class(es) to element(s).",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "selector",
                    is_passage_ref: false,
                    is_selector: true,
                    is_variable: false,
                    is_required: true,
                    kind: Selector,
                },
                MacroArgDef {
                    position: 1,
                    label: "class",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "removeclass",
            description: "Remove CSS class(es) from element(s).",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "selector",
                    is_passage_ref: false,
                    is_selector: true,
                    is_variable: false,
                    is_required: true,
                    kind: Selector,
                },
                MacroArgDef {
                    position: 1,
                    label: "class",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "toggleclass",
            description: "Toggle CSS class(es) on element(s).",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "selector",
                    is_passage_ref: false,
                    is_selector: true,
                    is_variable: false,
                    is_required: true,
                    kind: Selector,
                },
                MacroArgDef {
                    position: 1,
                    label: "class",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "css",
            description: "Inject inline CSS into the page.",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "script",
            description: "Execute JavaScript: `<<script>>…<</script>>`",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },

        // ── Links / Interaction ───────────────────────────────────────────────
        MacroDef {
            name: "link",
            description: "Inline link with click handler: `<<link \"label\" \"passage\">>…<</link>>`",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "button",
            description: "Button with click handler: `<<button \"label\" \"passage\">>…<</button>>`",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "linkappend",
            description: "Link that appends content when clicked: `<<linkappend \"label\">>…<</linkappend>>`",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "linkprepend",
            description: "Link that prepends content when clicked: `<<linkprepend \"label\">>…<</linkprepend>>`",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "linkreplace",
            description: "Link that replaces itself with content when clicked: `<<linkreplace \"label\">>…<</linkreplace>>`",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "actions",
            description: "Shorthand for a group of one-shot passage links.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "passage",
                is_passage_ref: true,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "click",
            description: "Alias for `<<link>>` (deprecated; prefer `<<link>>`).",
            has_body: true,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: true,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "passage",
                    is_passage_ref: true,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: true,
            deprecation_message: Some("<<click>> is deprecated. Use <<link>> instead."),
            category: MacroCategory::Links,
            container: None,
            container_any_of: None,
        },

        // ── Forms ─────────────────────────────────────────────────────────────
        MacroDef {
            name: "checkbox",
            description: "Bind a checkbox to a story variable.",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "variable",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: true,
                    is_required: false,
                    kind: Variable,
                },
                MacroArgDef {
                    position: 2,
                    label: "checked",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
                MacroArgDef {
                    position: 3,
                    label: "unchecked",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Forms,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "radiobutton",
            description: "Bind a radio button to a story variable.",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "label",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
                MacroArgDef {
                    position: 1,
                    label: "variable",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: true,
                    is_required: false,
                    kind: Variable,
                },
                MacroArgDef {
                    position: 2,
                    label: "value",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Forms,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "textarea",
            description: "Bind a `<textarea>` to a story variable.",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "variable",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: true,
                    is_required: true,
                    kind: Variable,
                },
                MacroArgDef {
                    position: 1,
                    label: "placeholder",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Forms,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "textbox",
            description: "Bind a text input to a story variable.",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "variable",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: true,
                    is_required: true,
                    kind: Variable,
                },
                MacroArgDef {
                    position: 1,
                    label: "placeholder",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: String,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Forms,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "numberbox",
            description: "Bind a numeric input to a story variable.",
            has_body: false,
            args: Some(&[
                MacroArgDef {
                    position: 0,
                    label: "variable",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: true,
                    is_required: true,
                    kind: Variable,
                },
                MacroArgDef {
                    position: 1,
                    label: "default",
                    is_passage_ref: false,
                    is_selector: false,
                    is_variable: false,
                    is_required: false,
                    kind: Expression,
                },
            ]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Forms,
            container: None,
            container_any_of: None,
        },

        // ── Navigation ────────────────────────────────────────────────────────
        MacroDef {
            name: "goto",
            description: "Navigate to a passage: `<<goto \"passage\">>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "passage",
                is_passage_ref: true,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Navigation,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "back",
            description: "Return to the previous passage.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Navigation,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "return",
            description: "Navigate using browser history.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Navigation,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "include",
            description: "Include and render another passage inline: `<<include \"passage\">>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "passage",
                is_passage_ref: true,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Navigation,
            container: None,
            container_any_of: None,
        },

        // ── Timing ────────────────────────────────────────────────────────────
        MacroDef {
            name: "timed",
            description: "Display content after a delay: `<<timed 2s>>…<</timed>>`",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "delay",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Timing,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "repeat",
            description: "Repeat content on an interval.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "interval",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Timing,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "stop",
            description: "Stop the nearest `<<timed>>` or `<<repeat>>`.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Timing,
            container: None,
            container_any_of: Some(&["timed", "repeat"]),
        },

        // ── Widgets / Audio ───────────────────────────────────────────────────
        MacroDef {
            name: "widget",
            description: "Define a reusable custom macro.",
            has_body: true,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "name",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Widgets,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "done",
            description: "Execute code after the passage is fully rendered.",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Output,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "audio",
            description: "Control audio: `<<audio \"id\" play>>`",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "id",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "playlist",
            description: "Control an audio playlist.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "masteraudio",
            description: "Control the master audio.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "createplaylist",
            description: "Define a new audio playlist.",
            has_body: true,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "cacheaudio",
            description: "Cache an audio track.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "waitforaudio",
            description: "Pause rendering until cached audio is ready.",
            has_body: false,
            args: None,
            deprecated: false,
            deprecation_message: None,
            category: MacroCategory::Audio,
            container: None,
            container_any_of: None,
        },

        // ── Deprecated macros ─────────────────────────────────────────────────
        MacroDef {
            name: "display",
            description: "Deprecated — use `<<include>>` instead.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "passageName",
                is_passage_ref: true,
                is_selector: false,
                is_variable: false,
                is_required: true,
                kind: String,
            }]),
            deprecated: true,
            deprecation_message: Some("<<display>> is deprecated. Use <<include>> instead."),
            category: MacroCategory::Navigation,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "remember",
            description: "Deprecated in SugarCube 2 — use `<<set>>` with persistent storage instead.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "$var to expr",
                is_passage_ref: false,
                is_selector: false,
                is_variable: false,
                is_required: false,
                kind: Expression,
            }]),
            deprecated: true,
            deprecation_message: Some("<<remember>> is deprecated. Use <<set>> with persistent storage instead."),
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "forget",
            description: "Deprecated in SugarCube 2 — use `<<set>>` with persistent storage instead.",
            has_body: false,
            args: Some(&[MacroArgDef {
                position: 0,
                label: "$var",
                is_passage_ref: false,
                is_selector: false,
                is_variable: true,
                is_required: true,
                kind: Variable,
            }]),
            deprecated: true,
            deprecation_message: Some("<<forget>> is deprecated. Use <<set>> with persistent storage instead."),
            category: MacroCategory::Variables,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "setcss",
            description: "Deprecated — use `<<addclass>>` or `<<removeclass>>` instead.",
            has_body: false,
            args: None,
            deprecated: true,
            deprecation_message: Some("<<setcss>> is deprecated. Use <<addclass>> or <<removeclass>> instead."),
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
        MacroDef {
            name: "settitle",
            description: "Deprecated — set `document.title` directly via `<<run>>` instead.",
            has_body: false,
            args: None,
            deprecated: true,
            deprecation_message: Some("<<settitle>> is deprecated. Set document.title directly via <<run>> instead."),
            category: MacroCategory::Dom,
            container: None,
            container_any_of: None,
        },
    ];

    BUILTINS
}

// ---------------------------------------------------------------------------
// Derived data — computed from BUILTINS
// ---------------------------------------------------------------------------

/// Block macro names (macros that have close tags and can contain children).
pub fn block_macro_names() -> HashSet<&'static str> {
    [
        "if", "elseif", "else", "for", "switch", "case", "default",
        "link", "button", "linkappend", "linkprepend", "linkreplace",
        "append", "prepend", "replace", "copy",
        "widget", "done", "nobr", "silently", "capture", "script", "type",
        "actions", "click",
    ]
    .into_iter()
    .collect()
}

/// Macros whose arguments include a passage-name reference.
pub fn passage_arg_macro_names() -> HashSet<&'static str> {
    builtin_macros()
        .iter()
        .filter(|m| m.args.as_ref().is_some_and(|args| args.iter().any(|a| a.is_passage_ref)))
        .map(|m| m.name)
        .collect()
}

/// For label+passage macros: when argCount >= 2, passage is at position 1; else 0.
pub fn label_then_passage_macros() -> HashSet<&'static str> {
    builtin_macros()
        .iter()
        .filter(|m| {
            m.args.as_ref().is_some_and(|args| {
                args.iter()
                    .any(|a| a.is_passage_ref && a.position > 0)
            })
        })
        .map(|m| m.name)
        .collect()
}

/// Macros that assign story variables.
pub fn variable_assignment_macros() -> HashSet<&'static str> {
    ["set", "capture"].into_iter().collect()
}

/// Macros that define reusable custom macros.
pub fn macro_definition_macros() -> HashSet<&'static str> {
    ["widget"].into_iter().collect()
}

/// Macros that contain inline script bodies.
pub fn inline_script_macros() -> HashSet<&'static str> {
    ["script"].into_iter().collect()
}

/// Parent constraints for structural validation — derived from BUILTINS schema.
///
/// Maps child macro name → set of valid parent macro names.
pub fn macro_parent_constraints() -> HashMap<&'static str, HashSet<&'static str>> {
    let mut map: HashMap<&'static str, HashSet<&'static str>> = HashMap::new();
    for m in builtin_macros() {
        let mut parents: Vec<&'static str> = Vec::new();
        if let Some(p) = m.container {
            parents.push(p);
        }
        if let Some(ps) = m.container_any_of {
            parents.extend_from_slice(ps);
        }
        if !parents.is_empty() {
            let set: HashSet<&'static str> = parents.into_iter().collect();
            map.insert(m.name, set);
        }
    }
    map
}

/// Macros that can navigate to a passage dynamically (variable args, runtime resolution).
pub fn dynamic_navigation_macros() -> HashSet<&'static str> {
    ["goto", "include", "link", "button", "replace", "append", "prepend"]
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// Special / System passage names
// ---------------------------------------------------------------------------

/// Special/lifecycle passage names in SugarCube.
pub fn special_passage_names() -> HashSet<&'static str> {
    [
        "StoryInit", "StoryCaption", "StoryBanner", "StorySubtitle",
        "StoryAuthor", "StoryMenu", "StoryDisplayTitle", "StoryShare",
        "StoryInterface",
        "PassageDone", "PassageHeader", "PassageFooter", "PassageReady",
    ]
    .into_iter()
    .collect()
}

/// System passages that are always reachable regardless of link structure.
pub fn system_passage_names() -> HashSet<&'static str> {
    ["StoryData", "Story JavaScript", "Story Stylesheet"]
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// Implicit passage reference patterns
// ---------------------------------------------------------------------------

/// Patterns that detect passage references in raw text / HTML / JS.
///
/// These are SugarCube-specific patterns for detecting implicit passage
/// references that are not standard `[[links]]` or `<<macro>>` passage-args,
/// such as `data-passage` attributes and `Engine.play()` calls.
pub fn implicit_passage_patterns() -> Vec<ImplicitPassagePattern> {
    vec![
        ImplicitPassagePattern {
            pattern: r#"data-passage\s*=\s*["']([^"']+)["'"#,
            description: "data-passage attribute",
        },
        ImplicitPassagePattern {
            pattern: r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["'"#,
            description: "Engine.play() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["'"#,
            description: "Engine.goto() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["'"#,
            description: "Story.get() call",
        },
        ImplicitPassagePattern {
            pattern: r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["'"#,
            description: "Story.passage() call",
        },
    ]
}

// ---------------------------------------------------------------------------
// Builtin globals
// ---------------------------------------------------------------------------

/// Built-in SugarCube global object definitions.
pub fn builtin_globals() -> &'static [GlobalDef] {
    use GlobalProperty as GP;
    static GLOBALS: &[GlobalDef] = &[
        GlobalDef {
            name: "State",
            description: "SugarCube state management API.",
            properties: Some(&[
                GP { name: "variables",    description: "Record<string, unknown> — story variables", is_method: false },
                GP { name: "temporary",    description: "Record<string, unknown> — temporary variables", is_method: false },
                GP { name: "turns",        description: "number — turn count", is_method: false },
                GP { name: "passage",      description: "string — current passage name", is_method: false },
                GP { name: "active",       description: "object — active passage info", is_method: false },
                GP { name: "top",          description: "object — top passage info", is_method: false },
                GP { name: "history",      description: "array — passage history", is_method: false },
                GP { name: "has()",        description: "boolean — check if passage visited", is_method: true },
                GP { name: "hasTag()",     description: "boolean — check if tag visited", is_method: true },
                GP { name: "index",        description: "number — current history index", is_method: false },
                GP { name: "size",         description: "number — history size", is_method: false },
            ]),
        },
        GlobalDef {
            name: "Engine",
            description: "Story engine control API.",
            properties: Some(&[
                GP { name: "play()",       description: "void — navigate to passage", is_method: true },
                GP { name: "forward()",    description: "void — go forward in history", is_method: true },
                GP { name: "backward()",   description: "void — go backward in history", is_method: true },
                GP { name: "goto()",       description: "void — navigate to passage", is_method: true },
                GP { name: "isIdle()",     description: "boolean — is engine idle", is_method: true },
                GP { name: "isPlaying()",  description: "boolean — is engine playing", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Story",
            description: "Story metadata and passage lookup API.",
            properties: Some(&[
                GP { name: "title",   description: "string — story title", is_method: false },
                GP { name: "has()",   description: "boolean — check passage exists", is_method: true },
                GP { name: "get()",   description: "object — get passage data", is_method: true },
                GP { name: "filter()", description: "array — filter passages", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Save",
            description: "Save/load API.",
            properties: Some(&[
                GP { name: "save()",    description: "void — save game", is_method: true },
                GP { name: "load()",    description: "void — load game", is_method: true },
                GP { name: "delete()",  description: "void — delete save", is_method: true },
                GP { name: "ok()",      description: "boolean — check save exists", is_method: true },
                GP { name: "sizes()",   description: "object — save sizes", is_method: true },
            ]),
        },
        GlobalDef {
            name: "Config",
            description: "Story configuration object.",
            properties: Some(&[
                GP { name: "debug",       description: "boolean — debug mode", is_method: false },
                GP { name: "history",     description: "object — history config", is_method: false },
                GP { name: "macros",      description: "object — macro config", is_method: false },
                GP { name: "navigation",  description: "object — navigation config", is_method: false },
                GP { name: "ui",          description: "object — UI config", is_method: false },
            ]),
        },
        GlobalDef {
            name: "UI",
            description: "UI utility API.",
            properties: Some(&[
                GP { name: "alert()",    description: "void — show alert dialog", is_method: true },
                GP { name: "restart()",  description: "void — restart story", is_method: true },
                GP { name: "squash()",   description: "void — squash history", is_method: true },
                GP { name: "goto()",     description: "void — navigate to passage", is_method: true },
                GP { name: "include()",  description: "void — include passage", is_method: true },
            ]),
        },
        GlobalDef { name: "Dialog",      description: "Dialog box API.", properties: None },
        GlobalDef { name: "Fullscreen",  description: "Fullscreen API.", properties: None },
        GlobalDef { name: "LoadScreen",  description: "Loading screen API.", properties: None },
        GlobalDef { name: "Macro",       description: "Macro registration API (e.g. Macro.add).", properties: None },
        GlobalDef { name: "Passage",     description: "Current passage info.", properties: None },
        GlobalDef { name: "Setting",     description: "Settings API.", properties: None },
        GlobalDef { name: "Settings",    description: "Settings object.", properties: None },
        GlobalDef { name: "SimpleAudio", description: "Simple audio API.", properties: None },
        GlobalDef { name: "Template",    description: "Template API.", properties: None },
        GlobalDef { name: "UIBar",       description: "Story navigation bar API.", properties: None },
        GlobalDef { name: "SugarCube",   description: "Global SugarCube namespace.", properties: None },
        GlobalDef { name: "setup",       description: "Author setup object for shared data.", properties: None },
        GlobalDef { name: "prehistory",  description: "Prehistory task array.", properties: None },
        GlobalDef { name: "predisplay",  description: "Predisplay task array.", properties: None },
        GlobalDef { name: "prerender",   description: "Prerender task array.", properties: None },
        GlobalDef { name: "postdisplay", description: "Postdisplay task array.", properties: None },
        GlobalDef { name: "postrender",  description: "Postrender task array.", properties: None },
    ];
    GLOBALS
}

// ---------------------------------------------------------------------------
// Snippet definitions — per-macro VS Code snippet overrides
// ---------------------------------------------------------------------------

/// A per-macro snippet body for completion.
///
/// The snippet is inserted AFTER `<<` and BEFORE the auto-closed `>>`.
///
/// Tabstop conventions:
/// - `$1`, `$2` … — positional tab stops
/// - `${1:placeholder}` — tab stop with placeholder text
/// - `$0` — final cursor position
pub fn macro_snippet(name: &str) -> Option<&'static str> {
    match name {
        // ── Variables ─────────────────────────────────────────────────────
        "set"     => Some(r#"set ${1:\$var} to ${2:value}"#),
        "unset"   => Some(r#"unset ${1:\$var}"#),
        "run"     => Some(r#"run ${1:expression}"#),
        "capture" => Some(r#"capture ${1:\$var}>>\n$2\n<</capture"#),

        // ── Output ────────────────────────────────────────────────────────
        "print"    => Some(r#"print ${1:expression}"#),
        "="        => Some(r#"= ${1:expression}"#),
        "-"        => Some(r#"- ${1:expression}"#),
        "type"     => Some(r#"type ${1:speed}>>\n$2\n<</type"#),
        "nobr"     => Some(r#"nobr>>\n$2\n<</nobr"#),
        "silently" => Some(r#"silently>>\n$2\n<</silently"#),

        // ── Control flow ──────────────────────────────────────────────────
        "if"      => Some(r#"if ${1:condition}>>\n$2\n<</if"#),
        "elseif"  => Some(r#"elseif ${1:condition}"#),
        "for"     => Some(r#"for ${1:_i}, ${2:\$array}>>\n$3\n<</for"#),
        "switch"  => Some(r#"switch ${1:\$var}>>\n<<case ${2:value}>>\n$3\n<</switch"#),

        // ── Links / interaction ───────────────────────────────────────────
        "link"        => Some(r#"link "${1:label}" "${2:passage}">>\n$3\n<</link"#),
        "button"      => Some(r#"button "${1:label}" "${2:passage}">>\n$3\n<</button"#),
        "linkappend"  => Some(r#"linkappend "${1:label}">>\n$2\n<</linkappend"#),
        "linkprepend" => Some(r#"linkprepend "${1:label}">>\n$2\n<</linkprepend"#),
        "linkreplace" => Some(r#"linkreplace "${1:label}">>\n$2\n<</linkreplace"#),

        // ── Navigation ────────────────────────────────────────────────────
        "goto"    => Some(r#"goto "${1:passage}""#),
        "include" => Some(r#"include "${1:passage}""#),
        "back"    => Some(r#"back"#),
        "return"  => Some(r#"return"#),

        // ── DOM ───────────────────────────────────────────────────────────
        "append"      => Some(r#"append "${1:#selector}">>\n$2\n<</append"#),
        "prepend"     => Some(r#"prepend "${1:#selector}">>\n$2\n<</prepend"#),
        "replace"     => Some(r#"replace "${1:#selector}">>\n$2\n<</replace"#),
        "remove"      => Some(r#"remove "${1:#selector}""#),
        "addclass"    => Some(r#"addclass "${1:#selector}" "${2:class}""#),
        "removeclass" => Some(r#"removeclass "${1:#selector}" "${2:class}""#),
        "toggleclass" => Some(r#"toggleclass "${1:#selector}" "${2:class}""#),

        // ── Widgets / scripting ───────────────────────────────────────────
        "widget" => Some(r#"widget "${1:name}">>\n$2\n<</widget"#),
        "script" => Some(r#"script>>\n$1\n<</script"#),
        "done"   => Some(r#"done>>\n$1\n<</done"#),

        // ── Timing ────────────────────────────────────────────────────────
        "timed"  => Some(r#"timed ${1:2s}>>\n$2\n<</timed"#),
        "repeat" => Some(r#"repeat ${1:2s}>>\n$2\n<</repeat"#),

        // ── Forms ─────────────────────────────────────────────────────────
        "checkbox"    => Some(r#"checkbox "${1:label}" ${2:\$var} "${3:checked}" "${4:unchecked}""#),
        "radiobutton" => Some(r#"radiobutton "${1:label}" ${2:\$var} "${3:value}""#),
        "textbox"     => Some(r#"textbox ${1:\$var} "${2:placeholder}""#),
        "textarea"    => Some(r#"textarea ${1:\$var} "${2:placeholder}""#),
        "numberbox"   => Some(r#"numberbox ${1:\$var} ${2:0}"#),

        _ => None,
    }
}

/// Build a snippet for a macro, using the per-macro override or a generic fallback.
pub fn build_macro_snippet(name: &str, has_body: bool) -> String {
    if let Some(custom) = macro_snippet(name) {
        return custom.to_string();
    }

    // Generic fallback
    let is_block = has_body || block_macro_names().contains(name);
    if is_block {
        format!("{name} $1>>\n$2\n<</{name}")
    } else {
        format!("{name} $1")
    }
}

// ---------------------------------------------------------------------------
// Variable sigil / operator data
// ---------------------------------------------------------------------------

/// SugarCube variable sigils: `$` = story (persistent), `_` = temporary.
pub fn variable_sigils() -> Vec<VariableSigilInfo> {
    vec![
        VariableSigilInfo {
            sigil: '$',
            description: "SugarCube story variable — persists across passages",
        },
        VariableSigilInfo {
            sigil: '_',
            description: "SugarCube temporary variable — scoped to the current passage",
        },
    ]
}

/// Resolve a variable sigil character to its type name.
///
/// Returns `"story"` for `$`, `"temporary"` for `_`, or `None` for unknown sigils.
pub fn resolve_variable_sigil(sigil: char) -> Option<&'static str> {
    match sigil {
        '$' => Some("story"),
        '_' => Some("temporary"),
        _ => None,
    }
}

/// Describe a variable sigil for hover documentation.
///
/// Returns a human-readable description of the sigil's meaning in SugarCube.
pub fn describe_variable_sigil(sigil: char) -> Option<&'static str> {
    match sigil {
        '$' => Some("SugarCube story variable — persists across passages"),
        '_' => Some("SugarCube temporary variable — scoped to the current passage"),
        _ => None,
    }
}

/// SugarCube operator normalization mappings (for virtual JS generation).
///
/// Maps SugarCube's English-like operators to their JavaScript equivalents.
pub fn operator_normalization() -> Vec<OperatorNormalization> {
    vec![
        OperatorNormalization { from: "to",    to: "=" },
        OperatorNormalization { from: "eq",    to: "===" },
        OperatorNormalization { from: "neq",   to: "!==" },
        OperatorNormalization { from: "is",    to: "===" },
        OperatorNormalization { from: "isnot", to: "!==" },
        OperatorNormalization { from: "gt",    to: ">" },
        OperatorNormalization { from: "gte",   to: ">=" },
        OperatorNormalization { from: "lt",    to: "<" },
        OperatorNormalization { from: "lte",   to: "<=" },
        OperatorNormalization { from: "and",   to: "&&" },
        OperatorNormalization { from: "or",    to: "||" },
        OperatorNormalization { from: "not",   to: "!" },
    ]
}

/// SugarCube operator precedence (lower number = lower precedence).
pub fn operator_precedence() -> Vec<(&'static str, u8)> {
    vec![
        ("to", 0),
        ("or", 1),
        ("and", 2),
        ("eq", 3),
        ("neq", 3),
        ("is", 3),
        ("isnot", 3),
        ("gt", 4),
        ("gte", 4),
        ("lt", 4),
        ("lte", 4),
    ]
}

/// SugarCube assignment operators.
pub fn assignment_operators() -> Vec<&'static str> {
    vec!["to", "="]
}

/// SugarCube comparison operators.
pub fn comparison_operators() -> Vec<&'static str> {
    vec!["gt", "gte", "lt", "lte"]
}

/// Script tag passage names.
pub fn script_tags() -> Vec<&'static str> {
    vec!["script"]
}

/// Stylesheet tag passage names.
pub fn stylesheet_tags() -> Vec<&'static str> {
    vec!["stylesheet", "style"]
}

// ---------------------------------------------------------------------------
// Global object hover descriptions
// ---------------------------------------------------------------------------

/// Hover text for SugarCube global objects.
///
/// Returns rich Markdown hover text for known SugarCube globals like
/// `State`, `Engine`, `Story`, `Save`, `Config`, `UI`, etc.
pub fn global_hover_text(name: &str) -> Option<&'static str> {
    match name {
        "State"      => Some("**SugarCube** `State` — the story history and variable store."),
        "Engine"     => Some("**SugarCube** `Engine` — controls passage navigation."),
        "Story"      => Some("**SugarCube** `Story` — passage access and metadata."),
        "SugarCube"  => Some("**SugarCube** version metadata object."),
        "setup"      => Some("**SugarCube** `setup` — author-defined initialisation object."),
        "passage"    => Some("**SugarCube** `passage` — title of the current passage."),
        "tags"       => Some("**SugarCube** `tags` — tag array of the current passage."),
        "visited"    => Some("**SugarCube** `visited(...passages)` — times any listed passage was visited."),
        "turns"      => Some("**SugarCube** `turns` — number of turns elapsed."),
        "time"       => Some("**SugarCube** `time` — milliseconds since last `<<timed>>` or `<<repeat>>`."),
        "$args"      => Some("**SugarCube** `$args` — arguments passed to the current `<<widget>>`."),
        "Dialog"     => Some("**SugarCube** `Dialog` — dialog box API."),
        "Fullscreen" => Some("**SugarCube** `Fullscreen` — fullscreen API."),
        "LoadScreen" => Some("**SugarCube** `LoadScreen` — loading screen API."),
        "Macro"      => Some("**SugarCube** `Macro` — macro registration API (e.g. Macro.add)."),
        "Passage"    => Some("**SugarCube** `Passage` — current passage info."),
        "Save"       => Some("**SugarCube** `Save` — save/load API."),
        "Setting"    => Some("**SugarCube** `Setting` — settings API."),
        "Settings"   => Some("**SugarCube** `Settings` — settings object."),
        "SimpleAudio"=> Some("**SugarCube** `SimpleAudio` — simple audio API."),
        "Template"   => Some("**SugarCube** `Template` — template API."),
        "UI"         => Some("**SugarCube** `UI` — UI utility API."),
        "UIBar"      => Some("**SugarCube** `UIBar` — story navigation bar API."),
        "Config"     => Some("**SugarCube** `Config` — story configuration object."),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Legacy compat: sugarcube_macro_signatures()
// ---------------------------------------------------------------------------

/// Built-in SugarCube macro signatures (legacy compat layer).
///
/// This provides the simpler `MacroSignature` view used by existing handlers.
pub fn sugarcube_macro_signatures() -> Vec<MacroSignature> {
    builtin_macros()
        .iter()
        .map(|m| {
            let signature = m
                .args
                .as_ref()
                .map(|args| {
                    args.iter()
                        .map(|a| a.label)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();

            MacroSignature {
                name: m.name,
                signature: signature.clone(),
                description: m.description,
                has_params: !signature.is_empty(),
                deprecated: m.deprecated,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

/// Find a macro definition by name.
///
/// Returns `None` if no builtin macro with the given name exists.
pub fn find_macro(name: &str) -> Option<&'static MacroDef> {
    builtin_macros().iter().find(|m| m.name == name)
}

/// Get the passage argument index for a given macro and arg count.
///
/// Returns the 0-based position of the passage-name argument, or `-1` if
/// the macro doesn't have a passage argument.
///
/// For label+passage macros (like `<<link "label" "passage">>`), when
/// `arg_count >= 2`, the passage is at position 1; otherwise at position 0.
pub fn get_passage_arg_index(macro_name: &str, arg_count: usize) -> i32 {
    if !passage_arg_macro_names().contains(macro_name) {
        return -1;
    }
    // For label+passage macros: if 2+ args, passage is at position 1; else 0
    if label_then_passage_macros().contains(macro_name) && arg_count >= 2 {
        return 1;
    }
    0
}

// ---------------------------------------------------------------------------
// Structural validation data (consolidated from sugarcube/mod.rs)
// ---------------------------------------------------------------------------

/// Structural constraints: maps child macro name → set of valid parent names.
///
/// Derived from the SugarCube macro catalog. For example:
/// - `elseif` must be inside `if` or `elseif`
/// - `else` must be inside `if`
/// - `break`/`continue` must be inside `for`
/// - `case`/`default` must be inside `switch`
/// - `stop` must be inside `timed` or `repeat`
pub fn structural_constraints() -> HashMap<&'static str, HashSet<&'static str>> {
    let mut map: HashMap<&'static str, HashSet<&'static str>> = HashMap::new();
    map.insert("elseif", ["if", "elseif"].into_iter().collect());
    map.insert("else", ["if"].into_iter().collect());
    map.insert("break", ["for"].into_iter().collect());
    map.insert("continue", ["for"].into_iter().collect());
    map.insert("case", ["switch"].into_iter().collect());
    map.insert("default", ["switch"].into_iter().collect());
    map.insert("stop", ["timed", "repeat"].into_iter().collect());
    map
}

/// Deprecated macro names and their deprecation messages.
pub fn deprecated_macros() -> HashMap<&'static str, &'static str> {
    let mut map: HashMap<&'static str, &'static str> = HashMap::new();
    map.insert("click", "<<click>> is deprecated. Use <<link>> instead.");
    map.insert("display", "<<display>> is deprecated. Use <<include>> instead.");
    map.insert("remember", "<<remember>> is deprecated. Use <<set>> with persistent storage instead.");
    map.insert("forget", "<<forget>> is deprecated. Use <<set>> with persistent storage instead.");
    map.insert("setcss", "<<setcss>> is deprecated. Use <<addclass>> or <<removeclass>> instead.");
    map.insert("settitle", "<<settitle>> is deprecated. Set document.title directly via <<run>> instead.");
    map
}

/// Known macro names (all builtins). Used for unknown-macro detection.
pub fn known_macro_names() -> HashSet<&'static str> {
    [
        // Control
        "if", "elseif", "else", "for", "break", "continue", "switch", "case", "default",
        // Variables
        "set", "unset", "capture", "run",
        // Output
        "print", "=", "-", "type", "nobr", "silently", "done",
        // DOM
        "append", "prepend", "replace", "remove", "copy",
        "addclass", "removeclass", "toggleclass", "css", "script",
        // Links
        "link", "button", "linkappend", "linkprepend", "linkreplace",
        "actions", "click",
        // Forms
        "checkbox", "radiobutton", "textarea", "textbox", "numberbox",
        // Navigation
        "goto", "back", "return", "include",
        // Timing
        "timed", "repeat", "stop",
        // Widgets
        "widget",
        // Audio
        "audio", "playlist", "masteraudio", "createplaylist", "cacheaudio", "waitforaudio",
        // Deprecated
        "display", "remember", "forget", "setcss", "settitle",
    ]
    .into_iter()
    .collect()
}

/// Check whether a macro name is a block macro (has close tags and can contain children).
pub fn is_block_macro(name: &str) -> bool {
    matches!(
        name,
        "if" | "for" | "switch"
        | "link" | "button" | "linkappend" | "linkprepend" | "linkreplace"
        | "append" | "prepend" | "replace" | "copy"
        | "widget" | "done" | "nobr" | "silently" | "capture" | "script" | "type"
        | "actions" | "click"
        | "timed" | "repeat"
        | "createplaylist" | "css"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
