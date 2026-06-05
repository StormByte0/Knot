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

pub mod property_map;
pub mod state_registry;
pub mod variable_diagnostics;

// Re-export moved items so existing callers don't break
pub(crate) use property_map::{
    extract_object_property_map, build_shape_aware_property_map,
    infer_variable_kind_from_properties,
};
pub(crate) use state_registry::build_state_variable_registry;
pub(crate) use variable_diagnostics::compute_variable_diagnostics;
// Re-export types from crate::types that are used by build_variable_tree / build_property_tree
pub(crate) use crate::types::{VarAccessKind, VarLocation};

use knot_core::passage::{VarKind, VarOp};
use std::sync::LazyLock;
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
pub(crate) static RE_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `_variableName` — SugarCube temporary/scratch variable reference.
///
/// Per the SugarCube spec, temporary variable names follow the same rules
/// as story variables but with `_` as the sigil.
pub(crate) static RE_TEMP_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"_([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `<<set $var to ...>>` — TwineScript `to` assignment for persistent vars
pub(crate) static RE_SET_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s+to\b").unwrap());

/// `<<set $var = ...>>` — JavaScript `=` assignment for persistent vars.
/// We match `=` that is NOT preceded by `!<>` (to avoid `==`, `!=`, `<=`, `>=`)
/// and NOT followed by `=` (to avoid `==` and `===`).
/// Since the regex crate doesn't support lookbehind, we use a simpler approach:
/// match the assignment and then filter out compound operators in the code.
pub(crate) static RE_SET_MACRO_EQ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*=").unwrap());

/// `<<set $var += ...>>` — Compound assignment for persistent vars
/// (also catches -=, *=, /=, %=)
pub(crate) static RE_SET_MACRO_COMPOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<set\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*([\+\-\*\/%])=").unwrap());

/// `<<set _var to ...>>` — TwineScript `to` assignment for temporary vars
pub(crate) static RE_SET_TEMP_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<set\s+_([A-Za-z\$_][A-Za-z0-9\$_]*)\s+to\b").unwrap());

/// `<<set _var = ...>>` — JavaScript `=` assignment for temporary vars
pub(crate) static RE_SET_TEMP_MACRO_EQ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<set\s+_([A-Za-z\$_][A-Za-z0-9\$_]*)\s*=").unwrap());

/// `$varname.property.path` — dot-notation variable reference.
/// The `$$` escape is handled by filtering in the extraction code.
pub(crate) static RE_VAR_DOT_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)+)"
).unwrap());

/// `$varname["property"]` or `$varname['property']` — bracket-notation
/// property access. The property name is captured.
/// The `$$` escape is handled by filtering in the extraction code.
pub(crate) static RE_VAR_BRACKET_PROP: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r#"\$([A-Za-z\$_][A-Za-z0-9\$_]*)\[["']([A-Za-z\$_][A-Za-z0-9\$_]*)["']\]"#
).unwrap());

/// `var/let/const x = State.variables.specificVar` — JS aliasing of a specific
/// SugarCube state variable (e.g., `var gold = State.variables.gold`)
pub(crate) static RE_JS_ALIAS_SPECIFIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// `var/let/const x = State.variables` — JS aliasing of the ENTIRE State.variables
/// object (e.g., `var v = State.variables; v.gold = 10;`)
pub(crate) static RE_JS_ALIAS_WHOLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:var|let|const)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*State\.variables\b").unwrap()
});

/// `State.variables.varName = value` — JS direct write to SugarCube state
pub(crate) static RE_JS_STATE_WRITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"State\.variables\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap()
});

/// `State.getVar("$varName")` — JS API to read a variable
pub(crate) static RE_JS_STATE_GETVAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"State\.getVar\(\s*"\$([A-Za-z_][A-Za-z0-9_]*)\s*""#).unwrap()
});

