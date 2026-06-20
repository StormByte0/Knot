//! Core parser — main parse loop and text flushing.

use crate::sugarcube::ast::*;
use super::predicates::is_ident_start;
use super::variable_scan::{scan_variable, scan_inline_vars};
use super::macro_parser::parse_macro;
use super::link_parser::parse_link;
use super::comment::{
    parse_block_comment,
    parse_cstyle_comment,
    parse_js_line_comment,
    parse_html_comment,
    parse_html_conditional_comment,
};

/// Parse body text into AST nodes.
///
/// `offset` is the byte offset within the body where this segment starts
/// (0 for the top level, nonzero for nested content inside block macros).
pub(super) fn parse_body(text: &str, offset: usize) -> Vec<AstNode> {
    let mut nodes = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;
    let bytes = text.as_bytes();
    let len = bytes.len();

    while i < len {
        // Try to match a delimiter at the current position
        let matched = match bytes[i] {
            b'<' if i + 1 < len && bytes[i + 1] == b'<' => {
                // << — macro open
                let start = i;
                i += 2;
                let node = parse_macro(text, &mut i, offset, start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'[' if i + 1 < len && bytes[i + 1] == b'[' => {
                // [[ — link
                let start = i;
                i += 2;
                let node = parse_link(text, &mut i, offset, start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'%' => {
                // /% or /%% — Twine/SugarCube comment
                let start = i;
                let is_sugarcube = i + 2 < len && bytes[i + 2] == b'%';
                let delim_len = if is_sugarcube { 3 } else { 2 };
                i += delim_len;
                let node = parse_block_comment(text, &mut i, offset + start, is_sugarcube);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                // /* — C-style block comment (CSS/JS)
                let start = i;
                i += 2;
                let node = parse_cstyle_comment(text, &mut i, offset + start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                // // — Could be italic formatting (//text//) or a JS line comment.
                // Check for italic first: if there's a closing // on the same line,
                // it's formatting, not a comment.
                let has_closing_double_slash = {
                    let mut k = i + 2;
                    let mut found = false;
                    while k + 1 < len && bytes[k] != b'\n' {
                        if bytes[k] == b'/' && bytes[k + 1] == b'/' {
                            found = true;
                            break;
                        }
                        k += 1;
                    }
                    found
                };

                if has_closing_double_slash {
                    // Italic formatting: //text//
                    let start = i;
                    i += 2;
                    let content_start = i;
                    while i + 1 < len && !(bytes[i] == b'/' && bytes[i + 1] == b'/') {
                        i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                    }
                    let content = text[content_start..i].to_string();
                    if i + 1 < len {
                        i += 2; // skip closing //
                    }
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(AstNode::TextFormat {
                        kind: TextFormatKind::Italic,
                        content,
                        span: offset + start..offset + i,
                    })
                } else {
                    // Not italic — check if this // is a line comment.
                    //
                    // In SugarCube prose, // is ALWAYS a comment (unless it's
                    // italic formatting, which we already checked above). The
                    // only exception is // inside a URL like http://example.com
                    // — but there // is preceded by ':', not whitespace.
                    //
                    // So the rule is simple: if // is at line start OR preceded
                    // by whitespace (space/tab), it's a comment. We do NOT
                    // require a space AFTER // — `//comment` (no space) is just
                    // as valid a comment as `// comment`.
                    let is_comment_context = if i + 2 <= len {
                        let at_line_start = i == 0 || bytes[i - 1] == b'\n';
                        let preceded_by_whitespace = i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t');
                        at_line_start || preceded_by_whitespace
                    } else {
                        true
                    };
                    if is_comment_context {
                        let start = i;
                        i += 2;
                        let node = parse_js_line_comment(text, &mut i, offset + start);
                        flush_text(text, &mut text_start, start, offset, &mut nodes);
                        Some(node)
                    } else {
                        i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                        None
                    }
                }
            }
            b'\'' if i + 1 < len && bytes[i + 1] == b'\'' => {
                // '' — bold formatting: ''text''
                let start = i;
                i += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'\'' && bytes[i + 1] == b'\'') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Bold, content, span: offset + start..offset + i,
                })
            }
            b'_' if i + 1 < len && bytes[i + 1] == b'_' => {
                // __ — underline formatting: __text__
                let start = i;
                i += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'_' && bytes[i + 1] == b'_') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Underline, content, span: offset + start..offset + i,
                })
            }
            b'=' if i + 1 < len && bytes[i + 1] == b'=' => {
                // == — strike formatting: ==text==
                let start = i;
                i += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'=' && bytes[i + 1] == b'=') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Strike, content, span: offset + start..offset + i,
                })
            }
            b'~' if i + 1 < len && bytes[i + 1] == b'~' => {
                // ~~ — subscript formatting: ~~text~~
                let start = i;
                i += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'~' && bytes[i + 1] == b'~') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Sub, content, span: offset + start..offset + i,
                })
            }
            b'^' if i + 1 < len && bytes[i + 1] == b'^' => {
                // ^^ — superscript formatting: ^^text^^
                let start = i;
                i += 2;
                let content_start = i;
                while i + 1 < len && !(bytes[i] == b'^' && bytes[i + 1] == b'^') {
                    i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
                let content = text[content_start..i].to_string();
                if i + 1 < len { i += 2; }
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(AstNode::TextFormat {
                    kind: TextFormatKind::Super, content, span: offset + start..offset + i,
                })
            }
            b'<' if i + 3 < len && &text[i..i + 4] == "<!--" => {
                // <!-- — HTML comment (or conditional comment <!--[if ...]>)
                let start = i;
                i += 4;
                // Check for conditional comment: <!--[if ...]>
                let is_conditional = text[i..].trim_start().starts_with("[if");
                if is_conditional {
                    let node = parse_html_conditional_comment(text, &mut i, offset + start);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                } else {
                    let node = parse_html_comment(text, &mut i, offset + start);
                    flush_text(text, &mut text_start, start, offset, &mut nodes);
                    Some(node)
                }
            }
            b'$' if i + 1 < len && bytes[i + 1] == b'$' => {
                // $$ — escaped dollar, include in text
                i += 2;
                None
            }
            b'$' if i + 1 < len && is_ident_start(bytes[i + 1]) => {
                // $var — story variable in text
                let (_var_ref, end) = scan_variable(text, i, false);
                // Don't create a separate node for inline vars in text.
                // Instead, they'll be picked up when we flush the text node.
                i = end;
                // We don't break the text gap here — inline $vars in prose
                // are part of the text flow. The var_refs will be extracted
                // from the text content when the text node is flushed.
                // However, we DO want to track the position for the text
                // node's var_refs list. So let's just advance and let
                // flush_text extract them.
                None
            }
            b'@' if i + 1 < len && (bytes[i + 1] == b'@' || bytes[i + 1] == b'.' || bytes[i + 1] == b'#' || is_ident_start(bytes[i + 1])) => {
                // @ or @@ — SugarCube inline styling
                // Double-at: @@class;text@@
                // Single-at: @class;text@ (class may start with . or # for CSS selectors)
                let is_double_at = bytes[i + 1] == b'@';
                let start = i;
                i += if is_double_at { 2 } else { 1 };
                let node = parse_inline_style(text, &mut i, offset, start, is_double_at);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            _ => {
                i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
                None
            }
        };

        if let Some(node) = matched {
            nodes.push(node);
            // After a delimiter has been consumed (macro, link, comment),
            // the text gap must start at the current position (past the
            // consumed content), not at the delimiter's start position.
            // Without this, the final flush would include the delimiter
            // content as text, causing variables/links inside comments to
            // be incorrectly extracted.
            text_start = i;
        }
    }

    // Flush remaining text
    flush_text(text, &mut text_start, len, offset, &mut nodes);

    nodes
}

