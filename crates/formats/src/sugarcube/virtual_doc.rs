//! SugarCube-specific virtual document hooks.
//!
//! This module provides SugarCube-specific implementations of the virtual
//! document hooks defined by `VirtualDocHooks`. The core virtual document
//! builder (`crate::virtual_doc`) handles the format-agnostic parts (script
//! passage collection, concatenation, line mapping), while this module
//! provides:
//!
//! - **Startup alias extraction**: SugarCube-specific regex patterns for
//!   `State.variables`, `gs()`, `SugarCube.State.Variables`, `reg()`, etc.
//! - **Macro detection**: Checking for `<<set>>`, `<<run>>`, `$`, etc.
//! - **Macro→JS translation**: Faithful conversion of SugarCube macros to
//!   JavaScript using `OperatorNormalization` mappings, with full support
//!   for control flow (`<<if>>`, `<<for>>`, `<<switch>>`), output
//!   (`<<print>>`, `<<=>>`), forms (`<<textbox>>`, `<<checkbox>>`), and
//!   link macros with deferred bodies.
//! - **User callable extraction**: Detection of custom macros (`Macro.add`)
//!   and widget definitions (`<<widget>>`), enabling invocation translation
//!   as function calls.
//!
//! The SugarCube plugin's `FormatPlugin` implementation delegates its virtual
//! doc hook methods to the public functions in this module. The core builder
//! then calls these hooks at the right points.

use crate::types::{
    AliasResolution, MacroDef, PassageInfo, StartupAlias, UserCallable, UserCallableKind,
    VirtualSection, VirtualSectionKind,
};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

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
// Regexes for macro-to-JS translation
// ---------------------------------------------------------------------------

/// Generic macro tag parser: matches `<<name args>>`, `<</name>>`, `<<= args>>`, `<<- args>>`
/// Group 1: "/" if close tag, empty if open tag
/// Group 2: macro name (=, -, or alphanumeric)
/// Group 3: arguments (optional)
static RE_MACRO_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<<(/?)(=|-|[A-Za-z_][A-Za-z0-9_]*)(?:\s+([\s\S]*?))?>>").unwrap()
});

/// `$var++` / `++$var` / `$var--` / `--$var`
static RE_VD_INCREMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)\+\+|\+\+\$([A-Za-z\$_][A-Za-z0-9\$_]*))")
        .unwrap()
});

static RE_VD_DECREMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\$([A-Za-z\$_][A-Za-z0-9\$_]*)--|--\$([A-Za-z\$_][A-Za-z0-9\$_]*))")
        .unwrap()
});

/// `$var` naked reference (read) — for extracting from macro bodies
/// This is intentionally simple; the full extraction is done by vars.rs
static RE_VD_DOLLAR_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)*)"#).unwrap()
});

// ---------------------------------------------------------------------------
// Regexes for user callable extraction
// ---------------------------------------------------------------------------

/// `Macro.add('name', ...)` — finds the start of a single-name Macro.add call.
/// Group 1: macro name. The handler body must be extracted with brace-counting
/// because regex can't count nested braces.
static RE_MACRO_ADD_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"Macro\.add\s*\(\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]\s*,"#)
    .unwrap()
});

/// `Macro.add(['name1', 'name2'], ...)` — finds the start of a multi-name Macro.add call.
/// Group 1: the array of names. The handler body must be extracted with
/// brace-counting.
static RE_MACRO_ADD_MULTI_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"Macro\.add\s*\(\s*\[([^\]]*)\]\s*,"#)
    .unwrap()
});

/// `handler : function(params) {` — matches the handler function header inside
/// a Macro.add object literal. Group 1: the params. After this match, the
/// handler body must be extracted with brace-counting from the position after
/// the opening `{`.
static RE_HANDLER_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"handler\s*:\s*function\s*\(([^)]*)\)\s*\{")
    .unwrap()
});

/// Extract a brace-balanced body from `text` starting at `start_pos` (which
/// should point just after the opening `{`).
///
/// Counts `{` and `}` while respecting string literals (single-quoted,
/// double-quoted, and backtick template literals) and `//` line comments
/// and `/* */` block comments so that braces inside strings/comments
/// don't affect the count.
///
/// Returns the body content between the opening and closing braces
/// (exclusive of the braces themselves), or `None` if the braces are
/// unbalanced.
fn extract_brace_body(text: &str, start_pos: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut depth = 1u32;
    let mut i = start_pos;

    while i < len && depth > 0 {
        let ch = bytes[i];

        // String literals: skip their contents
        if ch == b'"' || ch == b'\'' || ch == b'`' {
            let quote = ch;
            i += 1;
            let mut escaped = false;
            while i < len {
                let c = bytes[i];
                if escaped {
                    escaped = false;
                    i += 1;
                    continue;
                }
                if c == b'\\' {
                    escaped = true;
                    i += 1;
                    continue;
                }
                if c == quote {
                    i += 1;
                    break;
                }
                // Template literal ${...} nesting — just skip the $, brace
                // counting will handle the braces normally
                i += 1;
            }
            continue;
        }

        // Line comments: skip to end of line
        if ch == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Block comments: skip to */
        if ch == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // Braces
        if ch == b'{' {
            depth += 1;
        } else if ch == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(text[start_pos..i].to_string());
            }
        }

        i += 1;
    }

    None
}

/// Extract the handler function from a Macro.add() call starting at
/// the given position in the text.
///
/// Looks for `handler: function(params) {` and then uses brace-counting
/// to extract the complete handler body, including nested braces.
///
/// Returns `(handler_params, handler_body, match_end)` where:
/// - `handler_params`: the parameter string from `function(params)`
/// - `handler_body`: the complete body between `{` and the matching `}`
/// - `match_end`: the byte offset just after the closing `}`
fn extract_handler_from_macro_add(text: &str, search_from: usize) -> Option<(String, String, usize)> {
    // Find the handler function header within the Macro.add object literal
    let handler_caps = RE_HANDLER_HEADER.captures(&text[search_from..])?;
    let full_match = handler_caps.get(0)?;
    let params = handler_caps.get(1)?.as_str().to_string();

    // The handler body starts just after the `{` at the end of the match
    let body_start = search_from + full_match.end();

    // Use brace-counting to extract the complete handler body
    let body = extract_brace_body(text, body_start)?;

    // The match end is just after the closing `}` of the handler
    let body_end = body_start + body.len() + 1; // +1 for the closing `}`

    Some((params, body, body_end))
}

/// `this.args[N]` usage in handler body — to determine arg count
static RE_THIS_ARGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"this\.args\s*\[\s*(\d+)\s*\]").unwrap()
});

/// `<<widget name>>` — widget definition opening tag
static RE_WIDGET_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<<widget\s+([A-Za-z_][A-Za-z0-9_]*)\s*>>").unwrap()
});

// NOTE: Comment-stripping regexes moved to core module (crate::virtual_doc).
// SugarCube-specific code uses crate::virtual_doc::strip_comments() directly.

// ---------------------------------------------------------------------------
// Virtual document builder
// ---------------------------------------------------------------------------

// NOTE: build_virtual_document() has been moved to the core builder
// at crate::virtual_doc::build_core_virtual_document(). The SugarCube
// plugin now implements the VirtualDocHooks trait methods instead of
// building the document directly.

// NOTE: extract_body_text() has been moved to the core builder
// at crate::virtual_doc::extract_body_text().

// ---------------------------------------------------------------------------
// Startup alias extraction
// ---------------------------------------------------------------------------

