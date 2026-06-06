//! Lookup helpers, signature generation, and structural validation data.
//!
//! Provides functions for looking up macro definitions, generating signatures,
//! finding passage argument indices, and structural validation data.
//! Depends on the catalog and classifiers modules.

use std::collections::{HashMap, HashSet};

use crate::types::{MacroDef, MacroSignature};

use super::catalog::builtin_macros;
use super::classifiers::{label_then_passage_macros, passage_arg_macro_names};

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
