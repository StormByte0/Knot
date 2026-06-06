//! SugarCube recursive descent parser.
//!
//! This is the heart of the rewrite. A single parser replaces ~2500 lines of
//! regex code (vars/, links/, validation/, macro_scan/, workspace/, comments/,
//! passage_tree/). The parser handles SugarCube's delimiter-based syntax
//! natively, tracking nesting depth, string contexts, and the `>>` vs `>>>`
//! ambiguity.
//!
//! ## Delimiters
//!
//! | Sequence | Token |
//! |----------|-------|
//! | `<<`     | Macro open |
//! | `>>`     | Macro close (if inside `<<`) |
//! | `[[`     | Link open |
//! | `]]`     | Link close (if inside `[[`) |
//! | `$id`    | Story variable |
//! | `_id`    | Temporary variable (word boundary) |
//! | `/%`     | Twine block comment open |
//! | `%/`     | Twine block comment close |
//! | `/%%`    | SugarCube block comment open |
//! | `%%/`    | SugarCube block comment close |
//! | `<!--`   | HTML comment open |
//! | `-->`    | HTML comment close |
//! | `$$`     | Escaped dollar (not a variable) |
//!
//! ## Algorithm
//!
//! The parser scans left-to-right, recognizing delimiters by their leading
//! character. When a delimiter is found, it dispatches to a specialized
//! handler that scans to the matching close delimiter, handling nesting
//! and string escaping along the way.

use super::ast::*;
use super::macros;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a SugarCube passage body into an AST.
///
/// `body` is the raw text between the header line and the next passage header.
/// `body_offset` is the byte offset where `body` starts in the document.
/// This is used to compute document-absolute spans.
///
/// Returns a `PassageAst` with the node list, extracted links, and
/// variable operations.
pub fn parse_passage_body(body: &str, body_offset: usize, mode: ParseMode) -> PassageAst {
    match mode {
        ParseMode::Normal | ParseMode::Widget | ParseMode::Interface => {
            let nodes = parse_body(body, 0);
            let links = extract_links_from_ast(&nodes);
            let var_ops = extract_var_ops_from_ast(&nodes);
            PassageAst {
                nodes,
                links,
                var_ops,
                mode,
            }
        }
        ParseMode::Script | ParseMode::Stylesheet | ParseMode::Minimal => {
            PassageAst::empty(mode)
        }
    }
}

// ---------------------------------------------------------------------------
// Core parser
// ---------------------------------------------------------------------------

