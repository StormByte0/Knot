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

    // Parse the link content. Sub-spans returned by `parse_link_content`
    // are relative to the start of `content` (= `content_start` in `text`).
    // We add `offset + link_start + 2` to convert them to body-relative
    // offsets (the same coordinate space as `span`):
    //   - `offset + link_start` = body-relative position of `[[`
    //   - `+ 2` skips past `[[` to the start of `content`
    let content_offset = offset + link_start + 2;
    let (display, target, kind, setter_var, image_url, display_span, target_span, setter_span) =
        parse_link_content(content);

    let shift = |r: std::ops::Range<usize>| -> std::ops::Range<usize> {
        (content_offset + r.start)..(content_offset + r.end)
    };

    AstNode::Link {
        display,
        target,
        kind,
        setter_var,
        image_url,
        span: offset + link_start..offset + *i,
        display_span: display_span.map(&shift),
        target_span: shift(target_span),
        setter_span: setter_span.map(&shift),
    }
}

/// Parsed link content with sub-spans.
///
/// All spans are relative to the start of `content` (the text between `[[`
/// and `]]`, with no brackets). The caller is responsible for shifting them
/// to body-relative offsets.
type ParsedLink = (
    Option<String>,            // display
    String,                    // target
    LinkKind,                  // kind
    Option<String>,            // setter_var
    Option<String>,            // image_url
    Option<std::ops::Range<usize>>, // display_span (relative to content)
    std::ops::Range<usize>,    // target_span (relative to content)
    Option<std::ops::Range<usize>>, // setter_span (relative to content)
);

