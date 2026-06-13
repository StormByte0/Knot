//! Derived classifiers — computed from the macro catalog.
//!
//! Provides classification functions that derive sets of macro names from
//! the builtin macro catalog, used for completion, validation, and structural
//! analysis.

use std::collections::{HashMap, HashSet};

use crate::types::MacroArgKind;
use super::catalog::builtin_macros;

/// Macro names that can have a body (Container macros).
///
/// Derived from the catalog's `BodyRequirement`: macros with `Required` body
/// are Container macros that always need close tags. This replaces the old
/// hardcoded list that had drifted from the catalog and incorrectly included
/// structural modifiers (`else`, `case`, `default`).
///
/// Used by folding region detection and close-tag completion.
pub fn body_macro_names() -> HashSet<&'static str> {
    builtin_macros()
        .iter()
        .filter(|m| m.body != crate::types::BodyRequirement::Never)
        .map(|m| m.name)
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
///
/// Derived from the catalog: macros whose args include at least one passage
/// reference, OR that are known to navigate dynamically even without a
/// declared passage arg (e.g., `back`, `return` which navigate via history).
pub fn dynamic_navigation_macros() -> HashSet<&'static str> {
    let mut set = builtin_macros()
        .iter()
        .filter(|m| m.args.as_ref().is_some_and(|args| args.iter().any(|a| a.is_passage_ref)))
        .map(|m| m.name)
        .collect::<HashSet<_>>();
    // back and return navigate dynamically but have no passage arg in the catalog
    set.insert("back");
    set.insert("return");
    set
}

/// Macro names whose args are always a JS expression.
///
/// These macros unconditionally have JS expression arguments — their args
/// should always be sent to oxc for analysis. Macros NOT in this set may
/// still contain JS (detected by the `$`/`_` heuristic fallback), but their
/// args aren't guaranteed to be JS expressions (e.g., `<<goto "Cave">>`
/// has a simple string arg, not a JS expression).
///
/// The list includes:
/// - Macros with declared args of kind `Expression` or `VariableRef`
/// - Control-flow macros with undeclared but always-JS args (`if`, `elseif`, etc.)
/// - Output macros whose args are expressions (`print`, `run`, etc.)
pub fn inline_js_macro_names() -> HashSet<&'static str> {
    // Start with macros that have Expression/VariableRef args in the catalog
    let mut set: HashSet<&'static str> = builtin_macros()
        .iter()
        .filter(|m| {
            m.args.as_ref().is_some_and(|args| {
                args.iter().any(|a| {
                    matches!(a.kind, MacroArgKind::Expression | MacroArgKind::Variable)
                })
            })
        })
        .map(|m| m.name)
        .collect();

    // Add control-flow macros whose args are undeclared but always JS expressions.
    // The catalog has `args: None` for these, but SugarCube always treats their
    // args as JS (conditions for if/elseif, loop spec for for, switch value, etc.)
    for name in ["if", "elseif", "else", "for", "switch", "while"] {
        set.insert(name);
    }

    set
}
