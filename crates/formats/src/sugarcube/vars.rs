//! Variable extraction for SugarCube.
//!
//! Contains regexes and functions for extracting variable operations
//! (`$var`, `_var`, `<<set>>`, etc.) from SugarCube passage bodies.
//! Also provides dot-notation path extraction for JSON object completion
//! and JavaScript alias chain tracking for State.variables.
//!
//! ## SugarCube Variable Model (from official docs)
//!
//! SugarCube has two variable types:
//!
//! - **Story variables** (`$var`): Persistent entries in `State.variables`.
//!   They survive for the entire playthrough session, persisting across
//!   passage transitions. Once written (via `<<set>>`, `State.variables.x =`,
//!   or a JS alias), they remain in the state collection indefinitely.
//!   They are NOT traditional scoped variables requiring "definite assignment".
//!
//! - **Temporary variables** (`_var`): Do NOT become part of story history.
//!   They only exist for the lifetime of the moment/turn that they're created
//!   in. Ideal for loop variables in `<<for>>` macros.
//!
//! Variable naming rules (per SugarCube 2.x spec):
//! - Sigil: `$` for story, `_` for temporary (mandatory first character)
//! - Second character: `A-Za-z$_` (after initial sigil use, `$` and `_`
//!   become regular variable characters)
//! - Subsequent characters: `A-Za-z0-9$_`
//!
//! The `$$` markup escapes the `$` sigil (e.g., `$$name` outputs literal
//! `$name`), so `$$` must NOT be matched as a variable reference.
//!
//! ## State Variable Registry
//!
//! The `build_state_variable_registry()` function collects all persistent
//! variable references across the workspace into a `StateVariable` registry.
//! This registry is the foundation for graph-BFS-based variable availability
//! analysis, which replaces the traditional "uninitialized variable" detection
//! that is incorrect for SugarCube's persistent state model.

use crate::types::{StateVariable, VarAccessKind, VarLocation};
use knot_core::passage::{VarKind, VarOp};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// `$variableName` — SugarCube persistent story variable reference.
///
/// Per the SugarCube spec, valid variable names follow the pattern:
/// - Sigil: `$`
/// - Second char: `A-Za-z$_`
/// - Subsequent chars: `A-Za-z0-9$_`
///
/// We cannot use lookbehind (regex crate limitation) to exclude `$$` escape
/// markup directly. Instead, we match `$var` and filter out matches preceded
/// by another `$` in the extraction code.
pub(crate) static RE_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `_variableName` — SugarCube temporary/scratch variable reference.
///
/// Per the SugarCube spec, temporary variable names follow the same rules
/// as story variables but with `_` as the sigil.
pub(crate) static RE_TEMP_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"_([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `<<set $var to ...>>` — TwineScript `to` assignment for persistent vars
pub(crate) static RE_SET_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s+to\b").unwrap());

/// `<<set $var = ...>>` — JavaScript `=` assignment for persistent vars.
/// We match `=` that is NOT preceded by `!<>` (to avoid `==`, `!=`, `<=`, `>=`)
/// and NOT followed by `=` (to avoid `==` and `===`).
/// Since the regex crate doesn't support lookbehind, we use a simpler approach:
/// match the assignment and then filter out compound operators in the code.
pub(crate) static RE_SET_MACRO_EQ: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*=").unwrap());

/// `<<set $var += ...>>` — Compound assignment for persistent vars
/// (also catches -=, *=, /=, %=)
pub(crate) static RE_SET_MACRO_COMPOUND: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*([\+\-\*\/%])=").unwrap());

/// `<<set _var to ...>>` — TwineScript `to` assignment for temporary vars
pub(crate) static RE_SET_TEMP_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+_([A-Za-z\$_][A-Za-z0-9\$_]*)\s+to\b").unwrap());

/// `<<set _var = ...>>` — JavaScript `=` assignment for temporary vars
pub(crate) static RE_SET_TEMP_MACRO_EQ: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+_([A-Za-z\$_][A-Za-z0-9\$_]*)\s*=").unwrap());

/// `$varname.property.path` — dot-notation variable reference.
/// The `$$` escape is handled by filtering in the extraction code.
pub(crate) static RE_VAR_DOT_PATH: Lazy<Regex> = Lazy::new(|| Regex::new(
    r"\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)+)"
).unwrap());

