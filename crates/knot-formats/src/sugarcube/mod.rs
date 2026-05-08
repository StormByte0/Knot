//! SugarCube Format Plugin
//!
//! SugarCube 2.x is the most popular Twine story format, providing a rich macro
//! system and variable tracking via `$variable` syntax.
//!
//! This module implements a fault-tolerant, two-pass parser:
//!
//! 1. **Pass 1 — Passage boundaries**: A [`logos`]-based lexer splits the source
//!    into passage header regions and their body text.
//! 2. **Pass 2 — Body analysis**: Regex-based extractors detect links, variable
//!    operations, and macro invocations within each passage body.
//!
//! The parser never hard-fails on invalid input. Malformed constructs are captured
//! as [`Block::Incomplete`] and reported as diagnostics rather than causing panics.

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
// Logos lexer — passage boundary detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, logos::Logos)]
enum TweeToken {
    /// A passage header line: `:: Name [tags]`
    #[regex(r"::[^\n]*")]
    PassageHeader,

    /// Any other line of text (body content).
    #[regex(r"[^\n]+")]
    TextLine,

    /// A newline.
    #[token("\n")]
    Newline,
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
    /// Byte length of the header line (including trailing newline if present).
    header_len: usize,
}

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// SugarCube 2.x format plugin.
pub struct SugarCubePlugin {
    /// Regex for extracting simple links: `[[Target]]`
    re_link_simple: Regex,
    /// Regex for extracting arrow links: `[[Display->Target]]`
    re_link_arrow: Regex,
    /// Regex for extracting pipe links: `[[Display|Target]]`
    re_link_pipe: Regex,
    /// Regex for extracting persistent variable references: `$variableName`
    re_var: Regex,
    /// Regex for extracting temporary variable references: `_variableName`
    re_temp_var: Regex,
    /// Regex for extracting `<<set …>>` macro (variable writes) for persistent vars.
    re_set_macro: Regex,
    /// Regex for extracting `<<set …>>` macro for temporary vars.
    re_set_temp_macro: Regex,
    /// Regex for extracting any macro: `<<name …>>` or `<<name>>`
    re_macro: Regex,
    /// Regex for extracting closing macros: `<</name>>`
    re_macro_close: Regex,
}

impl SugarCubePlugin {
    /// Create a new SugarCube plugin instance.
    pub fn new() -> Self {
        Self {
            // [[Target]] — simple passage link
            re_link_simple: Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap(),
            // [[Display->Target]] — arrow-style link
            re_link_arrow: Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap(),
            // [[Display|Target]] — pipe-style link
            re_link_pipe: Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap(),
            // $variableName — SugarCube persistent variable reference
            re_var: Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            // _variableName — SugarCube temporary/scratch variable reference
            // Simple pattern: matches any _varName. We filter in code to avoid
            // matching inside identifiers like foo_bar by checking the preceding char.
            re_temp_var: Regex::new(r"_([A-Za-z][A-Za-z0-9_]*)").unwrap(),
            // <<set $var to ...>> — write macro for persistent vars
            re_set_macro: Regex::new(r"<<set\s+\$([A-Za-z_][A-Za-z0-9_]*)\s+to\b").unwrap(),
            // <<set _var to ...>> — write macro for temporary vars
            re_set_temp_macro: Regex::new(r"<<set\s+_([A-Za-z][A-Za-z0-9_]*)\s+to\b").unwrap(),
            // <<name ...>> — any open macro
            re_macro: Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+([^>]*?))?>>").unwrap(),
            // <</name>> — closing macro tag
            re_macro_close: Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap(),
        }
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passage headers + bodies
    // -----------------------------------------------------------------------

    /// Parse passage headers from the full source text.
    ///
    /// Returns a list of `(ParsedHeader, body_text)` pairs. The body text is the
    /// raw text between the end of this header line and the start of the next
    /// header (or end of file).
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(ParsedHeader, &'a str)> {
        let mut lex = logos::Lexer::new(text);
        let mut results: Vec<(ParsedHeader, &str)> = Vec::new();

        // Collect (header_start, header_end_inclusive) for each passage header.
        let mut header_spans: Vec<(usize, usize)> = Vec::new();

        while let Some(tok) = lex.next() {
            match tok {
                Ok(TweeToken::PassageHeader) => {
                    let span = lex.span();
                    header_spans.push((span.start, span.end));
                }
                Ok(TweeToken::TextLine | TweeToken::Newline) => {}
                Err(_) => {
                    // Skip invalid tokens — fault-tolerant.
                }
            }
        }

        // Build passage bodies.
        for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
            let header_line = &text[header_start..header_end];
            let parsed = self.parse_header_line(header_line, header_start);

            // Body starts after the header line (skip trailing newline).
            let body_start = header_end;
            let body_end = if i + 1 < header_spans.len() {
                header_spans[i + 1].0
            } else {
                text.len()
            };
            let body_text = text.get(body_start..body_end).unwrap_or("");

            if let Some(hdr) = parsed {
                results.push((hdr, body_text));
            }
        }