/// Parse the content between `[[` and `]]` into display/target + sub-spans.
///
/// All returned spans are relative to the start of `content`. The caller
/// shifts them by `link_start + 2` (the body-relative offset of `content[0]`)
/// to produce body-relative spans.
pub(super) fn parse_link_content(content: &str) -> ParsedLink {
    // Compute the byte offset of `trimmed` within `content` — needed so the
    // returned spans are relative to `content`, not `trimmed`.
    let leading_ws = content.len() - content.trim_start().len();
    let trimmed = &content[leading_ws..];

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
            let after_url = &rest[url_end..];
            // `remaining_offset` is the byte offset (within `content`) where
            // `remaining` (the post-image part) starts.
            let remaining_offset = leading_ws + "img[".len() + url_end;
            // Skip leading ] and [ characters
            let skip = after_url.bytes().take_while(|&b| b == b']' || b == b'[').count();
            let remaining = &after_url[skip..];
            if !remaining.is_empty() {
                let (display, target, _kind, display_sub, target_sub) =
                    parse_link_display_target_with_spans(remaining);
                let abs_target_span = (remaining_offset + skip + target_sub.start)
                    ..(remaining_offset + skip + target_sub.end);
                let abs_display_span = display_sub.map(|d| {
                    (remaining_offset + skip + d.start)..(remaining_offset + skip + d.end)
                });
                return (
                    display,
                    target,
                    LinkKind::Image,
                    None,
                    Some(image_url),
                    abs_display_span,
                    abs_target_span,
                    None,
                );
            }
        }
    }

    // Check for setter syntax: [[target][$var to value]]
    if let Some(bracket_pos) = trimmed.rfind("][") {
        let before_bracket = &trimmed[..bracket_pos];
        let after_bracket = &trimmed[bracket_pos + 2..];
        // Strip trailing ] from before_bracket and leading [ from after_bracket
        let trailing_strip = before_bracket.len()
            - before_bracket.trim_end_matches(']').len();
        let leading_strip = after_bracket.len()
            - after_bracket.trim_start_matches('[').len();
        let inner_before = &before_bracket[..before_bracket.len() - trailing_strip];
        let inner_after = &after_bracket[leading_strip..];

        let (display, target, kind, display_sub, target_sub) =
            parse_link_display_target_with_spans(inner_before);
        // Shift display/target sub-spans by `leading_ws` (offset of `trimmed`
        // within `content`).
        let abs_target_span = (leading_ws + target_sub.start)..(leading_ws + target_sub.end);
        let abs_display_span = display_sub
            .map(|d| (leading_ws + d.start)..(leading_ws + d.end));
        // Setter span: from the start of `inner_after` to the end of `content`
        // (within `trimmed`). `inner_after` starts at offset `bracket_pos + 2 +
        // leading_strip` within `trimmed`, so within `content` it's `leading_ws
        // + bracket_pos + 2 + leading_strip`.
        let setter_start_in_content = leading_ws + bracket_pos + 2 + leading_strip;
        // `inner_after` may have trailing whitespace before `]]` — keep the
        // setter span tight to the actual expression by trimming the end.
        let setter_end_in_content = leading_ws + bracket_pos + 2 + leading_strip
            + inner_after.trim_end().len();
        let setter_span = if setter_end_in_content > setter_start_in_content {
            Some(setter_start_in_content..setter_end_in_content)
        } else {
            None
        };
        let setter_var = if inner_after.starts_with('$') || inner_after.starts_with('_') {
            Some(inner_after.split_whitespace().next().unwrap_or(inner_after).to_string())
        } else {
            None
        };
        return (
            display,
            target,
            kind,
            setter_var,
            None,
            abs_display_span,
            abs_target_span,
            setter_span,
        );
    }

    // Check for pipe syntax: [[display|target]]
    if let Some(pipe_pos) = trimmed.rfind('|') {
        let display_str = trimmed[..pipe_pos].to_string();
        let target_str = trimmed[pipe_pos + 1..].trim().to_string();
        let display_start = leading_ws;
        let display_end = leading_ws + pipe_pos;
        // Target starts after `|`, then skip leading whitespace.
        let target_pre_trim = pipe_pos + 1;
        let target_ws = trimmed[target_pre_trim..].len()
            - trimmed[target_pre_trim..].trim_start().len();
        let target_start = leading_ws + target_pre_trim + target_ws;
        let target_end = target_start + target_str.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::Pipe,
            None,
            None,
            Some(display_start..display_end),
            target_start..target_end,
            None,
        );
    }

    // Check for right arrow syntax: [[display->target]]
    if let Some(arrow_pos) = trimmed.rfind("->") {
        let display_str = trimmed[..arrow_pos].to_string();
        let target_str = trimmed[arrow_pos + 2..].trim().to_string();
        let display_start = leading_ws;
        let display_end = leading_ws + arrow_pos;
        let target_pre_trim = arrow_pos + 2;
        let target_ws = trimmed[target_pre_trim..].len()
            - trimmed[target_pre_trim..].trim_start().len();
        let target_start = leading_ws + target_pre_trim + target_ws;
        let target_end = target_start + target_str.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::ArrowRight,
            None,
            None,
            Some(display_start..display_end),
            target_start..target_end,
            None,
        );
    }

    // Check for left arrow syntax: [[target<-display]]
    if let Some(arrow_pos) = trimmed.rfind("<-") {
        let target_str = trimmed[..arrow_pos].trim().to_string();
        let display_str = trimmed[arrow_pos + 2..].to_string();
        // Target = trimmed[..arrow_pos], trimmed on the right.
        let target_pre_trim = 0;
        let target_ws_right = trimmed[..arrow_pos].len()
            - trimmed[..arrow_pos].trim_end().len();
        let target_start = leading_ws + target_pre_trim;
        let target_end = leading_ws + arrow_pos - target_ws_right;
        let display_start = leading_ws + arrow_pos + 2;
        let display_end = leading_ws + trimmed.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::ArrowLeft,
            None,
            None,
            Some(display_start..display_end),
            target_start..target_end,
            None,
        );
    }

    // Simple link: [[target]]
    let target_str = trimmed.to_string();
    let target_start = leading_ws;
    let target_end = leading_ws + target_str.len();
    (
        None,
        target_str,
        LinkKind::Simple,
        None,
        None,
        None,
        target_start..target_end,
        None,
    )
}