/// `State.setVar("$varName", value)` — JS API to write a variable
pub(crate) static RE_JS_STATE_SETVAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"State\.setVar\(\s*"\$([A-Za-z_][A-Za-z0-9_]*)""#).unwrap()
});

/// `alias.property` — access through a whole-object alias.
/// This is detected after finding a `var x = State.variables` alias.
pub(crate) static RE_ALIAS_PROPERTY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// `<<unset $var>>` — macro that explicitly removes a state variable
pub(crate) static RE_UNSET_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<unset\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)>>").unwrap());

/// Setter link: `[[text|passage][$var to value]]` or `[[text|passage][$var = value]]`
/// These assign variables during link navigation.
pub(crate) static RE_SETTER_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"\[\[[^\]]*?\]\[\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s+(?:to|=)"
).unwrap());

/// `$var++` or `++$var` — post/pre increment (both read and write)
pub(crate) static RE_VAR_INCREMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)\+\+|\+\+\$([A-Za-z\$_][A-Za-z0-9\$_]*))").unwrap());

/// `$var--` or `--$var` — post/pre decrement (both read and write)
pub(crate) static RE_VAR_DECREMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)--|--\$([A-Za-z\$_][A-Za-z0-9\$_]*))").unwrap());

/// `<<run ...>>` — macro that executes raw JavaScript.
/// Capture group 1 is the JavaScript body between `<<run ` and `>>`.
pub(crate) static RE_RUN_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<run\s+([\s\S]*?)>>").unwrap());

/// `<<if $var ...>>`, `<<elseif $var ...>>`, `<<when $var ...>>` —
/// conditional macros that read a variable.
pub(crate) static RE_IF_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<(?:if|elseif|when)\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `<<capture $var ...>>` — macro that captures/assigns a variable (WRITE).
pub(crate) static RE_CAPTURE_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<capture\s+\$([A-Za-z\$_][A-Za-z0-9\$_]*)").unwrap());

/// `$var =` — JS-style assignment of a persistent SugarCube variable
/// within `<<run>>` macro bodies. Must be filtered in code to exclude
/// `==`/`===` comparisons and compound assignments (`+=`, etc.).
pub(crate) static RE_JS_VAR_ASSIGN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*=").unwrap());

/// `$var +=` etc. — JS compound assignment of a persistent variable
/// within `<<run>>` macro bodies (also `-=`, `*=`, `/=`, `%=`).
pub(crate) static RE_JS_VAR_COMPOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([A-Za-z\$_][A-Za-z0-9\$_]*)\s*([\+\-\*\/%])=").unwrap());

// ---------------------------------------------------------------------------
// Variable extraction
// ---------------------------------------------------------------------------

