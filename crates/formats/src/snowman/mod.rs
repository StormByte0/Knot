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
    Block, Link, MatchStrategy, Passage, SpecialPassageBehavior, SpecialPassageDef, SpecialPassageLayer,
    StoryFormat, VarKind, VarOp,
};
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;
use url::Url;


use crate::header::{self, TweeHeader};
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
// Compiled regexes (module-level LazyLock)
// ---------------------------------------------------------------------------

/// Regex for simple links: `[[Target]]`
static RE_LINK_SIMPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap());
/// Regex for arrow links: `[[Display->Target]]`
static RE_LINK_ARROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap());
/// Regex for pipe links: `[[Display|Target]]`
static RE_LINK_PIPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap());
/// Regex for Snowman state variable reads: `s.variableName`
static RE_VAR_READ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bs\.([A-Za-z_][A-Za-z0-9_]*)").unwrap());
/// Regex for Snowman state variable writes: `s.variableName =`
static RE_VAR_WRITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bs\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap());
/// Regex for window.story.state variable reads: `window.story.state.variableName`
static RE_WSS_VAR_READ: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"window\.story\.state\.([A-Za-z_][A-Za-z0-9_]*)").unwrap());
/// Regex for window.story.state variable writes: `window.story.state.variableName =`
static RE_WSS_VAR_WRITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"window\.story\.state\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap());
/// Detect passage header lines: starts with `::` followed by at least one
/// non-whitespace character. Actual parsing done by unified parser.
static RE_HEADER_DETECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^::\s*\S").unwrap());

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// Snowman format plugin.
pub struct SnowmanPlugin {}

