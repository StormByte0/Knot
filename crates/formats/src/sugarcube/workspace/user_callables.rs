//! User-defined callable extraction (custom macros & widgets).
//!
//! This module detects two kinds of user-defined callables:
//!
//! 1. **Custom macros** defined via `Macro.add('name', { handler: function() { ... } })`
//!    in `[script]` passages. The handler's `this.args[N]` usage determines
//!    the argument count.
//!
//! 2. **Widgets** defined via `<<widget name>>...<</widget>>` in passages
//!    tagged `[widget]`. Widgets act as reusable macros whose body is
//!    executed when invoked.

use super::helpers::{line_from_offset, strip_comments};
use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// User callable types
// ---------------------------------------------------------------------------

/// The kind of a user-defined callable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCallableKind {
    /// Custom macro defined via `Macro.add('name', { handler: function() { ... } })`.
    CustomMacro,
    /// Widget defined via `<<widget name>>...<</widget>>` in a [widget]-tagged passage.
    Widget,
}

/// A user-defined callable (custom macro or widget) that can be invoked
/// like a function from macro passages.
///
/// Custom macros are defined in `[script]` passages using SugarCube's
/// `Macro.add()` API. Widgets are defined in `[widget]`-tagged passages
/// using `<<widget name>>...<</widget>>`.
#[derive(Debug, Clone)]
pub struct UserCallable {
    /// The callable name (e.g., "useItem" for `Macro.add('useItem', ...)`).
    pub name: String,
    /// The kind of callable (custom macro or widget).
    pub kind: UserCallableKind,
    /// Number of arguments this callable accepts, if known.
    /// For custom macros, derived from `this.args[N]` usage in the handler.
    /// For widgets, defaults to variadic (None) unless explicitly annotated.
    pub arg_count: Option<usize>,
    /// The passage name where this callable is defined.
    pub defined_in: String,
    /// The file URI where this callable is defined.
    pub file_uri: String,
    /// The 0-based line number where this callable is defined.
    pub defined_at_line: u32,
    /// The body of the callable's handler/widget code (for analysis of
    /// variable effects). For custom macros, this is the `handler` function
    /// body. For widgets, this is the content between `<<widget>>` and
    /// `<</widget>>`.
    pub body: Option<String>,
}

/// Minimal passage info passed to the `extract_user_callables` hook.
///
/// The workspace-level callable extractor collects this information from all
/// passages and detects custom macro definitions (in script passages) and
/// widget definitions (in widget-tagged passages).
#[derive(Debug, Clone)]
pub struct PassageInfo {
    /// The passage name.
    pub name: String,
    /// The file URI where this passage lives.
    pub file_uri: String,
    /// The passage tags (e.g., ["script"], ["widget"]).
    pub tags: Vec<String>,
    /// The passage body text.
    pub body_text: String,
}

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

/// `this.args[N]` usage in handler body — to determine arg count
static RE_THIS_ARGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"this\.args\s*\[\s*(\d+)\s*\]").unwrap()
});

/// `<<widget name>>` — widget definition opening tag
static RE_WIDGET_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<<widget\s+([A-Za-z_][A-Za-z0-9_]*)\s*>>").unwrap()
});

// ---------------------------------------------------------------------------
// Brace-balanced body extraction
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// extract_user_callables()
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
                        // without a body so invocations can still be recognized
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
