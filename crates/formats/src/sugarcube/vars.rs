//! Variable extraction for SugarCube.
//!
//! Contains regexes and functions for extracting variable operations
//! (`$var`, `_var`, `<<set>>`, etc.) from SugarCube passage bodies.
//! Also provides dot-notation path extraction for JSON object completion
//! and JavaScript alias chain tracking for State.variables.

use knot_core::passage::{VarKind, VarOp};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// $variableName — SugarCube persistent variable reference
pub(crate) static RE_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

/// _variableName — SugarCube temporary/scratch variable reference
pub(crate) static RE_TEMP_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"_([A-Za-z][A-Za-z0-9_]*)").unwrap());

/// <<set $var to ...>> — init macro for persistent vars
pub(crate) static RE_SET_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z_][A-Za-z0-9_]*)\s+to\b").unwrap());

/// <<set _var to ...>> — init macro for temporary vars
pub(crate) static RE_SET_TEMP_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+_([A-Za-z][A-Za-z0-9_]*)\s+to\b").unwrap());

/// $varname.property.path — dot-notation variable reference
pub(crate) static RE_VAR_DOT_PATH: Lazy<Regex> = Lazy::new(|| Regex::new(
    r"\$([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)"
).unwrap());

/// var/let/const x = State.variables.specificVar — JS aliasing of a specific
/// SugarCube state variable (e.g., `var gold = State.variables.gold`)
pub(crate) static RE_JS_ALIAS_SPECIFIC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// var/let/const x = State.variables — JS aliasing of the ENTIRE State.variables
/// object (e.g., `var v = State.variables; v.gold = 10;`)
pub(crate) static RE_JS_ALIAS_WHOLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\b").unwrap()
});

/// State.variables.varName = value — JS direct write to SugarCube state
pub(crate) static RE_JS_STATE_WRITE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"State\.variables\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap()
});

/// alias.property — access through a whole-object alias
/// This is detected after finding a `var x = State.variables` alias
pub(crate) static RE_ALIAS_PROPERTY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

// ---------------------------------------------------------------------------
// Variable extraction
// ---------------------------------------------------------------------------