/// Parse body text into AST nodes.
///
/// `offset` is the byte offset within the body where this segment starts
/// (0 for the top level, nonzero for nested content inside block macros).
fn parse_body(text: &str, offset: usize) -> Vec<AstNode> {
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
                let node = parse_macro(text, &mut i, offset + start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'[' if i + 1 < len && bytes[i + 1] == b'[' => {
                // [[ — link
                let start = i;
                i += 2;
                let node = parse_link(text, &mut i, offset + start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'%' => {
                // /% or /%% — comment
                let start = i;
                let is_sugarcube = i + 2 < len && bytes[i + 2] == b'%';
                let delim_len = if is_sugarcube { 3 } else { 2 };
                i += delim_len;
                let node = parse_block_comment(text, &mut i, offset + start, is_sugarcube);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'<' if i + 3 < len && &text[i..i + 4] == "<!--" => {
                // <!-- — HTML comment
                let start = i;
                i += 4;
                let node = parse_html_comment(text, &mut i, offset + start);
                flush_text(text, &mut text_start, start, offset, &mut nodes);
                Some(node)
            }
            b'$' if i + 1 < len && bytes[i + 1] == b'$' => {
                // $$ — escaped dollar, include in text
                i += 2;
                None
            }
            b'$' if i + 1 < len && is_ident_start(bytes[i + 1]) => {
                // $var — story variable in text
                let start = i;
                let (var_ref, end) = scan_variable(text, i, false);
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
// Macro parser
// ---------------------------------------------------------------------------

/// Parse a macro starting after `<<`.
///
/// `i` points to the first character after `<<`.
/// On return, `i` points past the closing `>>` (or end of text).
fn parse_macro(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Skip whitespace after <<
    while *i < len && bytes[*i] == b' ' {
        *i += 1;
    }

    // Check for close tag: <</name>>
    if *i < len && bytes[*i] == b'/' {
        *i += 1;
        // Scan the close tag name
        let name_start = *i;
        while *i < len && is_ident_char(bytes[*i]) {
            *i += 1;
        }
        let name = text[name_start..*i].to_string();
        // Skip to >>
        skip_to_macro_close(text, i);
        return AstNode::Macro {
            name,
            args: String::new(),
            var_refs: Vec::new(),
            children: None, // close tags have no children
            name_span: span_start + 2..span_start + 2 + (*i - name_start),
            open_span: span_start..span_start + *i,
            close_span: None,
            full_span: span_start..span_start + *i,
        };
    }

    // Scan the macro name
    let name_start = *i;
    // Expression macros: <<=>> and <<->>
    if *i < len && (bytes[*i] == b'=' || bytes[*i] == b'-') {
        let kind = if bytes[*i] == b'=' { ExprKind::Print } else { ExprKind::Silent };
        *i += 1;
        // Skip to >>
        let content_start = *i;
        skip_to_macro_close(text, i);
        let content = text[content_start..*i].to_string();
        let var_refs = scan_inline_vars(&content, span_start + content_start);
        return AstNode::Expression {
            kind,
            content,
            var_refs,
            span: span_start..span_start + *i,
        };
    }

    // Regular macro name
    while *i < len && is_ident_char(bytes[*i]) {
        *i += 1;
    }
    let name = text[name_start..*i].to_string();
    let name_len = name.len();

    // Skip space between name and args
    while *i < len && bytes[*i] == b' ' {
        *i += 1;
    }

    // Scan args to matching >>
    let args_start = *i;
    let args_end = scan_macro_args(text, i);
    let args = text[args_start..args_end].to_string();

    let open_end = *i; // past the >>

    // Determine if this is a block macro (needs a close tag)
    let is_block = is_block_macro(&name);

    let var_refs = scan_inline_vars(&args, span_start + args_start);

    if is_block {
        // Parse the body until <</name>>
        let body_text = &text[open_end..];
        let (children, close_offset) = parse_block_body(body_text, &name, span_start + open_end);

        let close_span = if let Some(co) = close_offset {
            // The close tag was found at body_text[co..]
            // Scan the close tag to find its full extent
            let mut ci = co;
            // Skip past <</name>>
            while ci < body_text.len() && body_text.as_bytes()[ci] != b'>' {
                ci += 1;
            }
            if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                ci += 1;
                // Check for >>
                if ci < body_text.len() && body_text.as_bytes()[ci] == b'>' {
                    ci += 1;
                }
            }
            *i = open_end + ci;
            Some(span_start + open_end + co..span_start + open_end + ci)
        } else {
            // Unclosed block macro — the rest of the text is the body
            *i = len;
            None
        };

        let full_end = close_span.as_ref().map_or(span_start + *i, |s| s.end);

        AstNode::Macro {
            name,
            args,
            var_refs,
            children: Some(children),
            name_span: span_start + 2..span_start + 2 + name_len,
            open_span: span_start..span_start + open_end,
            close_span,
            full_span: span_start..full_end.max(span_start + *i),
        }
    } else {
        AstNode::Macro {
            name,
            args,
            var_refs,
            children: None,
            name_span: span_start + 2..span_start + 2 + name_len,
            open_span: span_start..span_start + open_end,
            close_span: None,
            full_span: span_start..span_start + open_end,
        }
    }
}

/// Scan macro arguments, handling nested `<<`/`>>` and strings.
///
/// Returns the byte position where args end (before `>>`).
/// Advances `i` past the closing `>>`.
fn scan_macro_args(text: &str, i: &mut usize) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let start = *i;
    let mut depth = 1u32; // We're inside one <<
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while *i < len {
        let b = bytes[*i];

        // String tracking
        if b == b'\\' && *i + 1 < len {
            *i += 2; // Skip escaped char
            continue;
        }
        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            *i += 1;
            continue;
        }
        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            *i += 1;
            continue;
        }
        if in_single_quote || in_double_quote {
            *i += 1;
            continue;
        }

        // Nested <<
        if b == b'<' && *i + 1 < len && bytes[*i + 1] == b'<' {
            depth += 1;
            *i += 2;
            continue;
        }

        // >> or >>>
        if b == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            depth -= 1;
            if depth == 0 {
                let args_end = *i;
                *i += 2; // Skip >>

                // SugarCube treats >>> as >> + >. If the next char is >,
                // it's part of the next token, not part of this close.
                // But >>> is actually a single sugar syntax for >>
                // followed by a literal >. So we just consume the two >.
                return args_end;
            }
            *i += 2;
            continue;
        }

        *i += 1;
    }

    // Unclosed macro — everything is args
    *i = len;
    len
}

/// Skip to the closing `>>` of a macro (for close tags and simple cases).
fn skip_to_macro_close(text: &str, i: &mut usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    while *i < len {
        if bytes[*i] == b'>' && *i + 1 < len && bytes[*i + 1] == b'>' {
            *i += 2;
            return;
        }
        *i += 1;
    }
}

/// Parse the body of a block macro until `<</name>>`.
///
/// Returns (children_nodes, close_tag_offset).
/// `close_tag_offset` is the position of `<</name>>` in `text`,
/// or None if the block is unclosed.
fn parse_block_body(text: &str, macro_name: &str, offset: usize) -> (Vec<AstNode>, Option<usize>) {
    // Find the matching <</name>>
    let close_tag = format!("<</{}>>", macro_name);
    let close_tag_alt = format!("<</ {}>>", macro_name); // with space after /

    let mut search_from = 0usize;
    let mut depth = 1u32;
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Scan for the matching close tag, tracking nesting depth
    while search_from < len {
        // Check for nested opening tag of the same macro
        let open_tag = format!("<<{}", macro_name);
        if search_from + open_tag.len() <= len
            && text[search_from..].starts_with(&open_tag)
            && search_from + open_tag.len() < len
            && is_ident_char(bytes[search_from + open_tag.len()])
        {
            // Check it's actually the opening tag (not <</name>>)
            // The character after the name must be a space or >
            let after_name = search_from + open_tag.len();
            if after_name < len && (bytes[after_name] == b' ' || bytes[after_name] == b'>') {
                depth += 1;
                search_from += open_tag.len();
                continue;
            }
        }

        // Check for close tag
        if text[search_from..].starts_with(&close_tag)
            || text[search_from..].starts_with(&close_tag_alt)
        {
            depth -= 1;
            if depth == 0 {
                // Found the matching close tag
                let body_content = &text[..search_from];
                let children = parse_body(body_content, offset);
                return (children, Some(search_from));
            }
            search_from += close_tag.len();
            continue;
        }

        search_from += 1;
    }

    // Unclosed — parse the rest as body
    let children = parse_body(text, offset);
    (children, None)
}

// ---------------------------------------------------------------------------
// Link parser
// ---------------------------------------------------------------------------

/// Parse a link starting after `[[`.
///
/// `i` points to the first character after `[[`.
/// On return, `i` points past the closing `]]`.
fn parse_link(text: &str, i: &mut usize, span_start: usize) -> AstNode {
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

    let is_dynamic = target.starts_with('$') || target.starts_with('_');

    AstNode::Link {
        display,
        target,
        kind,
        setter_var,
        span: span_start..span_start + *i,
    }
}

/// Parse the content between `[[` and `]]` into display/target.
fn parse_link_content(content: &str) -> (Option<String>, String, LinkKind, Option<String>) {
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

fn parse_link_display_target(content: &str) -> (Option<String>, String, LinkKind) {
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
// Comment parsers
// ---------------------------------------------------------------------------

/// Parse a block comment (/% ... %/ or /%% ... %%/).
fn parse_block_comment(text: &str, i: &mut usize, span_start: usize, is_sugarcube: bool) -> AstNode {
    let (close_delim, kind) = if is_sugarcube {
        ("%%/", CommentKind::SugarCube)
    } else {
        ("%/", CommentKind::Twine)
    };

    let content_start = *i;
    if let Some(pos) = text[*i..].find(close_delim) {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + close_delim.len();
        AstNode::Comment {
            content,
            kind,
            span: span_start..span_start + *i,
        }
    } else {
        // Unclosed comment
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind,
            span: span_start..span_start + text.len(),
        }
    }
}

/// Parse an HTML comment (<!-- ... -->).
fn parse_html_comment(text: &str, i: &mut usize, span_start: usize) -> AstNode {
    let content_start = *i;
    if let Some(pos) = text[*i..].find("-->") {
        let content_end = *i + pos;
        let content = text[content_start..content_end].to_string();
        *i = content_end + 3; // Skip -->
        AstNode::Comment {
            content,
            kind: CommentKind::Html,
            span: span_start..span_start + *i,
        }
    } else {
        let content = text[content_start..].to_string();
        *i = text.len();
        AstNode::Comment {
            content,
            kind: CommentKind::Html,
            span: span_start..span_start + text.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Variable scanning
// ---------------------------------------------------------------------------

/// Scan a variable reference starting at position `start`.
///
/// Returns (var_ref, end_position).
/// `start` points to the `$` or `_` sigil.
fn scan_variable(text: &str, start: usize, is_temporary: bool) -> (VarRef, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let sigil = bytes[start];

    // Scan identifier
    let mut i = start + 1;
    while i < len && is_ident_char(bytes[i]) {
        i += 1;
    }

    // Scan dot-notation property path
    let mut path_start = i;
    let mut property_path = String::new();
    while i < len && bytes[i] == b'.' {
        i += 1; // Skip the dot
        let prop_start = i;
        while i < len && is_ident_char(bytes[i]) {
            i += 1;
        }
        if i > prop_start {
            if !property_path.is_empty() {
                property_path.push('.');
            }
            property_path.push_str(&text[prop_start..i]);
        }
    }

    let name = text[start..path_start].to_string();

    (
        VarRef {
            name,
            property_path,
            is_temporary,
            is_write: false, // Write status determined by context
            span: start..i,
        },
        i,
    )
}

/// Scan inline variable references from a text string.
///
/// This finds all `$var` and `_var` references in text that is
/// NOT inside a macro or link (those are handled by their own parsers).
fn scan_inline_vars(text: &str, offset: usize) -> Vec<VarRef> {
    let mut refs = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'$' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            let (var_ref, end) = scan_variable(text, i, false);
            let mut vr = var_ref;
            vr.span = offset + vr.span.start..offset + vr.span.end;
            refs.push(vr);
            i = end;
        } else if bytes[i] == b'_' && i + 1 < len && is_ident_start(bytes[i + 1]) {
            // _var: only match at word boundary (not inside a word)
            let is_word_boundary = i == 0
                || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if is_word_boundary {
                let (var_ref, end) = scan_variable(text, i, true);
                // Verify it's a valid temporary variable (not just an underscore in text)
                // SugarCube temp vars are _ followed by an identifier
                let mut vr = var_ref;
                vr.span = offset + vr.span.start..offset + vr.span.end;
                refs.push(vr);
                i = end;
            } else {
                i += 1;
            }
        } else if bytes[i] == b'$' && i + 1 < len && bytes[i + 1] == b'$' {
            // $$ — escaped dollar, skip
            i += 2;
        } else {
            i += 1;
        }
    }

    refs
}

// ---------------------------------------------------------------------------
// AST extraction helpers
// ---------------------------------------------------------------------------

/// Extract LinkInfo from the AST (flattened, including nested macros).
fn extract_links_from_ast(nodes: &[AstNode]) -> Vec<LinkInfo> {
    let mut links = Vec::new();
    extract_links_recursive(nodes, &mut links);
    links
}

fn extract_links_recursive(nodes: &[AstNode], links: &mut Vec<LinkInfo>) {
    for node in nodes {
        match node {
            AstNode::Link {
                display,
                target,
                span,
                ..
            } => {
                let is_dynamic = target.starts_with('$') || target.starts_with('_');
                links.push(LinkInfo {
                    display: display.clone(),
                    target: target.clone(),
                    span: span.clone(),
                    is_dynamic,
                });
            }
            AstNode::Macro { children, .. } => {
                if let Some(ch) = children {
                    extract_links_recursive(ch, links);
                }
            }
            _ => {}
        }
    }
}

/// Extract VarOpInfo from the AST (flattened, including nested macros).
fn extract_var_ops_from_ast(nodes: &[AstNode]) -> Vec<VarOpInfo> {
    let mut ops = Vec::new();
    extract_var_ops_recursive(nodes, &mut ops, false);
    ops
}

fn extract_var_ops_recursive(
    nodes: &[AstNode],
    ops: &mut Vec<VarOpInfo>,
    in_assignment: bool,
) {
    for node in nodes {
        match node {
            AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    ops.push(VarOpInfo {
                        name: vr.name.clone(),
                        property_path: vr.property_path.clone(),
                        is_temporary: vr.is_temporary,
                        is_write: false, // Text vars are always reads
                        span: vr.span.clone(),
                    });
                }
            }
            AstNode::Macro {
                name,
                var_refs,
                children,
                ..
            } => {
                let is_assignment = is_assignment_macro(name);
                // Macro's own var refs
                for vr in var_refs {
                    ops.push(VarOpInfo {
                        name: vr.name.clone(),
                        property_path: vr.property_path.clone(),
                        is_temporary: vr.is_temporary,
                        is_write: is_assignment,
                        span: vr.span.clone(),
                    });
                }
                // Recurse into children
                if let Some(ch) = children {
                    extract_var_ops_recursive(ch, ops, is_assignment);
                }
            }
            AstNode::Expression { var_refs, .. } => {
                for vr in var_refs {
                    ops.push(VarOpInfo {
                        name: vr.name.clone(),
                        property_path: vr.property_path.clone(),
                        is_temporary: vr.is_temporary,
                        is_write: false, // Expressions are reads
                        span: vr.span.clone(),
                    });
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Helper predicates
// ---------------------------------------------------------------------------

/// Check if a character can start an identifier.
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

/// Check if a character can continue an identifier.
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Check if a macro name is a block macro (has a close tag).
fn is_block_macro(name: &str) -> bool {
    // Use the macros module's block_macro_names set
    macros::block_macro_names().contains(name)
        || name.eq_ignore_ascii_case("widget")
        || name.eq_ignore_ascii_case("script")
        || name.eq_ignore_ascii_case("style")
        || name.eq_ignore_ascii_case("css")
        || name.eq_ignore_ascii_case("nobr")
        || name.eq_ignore_ascii_case("silently")
        || name.eq_ignore_ascii_case("done")
        || name.eq_ignore_ascii_case("capture")
}

/// Check if a macro name assigns/writes variables.
fn is_assignment_macro(name: &str) -> bool {
    macros::variable_assignment_macros().contains(name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_text() {
        let ast = parse_passage_body("Hello world", 0, ParseMode::Normal);
        assert_eq!(ast.nodes.len(), 1);
        match &ast.nodes[0] {
            AstNode::Text { content, .. } => assert_eq!(content, "Hello world"),
            _ => panic!("Expected Text node"),
        }
    }

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
    fn parse_inline_macro() {
        let ast = parse_passage_body("<<set $hp to 100>>", 0, ParseMode::Normal);
        assert!(ast.var_ops.len() >= 1);
        assert_eq!(ast.var_ops[0].name, "$hp");
        assert!(ast.var_ops[0].is_write);
    }

    #[test]
    fn parse_block_macro() {
        let ast = parse_passage_body("<<if $alive>>You live!<</if>>", 0, ParseMode::Normal);
        // Should have: Text("You live!") inside the if block
        let macros = collect_macros(&ast.nodes);
        assert!(macros.len() >= 1);
        match macros[0] {
            AstNode::Macro { name, children, .. } => {
                assert_eq!(name, "if");
                assert!(children.is_some());
            }
            _ => panic!("Expected Macro node"),
        }
    }

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
    fn parse_variable_in_text() {
        let ast = parse_passage_body("You have $gold coins.", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert_eq!(ast.var_ops[0].name, "$gold");
        assert!(!ast.var_ops[0].is_write);
    }

    #[test]
    fn parse_temp_variable() {
        let ast = parse_passage_body("<<set _i to 0>>", 0, ParseMode::Normal);
        assert!(!ast.var_ops.is_empty());
        assert!(ast.var_ops.iter().any(|v| v.name == "_i" && v.is_temporary && v.is_write));
    }

    #[test]
    fn parse_nested_macros() {
        let ast = parse_passage_body(
            "<<if $alive>><<set $msg to \"yes\">>You live<</if>>",
            0,
            ParseMode::Normal,
        );
        let macros = collect_macros(&ast.nodes);
        // Should find both <<if>> and <<set>>
        assert!(macros.len() >= 2);
    }

    #[test]
    fn parse_unclosed_macro() {
        let ast = parse_passage_body("<<if $alive>>never closed", 0, ParseMode::Normal);
        // Should not panic, should produce an AST with the unclosed block
        let macros = collect_macros(&ast.nodes);
        assert!(macros.len() >= 1);
        match macros[0] {
            AstNode::Macro { name, close_span, .. } => {
                assert_eq!(name, "if");
                assert!(close_span.is_none()); // Unclosed
            }
            _ => panic!("Expected Macro node"),
        }
    }

    #[test]
    fn escaped_dollar() {
        let ast = parse_passage_body("$$notavar", 0, ParseMode::Normal);
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
    fn setter_link() {
        let ast = parse_passage_body("[[target][$var to 5]]", 0, ParseMode::Normal);
        assert!(ast.links.len() >= 1);
        assert_eq!(ast.links[0].target, "target");
    }

    #[test]
    fn expression_macro() {
        let ast = parse_passage_body("<<= $hp>>", 0, ParseMode::Normal);
        let has_expr = ast.nodes.iter().any(|n| matches!(n, AstNode::Expression { kind: ExprKind::Print, .. }));
        assert!(has_expr);
    }

    #[test]
    fn stylesheet_mode_empty() {
        let ast = parse_passage_body("body { color: red; }", 0, ParseMode::Stylesheet);
        assert!(ast.nodes.is_empty());
    }

    #[test]
    fn script_mode_empty() {
        let ast = parse_passage_body("var x = 5;", 0, ParseMode::Script);
        assert!(ast.nodes.is_empty());
    }
}