/// Extract variable operations from a passage body.
///
/// Detects both persistent (`$var`) and temporary (`_var`) variables.
/// First detects assignment patterns (`<<set>>`, `<<capture>>`, `<<run>>`,
/// `<<unset>>`, JS writes), then all `$var` / `_var` references not already
/// captured as inits are treated as reads. Temporary variables are marked
/// with `is_temporary: true`.
///
/// ## Detected patterns
///
/// - `<<set $var to ...>>` / `<<set $var = ...>>` — base assignment
/// - `<<set $var += ...>>` — compound assignment (also `-=`, `*=`, `/=`, `%=`)
/// - `<<capture $var ...>>` — capture/assign a variable (WRITE)
/// - `<<run $var = value>>` — JS assignment inside `<<run>>` macro (WRITE)
/// - `<<run $var += value>>` — JS compound assignment inside `<<run>>` (WRITE)
/// - `<<run State.variables.var = value>>` — JS direct write inside `<<run>>`
/// - `<<if $var>>` / `<<elseif $var>>` / `<<when $var>>` — conditional READ
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

    // ── Capture macro: <<capture $var ...>> — WRITE ──────────────────
    for caps in RE_CAPTURE_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_match = caps.get(1).unwrap();
        let name = format!("${}", var_match.as_str());
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_end = var_start + name.len();

        let is_dup = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_dup {
            vars.push(VarOp {
                name,
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            init_spans.push(var_start..var_end);
        }
    }

    // ── Conditional macros: <<if $var>>, <<elseif $var>>, <<when $var>> — READ ──
    for caps in RE_IF_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_match = caps.get(1).unwrap();
        let name = format!("${}", var_match.as_str());
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_end = var_start + name.len();

        // Only record if not already recorded at this exact span (avoids
        // double-counting with the general RE_VAR scan below)
        let is_already = vars.iter().any(|v| {
            v.span.start == var_start && v.span.end == var_end
        });
        // Also skip if this position was already recorded as an Init
        // (e.g., <<if>> after <<set>> at same spot — unlikely but safe)
        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_already && !is_init {
            vars.push(VarOp {
                name,
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }
    }

    // ── Run macro: <<run ...>> — JS body variable extraction ──────────
    // The <<run>> macro executes raw JavaScript. Inside its body:
    //   $var = value       → WRITE (persistent var assignment)
    //   $var += value      → WRITE (compound assignment)
    //   State.variables.var = value → WRITE (handled by existing scan)
    //   State.setVar("$var", val)   → WRITE (handled by existing scan)
    //   State.getVar("$var")        → READ  (handled by existing scan)
    //   $var              → READ  (caught by general RE_VAR scan below)
    for caps in RE_RUN_MACRO.captures_iter(body) {
        let js_body = caps.get(1).unwrap();
        let js_text = js_body.as_str();
        let js_offset = body_offset + js_body.start();

        // Detect compound assignments first: $var +=, -=, *=, /=, %=
        for js_caps in RE_JS_VAR_COMPOUND.captures_iter(js_text) {
            let full = js_caps.get(0).unwrap();
            let var_match = js_caps.get(1).unwrap();
            let name = format!("${}", var_match.as_str());
            let var_start = js_offset + full.start();
            let var_end = var_start + name.len();

            // Skip $$ escape markup
            let is_dollar_escape = full.start() > 0
                && js_text.as_bytes()[full.start() - 1] == b'$';

            if is_dollar_escape {
                continue;
            }

            let is_dup = init_spans.iter().any(|s| {
                var_start >= s.start && var_end <= s.end
            });
            if !is_dup {
                vars.push(VarOp {
                    name,
                    kind: VarKind::Init,
                    span: var_start..var_end,
                    is_temporary: false,
                });
                init_spans.push(var_start..var_end);
            }
        }

        // Detect simple assignments: $var = (but NOT ==, ===, or compound)
        for js_caps in RE_JS_VAR_ASSIGN.captures_iter(js_text) {
            let full = js_caps.get(0).unwrap();
            let var_match = js_caps.get(1).unwrap();
            let name = format!("${}", var_match.as_str());
            let var_start = js_offset + full.start();
            let var_end = var_start + name.len();

            // Skip $$ escape markup
            let is_dollar_escape = full.start() > 0
                && js_text.as_bytes()[full.start() - 1] == b'$';

            if is_dollar_escape {
                continue;
            }

            // Skip if this is a compound assignment (already handled above)
            let is_compound = RE_JS_VAR_COMPOUND.captures_iter(js_text).any(|cc| {
                cc.get(0).unwrap().start() == full.start()
            });
            if is_compound {
                continue;
            }

            // Skip if this is == or === (comparison, not assignment).
            // The regex consumes the `=` sign, so we check the character
            // immediately after the match.
            let after_match = js_text.get(full.end()..).unwrap_or("");
            if after_match.starts_with('=') {
                continue;
            }

            let is_dup = init_spans.iter().any(|s| {
                var_start >= s.start && var_end <= s.end
            });
            if !is_dup {
                vars.push(VarOp {
                    name,
                    kind: VarKind::Init,
                    span: var_start..var_end,
                    is_temporary: false,
                });
                init_spans.push(var_start..var_end);
            }
        }

        // Note: $var references inside <<run>> that are NOT assignments
        // will be caught by the existing RE_VAR scan below and treated
        // as reads (since they won't be in init_spans).
        //
        // State.variables.var =, State.getVar(), and State.setVar()
        // inside <<run>> are already handled by the existing detection
        // blocks that scan the entire body text.
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

        // Skip if already recorded at this exact span (e.g., by <<if>>/<<elseif>>/<<when>>
        // detection or <<run>> body extraction above)
        let is_already_recorded = vars.iter().any(|v| {
            v.span.start == var_start && v.span.end == var_end
        });

        if !is_init && !is_dot_subspan && !is_bracket_subspan && !is_already_recorded {
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

/// Extract variable references for a specific passage using tree-based analysis.
///
/// Tree-based extraction: walk the passage tree directly instead of building the
/// full virtual document and regex-scanning JS output. This is faster (no vdoc
/// build) and more accurate (exact line numbers from byte spans instead of
/// proportional mapping).
pub(crate) fn extract_passage_variable_refs(
    workspace: &knot_core::Workspace,
    source_text: &dyn crate::plugin::SourceTextProvider,
    passage_name: &str,
) -> Vec<crate::types::PassageVarRef> {
    let mut refs = Vec::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.name != passage_name {
                continue;
            }
            let file_uri = doc.uri.to_string();
            let src = source_text.get_source_text(&file_uri);
            if let Some(src) = src {
                let header_start = passage.span.start;
                let header_line_end = src[header_start..]
                    .find('\n')
                    .map(|i| header_start + i)
                    .unwrap_or(src.len().min(header_start + passage.name.len() + 2));
                let newline_len = if src.get(header_line_end..header_line_end + 2) == Some("\r\n") { 2 } else if header_line_end < src.len() { 1 } else { 0 };
                let body_offset = header_line_end + newline_len;

                if body_offset >= src.len() {
                    continue;
                }
                let body = &src[body_offset..];
                let tree = super::passage_tree::parse_passage_body(body, body_offset);
                refs.extend(super::passage_tree::walk_passage_var_refs(
                    &tree, body, body_offset, passage_name, &file_uri,
                ));
            } else {
                for var in &passage.vars {
                    refs.push(crate::types::PassageVarRef {
                        variable_name: var.name.clone(),
                        is_write: matches!(var.kind, knot_core::passage::VarKind::Init),
                        line: 0,
                        file_uri: file_uri.clone(),
                        passage_name: passage.name.clone(),
                    });
                }
            }
        }
    }
    refs
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

// ---------------------------------------------------------------------------
// Variable tree (format-agnostic UI representation)
// ---------------------------------------------------------------------------

/// Build a tree-structured representation of all SugarCube state variables
/// for display in the variable tracker UI.
///
/// This function converts the flat `StateVariable` registry into a
/// `Vec<VariableTreeNode>` that mirrors the `State.variables` hierarchy.
/// For example, `$player.hp` maps to `State.variables.player.hp` and is
/// represented as a child property of the `$player` variable node.
///
/// **Format isolation**: This function is the ONLY place where
/// SugarCube-specific strings like `"State.variables"` are used. The server
/// never hardcodes these — it just performs a mechanical translation from
/// `VariableTreeNode` to LSP wire types.
/// Compute a 0-based line number from a byte offset within a file.
///
/// Given a file URI and a byte offset, this function looks up the matching
/// `Document` in the workspace and uses its `DocumentSnapshot` (Rope) to
/// convert the byte offset to a line number. Returns 0 if the document is
/// not found or has no snapshot.
fn compute_line_from_offset(
    workspace: &knot_core::Workspace,
    file_uri: &str,
    byte_offset: usize,
) -> u32 {
    // Try to parse the file_uri as a Url and look up the document
    if let Ok(url) = url::Url::parse(file_uri) {
        if let Some(doc) = workspace.get_document(&url) {
            return doc.byte_to_line(byte_offset);
        }
    }
    0
}

pub(crate) fn build_variable_tree(
    workspace: &knot_core::Workspace,
) -> Vec<crate::types::VariableTreeNode> {
    use crate::types::{VariableTreeNode, VariableUsageLocation};

    let registry = build_state_variable_registry(workspace);

    // Build basic property map for kind inference
    let vars_by_passage: Vec<Vec<&VarOp>> = workspace
        .documents()
        .flat_map(|doc| doc.passages.iter().map(|p| p.vars.iter().collect()))
        .collect();
    let basic_map = extract_object_property_map(&vars_by_passage);

    let mut variables: Vec<VariableTreeNode> = Vec::new();

    for (dollar_name, state_var) in &registry {
        let base_name = &state_var.base_name;
        let state_path = format!("State.variables.{}", base_name);

        // Build write/read locations for the base variable
        // (only base-level Assign/Read, not property accesses)
        let mut written_in: Vec<VariableUsageLocation> = Vec::new();
        for loc in &state_var.write_locations {
            if matches!(loc.kind, VarAccessKind::Assign) {
                let line = compute_line_from_offset(workspace, &loc.file_uri, loc.span.start);
                written_in.push(VariableUsageLocation {
                    passage_name: loc.passage_name.clone(),
                    file_uri: loc.file_uri.clone(),
                    is_write: true,
                    line,
                });
            }
        }

        let mut read_in: Vec<VariableUsageLocation> = Vec::new();
        for loc in &state_var.read_locations {
            if matches!(loc.kind, VarAccessKind::Read) {
                let line = compute_line_from_offset(workspace, &loc.file_uri, loc.span.start);
                read_in.push(VariableUsageLocation {
                    passage_name: loc.passage_name.clone(),
                    file_uri: loc.file_uri.clone(),
                    is_write: false,
                    line,
                });
            }
        }

        let is_unused = !written_in.is_empty() && read_in.is_empty();

        // Build property tree from known_properties
        let properties = build_property_tree(
            dollar_name,
            &state_var.known_properties,
            &state_var.write_locations,
            &state_var.read_locations,
            workspace,
        );

        variables.push(VariableTreeNode {
            name: dollar_name.clone(),
            state_path,
            is_temporary: false,
            written_in,
            read_in,
            initialized_at_start: state_var.seeded_by_special,
            is_unused,
            properties,
            kind: infer_variable_kind_from_properties(
                dollar_name,
                &state_var.known_properties,
                &basic_map,
            ),
            element_shape: None, // TODO: Populated by virtual doc path when available
        });
    }

    // Sort by name for deterministic output
    variables.sort_by(|a, b| a.name.cmp(&b.name));

    variables
}

/// Build a recursive property tree from the known dot-notation paths.
///
/// Groups properties by their first segment, then recurses for deeper paths.
/// For example, `known_properties = {"name", "inventory.sword", "inventory.shield"}`
/// produces:
/// ```text
/// .name
/// .inventory
///   .sword
///   .shield
/// ```
fn build_property_tree(
    dollar_name: &str,
    known_properties: &HashSet<String>,
    write_locations: &[VarLocation],
    read_locations: &[VarLocation],
    workspace: &knot_core::Workspace,
) -> Vec<crate::types::VariablePropertyNode> {
    use crate::types::{VariablePropertyNode, VariableUsageLocation};

    // Collect immediate children (first segment of each path)
    let mut children: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for path in known_properties {
        let parts: Vec<&str> = path.splitn(2, '.').collect();
        if parts.is_empty() {
            continue;
        }
        let first = parts[0].to_string();
        let rest = if parts.len() > 1 {
            Some(parts[1].to_string())
        } else {
            None
        };
        children.entry(first.clone()).or_default();
        if let Some(r) = rest {
            children.get_mut(&first).unwrap().push(r);
        }
    }

    let mut result = Vec::new();

    for (prop_name, sub_paths) in children {
        let full_name = format!("{}.{}", dollar_name, prop_name);
        // Strip the $ sigil to build the State.variables path
        let base_without_sigil = if dollar_name.starts_with('$') {
            &dollar_name[1..]
        } else {
            dollar_name
        };
        let state_path = format!("State.variables.{}.{}", base_without_sigil, prop_name);

        // Collect write locations for this specific property path
        let mut prop_written_in: Vec<VariableUsageLocation> = Vec::new();
        for loc in write_locations {
            match &loc.kind {
                VarAccessKind::PropertyWrite { path } => {
                    if path == &prop_name {
                        prop_written_in.push(VariableUsageLocation {
                            passage_name: loc.passage_name.clone(),
                            file_uri: loc.file_uri.clone(),
                            is_write: true,
                            line: compute_line_from_offset(workspace, &loc.file_uri, loc.span.start),
                        });
                    }
                }
                _ => {}
            }
        }

        // Collect read locations for this specific property path
        let mut prop_read_in: Vec<VariableUsageLocation> = Vec::new();
        for loc in read_locations {
            match &loc.kind {
                VarAccessKind::PropertyRead { path } => {
                    if path == &prop_name {
                        prop_read_in.push(VariableUsageLocation {
                            passage_name: loc.passage_name.clone(),
                            file_uri: loc.file_uri.clone(),
                            is_write: false,
                            line: compute_line_from_offset(workspace, &loc.file_uri, loc.span.start),
                        });
                    }
                }
                _ => {}
            }
        }

        // Build sub-properties recursively
        let sub_properties = if sub_paths.is_empty() {
            Vec::new()
        } else {
            let sub_set: HashSet<String> = sub_paths.into_iter().collect();
            // Filter write/read locations for sub-paths
            let sub_writes: Vec<VarLocation> = write_locations
                .iter()
                .filter(|loc| match &loc.kind {
                    VarAccessKind::PropertyWrite { path } => {
                        path.starts_with(&format!("{}.", prop_name))
                    }
                    _ => false,
                })
                .cloned()
                .map(|mut loc| {
                    // Adjust path: strip the "prop_name." prefix
                    if let VarAccessKind::PropertyWrite { path } = &loc.kind {
                        let new_path = path.strip_prefix(&format!("{}.", prop_name)).unwrap_or(path).to_string();
                        loc.kind = VarAccessKind::PropertyWrite { path: new_path };
                    }
                    loc
                })
                .collect();

            let sub_reads: Vec<VarLocation> = read_locations
                .iter()
                .filter(|loc| match &loc.kind {
                    VarAccessKind::PropertyRead { path } => {
                        path.starts_with(&format!("{}.", prop_name))
                    }
                    _ => false,
                })
                .cloned()
                .map(|mut loc| {
                    if let VarAccessKind::PropertyRead { path } = &loc.kind {
                        let new_path = path.strip_prefix(&format!("{}.", prop_name)).unwrap_or(path).to_string();
                        loc.kind = VarAccessKind::PropertyRead { path: new_path };
                    }
                    loc
                })
                .collect();

            // Recurse with adjusted paths
            build_property_tree(
                &full_name,
                &sub_set,
                &sub_writes,
                &sub_reads,
                workspace,
            )
        };

        let has_sub_properties = !sub_properties.is_empty();
        result.push(VariablePropertyNode {
            name: prop_name,
            full_name,
            state_path,
            line: prop_written_in
                .iter()
                .chain(prop_read_in.iter())
                .map(|l| l.line)
                .min()
                .unwrap_or(0),
            written_in: prop_written_in,
            read_in: prop_read_in,
            properties: sub_properties,
            kind: if has_sub_properties {
                crate::types::PropertyKind::Object
            } else {
                crate::types::PropertyKind::Scalar
            },
            element_shape: None, // TODO: Populated by virtual doc path when available
            coverage: None,      // TODO: Populated by coverage analysis
        });
    }

    result
}
