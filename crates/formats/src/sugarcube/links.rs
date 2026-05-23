//! Link extraction for SugarCube.
//!
//! Contains regexes and functions for extracting passage links from
//! `[[...]]` syntax, implicit passage references (data-passage, Engine.play,
//! UI.goto, UI.include, Story.get, Story.has, etc.), and macro passage
//! references (<<goto>>, <<link>>, <<include>>, <<button>>, etc.).

use knot_core::passage::Link;
use std::sync::LazyLock;
use regex::Regex;
use std::ops::Range;

use super::macros;

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// [[Target]] — simple passage link
///
/// NOTE: This regex can match JavaScript bracket notation like `obj[[key]]`
/// which is NOT a Twine link. The `extract_links()` function filters out
/// these false positives by checking the character before the match.
pub(crate) static RE_LINK_SIMPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap());

/// [[Display->Target]] — arrow-style link
pub(crate) static RE_LINK_ARROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap());

/// [[Display|Target]] — pipe-style link
pub(crate) static RE_LINK_PIPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap());

/// HTML data-passage attribute — implicit passage reference
pub(crate) static RE_DATA_PASSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"data-passage\s*=\s*["']([^"']+)["']"#).unwrap());

/// Engine.play() — implicit passage reference
pub(crate) static RE_ENGINE_PLAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Engine.goto() — implicit passage reference
pub(crate) static RE_ENGINE_GOTO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.get() — implicit passage reference
pub(crate) static RE_STORY_GET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.passage() — implicit passage reference
pub(crate) static RE_STORY_PASSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.has() — implicit passage reference (checks if passage exists)
pub(crate) static RE_STORY_HAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"Story\s*\.\s*has\s*\(\s*["']([^"']+)["']"#).unwrap());

/// UI.goto() — implicit passage reference (navigates to passage)
pub(crate) static RE_UI_GOTO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"UI\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// UI.include() — implicit passage reference (includes passage content)
pub(crate) static RE_UI_INCLUDE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"UI\s*\.\s*include\s*\(\s*["']([^"']+)["']"#).unwrap());

// ---------------------------------------------------------------------------
// NOTE: Macro passage reference extraction now uses the string-aware scanner
// from blocks.rs instead of regex. The RE_MACRO pattern is kept in
// regexes.rs for backward compatibility but should not be used for
// new code that needs to handle > in macro conditions.
// ---------------------------------------------------------------------------
// Link extraction functions
// ---------------------------------------------------------------------------

/// Extract all `[[...]]` links from a passage body.
///
/// The `body_offset` is the byte offset of the body text within the full
/// source document, used to compute absolute spans.
pub(crate) fn extract_links(body: &str, body_offset: usize) -> Vec<Link> {
    let mut links = Vec::new();

    // Arrow-style links: [[Display->Target]]
    for caps in RE_LINK_ARROW.captures_iter(body) {
        let m = caps.get(0).unwrap();
        // Skip JS bracket notation false positives
        if is_js_bracket_context(body, m.start()) {
            continue;
        }
        let display = caps.get(1).unwrap().as_str().trim().to_string();
        let target = caps.get(2).unwrap().as_str().trim().to_string();
        // No "::" filter for arrow links — the -> separator already
        // disambiguates from JS bracket notation, so a target like
        // My::Passage after -> is always intentional.
        links.push(Link {
            display_text: Some(display),
            target,
            span: body_offset + m.start()..body_offset + m.end(),
            edge_type_hint: None,
        });
    }

    // Pipe-style links: [[Display|Target]]
    for caps in RE_LINK_PIPE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        // Skip JS bracket notation false positives
        if is_js_bracket_context(body, m.start()) {
            continue;
        }
        let display = caps.get(1).unwrap().as_str().trim().to_string();
        let target = caps.get(2).unwrap().as_str().trim().to_string();
        // No "::" filter for pipe links — the | separator already
        // disambiguates from JS bracket notation, so a target like
        // My::Passage after | is always intentional.
        links.push(Link {
            display_text: Some(display),
            target,
            span: body_offset + m.start()..body_offset + m.end(),
            edge_type_hint: None,
        });
    }

    // Simple links: [[Target]]
    // We must skip matches that are sub-spans of arrow/pipe links.
    // A simple approach: collect all arrow/pipe spans and filter overlaps.
    let arrow_pipe_spans: Vec<Range<usize>> = RE_LINK_ARROW
        .captures_iter(body)
        .chain(RE_LINK_PIPE.captures_iter(body))
        .filter_map(|caps| {
            let m = caps.get(0)?;
            Some(m.start()..m.end())
        })
        .collect();

    for caps in RE_LINK_SIMPLE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let span = m.start()..m.end();

        // Filter: skip sub-spans of arrow/pipe links
        let overlaps = arrow_pipe_spans.iter().any(|s| {
            span.start >= s.start && span.end <= s.end
        });
        if overlaps {
            continue;
        }

        // Filter: skip JavaScript bracket notation false positives.
        if is_js_bracket_context(body, m.start()) {
            continue;
        }

        let target = caps.get(1).unwrap().as_str().trim().to_string();

        // Filter: skip targets containing "::" ONLY when the match occurs
        // in a JavaScript bracket context (obj[[key]] with :: in the key).
        // Standalone [[Use::Operation]] in prose is unlikely but valid as a
        // passage name — let it through. JS bracket contexts are already
        // caught by is_js_bracket_context() above, so this is a secondary
        // defense for cases where is_js_bracket_context returns false but
        // the content looks like a C++/JS namespace pattern.
        if target.contains("::") && is_js_bracket_context(body, m.start()) {
            continue;
        }

        links.push(Link {
            display_text: None,
            target,
            span: body_offset + m.start()..body_offset + m.end(),
            edge_type_hint: None,
        });
    }

    links
}