/// Extract variable operations from a passage body.
///
/// Detects both persistent (`$var`) and temporary (`_var`) variables.
/// First detects `<<set $var …>>` / `<<set _var …>>` for inits, then all
/// `$var` / `_var` references not already captured as inits are treated
/// as reads. Temporary variables are marked with `is_temporary: true`.
///
/// Dot-notation paths like `$item.sword.damage` are also captured as
/// variable operations with the full path as the name, plus the base
/// variable name as a separate read operation.
///
/// JavaScript aliasing is tracked in two forms:
/// 1. **Specific alias**: `var x = State.variables.gold` → `$gold` read via `x`
/// 2. **Whole-object alias**: `var v = State.variables` → `$v.prop` tracks as `$prop`
pub(crate) fn extract_vars(body: &str, body_offset: usize) -> Vec<VarOp> {
    let mut vars = Vec::new();
    let mut init_spans: Vec<Range<usize>> = Vec::new();

    // Detect persistent inits via <<set $var to ...>>
    for caps in RE_SET_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_match = caps.get(1).unwrap();
        let name = format!("${}", var_match.as_str());
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_end = var_start + name.len();
        vars.push(VarOp {
            name,
            kind: VarKind::Init,
            span: var_start..var_end,
            is_temporary: false,
        });
        init_spans.push(var_start..var_end);
    }

    // Detect temporary inits via <<set _var to ...>>
    for caps in RE_SET_TEMP_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_match = caps.get(1).unwrap();
        let name = format!("_{}", var_match.as_str());
        let var_start = body_offset + m.start() + m.as_str().find('_').unwrap_or(0);
        let var_end = var_start + name.len();
        vars.push(VarOp {
            name,
            kind: VarKind::Init,
            span: var_start..var_end,
            is_temporary: true,
        });
        init_spans.push(var_start..var_end);
    }

    // Detect dot-notation variable references ($var.prop.path)
    for caps in RE_VAR_DOT_PATH.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_init {
            let name = full.as_str().to_string();
            vars.push(VarOp {
                name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    // Detect all persistent variable references ($varName) not already inits
    for caps in RE_VAR.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();
        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });

        // Skip if this match is part of a dot-notation path already captured
        let is_dot_subspan = RE_VAR_DOT_PATH
            .captures_iter(body)
            .any(|dcaps| {
                let dfull = dcaps.get(0).unwrap();
                full.start() >= dfull.start() && full.end() <= dfull.end()
            });

        if !is_init && !is_dot_subspan {
            let name = full.as_str().to_string();
            vars.push(VarOp {
                name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    // Detect all temporary variable references (_varName) not already inits
    // Filter: skip matches where the preceding character is alphanumeric (e.g., foo_bar)
    for caps in RE_TEMP_VAR.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        // Check if preceded by an alphanumeric character (part of another identifier)
        let preceded_by_alnum = full.start() > 0
            && body.as_bytes()[full.start() - 1].is_ascii_alphanumeric();

        if preceded_by_alnum {
            continue;
        }

        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_init {
            let name = full.as_str().to_string();
            vars.push(VarOp {
                name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: true,
            });
        }
    }

    // ── JavaScript alias tracking ──────────────────────────────────────

    // Track whole-object aliases: var v = State.variables
    // After finding one, any v.propertyName is treated as $propertyName
    let mut whole_aliases: HashMap<String, usize> = HashMap::new(); // alias_name → byte offset
    for caps in RE_JS_ALIAS_WHOLE.captures_iter(body) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        let full = caps.get(0).unwrap();
        let alias_offset = body_offset + full.start();

        // Check that this isn't also matched by the specific alias regex
        // (i.e., var x = State.variables.gold would match both, but the
        // specific match should take precedence)
        let is_specific = RE_JS_ALIAS_SPECIFIC.captures_iter(body)
            .any(|specific_caps| {
                specific_caps.get(0).unwrap().start() == full.start()
            });

        if !is_specific {
            whole_aliases.insert(alias_name, alias_offset);
        }
    }

    // Detect specific aliases: var x = State.variables.gold → $gold read
    for caps in RE_JS_ALIAS_SPECIFIC.captures_iter(body) {
        let _alias_name = caps.get(1).unwrap().as_str();
        let sc_var = caps.get(2).unwrap().as_str();
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        // Record the $var as a read (accessed via State.variables)
        let dollar_name = format!("${}", sc_var);
        let is_already = vars.iter().any(|v| {
            v.name == dollar_name && v.span.start == var_start
        });
        if !is_already {
            vars.push(VarOp {
                name: dollar_name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    // Resolve whole-object alias property accesses:
    // If we have `var v = State.variables` and later `v.gold` or `v.gold = 10`,
    // treat v.gold as $gold (read or write)
    if !whole_aliases.is_empty() {
        for caps in RE_ALIAS_PROPERTY.captures_iter(body) {
            let alias_name = caps.get(1).unwrap().as_str();
            let property = caps.get(2).unwrap().as_str();
            let full = caps.get(0).unwrap();

            if let Some(&alias_offset) = whole_aliases.get(alias_name) {
                // Skip the alias declaration itself (var v = State.variables)
                let prop_start = body_offset + full.start();
                if prop_start <= alias_offset {
                    continue;
                }

                let prop_end = body_offset + full.end();
                let dollar_name = format!("${}", property);

                // Determine if this is a write (look for = after the property access)
                let after_match = &body[full.end()..];
                let is_write = after_match.trim_start().starts_with('=')
                    && !after_match.trim_start().starts_with("==")
                    && !after_match.trim_start().starts_with("===");

                // Don't double-count if we already have this exact span
                let is_already = vars.iter().any(|v| {
                    v.span.start == prop_start && v.span.end == prop_end
                });

                if !is_already {
                    vars.push(VarOp {
                        name: dollar_name,
                        kind: if is_write { VarKind::Init } else { VarKind::Read },
                        span: prop_start..prop_end,
                        is_temporary: false,
                    });
                }
            }
        }
    }

    // Detect direct writes to State.variables.varName = value
    for caps in RE_JS_STATE_WRITE.captures_iter(body) {
        let sc_var = caps.get(1).unwrap().as_str();
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        let dollar_name = format!("${}", sc_var);
        let is_already_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });

        // Don't double-count if already captured via whole-alias or specific-alias
        let is_already = vars.iter().any(|v| {
            v.name == dollar_name && v.span.start == var_start
        });

        if !is_already_init && !is_already {
            vars.push(VarOp {
                name: dollar_name,
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    vars
}

// ---------------------------------------------------------------------------
// Dot-notation property map
// ---------------------------------------------------------------------------

/// Build a map of variable dot-path → set of immediate child property names.
///
/// Scans all variable operations across the workspace and builds a tree:
/// `{"item": {"sword": {}, "shield": {}}, "player": {"name": {}, "health": {}}}`
///
/// Returns a `HashMap<String, HashSet<String>>` mapping parent paths to their
/// immediate children. Used for dot-notation completion (e.g., `$item.` →
/// suggest "sword", "shield").
pub(crate) fn extract_object_property_map(
    vars_by_passage: &[Vec<&VarOp>],
) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();

    for vars in vars_by_passage {
        for var in vars {
            if var.is_temporary {
                continue;
            }

            // Only consider variables with dots in their name
            if !var.name.contains('.') {
                continue;
            }

            // Must start with $ for SugarCube
            if !var.name.starts_with('$') {
                continue;
            }

            // Split the name into path segments
            let without_sigil = &var.name[1..]; // strip $
            let segments: Vec<&str> = without_sigil.split('.').collect();

            // Build the property map by walking the path
            // For "$item.sword.damage", add:
            //   "$item" → {"sword"}
            //   "$item.sword" → {"damage"}
            for i in 0..segments.len().saturating_sub(1) {
                let parent = if i == 0 {
                    format!("${}", segments[0])
                } else {
                    format!("${}", segments[..=i].join("."))
                };
                let child = segments[i + 1].to_string();
                map.entry(parent).or_default().insert(child);
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_vars_basic() {
        let body = "<<set $gold to 10>>You have $gold coins.";
        let vars = extract_vars(body, 0);

        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn test_extract_vars_dot_notation() {
        let body = "You see $item.sword.damage.";
        let vars = extract_vars(body, 0);

        assert!(vars.iter().any(|v| v.name == "$item.sword.damage" && v.kind == VarKind::Read));
    }

    #[test]
    fn test_extract_vars_js_whole_alias() {
        let body = "var v = State.variables;\nv.gold = 10;\nvar x = v.health;";
        let vars = extract_vars(body, 0);

        // v.gold = 10 should be detected as $gold Init
        assert!(
            vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init),
            "Should detect v.gold = 10 as $gold Init"
        );
    }

    #[test]
    fn test_extract_vars_js_specific_alias() {
        let body = "var gold = State.variables.gold;";
        let vars = extract_vars(body, 0);

        // Should detect $gold as a Read
        assert!(
            vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read),
            "Should detect State.variables.gold as $gold Read"
        );
    }

    #[test]
    fn test_extract_vars_js_state_write() {
        let body = "State.variables.gold = 10;";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init),
            "Should detect State.variables.gold = as $gold Init"
        );
    }

    #[test]
    fn test_extract_object_property_map() {
        let v1 = VarOp {
            name: "$item.sword.damage".to_string(),
            kind: VarKind::Read,
            span: 0..10,
            is_temporary: false,
        };
        let v2 = VarOp {
            name: "$item.shield.defense".to_string(),
            kind: VarKind::Read,
            span: 0..10,
            is_temporary: false,
        };
        let v3 = VarOp {
            name: "$player.name".to_string(),
            kind: VarKind::Read,
            span: 0..10,
            is_temporary: false,
        };

        let vars_by_passage = vec![vec![&v1, &v2, &v3]];
        let map = extract_object_property_map(&vars_by_passage);

        assert!(map.contains_key("$item"));
        assert!(map["$item"].contains("sword"));
        assert!(map["$item"].contains("shield"));
        assert!(map.contains_key("$item.sword"));
        assert!(map["$item.sword"].contains("damage"));
        assert!(map.contains_key("$player"));
        assert!(map["$player"].contains("name"));
    }
}