/// Flush accumulated text into a Text node.
///
/// `text_start` is updated to `end` after flushing.
fn flush_text(
    text: &str,
    text_start: &mut usize,
    end: usize,
    offset: usize,
    nodes: &mut Vec<AstNode>,
) {
    if *text_start >= end {
        return;
    }
    let content = text[*text_start..end].to_string();
    if content.is_empty() {
        return;
    }

    // Extract inline variable references from this text gap
    let var_refs = scan_inline_vars(&content, offset + *text_start);

    nodes.push(AstNode::Text {
        content,
        var_refs,
        span: offset + *text_start..offset + end,
        is_prose: true, // Default: top-level text is always prose.
        // The tree builder will set is_prose = false for Text nodes
        // inside non-rendering macros (<<silently>>, <<script>>, <<style>>).
    });
    *text_start = end;
}

/// Parse SugarCube inline styling markup (`@@class;text@@` or `@class;text@`).
///
/// `i` points to the first character after the opening `@@` or `@`.
/// `start` is the position of the first `@` in `text`.
/// `is_double_at` is `true` for `@@...@@`, `false` for `@...@`.
///
/// For double-at: the class is between `@@` and `;`, the body is between
/// `;` and `@@`. The close delimiter is `@@`.
///
/// For single-at: same structure but with single `@` delimiters.
fn parse_inline_style(
    text: &str,
    i: &mut usize,
    offset: usize,
    start: usize,
    is_double_at: bool,
) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Find the class name (up to ; or the close delimiter)
    let (class, class_span, body_start) = find_class_and_body_start(
        text, *i, offset, is_double_at,
    );

    // Find the closing delimiter
    let close_delim = if is_double_at { "@@" } else { "@" };
    let body_end = text[body_start - offset..]
        .find(close_delim)
        .map(|pos| body_start - offset + pos)
        .unwrap_or(len);

    let body_content = if body_start - offset < body_end {
        &text[body_start - offset..body_end]
    } else {
        ""
    };

    // Recursively parse the body content for variables, links, etc.
    let children = if !body_content.is_empty() {
        parse_body(body_content, body_start - offset + offset)
    } else {
        Vec::new()
    };

    // Advance past the closing delimiter
    *i = body_end + close_delim.len();

    AstNode::InlineStyle {
        class,
        class_span,
        children,
        span: offset + start..offset + *i,
    }
}

/// Find the class name and body start position in an inline style construct.
///
/// Returns `(class, class_span, body_start)` where `body_start` is the
/// passage-body-relative byte offset where the body content begins.
fn find_class_and_body_start(
    text: &str,
    content_start: usize,
    offset: usize,
    is_double_at: bool,
) -> (String, std::ops::Range<usize>, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Find ; separating class from body, or the close delimiter if no ;
    let mut j = content_start;
    while j < len {
        if bytes[j] == b';' {
            // Found the class/body separator
            let class = text[content_start..j].to_string();
            let class_span = offset + content_start..offset + j;
            return (class, class_span, offset + j + 1); // body starts after ;
        }
        // Check for close delimiter
        if is_double_at && j + 1 < len && bytes[j] == b'@' && bytes[j + 1] == b'@' {
            break;
        }
        if !is_double_at && bytes[j] == b'@' {
            break;
        }
        j += text[j..].chars().next().map_or(1, |c| c.len_utf8());
    }

    // No ; found — the entire content is the class, no body
    let class = text[content_start..j].to_string();
    let class_span = offset + content_start..offset + j;
    (class, class_span, offset + j)
}

#[cfg(test)]
mod tests {
    use crate::sugarcube::ast::{AstNode, CommentKind, ParseMode, LinkSource, TextFormatKind};

    #[test]
    fn line_comment_no_space_after_slashes_is_recognized() {
        // //comment (no space after //) should be recognized as a comment,
        // not treated as prose text. This was the user's bug report:
        // `<<link "x" "y">>  //content1` was getting a Prose token instead
        // of a Comment token because the old heuristic required a space
        // AFTER // (followed_by_space).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "//content1", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 1);
        assert!(matches!(&ast.nodes[0], AstNode::Comment { kind: CommentKind::JsLine, .. }),
            "//content1 should be a Comment node, got {:?}", ast.nodes[0]);
    }

    #[test]
    fn line_comment_after_macro_no_space_is_recognized() {
        // <<link "x" "y">>  //content1 — the //content1 has no space after //
        // but IS preceded by spaces (after >>). Should be a Comment.
        // Note: <<link>> is a Required-body macro, so it goes on the stack
        // and the comment becomes its child (not a top-level node).
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<link \"x\" \"y\">>  //content1", 0, ParseMode::Normal,
        );

