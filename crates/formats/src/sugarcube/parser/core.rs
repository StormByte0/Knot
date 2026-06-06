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
                // // — JS single-line comment (to end of line)
                // Only treat as a comment if not inside a macro or link
                // (those contexts handle their own content). In normal
                // passage prose, // is ambiguous — but in Twine projects
                // that mix JS/CSS into passages, this is commonly used.
                // We only recognize // comments when followed by a space
                // or at line context to avoid false positives on URLs
                // like http://example.com.
                let is_comment_context = if i + 2 < len {
                    // // at start of line, or preceded by whitespace
                    let at_line_start = i == 0 || bytes[i - 1] == b'\n';
                    let preceded_by_space = i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t');
                    // // followed by space is a strong signal
                    let followed_by_space = bytes[i + 2] == b' ' || bytes[i + 2] == b'\t';
                    at_line_start || (preceded_by_space && followed_by_space)
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
                    i += 1;
                    None
                }
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
            _ => {
                i += 1;
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
    });
    *text_start = end;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::sugarcube::ast::{AstNode, ParseMode, LinkSource};

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
        assert!(connections.iter().any(|c| c.target == "Cave" && c.edge_type == knot_core::graph::EdgeType::Jump));
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
}
