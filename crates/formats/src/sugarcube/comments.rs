//! Comment handling for SugarCube.
//!
//! SugarCube supports several comment syntaxes that should be excluded from
//! link extraction, variable detection, and validation:
//!
//! - `/* ... */` — C-style block comments (valid everywhere)
//! - `/% ... %/` — Twine-style block comments (valid everywhere)
//! - `<!-- ... -->` — HTML block comments (valid everywhere)
//! - `// ...` — JavaScript line comments (valid only inside JavaScript
//!   contexts: script passages tagged `[script]`, or `<<script>>` blocks
//!   within normal passages)
//!
//! This module provides functions to identify comment spans so that regex
//! matches falling within comments can be filtered out, preserving accurate
//! byte offsets.

use once_cell::sync::Lazy;
use regex::Regex;
use std::ops::Range;

/// Regex to find `<<script>>...<</script>>` block spans in passage bodies.
/// Used to determine where `//` line comments are valid.
pub(crate) static RE_SCRIPT_BLOCK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<<script>>.*?<</script>>").unwrap());

// ---------------------------------------------------------------------------
// Block comment detection (valid globally in SugarCube passages)
// ---------------------------------------------------------------------------

/// Find all block comment spans in the given text.
///
/// Detects the following comment types that are valid globally in SugarCube:
/// - `/* ... */` — C-style block comments
/// - `/% ... %/` — Twine-style block comments
/// - `<!-- ... -->` — HTML block comments
///
/// Returns a sorted list of byte ranges covering each comment (including
/// the delimiters). These spans can be used to skip regex matches that
/// fall within comments.
///
/// SugarCube does NOT support nested block comments, so this function
/// uses a simple depth-0 scan for each comment type.
pub(crate) fn find_block_comment_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();

    spans.extend(find_c_style_comment_spans(text));
    spans.extend(find_twine_comment_spans(text));
    spans.extend(find_html_comment_spans(text));

    // Sort by start position for consistent overlap checking
    spans.sort_by_key(|s| s.start);

    spans
}

/// Find all `/* ... */` block comment spans.
fn find_c_style_comment_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 1 < len {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    spans.push(start..i);
                    break;
                }
                i += 1;
            }
            // Unclosed comment — treat the rest as a comment
            if i + 1 >= len && spans.last().map_or(true, |s| s.start != start) {
                spans.push(start..len);
            }
            continue;
        }
        i += 1;
    }

    spans
}

/// Find all `/% ... %/` Twine-style block comment spans.
fn find_twine_comment_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 1 < len {
        // Check for Twine comment start: /%
        if bytes[i] == b'/' && bytes[i + 1] == b'%' {
            let start = i;
            i += 2;
            // Scan for comment end: %/
            while i + 1 < len {
                if bytes[i] == b'%' && bytes[i + 1] == b'/' {
                    i += 2;
                    spans.push(start..i);
                    break;
                }
                i += 1;
            }
            // Unclosed Twine comment — treat the rest as a comment
            if i + 1 >= len && spans.last().map_or(true, |s| s.start != start) {
                spans.push(start..len);
            }
            continue;
        }
        i += 1;
    }

    spans
}

/// Find all `<!-- ... -->` HTML block comment spans.
fn find_html_comment_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 3 < len {
        // Check for HTML comment start: <!--
        if bytes[i] == b'<' && bytes[i + 1] == b'!' && bytes[i + 2] == b'-' && bytes[i + 3] == b'-' {
            let start = i;
            i += 4;
            // Scan for comment end: -->
            while i + 2 < len {
                if bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' {
                    i += 3;
                    spans.push(start..i);
                    break;
                }
                i += 1;
            }
            // Unclosed HTML comment — treat the rest as a comment
            if i + 2 >= len && spans.last().map_or(true, |s| s.start != start) {
                spans.push(start..len);
            }
            continue;
        }
        i += 1;
    }

    spans
}

// ---------------------------------------------------------------------------
// Line comment detection (JavaScript contexts only)
// ---------------------------------------------------------------------------

