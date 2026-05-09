//! Link extraction for SugarCube.
//!
//! Contains regexes and functions for extracting passage links from
//! `[[...]]` syntax, implicit passage references (data-passage, Engine.play,
//! etc.), and macro passage references (<<goto>>, <<link>>, <<include>>,
//! <<button>>, etc.).

use knot_core::passage::Link;
use once_cell::sync::Lazy;
use regex::Regex;
use std::ops::Range;

use super::macros;

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// [[Target]] — simple passage link
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

/// <<name ...>> — any open macro (used by extract_macro_passage_refs)
pub(crate) static RE_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+([^>]*?))?>>").unwrap());

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
        // Only include if not overlapped by an arrow/pipe link.
        let overlaps = arrow_pipe_spans.iter().any(|s| {
            span.start >= s.start && span.end <= s.end
        });
        if !overlaps {
            let target = caps.get(1).unwrap().as_str().trim().to_string();
            links.push(Link {
                display_text: None,
                target,
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }
    }

    links
}

/// Extract implicit passage references from raw text/HTML/JS.
///
/// Detects patterns like `data-passage="..."`, `Engine.play("...")`,
/// `Story.get("...")` that reference passages but aren't standard
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

    for caps in RE_MACRO.captures_iter(body) {
        let full_match = caps.get(0).unwrap();
        let macro_name = caps.get(1).unwrap().as_str();

        // Only process macros that have passage-ref arguments
        if !passage_arg_macros.contains(macro_name) {
            continue;
        }

        let args_str = caps.get(2).map(|a| a.as_str()).unwrap_or("");

        // Parse quoted string arguments from the args string.
        // In SugarCube, macro args are split at spaces. The first element
        // is the macro name. The last string arg is always the target
        // (required passage name). If there are more than one string arg,
        // the second one is the label.
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
            let target = string_args[idx].clone();
            if !target.is_empty() {
                links.push(Link {
                    display_text: None,
                    target,
                    span: body_offset + full_match.start()..body_offset + full_match.end(),
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
fn parse_quoted_args(args: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = args.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c == '"' || c == '\'' {
            let quote = c;
            chars.next(); // consume opening quote
            let mut content = String::new();
            while let Some(&cc) = chars.peek() {
                if cc == quote {
                    chars.next(); // consume closing quote
                    break;
                }
                content.push(cc);
                chars.next();
            }
            if !content.is_empty() {
                result.push(content);
            }
        } else {
            chars.next(); // skip non-quote characters
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_quoted_args() {
        assert_eq!(parse_quoted_args(r#""Forest""#), vec!["Forest"]);
        assert_eq!(parse_quoted_args(r#""Label" "Forest""#), vec!["Label", "Forest"]);
        assert_eq!(parse_quoted_args(r#"'Single'"#), vec!["Single"]);
        assert_eq!(parse_quoted_args(r#""Multi Word" "Other""#), vec!["Multi Word", "Other"]);
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
}