/// Parse display/target from a content slice, returning sub-spans relative
/// to the start of `content`.
fn parse_link_display_target_with_spans(
    content: &str,
) -> (Option<String>, String, LinkKind, Option<std::ops::Range<usize>>, std::ops::Range<usize>) {
    let leading_ws = content.len() - content.trim_start().len();
    let trimmed = &content[leading_ws..];

    if let Some(pipe_pos) = trimmed.rfind('|') {
        let display_str = trimmed[..pipe_pos].to_string();
        let target_str = trimmed[pipe_pos + 1..].trim().to_string();
        let display_start = leading_ws;
        let display_end = leading_ws + pipe_pos;
        let target_pre_trim = pipe_pos + 1;
        let target_ws = trimmed[target_pre_trim..].len()
            - trimmed[target_pre_trim..].trim_start().len();
        let target_start = leading_ws + target_pre_trim + target_ws;
        let target_end = target_start + target_str.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::Pipe,
            Some(display_start..display_end),
            target_start..target_end,
        );
    }

    if let Some(arrow_pos) = trimmed.rfind("->") {
        let display_str = trimmed[..arrow_pos].to_string();
        let target_str = trimmed[arrow_pos + 2..].trim().to_string();
        let display_start = leading_ws;
        let display_end = leading_ws + arrow_pos;
        let target_pre_trim = arrow_pos + 2;
        let target_ws = trimmed[target_pre_trim..].len()
            - trimmed[target_pre_trim..].trim_start().len();
        let target_start = leading_ws + target_pre_trim + target_ws;
        let target_end = target_start + target_str.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::ArrowRight,
            Some(display_start..display_end),
            target_start..target_end,
        );
    }

    if let Some(arrow_pos) = trimmed.rfind("<-") {
        let target_str = trimmed[..arrow_pos].trim().to_string();
        let display_str = trimmed[arrow_pos + 2..].to_string();
        let target_ws_right = trimmed[..arrow_pos].len()
            - trimmed[..arrow_pos].trim_end().len();
        let target_start = leading_ws;
        let target_end = leading_ws + arrow_pos - target_ws_right;
        let display_start = leading_ws + arrow_pos + 2;
        let display_end = leading_ws + trimmed.len();
        return (
            Some(display_str),
            target_str,
            LinkKind::ArrowLeft,
            Some(display_start..display_end),
            target_start..target_end,
        );
    }

    // Simple target (no display)
    let target_str = trimmed.to_string();
    let target_start = leading_ws;
    let target_end = leading_ws + target_str.len();
    (
        None,
        target_str,
        LinkKind::Simple,
        None,
        target_start..target_end,
    )
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
        // <<link "Forest">> — single arg is display text only (click handler).
        // SugarCube: `<<link "Display">>` with no second arg is a click
        // handler whose behavior is defined by the macro body. It does NOT
        // navigate to a passage named "Forest". Only `<<link "Display"
        // "Passage">>` (two args) navigates.
        //
        // Previous bug: the single arg was treated as BOTH display and
        // target, producing false "BrokenLink: Link target 'Forest' not
        // found" diagnostics for click-handler usage.
        let ast = parse_passage_body(r#"<<link "Forest">>"#, 0, ParseMode::Normal);
        let nav_links: Vec<_> = ast.links.iter().filter(|l| l.source == LinkSource::NavigationMacro).collect();
        assert_eq!(nav_links.len(), 1);
        assert_eq!(nav_links[0].display.as_deref(), Some("Forest"));
        assert_eq!(nav_links[0].target, ""); // no fixed target — click handler
        assert!(nav_links[0].is_dynamic); // dynamic — no BrokenLink diagnostic
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
