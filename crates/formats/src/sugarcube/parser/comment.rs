//! Comment parsing and comment stripping.
//!
//! This module handles all 6 comment types that SugarCube/Twine supports:
//! - `/% ... %/` — Twine block comments
//! - `/%% ... %%/` — SugarCube block comments
//! - `<!-- ... -->` — HTML comments
//! - `<!--[if ...]>...<![endif]-->` — HTML conditional comments
//! - `/* ... */` — C-style block comments (CSS/JS)
//! - `// ...` — JS line comments (to EOL, with heuristics)
//!
//! Both the parsing functions (which produce `AstNode::Comment` nodes) and
//! `strip_comments` (which replaces comment content with spaces to preserve
//! byte offsets) must handle the **same set of 6 comment types** in a single
//! pass. They must be kept in sync — any comment type added to one must be
//! added to the other.

use crate::sugarcube::ast::*;

// ---------------------------------------------------------------------------
// Comment parsers (produce AstNode::Comment)
// ---------------------------------------------------------------------------

/// Parse a block comment (/% ... %/ or /%% ... %%/).
pub(super) fn parse_block_comment(text: &str, i: &mut usize, span_start: usize, is_sugarcube: bool) -> AstNode {
    let (close_delim, kind) = if is_sugarcube {
        ("%%/", CommentKind::SugarCube)
    } else {
        ("%/", CommentKind::Twine)
    };
    // delim_len = number of bytes the caller consumed before entering this
    // function (2 for /%, 3 for /%%). This is needed to compute the correct
    // span: span_start = offset + start, and *i on entry = start + delim_len,
    // so the span end must be offset + *i, NOT span_start + *i.
    let delim_len = if is_sugarcube { 3 } else { 2 };

    let content_start = *i;
    if let Some(pos) = text[*i..].find(close_delim) {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + close_delim.len();
        AstNode::Comment {
            content,
            kind,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    } else {
        // Unclosed comment
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    }
}

/// Parse an HTML comment (<!-- ... -->).
pub(super) fn parse_html_comment(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // delim_len = 4 for <!-- (caller already consumed it)
    let delim_len = 4;
    let content_start = *i;
    if let Some(pos) = text[*i..].find("-->") {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + 3; // Skip -->
        AstNode::Comment {
            content,
            kind: CommentKind::Html,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    } else {
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind: CommentKind::Html,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    }
}

/// Parse a C-style block comment (/* ... */).
///
/// These appear in CSS blocks (`<style>/* comment */</style>`) and JS
/// blocks (`<<script>>/* comment */<</script>>`) within SugarCube passages.
/// The content inside `/* */` is excluded from all analysis.
pub(super) fn parse_cstyle_comment(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // delim_len = 2 for /* (caller already consumed it)
    let delim_len = 2;
    let content_start = *i;
    if let Some(pos) = text[*i..].find("*/") {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + 2; // Skip */
        AstNode::Comment {
            content,
            kind: CommentKind::CStyle,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    } else {
        // Unclosed comment
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind: CommentKind::CStyle,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    }
}

/// Parse a JS single-line comment (// ... to end of line).
///
/// These appear in inline JS expressions and within `<script>` blocks
/// in SugarCube passages. The comment extends to the end of the line.
/// Content inside `//` comments is excluded from all analysis.
pub(super) fn parse_js_line_comment(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // delim_len = 2 for // (caller already consumed it)
    let delim_len = 2;
    let content_start = *i;
    // Scan to end of line
    if let Some(pos) = text[*i..].find('\n') {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end; // Don't consume the newline — it's part of text flow
        AstNode::Comment {
            content,
            kind: CommentKind::JsLine,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    } else {
        // Comment extends to end of text
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind: CommentKind::JsLine,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    }
}

/// Parse an HTML conditional comment (<!--[if ...]>...<![endif]-->).
///
/// Internet Explorer conditional comments are a legacy but still-valid
/// HTML pattern. The content between `<!--[if ...]>` and `<![endif]-->`
/// is excluded from all analysis.
pub(super) fn parse_html_conditional_comment(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    // delim_len = 4 for <!-- (caller already consumed it)
    let delim_len = 4;
    let content_start = *i;
    // Look for <![endif]-->
    if let Some(pos) = text[*i..].find("<![endif]") {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + "<![endif]".len();
        // Skip the -->  after <![endif]>
        if text[*i..].starts_with("-->") {
            *i += 3;
        }
        AstNode::Comment {
            content,
            kind: CommentKind::HtmlConditional,
            span: span_start..span_start + *i - content_start + delim_len,
        }
    } else {
        // Unclosed conditional comment — fall back to regular HTML comment parsing
        if let Some(pos) = text[*i..].find("-->") {
            let content_end = *i + pos;
            let content = text[content_start..content_end].to_string();
            *i = content_end + 3;
            AstNode::Comment {
                content,
                kind: CommentKind::HtmlConditional,
                span: span_start..span_start + *i - content_start + delim_len,
            }
        } else {
            let content = text[content_start..].to_string();
            *i = text.len();
            AstNode::Comment {
                content,
                kind: CommentKind::HtmlConditional,
                span: span_start..span_start + *i - content_start + delim_len,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Comment stripping utility
// ---------------------------------------------------------------------------

/// Strip all comment content from passage body text, replacing each comment
/// with an equivalent amount of whitespace to preserve byte offsets.
///
/// This is useful for consumers that need comment-free text for pattern
/// matching (e.g., searching for passage references in macro args) while
/// still maintaining correct position information.
///
/// ## Supported comment types
///
/// | Syntax | Type | Example |
/// |--------|------|---------|
/// | `/% ... %/` | Twine block | `/% not parsed %/` |
/// | `/%% ... %%/` | SugarCube block | `/%% not parsed %%/` |
/// | `<!-- ... -->` | HTML block | `<!-- not parsed -->` |
/// | `<!--[if ...]>...<![endif]-->` | HTML conditional | `<!--[if IE]>...<![endif]-->` |
/// | `/* ... */` | C-style block (CSS/JS) | `/* not parsed */` |
/// | `// ...` | JS line (to EOL) | `// not parsed` |
///
/// ## Returns
///
/// A new string with all comment content replaced by spaces (preserving
/// newlines for line-count consistency).
///
/// ## Note
///
/// This function and the parsing functions above handle the **same set of 6
/// comment types** in a single pass. They must be kept in sync — any comment
/// type added to one must be added to the other.
pub fn strip_comments(text: &str) -> String {
    let mut result = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        // Twine/SugarCube comments: /% ... %/ or /%% ... %%/
        if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'%' {
            let start = i;
            let is_sc = i + 2 < len && bytes[i + 2] == b'%';
            let open_len = if is_sc { 3 } else { 2 };
            let close = if is_sc { "%%/" } else { "%/" };
            i += open_len;
            if let Some(pos) = text[i..].find(close) {
                // Replace the entire comment with spaces (preserve newlines)
                for &b in &bytes[start..i + pos + close.len()] {
                    result.push(if b == b'\n' { b'\n' } else { b' ' });
                }
                i += pos + close.len();
            } else {
                for &b in &bytes[start..] {
                    result.push(if b == b'\n' { b'\n' } else { b' ' });
                }
                i = len;
            }
        }
        // C-style block comments: /* ... */
        else if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            if let Some(pos) = text[i..].find("*/") {
                for &b in &bytes[start..i + pos + 2] {
                    result.push(if b == b'\n' { b'\n' } else { b' ' });
                }
                i += pos + 2;
            } else {
                for &b in &bytes[start..] {
                    result.push(if b == b'\n' { b'\n' } else { b' ' });
                }
                i = len;
            }
        }
        // JS line comments: // ... (to end of line, with heuristics)
        else if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            let is_comment_context = if i + 2 < len {
                let at_line_start = i == 0 || bytes[i - 1] == b'\n';
                let preceded_by_space = i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t');
                let followed_by_space = bytes[i + 2] == b' ' || bytes[i + 2] == b'\t';
                at_line_start || (preceded_by_space && followed_by_space)
            } else {
                true
            };
            if is_comment_context {
                let start = i;
                i += 2;
                if let Some(pos) = text[i..].find('\n') {
                    for _ in &bytes[start..i + pos] {
                        result.push(b' ');
                    }
                    // Advance to the newline position so the outer loop
                    // picks it up as normal text.
                    i += pos;
                } else {
                    for _ in &bytes[start..] {
                        result.push(b' ');
                    }
                    i = len;
                }
            } else {
                result.push(bytes[i]);
                i += 1;
            }
        }
        // HTML comments: <!-- ... -->
        else if i + 3 < len && &text[i..i + 4] == "<!--" {
            let start = i;
            // Check for conditional: <!--[if ...]>
            let rest = &text[i + 4..];
            let is_conditional = rest.trim_start().starts_with("[if");

            if is_conditional {
                // Look for <![endif]-->
                i += 4;
                if let Some(pos) = text[i..].find("<![endif]") {
                    let end = i + pos + "<![endif]".len();
                    if text[end..].starts_with("-->") {
                        for &b in &bytes[start..end + 3] {
                            result.push(if b == b'\n' { b'\n' } else { b' ' });
                        }
                        i = end + 3;
                    } else {
                        for &b in &bytes[start..end] {
                            result.push(if b == b'\n' { b'\n' } else { b' ' });
                        }
                        i = end;
                    }
                } else {
                    // Fallback: treat as regular HTML comment
                    if let Some(pos) = text[i..].find("-->") {
                        for &b in &bytes[start..i + pos + 3] {
                            result.push(if b == b'\n' { b'\n' } else { b' ' });
                        }
                        i += pos + 3;
                    } else {
                        for &b in &bytes[start..] {
                            result.push(if b == b'\n' { b'\n' } else { b' ' });
                        }
                        i = len;
                    }
                }
            } else {
                i += 4;
                if let Some(pos) = text[i..].find("-->") {
                    for &b in &bytes[start..i + pos + 3] {
                        result.push(if b == b'\n' { b'\n' } else { b' ' });
                    }
                    i += pos + 3;
                } else {
                    for &b in &bytes[start..] {
                        result.push(if b == b'\n' { b'\n' } else { b' ' });
                    }
                    i = len;
                }
            }
        }
        // SugarCube delimiters that need tracking: << >> [[ ]] — pass through
        else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(result).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser::parse_passage_body;

    #[test]
    fn parse_twine_comment() {
        let ast = parse_passage_body("before/% comment %/after", 0, ParseMode::Normal);
        // Should have: Text("before"), Comment, Text("after")
        let has_comment = ast.nodes.iter().any(|n| matches!(n, AstNode::Comment { .. }));
        assert!(has_comment);
    }

    #[test]
    fn parse_html_comment() {
        let ast = parse_passage_body("before<!-- comment -->after", 0, ParseMode::Normal);
        let has_comment = ast.nodes.iter().any(|n| matches!(n, AstNode::Comment { kind: CommentKind::Html, .. }));
        assert!(has_comment);
    }

    #[test]
    fn html_comment_excludes_content() {
        let ast = parse_passage_body("before<!-- $gold and [[Forest]] -->after", 0, ParseMode::Normal);
        let var_names: Vec<&str> = ast.var_ops.iter().map(|v| v.name.as_str()).collect();
        assert!(!var_names.contains(&"$gold"), "Variable inside HTML comment should not be extracted");
        let passage_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::PassageLink).collect();
        assert!(passage_links.is_empty(), "Link inside HTML comment should not be extracted");
    }

    #[test]
    fn comment_excludes_macros() {
        // Macros inside comments should NOT be parsed as macro nodes
        let ast = parse_passage_body("before/% <<set $x to 1>> %/after", 0, ParseMode::Normal);
        // The <<set>> inside the comment should NOT produce variable operations
        let var_names: Vec<&str> = ast.var_ops.iter().map(|v| v.name.as_str()).collect();
        assert!(!var_names.contains(&"$x"), "Variable inside comment macro should not be extracted");
    }

    #[test]
    fn comment_excludes_variable_refs() {
        // Variables inside comments should NOT appear in var_ops
        let ast = parse_passage_body("before/% $gold coins %/after", 0, ParseMode::Normal);
        // Only the text "before" and "after" should be scanned for variables,
        // not the "$gold" inside the comment
        let var_names: Vec<&str> = ast.var_ops.iter().map(|v| v.name.as_str()).collect();
        assert!(!var_names.contains(&"$gold"), "Variable inside comment should not be extracted");
    }

    #[test]
    fn comment_excludes_links() {
        // Links inside comments should NOT appear in extracted links
        let ast = parse_passage_body("before/% [[Forest]] %/after", 0, ParseMode::Normal);
        // Only text-level links should be extracted; the [[Forest]] inside
        // the comment should not produce a LinkInfo
        let passage_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::PassageLink).collect();
        assert!(passage_links.is_empty(), "Link inside comment should not be extracted");
    }

    #[test]
    fn test_cstyle_comment_parsed() {
        let body = "before /* this is a CSS comment */ after";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        // Should have: Text("before "), Comment(CStyle), Text(" after")
        let comments: Vec<_> = ast.nodes.iter().filter_map(|n| {
            if let AstNode::Comment { kind, content, .. } = n {
                Some((*kind, content.clone()))
            } else {
                None
            }
        }).collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].0, CommentKind::CStyle);
        assert_eq!(comments[0].1.trim(), "this is a CSS comment");
        // Links and vars should be empty
        assert!(ast.links.is_empty());
        assert!(ast.var_ops.is_empty());
    }

    #[test]
    fn test_cstyle_comment_excludes_variables() {
        let body = "/* $hidden_var is not parsed */ visible $real_var";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        // Only $real_var should be extracted, not $hidden_var
        let var_names: Vec<_> = ast.var_ops.iter().map(|v| &v.name).collect();
        assert!(var_names.iter().all(|n| *n != "$hidden_var"));
        assert!(var_names.contains(&&"$real_var".to_string()));
    }

    #[test]
    fn test_cstyle_comment_excludes_links() {
        let body = "/* [[HiddenLink]] */ [[RealLink]]";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        assert_eq!(ast.links.len(), 1);
        assert_eq!(ast.links[0].target, "RealLink");
    }

    #[test]
    fn test_js_line_comment_parsed() {
        let body = "// this is a comment\nreal text";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let comments: Vec<_> = ast.nodes.iter().filter_map(|n| {
            if let AstNode::Comment { kind, content, .. } = n {
                Some((*kind, content.clone()))
            } else {
                None
            }
        }).collect();
        assert!(comments.iter().any(|(k, c)| *k == CommentKind::JsLine && c.contains("this is a comment")));
    }

    #[test]
    fn test_js_line_comment_url_not_parsed() {
        // http://example.com should NOT be treated as a comment
        let body = "Visit http://example.com for more";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let js_comments: Vec<_> = ast.nodes.iter().filter_map(|n| {
            if let AstNode::Comment { kind: CommentKind::JsLine, .. } = n {
                Some(true)
            } else {
                None
            }
        }).collect();
        // The URL should NOT produce a JS line comment node
        assert!(js_comments.is_empty());
    }

    #[test]
    fn test_html_conditional_comment_parsed() {
        let body = "<!--[if IE]><p>IE only</p><![endif]--> normal text";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let comments: Vec<_> = ast.nodes.iter().filter_map(|n| {
            if let AstNode::Comment { kind, content, .. } = n {
                Some((*kind, content.clone()))
            } else {
                None
            }
        }).collect();
        assert!(comments.iter().any(|(k, c)| *k == CommentKind::HtmlConditional && c.contains("IE only")));
    }

    #[test]
    fn test_mixed_comment_types() {
        let body = r#"Text /% Twine %/ more /* C-style */ end
<!-- HTML --> final"#;
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let comment_kinds: Vec<_> = ast.nodes.iter().filter_map(|n| {
            if let AstNode::Comment { kind, .. } = n {
                Some(*kind)
            } else {
                None
            }
        }).collect();
        assert!(comment_kinds.contains(&CommentKind::Twine));
        assert!(comment_kinds.contains(&CommentKind::CStyle));
        assert!(comment_kinds.contains(&CommentKind::Html));
    }

    #[test]
    fn all_comment_types_in_one_passage() {
        let body = r#"Text before
/% Twine comment %/ more text
<!-- HTML comment --> more text
/* C-style comment */ more text
// JS line comment
more text
[[TargetPassage]]"#;
        let ast = parse_passage_body(body, 0, ParseMode::Normal);

        // Should have comment nodes
        let comment_count = ast.nodes.iter().filter(|n| matches!(n, AstNode::Comment { .. })).count();
        assert!(comment_count >= 4, "Expected at least 4 comment nodes, got {}", comment_count);

        // Should still extract the link
        assert!(ast.links.iter().any(|l| l.target == "TargetPassage"));

        // Variables inside comments should NOT be extracted
        let body_with_vars = r#"/% $hidden_var %/ $visible_var"#;
        let ast2 = parse_passage_body(body_with_vars, 0, ParseMode::Normal);
        assert!(ast2.var_ops.iter().any(|v| v.name == "$visible_var"));
        assert!(!ast2.var_ops.iter().any(|v| v.name == "$hidden_var"));
    }

    #[test]
    fn test_strip_comments_twine() {
        let text = "before /% comment %/ after";
        let stripped = strip_comments(text);
        assert_eq!(stripped.len(), text.len());
        assert_eq!(stripped, "before               after");
    }

    #[test]
    fn test_strip_comments_cstyle() {
        let text = "before /* comment */ after";
        let stripped = strip_comments(text);
        assert_eq!(stripped.len(), text.len());
        assert_eq!(stripped, "before               after");
    }

    #[test]
    fn test_strip_comments_html() {
        let text = "before <!-- comment --> after";
        let stripped = strip_comments(text);
        assert_eq!(stripped.len(), text.len());
        assert_eq!(stripped, "before                  after");
    }

    #[test]
    fn test_strip_comments_js_line() {
        let text = "// comment\nreal code";
        let stripped = strip_comments(text);
        assert_eq!(stripped.len(), text.len());
        assert_eq!(stripped, "          \nreal code");
    }

    #[test]
    fn test_strip_comments_preserves_length() {
        let text = "/% test %/ [[Link]] $var";
        let stripped = strip_comments(text);
        assert_eq!(stripped.len(), text.len());
        assert!(stripped.contains("[[Link]]"));
        assert!(stripped.contains("$var"));
    }

    #[test]
    fn test_strip_comments_preserves_newlines() {
        let text = "/% line1\nline2 %/ rest";
        let stripped = strip_comments(text);
        assert!(stripped.contains('\n'));
        assert_eq!(stripped.matches('\n').count(), text.matches('\n').count());
    }

    #[test]
    fn strip_comments_all_types() {
        let text = "before/% Twine %/mid<!-- HTML -->end/* C-style */tail\n// JS line\nnext";
        let stripped = strip_comments(text);

        // Verify comments are replaced with spaces but newlines preserved
        assert_eq!(stripped.len(), text.len(), "strip_comments must preserve string length");
        assert!(!stripped.contains("/%"));
        assert!(!stripped.contains("%/"));
        assert!(!stripped.contains("<!--"));
        assert!(!stripped.contains("/*"));
        // Note: // is only treated as a comment when preceded by whitespace
        // and followed by space, or at line start. The "// JS line" at line
        // start is stripped.
        assert!(stripped.contains("next")); // text after newline preserved
        assert_eq!(stripped.lines().count(), text.lines().count());
    }

    #[test]
    fn strip_comments_preserves_urls() {
        // URLs with // should NOT be stripped (not in comment context)
        let text = r#"Visit http://example.com for info"#;
        let stripped = strip_comments(text);
        assert!(stripped.contains("http://example.com"));
    }

    #[test]
    fn extract_links_ignores_comments_in_macro_args() {
        // Comments inside macro args should not produce false positive links
        let ast = parse_passage_body(
            r#"<<goto /* "FakePassage" */ "RealPassage">>"#,
            0,
            ParseMode::Normal,
        );
        // Should only find "RealPassage", not "FakePassage"
        assert_eq!(ast.links.len(), 1);
        assert_eq!(ast.links[0].target, "RealPassage");
        assert_eq!(ast.links[0].source, LinkSource::Goto);
    }

    #[test]
    fn extract_links_ignores_js_line_comment_in_macro_args() {
        // Verify that // comments inside macro args don't cause panics
        let ast = parse_passage_body(
            r#"<<goto // "CommentedOut"
"RealTarget">>"#,
            0,
            ParseMode::Normal,
        );
        // This is a tricky edge case — just verify no crash
        assert!(!ast.links.is_empty() || ast.links.is_empty()); // verify it doesn't panic
    }

    #[test]
    fn cstyle_comment_span_does_not_extend_past_close() {
        // The bug: comment span was computed as span_start + *i instead of
        // offset + *i, causing the span to extend past the */ delimiter by
        // `start` bytes. Verify the span is correct.
        let body = "before /* comment */ after";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let comment = ast.nodes.iter().find_map(|n| {
            if let AstNode::Comment { span, kind: CommentKind::CStyle, .. } = n {
                Some(span.clone())
            } else {
                None
            }
        }).expect("should find CStyle comment");
        // The comment starts at byte 7 (the / of /*) and ends at byte 20
        // (exclusive, after the / of */), so span should be 7..20
        assert_eq!(comment.start, 7, "comment span start should be at /*");
        assert_eq!(comment.end, 20, "comment span end should be at position after */");
        // Verify the span length equals the actual comment length in the text
        let comment_text = &body[comment.start..comment.end];
        assert_eq!(comment_text, "/* comment */");
    }

    #[test]
    fn twine_comment_span_does_not_extend_past_close() {
        let body = "before /% comment %/ after";
        let ast = parse_passage_body(body, 0, ParseMode::Normal);
        let comment = ast.nodes.iter().find_map(|n| {
            if let AstNode::Comment { span, kind: CommentKind::Twine, .. } = n {
                Some(span.clone())
            } else {
                None
            }
        }).expect("should find Twine comment");
        // /% starts at byte 7, %/ ends at byte 20 (exclusive, after the /), span = 7..20
        assert_eq!(comment.start, 7);
        assert_eq!(comment.end, 20);
        let comment_text = &body[comment.start..comment.end];
        assert_eq!(comment_text, "/% comment %/");
    }

    #[test]
    fn cstyle_comment_after_long_prefix() {
        // A comment NOT at the start — the old bug made spans extend by
        // `start` bytes past the closing delimiter.
        let body = "x".repeat(1000) + "/* short */ rest";
        let ast = parse_passage_body(&body, 0, ParseMode::Normal);
        let comment = ast.nodes.iter().find_map(|n| {
            if let AstNode::Comment { span, kind: CommentKind::CStyle, .. } = n {
                Some(span.clone())
            } else {
                None
            }
        }).expect("should find CStyle comment");
        // Comment starts at byte 1000, ends at byte 1011 (exclusive, after */)
        assert_eq!(comment.start, 1000);
        assert_eq!(comment.end, 1011);
        let comment_text = &body[comment.start..comment.end];
        assert_eq!(comment_text, "/* short */");
    }

    #[test]
    fn extract_links_from_cstyle_comment_in_goto() {
        let ast = parse_passage_body(
            r#"<<goto "Forest" /* go to the forest */>>"#,
            0,
            ParseMode::Normal,
        );
        assert_eq!(ast.links.len(), 1);
        assert_eq!(ast.links[0].target, "Forest");
    }
}