/// Extract implicit passage references from raw text/HTML/JS.
///
/// Detects patterns like `data-passage="..."`, `Engine.play("...")`,
/// `Story.get("...")`, `Story.has("...")`, `UI.goto("...")`,
/// `UI.include("...")` that reference passages but aren't standard
/// `[[links]]` or `<<macro>>` passage-args.
pub(crate) fn extract_implicit_passage_refs(body: &str, body_offset: usize) -> Vec<Link> {
    let mut links = Vec::new();

    // Each pattern paired with its edge type hint.
    // - Engine.goto() / UI.goto() are unconditional redirects → Jump
    // - UI.include() is a passage inclusion → Include
    // - Everything else (data-passage, Engine.play, Story.get/has/passage)
    //   is either navigation or data access → None (Navigation default)
    let patterns: &[(&LazyLock<Regex>, Option<knot_core::graph::EdgeType>)] = &[
        (&RE_DATA_PASSAGE, None),
        (&RE_ENGINE_PLAY, None),
        (&RE_ENGINE_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_STORY_GET, None),
        (&RE_STORY_PASSAGE, None),
        (&RE_STORY_HAS, None),
        (&RE_UI_GOTO, Some(knot_core::graph::EdgeType::Jump)),
        (&RE_UI_INCLUDE, Some(knot_core::graph::EdgeType::Include)),
    ];

    for (re, edge_hint) in patterns {
        for caps in re.captures_iter(body) {
            if let Some(target_match) = caps.get(1) {
                let full_match = caps.get(0).unwrap();
                let target = target_match.as_str().trim().to_string();
                if !target.is_empty() {
                    // No "::" filter for implicit passage refs — quoted
                    // string arguments in Engine.play("..."), data-passage="...",
                    // etc. are always intentional passage references. JS
                    // namespace syntax Use::Operation never appears as a
                    // quoted string argument to these functions.
                    links.push(Link {
                        display_text: None,
                        target,
                        span: body_offset + full_match.start()..body_offset + full_match.end(),
                        edge_type_hint: *edge_hint,
                    });
                }
            }
        }
    }

    links
}