/// Find `//` line comment spans within the given text, optionally restricted
/// to specific byte ranges (script block spans).
///
/// SugarCube does NOT support `//` as a comment syntax in passage markup.
/// However, inside JavaScript contexts — script passages or `<<script>>`
/// blocks — `//` is a valid line comment.
///
/// # Arguments
///
/// * `text` — The passage body text to scan.
/// * `script_block_spans` — If `Some`, only detect `//` line comments within
///   these byte ranges (representing `<<script>>...<</script>>` blocks).
///   If `None`, the entire text is treated as a script context (for passages
///   tagged `[script]` or named "Story JavaScript").
pub(crate) fn find_line_comment_spans(
    text: &str,
    script_block_spans: Option<&[Range<usize>]>,
) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 1 < len {
        // Only scan within script block spans if provided
        if let Some(script_spans) = script_block_spans {
            let pos = i;
            let in_script = script_spans.iter().any(|s| pos >= s.start && pos < s.end);
            if !in_script {
                i += 1;
                continue;
            }
        }

        // Check for line comment start: //
        // But not inside a string literal — we need a simple heuristic:
        // Look backwards to check if we're inside a quoted string.
        // A more robust approach would track string state, but for
        // the typical SugarCube usage pattern, a simple scan works.
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Heuristic: skip if preceded by `:` (could be a URL like http://)
            // or if inside a string. We check for URL patterns specifically.
            if is_url_double_slash(text, i) {
                i += 2;
                continue;
            }

            let start = i;
            // Scan to end of line
            i += 2;
            while i < len {
                if bytes[i] == b'\n' {
                    // Include the newline in the comment span so that
                    // anything on the next line is not considered commented
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(start..i);
            continue;
        }
        i += 1;
    }

    spans
}

/// Check if a `//` at the given position is part of a URL (e.g., `http://`,
/// `https://`, `file://`). This prevents false positives where URL paths
/// are mistakenly treated as line comments.
fn is_url_double_slash(text: &str, slash_pos: usize) -> bool {
    // Check for protocol prefixes before the //
    let prefix_start = if slash_pos >= 7 { slash_pos - 7 } else { 0 };
    let prefix = &text[prefix_start..slash_pos].to_lowercase();

    // Common URL protocols
    if prefix.ends_with("http:") || prefix.ends_with("https:") || prefix.ends_with("file:")
        || prefix.ends_with("ftp:") || prefix.ends_with("ws:") || prefix.ends_with("wss:")
    {
        return true;
    }

    // Also check for // inside a string literal by looking for an odd number
    // of quote characters before this position on the same line.
    // This is a simple heuristic that works for most cases.
    let line_start = text[..slash_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let before = &text[line_start..slash_pos];
    let double_quotes = before.chars().filter(|&c| c == '"').count();
    let single_quotes = before.chars().filter(|&c| c == '\'').count();

    // If there's an odd number of quotes before us, we're inside a string
    // and this // is not a comment.
    double_quotes % 2 == 1 || single_quotes % 2 == 1
}

/// Find `<<script>>...<</script>>` block spans in a passage body.
///
/// Returns the byte ranges of the content between `<<script>>` and
/// `<</script>>` tags (exclusive of the tags themselves), suitable for
/// use with `find_line_comment_spans()`.
pub(crate) fn find_script_block_spans(body: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();

    for caps in RE_SCRIPT_BLOCK.captures_iter(body) {
        if let Some(full) = caps.get(0) {
            // The entire match includes the <<script>> and <</script>> tags.
            // We want to include the tags in the script span so that
            // line comments within the full block are detected.
            spans.push(full.start()..full.end());
        }
    }

    spans
}

// ---------------------------------------------------------------------------
// Convenience: find all comment spans for a passage
// ---------------------------------------------------------------------------

/// Find all comment spans for a SugarCube passage body.
///
/// This combines block comments and line comments into a single sorted list.
///
/// # Arguments
///
/// * `text` — The passage body text.
/// * `is_script_passage` — Whether this passage is a JavaScript passage
///   (tagged `[script]` or named "Story JavaScript"). In script passages,
///   `//` line comments are valid everywhere.
pub(crate) fn find_all_comment_spans(text: &str, is_script_passage: bool) -> Vec<Range<usize>> {
    let mut spans = find_block_comment_spans(text);

    if is_script_passage {
        // In script passages, // is a line comment everywhere
        spans.extend(find_line_comment_spans(text, None));
    } else {
        // In normal passages, // is only valid inside <<script>> blocks
        let script_blocks = find_script_block_spans(text);
        if !script_blocks.is_empty() {
            spans.extend(find_line_comment_spans(text, Some(&script_blocks)));
        }
    }

    // Sort by start position
    spans.sort_by_key(|s| s.start);

    spans
}

/// Check whether a given byte range overlaps with any comment span.
///
/// A match is considered "inside a comment" if it starts within a
/// comment span, or if the match range overlaps with a comment span.
pub(crate) fn is_in_comment(spans: &[Range<usize>], range: &Range<usize>) -> bool {
    spans.iter().any(|s| {
        // Overlap check: two ranges [a,b) and [c,d) overlap if a < d && c < b
        range.start < s.end && s.start < range.end
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── C-style block comments ────────────────────────────────────────

    #[test]
    fn test_find_c_style_comment_basic() {
        let text = "before /* comment */ after";
        let spans = find_c_style_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* comment */");
    }

    #[test]
    fn test_find_c_style_comment_multiline() {
        let text = "before /* line1\nline2 */ after";
        let spans = find_c_style_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* line1\nline2 */");
    }

    #[test]
    fn test_find_c_style_comment_multiple() {
        let text = "a /* c1 */ b /* c2 */ c";
        let spans = find_c_style_comment_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].clone()], "/* c1 */");
        assert_eq!(&text[spans[1].clone()], "/* c2 */");
    }

    #[test]
    fn test_find_c_style_comment_unclosed() {
        let text = "before /* unclosed comment";
        let spans = find_c_style_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* unclosed comment");
    }

    #[test]
    fn test_find_c_style_comment_none() {
        let text = "no comments here";
        let spans = find_c_style_comment_spans(text);
        assert!(spans.is_empty());
    }

    // ── Twine-style block comments ────────────────────────────────────

    #[test]
    fn test_find_twine_comment_basic() {
        let text = "before /% comment %/ after";
        let spans = find_twine_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/% comment %/");
    }

    #[test]
    fn test_find_twine_comment_multiline() {
        let text = "before /% line1\nline2 %/ after";
        let spans = find_twine_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/% line1\nline2 %/");
    }

    #[test]
    fn test_find_twine_comment_unclosed() {
        let text = "before /% unclosed";
        let spans = find_twine_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/% unclosed");
    }

    #[test]
    fn test_twine_comment_skips_links() {
        let text = "/% [[HiddenLink]] %/ visible [[RealLink]]";
        let spans = find_block_comment_spans(text);
        // The Twine comment span should include HiddenLink
        assert!(is_in_comment(&spans, &(4..18)));
        // RealLink should NOT be in a comment
        assert!(!is_in_comment(&spans, &(28..42)));
    }

    // ── HTML block comments ───────────────────────────────────────────

    #[test]
    fn test_find_html_comment_basic() {
        let text = "before <!-- comment --> after";
        let spans = find_html_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "<!-- comment -->");
    }

    #[test]
    fn test_find_html_comment_multiline() {
        let text = "before <!-- line1\nline2 --> after";
        let spans = find_html_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "<!-- line1\nline2 -->");
    }

    #[test]
    fn test_find_html_comment_unclosed() {
        let text = "before <!-- unclosed";
        let spans = find_html_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "<!-- unclosed");
    }

    #[test]
    fn test_html_comment_skips_links() {
        let text = "<!-- [[HiddenLink]] --> visible [[RealLink]]";
        let spans = find_block_comment_spans(text);
        assert!(is_in_comment(&spans, &(5..21)));
        assert!(!is_in_comment(&spans, &(30..44)));
    }

    // ── Line comments (script contexts) ───────────────────────────────

    #[test]
    fn test_line_comments_in_script_passage() {
        let text = r#"// This is a comment
Engine.play("Village");
// Engine.play("Forest");"#;
        let spans = find_line_comment_spans(text, None);
        assert_eq!(spans.len(), 2);
        // First line comment
        assert_eq!(&text[spans[0].clone()], "// This is a comment\n");
        // Second line comment
        assert!(text[spans[1].clone()].starts_with("// Engine.play"));
    }

    #[test]
    fn test_line_comments_not_in_urls() {
        let text = r#"var url = "http://example.com";"#;
        let spans = find_line_comment_spans(text, None);
        // The // inside the URL string should NOT be treated as a line comment
        assert!(spans.is_empty());
    }

    #[test]
    fn test_line_comments_only_in_script_blocks() {
        let text = r#"Some text
<<script>>
// This is a JS comment
Engine.play("Village");
<</script>>
// This is NOT a comment in Twine"#;
        let script_blocks = find_script_block_spans(text);
        let spans = find_line_comment_spans(text, Some(&script_blocks));
        // Only one line comment should be found (inside <<script>>)
        assert_eq!(spans.len(), 1);
        assert!(text[spans[0].clone()].contains("This is a JS comment"));
    }

    // ── Combined comment detection ────────────────────────────────────

    #[test]
    fn test_find_all_comment_spans_normal_passage() {
        let text = r#"Text /* block */ more /% twine %/ end
<<script>>
// JS comment
Engine.play("Village");
<</script>>
<!-- HTML comment -->
[[RealLink]]"#;
        let spans = find_all_comment_spans(text, false);
        // Should find: /* block */, /% twine %/, // JS comment, <!-- HTML comment -->
        assert!(spans.len() >= 4, "Expected at least 4 comment spans, got {}", spans.len());

        // [[RealLink]] should NOT be in a comment
        let link_pos = text.find("[[RealLink]]").unwrap();
        assert!(!is_in_comment(&spans, &(link_pos..link_pos + 12)));
    }

    #[test]
    fn test_find_all_comment_spans_script_passage() {
        let text = r#"// Script passage comment
var x = 1;
/* block comment */
Engine.play("Forest");"#;
        let spans = find_all_comment_spans(text, true);
        // Should find the // line comment and /* block comment */
        assert!(spans.len() >= 2);
    }

    #[test]
    fn test_is_in_comment() {
        let text = "before /* comment */ after";
        let spans = find_block_comment_spans(text);

        // Before comment
        assert!(!is_in_comment(&spans, &(0..6)));
        // Inside comment
        assert!(is_in_comment(&spans, &(8..15)));
        // After comment
        assert!(!is_in_comment(&spans, &(21..26)));
        // Overlapping comment start
        assert!(is_in_comment(&spans, &(5..10)));
    }

    #[test]
    fn test_comment_skips_links() {
        let text = "/* [[HiddenLink]] */ visible [[RealLink]]";
        let spans = find_block_comment_spans(text);

        // "HiddenLink" is inside a comment
        assert!(is_in_comment(&spans, &(3..20)));
        // "RealLink" is NOT inside a comment
        assert!(!is_in_comment(&spans, &(29..40)));
    }

    #[test]
    fn test_mixed_comment_types_filter_links() {
        let text = r#"/% [[TwineHidden]] %/ <!-- [[HTMLHidden]] --> visible [[RealLink]]"#;
        let spans = find_block_comment_spans(text);

        // Both hidden links should be in comments
        let twine_link_pos = text.find("[[TwineHidden]]").unwrap();
        assert!(is_in_comment(&spans, &(twine_link_pos..twine_link_pos + 16)));

        let html_link_pos = text.find("[[HTMLHidden]]").unwrap();
        assert!(is_in_comment(&spans, &(html_link_pos..html_link_pos + 15)));

        // Real link should NOT be in a comment
        let real_link_pos = text.find("[[RealLink]]").unwrap();
        assert!(!is_in_comment(&spans, &(real_link_pos..real_link_pos + 12)));
    }
}
