//! Chapbook Format Plugin
//!
//! Chapbook is a story format designed for simplicity, using a markdown-like
//! syntax with modifier blocks and a JavaScript-based state model.
//!
//! ## Supported Features
//!
//! - Passage header parsing with byte-offset-accurate splitting
//! - Link extraction: `[[Target]]`, `[[Display->Target]]`, `[[Display|Target]]`
//! - `[javascript]` block parsing with `state.variable` extraction
//! - `[modify]` block parsing with key-value variable writes
//! - `{{expression}}` insert parsing with variable read extraction
//! - Chapbook-specific diagnostics (unclosed blocks, links, expressions)
//! - Full block model: Text, Macro (javascript/modify), Expression (inserts)
//!
//! ## Variable Tracking
//!
//! Chapbook uses `state.variableName` inside `[javascript]` blocks and
//! `{{state.variableName}}` inside inserts for state management. Variable
//! tracking is supported for these patterns. The architecture marks Chapbook
//! variable tracking as "Unsupported" for cross-passage dataflow, but we can
//! still extract per-passage variable operations for IDE features like
//! highlighting and completion.

use knot_core::passage::{
    Block, Link, Passage, SpecialPassageBehavior, SpecialPassageDef, StoryFormat, VarKind, VarOp,
};
use regex::Regex;
use url::Url;

use crate::plugin::{
    FormatDiagnostic, FormatDiagnosticSeverity, FormatPlugin, ParseResult, SemanticToken,
    SemanticTokenModifier, SemanticTokenType,
};

// ---------------------------------------------------------------------------
// Parsed header
// ---------------------------------------------------------------------------

/// The result of parsing a single passage header line with byte-offset tracking.
struct ParsedHeader {
    name: String,
    tags: Vec<String>,
    /// Byte offset where the header line starts in the source text.
    header_start: usize,
    /// Byte length of the header line (not including trailing newline).
    header_len: usize,
}

// ---------------------------------------------------------------------------
// Template segment
// ---------------------------------------------------------------------------

/// A segment of a Chapbook passage body produced by template parsing.
enum TemplateSegment {
    /// Plain text content.
    Text { start: usize, end: usize },
    /// A `[javascript]...[/javascript]` block.
    Javascript { start: usize, end: usize, content_start: usize, content_end: usize },
    /// A `[modify]...[/modify]` block.
    Modify { start: usize, end: usize, content_start: usize, content_end: usize },
    /// A `{{expression}}` insert.
    Insert { start: usize, end: usize, expr_start: usize, expr_end: usize },
    /// An unclosed `[javascript]` block.
    UnclosedJavascript { start: usize, end: usize },
    /// An unclosed `[modify]` block.
    UnclosedModify { start: usize, end: usize },
    /// An unclosed `{{` insert.
    UnclosedInsert { start: usize, end: usize },
}

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// Chapbook format plugin.
pub struct ChapbookPlugin {
    /// Regex for simple links: `[[Target]]`
    re_link_simple: Regex,
    /// Regex for arrow links: `[[Display->Target]]`
    re_link_arrow: Regex,
    /// Regex for pipe links: `[[Display|Target]]`
    re_link_pipe: Regex,
    /// Regex for passage headers: `:: Name [tags]`
    re_header: Regex,
    /// Regex for state variable writes: `state.varName =`
    re_state_write: Regex,
    /// Regex for state variable reads: `state.varName`
    re_state_read: Regex,
    /// Regex for `[modify]` key-value lines: `key: value`
    re_modify_kv: Regex,
}