/// Extract passage references from macro invocations.
///
/// Uses the **string-aware macro scanner** from `blocks.rs` instead of regex
/// to correctly handle `>` and `>>` inside macro conditions.
///
/// Uses the builtin macro catalog's `is_passage_ref` flags on argument
/// definitions to determine which arguments are passage references.
/// For macros in `passage_arg_macro_names()`, extracts the passage name
/// from the appropriate argument position.
///
/// Examples:
/// - `<<goto "PassageName">>` → Link to "PassageName"
/// - `<<link "Label" "PassageName">>` → Link to "PassageName"
/// - `<<include "PassageName">>` → Link to "PassageName"
pub(crate) fn extract_macro_passage_refs(body: &str, body_offset: usize) -> Vec<Link> {
    let mut links = Vec::new();
    let passage_arg_macros = macros::passage_arg_macro_names();

    let parsed_macros = super::blocks::scan_macros(body);

    for m in &parsed_macros {
        // Skip close tags
        if m.name.starts_with('/') {
            continue;
        }

        let macro_name = m.name.as_str();

        // Only process macros that have passage-ref arguments
        if !passage_arg_macros.contains(macro_name) {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        // Parse quoted string arguments from the args string.
        let string_args = super::blocks::parse_quoted_args(args_str);

        if string_args.is_empty() {
            continue;
        }

        // Determine which argument is the passage reference.
        let arg_count = string_args.len();
        let passage_idx = macros::get_passage_arg_index(macro_name, arg_count);

        if passage_idx < 0 {
            continue;
        }

        let idx = passage_idx as usize;
        if idx < string_args.len() {
            let (content, rel_start, rel_end) = &string_args[idx];
            if !content.is_empty() {
                // No "::" filter for macro passage refs — quoted string
                // arguments in <<goto "...">>, <<link "..." "...">>, etc.
                // are always intentional passage references. JS namespace
                // syntax Use::Operation never appears as a quoted string
                // argument to SugarCube macros.

                // Compute the args offset in the body.
                // The args string is trimmed from body[name_end..closing_gt_start].
                let name_end_in_body = m.name_start + m.name_len;
                let body_after_name = &body[name_end_in_body..m.end.saturating_sub(2)];
                let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                let args_offset_in_body = name_end_in_body + trimmed_start;

                // Classify the edge type based on the macro name.
                // <<goto>> is an unconditional redirect (Jump), <<include>>
                // is a passage inclusion (Include). Everything else (<<link>>,
                // <<button>>, <<actions>>, etc.) is a player-choice navigation.
                let edge_type_hint = match macro_name {
                    "goto" => Some(knot_core::graph::EdgeType::Jump),
                    "include" => Some(knot_core::graph::EdgeType::Include),
                    _ => None,
                };

                links.push(Link {
                    display_text: None,
                    target: content.clone(),
                    span: body_offset + args_offset_in_body + *rel_start
                        ..body_offset + args_offset_in_body + *rel_end,
                    edge_type_hint,
                });
            }
        }
    }

    links
}

/// Check whether a `[[` at the given position in `text` is a JavaScript
/// bracket notation context rather than a genuine Twine link.
///
/// Returns `true` if the character immediately before position `pos` is
/// one that indicates JS computed property access (`obj[[key]]`):
/// - `[` — chained bracket access: `arr[i][[key]]`
/// - `]` — post-index bracket: `arr[0][[key]]`
/// - `)` — function result access: `func()[[key]]`
/// - `}` — object literal result: `{}[[key]]`
/// - alphanumeric — variable access without space: `cursor[[key]]`
/// - `_` — identifier continuation: `variable_name[[key]]`
/// - `$` — SugarCube variable: `$var[[key]]`
fn is_js_bracket_context(text: &str, pos: usize) -> bool {
    if pos == 0 {
        return false;
    }
    let prev = text.as_bytes()[pos - 1];
    prev == b'['
        || prev == b']'
        || prev == b')'
        || prev == b'}'
        || prev.is_ascii_alphanumeric()
        || prev == b'_'
        || prev == b'$'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_quoted_args() {
        let args = super::super::blocks::parse_quoted_args(r#""Forest""#);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].0, "Forest");

        let args = super::super::blocks::parse_quoted_args(r#""Label" "Forest""#);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].0, "Label");
        assert_eq!(args[1].0, "Forest");

        let args = super::super::blocks::parse_quoted_args(r#"'Single'"#);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].0, "Single");

        let args = super::super::blocks::parse_quoted_args(r#""Multi Word" "Other""#);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].0, "Multi Word");
        assert_eq!(args[1].0, "Other");
    }

    #[test]
    fn test_extract_macro_passage_refs_goto() {
        let body = r#"<<goto "Forest">>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_extract_macro_passage_refs_link() {
        let body = r#"<<link "Click me" "Forest">>Go<</link>>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_extract_macro_passage_refs_include() {
        let body = r#"<<include "Sidebar">>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Sidebar");
    }

    #[test]
    fn test_extract_macro_passage_refs_button() {
        let body = r#"<<button "Go" "Forest">>Click<</button>>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    // ── New implicit passage reference patterns ────────────────────────

    #[test]
    fn test_extract_implicit_story_has() {
        let body = r#"Story.has("Forest")"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_extract_implicit_ui_goto() {
        let body = r#"UI.goto("Village")"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Village");
    }

    #[test]
    fn test_extract_implicit_ui_include() {
        let body = r#"UI.include("Sidebar")"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Sidebar");
    }

    #[test]
    fn test_extract_implicit_ui_goto_with_whitespace() {
        let body = r#"UI . goto ( "Village" )"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Village");
    }

    #[test]
    fn test_extract_implicit_single_quotes() {
        let body = r#"UI.goto('Forest')"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_all_implicit_patterns_together() {
        let body = r#"Engine.play("A"); Engine.goto("B"); Story.get("C");
            Story.passage("D"); Story.has("E"); UI.goto("F"); UI.include("G");
            data-passage="H""#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 8);
        let targets: Vec<&str> = links.iter().map(|l| l.target.as_str()).collect();
        assert!(targets.contains(&"A"));
        assert!(targets.contains(&"B"));
        assert!(targets.contains(&"C"));
        assert!(targets.contains(&"D"));
        assert!(targets.contains(&"E"));
        assert!(targets.contains(&"F"));
        assert!(targets.contains(&"G"));
        assert!(targets.contains(&"H"));
    }

    // ── Regression tests for Use::Operation false positive ────────────────

    #[test]
    fn test_double_colon_filter_rejects_js_namespace() {
        // Bug 3 regression: Use::Operation in JS must NOT be treated as a
        // passage reference. The `::` filter in extract_links() rejects
        // link targets containing "::".
        let body = r#"var x = Use::Operation;"#;
        let links = extract_links(body, 0);
        // No [[...]] syntax present, so no links should be extracted
        assert!(links.is_empty());
    }

    #[test]
    fn test_double_colon_in_simple_link_after_space_allowed() {
        // After a space, [[Use::Operation]] is NOT in a JS bracket context,
        // so the `::` filter does not apply — this is allowed through as a
        // (rare but valid) passage name.
        let body = r#"Before [[Use::Operation]] after"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1, ":: in simple link after space should be allowed");
        assert_eq!(links[0].target, "Use::Operation");
    }

    #[test]
    fn test_double_colon_in_simple_link_in_js_context_filtered() {
        // obj[[Use::Operation]] is JS bracket access with :: in the key —
        // should be filtered by the combined :: + JS context check.
        let body = r#"obj[[Use::Operation]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), ":: in JS bracket context should be filtered");
    }

    #[test]
    fn test_js_bracket_notation_filtered() {
        // Bug 3 regression: obj[[key]] is JS bracket access, not a Twine link.
        let body = r#"var x = obj[[key]];"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket notation should be filtered");
    }

    #[test]
    fn test_js_chained_bracket_notation_filtered() {
        // arr[i][[key]] — chained bracket access
        let body = r#"var x = arr[i][[key]];"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "Chained JS bracket notation should be filtered");
    }

    #[test]
    fn test_js_function_result_bracket_filtered() {
        // func()[[key]] — function result access
        let body = r#"var x = func()[[key]];"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "Function result bracket notation should be filtered");
    }

    #[test]
    fn test_sugarcube_var_bracket_filtered() {
        // $var[[key]] — SugarCube variable bracket access
        let body = r#"$items[[0]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "SugarCube variable bracket notation should be filtered");
    }

    #[test]
    fn test_normal_link_not_filtered() {
        // A genuine [[Forest]] link should NOT be filtered
        let body = r#"You go [[Forest]]."#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_normal_arrow_link_not_filtered() {
        // [[Go north->Forest]] should work normally
        let body = r#"[[Go north->Forest]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    // ── Regression tests for "::" filter (Bug 3 defense) ──────────────

    #[test]
    fn test_double_colon_simple_link_at_start_allowed() {
        // [[Use::Operation]] at position 0 is NOT in a JS bracket context,
        // so the `::` filter does not apply — this is allowed through as a
        // (rare but valid) passage name.
        let body = r#"[[Use::Operation]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1, ":: at line start should be allowed");
        assert_eq!(links[0].target, "Use::Operation");
    }

    #[test]
    fn test_double_colon_simple_link_after_alphanumeric_filtered() {
        // cursor[[Some::Namespace]] — alphanumeric before [[ makes this a
        // JS bracket context, so :: in the target is filtered.
        let body = r#"cursor[[Some::Namespace]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), ":: in JS bracket context should be filtered");
    }

    #[test]
    fn test_double_colon_arrow_link_allowed() {
        // Arrow links with :: in target are always allowed — the -> separator
        // disambiguates from JS bracket notation.
        let body = r#"[[Go->My::Passage]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_pipe_link_allowed() {
        // Pipe links with :: in target are always allowed — the | separator
        // disambiguates from JS bracket notation.
        let body = r#"[[Click|My::Passage]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_normal_passage_name_with_single_colon() {
        // A single colon in a passage name is fine (though unusual)
        // Only "::" (double colon) triggers the filter
        let body = r#"[[Chapter 1: Intro]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1, "Single colon should NOT trigger the '::' filter");
        assert_eq!(links[0].target, "Chapter 1: Intro");
    }

    // ── Regression tests for is_js_bracket_context() ──────────────────

    #[test]
    fn test_js_bracket_context_after_open_bracket() {
        // arr[[key]] — open bracket before [[
        let body = r#"var x = arr[[key]];"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after '[' should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_close_bracket() {
        // arr[0][[key]] — close bracket before [[
        let body = r#"var x = arr[0][[key]];"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after ']' should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_alphanumeric() {
        // cursor[[key]] — alphanumeric before [[
        let body = r#"cursor[[key]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after alphanumeric should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_underscore() {
        // my_var[[key]] — underscore before [[
        let body = r#"my_var[[key]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after '_' should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_dollar() {
        // $items[[0]] — SugarCube variable before [[
        let body = r#"$items[[0]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after '$' should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_close_paren() {
        // func()[[key]] — close paren before [[
        let body = r#"func()[[key]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after ')' should be filtered");
    }

    #[test]
    fn test_js_bracket_context_after_close_brace() {
        // {}[[key]] — close brace before [[
        let body = r#"{}[[key]]"#;
        let links = extract_links(body, 0);
        assert!(links.is_empty(), "JS bracket after '}}' should be filtered");
    }

    #[test]
    fn test_link_after_space_not_filtered() {
        // "Go to [[Forest]]" — space before [[ is NOT a JS context
        let body = r#"Go to [[Forest]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    #[test]
    fn test_link_at_line_start_not_filtered() {
        // [[Forest]] at position 0 — no previous char, not JS context
        let body = r#"[[Forest]]"#;
        let links = extract_links(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    // ── Context-aware :: filter — implicit and macro refs ─────────────

    #[test]
    fn test_double_colon_in_engine_play_allowed() {
        // Engine.play("My::Passage") — quoted string args are always
        // intentional passage references; :: should NOT be filtered.
        let body = r#"Engine.play("My::Passage")"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_in_data_passage_allowed() {
        // data-passage="My::Passage" — always intentional
        let body = r#"data-passage="My::Passage""#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_in_story_has_allowed() {
        // Story.has("My::Passage") — always intentional
        let body = r#"Story.has("My::Passage")"#;
        let links = extract_implicit_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_in_macro_goto_allowed() {
        // <<goto "My::Passage">> — always intentional
        let body = r#"<<goto "My::Passage">>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_in_macro_link_allowed() {
        // <<link "Click" "My::Passage">> — always intentional
        let body = r#"<<link "Click" "My::Passage">>Go<</link>>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }

    #[test]
    fn test_double_colon_in_macro_include_allowed() {
        // <<include "My::Passage">> — always intentional
        let body = r#"<<include "My::Passage">>"#;
        let links = extract_macro_passage_refs(body, 0);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "My::Passage");
    }
}
