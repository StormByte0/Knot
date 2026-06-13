//! Link parsing for `[[ ]]` syntax.

use crate::sugarcube::ast::*;

/// Parse a link starting after `[[`.
///
/// `i` points to the first character after `[[`.
/// On return, `i` points past the closing `]]`.
///
/// `offset` is the base byte offset for the body text being parsed
/// (0 for top-level, nonzero for nested block content).
/// `link_start` is the position of `[[` in `text`.
pub(super) fn parse_link(text: &str, i: &mut usize, offset: usize, link_start: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let content_start = *i;

    // Scan to matching ]], handling nested [[ (rare but valid in edge cases)
    let mut depth = 1u32;
    while *i < len {
        if bytes[*i] == b'[' && *i + 1 < len && bytes[*i + 1] == b'[' {
            depth += 1;
            *i += 2;
            continue;
        }
        if bytes[*i] == b']' && *i + 1 < len && bytes[*i + 1] == b']' {
            depth -= 1;
            if depth == 0 {
                break;
            }
            *i += 2;
            continue;
        }
        // Advance by full UTF-8 character to avoid landing inside
        // a multi-byte sequence (e.g. em dash —), which would cause
        // a panic when slicing `text` later.
        *i += text[*i..].chars().next().map_or(1, |c| c.len_utf8());
    }

    let content_end = *i;
    let content = if content_start < content_end {
        &text[content_start..content_end]
    } else {
        ""
    };

    // Advance past ]]
    if *i + 1 < len {
        *i += 2; // Skip ]]
    } else {
        *i = len;
    }

    // Parse the link content
    let (display, target, kind, setter_var, image_url) = parse_link_content(content);

    AstNode::Link {
        display,
        target,
        kind,
        setter_var,
        image_url,
        span: offset + link_start..offset + *i,
    }
}

/// Parse the content between `[[` and `]]` into display/target.
pub(super) fn parse_link_content(content: &str) -> (Option<String>, String, LinkKind, Option<String>, Option<String>) {
    let trimmed = content.trim();

    // Check for image link syntax: [[img[URL][Passage]] or [[img[URL][Display|Passage]]
    // This must be checked BEFORE the setter ][ detection, because image links
    // also contain ][ but it separates the image URL from the passage target.
    if let Some(rest) = trimmed.strip_prefix("img[") {
        // Find the closing ] of the image URL
        if let Some(url_end) = rest.find(']') {
            let image_url = rest[..url_end].to_string();
            // After the URL's ], skip any ] and [ to reach the passage target.
            // The content structure is: img[url][passage  (outer [[ ]] stripped)
            // So after url_end we have: ][passage
            let remaining = &rest[url_end..];
            let remaining = remaining.trim_start_matches(']').trim_start_matches('[');
            if !remaining.is_empty() {
                let (display, target, _kind) = parse_link_display_target(remaining);
                return (display, target, LinkKind::Image, None, Some(image_url));
            }
        }
    }

    // Check for setter syntax: [[target][$var to value]]
    if let Some(bracket_pos) = trimmed.rfind("][") {
        let before_bracket = &trimmed[..bracket_pos];
        let after_bracket = &trimmed[bracket_pos + 2..];
        // Strip trailing ] from before_bracket and leading [ from after_bracket
        let inner_before = before_bracket.trim_end_matches(']');
        let inner_after = after_bracket.trim_start_matches('[');

        let (display, target, kind) = parse_link_display_target(inner_before);
        let setter_var = if inner_after.starts_with('$') || inner_after.starts_with('_') {
            Some(inner_after.split_whitespace().next().unwrap_or(inner_after).to_string())
        } else {
            None
        };
        return (display, target, kind, setter_var, None);
    }

    // Check for pipe syntax: [[display|target]]
    if let Some(pipe_pos) = trimmed.rfind('|') {
        let display = trimmed[..pipe_pos].to_string();
        let target = trimmed[pipe_pos + 1..].trim().to_string();
        return (Some(display), target, LinkKind::Pipe, None, None);
    }

    // Check for right arrow syntax: [[display->target]]
    if let Some(arrow_pos) = trimmed.rfind("->") {
        let display = trimmed[..arrow_pos].to_string();
        let target = trimmed[arrow_pos + 2..].trim().to_string();
        return (Some(display), target, LinkKind::ArrowRight, None, None);
    }

    // Check for left arrow syntax: [[target<-display]]
    if let Some(arrow_pos) = trimmed.rfind("<-") {
        let target = trimmed[..arrow_pos].trim().to_string();
        let display = trimmed[arrow_pos + 2..].to_string();
        return (Some(display), target, LinkKind::ArrowLeft, None, None);
    }

    // Simple link: [[target]]
    (None, trimmed.to_string(), LinkKind::Simple, None, None)
}

