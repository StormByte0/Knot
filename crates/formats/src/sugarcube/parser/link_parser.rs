//! Link parsing for `[[ ]]` syntax.

use crate::sugarcube::ast::*;

/// Parse a link starting after `[[`.
///
/// `i` points to the first character after `[[`.
/// On return, `i` points past the closing `]]`.
pub(super) fn parse_link(text: &str, i: &mut usize, span_start: usize) -> AstNode {
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
        *i += 1;
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
    let (display, target, kind, setter_var) = parse_link_content(content);

    AstNode::Link {
        display,
        target,
        kind,
        setter_var,
        span: span_start..span_start + *i,
    }
}

/// Parse the content between `[[` and `]]` into display/target.
pub(super) fn parse_link_content(content: &str) -> (Option<String>, String, LinkKind, Option<String>) {
    let trimmed = content.trim();

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
        return (display, target, kind, setter_var);
    }

    // Check for pipe syntax: [[display|target]]
    if let Some(pipe_pos) = trimmed.rfind('|') {
        let display = trimmed[..pipe_pos].to_string();
        let target = trimmed[pipe_pos + 1..].trim().to_string();
        return (Some(display), target, LinkKind::Pipe, None);
    }

    // Check for right arrow syntax: [[display->target]]
    if let Some(arrow_pos) = trimmed.rfind("->") {
        let display = trimmed[..arrow_pos].to_string();
        let target = trimmed[arrow_pos + 2..].trim().to_string();
        return (Some(display), target, LinkKind::ArrowRight, None);
    }

    // Check for left arrow syntax: [[target<-display]]
    if let Some(arrow_pos) = trimmed.rfind("<-") {
        let target = trimmed[..arrow_pos].trim().to_string();
        let display = trimmed[arrow_pos + 2..].to_string();
        return (Some(display), target, LinkKind::ArrowLeft, None);
    }

    // Simple link: [[target]]
    (None, trimmed.to_string(), LinkKind::Simple, None)
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
}