impl Default for ChapbookPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ChapbookPlugin {
    /// Create a new Chapbook plugin instance.
    pub fn new() -> Self {
        Self {
            re_link_simple: Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap(),
            re_link_arrow: Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap(),
            re_link_pipe: Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap(),
            re_header: Regex::new(r"^::\s*(.+?)(?:\s+\[([^\]]*)\])?\s*$").unwrap(),
            // state.varName = value — write
            re_state_write: Regex::new(r"\bstate\.([A-Za-z_][A-Za-z0-9_]*)\s*=").unwrap(),
            // state.varName — read (also matches writes; filtered in code)
            re_state_read: Regex::new(r"\bstate\.([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            // key: value (inside [modify] blocks)
            re_modify_kv: Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:").unwrap(),
        }
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passage headers + bodies
    // -----------------------------------------------------------------------

    /// Parse passage headers from the full source text using byte-offset tracking.
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(ParsedHeader, &'a str)> {
        let mut results: Vec<(ParsedHeader, &str)> = Vec::new();
        let mut header_spans: Vec<(usize, usize)> = Vec::new();
        let mut byte_offset = 0;

        // Collect header line positions with accurate byte offsets.
        for line in text.lines() {
            let line_start = byte_offset;
            let line_end = line_start + line.len();

            if self.re_header.is_match(line) {
                header_spans.push((line_start, line_end));
            }

            // Advance past line + newline character.
            byte_offset = line_end + 1;
        }

        // Build passage bodies from header spans.
        for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
            let header_line = &text[header_start..header_end];
            let parsed = self.parse_header_line(header_line, header_start);

            // Body starts after the header line (skip trailing newline).
            let body_start = header_end + 1; // +1 for the newline after the header
            let body_end = if i + 1 < header_spans.len() {
                header_spans[i + 1].0
            } else {
                text.len()
            };
            let body_text = text.get(body_start.min(text.len())..body_end.min(text.len())).unwrap_or("");

            if let Some(hdr) = parsed {
                results.push((hdr, body_text));
            }
        }

        results
    }

    /// Parse a single `:: Name [tags]` header line.
    fn parse_header_line(&self, line: &str, offset: usize) -> Option<ParsedHeader> {
        let rest = line.strip_prefix("::")?;
        let rest = rest.trim_start();

        let (name, tags) = if let Some(bracket_start) = rest.rfind('[') {
            if rest.ends_with(']') {
                let name_part = rest[..bracket_start].trim();
                let tag_part = &rest[bracket_start + 1..rest.len() - 1];
                let tags = tag_part
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                (name_part.to_string(), tags)
            } else {
                (rest.trim().to_string(), Vec::new())
            }
        } else {
            (rest.trim().to_string(), Vec::new())
        };

        if name.is_empty() {
            return None;
        }

        Some(ParsedHeader {
            name,
            tags,
            header_start: offset,
            header_len: line.len(),
        })
    }

    // -----------------------------------------------------------------------
    // Pass 2: Body analysis
    // -----------------------------------------------------------------------

    /// Extract links from a passage body.
    fn extract_links(&self, body: &str, body_offset: usize) -> Vec<Link> {
        let mut links = Vec::new();

        // Arrow-style links: [[Display->Target]]
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

        // Pipe-style links: [[Display|Target]]
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

        // Simple links: [[Target]] (skip overlaps with arrow/pipe).
        let known_spans: Vec<std::ops::Range<usize>> = self
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
            let overlaps = known_spans.iter().any(|s| span.start >= s.start && span.end <= s.end);
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

    /// Parse the body text into template segments: [javascript], [modify], {{inserts}}.
    fn parse_template_segments(&self, body: &str) -> Vec<TemplateSegment> {
        let mut segments = Vec::new();
        let bytes = body.as_bytes();
        let len = bytes.len();
        let mut pos = 0;

        while pos < len {
            // Check for [javascript] block
            if body[pos..].starts_with("[javascript]") {
                let block_start = pos;
                let content_start = pos + "[javascript]".len();
                if let Some(close_pos) = body[content_start..].find("[/javascript]") {
                    let content_end = content_start + close_pos;
                    let block_end = content_end + "[/javascript]".len();
                    segments.push(TemplateSegment::Javascript {
                        start: block_start,
                        end: block_end,
                        content_start,
                        content_end,
                    });
                    pos = block_end;
                    continue;
                } else {
                    // Unclosed [javascript] block
                    segments.push(TemplateSegment::UnclosedJavascript {
                        start: block_start,
                        end: len,
                    });
                    pos = len;
                    continue;
                }
            }

            // Check for [modify] block
            if body[pos..].starts_with("[modify]") {
                let block_start = pos;
                let content_start = pos + "[modify]".len();
                if let Some(close_pos) = body[content_start..].find("[/modify]") {
                    let content_end = content_start + close_pos;
                    let block_end = content_end + "[/modify]".len();
                    segments.push(TemplateSegment::Modify {
                        start: block_start,
                        end: block_end,
                        content_start,
                        content_end,
                    });
                    pos = block_end;
                    continue;
                } else {
                    // Unclosed [modify] block
                    segments.push(TemplateSegment::UnclosedModify {
                        start: block_start,
                        end: len,
                    });
                    pos = len;
                    continue;
                }
            }

            // Check for {{expression}} insert
            if pos + 1 < len && bytes[pos] == b'{' && bytes[pos + 1] == b'{' {
                let insert_start = pos;
                let search_from = pos + 2;
                if let Some(close_pos) = body[search_from..].find("}}") {
                    let expr_start = search_from;
                    let expr_end = search_from + close_pos;
                    let insert_end = expr_end + 2;
                    segments.push(TemplateSegment::Insert {
                        start: insert_start,
                        end: insert_end,
                        expr_start,
                        expr_end,
                    });
                    pos = insert_end;
                    continue;
                } else {
                    // Unclosed {{ insert
                    segments.push(TemplateSegment::UnclosedInsert {
                        start: insert_start,
                        end: len,
                    });
                    pos = len;
                    continue;
                }
            }

            // Plain text — advance to the next special token or end
            let next_special = self.find_next_special(body, pos);
            let text_end = next_special.unwrap_or(len);
            if text_end > pos {
                segments.push(TemplateSegment::Text {
                    start: pos,
                    end: text_end,
                });
            }
            pos = text_end;
        }

        segments
    }

    /// Find the position of the next special token in the body starting from `pos`.
    fn find_next_special(&self, body: &str, pos: usize) -> Option<usize> {
        let mut earliest: Option<usize> = None;

        for pattern in &["[javascript]", "[modify]", "{{"] {
            if let Some(idx) = body[pos..].find(pattern) {
                let abs = pos + idx;
                earliest = Some(earliest.map_or(abs, |e| e.min(abs)));
            }
        }

        earliest
    }

    /// Extract variable operations from [javascript] block content.
    fn extract_js_vars(&self, content: &str, content_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();
        let mut write_spans: Vec<std::ops::Range<usize>> = Vec::new();

        // Detect writes: state.varName = value
        for caps in self.re_state_write.captures_iter(content) {
            let full = caps.get(0).unwrap();
            let var_name = format!("state.{}", caps.get(1).unwrap().as_str());
            let var_start = content_offset + full.start();
            let var_end = var_start + var_name.len();
            vars.push(VarOp {
                name: var_name,
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect reads: state.varName (not already a write)
        for caps in self.re_state_read.captures_iter(content) {
            let full = caps.get(0).unwrap();
            let var_start = content_offset + full.start();
            let var_end = content_offset + full.end();
            let is_write = write_spans
                .iter()
                .any(|s| var_start >= s.start && var_end <= s.end);
            if !is_write {
                vars.push(VarOp {
                    name: full.as_str().to_string(),
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: false,
                });
            }
        }

        vars
    }

    /// Extract variable operations from [modify] block content.
    ///
    /// [modify] blocks contain key-value pairs like:
    /// ```chapbook
    /// [modify]
    /// gold: 10
    /// name: Alice
    /// [/modify]
    /// ```
    ///
    /// Each key becomes a variable write with the name `modify.keyName`.
    fn extract_modify_vars(&self, content: &str, content_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();
        let mut line_offset = 0;

        for line in content.lines() {
            if let Some(caps) = self.re_modify_kv.captures(line) {
                let key = caps.get(1).unwrap().as_str();
                let var_name = format!("modify.{}", key);
                // Find the key position within the line
                if let Some(key_pos) = line.find(key) {
                    let var_start = content_offset + line_offset + key_pos;
                    let var_end = var_start + key.len();
                    vars.push(VarOp {
                        name: var_name,
                        kind: VarKind::Init,
                        span: var_start..var_end,
                        is_temporary: false,
                    });
                }
            }
            line_offset += line.len() + 1; // +1 for newline
        }

        vars
    }

    /// Extract variable reads from `{{expression}}` inserts.
    fn extract_insert_vars(&self, expr: &str, expr_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();

        for caps in self.re_state_read.captures_iter(expr) {
            let full = caps.get(0).unwrap();
            let var_start = expr_offset + full.start();
            let var_end = expr_offset + full.end();
            vars.push(VarOp {
                name: full.as_str().to_string(),
                kind: VarKind::Read,
                span: var_start..var_end,
                is_temporary: false,
            });
        }

        vars
    }

    /// Build blocks from template segments.
    fn build_blocks(&self, body: &str, body_offset: usize, segments: &[TemplateSegment]) -> Vec<Block> {
        let mut blocks = Vec::new();

        for seg in segments {
            match seg {
                TemplateSegment::Text { start, end } => {
                    let content = body[*start..*end].to_string();
                    if !content.trim().is_empty() {
                        blocks.push(Block::Text {
                            content,
                            span: body_offset + *start..body_offset + *end,
                        });
                    }
                }
                TemplateSegment::Javascript { start, end, content_start, content_end } => {
                    let code = body[*content_start..*content_end].to_string();
                    blocks.push(Block::Macro {
                        name: "javascript".to_string(),
                        args: code,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
                TemplateSegment::Modify { start, end, content_start, content_end } => {
                    let content = body[*content_start..*content_end].to_string();
                    blocks.push(Block::Macro {
                        name: "modify".to_string(),
                        args: content,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
                TemplateSegment::Insert { start, end, expr_start, expr_end } => {
                    let expr = body[*expr_start..*expr_end].to_string();
                    blocks.push(Block::Expression {
                        content: expr,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
                TemplateSegment::UnclosedJavascript { start, end } => {
                    let content = body[*start..*end].to_string();
                    blocks.push(Block::Incomplete {
                        content,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
                TemplateSegment::UnclosedModify { start, end } => {
                    let content = body[*start..*end].to_string();
                    blocks.push(Block::Incomplete {
                        content,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
                TemplateSegment::UnclosedInsert { start, end } => {
                    let content = body[*start..*end].to_string();
                    blocks.push(Block::Incomplete {
                        content,
                        span: body_offset + *start..body_offset + *end,
                    });
                }
            }
        }

        blocks
    }

    /// Generate semantic tokens for a passage body.
    fn body_tokens(&self, body: &str, body_offset: usize) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();
        let segments = self.parse_template_segments(body);

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

        // Variable tokens from [javascript] and {{insert}} blocks.
        let mut write_spans: Vec<std::ops::Range<usize>> = Vec::new();

        for seg in &segments {
            match seg {
                TemplateSegment::Javascript { content_start, content_end, .. } => {
                    let content = &body[*content_start..*content_end];
                    let content_offset = body_offset + *content_start;

                    // Write tokens
                    for caps in self.re_state_write.captures_iter(content) {
                        let full = caps.get(0).unwrap();
                        let var_name = format!("state.{}", caps.get(1).unwrap().as_str());
                        let var_start = content_offset + full.start();
                        let var_end = var_start + var_name.len();
                        tokens.push(SemanticToken {
                            start: var_start,
                            length: var_name.len(),
                            token_type: SemanticTokenType::Variable,
                            modifier: Some(SemanticTokenModifier::Definition),
                        });
                        write_spans.push(var_start..var_end);
                    }

                    // Read tokens
                    for caps in self.re_state_read.captures_iter(content) {
                        let full = caps.get(0).unwrap();
                        let var_start = content_offset + full.start();
                        let var_end = content_offset + full.end();
                        let is_write = write_spans
                            .iter()
                            .any(|s| var_start >= s.start && var_end <= s.end);
                        if !is_write {
                            tokens.push(SemanticToken {
                                start: var_start,
                                length: full.end() - full.start(),
                                token_type: SemanticTokenType::Variable,
                                modifier: None,
                            });
                        }
                    }

                    // Macro token for the [javascript] block
                    tokens.push(SemanticToken {
                        start: body_offset + *content_start - "[javascript]".len(),
                        length: "[javascript]".len(),
                        token_type: SemanticTokenType::Keyword,
                        modifier: None,
                    });
                }
                TemplateSegment::Modify { start, content_start: _, .. } => {
                    // Macro token for the [modify] block
                    tokens.push(SemanticToken {
                        start: body_offset + *start,
                        length: "[modify]".len(),
                        token_type: SemanticTokenType::Keyword,
                        modifier: None,
                    });
                }
                TemplateSegment::Insert { start, end, .. } => {
                    tokens.push(SemanticToken {
                        start: body_offset + *start,
                        length: end - start,
                        token_type: SemanticTokenType::Variable,
                        modifier: None,
                    });
                }
                _ => {}
            }
        }

        tokens
    }

    /// Generate format-specific diagnostics for a passage body.
    fn validate(&self, body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();
        let segments = self.parse_template_segments(body);

        for seg in &segments {
            match seg {
                TemplateSegment::UnclosedJavascript { start, .. } => {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + *start..body_offset + *start + "[javascript]".len(),
                        message: "Unclosed [javascript] block — missing [/javascript]".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "cb-unclosed-javascript".into(),
                    });
                }
                TemplateSegment::UnclosedModify { start, .. } => {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + *start..body_offset + *start + "[modify]".len(),
                        message: "Unclosed [modify] block — missing [/modify]".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "cb-unclosed-modify".into(),
                    });
                }
                TemplateSegment::UnclosedInsert { start, .. } => {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + *start..body_offset + *start + 2,
                        message: "Unclosed {{ insert — missing }}".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "cb-unclosed-insert".into(),
                    });
                }
                _ => {}
            }
        }

        // Check for unclosed link syntax: [[ without ]]
        let bytes = body.as_bytes();
        let mut link_depth = 0i32;
        let mut link_open: Option<usize> = None;
        let mut i = 0;
        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[' {
                if link_depth == 0 {
                    link_open = Some(i);
                }
                link_depth += 1;
                i += 2;
                continue;
            }
            if i + 1 < bytes.len() && bytes[i] == b']' && bytes[i + 1] == b']' {
                link_depth -= 1;
                if link_depth < 0 {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + i..body_offset + i + 2,
                        message: "Unexpected link closing `]]` without matching `[[`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "cb-broken-link".into(),
                    });
                    link_depth = 0;
                }
                i += 2;
                continue;
            }
            i += 1;
        }

        if link_depth > 0
            && let Some(pos) = link_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed link `[[` — missing `]]`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "cb-broken-link".into(),
                });
            }

        diagnostics
    }

    /// Chapbook special passage definitions.
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
                name: "look".into(),
                behavior: SpecialPassageBehavior::Custom("ChapbookLook".into()),
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

impl FormatPlugin for ChapbookPlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::Chapbook
    }

    fn parse(&self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        let raw_passages = self.split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_start + header.header_len + 1; // +1 for newline after header

            let special_defs = Self::special_passage_defs();
            let special_def = special_defs.iter().find(|d| d.name == header.name).cloned();

            // Also check for tagged header/footer passages
            let special_def = special_def.or_else(|| {
                if header.tags.contains(&"header".to_string()) {
                    Some(SpecialPassageDef {
                        name: header.name.clone(),
                        behavior: SpecialPassageBehavior::Chrome,
                        contributes_variables: false,
                        participates_in_graph: false,
                        execution_priority: Some(90),
                    })
                } else if header.tags.contains(&"footer".to_string()) {
                    Some(SpecialPassageDef {
                        name: header.name.clone(),
                        behavior: SpecialPassageBehavior::Chrome,
                        contributes_variables: false,
                        participates_in_graph: false,
                        execution_priority: Some(110),
                    })
                } else {
                    None
                }
            });

            let mut passage = if let Some(def) = special_def {
                Passage::new_special(header.name.clone(), header.header_start..body_offset + body.len(), def)
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();

            // Extract links
            passage.links = self.extract_links(body, body_offset);

            // Extract variables from template segments
            let segments = self.parse_template_segments(body);
            let mut vars = Vec::new();

            for seg in &segments {
                match seg {
                    TemplateSegment::Javascript { content_start, content_end, .. } => {
                        let content = &body[*content_start..*content_end];
                        vars.extend(self.extract_js_vars(content, body_offset + *content_start));
                    }
                    TemplateSegment::Modify { content_start, content_end, .. } => {
                        let content = &body[*content_start..*content_end];
                        vars.extend(self.extract_modify_vars(content, body_offset + *content_start));
                    }
                    TemplateSegment::Insert { expr_start, expr_end, .. } => {
                        let expr = &body[*expr_start..*expr_end];
                        vars.extend(self.extract_insert_vars(expr, body_offset + *expr_start));
                    }
                    _ => {}
                }
            }

            passage.vars = vars;

            // Build block model from segments
            passage.body = self.build_blocks(body, body_offset, &segments);

            // Semantic tokens for header.
            tokens.push(SemanticToken {
                start: header.header_start,
                length: 2,
                token_type: SemanticTokenType::PassageHeader,
                modifier: None,
            });

            // Semantic tokens for body.
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

        // Extract variables from template segments
        let segments = self.parse_template_segments(passage_text);
        let mut vars = Vec::new();

        for seg in &segments {
            match seg {
                TemplateSegment::Javascript { content_start, content_end, .. } => {
                    let content = &passage_text[*content_start..*content_end];
                    vars.extend(self.extract_js_vars(content, *content_start));
                }
                TemplateSegment::Modify { content_start, content_end, .. } => {
                    let content = &passage_text[*content_start..*content_end];
                    vars.extend(self.extract_modify_vars(content, *content_start));
                }
                TemplateSegment::Insert { expr_start, expr_end, .. } => {
                    let expr = &passage_text[*expr_start..*expr_end];
                    vars.extend(self.extract_insert_vars(expr, *expr_start));
                }
                _ => {}
            }
        }

        passage.vars = vars;
        passage.body = self.build_blocks(passage_text, 0, &segments);

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    fn display_name(&self) -> &str {
        "Chapbook"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_passage() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nWelcome [[Cave]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Cave");
    }

    #[test]
    fn detect_special_passages() {
        let plugin = ChapbookPlugin::new();
        assert!(plugin.is_special_passage("look"));
        assert!(plugin.is_special_passage("PassageHeader"));
        assert!(plugin.is_special_passage("PassageFooter"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = ChapbookPlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");
        assert!(result.passages.is_empty());
    }

    // -----------------------------------------------------------------------
    // [javascript] block tests
    // -----------------------------------------------------------------------

    #[test]
    fn javascript_block_variable_write() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.gold = 10;\n[/javascript]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Init),
            "Should detect state.gold write"
        );
    }

    #[test]
    fn javascript_block_variable_read() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nconsole.log(state.gold);\n[/javascript]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Read),
            "Should detect state.gold read"
        );
    }

    #[test]
    fn javascript_block_write_and_read() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.gold = 10;\nconsole.log(state.gold);\n[/javascript]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn javascript_block_creates_macro_block() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.x = 1;\n[/javascript]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let blocks = &result.passages[0].body;
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Macro { name, .. } if name == "javascript")),
            "Should create a Macro block for [javascript]"
        );
    }

    #[test]
    fn unclosed_javascript_block_diagnostic() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.x = 1;\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "cb-unclosed-javascript"),
            "Should warn about unclosed [javascript] block"
        );
    }

    #[test]
    fn unclosed_javascript_block_creates_incomplete() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.x = 1;\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let blocks = &result.passages[0].body;
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Incomplete { .. })),
            "Unclosed [javascript] should produce an Incomplete block"
        );
    }

    // -----------------------------------------------------------------------
    // [modify] block tests
    // -----------------------------------------------------------------------

    #[test]
    fn modify_block_variable_write() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[modify]\ngold: 10\nname: Alice\n[/modify]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "modify.gold" && v.kind == VarKind::Init),
            "Should detect modify.gold write"
        );
        assert!(
            vars.iter().any(|v| v.name == "modify.name" && v.kind == VarKind::Init),
            "Should detect modify.name write"
        );
    }

    #[test]
    fn modify_block_creates_macro_block() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[modify]\ngold: 10\n[/modify]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let blocks = &result.passages[0].body;
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Macro { name, .. } if name == "modify")),
            "Should create a Macro block for [modify]"
        );
    }

    #[test]
    fn unclosed_modify_block_diagnostic() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[modify]\ngold: 10\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "cb-unclosed-modify"),
            "Should warn about unclosed [modify] block"
        );
    }

    // -----------------------------------------------------------------------
    // {{insert}} tests
    // -----------------------------------------------------------------------

    #[test]
    fn insert_expression_variable_read() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nYou have {{state.gold}} coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Read),
            "Should detect state.gold read from {{insert}}"
        );
    }

    #[test]
    fn insert_creates_expression_block() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nYou have {{state.gold}} coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let blocks = &result.passages[0].body;
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Expression { .. })),
            "Should create an Expression block for {{insert}}"
        );
    }

    #[test]
    fn unclosed_insert_diagnostic() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nYou have {{state.gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "cb-unclosed-insert"),
            "Should warn about unclosed {{ insert"
        );
    }

    // -----------------------------------------------------------------------
    // Link diagnostics
    // -----------------------------------------------------------------------

    #[test]
    fn unclosed_link_diagnostic() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nGo to [[Cave\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "cb-broken-link"),
            "Should warn about unclosed link"
        );
    }

    // -----------------------------------------------------------------------
    // Mixed content tests
    // -----------------------------------------------------------------------

    #[test]
    fn mixed_blocks_and_links() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\nWelcome [[Cave]].\n[javascript]\nstate.visited = true;\n[/javascript]\nYou have {{state.gold}} coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let passage = &result.passages[0];

        // Should have a link
        assert_eq!(passage.links.len(), 1);
        assert_eq!(passage.links[0].target, "Cave");

        // Should have variable operations
        assert!(passage.vars.iter().any(|v| v.name == "state.visited" && v.kind == VarKind::Init));
        assert!(passage.vars.iter().any(|v| v.name == "state.gold" && v.kind == VarKind::Read));

        // Should have mixed blocks
        let block_types: Vec<&str> = passage.body.iter().map(|b| match b {
            Block::Text { .. } => "Text",
            Block::Macro { name, .. } => name.as_str(),
            Block::Expression { .. } => "Expression",
            Block::Incomplete { .. } => "Incomplete",
            Block::Heading { .. } => "Heading",
        }).collect();
        assert!(block_types.contains(&"Text"), "Should have Text blocks");
        assert!(block_types.contains(&"javascript"), "Should have javascript Macro block");
        assert!(block_types.contains(&"Expression"), "Should have Expression block");
    }

    #[test]
    fn empty_javascript_block() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\n[/javascript]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert!(result.passages[0].vars.is_empty(), "Empty [javascript] block should have no variables");
    }

    #[test]
    fn passage_with_tags() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    #[test]
    fn multiple_passages_with_javascript() {
        let plugin = ChapbookPlugin::new();
        let src = ":: Start\n[javascript]\nstate.x = 1;\n[/javascript]\n:: Forest\n{{state.x}} trees.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
        assert!(result.passages[0].vars.iter().any(|v| v.name == "state.x" && v.kind == VarKind::Init));
        assert!(result.passages[1].vars.iter().any(|v| v.name == "state.x" && v.kind == VarKind::Read));
    }

    #[test]
    fn tagged_header_passage() {
        let plugin = ChapbookPlugin::new();
        let src = ":: MyHeader [header]\nThis is a header passage.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert!(result.passages[0].is_special, "Tagged [header] passage should be special");
    }

    #[test]
    fn tagged_footer_passage() {
        let plugin = ChapbookPlugin::new();
        let src = ":: MyFooter [footer]\nThis is a footer passage.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert!(result.passages[0].is_special, "Tagged [footer] passage should be special");
    }

    #[test]
    fn split_passages_byte_offset_tracking() {
        let plugin = ChapbookPlugin::new();
        // Two identical header lines to test that text.find doesn't cause issues
        let src = ":: Room\nHello\n:: Room\nWorld\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        // With the buggy approach, this would produce only 1 passage.
        // With byte-offset tracking, we get 2.
        assert_eq!(result.passages.len(), 2, "Should correctly split duplicate passage headers");
        assert_eq!(result.passages[1].name, "Room");
    }
}
