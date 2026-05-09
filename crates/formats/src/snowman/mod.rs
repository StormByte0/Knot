//! Snowman Format Plugin
//!
//! Snowman is a minimal story format aimed at developers who prefer writing
//! JavaScript and Underscore.js templates.
//!
//! This module implements a full, production-quality parser with:
//!
//! - **Byte-offset tracking** for passage splitting (no buggy `text.find()`)
//! - **ERB-style template parsing**: `<%= expr %>`, `<% code %>`, `<%- expr %>`
//! - **Variable tracking**: `s.varName` reads/writes, `window.story.state` alias
//! - **Diagnostics**: unclosed `<% %>`, unclosed `[[`, undefined variable warnings
//! - **Block model**: Text, Macro (`<% %>`), Expression (`<%= %>`), Incomplete

use knot_core::passage::{
    Block, Link, Passage, SpecialPassageBehavior, SpecialPassageDef, StoryFormat, VarKind, VarOp,
};
use regex::Regex;
use std::ops::Range;
use url::Url;

use crate::plugin::{
    FormatDiagnostic, FormatDiagnosticSeverity, FormatPlugin, ParseResult, SemanticToken,
    SemanticTokenModifier, SemanticTokenType,
};

// ---------------------------------------------------------------------------
// ERB template segment
// ---------------------------------------------------------------------------

/// A parsed segment of a Snowman passage body.
#[derive(Debug, Clone)]
enum TemplateSegment {
    /// Plain text content.
    Text {
        content: String,
        span: Range<usize>,
    },
    /// Script block: `<% code %>`
    Script {
        code: String,
        span: Range<usize>,
    },
    /// Expression output: `<%= expr %>`
    Expression {
        expr: String,
        span: Range<usize>,
    },
    /// Unescaped expression output: `<%- expr %>`
    UnescapedExpression {
        expr: String,
        span: Range<usize>,
    },
    /// Incomplete (unclosed) block: `<%` without `%>` or `[[` without `]]`
    Incomplete {
        content: String,
        span: Range<usize>,
    },
}

// ---------------------------------------------------------------------------
// Parsed header
// ---------------------------------------------------------------------------

/// The result of parsing a single passage header line.
struct ParsedHeader {
    name: String,
    tags: Vec<String>,
    /// Byte offset where the header line starts.
    header_start: usize,
    /// Byte offset where the header line ends (exclusive, before newline).
    header_end: usize,
}

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// Snowman format plugin.
pub struct SnowmanPlugin {
    /// Regex for simple links: `[[Target]]`
    re_link_simple: Regex,
    /// Regex for arrow links: `[[Display->Target]]`
    re_link_arrow: Regex,
    /// Regex for pipe links: `[[Display|Target]]`
    re_link_pipe: Regex,
    /// Regex for Snowman state variable reads: `s.variableName`
    re_var_read: Regex,
    /// Regex for Snowman state variable writes: `s.variableName =`
    re_var_write: Regex,
    /// Regex for window.story.state variable reads: `window.story.state.variableName`
    re_wss_var_read: Regex,
    /// Regex for window.story.state variable writes: `window.story.state.variableName =`
    re_wss_var_write: Regex,
    /// Regex for passage headers.
    re_header: Regex,
}