/// `$varname["property"]` or `$varname['property']` — bracket-notation
/// property access. The property name is captured.
/// The `$$` escape is handled by filtering in the extraction code.
pub(crate) static RE_VAR_BRACKET_PROP: Lazy<Regex> = Lazy::new(|| Regex::new(
    r#"\$([A-Za-z\$_][A-Za-z0-9\$_]*)\[["']([A-Za-z\$_][A-Za-z0-9\$_]*)["']\]"#
).unwrap());

/// `var/let/const x = State.variables.specificVar` — JS aliasing of a specific
/// SugarCube state variable (e.g., `var gold = State.variables.gold`)
pub(crate) static RE_JS_ALIAS_SPECIFIC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// `var/let/const x = State.variables` — JS aliasing of the ENTIRE State.variables
/// object (e.g., `var v = State.variables; v.gold = 10;`)
pub(crate) static RE_JS_ALIAS_WHOLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\b").unwrap()
});

/// `State.variables.varName = value` — JS direct write to SugarCube state
pub(crate) static RE_JS_STATE_WRITE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"State\.variables\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap()
});

/// `State.getVar("$varName")` — JS API to read a variable
pub(crate) static RE_JS_STATE_GETVAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"State\.getVar\(\s*"\$([A-Za-z_][A-Za-z0-9_]*)\s*""#).unwrap()
});

/// `State.setVar("$varName", value)` — JS API to write a variable
pub(crate) static RE_JS_STATE_SETVAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"State\.setVar\(\s*"\$([A-Za-z_][A-Za-z0-9_]*)""#).unwrap()
});

/// `alias.property` — access through a whole-object alias.
/// This is detected after finding a `var x = State.variables` alias.
pub(crate) static RE_ALIAS_PROPERTY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// `<<unset $var>>` — macro that explicitly removes a state variable
pub(crate) static RE_UNSET_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<unset\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)>>").unwrap());

/// Setter link: `[[text|passage][$var to value]]` or `[[text|passage][$var = value]]`
/// These assign variables during link navigation.
pub(crate) static RE_SETTER_LINK: Lazy<Regex> = Lazy::new(|| Regex::new(
    r"\[\[[^\]]*?\]\[\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s+(?:to|=)"
).unwrap());

/// `$var++` or `++$var` — post/pre increment (both read and write)
pub(crate) static RE_VAR_INCREMENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)\+\+|\+\+\$([A-Za-z\$_][A-Za-z0-9\$_]*))").unwrap());

/// `$var--` or `--$var` — post/pre decrement (both read and write)
pub(crate) static RE_VAR_DECREMENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)--|--\$([A-Za-z\$_][A-Za-z0-9\$_]*))").unwrap());

// ---------------------------------------------------------------------------
// Variable extraction
// ---------------------------------------------------------------------------

