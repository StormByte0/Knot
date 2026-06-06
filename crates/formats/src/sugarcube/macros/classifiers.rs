//! Derived classifiers — computed from the macro catalog.
//!
//! Provides classification functions that derive sets of macro names from
//! the builtin macro catalog, used for completion, validation, and structural
//! analysis.

use std::collections::{HashMap, HashSet};

use super::catalog::builtin_macros;

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

/// Block macro names that are structural modifiers (no close tag of their own).
///
/// These are part of a parent block and are folded together with it.
/// They should NOT be pushed onto the folding-range stack.
pub fn folding_modifier_names() -> HashSet<&'static str> {
    ["else", "elseif", "case", "default"].into_iter().collect()
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
    ["goto", "include", "link", "button", "replace", "append", "prepend", "return", "back"]
        .into_iter()
        .collect()
}
