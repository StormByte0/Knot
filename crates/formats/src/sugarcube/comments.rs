//! Comment handling for SugarCube.
//!
//! SugarCube supports C-style block comments (`/* ... */`) that should be
//! excluded from link extraction, variable detection, and validation.
//!
//! This module provides functions to identify comment spans so that regex
//! matches falling within comments can be filtered out, preserving accurate
//! byte offsets.

use std::ops::Range;

/// Find all `/* ... */` block comment spans in the given text.
///
/// Returns a sorted list of byte ranges covering each comment (including
/// the `/*` and `*/` delimiters). These spans can be used to skip regex
/// matches that fall within comments.
///
/// SugarCube does NOT support nested block comments, so this function
/// uses a simple depth-0 scan.
pub(crate) fn find_comment_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 1 < len {
        // Check for comment start: /*
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            // Scan for comment end: */
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    spans.push(start..i);
                    break;
                }
                i += 1;
            }
            // If we reached the end without finding */, the comment
            // is unclosed — treat the rest of the text as a comment
            if i + 1 >= len && spans.last().map_or(true, |s| s.start != start) {
                spans.push(start..len);
            }
            continue;
        }
        i += 1;
    }

    spans
}

/// Check whether a given byte range overlaps with any comment span.
///
/// A match is considered "inside a comment" if it starts within a
/// comment span, or if the match range overlaps with a comment span.
pub(crate) fn is_in_comment(spans: &[Range<usize>], range: &Range<usize>) -> bool {
    // Binary search for efficiency since spans are sorted
    spans.iter().any(|s| {
        // Overlap check: two ranges [a,b) and [c,d) overlap if a < d && c < b
        range.start < s.end && s.start < range.end
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_comment_spans_basic() {
        let text = "before /* comment */ after";
        let spans = find_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* comment */");
    }

    #[test]
    fn test_find_comment_spans_multiline() {
        let text = "before /* line1\nline2 */ after";
        let spans = find_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* line1\nline2 */");
    }

    #[test]
    fn test_find_comment_spans_multiple() {
        let text = "a /* c1 */ b /* c2 */ c";
        let spans = find_comment_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].clone()], "/* c1 */");
        assert_eq!(&text[spans[1].clone()], "/* c2 */");
    }

    #[test]
    fn test_find_comment_spans_unclosed() {
        let text = "before /* unclosed comment";
        let spans = find_comment_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].clone()], "/* unclosed comment");
    }

    #[test]
    fn test_find_comment_spans_no_comments() {
        let text = "no comments here";
        let spans = find_comment_spans(text);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_is_in_comment() {
        let text = "before /* comment */ after";
        let spans = find_comment_spans(text);

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
        let spans = find_comment_spans(text);

        // "HiddenLink" at position ~5 is inside a comment
        assert!(is_in_comment(&spans, &(3..20)));
        // "RealLink" at position ~30 is NOT inside a comment
        assert!(!is_in_comment(&spans, &(29..40)));
    }
}