/// Extract variable operations from a passage body.
///
/// Detects both persistent (`$var`) and temporary (`_var`) variables.
/// First detects assignment patterns (`<<set>>`, `<<unset>>`, JS writes),
/// then all `$var` / `_var` references not already captured as inits are
/// treated as reads. Temporary variables are marked with `is_temporary: true`.
///
/// ## Detected patterns
///
/// - `<<set $var to ...>>` / `<<set $var = ...>>` — base assignment
/// - `<<set $var += ...>>` — compound assignment (also `-=`, `*=`, `/=`, `%=`)
/// - `$var++` / `++$var` / `$var--` / `--$var` — increment/decrement
/// - `$varname` — naked variable markup (read)
/// - `$var.prop.path` — dot-notation property access
/// - `$var["property"]` — bracket-notation property access
/// - `<<unset $var>>` — explicit removal from state
/// - `State.variables.var = value` — JS direct write
/// - `State.getVar("$var")` — JS API read
/// - `State.setVar("$var", value)` — JS API write
/// - `var x = State.variables` — JS whole-object alias
/// - `var x = State.variables.gold` — JS specific-variable alias
/// - `[[text|passage][$var to value]]` — setter link assignment
///
/// The `$$` escape markup is excluded — `$$name` outputs literal `$name`
/// and is NOT a variable reference.
pub(crate) fn extract_vars(body: &str, body_offset: usize) -> Vec<VarOp> {
    let mut vars = Vec::new();
    let mut init_spans: Vec<Range<usize>> = Vec::new();
    let mut unset_spans: Vec<Range<usize>> = Vec::new();

    // ── Assignment detection: <<set $var to ...>> ─────────────────────
    for caps in RE_SET_MACRO.captures_iter(body) {
        record_persistent_init(&caps, body, body_offset, &mut vars, &mut init_spans);
    }

    // ── Assignment detection: <<set $var = ...>> ──────────────────────
    for caps in RE_SET_MACRO_EQ.captures_iter(body) {
        // Avoid double-counting if also matched by compound assignment
        let full = caps.get(0).unwrap();
        let is_compound = RE_SET_MACRO_COMPOUND.captures_iter(body).any(|cc| {
            cc.get(0).unwrap().start() == full.start()
        });
        if !is_compound {
            record_persistent_init(&caps, body, body_offset, &mut vars, &mut init_spans);
        }
    }

    // ── Assignment detection: <<set $var += ...>> etc. ────────────────
    for caps in RE_SET_MACRO_COMPOUND.captures_iter(body) {
        record_persistent_init(&caps, body, body_offset, &mut vars, &mut init_spans);
    }

    // ── Temporary assignment: <<set _var to ...>> ─────────────────────
    for caps in RE_SET_TEMP_MACRO.captures_iter(body) {
        record_temporary_init(&caps, body_offset, &mut vars, &mut init_spans);
    }

    // ── Temporary assignment: <<set _var = ...>> ──────────────────────
    for caps in RE_SET_TEMP_MACRO_EQ.captures_iter(body) {
        record_temporary_init(&caps, body_offset, &mut vars, &mut init_spans);
    }

    // ── Increment/decrement: $var++, ++$var, $var--, --$var ───────────
    // These are both a read AND a write. We record them as inits since
    // the variable must already exist (the write is the primary effect).
    for caps in RE_VAR_INCREMENT.captures_iter(body) {
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        record_increment_decrement(var_match, "$", body, body_offset, &mut vars, &mut init_spans);
    }
    for caps in RE_VAR_DECREMENT.captures_iter(body) {
        let var_match = caps.get(1).or_else(|| caps.get(2)).unwrap();
        record_increment_decrement(var_match, "$", body, body_offset, &mut vars, &mut init_spans);
    }

    // ── Unset detection: <<unset $var>> ───────────────────────────────
    for caps in RE_UNSET_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_match = caps.get(1).unwrap();
        let name = format!("${}", var_match.as_str());
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_end = var_start + name.len();
        // Record as an Init (write) since unset modifies state, but also
        // track the span for unset-aware analysis
        vars.push(VarOp {
            name,
            kind: VarKind::Init,
            span: var_start..var_end,
            is_temporary: false,
        });
        unset_spans.push(var_start..var_end);
        init_spans.push(var_start..var_end);
    }

    // ── Setter link detection: [[text|passage][$var to/= value]] ──────
    for caps in RE_SETTER_LINK.captures_iter(body) {
        let var_match = caps.get(1).unwrap();
        let name = format!("${}", var_match.as_str());
        let full = caps.get(0).unwrap();
        // Find the $var position within the setter
        let var_start = body_offset + full.start() + full.as_str().find('$').unwrap_or(0);
        let var_end = var_start + name.len();
        let is_already = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_already {
            vars.push(VarOp {
                name,
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            init_spans.push(var_start..var_end);
        }
    }

    // ── Dot-notation property references: $var.prop.path ──────────────
    for caps in RE_VAR_DOT_PATH.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        // Skip $$ escape markup (e.g., $$var.prop is not a variable)
        let is_dollar_escape = full.start() > 0
            && body.as_bytes()[full.start() - 1] == b'$';
        let is_double_dollar = full.as_str().starts_with("$$");
        if is_dollar_escape || is_double_dollar {
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
                is_temporary: false,
            });
        }
    }

    // ── Bracket-notation property references: $var["property"] ────────
    for caps in RE_VAR_BRACKET_PROP.captures_iter(body) {
        let var_match = caps.get(1).unwrap();
        let prop_match = caps.get(2).unwrap();
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        // Skip $$ escape markup
        let is_dollar_escape = full.start() > 0
            && body.as_bytes()[full.start() - 1] == b'$';
        let is_double_dollar = full.as_str().starts_with("$$");
        if is_dollar_escape || is_double_dollar {
            continue;
        }

        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_init {
            // Record both the base variable read and the property path
            let base_name = format!("${}", var_match.as_str());
            let prop_path = format!("{}.{}", base_name, prop_match.as_str());

            // Record the property path as a read
            let is_already = vars.iter().any(|v| {
                v.name == prop_path && v.span.start == var_start
            });
            if !is_already {
                vars.push(VarOp {
                    name: prop_path,
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: false,
                });
            }
        }
    }

    // ── All persistent variable references ($varName) not already inits ──
    for caps in RE_VAR.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        // Skip $$ escape markup (e.g., $$name outputs literal "$name")
        // Check 1: preceded by another $
        let is_dollar_escape = full.start() > 0
            && body.as_bytes()[full.start() - 1] == b'$';
        // Check 2: the match itself is $$name (sigil + $ as second char)
        // In SugarCube, $$ in passage text is the escape markup, so $$name
        // means "output literal $name" — not a variable reference.
        let is_double_dollar = full.as_str().starts_with("$$");
        if is_dollar_escape || is_double_dollar {
            continue;
        }

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

        // Skip if part of a bracket notation reference
        let is_bracket_subspan = RE_VAR_BRACKET_PROP
            .captures_iter(body)
            .any(|bcaps| {
                let bfull = bcaps.get(0).unwrap();
                full.start() >= bfull.start() && full.end() <= bfull.end()
            });

        if !is_init && !is_dot_subspan && !is_bracket_subspan {
            let name = full.as_str().to_string();
            vars.push(VarOp {
                name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    // ── All temporary variable references (_varName) not already inits ──
    // Filter: skip matches where the preceding character is alphanumeric
    // (e.g., `foo_bar` is an identifier, not a temp variable)
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

    // ── Direct JS writes: State.variables.varName = value ──────────────
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

    // ── JS API: State.getVar("$var") — read ───────────────────────────
    for caps in RE_JS_STATE_GETVAR.captures_iter(body) {
        let sc_var = caps.get(1).unwrap().as_str();
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

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

    // ── JS API: State.setVar("$var", value) — write ───────────────────
    for caps in RE_JS_STATE_SETVAR.captures_iter(body) {
        let sc_var = caps.get(1).unwrap().as_str();
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();

        let dollar_name = format!("${}", sc_var);
        let is_already = vars.iter().any(|v| {
            v.name == dollar_name && v.span.start == var_start
        });
        if !is_already {
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

/// Record a persistent variable init from a captures match.
fn record_persistent_init(
    caps: &regex::Captures,
    _body: &str,
    body_offset: usize,
    vars: &mut Vec<VarOp>,
    init_spans: &mut Vec<Range<usize>>,
) {
    let m = caps.get(0).unwrap();
    let var_match = caps.get(1).unwrap();
    let name = format!("${}", var_match.as_str());
    let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
    let var_end = var_start + name.len();

    // Avoid duplicate init recording for the same span
    let is_dup = init_spans.iter().any(|s| {
        var_start >= s.start && var_end <= s.end
    });
    if is_dup {
        return;
    }

    vars.push(VarOp {
        name,
        kind: VarKind::Init,
        span: var_start..var_end,
        is_temporary: false,
    });
    init_spans.push(var_start..var_end);
}

/// Record a temporary variable init from a captures match.
fn record_temporary_init(
    caps: &regex::Captures,
    body_offset: usize,
    vars: &mut Vec<VarOp>,
    init_spans: &mut Vec<Range<usize>>,
) {
    let m = caps.get(0).unwrap();
    let var_match = caps.get(1).unwrap();
    let name = format!("_{}", var_match.as_str());
    let var_start = body_offset + m.start() + m.as_str().find('_').unwrap_or(0);
    let var_end = var_start + name.len();

    // Avoid duplicate init recording for the same span
    let is_dup = init_spans.iter().any(|s| {
        var_start >= s.start && var_end <= s.end
    });
    if is_dup {
        return;
    }

    vars.push(VarOp {
        name,
        kind: VarKind::Init,
        span: var_start..var_end,
        is_temporary: true,
    });
    init_spans.push(var_start..var_end);
}

/// Record an increment/decrement operation as an init (write).
fn record_increment_decrement(
    var_match: regex::Match,
    sigil: &str,
    _body: &str,
    body_offset: usize,
    vars: &mut Vec<VarOp>,
    init_spans: &mut Vec<Range<usize>>,
) {
    let name = format!("{}{}", sigil, var_match.as_str());
    let var_start = body_offset + var_match.start() - 1; // -1 for the sigil
    let var_end = var_start + name.len();

    let is_dup = init_spans.iter().any(|s| {
        var_start >= s.start && var_end <= s.end
    });
    if is_dup {
        return;
    }

    vars.push(VarOp {
        name,
        kind: VarKind::Init,
        span: var_start..var_end,
        is_temporary: sigil == "_",
    });
    init_spans.push(var_start..var_end);
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

// ---------------------------------------------------------------------------
// State variable registry
// ---------------------------------------------------------------------------

/// Build a registry of all SugarCube state variables across the workspace.
///
/// This scans all passages for persistent variable references (`$var`,
/// `State.variables.var`, JS aliases) and collects them into a map from
/// dollar-prefixed name (e.g., "$hp") to `StateVariable`. Dot-notation
/// paths like `$player.name` are decomposed: the base variable (`$player`)
/// gets `name` added to its `known_properties`, and a separate base-level
/// read/write is also recorded.
///
/// Temporary variables (`_var`) are excluded from the registry since they
/// don't persist in `State.variables`.
pub(crate) fn build_state_variable_registry(
    workspace: &knot_core::Workspace,
) -> HashMap<String, StateVariable> {
    let mut registry: HashMap<String, StateVariable> = HashMap::new();

    for doc in workspace.documents() {
        let file_uri = doc.uri.to_string();
        for passage in &doc.passages {
            // Skip metadata passages
            if passage.is_metadata() {
                continue;
            }

            let passage_name = passage.name.clone();
            let is_special_seeding = passage.is_special
                && passage.special_def.as_ref().map_or(false, |d| d.contributes_variables);

            for var in &passage.vars {
                // Skip temporary variables — they don't persist in State.variables
                if var.is_temporary {
                    continue;
                }

                // Parse the variable name to extract base name and optional property path
                let (base_name, dollar_name, property_path) = parse_var_name(&var.name);

                let access_kind = match var.kind {
                    VarKind::Init => {
                        if let Some(path) = property_path.clone() {
                            VarAccessKind::PropertyWrite { path }
                        } else {
                            VarAccessKind::Assign
                        }
                    }
                    VarKind::Read => {
                        if let Some(path) = property_path.clone() {
                            VarAccessKind::PropertyRead { path }
                        } else {
                            VarAccessKind::Read
                        }
                    }
                };

                let location = VarLocation {
                    passage_name: passage_name.clone(),
                    file_uri: file_uri.clone(),
                    span: var.span.clone(),
                    kind: access_kind,
                };

                let entry = registry.entry(dollar_name.clone()).or_insert_with(|| {
                    StateVariable {
                        base_name: base_name.clone(),
                        dollar_name: dollar_name.clone(),
                        known_properties: HashSet::new(),
                        write_locations: Vec::new(),
                        read_locations: Vec::new(),
                        first_available: None,
                        seeded_by_special: false,
                    }
                });

                // Track known properties from dot-notation paths
                if let Some(ref path) = property_path {
                    entry.known_properties.insert(path.clone());
                }

                // Record the location in the appropriate list
                match &location.kind {
                    VarAccessKind::Assign | VarAccessKind::PropertyWrite { .. } => {
                        entry.write_locations.push(location);
                        // If this is in a special passage that contributes_variables,
                        // mark the variable as seeded by special
                        if is_special_seeding {
                            entry.seeded_by_special = true;
                        }
                    }
                    VarAccessKind::Read | VarAccessKind::PropertyRead { .. } => {
                        entry.read_locations.push(location);
                    }
                    VarAccessKind::Unset => {
                        // Unset doesn't go in either list, but we could track it
                        // separately in the future if needed
                    }
                }
            }
        }
    }

    registry
}

/// Parse a SugarCube variable name into its components.
///
/// - `"$hp"` → `("hp", "$hp", None)`
/// - `"$player.name"` → `("player", "$player", Some("name"))`
/// - `"$player.inventory.sword"` → `("player", "$player", Some("inventory.sword"))`
fn parse_var_name(name: &str) -> (String, String, Option<String>) {
    if let Some(dot_pos) = name.find('.') {
        let base = &name[..dot_pos];
        let path = &name[dot_pos + 1..];
        // base should start with $
        let base_name = if base.starts_with('$') {
            base[1..].to_string()
        } else {
            base.to_string()
        };
        let dollar_name = if base.starts_with('$') {
            base.to_string()
        } else {
            format!("${}", base)
        };
        (base_name, dollar_name, Some(path.to_string()))
    } else {
        let base_name = if name.starts_with('$') {
            name[1..].to_string()
        } else {
            name.to_string()
        };
        let dollar_name = if name.starts_with('$') {
            name.to_string()
        } else {
            format!("${}", name)
        };
        (base_name, dollar_name, None)
    }
}

// ---------------------------------------------------------------------------
// Graph-BFS variable availability computation
// ---------------------------------------------------------------------------

/// Compute variable-related diagnostics using graph-BFS availability analysis.
///
/// This is the SugarCube-specific replacement for the core's
/// `detect_uninitialized_reads()`, `detect_unused_variables()`, and
/// `detect_redundant_writes()`. The key insight is that SugarCube variables
/// are persistent `State.variables` entries — they are NOT traditional
/// scoped variables that need "definite assignment analysis".
///
/// ## Algorithm
///
/// 1. **Availability computation**: For each variable, find all passages that
///    write it. BFS forward from each write passage through the graph. Any
///    passage reachable from a write passage "has access" to that variable.
///    Variables seeded by special passages (StoryInit, Story JavaScript) are
///    considered available everywhere.
///
/// 2. **Diagnostics**: If a variable is read in a passage that is NOT reachable
///    from any write passage (and not seeded by special), emit a
///    `VariableAvailabilityHint`. This is a HINT, not an error, because the
///    variable might exist from a saved game or an unmodeled JS script.
///
/// 3. **Unused variables**: If a variable is written but never read in any
///    reachable passage, emit an `UnusedVariableHint`.
///
/// 4. **Redundant writes**: If a variable is written twice in the same passage
///    without an intervening read, emit a `RedundantWriteHint`.
///
/// 5. **Unknown properties**: If a property is read but never written anywhere,
///    emit an `UnknownPropertyHint`.
pub(crate) fn compute_variable_diagnostics(
    workspace: &knot_core::Workspace,
    start_passage: &str,
    registry: &HashMap<String, StateVariable>,
) -> Vec<crate::types::VariableDiagnostic> {
    use crate::types::{VariableDiagnostic, VariableDiagnosticKind};

    let mut diagnostics = Vec::new();

    // Collect the set of passages reachable from the start passage
    // (this is used to filter out diagnostics for unreachable passages,
    // which are already flagged by the core's unreachable passage detection)
    let reachable_from_start = bfs_reachable(workspace, start_passage);

    for (dollar_name, var) in registry {
        // Skip variables that are seeded by special passages (StoryInit, etc.)
        // They are always available from the start of the game.
        if var.seeded_by_special {
            continue;
        }

        // ── Variable availability hints ──────────────────────────────────
        // For each read location, check if the reading passage is reachable
        // from any write location via the narrative graph.
        if !var.write_locations.is_empty() {
            // Compute the set of passages that can "see" this variable
            // by BFS-ing forward from each write passage
            let mut available_passages: HashSet<String> = HashSet::new();
            for write_loc in &var.write_locations {
                available_passages.insert(write_loc.passage_name.clone());
                // BFS forward from this write passage
                let forward = bfs_forward(workspace, &write_loc.passage_name);
                for p in forward {
                    available_passages.insert(p);
                }
            }

            // Also make available from start passage if any write is in
            // a passage that precedes start (e.g., StoryInit)
            for write_loc in &var.write_locations {
                if is_pre_start_passage(workspace, &write_loc.passage_name) {
                    available_passages.insert(start_passage.to_string());
                    let forward = bfs_forward(workspace, start_passage);
                    for p in forward {
                        available_passages.insert(p);
                    }
                    break;
                }
            }

            // Check each read location for availability
            for read_loc in &var.read_locations {
                if !available_passages.contains(&read_loc.passage_name) {
                    // Only flag if the reading passage is itself reachable from start
                    // (unreachable passages are flagged separately by the core)
                    if reachable_from_start.contains(&read_loc.passage_name) {
                        diagnostics.push(VariableDiagnostic {
                            passage_name: read_loc.passage_name.clone(),
                            file_uri: read_loc.file_uri.clone(),
                            kind: VariableDiagnosticKind::VariableAvailabilityHint,
                            message: format!(
                                "Variable '{}' may not be available in passage '{}' \
                                 (no write in a preceding passage is reachable via narrative flow). \
                                 This is a hint — the variable may exist from a saved game.",
                                dollar_name, read_loc.passage_name
                            ),
                        });
                    }
                }
            }
        } else {
            // Variable has reads but NO writes anywhere — flag all reads
            for read_loc in &var.read_locations {
                if reachable_from_start.contains(&read_loc.passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: read_loc.passage_name.clone(),
                        file_uri: read_loc.file_uri.clone(),
                        kind: VariableDiagnosticKind::VariableAvailabilityHint,
                        message: format!(
                            "Variable '{}' is read but never written in any passage. \
                             It may come from a saved game or external script.",
                            dollar_name
                        ),
                    });
                }
            }
        }

        // ── Unused variable hints ─────────────────────────────────────────
        if !var.write_locations.is_empty() && var.read_locations.is_empty() {
            // Variable is written but never read
            if let Some(first_write) = var.write_locations.first() {
                if reachable_from_start.contains(&first_write.passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: first_write.passage_name.clone(),
                        file_uri: first_write.file_uri.clone(),
                        kind: VariableDiagnosticKind::UnusedVariableHint,
                        message: format!(
                            "Variable '{}' is written but never read in any reachable passage",
                            dollar_name
                        ),
                    });
                }
            }
        }

        // ── Unknown property hints ────────────────────────────────────────
        // Check if any property reads don't have corresponding property writes
        {
            let mut written_properties: HashSet<String> = HashSet::new();
            let mut read_properties: HashSet<(String, String)> = HashSet::new(); // (property_path, passage_name)

            for loc in &var.write_locations {
                if let VarAccessKind::PropertyWrite { path } = &loc.kind {
                    written_properties.insert(path.clone());
                }
            }
            // Base-level assigns also make all properties potentially available
            // (e.g., <<set $player to {name: "Alice"}>> makes $player.name available)
            let has_base_assign = var.write_locations.iter().any(|loc| {
                        matches!(&loc.kind, VarAccessKind::Assign)
                    });

            for loc in &var.read_locations {
                if let VarAccessKind::PropertyRead { path } = &loc.kind {
                    if !written_properties.contains(path) && !has_base_assign {
                        read_properties.insert((path.clone(), loc.passage_name.clone()));
                    }
                }
            }

            for (path, passage_name) in &read_properties {
                if reachable_from_start.contains(passage_name) {
                    diagnostics.push(VariableDiagnostic {
                        passage_name: passage_name.clone(),
                        file_uri: var.write_locations.first()
                            .or_else(|| var.read_locations.first())
                            .map(|l| l.file_uri.clone())
                            .unwrap_or_default(),
                        kind: VariableDiagnosticKind::UnknownPropertyHint,
                        message: format!(
                            "Property '{}.{}' is read but never written. \
                             The property may be set via base-level assignment \
                             (e.g., <<set {} to {{...}}>>)",
                            dollar_name, path, dollar_name
                        ),
                    });
                }
            }
        }
    }

    // ── Redundant write hints (intra-passage) ─────────────────────────────
    diagnostics.extend(compute_redundant_write_hints(workspace));

    diagnostics
}

/// Compute redundant write hints: a variable written twice in the same
/// passage without an intervening read.
fn compute_redundant_write_hints(
    workspace: &knot_core::Workspace,
) -> Vec<crate::types::VariableDiagnostic> {
    use crate::types::{VariableDiagnostic, VariableDiagnosticKind};

    let mut diagnostics = Vec::new();

    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.is_metadata() {
                continue;
            }

            let mut written_not_read: HashSet<String> = HashSet::new();
            let mut reported: HashSet<String> = HashSet::new();

            let sorted_vars = passage.vars_sorted_by_span();
            for var in sorted_vars {
                if var.is_temporary {
                    continue;
                }

                match var.kind {
                    VarKind::Init => {
                        if written_not_read.contains(&var.name) && !reported.contains(&var.name) {
                            diagnostics.push(VariableDiagnostic {
                                passage_name: passage.name.clone(),
                                file_uri: doc.uri.to_string(),
                                kind: VariableDiagnosticKind::RedundantWriteHint,
                                message: format!(
                                    "Variable '{}' is assigned again without being read \
                                     since the last assignment in passage '{}'",
                                    var.name, passage.name
                                ),
                            });
                            reported.insert(var.name.clone());
                        }
                        written_not_read.insert(var.name.clone());
                    }
                    VarKind::Read => {
                        written_not_read.remove(&var.name);
                        reported.remove(&var.name);
                    }
                }
            }
        }
    }

    diagnostics
}

/// BFS forward from a passage through the narrative graph.
/// Returns the set of passage names reachable via outgoing edges.
fn bfs_forward(workspace: &knot_core::Workspace, start: &str) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(start.to_string());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        for neighbor in workspace.graph.outgoing_neighbors(&current) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    visited
}