impl Default for SnowmanPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SnowmanPlugin {
    /// Create a new Snowman plugin instance.
    pub fn new() -> Self {
        Self {}
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passages (byte-offset tracking)
    // -----------------------------------------------------------------------

    /// Split source into passages using proper byte-offset tracking.
    ///
    /// Returns a list of `(TweeHeader, body_text)` pairs. The body text is
    /// the raw text between the end of this header line and the start of the
    /// next header (or end of file).
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(TweeHeader, &'a str)> {
        // Collect header spans: (line_start, line_end) for each detected header line.
        let mut header_spans: Vec<(usize, usize)> = Vec::new();
        let mut byte_offset = 0;

        for line in text.lines() {
            let line_start = byte_offset;
            let line_end = line_start + line.len();

            if RE_HEADER_DETECT.is_match(line) {
                header_spans.push((line_start, line_end));
            }

            // Detect actual newline length: CRLF is 2 bytes, LF is 1 byte.
            let newline_len = if text.get(line_end..line_end + 2) == Some("\r\n") { 2 } else if line_end < text.len() { 1 } else { 0 };
            byte_offset = line_end + newline_len;
        }

        // Build passage bodies and parse headers via the unified parser.
        let mut results = Vec::new();
        for &(line_start, line_end) in &header_spans {
            let header_line = &text[line_start..line_end];

            // Use the unified header parser for content extraction.
            let header = match header::parse_twee_header(header_line, line_start) {
                Some(h) => h,
                None => continue,
            };

            // Body starts after the header line + its trailing newline (CRLF = 2, LF = 1)
            let newline_len = if text.get(line_end..line_end + 2) == Some("\r\n") { 2 } else if line_end < text.len() { 1 } else { 0 };
            let body_start = line_end + newline_len;

            // Body ends at the start of the next header, or end of text.
            let body_end = header_spans
                .iter()
                .find(|&&(s, _)| s > line_start)
                .map(|&(s, _)| s)
                .unwrap_or(text.len());

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
        for caps in RE_LINK_ARROW.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let display = caps.get(1).unwrap().as_str().trim().to_string();
            let target = caps.get(2).unwrap().as_str().trim().to_string();
            // Filter: skip targets containing "::" — JS namespace accessor
            if target.contains("::") {
                continue;
            }
            links.push(Link {
                display_text: Some(display),
                target,
                span: body_offset + m.start()..body_offset + m.end(),
                edge_type_hint: None,
            });
        }

        // Pipe links.
        for caps in RE_LINK_PIPE.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let display = caps.get(1).unwrap().as_str().trim().to_string();
            let target = caps.get(2).unwrap().as_str().trim().to_string();
            // Filter: skip targets containing "::" — JS namespace accessor
            if target.contains("::") {
                continue;
            }
            links.push(Link {
                display_text: Some(display),
                target,
                span: body_offset + m.start()..body_offset + m.end(),
                edge_type_hint: None,
            });
        }

        // Simple links (skip overlaps with arrow/pipe).
        let known_spans: Vec<Range<usize>> = RE_LINK_ARROW
            .captures_iter(body)
            .chain(RE_LINK_PIPE.captures_iter(body))
            .filter_map(|caps| {
                let m = caps.get(0)?;
                Some(m.start()..m.end())
            })
            .collect();

        for caps in RE_LINK_SIMPLE.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let span = m.start()..m.end();
            let overlaps = known_spans
                .iter()
                .any(|s| span.start >= s.start && span.end <= s.end);
            if !overlaps {
                let target = caps.get(1).unwrap().as_str().trim().to_string();
                // Filter: skip targets containing "::" — JS namespace accessor
                if target.contains("::") {
                    continue;
                }
                links.push(Link {
                    display_text: None,
                    target,
                    span: body_offset + m.start()..body_offset + m.end(),
                    edge_type_hint: None,
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
        for caps in RE_VAR_WRITE.captures_iter(body) {
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
        for caps in RE_WSS_VAR_WRITE.captures_iter(body) {
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
        for caps in RE_VAR_READ.captures_iter(body) {
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
        for caps in RE_WSS_VAR_READ.captures_iter(body) {
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
        for caps in RE_VAR_WRITE.captures_iter(body) {
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
        for caps in RE_WSS_VAR_WRITE.captures_iter(body) {
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
        for caps in RE_VAR_READ.captures_iter(body) {
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
        for caps in RE_WSS_VAR_READ.captures_iter(body) {
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
        for caps in RE_LINK_ARROW.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
        for caps in RE_LINK_PIPE.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
        for caps in RE_LINK_SIMPLE.captures_iter(body) {
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

    /// Snowman name-matched special passage definitions.
    ///
    /// Only `Script`, `Style`, `PassageHeader`, and `PassageFooter` are
    /// name-matched. The `[header]` and `[footer]` tag-matched definitions
    /// live in `tag_matched_special_passages()`.
    fn special_passage_defs() -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "Script".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::Custom("SnowmanScript".into()),
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "Style".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::Custom("SnowmanStyle".into()),
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "PassageHeader".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(90),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "PassageFooter".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(110),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
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
            let header_line_end = text[header.header_start..]
                .find('\n')
                .map(|i| header.header_start + i)
                .unwrap_or(text.len());
            let newline_len = if text.get(header_line_end..header_line_end + 2) == Some("\r\n") { 2 } else if header_line_end < text.len() { 1 } else { 0 };
            let body_offset = header_line_end + newline_len;

            // Determine if this is a special passage using the unified
            // classification system. Tags are checked FIRST (per the
            // Twee 3 spec), then names. This replaces the old manual
            // three-stage lookup (format defs → core defs → tag fallback).
            let special_def = self.classify_passage(&header.name, &header.tags);

            let mut passage = if let Some(ref def) = special_def {
                Passage::new_special(
                    header.name.clone(),
                    header.header_start..body_offset + body.len(),
                    def.clone(),
                )
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();

            // ── Context-aware parsing ──────────────────────────────────────
            // Detect script and stylesheet passages. These contain non-Twine
            // content (JavaScript or CSS) and should NOT be parsed with
            // Snowman's ERB template segment regexes.
            //
            // Script passages: tagged [script] (Twine-core tag)
            // Stylesheet passages: tagged [stylesheet] or [style] (Twine-core tags)
            let is_script = passage.is_script_passage();
            let is_stylesheet = passage.is_stylesheet_passage();

            if is_script || is_stylesheet {
                // Script/stylesheet passages: store as raw text, skip
                // Snowman-specific template segment parsing.
                passage.body = crate::core_specials::raw_body_blocks(body, body_offset);

                // Header + tag tokens.
                // Use core_specials helpers for correct token types and layer
                // modifiers — this fixes the bug where tag-matched core passages
                // (e.g., [script], [stylesheet]) got PassageHeader/PassageName
                // instead of SpecialPassageHeader/SpecialPassage.
                let layer = crate::core_specials::layer_from_special_def(special_def.as_ref());
                tokens.extend(crate::core_specials::build_special_header_tokens(
                    header.name_start,
                    header.name.len(),
                    layer,
                ));
                tokens.extend(crate::core_specials::build_tag_tokens(header, self));
            } else {
                passage.links = self.extract_links(body, body_offset);
                passage.vars = self.extract_vars(body, body_offset);

                // Build blocks from template segments
                let segments = self.parse_template_segments(body, body_offset);
                passage.body = self.build_blocks(&segments);

                // Header + tag tokens.
                // Use core_specials helpers for correct token types and layer
                // modifiers for special passages (both name-matched and
                // tag-matched core passages).
                let is_special_for_tokens = crate::core_specials::is_special_for_tokens(
                    self, &header.name, &header.tags, special_def.as_ref(),
                );
                if is_special_for_tokens {
                    let layer = crate::core_specials::layer_from_special_def(special_def.as_ref());
                    tokens.extend(crate::core_specials::build_special_header_tokens(
                        header.name_start,
                        header.name.len(),
                        layer,
                    ));
                } else {
                    tokens.push(SemanticToken {
                        start: header.header_start,
                        length: 2,
                        token_type: SemanticTokenType::PassageHeader,
                        modifier: None,
                    });
                    tokens.push(SemanticToken {
                        start: header.name_start,
                        length: header.name.len(),
                        token_type: SemanticTokenType::PassageName,
                        modifier: None,
                    });
                }
                tokens.extend(crate::core_specials::build_tag_tokens(header, self));

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
            }

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

    fn parse_passage(&self, passage_name: &str, passage_tags: &[String], passage_text: &str) -> Option<Passage> {
        // For incremental re-parse: we receive body text and tags.
        let special_def = self.classify_passage(passage_name, passage_tags);

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.tags = passage_tags.to_vec();

        // Context-aware parsing: skip format-specific body parsing for
        // Twine-core script/stylesheet passages.
        let is_script = passage.is_script_passage();
        let is_stylesheet = passage.is_stylesheet_passage();

        if is_script || is_stylesheet {
            // Script/stylesheet passages: store as raw text.
            passage.body = crate::core_specials::raw_body_blocks(passage_text, 0);
        } else {
            passage.links = self.extract_links(passage_text, 0);
            passage.vars = self.extract_vars(passage_text, 0);

            // Build blocks from template segments
            let segments = self.parse_template_segments(passage_text, 0);
            passage.body = self.build_blocks(&segments);
        }

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    /// Snowman tag-matched special passage definitions.
    ///
    /// In Snowman, `[header]` and `[footer]` are TAG-based special
    /// passages — the passage name is user-defined and irrelevant for
    /// classification. A passage like `:: TopBar [header]` is classified
    /// as a Chrome passage by its tag, not its name.
    ///
    /// This override ensures that `classify_passage()` (used by both
    /// `parse()` and `parse_passage()`) correctly identifies tag-matched
    /// special passages, fixing the incremental re-parse path that was
    /// previously broken because the default `tag_matched_special_passages()`
    /// returned an empty vec.
    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "header".into(),
                match_strategy: MatchStrategy::Tag,
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(90),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "footer".into(),
                match_strategy: MatchStrategy::Tag,
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(110),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
        ]
    }

    fn display_name(&self) -> &str {
        "Snowman 2"
    }

    // -------------------------------------------------------------------
    // Variable tracking capability
    // -------------------------------------------------------------------

    fn supports_full_variable_tracking(&self) -> bool {
        true
    }

    // -------------------------------------------------------------------
    // Syntax detection (format-aware handler dispatch)
    // -------------------------------------------------------------------

    fn find_macro_at_position(
        &self,
        line: &str,
        byte_pos: usize,
    ) -> Option<crate::plugin::MacroAtPosition> {
        use crate::plugin::MacroAtPosition;

        // Snowman uses ERB-style templates:
        //   <%= expression %>  — inline expression (output)
        //   <% code %>         — code block (no output)
        // Detect these at position.

        // Check for <%= ... %> (expression)
        let re_expr = regex::Regex::new(r"<%=\s*").unwrap();
        if let Some(m) = re_expr.find(line) {
            let start = m.start();
            if byte_pos >= start {
                // Find closing %>
                if let Some(end_offset) = line[start..].find("%>") {
                    let end = start + end_offset + 2;
                    if byte_pos <= end {
                        let content = &line[m.end()..start + end_offset];
                        let name = content.split_whitespace().next().unwrap_or(content);
                        let name_start = m.end();
                        let name_end = name_start + name.len();
                        return Some(MacroAtPosition {
                            name: name.to_string(),
                            full_range: start..end,
                            name_range: name_start..name_end,
                            is_unclosed: false,
                        });
                    }
                } else if byte_pos >= start {
                    // Unclosed expression
                    let content = &line[m.end()..];
                    let name = content.split_whitespace().next().unwrap_or(content);
                    let name_start = m.end();
                    let name_end = name_start + name.len();
                    return Some(MacroAtPosition {
                        name: name.to_string(),
                        full_range: start..line.len(),
                        name_range: name_start..name_end,
                        is_unclosed: true,
                    });
                }
            }
        }

        // Check for <% ... %> (code block)
        let re_code = regex::Regex::new(r"<%(?!=)\s*").unwrap();
        if let Some(m) = re_code.find(line) {
            let start = m.start();
            if byte_pos >= start {
                if let Some(end_offset) = line[start..].find("%>") {
                    let end = start + end_offset + 2;
                    if byte_pos <= end {
                        let content = &line[m.end()..start + end_offset];
                        let name = content.split_whitespace().next().unwrap_or("script");
                        let name_start = m.end();
                        let name_end = name_start + name.len();
                        return Some(MacroAtPosition {
                            name: name.to_string(),
                            full_range: start..end,
                            name_range: name_start..name_end,
                            is_unclosed: false,
                        });
                    }
                } else if byte_pos >= start {
                    let content = &line[m.end()..];
                    let name = content.split_whitespace().next().unwrap_or("script");
                    let name_start = m.end();
                    let name_end = name_start + name.len();
                    return Some(MacroAtPosition {
                        name: name.to_string(),
                        full_range: start..line.len(),
                        name_range: name_start..name_end,
                        is_unclosed: true,
                    });
                }
            }
        }

        None
    }

    fn scan_line_for_macro_events(
        &self,
        _line: &str,
        _line_idx: u32,
    ) -> Vec<crate::plugin::MacroBlockEvent> {
        // Snowman uses ERB-style blocks (<% %>) which are inline — they
        // don't have open/close pairs like SugarCube or Chapbook blocks.
        Vec::new()
    }

    fn format_macro_label(&self, name: &str) -> String {
        format!("<%= {} %>", name)
    }

    fn format_macro_signature_label(&self, name: &str, params: &str) -> String {
        if params.is_empty() {
            format!("<%= {} %>", name)
        } else {
            format!("<%= {}({}) %>", name, params)
        }
    }

    fn format_close_macro_label(&self, _name: &str) -> String {
        String::new() // Snowman has no close tags
    }

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        if has_body {
            format!("<% {} %>\n$2\n<% }} %>", name)
        } else {
            format!("<%= {} %>", name)
        }
    }

    fn detect_close_tag_context(&self, _before_cursor: &str) -> Option<String> {
        None // Snowman has no close tags
    }

    fn has_block_macros_with_close_tags(&self) -> bool {
        false // Snowman uses ERB-style inline blocks
    }

    fn variable_assignment_snippet(&self, var_name: &str, value: &str) -> Option<String> {
        // Snowman uses ERB-style: <% s.var = value %>
        // Strip the $ sigil if present (Snowman uses bare names in s.*)
        let bare = var_name.trim_start_matches('$');
        Some(format!("<% s.{} = {} %>", bare, value))
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

    // -----------------------------------------------------------------------
    // Incremental re-parse (parse_passage) with tag-matched passages
    // -----------------------------------------------------------------------

    #[test]
    fn parse_passage_tagged_header() {
        let plugin = SnowmanPlugin::new();
        let result = plugin.parse_passage(
            "TopBar",
            &["header".to_string()],
            "Header content\n",
        );
        let p = result.expect("tagged [header] passage should be classified as special");
        assert!(p.is_special, "Passage tagged 'header' should be special via classify_passage");
        assert!(p.special_def.is_some(), "special_def should be populated for tagged [header]");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::Chrome));
    }

    #[test]
    fn parse_passage_tagged_footer() {
        let plugin = SnowmanPlugin::new();
        let result = plugin.parse_passage(
            "BottomBar",
            &["footer".to_string()],
            "Footer content\n",
        );
        let p = result.expect("tagged [footer] passage should be classified as special");
        assert!(p.is_special, "Passage tagged 'footer' should be special via classify_passage");
        assert!(p.special_def.is_some(), "special_def should be populated for tagged [footer]");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::Chrome));
    }

    #[test]
    fn parse_passage_name_matched_passage_header() {
        let plugin = SnowmanPlugin::new();
        let result = plugin.parse_passage(
            "PassageHeader",
            &[],
            "Header content\n",
        );
        let p = result.expect("PassageHeader (name-matched) should be classified as special");
        assert!(p.is_special, "PassageHeader should be special via name matching");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::Chrome));
    }
}