        fn has_comment_recursive(nodes: &[AstNode]) -> bool {
            for n in nodes {
                if matches!(n, AstNode::Comment { kind: CommentKind::JsLine, .. }) {
                    return true;
                }
                if let AstNode::Macro { children: Some(ch), .. } = n {
                    if has_comment_recursive(ch) {
                        return true;
                    }
                }
            }
            false
        }
        assert!(has_comment_recursive(&ast.nodes),
            "should have a Comment node for //content1 somewhere in the tree");
    }

    #[test]
    fn line_comment_inside_block_no_space_is_recognized() {
        // The user's exact scenario: //content2 and //content3 inside
        // a <<link>> block, with no space after //.
        let input = "<<link \"Chat\" \"Coworker\">>  //content1\n  <<if true>>  //content2\n    <<adjustStat \"stress\" -3>>  //content3\n    <<addTime 10>>\n  <</if>>\n<</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);

        // Collect all Comment nodes by walking the tree
        fn count_comments(nodes: &[AstNode]) -> usize {
            let mut count = 0;
            for n in nodes {
                if matches!(n, AstNode::Comment { .. }) {
                    count += 1;
                }
                if let AstNode::Macro { children: Some(ch), .. } = n {
                    count += count_comments(ch);
                }
            }
            count
        }
        let comment_count = count_comments(&ast.nodes);
        assert_eq!(comment_count, 3,
            "should have 3 Comment nodes (//content1, //content2, //content3), got {}", comment_count);
    }

    #[test]
    fn trailing_text_after_inline_macro_not_swallowed() {
        // Regression: inline macros like <<set>> were pushed onto the tree
        // builder's stack, swallowing trailing text/comments into
        // pending_children and dropping them when finalized as inline.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<set $x to 1>> some narrative text", 0, ParseMode::Normal,
        );
        assert_eq!(ast.nodes.len(), 2, "should have Macro + Text nodes");
        assert!(matches!(&ast.nodes[0], AstNode::Macro { name, .. } if name == "set"));
        match &ast.nodes[1] {
            AstNode::Text { content, .. } => assert!(content.contains("narrative text")),
            other => panic!("expected Text node, got {:?}", other),
        }
    }

    #[test]
    fn parse_simple_text() {
        let ast = crate::sugarcube::parser::parse_passage_body("Hello world", 0, ParseMode::Normal);
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Text { content, .. } => assert_eq!(content, "Hello world"),
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn parse_variable_in_text() {
        let ast = crate::sugarcube::parser::parse_passage_body("You have $gold coins.", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert_eq!(ast.var_ops[0].name, "$gold");
        assert!(!ast.var_ops[0].is_write);
    }

    #[test]
    fn parse_temp_variable() {
        let ast = crate::sugarcube::parser::parse_passage_body("<<set _i to 0>>", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert!(ast.var_ops.iter().any(|v| v.name == "_i" && v.is_temporary && v.is_write));
    }

    #[test]
    fn escaped_dollar() {
        let ast = crate::sugarcube::parser::parse_passage_body("$$notavar", 0, ParseMode::Normal);
        // $$ should not be treated as a variable reference
        match &ast.nodes[0] {
            AstNode::Text { content, var_refs, .. } => {
                assert!(content.contains("$$notavar"));
                assert!(var_refs.is_empty());
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn stylesheet_mode_empty() {
        let ast = crate::sugarcube::parser::parse_passage_body("body { color: red; }", 0, ParseMode::Stylesheet);
        assert!(ast.nodes.is_empty());
    }

    #[test]
    fn script_mode_empty() {
        let ast = crate::sugarcube::parser::parse_passage_body("var x = 5;", 0, ParseMode::Script);
        assert!(ast.nodes.is_empty());
    }

    #[test]
    fn graph_connections_from_ast() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            r#"[[Forest]] <<goto "Cave">> <<include "Shop">>"#,
            0,
            ParseMode::Normal,
        );
        let connections = ast.graph_connections();
        assert!(connections.iter().any(|c| c.target == "Forest" && c.edge_type == knot_core::graph::EdgeType::Navigation));
        assert!(connections.iter().any(|c| c.target == "Cave" && c.edge_type == knot_core::graph::EdgeType::Navigation));
        assert!(connections.iter().any(|c| c.target == "Shop" && c.edge_type == knot_core::graph::EdgeType::Include));
    }

    #[test]
    fn interface_mode_extracts_data_passage() {
        let html = r#"<div id="story"><div data-passage="Sidebar"></div></div>"#;
        let ast = crate::sugarcube::parser::parse_passage_body(html, 0, ParseMode::Interface);
        let dp_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::DataPassage).collect();
        assert_eq!(dp_links.len(), 1);
        assert_eq!(dp_links[0].target, "Sidebar");
    }

    #[test]
    fn data_passage_extraction() {
        let html = r#"<div data-passage="SidebarStats"></div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "SidebarStats");
        assert_eq!(links[0].source, LinkSource::DataPassage);
    }

    #[test]
    fn data_passage_single_quotes() {
        let html = "<div data-passage='MyPassage'></div>";
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "MyPassage");
    }

    #[test]
    fn data_passage_multiple() {
        let html = r#"<div data-passage="P1"></div><div data-passage="P2"></div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "P1");
        assert_eq!(links[1].target, "P2");
    }

    #[test]
    fn data_passage_ignores_comments() {
        let html = r#"<div data-passage="RealTarget">/* "FakeTarget" */</div>"#;
        let links = super::super::extraction::extract_data_passage_refs(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "RealTarget");
    }

    // ── Prose context tests ──────────────────────────────────────────────

    #[test]
    fn prose_top_level_text_is_prose() {
        // Top-level text in a passage body is always prose
        let ast = crate::sugarcube::parser::parse_passage_body("Hello world", 0, ParseMode::Normal);
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Text { content, is_prose, .. } => {
                assert_eq!(content, "Hello world");
                assert!(*is_prose, "top-level text should be prose");
            }
            _ => panic!("Expected Text node"),
        }
    }

    #[test]
    fn prose_inside_if_is_prose() {
        // Text inside <<if>> body is prose — it renders to the player.
        // Using a simple <<if>> without <<else>> to avoid the known tree builder
        // issue where <<else>> (a BodyRequirement::Never inline clause marker)
        // consumes subsequent text as pending_children that get discarded.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<if true>>go to town<</if>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "if");
                match &children[0] {
                    AstNode::Text { content, is_prose, .. } => {
                        assert!(content.contains("go to town"));
                        assert!(*is_prose, "text inside <<if>> should be prose");
                    }
                    _ => panic!("Expected Text node inside <<if>>"),
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_silently_is_not_prose() {
        // Text inside <<silently>> is NOT prose — it's executed but not rendered
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<silently>>some text<</silently>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "silently");
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<silently>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_script_is_not_prose() {
        // <<script>> body is not prose — it's code
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<script>>var x = 1;<</script>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "script");
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<script>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_mixed_context() {
        // Text both inside and outside <<silently>>
        let ast = crate::sugarcube::parser::parse_passage_body(
            "visible text<<silently>>hidden code<</silently>>more visible",
            0,
            ParseMode::Normal,
        );
        // The tree builder nests children into <<silently>>, so:
        //   [Text("visible text"), Macro("silently", children=[...]), Text("more visible")]
        // But "more visible" might end up inside the silently macro's children
        // if the tree builder picks it up before the close tag. Let's check
        // the actual structure flexibly.
        let top_text_nodes: Vec<_> = ast.nodes.iter()
            .filter_map(|n| match n {
                AstNode::Text { content, is_prose, .. } => Some((content.clone(), *is_prose)),
                _ => None,
            })
            .collect();

        // "visible text" should be prose
        let visible = top_text_nodes.iter().find(|(c, _)| c.contains("visible text"));
        assert!(visible.is_some(), "should find 'visible text' as top-level node");
        assert!(visible.unwrap().1, "top-level text before <<silently>> should be prose");

        // Find the silently macro and verify its children are NOT prose
        let silently = ast.nodes.iter().find_map(|n| match n {
            AstNode::Macro { name, children, .. } if name == "silently" => children.clone(),
            _ => None,
        });
        if let Some(silently_children) = silently {
            for child in &silently_children {
                if let AstNode::Text { content, is_prose, .. } = child {
                    if content.contains("hidden") {
                        assert!(!*is_prose, "text inside <<silently>> should NOT be prose");
                    }
                }
            }
        }

        // "more visible" should be prose (it's after <</silently>>)
        let more_visible = top_text_nodes.iter().find(|(c, _)| c.contains("more visible"));
        if let Some((_, is_prose)) = more_visible {
            assert!(is_prose, "top-level text after <<silently>> should be prose");
        }
        // If "more visible" isn't a top-level node, it may be inside the
        // silently macro — but that shouldn't happen with proper close-tag pairing.
    }

    #[test]
    fn prose_nested_if_inside_silently() {
        // <<silently>><<if>>text<</if>><</silently>> — text inside nested <<if>>
        // should still be non-prose because the parent <<silently>> suppresses rendering
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<silently>><<if true>>hidden<</if>><</silently>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "silently");
                // Find the <<if>> macro inside
                for child in children {
                    if let AstNode::Macro { name, children: Some(if_children), .. } = child {
                        assert_eq!(name, "if");
                        for if_child in if_children {
                            if let AstNode::Text { is_prose, .. } = if_child {
                                assert!(!*is_prose,
                                    "text inside <<if>> nested in <<silently>> should NOT be prose");
                            }
                        }
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn prose_inside_done_is_not_prose() {
        // <<done>> executes code after rendering — its body is not narrative prose.
        let ast = crate::sugarcube::parser::parse_passage_body(
            "<<done>><<set $x to 1>><</done>>",
            0,
            ParseMode::Normal,
        );
        match &ast.nodes[0] {
            AstNode::Macro { name, children: Some(children), .. } => {
                assert_eq!(name, "done");
                // The tree builder should have marked any Text children as non-prose
                for child in children {
                    if let AstNode::Text { is_prose, .. } = child {
                        assert!(!*is_prose, "text inside <<done>> should NOT be prose");
                    }
                }
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn expression_sigil_emits_macro_token() {
        // <<=>> should emit a Macro token for the = sigil
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<= $hp>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        // Should have at least a Macro token for the = sigil and a Variable token for $hp
        let macro_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!macro_tokens.is_empty(), "<<=>> should emit a Macro token for the sigil");
        // The sigil token should be at offset 2 (past <<) and length 1
        let sigil = &macro_tokens[0];
        assert_eq!(sigil.start, 2, "sigil token should start at offset 2");
        assert_eq!(sigil.length, 1, "sigil token length should be 1");
        assert!(sigil.modifier.is_none(), "<<=>> sigil should have no modifier");
    }

    #[test]
    fn silent_expression_sigil_has_control_flow_modifier() {
        // <<->> should emit a Macro token with ControlFlow modifier
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<- $hp>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let sigil_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();
        assert!(!sigil_tokens.is_empty(), "<<->> should emit a Macro token for the sigil");
        let sigil = &sigil_tokens[0];
        assert_eq!(sigil.start, 2, "sigil token should start at offset 2");
        assert_eq!(sigil.length, 1, "sigil token length should be 1");
        assert_eq!(sigil.modifier, Some(SemanticTokenModifier::ControlFlow),
            "<<->> sigil should have ControlFlow modifier");
    }

    #[test]
    fn inline_macro_emits_open_close_delimiter_tokens() {
        // <<set $hp to 10>> should emit two MacroDelimiter tokens:
        //   - `<<` at offset 0, length 2
        //   - `>>` at offset 14, length 2
        // Both with no modifier (inline macro, not deprecated).
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<set $hp to 10>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 2, "inline macro should emit exactly 2 delimiter tokens (<< and >>)");

        // Sort by start offset to make assertions order-independent
        let mut delims_sorted = delims.clone();
        delims_sorted.sort_by_key(|t| t.start);

        // First delimiter: `<<`
        assert_eq!(delims_sorted[0].start, 0, "`<<` should start at offset 0");
        assert_eq!(delims_sorted[0].length, 2, "`<<` should be length 2");
        assert!(delims_sorted[0].modifier.is_none(),
            "inline non-deprecated macro delimiters should have no modifier");

        // Second delimiter: `>>` — at the end of the input minus 2
        let expected_close_start = input.len() - 2;
        assert_eq!(delims_sorted[1].start, expected_close_start,
            "`>>` should start at offset {}", expected_close_start);
        assert_eq!(delims_sorted[1].length, 2, "`>>` should be length 2");
        assert!(delims_sorted[1].modifier.is_none(),
            "inline non-deprecated macro delimiters should have no modifier");
    }

    #[test]
    fn block_macro_emits_four_delimiter_tokens_with_depth() {
        // <<if $hp gte 10>>Alive<</if>> should emit four MacroDelimiter tokens:
        //   - `<<` at offset 0 (open)
        //   - `>>` at offset 16 (open end)
        //   - `<</` at offset 22 (close start)
        //   - `>>` at offset 28 (close end)
        // Top-level block macro is at depth 0 → all four delimiters get None
        // (base delimiter color). Depth modifiers only kick in when nested.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<if $hp gte 10>>Alive<</if>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 4,
            "block macro should emit 4 delimiter tokens (<<, >>, <</, >>), got {:?}", delims);

        // All four should have None (top-level, depth 0 = base color)
        for (i, d) in delims.iter().enumerate() {
            assert!(d.modifier.is_none(),
                "delimiter {} should have NO modifier (depth 0 = base color), got {:?}", i, d.modifier);
        }

        // Verify the `<</` is 3 bytes
        let slash_open = delims.iter()
            .find(|t| t.length == 3)
            .expect("should have one 3-byte delimiter (`<</`)");
        assert!(slash_open.start >= 17,
            "`<</` should start after the open tag's `>>`");
    }

    #[test]
    fn nested_block_macros_delimiters_track_depth() {
        // Outer <<if>> at depth 0 (top-level), inner <<if>> at depth 1.
        // Outer delimiters → None (base, depth 0)
        // Inner delimiters → BlockDepth1 (depth 1, inside one block)
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<if $a>><<if $b>>nested<</if>><</if>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let depth0_delims = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)
                && t.modifier.is_none())
            .count();
        let depth1_delims = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)
                && t.modifier == Some(SemanticTokenModifier::BlockDepth1))
            .count();

        // Outer block macro: 4 delimiters at depth 0 (None) — <<, >>, <</, >>
        // Inner block macro: 4 delimiters at depth 1 (BlockDepth1) — <<, >>, <</, >>
        assert_eq!(depth0_delims, 4, "outer macro should contribute 4 base-color delimiters (None modifier)");
        assert_eq!(depth1_delims, 4, "inner macro should contribute 4 BlockDepth1 delimiters");
    }

    #[test]
    fn inline_macro_inside_block_one_deeper_than_block() {
        // Depth semantics: DELIMITERS track nesting depth, but the macro
        // NAME does NOT — the name always uses the base `macro` color so
        // the identifier stays visually stable regardless of nesting.
        //
        // So `<<set>>` inside `<<link>>`:
        //   - `link` name → None (base macro color)
        //   - `set` name  → None (base macro color)
        //   - `<<link>>` delimiters → None (depth 0 = base delimiter color)
        //   - `<<set>>` delimiters  → BlockDepth1 (depth 1, inside one block)
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<link \"Go\" \"Forest\">><<set $x to 1>><</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        // <<link>> name at offset 2, length 4 — should be None (base color, no depth)
        let link_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.start == 2 && t.length == 4)
            .expect("should find link name token");
        assert!(link_name.modifier.is_none(),
            "<<link>> name should have NO depth modifier (base macro color), got {:?}",
            link_name.modifier);

        // <<set>> name at offset 24, length 3 — should be None (base color, no depth)
        let set_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.start == 24 && t.length == 3)
            .expect("should find set name token");
        assert!(set_name.modifier.is_none(),
            "<<set>> name should have NO depth modifier (base macro color), got {:?}",
            set_name.modifier);

        // <<link>> delimiters (offset 0) → None (depth 0 = base color)
        let link_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 0)
            .expect("should find `<<` delimiter for <<link>>");
        assert!(link_open_delim.modifier.is_none(),
            "<<link>> `<<` delimiter should have NO modifier (depth 0 = base), got {:?}",
            link_open_delim.modifier);

        // <<set>> delimiters (offset 22, 35) → BlockDepth1 (depth 1, inside link)
        let set_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 22)
            .expect("should find `<<` delimiter for inner <<set>>");
        assert_eq!(set_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "inner `<<` delimiter should be BlockDepth1 (inside one block), got {:?}",
            set_open_delim.modifier);

        let set_close_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 35)
            .expect("should find `>>` delimiter for inner <<set>>");
        assert_eq!(set_close_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "inner `>>` delimiter should be BlockDepth1 (inside one block), got {:?}",
            set_close_delim.modifier);
    }

    #[test]
    fn deeply_nested_inline_macro_inside_two_blocks() {
        // The user's exact scenario from chat:
        //
        //   <<link>>                  // delimiters: None (depth 0 = base)
        //     <<if true>>             // delimiters: BlockDepth1 (depth 1)
        //       <<adjustStat ...>>    // delimiters: BlockDepth2 (depth 2) ← key assertion
        //     <</if>>
        //   <</link>>
        //
        // The macro NAMES (link, if, adjustStat) all use the base `macro`
        // color — NO depth modifier on names. Only the delimiters track depth.
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<link \"Chat\" \"Coworker\">><<if true>><<adjustStat \"stress\" -3>><</if>><</link>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let macro_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Macro))
            .collect();

        // All macro NAMES should have NO depth modifier — base macro color only.
        // (link at offset 2 len 4, if at offset 28 len 2, adjustStat at offset 39 len 10)
        for t in &macro_tokens {
            assert!(t.modifier.is_none(),
                "macro name at offset {} should have NO depth modifier (base color only), got {:?}",
                t.start, t.modifier);
        }

        // Delimiters track depth — verify the `<<` before each name.
        // <<link>> `<<` at offset 0 → None (depth 0 = base color)
        let link_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 0)
            .expect("should find `<<` delimiter for <<link>>");
        assert!(link_open_delim.modifier.is_none(),
            "<<link>> `<<` delimiter should have NO modifier (depth 0 = base), got {:?}",
            link_open_delim.modifier);

        // <<if>> `<<` at offset 26 (28 - 2) → BlockDepth1 (depth 1, inside link)
        let if_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 26)
            .expect("should find `<<` delimiter for <<if>>");
        assert_eq!(if_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth1),
            "<<if>> `<<` delimiter should be BlockDepth1 (inside one block), got {:?}",
            if_open_delim.modifier);

        // <<adjustStat>> `<<` at offset 37 (39 - 2) → BlockDepth2 (depth 2, inside link+if)
        let adjust_open_delim = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter) && t.start == 37)
            .expect("should find `<<` delimiter at offset 37 (immediately before adjustStat)");
        assert_eq!(adjust_open_delim.modifier, Some(SemanticTokenModifier::BlockDepth2),
            "<<adjustStat>>'s `<<` delimiter should be BlockDepth2 (inside two blocks), got {:?}",
            adjust_open_delim.modifier);
    }

    #[test]
    fn top_level_inline_macro_has_no_depth_modifier() {
        // Sanity: a bare `<<set>>` at the top level (not inside any block)
        // should still get `None` for its modifier — no enclosing block to
        // inherit depth from. This guards against the fix above over-applying
        // depth modifiers to top-level inline macros.
        use crate::plugin::{SemanticTokenType, SemanticTokenModifier};
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<set $x to 1>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let set_name = tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Macro) && t.length == 3)
            .expect("should find `set` name token");
        assert!(set_name.modifier.is_none(),
            "top-level `<<set>>` should have no depth modifier, got {:?}",
            set_name.modifier);

        // All delimiter tokens at top level should also have no modifier
        for t in tokens.iter().filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter)) {
            assert!(t.modifier.is_none(),
                "top-level delimiter at offset {} should have no modifier, got {:?}",
                t.start, t.modifier);
        }
    }

    #[test]
    fn expression_macro_emits_delimiter_tokens() {
        // <<= $hp>> should emit two MacroDelimiter tokens for `<<` and `>>`.
        // The sigil (`=`) stays as a Macro token — delimiters are separate.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let input = "<<= $hp>>";
        let ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let delims: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter))
            .collect();
        assert_eq!(delims.len(), 2,
            "expression macro should emit 2 delimiter tokens (<< and >>)");

        let mut sorted = delims.clone();
        sorted.sort_by_key(|t| t.start);
        assert_eq!(sorted[0].start, 0, "`<<` at offset 0");
        assert_eq!(sorted[0].length, 2, "`<<` length 2");
        assert_eq!(sorted[1].start, input.len() - 2, "`>>` at end-2");
        assert_eq!(sorted[1].length, 2, "`>>` length 2");
    }

    #[test]
    fn delimiter_tokens_are_distinct_type_from_name() {
        // Sanity: the macro NAME token must be `Macro`, not `MacroDelimiter`,
        // and vice versa. This guards against accidental collapse.
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body("<<set $x to 1>>", 0, ParseMode::Normal);
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let has_macro_name = tokens.iter().any(|t| matches!(t.token_type, SemanticTokenType::Macro));
        let has_delimiter = tokens.iter().any(|t| matches!(t.token_type, SemanticTokenType::MacroDelimiter));
        assert!(has_macro_name, "should have a Macro token for the name `set`");
        assert!(has_delimiter, "should have MacroDelimiter tokens for << >>");
    }

    #[test]
    fn print_and_expression_emit_equivalent_variable_tokens() {
        // <<print $hp>> and <<= $hp>> should emit the same Variable tokens
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast_print = crate::sugarcube::parser::parse_passage_body(
            "<<print $hp>>", 0, ParseMode::Normal,
        );
        let ast_expr = crate::sugarcube::parser::parse_passage_body(
            "<<= $hp>>", 0, ParseMode::Normal,
        );

        let mut tokens_print = Vec::new();
        let mut tokens_expr = Vec::new();
        build_semantic_tokens(&ast_print.nodes, &mut tokens_print, 0, &HashSet::new());
        build_semantic_tokens(&ast_expr.nodes, &mut tokens_expr, 0, &HashSet::new());

        let var_tokens_print: Vec<_> = tokens_print.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .collect();
        let var_tokens_expr: Vec<_> = tokens_expr.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .collect();

        assert!(!var_tokens_print.is_empty(), "<<print>> should emit Variable tokens");
        assert!(!var_tokens_expr.is_empty(), "<<=>> should emit Variable tokens");
        // Both should have the same number of Variable tokens for the same expression
        assert_eq!(var_tokens_print.len(), var_tokens_expr.len(),
            "<<print>> and <<=>> should emit the same number of Variable tokens");
    }

    #[test]
    fn inline_style_double_at() {
        // @@.highlight;important text@@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;important text@@", 0, ParseMode::Normal,
        );
        let style_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::InlineStyle { class, .. } => Some(class.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        assert_eq!(style_node, ".highlight");

        // Verify children contain prose text
        let style = ast.nodes.iter().find_map(|n| match n {
            node @ AstNode::InlineStyle { .. } => Some(node.clone()),
            _ => None,
        }).unwrap();
        match &style {
            AstNode::InlineStyle { children, .. } => {
                assert!(!children.is_empty(), "InlineStyle should have children");
                let has_prose = children.iter().any(|c| matches!(c, AstNode::Text { is_prose: true, .. }));
                assert!(has_prose, "children should contain prose text");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn inline_style_single_at() {
        // @.red;warning text@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@.red;warning text@", 0, ParseMode::Normal,
        );
        let style_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::InlineStyle { class, .. } => Some(class.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        assert_eq!(style_node, ".red");
    }

    #[test]
    fn inline_style_with_variable() {
        // @@.highlight;You have $gold coins.@@
        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;You have $gold coins.@@",
            0,
            ParseMode::Normal,
        );
        let style = ast.nodes.iter().find_map(|n| match n {
            node @ AstNode::InlineStyle { .. } => Some(node.clone()),
            _ => None,
        }).expect("should find InlineStyle node");
        match &style {
            AstNode::InlineStyle { class, children, .. } => {
                assert_eq!(class, ".highlight");
                // Children should contain a Text node with a $gold variable ref
                let text_with_var = children.iter().any(|c| {
                    matches!(c, AstNode::Text { var_refs, .. } if !var_refs.is_empty())
                });
                assert!(text_with_var, "children should contain Text with variable refs");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn inline_style_emits_token() {
        // Verify InlineStyle semantic token emission
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body(
            "@@.highlight;important@@",
            0,
            ParseMode::Normal,
        );
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let style_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::InlineStyle))
            .collect();
        assert!(!style_tokens.is_empty(), "should emit InlineStyle token for class name");
        assert_eq!(style_tokens[0].length, ".highlight".len(),
            "InlineStyle token should cover the class name");
    }

    #[test]
    fn text_format_bold() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is ''bold'' text", 0, ParseMode::Normal,
        );
        let bold_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Bold, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Bold TextFormat node");
        assert_eq!(bold_node, "bold");
    }

    #[test]
    fn text_format_italic() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is //italic// text", 0, ParseMode::Normal,
        );
        let italic_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Italic, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Italic TextFormat node");
        assert_eq!(italic_node, "italic");
    }

    #[test]
    fn text_format_strike() {
        let ast = crate::sugarcube::parser::parse_passage_body(
            "This is ==struck== text", 0, ParseMode::Normal,
        );
        let node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Strike, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Strike TextFormat node");
        assert_eq!(node, "struck");
    }

    #[test]
    fn text_format_emits_token() {
        use crate::plugin::SemanticTokenType;
        use crate::sugarcube::lsp::token_builder::build_semantic_tokens;
        use std::collections::HashSet;

        let ast = crate::sugarcube::parser::parse_passage_body(
            "Some ''bold'' text", 0, ParseMode::Normal,
        );
        let mut tokens = Vec::new();
        build_semantic_tokens(&ast.nodes, &mut tokens, 0, &HashSet::new());

        let format_tokens: Vec<_> = tokens.iter()
            .filter(|t| matches!(t.token_type, SemanticTokenType::TextFormat))
            .collect();
        assert!(!format_tokens.is_empty(), "should emit TextFormat token for bold");
    }

    #[test]
    fn text_format_with_multibyte_utf8() {
        // Regression test: unclosed text-format delimiters followed by
        // multi-byte UTF-8 characters (e.g. em dash —) must not panic.
        // Previously the byte-by-byte scan could land inside a multi-byte
        // char, causing a panic on string slicing.
        let cases = [
            ("''bold —", TextFormatKind::Bold),
            ("//italic —", TextFormatKind::Italic),
            ("==strike —", TextFormatKind::Strike),
            ("__underline —", TextFormatKind::Underline),
            ("~~sub —", TextFormatKind::Sub),
            ("^^super —", TextFormatKind::Super),
        ];
        for (input, _expected_kind) in &cases {
            // Must not panic
            let _ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        }
    }

    #[test]
    fn text_format_closed_with_multibyte_content() {
        // Text-format markup with multi-byte characters inside should parse correctly
        let ast = crate::sugarcube::parser::parse_passage_body(
            "''bold — dash''", 0, ParseMode::Normal,
        );
        let node = ast.nodes.iter().find_map(|n| match n {
            AstNode::TextFormat { kind: TextFormatKind::Bold, content, .. } => Some(content.clone()),
            _ => None,
        }).expect("should find Bold TextFormat node");
        assert_eq!(node, "bold — dash");
    }

    #[test]
    fn prose_with_em_dash_no_panic() {
        // Plain prose with em dashes should never panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "The state — never here — is tracked.", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn macro_args_with_multibyte_utf8() {
        // Regression test: macro arguments containing multi-byte UTF-8
        // characters (e.g. em dash — in strings or comments) must not panic.
        // The scanner previously advanced by single bytes, which could land
        // inside a multi-byte char, causing a panic on string slicing.
        let cases = [
            // Em dash inside a quoted string in macro args
            r#"<<set $x = "a—b">>"#,
            // Em dash inside a block comment in macro args
            "<<set $x = 1 /* comment — with dash */ + 2>>",
            // Em dash in a line comment in macro args
            "<<set $x = 1 // comment — dash\n+ 3>>",
            // Em dash in plain args (not in string or comment)
            "<<set $x = 1>>", // no em dash but safe
            // Multiple em dashes
            "The — quick — brown — fox",
        ];
        for input in &cases {
            let _ast = crate::sugarcube::parser::parse_passage_body(input, 0, ParseMode::Normal);
        }
    }

    #[test]
    fn expression_macro_with_multibyte_utf8() {
        // <<= and <<->> with multi-byte chars in the expression
        let _ast1 = crate::sugarcube::parser::parse_passage_body(
            "<<= 'hello—world'>>", 0, ParseMode::Normal,
        );
        let _ast2 = crate::sugarcube::parser::parse_passage_body(
            "<<- 'silent—expr'>>", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn script_macro_with_multibyte_utf8_body() {
        // <<script>> body with em dashes should not panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "<<script>>\n// comment — dash\nvar x = 1;\n<</script>>",
            0, ParseMode::Normal,
        );
    }

    #[test]
    fn style_macro_with_multibyte_utf8_body() {
        // <<style>> body with em dashes should not panic
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "<<style>>\n/* style — dash */\n.foo { color: red; }\n<</style>>",
            0, ParseMode::Normal,
        );
    }

    #[test]
    fn inline_vars_with_multibyte_utf8() {
        // Variable scanning in text with multi-byte chars
        let _ast = crate::sugarcube::parser::parse_passage_body(
            "The value — $gs.x — is tracked.", 0, ParseMode::Normal,
        );
    }

    #[test]
    fn special_twee_like_content_no_panic() {
        // Simulates the content pattern from _special.twee that triggered
        // the original UTF-8 boundary panic. The file contains <<set>> macros
        // with JS object literals containing comments with em dashes, e.g.:
        //   <<set $SLOTS = {
        //     "slot": { description: "tracked — never here" },
        //   }>>
        // The em dash inside the macro args caused byte 275 to land inside
        // the 3-byte UTF-8 sequence, panicking on string slicing.
        let content = r#"/*
  REGISTRIES
  Read-only master definitions. Set once here, never mutated.
  Dynamic state for all entities lives exclusively in $gs.
  All registry values are accessed by key string.
*/
<<set $SLOTS = {
  "underwear-top":    { description: "Worn — against the skin" },
  "underwear-bottom": { description: "Lower — body coverage" },
  "legwear":          { description: "Stockings — tights, socks" },
  "bottom":           { description: "Skirt — trousers, shorts" },
}>>
<<set $ITEMS = {
  "blazer": { label: "Blazer", type: "top", description: "A tailored blazer — sharp and professional." },
  "skirt":  { label: "Skirt",  type: "bottom", description: "A pleated skirt — elegant." },
}>>
/* The inventory — never stored directly in items */
<<set $NPCS = {
  "mai": { name: "Mai", description: "Your coworker — friendly and observant" },
}>>
Some narrative text with — em dashes — and $gs.inventory references."#;
        let _ast = crate::sugarcube::parser::parse_passage_body(content, 0, ParseMode::Normal);
        // If we get here without panicking, the fix works.
    }

    #[test]
    fn block_comment_inside_set_args_emits_comment_token() {
        // Comments inside <<set>> JS expressions (e.g. inside object literals)
        // should be recognized as Comment tokens via the JS annotation pass.
        // oxc strips comments from the AST, so we scan the raw preprocessed
        // source separately in js_walk::extract_comments().
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = { /* inner comment */ a: 1 }>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for /* inner comment */ inside <<set>> args");
    }

    #[test]
    fn line_comment_inside_set_args_emits_comment_token() {
        // // line comments inside <<set>> JS expressions should also be
        // recognized as Comment tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = { a: 1, // inner line comment\n b: 2 }>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for // inner line comment inside <<set>> args, got {} comment tokens",
            comment_tokens.len());
    }

    #[test]
    fn multiline_block_comment_inside_set_args_emits_comment_token() {
        // Multi-line /* */ block comments inside <<set>> JS expressions
        // should be recognized as a single Comment token spanning ALL lines
        // including the closing */.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $x = {\n  /* this is\n     a multi-line\n     comment */\n  a: 1\n}>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let comment_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Comment))
            .collect();
        assert!(!comment_tokens.is_empty(),
            "should have at least one Comment token for multi-line /* */ inside <<set>> args, got {} comment tokens",
            comment_tokens.len());

        // The comment token should span the FULL multi-line comment including
        // the closing */ and all content lines.
        let full_comment_text: String = comment_tokens.iter()
            .map(|t| text[t.start.min(text.len())..(t.start + t.length).min(text.len())].to_string())
            .collect();
        assert!(full_comment_text.contains("*/"),
            "comment token should include the closing */, got: {:?}", full_comment_text);
        assert!(full_comment_text.contains("multi-line"),
            "comment token should include 'multi-line' from line 2, got: {:?}", full_comment_text);
        assert!(full_comment_text.contains("this is"),
            "comment token should include 'this is' from line 1, got: {:?}", full_comment_text);
    }

    #[test]
    fn set_array_of_objects_emits_literal_tokens() {
        // <<set $arr = [{a:1}, {b:2}]>> — array of objects.
        // The array handler must recurse into nested ObjectExpression
        // elements so property values get literal tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $arr = [{a:1}, {b:2}]>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let number_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Number))
            .collect();
        assert_eq!(number_tokens.len(), 2,
            "should have 2 Number tokens for 1 and 2 inside objects in array, got {}", number_tokens.len());
    }

    #[test]
    fn set_nested_array_emits_literal_tokens() {
        // <<set $arr = [[1,2], [3,4]]>> — array of arrays.
        // The array handler must recurse into nested ArrayExpression
        // elements so inner literals get tokens.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<set $arr = [[1,2], [3,4]]>>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let number_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Number))
            .collect();
        assert_eq!(number_tokens.len(), 4,
            "should have 4 Number tokens for 1,2,3,4 inside nested arrays, got {}", number_tokens.len());
    }

    #[test]
    fn prose_token_does_not_overlap_variable_tokens() {
        // Naked $variables in prose should get Variable tokens that are
        // NOT overlapped by the Prose token. The Prose token is split
        // around variable positions so each position has exactly one
        // semantic token type.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\nYou have $gold coins.\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let all_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .collect();

        let var_token = all_tokens.iter()
            .find(|t| matches!(t.token_type, SemanticTokenType::Variable))
            .expect("should have a Variable token for $gold");

        for t in &all_tokens {
            if matches!(t.token_type, SemanticTokenType::Prose) {
                let var_start = var_token.start;
                let var_end = var_token.start + var_token.length;
                let prose_start = t.start;
                let prose_end = t.start + t.length;
                let overlaps = var_start < prose_end && prose_start < var_end;
                assert!(!overlaps,
                    "Prose token [{},{}) should not overlap Variable token [{},{})",
                    prose_start, prose_end, var_start, var_end);
            }
        }
    }

    #[test]
    fn template_invocation_includes_question_mark() {
        // ?playerName in prose should get a Function token that INCLUDES
        // the ? sigil, so the whole ?playerName is visually distinct.
        use crate::plugin::{FormatPluginMut, SemanticTokenType};
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let text = ":: Start\nWelcome ?playerName to the game.\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), text);

        let func_tokens: Vec<_> = result.token_groups.iter()
            .flat_map(|g| g.tokens.iter())
            .filter(|t| matches!(t.token_type, SemanticTokenType::Function))
            .collect();
        assert!(!func_tokens.is_empty(), "should have a Function token for ?playerName");

        let tok = &func_tokens[0];
        let token_text = &text[tok.start.min(text.len())..(tok.start + tok.length).min(text.len())];
        assert!(token_text.starts_with('?'),
            "template token should include the ? sigil, got: {:?}", token_text);
    }
}