pub(super) fn parse_link_display_target(content: &str) -> (Option<String>, String, LinkKind) {
    let trimmed = content.trim();

    if let Some(pipe_pos) = trimmed.rfind('|') {
        let display = trimmed[..pipe_pos].to_string();
        let target = trimmed[pipe_pos + 1..].trim().to_string();
        return (Some(display), target, LinkKind::Pipe);
    }

    if let Some(arrow_pos) = trimmed.rfind("->") {
        let display = trimmed[..arrow_pos].to_string();
        let target = trimmed[arrow_pos + 2..].trim().to_string();
        return (Some(display), target, LinkKind::ArrowRight);
    }

    if let Some(arrow_pos) = trimmed.rfind("<-") {
        let target = trimmed[..arrow_pos].trim().to_string();
        let display = trimmed[arrow_pos + 2..].to_string();
        return (Some(display), target, LinkKind::ArrowLeft);
    }

    (None, trimmed.to_string(), LinkKind::Simple)
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
    fn parse_simple_link() {
        let ast = parse_passage_body("Go [[Forest]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "Forest");
    }

    #[test]
    fn parse_pipe_link() {
        let ast = parse_passage_body("Go [[dark forest|Forest]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "Forest");
        assert_eq!(ast.links[0].display.as_deref(), Some("dark forest"));
    }

    #[test]
    fn parse_arrow_link() {
        let ast = parse_passage_body("Go [[dark forest->Forest]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "Forest");
    }

    #[test]
    fn parse_left_arrow_link() {
        let ast = parse_passage_body("Go [[Forest<-dark forest]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "Forest");
    }

    #[test]
    fn setter_link() {
        let ast = parse_passage_body("[[target][$var to 5]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "target");
    }

    #[test]
    fn link_macro_single_arg() {
        // <<link "Forest">> — single arg is both display and target
        let ast = parse_passage_body(r#"<<link "Forest">>"#, 0, ParseMode::Normal);
        let nav_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::NavigationMacro).collect();
        assert_eq!(nav_links.len(), 1);
        assert_eq!(nav_links[0].display.as_deref(), Some("Forest"));
        assert_eq!(nav_links[0].target, "Forest");
    }

    #[test]
    fn link_macro_two_args() {
        let ast = parse_passage_body(r#"<<link "Go to forest" "Forest">>"#, 0, ParseMode::Normal);
        let nav_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::NavigationMacro).collect();
        assert_eq!(nav_links.len(), 1);
        assert_eq!(nav_links[0].display.as_deref(), Some("Go to forest"));
        assert_eq!(nav_links[0].target, "Forest");
    }

    #[test]
    fn image_link_simple() {
        // [[img[http://example.com/pic.jpg][Forest]]
        let ast = parse_passage_body("[[img[http://example.com/pic.jpg][Forest]]", 0, ParseMode::Normal);
        // Find the link node in the AST
        let link_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Link { kind, .. } if *kind == LinkKind::Image => Some(n.clone()),
            _ => None,
        }).expect("should find an Image link");
        match &link_node {
            AstNode::Link { target, image_url, kind, .. } => {
                assert_eq!(*kind, LinkKind::Image);
                assert_eq!(target, "Forest", "image link target should be the passage name");
                assert_eq!(image_url.as_deref(), Some("http://example.com/pic.jpg"),
                    "image_url should contain the URL");
            }
            _ => panic!("Expected Link node"),
        }
    }

    #[test]
    fn image_link_with_display() {
        // [[img[http://example.com/pic.jpg][Dark Forest|Forest]]
        let ast = parse_passage_body("[[img[http://example.com/pic.jpg][Dark Forest|Forest]]", 0, ParseMode::Normal);
        let link_node = ast.nodes.iter().find_map(|n| match n {
            AstNode::Link { kind, .. } if *kind == LinkKind::Image => Some(n.clone()),
            _ => None,
        }).expect("should find an Image link");
        match &link_node {
            AstNode::Link { display, target, image_url, kind, .. } => {
                assert_eq!(*kind, LinkKind::Image);
                assert_eq!(target, "Forest");
                assert_eq!(display.as_deref(), Some("Dark Forest"));
                assert_eq!(image_url.as_deref(), Some("http://example.com/pic.jpg"));
            }
            _ => panic!("Expected Link node"),
        }
    }

    #[test]
    fn link_with_multibyte_utf8_no_panic() {
        // Regression test: links containing multi-byte UTF-8 characters
        // (e.g. em dash —) must not panic. Previously the byte-by-byte
        // scan in the link parser could land inside a multi-byte char,
        // causing a panic on string slicing.
        let _ast = parse_passage_body("Go [[a—b]] there", 0, ParseMode::Normal);
    }

    #[test]
    fn link_with_multibyte_utf8_content_correct() {
        // Link with em dash should parse correctly, not just not-panic
        let ast = parse_passage_body("Go [[a—b]] there", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1, "should find a link");
        // The target should include the em dash
        assert!(ast.links[0].target.contains("—"), "target should contain em dash, got: {:?}", ast.links[0].target);
    }
}
