//! Startup alias extraction from SugarCube script passages.
//!
//! In SugarCube, JavaScript in `[script]` passages can create aliases to
//! `State.variables` or to specific state properties. These aliases persist
//! for the entire game session and are used by both script and macro passages.
//!
//! This module extracts those aliases so that downstream consumers can
//! resolve references like `v.gold` → `State.variables.gold`.

use super::helpers::{line_from_offset, strip_comments};
use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Startup alias types
// ---------------------------------------------------------------------------

/// An alias extracted from the startup script section.
///
/// In SugarCube, JavaScript in `[script]` passages can create aliases to
/// `State.variables` or to specific state properties. These aliases persist
/// for the entire game session and are used by both script and macro passages.
#[derive(Debug, Clone)]
pub struct StartupAlias {
    /// The alias identifier (e.g., `g` for `var g = gs()`).
    pub alias_name: String,
    /// What this alias resolves to.
    pub resolution: AliasResolution,
    /// The virtual line number (0-based) where this alias is defined.
    pub defined_at_line: u32,
}

/// What a startup alias resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasResolution {
    /// The alias points to the entire `State.variables` object.
    /// (e.g., `var v = State.variables` or `var g = gs()`)
    StateVariables,
    /// The alias points to a specific property of `State.variables`.
    /// (e.g., `var profiles = State.variables.uiProfiles`)
    StateVariableProperty {
        /// The base variable name without `$` sigil.
        base_name: String,
        /// Optional dot-path after the base name.
        property_path: Option<String>,
    },
    /// The alias points to a known SugarCube getter function.
    /// (e.g., `reg` from `var reg = State.variables.reg` or a custom
    /// `function(name) { return State.variables[name]; }` pattern)
    GetterFunction,
}

// ---------------------------------------------------------------------------
// Regexes for alias extraction from unified script section
// ---------------------------------------------------------------------------

/// `var/let/const x = State.variables` — whole-object alias
static RE_VD_ALIAS_WHOLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\b")
        .unwrap()
});

/// `var/let/const x = gs()` — gs() is SugarCube's getter for State.variables
static RE_VD_ALIAS_GS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*gs\s*\(\s*\)")
        .unwrap()
});

/// `var/let/const x = SugarCube.State.Variables` — full-path whole-object alias
static RE_VD_ALIAS_FULL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*SugarCube\.State\.Variables\b",
    )
    .unwrap()
});

/// `var/let/const x = State.variables.propName` — specific property alias
static RE_VD_ALIAS_SPECIFIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\.([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)",
    )
    .unwrap()
});

/// `var/let/const x = function(name) { ... State.variables ... }` — getter function
static RE_VD_ALIAS_GETTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:function\s*\([^)]*\)\s*\{[^}]*State\.variables|reg)",
    )
    .unwrap()
});

/// `function reg(name) { return State.variables[name]; }` — named getter function definition
static RE_VD_NAMED_GETTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*\{[^}]*State\.variables",
    )
    .unwrap()
});

// ---------------------------------------------------------------------------
// extract_startup_aliases()
// ---------------------------------------------------------------------------

/// Extract startup aliases from script passage source text.
///
/// Takes the concatenated script passage text directly. Strips comments
/// and then applies regex patterns to find alias definitions:
///
/// - `var v = State.variables` → StateVariables alias
/// - `var g = gs()` → StateVariables alias (gs() returns State.variables)
/// - `var x = State.variables.propName` → StateVariableProperty alias
/// - `function reg(name) { return State.variables[name]; }` → GetterFunction alias
pub fn extract_startup_aliases(script_text: &str) -> Vec<StartupAlias> {
    let mut aliases: Vec<StartupAlias> = Vec::new();

    if script_text.is_empty() {
        return aliases;
    }

    // Strip comments before analyzing
    let stripped = strip_comments(script_text);

    // Track which alias names we've already seen (avoid duplicates)
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // ── Specific property aliases: var x = State.variables.propName ────
    // Checked before whole-object aliases to avoid the whole-object regex
    // (RE_VD_ALIAS_WHOLE) greedily matching State.variables.prop as a
    // StateVariables alias (since \b matches between 'variables' and '.').
    for caps in RE_VD_ALIAS_SPECIFIC.captures_iter(&stripped) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        let prop_path = caps.get(2).unwrap().as_str().to_string();
        if seen_names.contains(&alias_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(alias_name.clone());

        // Split prop_path into base and sub-path
        let (base_name, property_path) = if let Some(dot_pos) = prop_path.find('.') {
            (prop_path[..dot_pos].to_string(), Some(prop_path[dot_pos + 1..].to_string()))
        } else {
            (prop_path.clone(), None)
        };

        aliases.push(StartupAlias {
            alias_name,
            resolution: AliasResolution::StateVariableProperty {
                base_name,
                property_path,
            },
            defined_at_line: line_no,
        });
    }

    // ── Whole-object aliases: var v = State.variables ──────────────────
    for caps in RE_VD_ALIAS_WHOLE.captures_iter(&stripped) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        if seen_names.contains(&alias_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(alias_name.clone());
        aliases.push(StartupAlias {
            alias_name,
            resolution: AliasResolution::StateVariables,
            defined_at_line: line_no,
        });
    }

    // ── gs() aliases: var g = gs() ─────────────────────────────────────
    for caps in RE_VD_ALIAS_GS.captures_iter(&stripped) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        if seen_names.contains(&alias_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(alias_name.clone());
        aliases.push(StartupAlias {
            alias_name,
            resolution: AliasResolution::StateVariables,
            defined_at_line: line_no,
        });
    }

    // ── Full-path aliases: var v = SugarCube.State.Variables ───────────
    for caps in RE_VD_ALIAS_FULL.captures_iter(&stripped) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        if seen_names.contains(&alias_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(alias_name.clone());
        aliases.push(StartupAlias {
            alias_name,
            resolution: AliasResolution::StateVariables,
            defined_at_line: line_no,
        });
    }

    // ── Named getter functions: function reg(name) { ... State.variables ... } ──
    for caps in RE_VD_NAMED_GETTER.captures_iter(&stripped) {
        let func_name = caps.get(1).unwrap().as_str().to_string();
        if seen_names.contains(&func_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(func_name.clone());
        aliases.push(StartupAlias {
            alias_name: func_name,
            resolution: AliasResolution::GetterFunction,
            defined_at_line: line_no,
        });
    }

    // ── Inline getter aliases: var reg = function(name) { ... State.variables ... } ──
    for caps in RE_VD_ALIAS_GETTER.captures_iter(&stripped) {
        let alias_name = caps.get(1).unwrap().as_str().to_string();
        if seen_names.contains(&alias_name) {
            continue;
        }
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);
        seen_names.insert(alias_name.clone());
        aliases.push(StartupAlias {
            alias_name,
            resolution: AliasResolution::GetterFunction,
            defined_at_line: line_no,
        });
    }

    aliases
}