impl Default for SnowmanPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SnowmanPlugin {
    /// Create a new Snowman plugin instance.
    pub fn new() -> Self {
        Self {
            re_link_simple: Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap(),
            re_link_arrow: Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap(),
            re_link_pipe: Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap(),
            // s.variableName — read access (also matches writes; filtered below)
            re_var_read: Regex::new(r"\bs\.([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            // s.variableName = — write access
            re_var_write: Regex::new(r"\bs\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap(),
            // window.story.state.variableName — read access
            re_wss_var_read: Regex::new(r"window\.story\.state\.([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            // window.story.state.variableName = — write access
            re_wss_var_write: Regex::new(r"window\.story\.state\.([A-Za-z_][A-Za-z0-9_]*)\s*=")
                .unwrap(),
            re_header: Regex::new(r"^::\s*(.+?)(?:\s+\[([^\]]*)\])?\s*$").unwrap(),
        }
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passages (byte-offset tracking)
    // -----------------------------------------------------------------------

    /// Split source into passages using proper byte-offset tracking.
    ///
    /// Returns a list of `(ParsedHeader, body_text)` pairs. The body text is
    /// the raw text between the end of this header line and the start of the
    /// next header (or end of file).
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(ParsedHeader, &'a str)> {
        let mut headers: Vec<ParsedHeader> = Vec::new();
        let mut byte_offset = 0;

        for line in text.lines() {
            let line_start = byte_offset;
            let line_end = line_start + line.len();

            if let Some(caps) = self.re_header.captures(line) {
                let name = caps.get(1).unwrap().as_str().trim().to_string();
                let tags = caps
                    .get(2)
                    .map(|m| {
                        m.as_str()
                            .split_whitespace()
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                headers.push(ParsedHeader {
                    name,
                    tags,
                    header_start: line_start,
                    header_end: line_end,
                });
            }

            byte_offset = line_end + 1; // +1 for the newline character
        }

        // Build passage bodies
        let mut results = Vec::new();
        for header in headers.into_iter() {
            // Body starts after the header line + its trailing newline
            let body_start = if header.header_end < text.len() {
                header.header_end + 1
            } else {
                header.header_end
            };

            // Body ends at the start of the next header, or end of text
            // We need to find the next header start. Since we consumed headers,
            // we use a different approach: scan the remaining text.
            let body_end = {
                // Look for the next `::` at the start of a line
                let remaining = &text[body_start..];
                let mut found = text.len();
                let mut off = 0;
                for line in remaining.lines() {
                    let line_off = off;
                    off = line_off + line.len() + 1; // +1 for newline
                    if line.starts_with("::") {
                        found = body_start + line_off;
                        break;
                    }
                }
                found
            };

            let body = text.get(body_start..body_end).unwrap_or("");
            results.push((header, body));
        }

        results
    }

    // -----------------------------------------------------------------------
    // ERB template parsing
    // -----------------------------------------------------------------------

    /// Parse a passage body into template segments (text, script, expression,
    /// unescaped expression, and incomplete blocks).
    fn parse_template_segments(&self, body: &str, body_offset: usize) -> Vec<TemplateSegment> {
        let mut segments = Vec::new();
        let bytes = body.as_bytes();
        let len = bytes.len();
        let mut pos = 0;

        while pos < len {
            // Look for `<%` or `[[`
            let next_template = Self::find_next_opening(bytes, pos);

            match next_template {
                // Found `<%` at `open_pos`
                Some((open_pos, b'<')) => {
                    // Emit any text before this opening
                    if open_pos > pos {
                        let text_content = body[pos..open_pos].to_string();
                        segments.push(TemplateSegment::Text {
                            content: text_content,
                            span: body_offset + pos..body_offset + open_pos,
                        });
                    }

                    // Determine the type: `<%=`, `<%-`, or `<%`
                    let tag_type = if open_pos + 2 < len && bytes[open_pos + 2] == b'=' {
                        Some(b'=')
                    } else if open_pos + 2 < len && bytes[open_pos + 2] == b'-' {
                        Some(b'-')
                    } else {
                        None
                    };

                    // Content starts after the opening tag
                    let content_start = open_pos + 2 + if tag_type.is_some() { 1 } else { 0 };

                    // Find the closing `%>`
                    let close_pos = Self::find_close_percent_brace(bytes, content_start);

                    match close_pos {
                        Some(cp) => {
                            let content = body[content_start..cp].to_string();
                            let full_end = cp + 2; // past `%>`

                            match tag_type {
                                Some(b'=') => {
                                    segments.push(TemplateSegment::Expression {
                                        expr: content,
                                        span: body_offset + open_pos..body_offset + full_end,
                                    });
                                }
                                Some(b'-') => {
                                    segments.push(TemplateSegment::UnescapedExpression {
                                        expr: content,
                                        span: body_offset + open_pos..body_offset + full_end,
                                    });
                                }
                                None => {
                                    segments.push(TemplateSegment::Script {
                                        code: content,
                                        span: body_offset + open_pos..body_offset + full_end,
                                    });
                                }
                                _ => unreachable!(),
                            }
                            pos = full_end;
                        }
                        None => {
                            // Unclosed `<%` — incomplete block
                            let content = body[open_pos..].to_string();
                            segments.push(TemplateSegment::Incomplete {
                                content,
                                span: body_offset + open_pos..body_offset + len,
                            });
                            pos = len;
                        }
                    }
                }

                // Found `[[` at `open_pos`
                Some((open_pos, b'[')) => {
                    // Emit any text before this opening
                    if open_pos > pos {
                        let text_content = body[pos..open_pos].to_string();
                        segments.push(TemplateSegment::Text {
                            content: text_content,
                            span: body_offset + pos..body_offset + open_pos,
                        });
                    }

                    // Find the closing `]]`
                    let close_pos = Self::find_close_brackets(bytes, open_pos + 2);

                    match close_pos {
                        Some(cp) => {
                            // This is a link — treat the whole `[[...]]` as text
                            // (links are extracted separately via extract_links)
                            let link_text = body[open_pos..cp + 2].to_string();
                            segments.push(TemplateSegment::Text {
                                content: link_text,
                                span: body_offset + open_pos..body_offset + cp + 2,
                            });
                            pos = cp + 2;
                        }
                        None => {
                            // Unclosed `[[` — incomplete block
                            let content = body[open_pos..].to_string();
                            segments.push(TemplateSegment::Incomplete {
                                content,
                                span: body_offset + open_pos..body_offset + len,
                            });
                            pos = len;
                        }
                    }
                }

                // No more opening tags — emit remaining text
                None => {
                    if pos < len {
                        let text_content = body[pos..].to_string();
                        segments.push(TemplateSegment::Text {
                            content: text_content,
                            span: body_offset + pos..body_offset + len,
                        });
                    }
                    pos = len;
                }

                // Unreachable: find_next_opening only returns b'<' or b'['
                Some(_) => {
                    let text_content = body[pos..].to_string();
                    segments.push(TemplateSegment::Text {
                        content: text_content,
                        span: body_offset + pos..body_offset + len,
                    });
                    pos = len;
                }
            }
        }

        segments
    }

    /// Find the next opening `<%` or `[[` starting from `pos`.
    /// Returns `(position, first_byte)` where first_byte is `b'<'` or `b'['`.
    fn find_next_opening(bytes: &[u8], pos: usize) -> Option<(usize, u8)> {
        let len = bytes.len();
        let mut i = pos;
        while i < len {
            if i + 1 < len && bytes[i] == b'<' && bytes[i + 1] == b'%' {
                return Some((i, b'<'));
            }
            if i + 1 < len && bytes[i] == b'[' && bytes[i + 1] == b'[' {
                return Some((i, b'['));
            }
            i += 1;
        }
        None
    }

    /// Find `%>` starting from `pos`.
    fn find_close_percent_brace(bytes: &[u8], pos: usize) -> Option<usize> {
        let len = bytes.len();
        let mut i = pos;
        while i + 1 < len {
            if bytes[i] == b'%' && bytes[i + 1] == b'>' {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Find `]]` starting from `pos`.
    fn find_close_brackets(bytes: &[u8], pos: usize) -> Option<usize> {
        let len = bytes.len();
        let mut i = pos;
        while i + 1 < len {
            if bytes[i] == b']' && bytes[i + 1] == b']' {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    // -----------------------------------------------------------------------
    // Link extraction
    // -----------------------------------------------------------------------

    /// Extract links from a passage body.
    fn extract_links(&self, body: &str, body_offset: usize) -> Vec<Link> {
        let mut links = Vec::new();

        // Arrow links.
        for caps in self.re_link_arrow.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let display = caps.get(1).unwrap().as_str().trim().to_string();
            let target = caps.get(2).unwrap().as_str().trim().to_string();
            links.push(Link {
                display_text: Some(display),
                target,
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }

        // Pipe links.
        for caps in self.re_link_pipe.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let display = caps.get(1).unwrap().as_str().trim().to_string();
            let target = caps.get(2).unwrap().as_str().trim().to_string();
            links.push(Link {
                display_text: Some(display),
                target,
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }

        // Simple links (skip overlaps with arrow/pipe).
        let known_spans: Vec<Range<usize>> = self
            .re_link_arrow
            .captures_iter(body)
            .chain(self.re_link_pipe.captures_iter(body))
            .filter_map(|caps| {
                let m = caps.get(0)?;
                Some(m.start()..m.end())
            })
            .collect();

        for caps in self.re_link_simple.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let span = m.start()..m.end();
            let overlaps = known_spans
                .iter()
                .any(|s| span.start >= s.start && span.end <= s.end);
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

    // -----------------------------------------------------------------------
    // Variable extraction
    // -----------------------------------------------------------------------

    /// Extract variable operations from a passage body.
    ///
    /// Snowman uses `s.variableName = value` for writes and `s.variableName`
    /// for reads. Also supports `window.story.state.variableName` as an alias.
    /// The prefix is stripped: stored as just `variableName`.
    fn extract_vars(&self, body: &str, body_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();
        let mut write_spans: Vec<Range<usize>> = Vec::new();

        // Detect writes via s.varName =
        for caps in self.re_var_write.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            let prefix = format!("s.{}", var_name);
            let var_start = body_offset + full.start();
            let var_end = var_start + prefix.len();
            vars.push(VarOp {
                name: var_name.to_string(),
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect writes via window.story.state.varName =
        for caps in self.re_wss_var_write.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            let prefix = format!("window.story.state.{}", var_name);
            let var_start = body_offset + full.start();
            let var_end = var_start + prefix.len();
            vars.push(VarOp {
                name: var_name.to_string(),
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect reads via s.varName (not already a write)
        for caps in self.re_var_read.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
            let is_write = write_spans
                .iter()
                .any(|s| var_start >= s.start && var_end <= s.end);
            if !is_write {
                let var_name = caps.get(1).unwrap().as_str();
                vars.push(VarOp {
                    name: var_name.to_string(),
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: false,
                });
            }
        }

        // Detect reads via window.story.state.varName (not already a write)
        for caps in self.re_wss_var_read.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
            let is_write = write_spans
                .iter()
                .any(|s| var_start >= s.start && var_end <= s.end);
            if !is_write {
                let var_name = caps.get(1).unwrap().as_str();
                vars.push(VarOp {
                    name: var_name.to_string(),
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: false,
                });
            }
        }

        vars
    }

    // -----------------------------------------------------------------------
    // Block building from template segments
    // -----------------------------------------------------------------------

    /// Build Block list from template segments.
    fn build_blocks(&self, segments: &[TemplateSegment]) -> Vec<Block> {
        segments
            .iter()
            .map(|seg| match seg {
                TemplateSegment::Text { content, span } => Block::Text {
                    content: content.clone(),
                    span: span.clone(),
                },
                TemplateSegment::Script { code, span } => Block::Macro {
                    name: "script".to_string(),
                    args: code.clone(),
                    span: span.clone(),
                },
                TemplateSegment::Expression { expr, span } => Block::Expression {
                    content: expr.clone(),
                    span: span.clone(),
                },
                TemplateSegment::UnescapedExpression { expr, span } => Block::Expression {
                    content: format!("-{}", expr),
                    span: span.clone(),
                },
                TemplateSegment::Incomplete { content, span } => Block::Incomplete {
                    content: content.clone(),
                    span: span.clone(),
                },
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Diagnostics
    // -----------------------------------------------------------------------

    /// Validate a passage body and produce diagnostics.
    fn validate(&self, body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();
        let bytes = body.as_bytes();
        let len = bytes.len();

        // Check for unclosed `<% %>` blocks
        let mut i = 0;
        while i < len {
            if i + 1 < len && bytes[i] == b'<' && bytes[i + 1] == b'%' {
                let open_pos = i;
                // Skip the modifier character if present (= or -)
                let content_start =
                    if i + 2 < len && (bytes[i + 2] == b'=' || bytes[i + 2] == b'-') {
                        i + 3
                    } else {
                        i + 2
                    };

                // Find closing `%>`
                let mut found_close = false;
                let mut j = content_start;
                while j + 1 < len {
                    if bytes[j] == b'%' && bytes[j + 1] == b'>' {
                        found_close = true;
                        i = j + 2;
                        break;
                    }
                    j += 1;
                }

                if !found_close {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + open_pos..body_offset + len.min(open_pos + 2),
                        message: "Unclosed template block `<%` — missing `%>`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sm-unclosed-template".into(),
                    });
                    break; // no more processing needed
                }
            } else {
                i += 1;
            }
        }

        // Check for unclosed link syntax: `[[` without `]]`
        i = 0;
        while i < len {
            if i + 1 < len && bytes[i] == b'[' && bytes[i + 1] == b'[' {
                let open_pos = i;
                let mut found_close = false;
                let mut j = i + 2;
                while j + 1 < len {
                    if bytes[j] == b']' && bytes[j + 1] == b']' {
                        found_close = true;
                        i = j + 2;
                        break;
                    }
                    j += 1;
                }

                if !found_close {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + open_pos..body_offset + len.min(open_pos + 2),
                        message: "Unclosed link `[[` — missing `]]`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sm-unclosed-link".into(),
                    });
                    break;
                }
            } else {
                i += 1;
            }
        }

        diagnostics
    }

    /// Check for undefined variable access across all passages.
    /// A variable read with no preceding write anywhere is suspicious.
    fn check_undefined_vars(passages: &[Passage]) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();

        // Collect all variable names that are written anywhere
        let written_vars: std::collections::HashSet<String> = passages
            .iter()
            .flat_map(|p| p.vars.iter())
            .filter(|v| v.kind == VarKind::Init)
            .map(|v| v.name.clone())
            .collect();

        // Check reads for variables that are never written
        for passage in passages {
            for var in &passage.vars {
                if var.kind == VarKind::Read && !written_vars.contains(&var.name) {
                    diagnostics.push(FormatDiagnostic {
                        range: var.span.clone(),
                        message: format!(
                            "Variable `s.{}` is read but never written to in any passage",
                            var.name
                        ),
                        severity: FormatDiagnosticSeverity::Hint,
                        code: "sm-undefined-var".into(),
                    });
                }
            }
        }

        diagnostics
    }

    // -----------------------------------------------------------------------
    // Semantic tokens
    // -----------------------------------------------------------------------

    /// Generate semantic tokens for a passage body.
    fn body_tokens(&self, body: &str, body_offset: usize) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();
        let mut write_spans: Vec<Range<usize>> = Vec::new();

        // Variable write tokens (s.varName =)
        for caps in self.re_var_write.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            let prefix = format!("s.{}", var_name);
            let start = body_offset + full.start();
            let end = start + prefix.len();
            tokens.push(SemanticToken {
                start,
                length: prefix.len(),
                token_type: SemanticTokenType::Variable,
                modifier: Some(SemanticTokenModifier::Definition),
            });
            write_spans.push(start..end);
        }

        // Variable write tokens (window.story.state.varName =)
        for caps in self.re_wss_var_write.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = caps.get(1).unwrap().as_str();
            let prefix = format!("window.story.state.{}", var_name);
            let start = body_offset + full.start();
            let end = start + prefix.len();
            tokens.push(SemanticToken {
                start,
                length: prefix.len(),
                token_type: SemanticTokenType::Variable,
                modifier: Some(SemanticTokenModifier::Definition),
            });
            write_spans.push(start..end);
        }

        // Variable read tokens (s.varName)
        for caps in self.re_var_read.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let start = body_offset + full.start();
            let end = body_offset + full.end();
            let is_write = write_spans.iter().any(|s| start >= s.start && end <= s.end);
            if !is_write {
                tokens.push(SemanticToken {
                    start,
                    length: full.end() - full.start(),
                    token_type: SemanticTokenType::Variable,
                    modifier: None,
                });
            }
        }

        // Variable read tokens (window.story.state.varName)
        for caps in self.re_wss_var_read.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let start = body_offset + full.start();
            let end = body_offset + full.end();
            let is_write = write_spans.iter().any(|s| start >= s.start && end <= s.end);
            if !is_write {
                tokens.push(SemanticToken {
                    start,
                    length: full.end() - full.start(),
                    token_type: SemanticTokenType::Variable,
                    modifier: None,
                });
            }
        }

        // ERB template block tokens
        let segments = self.parse_template_segments(body, body_offset);
        for seg in &segments {
            match seg {
                TemplateSegment::Script { span, .. } => {
                    tokens.push(SemanticToken {
                        start: span.start,
                        length: span.end - span.start,
                        token_type: SemanticTokenType::Macro,
                        modifier: None,
                    });
                }
                TemplateSegment::Expression { span, .. }
                | TemplateSegment::UnescapedExpression { span, .. } => {
                    tokens.push(SemanticToken {
                        start: span.start,
                        length: span.end - span.start,
                        token_type: SemanticTokenType::Macro,
                        modifier: None,
                    });
                }
                TemplateSegment::Incomplete { span, .. } => {
                    tokens.push(SemanticToken {
                        start: span.start,
                        length: span.end - span.start,
                        token_type: SemanticTokenType::Keyword,
                        modifier: None,
                    });
                }
                TemplateSegment::Text { .. } => {}
            }
        }

        // Link tokens.
        for caps in self.re_link_arrow.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
        for caps in self.re_link_pipe.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
        for caps in self.re_link_simple.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }

        tokens
    }

    // -----------------------------------------------------------------------
    // Special passage definitions
    // -----------------------------------------------------------------------

    /// Snowman special passage definitions.
    fn special_passage_defs() -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "StoryTitle".into(),
                behavior: SpecialPassageBehavior::Metadata,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
            },
            SpecialPassageDef {
                name: "StoryData".into(),
                behavior: SpecialPassageBehavior::Metadata,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
            },
            SpecialPassageDef {
                name: "Script".into(),
                behavior: SpecialPassageBehavior::Custom("SnowmanScript".into()),
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
            },
            SpecialPassageDef {
                name: "Style".into(),
                behavior: SpecialPassageBehavior::Custom("SnowmanStyle".into()),
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
            },
            SpecialPassageDef {
                name: "PassageHeader".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(90),
            },
            SpecialPassageDef {
                name: "PassageFooter".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(110),
            },
        ]
    }
}

impl FormatPlugin for SnowmanPlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::Snowman
    }

    fn parse(&self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        let raw_passages = self.split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_end + 1;

            // Determine if this is a special passage by name
            let special_defs = Self::special_passage_defs();
            let special_def = special_defs.iter().find(|d| d.name == header.name).cloned();

            // Check if this passage has header/footer tags
            let is_header_tagged = header.tags.iter().any(|t| t == "header");
            let is_footer_tagged = header.tags.iter().any(|t| t == "footer");

            let mut passage = if let Some(def) = special_def {
                Passage::new_special(
                    header.name.clone(),
                    header.header_start..body_offset + body.len(),
                    def,
                )
            } else if is_header_tagged {
                Passage::new_special(
                    header.name.clone(),
                    header.header_start..body_offset + body.len(),
                    SpecialPassageDef {
                        name: "PassageHeader".into(),
                        behavior: SpecialPassageBehavior::Chrome,
                        contributes_variables: false,
                        participates_in_graph: false,
                        execution_priority: Some(90),
                    },
                )
            } else if is_footer_tagged {
                Passage::new_special(
                    header.name.clone(),
                    header.header_start..body_offset + body.len(),
                    SpecialPassageDef {
                        name: "PassageFooter".into(),
                        behavior: SpecialPassageBehavior::Chrome,
                        contributes_variables: false,
                        participates_in_graph: false,
                        execution_priority: Some(110),
                    },
                )
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();
            passage.links = self.extract_links(body, body_offset);
            passage.vars = self.extract_vars(body, body_offset);

            // Build blocks from template segments
            let segments = self.parse_template_segments(body, body_offset);
            passage.body = self.build_blocks(&segments);

            // Header token.
            tokens.push(SemanticToken {
                start: header.header_start,
                length: 2,
                token_type: SemanticTokenType::PassageHeader,
                modifier: None,
            });

            // Body tokens.
            tokens.extend(self.body_tokens(body, body_offset));

            // Validation diagnostics.
            let body_diags = self.validate(body, body_offset);
            for d in &body_diags {
                if matches!(d.severity, FormatDiagnosticSeverity::Error) {
                    has_errors = true;
                }
            }
            diagnostics.extend(body_diags);

            passages.push(passage);
        }

        // Cross-passage undefined variable check
        let var_diags = Self::check_undefined_vars(&passages);
        diagnostics.extend(var_diags);

        ParseResult {
            passages,
            tokens,
            diagnostics,
            is_complete: !has_errors,
        }
    }

    fn parse_passage(&self, passage_name: &str, passage_text: &str) -> Option<Passage> {
        let special_defs = Self::special_passage_defs();
        let special_def = special_defs.iter().find(|d| d.name == passage_name).cloned();

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.links = self.extract_links(passage_text, 0);
        passage.vars = self.extract_vars(passage_text, 0);

        // Build blocks from template segments
        let segments = self.parse_template_segments(passage_text, 0);
        passage.body = self.build_blocks(&segments);

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    fn display_name(&self) -> &str {
        "Snowman 2"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_passage() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nYou are in a room. [[Cave]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Cave");
    }

    #[test]
    fn parse_variable_operations() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% s.gold = 10; %>You have <%= s.gold %> coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Init),
            "Should detect s.gold write"
        );
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Read),
            "Should detect s.gold read"
        );
    }

    #[test]
    fn detect_special_passages() {
        let plugin = SnowmanPlugin::new();
        assert!(plugin.is_special_passage("Script"));
        assert!(plugin.is_special_passage("Style"));
        assert!(plugin.is_special_passage("PassageHeader"));
        assert!(plugin.is_special_passage("PassageFooter"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = SnowmanPlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");
        assert!(result.passages.is_empty());
    }

    // -----------------------------------------------------------------------
    // ERB template parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn expression_block_variable_read() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nYou have <%= s.gold %> coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Read),
            "Should detect s.gold as read in <%= %> block"
        );

        // Check that we have an Expression block
        let expr_blocks: Vec<_> = result.passages[0]
            .body
            .iter()
            .filter(|b| matches!(b, Block::Expression { .. }))
            .collect();
        assert_eq!(expr_blocks.len(), 1, "Should have one Expression block");
        if let Block::Expression { content, .. } = expr_blocks[0] {
            assert_eq!(content.trim(), "s.gold");
        }
    }

    #[test]
    fn script_block_variable_write() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% s.gold = 10; %>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Init),
            "Should detect s.gold as write in <% %> block"
        );

        // Check that we have a Macro block
        let macro_blocks: Vec<_> = result.passages[0]
            .body
            .iter()
            .filter(|b| matches!(b, Block::Macro { .. }))
            .collect();
        assert_eq!(macro_blocks.len(), 1, "Should have one Macro block");
        if let Block::Macro { name, args, .. } = macro_blocks[0] {
            assert_eq!(name, "script");
            assert!(args.contains("s.gold = 10"));
        }
    }

    #[test]
    fn unescaped_expression_block() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nRaw: <%- s.gold %>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);

        // Check variable read
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Read),
            "Should detect s.gold as read in <%- %> block"
        );

        // Check Expression block (unescaped expressions are also Expression blocks)
        let expr_blocks: Vec<_> = result.passages[0]
            .body
            .iter()
            .filter(|b| matches!(b, Block::Expression { .. }))
            .collect();
        assert_eq!(expr_blocks.len(), 1, "Should have one Expression block for <%- %>");
        if let Block::Expression { content, .. } = expr_blocks[0] {
            assert!(
                content.contains("s.gold"),
                "Expression content should contain s.gold, got: {}",
                content
            );
        }
    }

    #[test]
    fn unclosed_template_diagnostic() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% s.gold = 10\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "sm-unclosed-template"),
            "Should produce unclosed template diagnostic"
        );
    }

    #[test]
    fn unclosed_link_diagnostic() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n[[Unclosed link\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "sm-unclosed-link"),
            "Should produce unclosed link diagnostic"
        );
    }

    #[test]
    fn mixed_text_expression_script_blocks() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nHello <% s.name = \"world\"; %><%= s.name %>!\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let body = &result.passages[0].body;

        // Should have: Text, Macro, Expression, Text
        let text_count = body.iter().filter(|b| matches!(b, Block::Text { .. })).count();
        let macro_count = body.iter().filter(|b| matches!(b, Block::Macro { .. })).count();
        let expr_count = body
            .iter()
            .filter(|b| matches!(b, Block::Expression { .. }))
            .count();

        assert!(
            text_count >= 2,
            "Should have at least 2 Text blocks, got {}",
            text_count
        );
        assert_eq!(macro_count, 1, "Should have 1 Macro block");
        assert_eq!(expr_count, 1, "Should have 1 Expression block");

        // Variable operations
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "name" && v.kind == VarKind::Init),
            "Should detect s.name write"
        );
        assert!(
            vars.iter().any(|v| v.name == "name" && v.kind == VarKind::Read),
            "Should detect s.name read"
        );
    }

    #[test]
    fn variable_read_in_text_context() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nYou have s.gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Read),
            "Should detect s.gold as read even in plain text context"
        );
    }

    #[test]
    fn variable_write_in_script_context() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% s.health = 100; s.mana = 50; %>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "health" && v.kind == VarKind::Init),
            "Should detect s.health write"
        );
        assert!(
            vars.iter().any(|v| v.name == "mana" && v.kind == VarKind::Init),
            "Should detect s.mana write"
        );
    }

    #[test]
    fn multiple_variable_operations() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% s.x = 1; s.y = 2; %>Sum: <%= s.x + s.y %>.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;

        // Writes
        assert!(
            vars.iter().any(|v| v.name == "x" && v.kind == VarKind::Init),
            "Should detect s.x write"
        );
        assert!(
            vars.iter().any(|v| v.name == "y" && v.kind == VarKind::Init),
            "Should detect s.y write"
        );

        // Reads
        assert!(
            vars.iter().any(|v| v.name == "x" && v.kind == VarKind::Read),
            "Should detect s.x read"
        );
        assert!(
            vars.iter().any(|v| v.name == "y" && v.kind == VarKind::Read),
            "Should detect s.y read"
        );
    }

    #[test]
    fn empty_script_block() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% %>empty block\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);

        // Should have a Macro block with empty args
        let macro_blocks: Vec<_> = result.passages[0]
            .body
            .iter()
            .filter(|b| matches!(b, Block::Macro { .. }))
            .collect();
        assert_eq!(macro_blocks.len(), 1, "Should have one Macro block");
        if let Block::Macro { name, args, .. } = macro_blocks[0] {
            assert_eq!(name, "script");
            assert_eq!(args.trim(), "", "Empty script block should have empty args");
        }
    }

    #[test]
    fn incomplete_block_from_unclosed_template() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\nHello <% unclosed\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let incomplete: Vec<_> = result.passages[0]
            .body
            .iter()
            .filter(|b| matches!(b, Block::Incomplete { .. }))
            .collect();
        assert_eq!(incomplete.len(), 1, "Should have one Incomplete block");
    }

    #[test]
    fn passage_header_footer_special() {
        let plugin = SnowmanPlugin::new();
        // Passages tagged with [header] and [footer] should be treated as special
        let src = ":: MyHeader [header]\nHeader content\n:: MyFooter [footer]\nFooter content\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert!(
            result.passages[0].is_special,
            "Header-tagged passage should be special"
        );
        assert!(
            result.passages[1].is_special,
            "Footer-tagged passage should be special"
        );
    }

    #[test]
    fn split_passages_byte_offset_tracking() {
        let plugin = SnowmanPlugin::new();
        // Test with duplicate content that would break text.find()
        let src = ":: Start\nYou are here.\nYou are here.\n:: Cave\nYou are here.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Cave");
    }

    #[test]
    fn window_story_state_alias() {
        let plugin = SnowmanPlugin::new();
        let src = ":: Start\n<% window.story.state.gold = 10; %>You have <%= window.story.state.gold %>.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Init),
            "Should detect window.story.state.gold write"
        );
        assert!(
            vars.iter().any(|v| v.name == "gold" && v.kind == VarKind::Read),
            "Should detect window.story.state.gold read"
        );
    }

    #[test]
    fn undefined_variable_hint() {
        let plugin = SnowmanPlugin::new();
        // s.never_written is read but never written anywhere
        let src = ":: Start\nYou have <%= s.never_written %> coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "sm-undefined-var"),
            "Should warn about undefined variable"
        );
    }

    #[test]
    fn no_undefined_var_warning_when_written() {
        let plugin = SnowmanPlugin::new();
        // s.gold is written in one passage and read in another
        let src = ":: Start\n<% s.gold = 10; %>\n:: Room\nYou have <%= s.gold %> coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.code == "sm-undefined-var" && d.message.contains("gold")),
            "Should NOT warn about s.gold when it is written in another passage"
        );
    }
}