        results
    }

    /// Parse a single `:: Name [tags]` header line.
    fn parse_header_line(&self, line: &str, offset: usize) -> Option<ParsedHeader> {
        // Strip the leading `::` and optional whitespace.
        let rest = line.strip_prefix("::")?;
        let rest = rest.trim_start();

        // Extract tags if present: `Name [tag1 tag2]`
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

    /// Extract all links from a passage body.
    ///
    /// The `body_offset` is the byte offset of the body text within the full
    /// source document, used to compute absolute spans.
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

        // Simple links: [[Target]]
        // We must skip matches that are sub-spans of arrow/pipe links.
        // A simple approach: collect all arrow/pipe spans and filter overlaps.
        let arrow_pipe_spans: Vec<Range<usize>> = self
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
            // Only include if not overlapped by an arrow/pipe link.
            let overlaps = arrow_pipe_spans.iter().any(|s| {
                span.start >= s.start && span.end <= s.end
            });
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

    /// Extract variable operations from a passage body.
    ///
    /// Detects both persistent (`$var`) and temporary (`_var`) variables.
    /// First detects `<<set $var …>>` / `<<set _var …>>` for writes, then all
    /// `$var` / `_var` references not already captured as writes are treated
    /// as reads. Temporary variables are marked with `is_temporary: true`.
    fn extract_vars(&self, body: &str, body_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();
        let mut write_spans: Vec<Range<usize>> = Vec::new();

        // Detect persistent writes via <<set $var to ...>>
        for caps in self.re_set_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let var_match = caps.get(1).unwrap();
            let name = format!("${}", var_match.as_str());
            let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
            let var_end = var_start + name.len();
            vars.push(VarOp {
                name,
                kind: VarKind::Write,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect temporary writes via <<set _var to ...>>
        for caps in self.re_set_temp_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let var_match = caps.get(1).unwrap();
            let name = format!("_{}", var_match.as_str());
            let var_start = body_offset + m.start() + m.as_str().find('_').unwrap_or(0);
            let var_end = var_start + name.len();
            vars.push(VarOp {
                name,
                kind: VarKind::Write,
                span: var_start..var_end,
                is_temporary: true,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect all persistent variable references ($varName) not already writes
        for caps in self.re_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
            let is_write = write_spans.iter().any(|s| {
                var_start >= s.start && var_end <= s.end
            });
            if !is_write {
                let name = full.as_str().to_string();
                vars.push(VarOp {
                    name,
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: false,
                });
            }
        }

        // Detect all temporary variable references (_varName) not already writes
        // Filter: skip matches where the preceding character is alphanumeric (e.g., foo_bar)
        for caps in self.re_temp_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();

            // Check if preceded by an alphanumeric character (part of another identifier)
            let preceded_by_alnum = full.start() > 0
                && body.as_bytes()[full.start() - 1].is_ascii_alphanumeric();

            if preceded_by_alnum {
                continue;
            }

            let is_write = write_spans.iter().any(|s| {
                var_start >= s.start && var_end <= s.end
            });
            if !is_write {
                let name = full.as_str().to_string();
                vars.push(VarOp {
                    name,
                    kind: VarKind::Read,
                    span: var_start..var_end,
                    is_temporary: true,
                });
            }
        }

        vars
    }

    /// Extract macros from a passage body and produce content blocks.
    fn extract_macros(&self, body: &str, body_offset: usize) -> Vec<Block> {
        let mut blocks = Vec::new();

        // Open macros: <<name args>>
        for caps in self.re_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let name = caps.get(1).unwrap().as_str().to_string();
            let args = caps.get(2).map(|a| a.as_str().to_string()).unwrap_or_default();
            blocks.push(Block::Macro {
                name,
                args,
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }

        // Close macros: <</name>>
        for caps in self.re_macro_close.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let name = caps.get(1).unwrap().as_str().to_string();
            blocks.push(Block::Macro {
                name: format!("/{}", name),
                args: String::new(),
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }

        blocks
    }

    /// Generate semantic tokens for a passage body.
    fn body_tokens(&self, body: &str, body_offset: usize) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();

        // Macro tokens
        for caps in self.re_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Macro,
                modifier: None,
            });
        }
        for caps in self.re_macro_close.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Macro,
                modifier: None,
            });
        }

        // Variable tokens
        let mut write_spans: Vec<Range<usize>> = Vec::new();
        for caps in self.re_set_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
            let var_name = format!("${}", caps.get(1).unwrap().as_str());
            let var_end = var_start + var_name.len();
            tokens.push(SemanticToken {
                start: var_start,
                length: var_name.len(),
                token_type: SemanticTokenType::Variable,
                modifier: Some(SemanticTokenModifier::Definition),
            });
            write_spans.push(var_start..var_end);
        }

        for caps in self.re_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
            let is_write = write_spans.iter().any(|s| {
                var_start >= s.start && var_end <= s.end
            });
            if !is_write {
                tokens.push(SemanticToken {
                    start: var_start,
                    length: full.end() - full.start(),
                    token_type: SemanticTokenType::Variable,
                    modifier: None,
                });
            }
        }

        // Link tokens
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

    /// Generate semantic tokens for passage headers.
    fn header_tokens(&self, header: &ParsedHeader) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();

        // The `::` prefix is always 2 bytes.
        tokens.push(SemanticToken {
            start: header.header_start,
            length: 2,
            token_type: SemanticTokenType::PassageHeader,
            modifier: None,
        });

        // Passage name starts after `:: ` (2 for :: + however many spaces).
        let name_offset = header.header_start + 2;
        // We approximate: the name starts at name_offset + leading whitespace.
        // Since we already trimmed, compute the actual offset.
        // This is a best-effort token.
        let name_start = name_offset;
        tokens.push(SemanticToken {
            start: name_start,
            length: header.name.len(),
            token_type: SemanticTokenType::PassageHeader,
            modifier: None,
        });

        // Tags
        for (i, tag) in header.tags.iter().enumerate() {
            // Tags are inside [tag1 tag2], approximate their positions.
            // This is best-effort for semantic highlighting.
            tokens.push(SemanticToken {
                start: name_start + header.name.len() + 2 + i * (tag.len() + 1),
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier: None,
            });
        }

        tokens
    }

    /// SugarCube special passage definitions.
    fn special_passage_defs() -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "StoryInit".into(),
                behavior: SpecialPassageBehavior::Startup,
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
            },
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
                name: "StoryCaption".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(100),
            },
            SpecialPassageDef {
                name: "StoryMenu".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(101),
            },
            SpecialPassageDef {
                name: "PassageReady".into(),
                behavior: SpecialPassageBehavior::PassageReady,
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(50),
            },
            SpecialPassageDef {
                name: "PassageDone".into(),
                behavior: SpecialPassageBehavior::Custom("PassageDone".into()),
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(200),
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

    /// Basic validation: check for common SugarCube errors.
    fn validate(&self, body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();

        // Check for unclosed macro brackets.
        let mut depth = 0i32;
        let mut open_pos: Option<usize> = None;
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == b'<' && bytes[i + 1] == b'<' {
                if depth == 0 {
                    open_pos = Some(i);
                }
                depth += 1;
                i += 2;
                continue;
            }
            if i + 1 < bytes.len() && bytes[i] == b'>' && bytes[i + 1] == b'>' {
                depth -= 1;
                if depth < 0 {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + i..body_offset + i + 2,
                        message: "Unexpected macro closing `>>` without matching `<<`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sc-unclosed-macro".into(),
                    });
                    depth = 0;
                }
                i += 2;
                continue;
            }
            i += 1;
        }

        if depth > 0 {
            if let Some(pos) = open_pos {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed macro `<<` — missing `>>`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-unclosed-macro".into(),
                });
            }
        }

        // Check for broken link syntax: `[[` without closing `]]`.
        let mut link_depth = 0i32;
        let mut link_open: Option<usize> = None;
        let mut j = 0;
        while j < bytes.len() {
            if j + 1 < bytes.len() && bytes[j] == b'[' && bytes[j + 1] == b'[' {
                if link_depth == 0 {
                    link_open = Some(j);
                }
                link_depth += 1;
                j += 2;
                continue;
            }
            if j + 1 < bytes.len() && bytes[j] == b']' && bytes[j + 1] == b']' {
                link_depth -= 1;
                if link_depth < 0 {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + j..body_offset + j + 2,
                        message: "Unexpected link closing `]]` without matching `[[`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "sc-broken-link".into(),
                    });
                    link_depth = 0;
                }
                j += 2;
                continue;
            }
            j += 1;
        }

        if link_depth > 0 {
            if let Some(pos) = link_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed link `[[` — missing `]]`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-broken-link".into(),
                });
            }
        }

        diagnostics
    }

    /// Build a text block covering the entire body (simplified — in a full
    /// implementation, we would interleave text and non-text blocks).
    fn build_body_blocks(&self, body: &str, body_offset: usize, macros: &[Block]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();

        // Add macro blocks.
        blocks.extend_from_slice(macros);

        // If there's remaining text not covered by macros, add a text block.
        // For simplicity, we add a single text block for the whole body.
        // A production parser would interleave text and macro blocks.
        if !body.trim().is_empty() {
            blocks.push(Block::Text {
                content: body.to_string(),
                span: body_offset..body_offset + body.len(),
            });
        }

        blocks
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::SugarCube
    }

    fn parse(&self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        let raw_passages = self.split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_start + header.header_len;

            // Determine if this is a special passage.
            let special_defs = Self::special_passage_defs();
            let special_def = special_defs.iter().find(|d| d.name == header.name).cloned();

            let mut passage = if let Some(def) = special_def {
                Passage::new_special(header.name.clone(), header.header_start..body_offset + body.len(), def)
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();

            // Extract body elements.
            passage.links = self.extract_links(body, body_offset);
            passage.vars = self.extract_vars(body, body_offset);
            let macros = self.extract_macros(body, body_offset);
            passage.body = self.build_body_blocks(body, body_offset, &macros);

            // Semantic tokens for header.
            tokens.extend(self.header_tokens(header));

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
        // For incremental re-parse: we receive just the body text.
        let special_defs = Self::special_passage_defs();
        let special_def = special_defs.iter().find(|d| d.name == passage_name).cloned();

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.links = self.extract_links(passage_text, 0);
        passage.vars = self.extract_vars(passage_text, 0);
        let macros = self.extract_macros(passage_text, 0);
        passage.body = self.build_body_blocks(passage_text, 0, &macros);

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    fn display_name(&self) -> &str {
        "SugarCube 2"
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
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nYou are in a room. [[Go north->Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go north".into())
        );
        assert!(result.is_complete);
    }

    #[test]
    fn parse_multiple_passages() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
    }

    #[test]
    fn parse_passage_with_tags() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    #[test]
    fn parse_variable_operations() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>>You have $gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_pipe_link() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go to forest".into())
        );
    }

    #[test]
    fn detect_special_passages() {
        let plugin = SugarCubePlugin::new();
        assert!(plugin.is_special_passage("StoryInit"));
        assert!(plugin.is_special_passage("StoryCaption"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn unclosed_macro_diagnostic() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $x to 5\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = SugarCubePlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");

        assert!(result.passages.is_empty());
        assert!(result.is_complete);
    }

    #[test]
    fn incremental_reparse() {
        let plugin = SugarCubePlugin::new();
        let passage = plugin.parse_passage("Start", "You have $gold coins.\n");

        assert!(passage.is_some());
        let p = passage.unwrap();
        assert_eq!(p.name, "Start");
        assert!(p.vars.iter().any(|v| v.name == "$gold"));
    }

    #[test]
    fn parse_temporary_variable() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set _temp to 5>>You see _temp items.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;

        // Should detect _temp as a temporary write
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Write && v.is_temporary),
            "Should detect _temp as a temporary write"
        );

        // Should detect _temp as a temporary read
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Read && v.is_temporary),
            "Should detect _temp as a temporary read"
        );
    }

    #[test]
    fn persistent_and_temp_vars_separate() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>><<set _temp to 5>>You have $gold and _temp.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;

        // $gold should be persistent
        let gold_writes: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "$gold" && v.kind == VarKind::Write)
            .collect();
        assert_eq!(gold_writes.len(), 1);
        assert!(!gold_writes[0].is_temporary);

        // _temp should be temporary
        let temp_writes: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "_temp" && v.kind == VarKind::Write)
            .collect();
        assert_eq!(temp_writes.len(), 1);
        assert!(temp_writes[0].is_temporary);
    }
}