/// Extract startup aliases from the unified script section.
///
/// This runs after the unified script section is built. It strips comments
/// and then applies regex patterns to find alias definitions:
///
/// - `var v = State.variables` → StateVariables alias
/// - `var g = gs()` → StateVariables alias (gs() returns State.variables)
/// - `var x = State.variables.propName` → StateVariableProperty alias
/// - `function reg(name) { return State.variables[name]; }` → GetterFunction alias
pub fn extract_startup_aliases(sections: &[VirtualSection]) -> Vec<StartupAlias> {
    let mut aliases: Vec<StartupAlias> = Vec::new();

    // Find the unified script section
    let script_section = match sections.iter().find(|s| s.kind == VirtualSectionKind::UnifiedScript) {
        Some(s) => s,
        None => return aliases,
    };

    // Strip comments before analyzing
    let stripped = strip_comments(&script_section.source_text);

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

// ---------------------------------------------------------------------------
// User callable extraction (custom macros & widgets)
// ---------------------------------------------------------------------------

/// Extract user-defined callables from all passages.
///
/// This scans for two kinds of user-defined callables:
///
/// 1. **Custom macros** defined via `Macro.add('name', { handler: function() { ... } })`
///    in `[script]` passages. The handler's `this.args[N]` usage determines
///    the argument count.
///
/// 2. **Widgets** defined via `<<widget name>>...<</widget>>` in passages
///    tagged `[widget]`. Widgets act as reusable macros whose body is
///    executed when invoked.
pub fn extract_user_callables(passages: &[PassageInfo]) -> Vec<UserCallable> {
    let mut callables: Vec<UserCallable> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for passage in passages {
        // ── Custom macros from script passages ──────────────────────────
        if passage.tags.contains(&"script".to_string()) {
            let stripped = strip_comments(&passage.body_text);

            // Single-name Macro.add('name', ...)
            // We find the start pattern, then use brace-counting to extract
            // the complete handler body (including nested braces).
            for caps in RE_MACRO_ADD_START.captures_iter(&stripped) {
                let name = caps.get(1).unwrap().as_str().to_string();
                if seen_names.contains(&name) {
                    continue;
                }
                let macro_add_start = caps.get(0).unwrap().start();
                let macro_add_end = caps.get(0).unwrap().end();
                let line_no = line_from_offset(&stripped, macro_add_start);

                // Extract handler using brace-counting from the Macro.add start
                match extract_handler_from_macro_add(&stripped, macro_add_end) {
                    Some((handler_params, handler_body, _match_end)) => {
                        let arg_count = compute_macro_arg_count(&handler_body, &handler_params);
                        seen_names.insert(name.clone());
                        callables.push(UserCallable {
                            name,
                            kind: UserCallableKind::CustomMacro,
                            arg_count,
                            defined_in: passage.name.clone(),
                            file_uri: passage.file_uri.clone(),
                            defined_at_line: line_no,
                            body: Some(handler_body),
                        });
                    }
                    None => {
                        // Couldn't extract handler — still register the macro
                        // without a body so invocations can be translated
                        seen_names.insert(name.clone());
                        callables.push(UserCallable {
                            name,
                            kind: UserCallableKind::CustomMacro,
                            arg_count: None,
                            defined_in: passage.name.clone(),
                            file_uri: passage.file_uri.clone(),
                            defined_at_line: line_no,
                            body: None,
                        });
                    }
                }
            }

            // Multi-name Macro.add(['name1', 'name2'], ...)
            for caps in RE_MACRO_ADD_MULTI_START.captures_iter(&stripped) {
                let names_str = caps.get(1).unwrap().as_str();
                let macro_add_start = caps.get(0).unwrap().start();
                let macro_add_end = caps.get(0).unwrap().end();
                let line_no = line_from_offset(&stripped, macro_add_start);

                // Extract handler using brace-counting
                let extraction = extract_handler_from_macro_add(&stripped, macro_add_end);
                let (arg_count, handler_body) = match extraction {
                    Some((handler_params, handler_body, _match_end)) => {
                        (compute_macro_arg_count(&handler_body, &handler_params), Some(handler_body))
                    }
                    None => (None, None),
                };

                // Parse the array of names: ['name1', 'name2']
                for name_cap in Regex::new(r#"['"]([A-Za-z_][A-Za-z0-9_]*)['"]"#)
                    .unwrap()
                    .captures_iter(names_str)
                {
                    let name = name_cap.get(1).unwrap().as_str().to_string();
                    if seen_names.contains(&name) {
                        continue;
                    }
                    seen_names.insert(name.clone());
                    callables.push(UserCallable {
                        name,
                        kind: UserCallableKind::CustomMacro,
                        arg_count,
                        defined_in: passage.name.clone(),
                        file_uri: passage.file_uri.clone(),
                        defined_at_line: line_no,
                        body: handler_body.clone(),
                    });
                }
            }
        }

        // ── Widgets from widget-tagged passages ─────────────────────────
        if passage.tags.contains(&"widget".to_string()) {
            // Scan the passage body for <<widget name>> definitions
            for caps in RE_WIDGET_DEF.captures_iter(&passage.body_text) {
                let name = caps.get(1).unwrap().as_str().to_string();
                if seen_names.contains(&name) {
                    continue;
                }
                let match_start = caps.get(0).unwrap().start();
                let line_no = line_from_offset(&passage.body_text, match_start);

                // Extract widget body (between <<widget name>> and <</widget>>)
                let body = extract_widget_body(&passage.body_text, &name);

                seen_names.insert(name.clone());
                callables.push(UserCallable {
                    name,
                    kind: UserCallableKind::Widget,
                    arg_count: None, // Widgets are variadic by default
                    defined_in: passage.name.clone(),
                    file_uri: passage.file_uri.clone(),
                    defined_at_line: line_no,
                    body,
                });
            }
        }
    }

    callables
}

/// Compute the argument count for a custom macro from its handler body.
///
/// Scans for `this.args[N]` patterns and returns the highest index + 1.
/// If no `this.args` patterns are found, returns None (unknown/variadic).
fn compute_macro_arg_count(handler_body: &str, _handler_params: &str) -> Option<usize> {
    let mut max_idx: Option<usize> = None;
    for caps in RE_THIS_ARGS.captures_iter(handler_body) {
        let idx: usize = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
        max_idx = Some(match max_idx {
            None => idx + 1,
            Some(m) => m.max(idx + 1),
        });
    }
    max_idx
}

/// Extract the body content of a widget definition.
///
/// Looks for `<<widget name>>...<</widget>>` and returns the content between.
fn extract_widget_body(passage_body: &str, widget_name: &str) -> Option<String> {
    let open_tag = format!("<<widget {}>>", widget_name);
    let close_tag = "<</widget>>";

    let start = passage_body.find(&open_tag)?;
    let content_start = start + open_tag.len();
    let content_end = passage_body[content_start..].find(close_tag)?;
    Some(passage_body[content_start..content_start + content_end].to_string())
}

// ---------------------------------------------------------------------------
// Macro-to-JS translation (catalog-driven, block-aware)
// ---------------------------------------------------------------------------

/// Check if a passage body contains variable-affecting macros.
///
/// Uses the `MacroDef` catalog from `builtin_macros()` to identify which
/// macro names to check for, rather than hardcoding a list.
pub fn has_variable_macros(body: &str) -> bool {
    // Quick check for dollar references
    if body.contains('$') {
        return true;
    }

    if !body.contains("<<") {
        return false;
    }

    // Check against the builtin macro catalog
    for macro_def in crate::sugarcube::macros::builtin_macros() {
        if macro_def.name == "=" {
            if body.contains("<<=") {
                return true;
            }
        } else if macro_def.name == "-" {
            if body.contains("<<-") {
                return true;
            }
        } else if body.contains(&format!("<<{}", macro_def.name)) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Translation context
// ---------------------------------------------------------------------------

/// Context for macro-to-JS translation, holding the builtin catalog and
/// user callables for lookup during recursive descent.
pub(crate) struct TranslationContext<'a> {
    /// Lookup from macro name to its definition in the builtin catalog.
    pub(crate) builtin_lookup: HashMap<&'static str, &'static MacroDef>,
    /// Set of user-defined callable names (custom macros and widgets).
    pub(crate) callable_names: std::collections::HashSet<&'a str>,
}

impl<'a> TranslationContext<'a> {
    pub(crate) fn new(callables: &'a [UserCallable]) -> Self {
        let builtin_lookup: HashMap<&'static str, &'static MacroDef> =
            crate::sugarcube::macros::builtin_macros()
                .iter()
                .map(|m| (m.name, m))
                .collect();

        let callable_names: std::collections::HashSet<&'a str> = callables
            .iter()
            .map(|c| c.name.as_str())
            .collect();

        TranslationContext {
            builtin_lookup,
            callable_names,
        }
    }
}

/// Translate SugarCube macros in a passage body to JavaScript.
///
/// Uses the `MacroDef` catalog from `builtin_macros()` to identify macro
/// types, and implements a recursive descent translator that handles block
/// macros (like `<<if>>...<</if>>`) with proper structural awareness.
///
/// This produces faithful translations of built-in SugarCube macros:
///
/// - **Variable macros**: `<<set>>`, `<<unset>>`, `<<capture>>`, `<<run>>`
/// - **Control flow**: `<<if>>`/`<<elseif>>`/`<<else>>`/`<</if>>`,
///   `<<for>>`/`<</for>>`, `<<switch>>`/`<<case>>`/`<<default>>`/`<</switch>>`
/// - **Output**: `<<print>>`/`<<=>>`
/// - **Navigation**: `<<goto>>`, `<<include>>`
/// - **Forms**: `<<textbox>>`, `<<checkbox>>`, `<<numberbox>>`,
///   `<<radiobutton>>`, `<<textarea>>` (write to variables)
/// - **Deferred execution**: `<<link>>`/`<<button>>`/`<<timed>>`/`<<repeat>>`
///   (bodies are translated but marked as deferred)
/// - **Inline JS**: `<<script>>...<</script>>`
/// - **Custom macros & widgets**: `<<macroName args>>` → `macroName(args);`
///
/// The translation uses `OperatorNormalization` mappings to convert
/// TwineScript operators (e.g., `to` → `=`, `is` → `===`, `gte` → `>=`).
///
/// NOTE: Superseded by `walk_translate()` in `passage_tree.rs` which uses
/// the tree for exact line mapping. This function is retained for backward
/// compatibility and as a reference implementation.
#[allow(dead_code)] // Replaced by walk_translate() in passage_tree.rs (Phase 3)
pub fn translate_macros_to_js(body: &str, callables: &[UserCallable]) -> String {
    let ctx = TranslationContext::new(callables);
    translate_body(&ctx, body, 0)
}

// ---------------------------------------------------------------------------
// Recursive descent translator
// ---------------------------------------------------------------------------

/// Translate a full passage body (or block body) recursively.
///
/// Scans for macro tags using `RE_MACRO_TAG`, dispatches to the appropriate
/// handler based on the `MacroDef` catalog, and recursively translates
/// block macro bodies.
///
/// NOTE: Superseded by `walk_translate()` in `passage_tree.rs`.
#[allow(dead_code)] // Replaced by walk_translate() in passage_tree.rs (Phase 3)
fn translate_body(ctx: &TranslationContext, text: &str, indent: usize) -> String {
    let mut result = String::new();
    let indent_str = "  ".repeat(indent);
    let mut pos = 0;

    while pos < text.len() {
        let remaining = &text[pos..];

        // Find the next macro tag
        match RE_MACRO_TAG.captures(remaining) {
            Some(caps) => {
                let full_match = caps.get(0).unwrap();
                let tag_start = full_match.start();
                let tag_end = full_match.end();

                // Emit text before this tag
                if tag_start > 0 {
                    let text_before = &remaining[..tag_start];
                    result.push_str(&translate_text_segment(text_before, indent));
                }

                let is_close = !caps.get(1).unwrap().as_str().is_empty();
                let name = caps.get(2).unwrap().as_str();
                let args = caps.get(3).map(|m| m.as_str()).unwrap_or("");

                if is_close {
                    // Close tag at this level — shouldn't normally happen in
                    // translate_body (block close tags are consumed by the
                    // block handler).  Emit as a stray comment for safety.
                    result.push_str(&format!(
                        "{}/* stray close: /{} */\n",
                        indent_str, name
                    ));
                    pos += tag_end;
                } else if is_block_macro(ctx, name) {
                    // Block macro — find matching close tag and translate body
                    let body_start = pos + tag_end;
                    match find_matching_close(&text[body_start..], name) {
                        Some(body_len) => {
                            let body_text = &text[body_start..body_start + body_len];
                            let close_tag_len = format!("<</{}>>", name).len();

                            // Special handling for <<script>> blocks (raw JS)
                            if name == "script" {
                                let translated_js =
                                    translate_dollar_refs_in_js(body_text);
                                for line in translated_js.lines() {
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        result.push_str(&format!(
                                            "{}{}\n",
                                            indent_str, trimmed
                                        ));
                                    }
                                }
                            } else {
                                result.push_str(&translate_block_open(
                                    ctx, name, args, indent,
                                ));
                                result.push_str(&translate_body(
                                    ctx,
                                    body_text,
                                    indent + 1,
                                ));
                                result.push_str(&translate_close_tag(name, indent));
                            }

                            pos = body_start + body_len + close_tag_len;
                        }
                        None => {
                            // No matching close tag — produce the block open
                            // without a body (incomplete block, common in tests)
                            result.push_str(&translate_block_open(
                                ctx, name, args, indent,
                            ));
                            pos += tag_end;
                        }
                    }
                } else if ctx.builtin_lookup.contains_key(name) {
                    // Inline builtin macro
                    result.push_str(&translate_inline_macro(ctx, name, args, indent));
                    pos += tag_end;
                } else if ctx.callable_names.contains(name) {
                    // User-defined callable
                    let translated_args = if args.is_empty() {
                        String::new()
                    } else {
                        translate_callable_args(args)
                    };
                    if translated_args.is_empty() {
                        result.push_str(&format!("{}{}();\n", indent_str, name));
                    } else {
                        result.push_str(&format!(
                            "{}{}({});\n",
                            indent_str, name, translated_args
                        ));
                    }
                    pos += tag_end;
                } else {
                    // Unknown macro
                    let full_tag = full_match.as_str();
                    result.push_str(&format!(
                        "{}/* unknown: {} */;\n",
                        indent_str, full_tag
                    ));
                    pos += tag_end;
                }
            }
            None => {
                // No more macro tags — emit remaining text
                if !remaining.is_empty() {
                    result.push_str(&translate_text_segment(remaining, indent));
                }
                break;
            }
        }
    }

    result
}

/// Check if a macro name is a block macro (`has_body: true`) in the catalog.
///
/// Also handles the special `<<when>>` pseudo-macro (backward compat).
pub(crate) fn is_block_macro(ctx: &TranslationContext, name: &str) -> bool {
    if name == "when" {
        return true;
    }
    ctx.builtin_lookup
        .get(name)
        .map(|m| m.has_body)
        .unwrap_or(false)
}

/// Find the position of the matching close tag for a block macro.
///
/// Scans the text for macro tags, tracking nesting depth for the given
/// macro name.  Returns the byte offset of the start of the close tag
/// relative to `text`, or `None` if no matching close tag is found.
///
/// NOTE: Superseded by the tree-based approach in `passage_tree.rs`.
#[allow(dead_code)] // No longer needed — tree has nesting
fn find_matching_close(text: &str, macro_name: &str) -> Option<usize> {
    let mut depth = 1;
    let mut pos = 0;

    while pos < text.len() {
        let remaining = &text[pos..];

        if let Some(caps) = RE_MACRO_TAG.captures(remaining) {
            let full_match = caps.get(0).unwrap();
            let match_offset = full_match.start();
            let is_close = !caps.get(1).unwrap().as_str().is_empty();
            let name = caps.get(2).unwrap().as_str();

            if is_close && name == macro_name {
                depth -= 1;
                if depth == 0 {
                    return Some(pos + match_offset);
                }
            } else if !is_close && name == macro_name {
                depth += 1;
            }

            pos += match_offset + full_match.len();
        } else {
            break;
        }
    }

    None
}

/// Translate the opening of a block macro (produces the JS header + `{`).
pub(crate) fn translate_block_open(
    _ctx: &TranslationContext,
    name: &str,
    args: &str,
    indent: usize,
) -> String {
    let indent_str = "  ".repeat(indent);

    match name {
        // Control flow
        "if" => {
            let translated_cond = translate_expression(args);
            format!("{}if ({}) {{\n", indent_str, translated_cond)
        }
        "when" => {
            let translated_cond = translate_expression(args);
            format!("{}/* when */ if ({}) {{\n", indent_str, translated_cond)
        }
        "for" => {
            let header = translate_for_macro(args);
            format!("{}{}\n", indent_str, header)
        }
        "switch" => {
            let translated_expr = translate_expression(args);
            format!("{}switch ({}) {{\n", indent_str, translated_expr)
        }

        // Variables
        "capture" => {
            let var_ref = extract_dollar_var_from_args(args);
            match var_ref {
                Some(v) => {
                    format!("{}{{ /* capture State.variables.{} */\n", indent_str, v)
                }
                None => format!("{}{{ /* capture */\n", indent_str),
            }
        }

        // Deferred execution (link, button, timed, repeat)
        "link" => {
            let translated_args = translate_expression(args);
            format!("{}/* deferred: link({}) */ {{\n", indent_str, translated_args)
        }
        "button" => {
            let translated_args = translate_expression(args);
            format!(
                "{}/* deferred: button({}) */ {{\n",
                indent_str, translated_args
            )
        }
        "timed" => {
            format!(
                "{}/* deferred: timed({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "repeat" => {
            format!(
                "{}/* deferred: repeat({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "linkappend" => {
            let translated_args = translate_expression(args);
            format!(
                "{}/* deferred: linkappend({}) */ {{\n",
                indent_str, translated_args
            )
        }
        "linkprepend" => {
            let translated_args = translate_expression(args);
            format!(
                "{}/* deferred: linkprepend({}) */ {{\n",
                indent_str, translated_args
            )
        }
        "linkreplace" => {
            let translated_args = translate_expression(args);
            format!(
                "{}/* deferred: linkreplace({}) */ {{\n",
                indent_str, translated_args
            )
        }
        "actions" => {
            format!(
                "{}/* deferred: actions({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "click" => {
            let translated_args = translate_expression(args);
            format!(
                "{}/* deferred: click({}) */ {{\n",
                indent_str, translated_args
            )
        }

        // DOM
        "append" => {
            format!(
                "{}/* dom: append({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "prepend" => {
            format!(
                "{}/* dom: prepend({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "replace" => {
            format!(
                "{}/* dom: replace({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "copy" => {
            format!("{}/* dom: copy({}) */ {{\n", indent_str, args.trim())
        }

        // Output / utility
        "nobr" => format!("{}/* nobr */ {{\n", indent_str),
        "silently" => format!("{}/* silently */ {{\n", indent_str),
        "type" => {
            format!(
                "{}/* output: type({}) */ {{\n",
                indent_str,
                args.trim()
            )
        }
        "done" => format!("{}/* done */ {{\n", indent_str),
        "css" => format!("{}/* css */ {{\n", indent_str),

        // Widget definition
        "widget" => {
            format!("{}/* widget: {} */ {{\n", indent_str, args.trim())
        }

        // Audio
        "createplaylist" => format!("{}/* audio: createplaylist */ {{\n", indent_str),

        // Default: generic block
        _ => {
            format!(
                "{}/* block: {}({}) */ {{\n",
                indent_str,
                name,
                args.trim()
            )
        }
    }
}

/// Translate a close tag for a block macro.
pub(crate) fn translate_close_tag(name: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);

    match name {
        "if" | "when" | "for" | "switch" | "capture" => {
            format!("{}}}\n", indent_str)
        }
        "link" => format!("{}}} /* end link */\n", indent_str),
        "button" => format!("{}}} /* end button */\n", indent_str),
        "timed" => format!("{}}} /* end timed */\n", indent_str),
        "repeat" => format!("{}}} /* end repeat */\n", indent_str),
        "linkappend" => format!("{}}} /* end linkappend */\n", indent_str),
        "linkprepend" => format!("{}}} /* end linkprepend */\n", indent_str),
        "linkreplace" => format!("{}}} /* end linkreplace */\n", indent_str),
        "actions" => format!("{}}} /* end actions */\n", indent_str),
        "click" => format!("{}}} /* end click */\n", indent_str),
        "nobr" => format!("{}}} /* end nobr */\n", indent_str),
        "silently" => format!("{}}} /* end silently */\n", indent_str),
        "append" => format!("{}}} /* end append */\n", indent_str),
        "prepend" => format!("{}}} /* end prepend */\n", indent_str),
        "replace" => format!("{}}} /* end replace */\n", indent_str),
        "copy" => format!("{}}} /* end copy */\n", indent_str),
        "widget" => format!("{}}} /* end widget */\n", indent_str),
        "type" => format!("{}}} /* end type */\n", indent_str),
        "done" => format!("{}}} /* end done */\n", indent_str),
        "css" => format!("{}}} /* end css */\n", indent_str),
        "createplaylist" => format!("{}}} /* end createplaylist */\n", indent_str),
        _ => format!("{}}} /* end {} */\n", indent_str, name),
    }
}

/// Translate an inline (non-block) builtin macro.
pub(crate) fn translate_inline_macro(
    _ctx: &TranslationContext,
    name: &str,
    args: &str,
    indent: usize,
) -> String {
    let indent_str = "  ".repeat(indent);

    match name {
        // Variables
        "set" => translate_set_macro(args, indent),
        "unset" => translate_unset_macro(args, indent),
        "run" => translate_run_macro(args, indent),

        // Control flow branches (inside <<if>> blocks)
        "elseif" => {
            let translated_cond = translate_expression(args);
            let parent_indent = "  ".repeat(indent.saturating_sub(1));
            format!("{}}} else if ({}) {{\n", parent_indent, translated_cond)
        }
        "else" => {
            let parent_indent = "  ".repeat(indent.saturating_sub(1));
            format!("{}}} else {{\n", parent_indent)
        }

        // Control flow keywords
        "break" => format!("{}break;\n", indent_str),
        "continue" => format!("{}continue;\n", indent_str),

        // Switch internals (inside <<switch>> blocks)
        "case" => {
            let val = args.trim();
            format!("{}case {}:\n", indent_str, val)
        }
        "default" => format!("{}default:\n", indent_str),

        // Output
        "print" | "=" | "-" => {
            let translated_expr = translate_expression(args);
            format!("{}/* print: {} */;\n", indent_str, translated_expr)
        }

        // Navigation
        "goto" => {
            let translated_expr = translate_expression(args);
            format!("{}/* navigation: goto {} */;\n", indent_str, translated_expr)
        }
        "include" => {
            let translated_expr = translate_expression(args);
            format!("{}/* include: {} */;\n", indent_str, translated_expr)
        }
        "back" => format!("{}/* navigation: back */;\n", indent_str),
        "return" => format!("{}/* navigation: return */;\n", indent_str),

        // Forms
        "textbox" => translate_form_macro(args, "textbox", indent),
        "numberbox" => translate_form_macro(args, "numberbox", indent),
        "checkbox" => translate_form_macro(args, "checkbox", indent),
        "radiobutton" => translate_form_macro(args, "radiobutton", indent),
        "textarea" => translate_form_macro(args, "textarea", indent),

        // DOM (inline)
        "remove" => {
            format!("{}/* dom: remove({}) */;\n", indent_str, args.trim())
        }
        "addclass" => {
            format!(
                "{}/* dom: addclass({}) */;\n",
                indent_str,
                args.trim()
            )
        }
        "removeclass" => {
            format!(
                "{}/* dom: removeclass({}) */;\n",
                indent_str,
                args.trim()
            )
        }
        "toggleclass" => {
            format!(
                "{}/* dom: toggleclass({}) */;\n",
                indent_str,
                args.trim()
            )
        }

        // Audio
        "audio" => format!("{}/* audio: {} */;\n", indent_str, args.trim()),
        "playlist" => {
            format!(
                "{}/* audio: playlist({}) */;\n",
                indent_str,
                args.trim()
            )
        }
        "masteraudio" => {
            format!(
                "{}/* audio: masteraudio({}) */;\n",
                indent_str,
                args.trim()
            )
        }
        "cacheaudio" => {
            format!(
                "{}/* audio: cacheaudio({}) */;\n",
                indent_str,
                args.trim()
            )
        }
        "waitforaudio" => format!("{}/* audio: waitforaudio */;\n", indent_str),
        "stop" => format!("{}/* timing: stop */;\n", indent_str),

        // Deprecated
        "display" => {
            let translated_expr = translate_expression(args);
            format!("{}/* include: {} */;\n", indent_str, translated_expr)
        }
        "remember" => translate_set_macro(args, indent),
        "forget" => translate_unset_macro(args, indent),

        // Fallback for any builtin not explicitly handled
        _ => {
            format!(
                "{}/* builtin: {}({}) */;\n",
                indent_str,
                name,
                args.trim()
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Per-macro inline translators
// ---------------------------------------------------------------------------

/// Translate `<<set ...>>` macro arguments.
///
/// Handles three forms:
/// - `$var to expr` → `State.variables.var = translated_expr;`
/// - `$var = expr` → `State.variables.var = translated_expr;`
/// - `$var += expr` → `State.variables.var += translated_expr;`
pub(crate) fn translate_set_macro(args: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);
    let args = args.trim();

    // Try compound assignment: $var += expr, $var -= expr, etc.
    let re_compound = Regex::new(
        r"^\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)*)\s*([\+\-\*\/%])=\s*([\s\S]*)$",
    )
    .unwrap();
    if let Some(caps) = re_compound.captures(args) {
        let var_path = caps.get(1).unwrap().as_str();
        let op = caps.get(2).unwrap().as_str();
        let expr = caps.get(3).unwrap().as_str().trim();
        let js_path = dollar_path_to_state_path(var_path);
        let translated_expr = translate_expression(expr);
        return format!("{}{} {}= {};\n", indent_str, js_path, op, translated_expr);
    }

    // Try "to" assignment: $var to expr
    let re_to = Regex::new(
        r"^\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)*)\s+to\s+([\s\S]*)$",
    )
    .unwrap();
    if let Some(caps) = re_to.captures(args) {
        let var_path = caps.get(1).unwrap().as_str();
        let expr = caps.get(2).unwrap().as_str().trim();
        let js_path = dollar_path_to_state_path(var_path);
        let translated_expr = translate_expression(expr);
        return format!("{}{} = {};\n", indent_str, js_path, translated_expr);
    }

    // Try "=" assignment: $var = expr
    let re_eq = Regex::new(
        r"^\$([A-Za-z\$_][A-Za-z0-9\$_]*(?:\.[A-Za-z\$_][A-Za-z0-9\$_]*)*)\s*=\s*([\s\S]*)$",
    )
    .unwrap();
    if let Some(caps) = re_eq.captures(args) {
        let var_path = caps.get(1).unwrap().as_str();
        let expr = caps.get(2).unwrap().as_str().trim();
        let js_path = dollar_path_to_state_path(var_path);
        let translated_expr = translate_expression(expr);
        return format!("{}{} = {};\n", indent_str, js_path, translated_expr);
    }

    // Fallback: translate the whole args as an expression
    let translated = translate_expression(args);
    format!("{}{};\n", indent_str, translated)
}

/// Translate `<<unset $var>>` macro arguments.
pub(crate) fn translate_unset_macro(args: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);
    let var_ref = extract_dollar_var_from_args(args);
    match var_ref {
        Some(v) => format!("{}delete State.variables.{};\n", indent_str, v),
        None => format!("{}/* unset: {} */;\n", indent_str, args.trim()),
    }
}

/// Translate `<<run jscode>>` macro arguments.
pub(crate) fn translate_run_macro(args: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);
    let translated_code = translate_dollar_refs_in_js(args);
    format!("{}{};\n", indent_str, translated_code.trim())
}

/// Translate a form macro (textbox, numberbox, etc.) that writes to a variable.
pub(crate) fn translate_form_macro(args: &str, macro_name: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);
    let var_ref = extract_dollar_var_from_args(args);
    match var_ref {
        Some(v) => {
            let js_path = format!("State.variables.{}", v);
            format!("{}{} = /* {} input */;\n", indent_str, js_path, macro_name)
        }
        None => format!("{}/* {}({}) */;\n", indent_str, macro_name, args.trim()),
    }
}

/// Extract the first `$varName` from macro arguments, returning the name
/// without the `$` sigil.
///
/// Returns `None` if the match is a `$$` escape (e.g., `$$name` should not
/// be treated as a variable reference).
pub(crate) fn extract_dollar_var_from_args(args: &str) -> Option<String> {
    // Guard $$ escape before matching
    const DOLLAR_ESCAPE_SENTINEL: &str = "\u{E000}\u{E001}";
    let guarded = args.replace("$$", DOLLAR_ESCAPE_SENTINEL);

    RE_VD_DOLLAR_REF
        .captures(&guarded)
        .map(|caps| caps.get(1).unwrap().as_str().to_string())
}

// ---------------------------------------------------------------------------
// Text segment translation
// ---------------------------------------------------------------------------

/// Translate a text segment between macro tags.
///
/// Handles `$var` references, `$var++`/`$var--`, `$$` escape markup, and
/// plain text. The `$$` escape is SugarCube's way of outputting a literal
/// `$` character — `$$name` outputs `$name` and must NOT be treated as a
/// variable reference.
pub(crate) fn translate_text_segment(text: &str, indent: usize) -> String {
    let indent_str = "  ".repeat(indent);
    let mut result = String::new();

    // Sentinel for $$ escape (same approach as translate_dollar_refs_in_js)
    const DOLLAR_ESCAPE_SENTINEL: &str = "\u{E000}\u{E001}";

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        // Guard $$ escape before any $-based replacement
        let mut processed = trimmed.replace("$$", DOLLAR_ESCAPE_SENTINEL);

        // Handle $var++ / ++$var
        processed = RE_VD_INCREMENT
            .replace_all(&processed, |caps: &regex::Captures| {
                let var_name = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .unwrap()
                    .as_str();
                format!("State.variables.{}++", var_name)
            })
            .to_string();

        // Handle $var-- / --$var
        processed = RE_VD_DECREMENT
            .replace_all(&processed, |caps: &regex::Captures| {
                let var_name = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .unwrap()
                    .as_str();
                format!("State.variables.{}--", var_name)
            })
            .to_string();

        // Handle $var references (reads)
        if processed.contains('$') && !processed.contains("State.variables.") {
            processed = RE_VD_DOLLAR_REF
                .replace_all(&processed, |caps: &regex::Captures| {
                    let var_path = caps.get(1).unwrap().as_str();
                    let js_path = dollar_path_to_state_path(var_path);
                    format!("/* read: {} */", js_path)
                })
                .to_string();
        }

        // Restore $$ escape sentinel → single literal $
        processed = processed.replace(DOLLAR_ESCAPE_SENTINEL, "$");

        if processed.contains("State.variables.") || trimmed.contains("$$") || trimmed.contains('$') {
            result.push_str(&format!("{}{}\n", indent_str, processed));
        } else {
            result.push_str(&format!("{}/* text: {} */\n", indent_str, processed));
        }
    }

    result
}

/// Split SugarCube macro arguments into individual tokens, respecting quoted
/// strings and bracket nesting.
///
/// SugarCube macros use space-separated args: `<<myWidget "foo" "bar" 5 $var>>`
/// In JS, these need to become comma-separated: `myWidget("foo", "bar", 5, State.variables.var)`
///
/// This function tokenizes the args string, handling:
/// - Double-quoted strings: `"hello world"` → single token
/// - Single-quoted strings: `'hello world'` → single token
/// - Backtick template literals: `` `template` `` → single token
/// - Bracketed expressions: `($x + 5)` → single token (tracks depth)
/// - Bare words and numbers: split on whitespace
fn split_macro_args(args: &str) -> Vec<String> {
    let args = args.trim();
    if args.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = args.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' => {
                // Whitespace: end current token if any
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
                chars.next();
            }
            '"' | '\'' => {
                // Quoted string: consume until matching close quote
                let quote = ch;
                current.push(ch);
                chars.next();
                let mut escaped = false;
                while let Some(&c) = chars.peek() {
                    current.push(c);
                    chars.next();
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if c == '\\' {
                        escaped = true;
                        continue;
                    }
                    if c == quote {
                        break;
                    }
                }
            }
            '`' => {
                // Template literal: consume until matching backtick
                current.push(ch);
                chars.next();
                while let Some(&c) = chars.peek() {
                    current.push(c);
                    chars.next();
                    if c == '`' {
                        break;
                    }
                }
            }
            '(' | '[' | '{' => {
                // Opening bracket: track nesting depth
                let close = match ch {
                    '(' => ')',
                    '[' => ']',
                    '{' => '}',
                    _ => unreachable!(),
                };
                let mut depth: u32 = 1;
                current.push(ch);
                chars.next();
                while let Some(&c) = chars.peek() {
                    current.push(c);
                    chars.next();
                    if c == ch {
                        depth += 1;
                    } else if c == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            _ => {
                // Regular character
                current.push(ch);
                chars.next();
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Translate macro args for a user-defined callable (widget/custom macro).
///
/// Splits space-separated SugarCube args and joins them with commas for
/// valid JS function call syntax.
///
/// Examples:
/// - `<<myWidget "foo" "bar">>` → `"foo", "bar"`
/// - `<<addTime 5 "hours">>` → `5, "hours"`
/// - `<<useItem $itemName>>` → `State.variables.itemName`
/// - `<<custom $x $y 10>>` → `State.variables.x, State.variables.y, 10`
pub(crate) fn translate_callable_args(args: &str) -> String {
    let tokens = split_macro_args(args);
    if tokens.is_empty() {
        return String::new();
    }

    // Translate each token individually (for $var refs and operator normalization)
    let translated: Vec<String> = tokens.iter()
        .map(|t| translate_expression(t))
        .collect();

    translated.join(", ")
}

/// Translate an expression containing TwineScript operators and `$var` refs to JS.
///
/// Applies `OperatorNormalization` mappings (e.g., `to` → `=`, `is` → `===`,
/// `gte` → `>=`) and converts `$var` to `State.variables.var`.
pub(crate) fn translate_expression(expr: &str) -> String {
    let mut result = expr.to_string();

    // Apply operator normalizations (longest-first to avoid partial matches)
    let normalizations = crate::sugarcube::macros::operator_normalization();
    // Sort by length descending to avoid partial matches (e.g., "isnot" before "is")
    let mut sorted: Vec<_> = normalizations.iter().collect();
    sorted.sort_by(|a, b| b.from.len().cmp(&a.from.len()));

    for norm in sorted {
        // Only replace whole-word operator occurrences
        // Use word boundaries to avoid replacing "to" inside "into"
        let pattern = format!(r"\b{}\b", regex::escape(norm.from));
        if let Ok(re) = Regex::new(&pattern) {
            result = re.replace_all(&result, norm.to).to_string();
        }
    }

    // Translate $var references to State.variables.var
    result = translate_dollar_refs_in_js(&result);

    result
}

/// Translate a `<<for>>` macro's arguments to a JS for-loop header.
///
/// SugarCube `<<for>>` has several forms:
/// - `<<for _i to 0; _i lt $arr.length; _i++>>` (C-style)
/// - `<<for _i, $arr>>` (iterate over array: index, value)
/// - `<<for $i, $arr>>` (iterate over array: value only)
/// - `<<for $i, $arr range>>` (range iteration)
/// - `<<for _i, $obj>>` (iterate over object keys)
pub(crate) fn translate_for_macro(for_args: &str) -> String {
    let args = for_args.trim();

    // Check for C-style for loop (contains semicolons)
    if args.contains(';') {
        let translated = translate_expression(args);
        return format!("for ({}) {{", translated);
    }

    // Check for "range" keyword
    if args.contains("range") {
        let translated = translate_expression(args);
        return format!("for (var _i in {}) {{", translated);
    }

    // Simple iteration: _i, $arr or $i, $arr
    // Split on comma
    let parts: Vec<&str> = args.splitn(2, ',').collect();
    if parts.len() == 2 {
        let index_var = parts[0].trim();
        let iter_expr = parts[1].trim();
        let translated_iter = translate_expression(iter_expr);
        if index_var.starts_with('_') {
            // _i, $arr → for (var _i = 0; _i < $arr.length; _i++)
            format!("for ({} in {}) {{", index_var, translated_iter)
        } else if index_var.starts_with('$') {
            // $val, $arr → for (var _i = 0; _i < $arr.length; _i++) { var $val = $arr[_i]; }
            let js_path = dollar_path_to_state_path(&index_var[1..]);
            format!("for (var _i in {}) {{ {} = {}[_i];", translated_iter, js_path, translated_iter)
        } else {
            format!("for ({} in {}) {{", index_var, translated_iter)
        }
    } else {
        // Single expression — translate as-is
        let translated = translate_expression(args);
        format!("for ({}) {{", translated)
    }
}

/// Convert a `$var.path` reference to `State.variables.var.path`.
pub(crate) fn dollar_path_to_state_path(dollar_path: &str) -> String {
    if dollar_path.starts_with('$') {
        format!("State.variables.{}", &dollar_path[1..])
    } else {
        format!("State.variables.{}", dollar_path)
    }
}

/// Translate `$var` references within `<<run>>` macro JS bodies.
///
/// Inside `<<run>>` bodies, SugarCube allows `$var` as shorthand for
/// `State.variables.var`. This function translates those references
/// while preserving other JS code.
///
/// ## `$$` Escape Handling
///
/// The `$$` markup escapes the `$` sigil in SugarCube — `$$name` outputs
/// literal `$name` and is NOT a variable reference. This function must
/// NOT translate `$$var` into `State.variables.var`. We handle this by
/// temporarily replacing `$$` with a sentinel, translating `$var` refs,
/// then restoring the sentinel back to `$$`.
pub(crate) fn translate_dollar_refs_in_js(js_code: &str) -> String {
    // Guard: replace $$ with a sentinel to prevent false matches.
    // We use a Unicode private-use character that cannot appear in
    // SugarCube source text. This avoids the need for lookbehind
    // (which the `regex` crate doesn't support).
    const DOLLAR_ESCAPE_SENTINEL: &str = "\u{E000}\u{E001}";
    let guarded = js_code.replace("$$", DOLLAR_ESCAPE_SENTINEL);

    let translated = RE_VD_DOLLAR_REF
        .replace_all(&guarded, |caps: &regex::Captures| {
            let var_path = caps.get(1).unwrap().as_str();
            dollar_path_to_state_path(var_path)
        })
        .to_string();

    // Restore the $$ escape markup (the user wants literal $, not a
    // variable reference). After sentinel restoration, `$$name` becomes
    // just `$name` in the translated output (the first $ is literal).
    translated.replace(DOLLAR_ESCAPE_SENTINEL, "$")
}

// NOTE: build_macro_line_map() has been moved to the core builder
// at crate::virtual_doc::build_format_section_line_map().

// ---------------------------------------------------------------------------
// Helpers (delegate to core module)
// ---------------------------------------------------------------------------

/// Strip JS comments from source text before alias extraction.
/// Delegates to the core virtual_doc module (format-agnostic).
fn strip_comments(src: &str) -> String {
    crate::virtual_doc::strip_comments(src)
}

/// Compute 0-based line number from a byte offset in a string.
/// Delegates to the core virtual_doc module (format-agnostic).
fn line_from_offset(text: &str, offset: usize) -> u32 {
    crate::virtual_doc::line_from_offset(text, offset)
}

// ---------------------------------------------------------------------------
// Cross-section variable extraction
// ---------------------------------------------------------------------------

/// A variable access extracted from the virtual document, with full context.
///
/// Unlike `VarOp` (which only tracks span within a single passage), this
/// type carries the resolved access path and the original source location
/// derived from the virtual document's line map.
#[derive(Debug, Clone)]
pub struct VirtualVarAccess {
    /// The normalized access path (e.g., "State.variables.player.name").
    pub access_path: String,
    /// The SugarCube dollar-name (e.g., "$player.name").
    pub dollar_name: String,
    /// Whether this is a write (true) or read (false).
    pub is_write: bool,
    /// The passage name where this access occurs.
    pub passage_name: String,
    /// The file URI where this access occurs.
    pub file_uri: String,
    /// The 0-based line number in the original source file.
    pub original_line: u32,
}

/// Extract all variable accesses from the virtual document, resolving
/// startup aliases in macro sections.
///
/// This is the main extraction function that replaces per-passage extraction
/// for the purpose of building the variable tree. It:
/// 1. Extracts from the unified script section (with full JS analysis)
/// 2. Extracts from each macro section (with startup alias resolution)
/// 3. Resolves `gs()`/`reg()`/alias patterns using the startup alias table
/// Note: No longer called from production code. Replaced by tree-based walks.
#[allow(dead_code)] // Replaced by walk_vars()/walk_passage_var_refs() in passage_tree.rs (Phase 4)
pub fn extract_virtual_var_accesses(
    vdoc: &crate::types::VirtualDocument,
) -> Vec<VirtualVarAccess> {
    let mut accesses: Vec<VirtualVarAccess> = Vec::new();

    // Build a quick lookup for whole-object aliases
    let whole_aliases: std::collections::HashMap<&str, &StartupAlias> = vdoc
        .startup_aliases
        .iter()
        .filter(|a| matches!(a.resolution, AliasResolution::StateVariables))
        .map(|a| (a.alias_name.as_str(), a))
        .collect();

    // Build a quick lookup for getter function aliases
    let getter_aliases: std::collections::HashMap<&str, &StartupAlias> = vdoc
        .startup_aliases
        .iter()
        .filter(|a| matches!(a.resolution, AliasResolution::GetterFunction))
        .map(|a| (a.alias_name.as_str(), a))
        .collect();

    // Build a quick lookup for specific property aliases
    let prop_aliases: std::collections::HashMap<&str, &StartupAlias> = vdoc
        .startup_aliases
        .iter()
        .filter(|a| matches!(a.resolution, AliasResolution::StateVariableProperty { .. }))
        .map(|a| (a.alias_name.as_str(), a))
        .collect();

    for section in &vdoc.sections {
        match &section.kind {
            VirtualSectionKind::UnifiedScript => {
                extract_from_script_section(section, &mut accesses);
            }
            VirtualSectionKind::MacroTranslated { passage_name } => {
                extract_from_macro_section(
                    section,
                    passage_name,
                    &whole_aliases,
                    &getter_aliases,
                    &prop_aliases,
                    &mut accesses,
                );
            }
        }
    }

    accesses
}

/// Extract variable accesses from the unified script section.
///
/// Script passages use pure JS, so we look for:
/// - `State.variables.x` patterns (read and write)
/// - Whole-object alias property accesses (e.g., `v.x` where `v = State.variables`)
/// - Getter function calls (e.g., `reg('UI_PROFILES')`)
fn extract_from_script_section(
    section: &VirtualSection,
    accesses: &mut Vec<VirtualVarAccess>,
) {
    let stripped = strip_comments(&section.source_text);

    // ── Direct State.variables patterns ────────────────────────────────

    // State.variables.name = value (write)
    for caps in RE_VD_ALIAS_SPECIFIC.captures_iter(&stripped) {
        let _alias = caps.get(1).unwrap().as_str();
        let prop = caps.get(2).unwrap().as_str();
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(&stripped, match_start);

        // Check if this is a write (look for = after the match, but not == or ===)
        let after = stripped[caps.get(0).unwrap().end()..].trim_start();
        let is_write = after.starts_with('=') && !after.starts_with("==") && !after.starts_with("===");

        if let Some(mapping) = section.line_map.get(line_no as usize) {
            let dollar_name = format!("${}", prop);
            accesses.push(VirtualVarAccess {
                access_path: format!("State.variables.{}", prop),
                dollar_name,
                is_write,
                passage_name: mapping.passage_name.clone(),
                file_uri: mapping.file_uri.clone(),
                original_line: mapping.original_line,
            });
        }
    }
}

/// Extract variable accesses from a macro-translated section, resolving
/// startup aliases.
fn extract_from_macro_section(
    section: &VirtualSection,
    passage_name: &str,
    whole_aliases: &std::collections::HashMap<&str, &StartupAlias>,
    getter_aliases: &std::collections::HashMap<&str, &StartupAlias>,
    prop_aliases: &std::collections::HashMap<&str, &StartupAlias>,
    accesses: &mut Vec<VirtualVarAccess>,
) {
    let js_text = &section.source_text;

    // ── Direct State.variables patterns ────────────────────────────────
    // These come from translated macros (<<set>>, <<run>>, etc.)

    // State.variables.x = value (write)
    static RE_SV_WRITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"State\.variables\.([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\s*=").unwrap()
    });

    // State.variables.x (read — no = after, or after /* read: */)
    static RE_SV_READ: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"State\.variables\.([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)").unwrap()
    });

    // ── Extract writes ─────────────────────────────────────────────────
    for caps in RE_SV_WRITE.captures_iter(js_text) {
        let prop_path = caps.get(1).unwrap().as_str();
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(js_text, match_start) as usize;

        let dollar_name = format!("${}", prop_path);
        let mapping = section.line_map.get(line_no);

        accesses.push(VirtualVarAccess {
            access_path: format!("State.variables.{}", prop_path),
            dollar_name,
            is_write: true,
            passage_name: mapping.map(|m| m.passage_name.clone()).unwrap_or_else(|| passage_name.to_string()),
            file_uri: mapping.map(|m| m.file_uri.clone()).unwrap_or_default(),
            original_line: mapping.map(|m| m.original_line).unwrap_or(0),
        });
    }

    // ── Extract reads ──────────────────────────────────────────────────
    for caps in RE_SV_READ.captures_iter(js_text) {
        let prop_path = caps.get(1).unwrap().as_str();
        let match_start = caps.get(0).unwrap().start();
        let line_no = line_from_offset(js_text, match_start) as usize;

        // Skip if this is a write (already captured above)
        let after = js_text[caps.get(0).unwrap().end()..].trim_start();
        if after.starts_with('=') && !after.starts_with("==") {
            continue; // This is a write, already handled
        }

        let dollar_name = format!("${}", prop_path);
        let mapping = section.line_map.get(line_no);

        accesses.push(VirtualVarAccess {
            access_path: format!("State.variables.{}", prop_path),
            dollar_name,
            is_write: false,
            passage_name: mapping.map(|m| m.passage_name.clone()).unwrap_or_else(|| passage_name.to_string()),
            file_uri: mapping.map(|m| m.file_uri.clone()).unwrap_or_default(),
            original_line: mapping.map(|m| m.original_line).unwrap_or(0),
        });
    }

    // ── Resolve alias-based accesses from <<run>> bodies ───────────────
    for alias in whole_aliases.keys() {
        let alias_dot = format!("{}.", alias);
        for (line_idx, line) in js_text.lines().enumerate() {
            if !line.contains(&alias_dot) {
                continue;
            }
            let re = Regex::new(&format!(
                r"{}\s*\.\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)",
                regex::escape(alias)
            ))
            .unwrap();

            for caps in re.captures_iter(line) {
                let prop_path = caps.get(1).unwrap().as_str();
                let after_match = &line[caps.get(0).unwrap().end()..];
                let is_write = after_match.trim_start().starts_with('=')
                    && !after_match.trim_start().starts_with("==")
                    && !after_match.trim_start().starts_with("===");

                let dollar_name = format!("${}", prop_path);
                let mapping = section.line_map.get(line_idx);

                accesses.push(VirtualVarAccess {
                    access_path: format!("State.variables.{}", prop_path),
                    dollar_name,
                    is_write,
                    passage_name: mapping.map(|m| m.passage_name.clone()).unwrap_or_else(|| passage_name.to_string()),
                    file_uri: mapping.map(|m| m.file_uri.clone()).unwrap_or_default(),
                    original_line: mapping.map(|m| m.original_line).unwrap_or(0),
                });
            }
        }
    }

    // ── Resolve getter function calls ──────────────────────────────────
    for alias in getter_aliases.keys() {
        let call_pattern = format!(r#"{}\s*\(\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]\s*\)"#, regex::escape(alias));
        let re = Regex::new(&call_pattern).unwrap();

        for (line_idx, line) in js_text.lines().enumerate() {
            for caps in re.captures_iter(line) {
                let var_name = caps.get(1).unwrap().as_str();
                let dollar_name = format!("${}", var_name);
                let mapping = section.line_map.get(line_idx);

                let after_match = &line[caps.get(0).unwrap().end()..];
                let is_write = after_match.trim_start().starts_with('=')
                    && !after_match.trim_start().starts_with("==")
                    && !after_match.trim_start().starts_with("===");

                accesses.push(VirtualVarAccess {
                    access_path: format!("State.variables.{}", var_name),
                    dollar_name,
                    is_write,
                    passage_name: mapping.map(|m| m.passage_name.clone()).unwrap_or_else(|| passage_name.to_string()),
                    file_uri: mapping.map(|m| m.file_uri.clone()).unwrap_or_default(),
                    original_line: mapping.map(|m| m.original_line).unwrap_or(0),
                });
            }
        }
    }

    // ── Resolve property aliases ───────────────────────────────────────
    for alias in prop_aliases.keys() {
        let alias_dot = format!("{}.", alias);
        for (line_idx, line) in js_text.lines().enumerate() {
            if !line.contains(&alias_dot) {
                continue;
            }
            let re = Regex::new(&format!(
                r"{}\s*\.\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)",
                regex::escape(alias)
            ))
            .unwrap();

            for caps in re.captures_iter(line) {
                let sub_prop = caps.get(1).unwrap().as_str();
                let mapping = section.line_map.get(line_idx);

                if let Some(alias_def) = prop_aliases.get(alias) {
                    if let AliasResolution::StateVariableProperty { base_name, property_path } = &alias_def.resolution {
                        let full_path = match property_path {
                            Some(pp) => format!("{}.{}.{}", base_name, pp, sub_prop),
                            None => format!("{}.{}", base_name, sub_prop),
                        };
                        let dollar_name = format!("${}", full_path);

                        let after_match = &line[caps.get(0).unwrap().end()..];
                        let is_write = after_match.trim_start().starts_with('=')
                            && !after_match.trim_start().starts_with("==")
                            && !after_match.trim_start().starts_with("===");

                        accesses.push(VirtualVarAccess {
                            access_path: format!("State.variables.{}", full_path),
                            dollar_name,
                            is_write,
                            passage_name: mapping.map(|m| m.passage_name.clone()).unwrap_or_else(|| passage_name.to_string()),
                            file_uri: mapping.map(|m| m.file_uri.clone()).unwrap_or_default(),
                            original_line: mapping.map(|m| m.original_line).unwrap_or(0),
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dollar_path_to_state_path() {
        assert_eq!(
            dollar_path_to_state_path("$player.name"),
            "State.variables.player.name"
        );
        assert_eq!(
            dollar_path_to_state_path("$gold"),
            "State.variables.gold"
        );
    }

    #[test]
    fn test_translate_set_to() {
        let body = "<<set $hp to 100>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.hp = 100;"));
    }

    #[test]
    fn test_translate_set_eq() {
        let body = "<<set $gold = 50>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.gold = 50;"));
    }

    #[test]
    fn test_translate_set_compound() {
        let body = "<<set $score += 10>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.score += 10;"));
    }

    #[test]
    fn test_translate_run() {
        let body = "<<run State.variables.hp -= 5>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.hp -= 5;"));
    }

    #[test]
    fn test_translate_unset() {
        let body = "<<unset $temp>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("delete State.variables.temp;"));
    }

    #[test]
    fn test_translate_if() {
        let body = "<<if $hp gte 10>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("if (State.variables.hp >= 10) {"));
    }

    #[test]
    fn test_translate_if_elseif_else() {
        let body = "<<if $hp gte 10>>\nhealthy\n<<elseif $hp gte 5>>\nwounded\n<<else>>\ncritical\n<</if>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("if (State.variables.hp >= 10) {"));
        assert!(js.contains("} else if (State.variables.hp >= 5) {"));
        assert!(js.contains("} else {"));
        assert!(js.contains("}"));
    }

    #[test]
    fn test_translate_for() {
        let body = "<<for _i, $items>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("for (_i in State.variables.items) {"));
    }

    #[test]
    fn test_translate_print() {
        let body = "<<print $player.name>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("/* print: State.variables.player.name */"));
    }

    #[test]
    fn test_translate_textbox() {
        let body = "<<textbox $name \"Enter name\">>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.name = /* textbox input */;"));
    }

    #[test]
    fn test_translate_custom_macro() {
        let callables = vec![UserCallable {
            name: "useItem".to_string(),
            kind: UserCallableKind::CustomMacro,
            arg_count: Some(1),
            defined_in: "Script".to_string(),
            file_uri: "file:///test.twee".to_string(),
            defined_at_line: 5,
            body: Some("var key = this.args[0];".to_string()),
        }];
        let body = "<<useItem matchbox>>";
        let js = translate_macros_to_js(body, &callables);
        assert!(js.contains("useItem(matchbox);"), "Got: {}", js);
    }

    #[test]
    fn test_translate_widget_invocation() {
        let callables = vec![UserCallable {
            name: "showInventory".to_string(),
            kind: UserCallableKind::Widget,
            arg_count: None,
            defined_in: "InventoryWidget".to_string(),
            file_uri: "file:///test.twee".to_string(),
            defined_at_line: 0,
            body: Some("<<print $inventory>>".to_string()),
        }];
        let body = "<<showInventory>>";
        let js = translate_macros_to_js(body, &callables);
        assert!(js.contains("showInventory();"), "Got: {}", js);
    }

    #[test]
    fn test_translate_dot_path() {
        let body = "<<set $player.name to \"Alice\">>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("State.variables.player.name = \"Alice\";"));
    }

    #[test]
    fn test_translate_switch() {
        let body = "<<switch $action>>\n<<case \"attack\">>\n<<default>>\n<</switch>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("switch (State.variables.action) {"));
        assert!(js.contains("case \"attack\":"));
        assert!(js.contains("default:"));
    }

    #[test]
    fn test_translate_link_deferred() {
        let body = "<<link \"Click\" \"Next\">><</link>>";
        let js = translate_macros_to_js(body, &[]);
        assert!(js.contains("/* deferred: link("));
        assert!(js.contains("} /* end link */"));
    }

    #[test]
    fn test_operator_normalization_in_expression() {
        let result = translate_expression("$hp gte 10 and $mp is 5");
        assert!(result.contains("State.variables.hp >= 10"));
        assert!(result.contains("State.variables.mp === 5"));
    }

    #[test]
    fn test_extract_user_callables_macro_add() {
        let passages = vec![PassageInfo {
            name: "Script".to_string(),
            file_uri: "file:///test.twee".to_string(),
            tags: vec!["script".to_string()],
            body_text: r#"
Macro.add('useItem', {
    handler: function () {
        var key = this.args[0];
        if (typeof key !== 'string') { return this.error('useItem: string key required'); }
        _dispatchItemUse(key);
    }
});
"#.to_string(),
        }];
        let callables = extract_user_callables(&passages);
        assert_eq!(callables.len(), 1);
        assert_eq!(callables[0].name, "useItem");
        assert_eq!(callables[0].kind, UserCallableKind::CustomMacro);
        assert_eq!(callables[0].arg_count, Some(1));
    }

    #[test]
    fn test_extract_user_callables_widget() {
        let passages = vec![PassageInfo {
            name: "InvWidget".to_string(),
            file_uri: "file:///test.twee".to_string(),
            tags: vec!["widget".to_string()],
            body_text: "<<widget showInventory>><<print $inventory>><</widget>>".to_string(),
        }];
        let callables = extract_user_callables(&passages);
        assert_eq!(callables.len(), 1);
        assert_eq!(callables[0].name, "showInventory");
        assert_eq!(callables[0].kind, UserCallableKind::Widget);
    }

    #[test]
    fn test_strip_comments() {
        let js = "var x = 5; // this is a comment\nvar y = State.variables.z; /* block comment */";
        let stripped = strip_comments(js);
        assert!(!stripped.contains("this is a comment"));
        assert!(!stripped.contains("block comment"));
        assert!(stripped.contains("var x = 5;"));
        assert!(stripped.contains("State.variables.z;"));
    }

    #[test]
    fn test_alias_extraction_whole() {
        let js = "var v = State.variables;\nv.gold = 10;";
        let section = VirtualSection {
            kind: VirtualSectionKind::UnifiedScript,
            source_text: js.to_string(),
            line_map: vec![
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 0 },
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 1 },
            ],
        };
        let aliases = extract_startup_aliases(&[section]);
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].alias_name, "v");
        assert_eq!(aliases[0].resolution, AliasResolution::StateVariables);
    }

    #[test]
    fn test_alias_extraction_gs() {
        let js = "var g = gs();\ng.meta.uiProfile = 'default';";
        let section = VirtualSection {
            kind: VirtualSectionKind::UnifiedScript,
            source_text: js.to_string(),
            line_map: vec![
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 0 },
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 1 },
            ],
        };
        let aliases = extract_startup_aliases(&[section]);
        assert!(aliases.iter().any(|a| a.alias_name == "g" && a.resolution == AliasResolution::StateVariables));
    }

    #[test]
    fn test_alias_extraction_named_getter() {
        let js = "function reg(name) { return State.variables[name]; }";
        let section = VirtualSection {
            kind: VirtualSectionKind::UnifiedScript,
            source_text: js.to_string(),
            line_map: vec![
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 0 },
            ],
        };
        let aliases = extract_startup_aliases(&[section]);
        assert!(aliases.iter().any(|a| a.alias_name == "reg" && matches!(a.resolution, AliasResolution::GetterFunction)));
    }

    #[test]
    fn test_alias_extraction_specific() {
        let js = "var profiles = State.variables.uiProfiles;";
        let section = VirtualSection {
            kind: VirtualSectionKind::UnifiedScript,
            source_text: js.to_string(),
            line_map: vec![
                crate::types::LineMapping { passage_name: "Script1".to_string(), file_uri: "file:///test.twee".to_string(), original_line: 0 },
            ],
        };
        let aliases = extract_startup_aliases(&[section]);
        assert!(aliases.iter().any(|a| a.alias_name == "profiles" && matches!(a.resolution, AliasResolution::StateVariableProperty { .. })));
    }

    // ── split_macro_args & translate_callable_args ────────────────────

    #[test]
    fn test_split_macro_args_simple() {
        let tokens = split_macro_args("\"foo\" \"bar\"");
        assert_eq!(tokens, vec!["\"foo\"", "\"bar\""]);
    }

    #[test]
    fn test_split_macro_args_numbers() {
        let tokens = split_macro_args("5 10");
        assert_eq!(tokens, vec!["5", "10"]);
    }

    #[test]
    fn test_split_macro_args_mixed() {
        let tokens = split_macro_args("\"foo\" 5 $var");
        assert_eq!(tokens, vec!["\"foo\"", "5", "$var"]);
    }

    #[test]
    fn test_split_macro_args_quoted_with_spaces() {
        let tokens = split_macro_args("\"hello world\" 42");
        assert_eq!(tokens, vec!["\"hello world\"", "42"]);
    }

    #[test]
    fn test_split_macro_args_bracketed() {
        let tokens = split_macro_args("($x + 5) 10");
        assert_eq!(tokens, vec!["($x + 5)", "10"]);
    }

    #[test]
    fn test_split_macro_args_single_quoted() {
        let tokens = split_macro_args("'hello' 'world'");
        assert_eq!(tokens, vec!["'hello'", "'world'"]);
    }

    #[test]
    fn test_split_macro_args_empty() {
        let tokens = split_macro_args("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_translate_callable_args_strings() {
        let result = translate_callable_args("\"foo\" \"bar\"");
        assert_eq!(result, "\"foo\", \"bar\"");
    }

    #[test]
    fn test_translate_callable_args_vars() {
        let result = translate_callable_args("$itemName");
        assert_eq!(result, "State.variables.itemName");
    }

    #[test]
    fn test_translate_callable_args_mixed() {
        let result = translate_callable_args("\"sword\" $gold 10");
        assert_eq!(result, "\"sword\", State.variables.gold, 10");
    }

    #[test]
    fn test_translate_callable_args_empty() {
        let result = translate_callable_args("");
        assert!(result.is_empty());
    }
}