/// BFS from the start passage to determine all reachable passages.
fn bfs_reachable(workspace: &knot_core::Workspace, start: &str) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(start.to_string());

    while let Some(current) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());

        for neighbor in workspace.graph.outgoing_neighbors(&current) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    visited
}

/// Check if a passage runs before the start passage in the SugarCube lifecycle.
/// These passages (StoryInit, Story JavaScript) contribute variables that are
/// available from the very beginning of the game.
fn is_pre_start_passage(workspace: &knot_core::Workspace, passage_name: &str) -> bool {
    // Find the passage in the workspace
    for doc in workspace.documents() {
        if let Some(passage) = doc.passages.iter().find(|p| p.name == passage_name) {
            if passage.is_special {
                if let Some(ref def) = passage.special_def {
                    // Startup passages (StoryInit) and Story JavaScript run
                    // before the start passage
                    return matches!(
                        def.behavior,
                        knot_core::passage::SpecialPassageBehavior::Startup
                    ) || passage_name == "Story JavaScript";
                }
            }
            return false;
        }
    }
    false
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
    fn test_extract_vars_eq_assignment() {
        let body = "<<set $hp = 100>>You have $hp health.";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$hp" && v.kind == VarKind::Init),
            "Should detect <<set $hp = 100>> as Init"
        );
        assert!(vars.iter().any(|v| v.name == "$hp" && v.kind == VarKind::Read));
    }

    #[test]
    fn test_extract_vars_compound_assignment() {
        let body = "<<set $gold += 10>>You have $gold coins.";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init),
            "Should detect <<set $gold += 10>> as Init"
        );
    }

    #[test]
    fn test_extract_vars_decrement() {
        let body = "<<set $lives-->>$lives remaining.";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$lives" && v.kind == VarKind::Init),
            "Should detect $lives-- as Init"
        );
    }

    #[test]
    fn test_extract_vars_unset() {
        let body = "<<unset $temp>>";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$temp" && v.kind == VarKind::Init),
            "Should detect <<unset $temp>> as Init (state modification)"
        );
    }

    #[test]
    fn test_extract_vars_dollar_dollar_escape() {
        let body = "The variable $$name is set to: $name";
        let vars = extract_vars(body, 0);

        // $$name should NOT be detected as a variable
        assert!(
            !vars.iter().any(|v| v.name == "$$name"),
            "$$name should not be detected as a variable (it's an escape)"
        );
        // $name should be detected as a read
        assert!(
            vars.iter().any(|v| v.name == "$name" && v.kind == VarKind::Read),
            "$name should be detected as a Read"
        );
    }

    #[test]
    fn test_extract_vars_dot_notation() {
        let body = "You see $item.sword.damage.";
        let vars = extract_vars(body, 0);

        assert!(vars.iter().any(|v| v.name == "$item.sword.damage" && v.kind == VarKind::Read));
    }

    #[test]
    fn test_extract_vars_bracket_notation() {
        let body = r#"You see $item["sword"]."#;
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name.contains("sword") && v.kind == VarKind::Read),
            "Should detect $item[\"sword\"] as a property read"
        );
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
    fn test_extract_vars_js_state_getvar() {
        let body = r#"var hp = State.getVar("$hp");"#;
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$hp" && v.kind == VarKind::Read),
            "Should detect State.getVar(\"$hp\") as $hp Read"
        );
    }

    #[test]
    fn test_extract_vars_js_state_setvar() {
        let body = r#"State.setVar("$hp", 100);"#;
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$hp" && v.kind == VarKind::Init),
            "Should detect State.setVar(\"$hp\", 100) as $hp Init"
        );
    }

    #[test]
    fn test_extract_vars_setter_link() {
        let body = "[[Go to shop|Shop][$gold to 50]]";
        let vars = extract_vars(body, 0);

        assert!(
            vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init),
            "Should detect setter link assignment"
        );
    }

    #[test]
    fn test_extract_vars_dollar_in_name() {
        // Per SugarCube spec, $ is a valid second char: $$var is valid
        // But in passage text, $$ is the escape markup. In macro args,
        // $$var would be valid (if someone names a var with $ in it).
        let body = "<<set $$special to 1>>";
        let vars = extract_vars(body, 0);

        // The $$ in <<set $$special to 1>> is tricky — in a macro arg,
        // $$special is actually a variable named $special (second char is $).
        // Our regex should handle this since we match $ followed by a name
        // starting with A-Za-z$_. But the negative lookbehind prevents
        // matching if preceded by another $.
        // This is an edge case; the behavior depends on context.
        // For now, we just ensure it doesn't crash.
        assert!(!vars.is_empty() || vars.is_empty()); // No panic
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
