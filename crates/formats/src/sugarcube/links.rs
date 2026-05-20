//! Link extraction for SugarCube.
//!
//! Contains regexes and functions for extracting passage links from
//! `[[...]]` syntax, implicit passage references (data-passage, Engine.play,
//! UI.goto, UI.include, Story.get, Story.has, etc.), and macro passage
//! references (<<goto>>, <<link>>, <<include>>, <<button>>, etc.).

use knot_core::passage::Link;
use once_cell::sync::Lazy;
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
pub(crate) static RE_LINK_SIMPLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap());

/// [[Display->Target]] — arrow-style link
pub(crate) static RE_LINK_ARROW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap());

/// [[Display|Target]] — pipe-style link
pub(crate) static RE_LINK_PIPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap());

/// HTML data-passage attribute — implicit passage reference
pub(crate) static RE_DATA_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"data-passage\s*=\s*["']([^"']+)["']"#).unwrap());

/// Engine.play() — implicit passage reference
pub(crate) static RE_ENGINE_PLAY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Engine.goto() — implicit passage reference
pub(crate) static RE_ENGINE_GOTO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.get() — implicit passage reference
pub(crate) static RE_STORY_GET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.passage() — implicit passage reference
pub(crate) static RE_STORY_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.has() — implicit passage reference (checks if passage exists)
pub(crate) static RE_STORY_HAS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*has\s*\(\s*["']([^"']+)["']"#).unwrap());

/// UI.goto() — implicit passage reference (navigates to passage)
pub(crate) static RE_UI_GOTO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"UI\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// UI.include() — implicit passage reference (includes passage content)
pub(crate) static RE_UI_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"UI\s*\.\s*include\s*\(\s*["']([^"']+)["']"#).unwrap());

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
        links.push(Link {
            display_text: Some(display),
            target,
            span: body_offset + m.start()..body_offset + m.end(),
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
        links.push(Link {
            display_text: Some(display),
            target,
            span: body_offset + m.start()..body_offset + m.end(),
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

        // Filter: skip targets containing "::" — this is JavaScript
        // namespace accessor syntax (e.g., Use::Operation), not a Twine
        // passage name. The "::" prefix is used for passage headers in
        // Twee format but never appears inside passage link targets.
        if target.contains("::") {
            continue;
        }

        links.push(Link {
            display_text: None,
            target,
            span: body_offset + m.start()..body_offset + m.end(),
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

    // All regexes are Lazy statics — compiled once, reused across all calls.
    let patterns: &[&Lazy<Regex>] = &[
        &RE_DATA_PASSAGE,
        &RE_ENGINE_PLAY,
        &RE_ENGINE_GOTO,
        &RE_STORY_GET,
        &RE_STORY_PASSAGE,
        &RE_STORY_HAS,
        &RE_UI_GOTO,
        &RE_UI_INCLUDE,
    ];

    for re in patterns {
        for caps in re.captures_iter(body) {
            if let Some(target_match) = caps.get(1) {
                let full_match = caps.get(0).unwrap();
                let target = target_match.as_str().trim().to_string();
                if !target.is_empty() {
                    links.push(Link {
                        display_text: None,
                        target,
                        span: body_offset + full_match.start()..body_offset + full_match.end(),
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
        let string_args = parse_quoted_args(args_str);

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
                // Compute the args offset in the body.
                // The args string is trimmed from body[name_end..closing_gt_start].
                let name_end_in_body = m.name_start + m.name_len;
                let body_after_name = &body[name_end_in_body..m.end.saturating_sub(2)];
                let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                let args_offset_in_body = name_end_in_body + trimmed_start;

                links.push(Link {
                    display_text: None,
                    target: content.clone(),
                    span: body_offset + args_offset_in_body + *rel_start
                        ..body_offset + args_offset_in_body + *rel_end,
                });
            }
        }
    }

    links
}

/// Parse quoted string arguments from a macro's argument string.
///
/// Extracts the content of `"..."` and `'...'` quoted strings from the
/// args portion of a macro invocation. This handles:
/// - `<<goto "PassageName">>` → ["PassageName"]
/// - `<<link "Label" "PassageName">>` → ["Label", "PassageName"]
/// - `<<include 'Some Passage'>>` → ["Some Passage"]
///
/// Returns tuples of (content, rel_start, rel_end) where rel_start/rel_end
/// are byte offsets relative to the args string, covering the content
/// INSIDE the quotes (not including the quote characters themselves).
fn parse_quoted_args(args: &str) -> Vec<(String, usize, usize)> {
    let mut result = Vec::new();
    let mut chars = args.char_indices().peekable();

    while let Some(&(_pos, c)) = chars.peek() {
        if c == '"' || c == '\'' {
            let quote = c;
            chars.next(); // consume opening quote
            let content_start = chars.peek().map(|&(i, _)| i).unwrap_or(args.len());
            let mut content = String::new();
            let mut content_end = content_start;
            while let Some(&(i, cc)) = chars.peek() {
                if cc == quote {
                    content_end = i;
                    chars.next(); // consume closing quote
                    break;
                }
                content.push(cc);
                content_end = i + cc.len_utf8();
                chars.next();
            }
            if !content.is_empty() {
                result.push((content, content_start, content_end));
            }
        } else {
            chars.next(); // skip non-quote characters
        }
    }

    result
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
        let args = parse_quoted_args(r#""Forest""#);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].0, "Forest");

        let args = parse_quoted_args(r#""Label" "Forest""#);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].0, "Label");
        assert_eq!(args[1].0, "Forest");

        let args = parse_quoted_args(r#"'Single'"#);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].0, "Single");

        let args = parse_quoted_args(r#""Multi Word" "Other""#);
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
}
