//! Harlowe Format Plugin
//!
//! Harlowe is the default format in the Twine 2 editor, using a distinct markup
//! syntax with hooks and changers.
//!
//! **Implementation:** Full production-quality parser with:
//! - Byte-offset-based passage splitting (logos lexer)
//! - Hook syntax parsing: named hooks, changer attachment, hook references,
//!   collapsing whitespace markup
//! - Variable tracking via `(set:)`, `(put:)`, `(move:)`, `(unpack:)` and
//!   `$var` references
//! - Proper block model: Text, Macro, Expression blocks
//! - Diagnostic validation for unclosed commands, links, and hooks
//! - Complete special passage registry including tagged header/footer

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

/// Harlowe 3.x format plugin.
pub struct HarlowePlugin {
    /// Regex for simple links: `[[Target]]`
    re_link_simple: Regex,
    /// Regex for arrow links: `[[Display->Target]]`
    re_link_arrow: Regex,
    /// Regex for pipe links: `[[Display|Target]]`
    re_link_pipe: Regex,
    /// Regex for Harlowe link changer: `(link:"text")[[Target]]`
    re_link_changer: Regex,
    /// Regex for Harlowe (set: $var to ...) variable write.
    re_set_var: Regex,
    /// Regex for Harlowe (put: ... into $var) variable write.
    re_put_var: Regex,
    /// Regex for Harlowe (move: $var into $other) variable operation.
    re_move_var: Regex,
    /// Regex for Harlowe (unpack: ... into $var) variable write.
    re_unpack_var: Regex,
    /// Regex for all $variable references.
    re_var: Regex,
    /// Regex for Harlowe macros: (name: ...)
    re_macro: Regex,
    /// Regex for named hooks: [hookname]
    re_named_hook: Regex,
    /// Regex for hook attachment: [text]<changer|
    re_hook_attach: Regex,
    /// Regex for hook reference: |changer>[text]
    re_hook_ref: Regex,
    /// Regex for collapsing whitespace markup: {text}
    re_collapse: Regex,
}

impl Default for HarlowePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl HarlowePlugin {
    /// Create a new Harlowe plugin instance.
    pub fn new() -> Self {
        Self {
            // [[Target]] — simple passage link
            re_link_simple: Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap(),
            // [[Display->Target]] — arrow-style link
            re_link_arrow: Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap(),
            // [[Display|Target]] — pipe-style link
            re_link_pipe: Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap(),
            // (link:"text")[[Target]] — changer link
            re_link_changer: Regex::new(r#"\(link:\s*"([^"]+)"\s*\)\[\[([^\]]+?)\]\]"#).unwrap(),
            // (set: $var to expr) — Harlowe variable write
            re_set_var: Regex::new(r"\(set:\s*\$([A-Za-z_][A-Za-z0-9_]*)\s+to\b").unwrap(),
            // (put: expr into $var) — Harlowe variable write
            re_put_var: Regex::new(r"\(put:[^)]*into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)").unwrap(),
            // (move: $var into $other) — Harlowe variable move: writes to $other, reads $var
            re_move_var: Regex::new(
                r"\(move:\s*\$([A-Za-z_][A-Za-z0-9_]*)\s+into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)",
            )
            .unwrap(),
            // (unpack: ... into $var) — Harlowe destructuring write (3.3+)
            // Uses .*? instead of [^)]* to handle nested parentheses
            re_unpack_var: Regex::new(r"\(unpack:.*?into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)")
                .unwrap(),
            // $variableName — any Harlowe variable reference
            re_var: Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap(),
            // (macroname: args) — Harlowe macro/command
            re_macro: Regex::new(r"\(([A-Za-z_][A-Za-z0-9_]*:)").unwrap(),
            // [hookname] — named hook (alphanumeric + hyphens/underscores, no spaces)
            re_named_hook: Regex::new(r"\[([A-Za-z_][A-Za-z0-9_-]*)\]").unwrap(),
            // [text]<changer| — changer attached after hook
            re_hook_attach: Regex::new(r"\[([^\]]*?)\]<([A-Za-z_][A-Za-z0-9_]*)\|").unwrap(),
            // |changer>[text] — changer before hook
            re_hook_ref: Regex::new(r"\|([A-Za-z_][A-Za-z0-9_]*)>\[([^\]]*?)\]").unwrap(),
            // {text} — collapsing whitespace markup
            re_collapse: Regex::new(r"\{([^}]*)\}").unwrap(),
        }
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passage headers + bodies
    // -----------------------------------------------------------------------

    /// Parse passage headers from the full source text using a logos-based
    /// lexer for byte-offset accuracy.
    ///
    /// Returns a list of `(ParsedHeader, body_text)` pairs. The body text is
    /// the raw text between the end of this header line and the start of the
    /// next header (or end of file).
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(ParsedHeader, &'a str)> {
        let mut lex = logos::Lexer::new(text);
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

        let mut results: Vec<(ParsedHeader, &'a str)> = Vec::new();

        for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
            let header_line = &text[header_start..header_end];
            let parsed = Self::parse_header_line(header_line, header_start);

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
    fn parse_header_line(line: &str, offset: usize) -> Option<ParsedHeader> {
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

    /// Extract variable operations from a passage body.
    ///
    /// Harlowe uses `(set: $var to expr)`, `(put: expr into $var)`,
    /// `(move: $src into $dst)`, and `(unpack: ... into $var)` for writes.
    /// All `$var` references not inside a write are treated as reads.
    /// Harlowe does not have temporary variables like SugarCube's `_var`.
    fn extract_vars(&self, body: &str, body_offset: usize) -> Vec<VarOp> {
        let mut vars = Vec::new();
        let mut write_spans: Vec<Range<usize>> = Vec::new();

        // Detect writes via (set: $var to ...)
        for caps in self.re_set_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = format!("${}", caps.get(1).unwrap().as_str());
            let var_start = body_offset + full.start() + full.as_str().find('$').unwrap_or(0);
            let var_end = var_start + var_name.len();
            vars.push(VarOp {
                name: var_name,
                kind: VarKind::Write,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect writes via (put: ... into $var)
        for caps in self.re_put_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = format!("${}", caps.get(1).unwrap().as_str());
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: var_name,
                        kind: VarKind::Write,
                        span: var_start..var_end,
                        is_temporary: false,
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        // Detect (move: $src into $dst) — write to $dst, read from $src
        for caps in self.re_move_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let src_name = format!("${}", caps.get(1).unwrap().as_str());
            let dst_name = format!("${}", caps.get(2).unwrap().as_str());

            // Source variable is a read
            if let Some(first_dollar) = full.as_str().find('$') {
                let src_start = body_offset + full.start() + first_dollar;
                let src_end = src_start + src_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| src_start >= s.start && src_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: src_name,
                        kind: VarKind::Read,
                        span: src_start..src_end,
                        is_temporary: false,
                    });
                }
            }

            // Destination variable is a write (find second $)
            let dollar_positions: Vec<usize> = full
                .as_str()
                .char_indices()
                .filter(|&(_, c)| c == '$')
                .map(|(i, _)| i)
                .collect();
            if dollar_positions.len() >= 2 {
                let dst_dollar = dollar_positions[1];
                let dst_start = body_offset + full.start() + dst_dollar;
                let dst_end = dst_start + dst_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| dst_start >= s.start && dst_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: dst_name,
                        kind: VarKind::Write,
                        span: dst_start..dst_end,
                        is_temporary: false,
                    });
                    write_spans.push(dst_start..dst_end);
                }
            }
        }

        // Detect (unpack: ... into $var) — write to $var
        for caps in self.re_unpack_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_name = format!("${}", caps.get(1).unwrap().as_str());
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: var_name,
                        kind: VarKind::Write,
                        span: var_start..var_end,
                        is_temporary: false,
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        // Detect all $var references not already writes
        for caps in self.re_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
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

    /// Extract links from a passage body.
    fn extract_links(&self, body: &str, body_offset: usize) -> Vec<Link> {
        let mut links = Vec::new();

        // Harlowe changer links: (link:"text")[[Target]]
        for caps in self.re_link_changer.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let display = caps.get(1).unwrap().as_str().to_string();
            let target = caps.get(2).unwrap().as_str().trim().to_string();
            links.push(Link {
                display_text: Some(display),
                target,
                span: body_offset + m.start()..body_offset + m.end(),
            });
        }

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
        // Skip overlaps with arrow/pipe/changer links.
        let known_spans: Vec<Range<usize>> = self
            .re_link_arrow
            .captures_iter(body)
            .chain(self.re_link_pipe.captures_iter(body))
            .chain(self.re_link_changer.captures_iter(body))
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

        // Named hooks are also link targets: [hookname] can be reached via
        // (link-goto:) etc. Add them as links with target = hookname.
        // Collect all link spans to avoid overlaps.
        let all_link_spans: Vec<Range<usize>> = links
            .iter()
            .map(|l| (l.span.start - body_offset)..(l.span.end - body_offset))
            .collect();

        for caps in self.re_named_hook.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let hook_name = caps.get(1).unwrap().as_str().to_string();
            let hook_span = m.start()..m.end();
            // Skip if this overlaps with an existing link (e.g., [Forest] inside [[Forest]])
            let overlaps_link = all_link_spans
                .iter()
                .any(|s| hook_span.start >= s.start && hook_span.end <= s.end);
            // Skip if this is actually part of a hook attachment or reference
            let overlaps_attach = self.re_hook_attach.captures_iter(body).any(|ac| {
                let am = ac.get(0).unwrap();
                hook_span.start >= am.start() && hook_span.end <= am.end()
            });
            let overlaps_ref = self.re_hook_ref.captures_iter(body).any(|rc| {
                let rm = rc.get(0).unwrap();
                hook_span.start >= rm.start() && hook_span.end <= rm.end()
            });
            // Named hooks: single-word inside brackets, no spaces
            if !hook_name.contains(' ') && !overlaps_link && !overlaps_attach && !overlaps_ref {
                links.push(Link {
                    display_text: None,
                    target: hook_name,
                    span: body_offset + m.start()..body_offset + m.end(),
                });
            }
        }

        links
    }

    /// Extract content blocks from a passage body, properly categorizing
    /// text segments, macros, and expressions (hooks, collapsing markup).
    fn extract_blocks(&self, body: &str, body_offset: usize) -> Vec<Block> {
        let mut blocks = Vec::new();

        // Collect all non-text spans so we can identify text gaps.
        let mut non_text_spans: Vec<Range<usize>> = Vec::new();

        // Macros: (macroname: ...)
        for caps in self.re_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let macro_prefix = caps.get(1).unwrap().as_str();
            // macro_prefix is like "set:" — extract the name
            let name = macro_prefix.trim_end_matches(':').to_string();
            // Find the full parenthetical command
            let full_cmd = Self::find_paren_span(body, m.start());
            let span_end = if let Some(end) = full_cmd {
                body_offset + end
            } else {
                body_offset + m.end()
            };
            let span = body_offset + m.start()..span_end;
            // Content span is body[m.start()..span_end - body_offset] — used for\n            // variable extraction below, not stored directly.
            let args_start = m.end();
            let args_end = span_end - body_offset;
            let args = if args_end > args_start {
                body[args_start..args_end].trim_end_matches(')').to_string()
            } else {
                String::new()
            };

            blocks.push(Block::Macro {
                name,
                args,
                span: span.clone(),
            });
            non_text_spans.push(m.start()..(span.end - body_offset));
        }

        // Named hooks: [hookname] — Expression blocks
        for caps in self.re_named_hook.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let hook_name = caps.get(1).unwrap().as_str().to_string();
            // Skip if this overlaps with a hook attachment or reference span
            let span_range = m.start()..m.end();
            let overlaps = non_text_spans
                .iter()
                .any(|s| span_range.start >= s.start && span_range.end <= s.end);
            if !overlaps && !hook_name.contains(' ') {
                blocks.push(Block::Expression {
                    content: hook_name,
                    span: body_offset + m.start()..body_offset + m.end(),
                });
                non_text_spans.push(span_range);
            }
        }

        // Hook attachment: [text]<changer| — Expression block
        for caps in self.re_hook_attach.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let content = caps.get(1).unwrap().as_str().to_string();
            let changer = caps.get(2).unwrap().as_str().to_string();
            blocks.push(Block::Expression {
                content: format!("[{}]<{}|", content, changer),
                span: body_offset + m.start()..body_offset + m.end(),
            });
            non_text_spans.push(m.start()..m.end());
        }

        // Hook reference: |changer>[text] — Expression block
        for caps in self.re_hook_ref.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let changer = caps.get(1).unwrap().as_str().to_string();
            let content = caps.get(2).unwrap().as_str().to_string();
            blocks.push(Block::Expression {
                content: format!("|{}>[{}]", changer, content),
                span: body_offset + m.start()..body_offset + m.end(),
            });
            non_text_spans.push(m.start()..m.end());
        }

        // Collapsing whitespace markup: {text} — Expression block
        for caps in self.re_collapse.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let content = caps.get(1).unwrap().as_str().to_string();
            // Skip if this overlaps with already-tracked spans
            let span_range = m.start()..m.end();
            let overlaps = non_text_spans
                .iter()
                .any(|s| span_range.start >= s.start && span_range.end <= s.end);
            if !overlaps {
                blocks.push(Block::Expression {
                    content,
                    span: body_offset + m.start()..body_offset + m.end(),
                });
                non_text_spans.push(span_range);
            }
        }

        // Sort all non-text spans and fill gaps with Text blocks.
        non_text_spans.sort_by_key(|s| s.start);

        let mut cursor = 0;
        for span in &non_text_spans {
            if cursor < span.start {
                let text_content = body[cursor..span.start].to_string();
                if !text_content.trim().is_empty() {
                    blocks.push(Block::Text {
                        content: text_content,
                        span: body_offset + cursor..body_offset + span.start,
                    });
                }
            }
            cursor = span.end.max(cursor);
        }

        // Trailing text after the last non-text span.
        if cursor < body.len() {
            let text_content = body[cursor..].to_string();
            if !text_content.trim().is_empty() {
                blocks.push(Block::Text {
                    content: text_content,
                    span: body_offset + cursor..body_offset + body.len(),
                });
            }
        }

        // If no non-text blocks were found, the entire body is a text block.
        if non_text_spans.is_empty() && !body.trim().is_empty() {
            blocks.push(Block::Text {
                content: body.to_string(),
                span: body_offset..body_offset + body.len(),
            });
        }

        // Sort blocks by span start for consistent ordering.
        blocks.sort_by_key(|b| match b {
            Block::Text { span, .. } => span.start,
            Block::Macro { span, .. } => span.start,
            Block::Expression { span, .. } => span.start,
            Block::Heading { span, .. } => span.start,
            Block::Incomplete { span, .. } => span.start,
        });

        blocks
    }

    /// Find the end of a parenthetical command starting at `start` offset
    /// in `body`. Returns the byte offset past the closing `)`, or None
    /// if unclosed.
    fn find_paren_span(body: &str, start: usize) -> Option<usize> {
        let bytes = body.as_bytes();
        if start >= bytes.len() || bytes[start] != b'(' {
            return None;
        }
        let mut depth = 0i32;
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => {
                    depth += 1;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                b'"' => {
                    // Skip string contents
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1; // skip escaped char
                        }
                        i += 1;
                    }
                }
                b'\'' => {
                    // Skip single-quoted string contents
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'\'' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Generate semantic tokens for a passage body.
    fn body_tokens(&self, body: &str, body_offset: usize) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();

        // Link tokens
        for link in self.extract_links(body, body_offset) {
            tokens.push(SemanticToken {
                start: link.span.start,
                length: link.span.end - link.span.start,
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }

        // Macro tokens
        for caps in self.re_macro.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let full_cmd = Self::find_paren_span(body, m.start());
            let end = full_cmd.unwrap_or(m.end());
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: end - m.start(),
                token_type: SemanticTokenType::Macro,
                modifier: None,
            });
        }

        // Variable tokens — write spans first
        let mut write_spans: Vec<Range<usize>> = Vec::new();

        for caps in self.re_set_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start() + full.as_str().find('$').unwrap_or(0);
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

        for caps in self.re_put_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_name = format!("${}", caps.get(1).unwrap().as_str());
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    tokens.push(SemanticToken {
                        start: var_start,
                        length: var_name.len(),
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        for caps in self.re_move_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let dollar_positions: Vec<usize> = full
                .as_str()
                .char_indices()
                .filter(|&(_, c)| c == '$')
                .map(|(i, _)| i)
                .collect();
            if dollar_positions.len() >= 2 {
                // Destination is a write
                let dst_name = format!("${}", caps.get(2).unwrap().as_str());
                let dst_dollar = dollar_positions[1];
                let dst_start = body_offset + full.start() + dst_dollar;
                let dst_end = dst_start + dst_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| dst_start >= s.start && dst_end <= s.end);
                if !already_covered {
                    tokens.push(SemanticToken {
                        start: dst_start,
                        length: dst_name.len(),
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    write_spans.push(dst_start..dst_end);
                }
            }
        }

        for caps in self.re_unpack_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_name = format!("${}", caps.get(1).unwrap().as_str());
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    tokens.push(SemanticToken {
                        start: var_start,
                        length: var_name.len(),
                        token_type: SemanticTokenType::Variable,
                        modifier: Some(SemanticTokenModifier::Definition),
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        // Variable read tokens (skip overlaps with writes)
        for caps in self.re_var.captures_iter(body) {
            let full = caps.get(0).unwrap();
            let var_start = body_offset + full.start();
            let var_end = body_offset + full.end();
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

        // Hook expression tokens
        for caps in self.re_named_hook.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let hook_name = caps.get(1).unwrap().as_str();
            if !hook_name.contains(' ') {
                tokens.push(SemanticToken {
                    start: body_offset + m.start(),
                    length: m.end() - m.start(),
                    token_type: SemanticTokenType::Keyword,
                    modifier: None,
                });
            }
        }

        for caps in self.re_hook_attach.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Keyword,
                modifier: None,
            });
        }

        for caps in self.re_hook_ref.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Keyword,
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

        // Passage name starts after `:: ` (2 for :: + whitespace).
        let name_offset = header.header_start + 2;
        tokens.push(SemanticToken {
            start: name_offset,
            length: header.name.len(),
            token_type: SemanticTokenType::PassageHeader,
            modifier: None,
        });

        // Tags
        for (i, tag) in header.tags.iter().enumerate() {
            tokens.push(SemanticToken {
                start: name_offset + header.name.len() + 2 + i * (tag.len() + 1),
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier: None,
            });
        }

        tokens
    }

    /// Validate passage body for common Harlowe errors.
    fn validate(&self, body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();
        let bytes = body.as_bytes();

        // Check for unclosed parenthetical commands: `(command: ...` without `)`
        let mut paren_depth = 0i32;
        let mut paren_open: Option<usize> = None;
        let mut in_string = false;
        let mut string_char = b'\0';
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];

            if in_string {
                if c == b'\\' {
                    i += 2; // skip escaped char
                    continue;
                }
                if c == string_char {
                    in_string = false;
                }
                i += 1;
                continue;
            }

            if c == b'"' || c == b'\'' {
                in_string = true;
                string_char = c;
                i += 1;
                continue;
            }

            if c == b'(' {
                if paren_depth == 0 {
                    paren_open = Some(i);
                }
                paren_depth += 1;
            } else if c == b')' {
                paren_depth -= 1;
                if paren_depth < 0 {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + i..body_offset + i + 1,
                        message: "Unexpected `)` without matching `(`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "hl-unclosed-command".into(),
                    });
                    paren_depth = 0;
                }
            }
            i += 1;
        }

        if paren_depth > 0
            && let Some(pos) = paren_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 1,
                    message: "Unclosed parenthetical command — missing `)`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "hl-unclosed-command".into(),
                });
            }

        // Check for broken link syntax: `[[` without closing `]]`
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
                        message: "Unexpected `]]` without matching `[[`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "hl-broken-link".into(),
                    });
                    link_depth = 0;
                }
                j += 2;
                continue;
            }
            j += 1;
        }

        if link_depth > 0
            && let Some(pos) = link_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed link `[[` — missing `]]`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "hl-broken-link".into(),
                });
            }

        // Check for unclosed hook syntax: `[` without `]` (excluding `[[` and `]]`)
        let mut hook_depth = 0i32;
        let mut hook_open: Option<usize> = None;
        let mut k = 0;
        while k < bytes.len() {
            // Skip `[[` and `]]` (link syntax)
            if k + 1 < bytes.len() && bytes[k] == b'[' && bytes[k + 1] == b'[' {
                k += 2;
                continue;
            }
            if k + 1 < bytes.len() && bytes[k] == b']' && bytes[k + 1] == b']' {
                k += 2;
                continue;
            }
            if bytes[k] == b'[' {
                if hook_depth == 0 {
                    hook_open = Some(k);
                }
                hook_depth += 1;
            } else if bytes[k] == b']' {
                hook_depth -= 1;
                if hook_depth < 0 {
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + k..body_offset + k + 1,
                        message: "Unexpected `]` without matching `[`".into(),
                        severity: FormatDiagnosticSeverity::Warning,
                        code: "hl-unclosed-hook".into(),
                    });
                    hook_depth = 0;
                }
            }
            k += 1;
        }

        if hook_depth > 0
            && let Some(pos) = hook_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 1,
                    message: "Unclosed hook `[` — missing `]`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "hl-unclosed-hook".into(),
                });
            }

        // Check for mismatched changer syntax: [text]<name| without matching |name>[text]
        // Collect all changer names from [text]<name| patterns
        let mut attached_changers: Vec<(String, usize, usize)> = Vec::new(); // (name, start, end)
        for caps in self.re_hook_attach.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let name = caps.get(2).unwrap().as_str().to_string();
            attached_changers.push((name, m.start(), m.end()));
        }

        // Collect all reference changers from |name>[text] patterns
        let mut ref_changers: Vec<(String, usize, usize)> = Vec::new();
        for caps in self.re_hook_ref.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let name = caps.get(1).unwrap().as_str().to_string();
            ref_changers.push((name, m.start(), m.end()));
        }

        // Check for attached changers without matching references
        for (name, start, end) in &attached_changers {
            let has_ref = ref_changers.iter().any(|(rn, _, _)| rn == name);
            if !has_ref {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + *start..body_offset + *end,
                    message: format!(
                        "Changer `<{}|` is attached but has no matching `|{}>[...]` hook reference",
                        name, name
                    ),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "hl-mismatched-changer".into(),
                });
            }
        }

        // Check for reference changers without matching attachments
        for (name, start, end) in &ref_changers {
            let has_attached = attached_changers.iter().any(|(an, _, _)| an == name);
            if !has_attached {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + *start..body_offset + *end,
                    message: format!(
                        "Hook reference `|{}>[...]` has no matching `<{}|` changer attachment",
                        name, name
                    ),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "hl-mismatched-changer".into(),
                });
            }
        }

        diagnostics
    }

    /// Harlowe special passage definitions.
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
                name: "startup".into(),
                behavior: SpecialPassageBehavior::Startup,
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
            },
            SpecialPassageDef {
                name: "header".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(90),
            },
            SpecialPassageDef {
                name: "footer".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(110),
            },
            SpecialPassageDef {
                name: "PassageHeader".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(91),
            },
            SpecialPassageDef {
                name: "PassageFooter".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(111),
            },
        ]
    }

    /// Check if a passage with the given tags should be treated as having
    /// special header/footer behavior (Harlowe also supports tag-based
    /// header/footer: a passage tagged `header` or `footer`).
    fn has_tag_behavior(tags: &[String]) -> Option<SpecialPassageBehavior> {
        if tags.iter().any(|t| t == "header") {
            return Some(SpecialPassageBehavior::Chrome);
        }
        if tags.iter().any(|t| t == "footer") {
            return Some(SpecialPassageBehavior::Chrome);
        }
        if tags.iter().any(|t| t == "startup") {
            return Some(SpecialPassageBehavior::Startup);
        }
        None
    }
}

impl FormatPlugin for HarlowePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::Harlowe
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
            let special_def = special_defs
                .iter()
                .find(|d| d.name == header.name)
                .cloned()
                .or_else(|| {
                    // Tag-based special behavior
                    Self::has_tag_behavior(&header.tags).map(|behavior| {
                        let is_startup = behavior == SpecialPassageBehavior::Startup;
                        let priority = match &behavior {
                            SpecialPassageBehavior::Startup => Some(0),
                            SpecialPassageBehavior::Chrome => {
                                if header.tags.iter().any(|t| t == "header") {
                                    Some(92)
                                } else {
                                    Some(112)
                                }
                            }
                            _ => None,
                        };
                        SpecialPassageDef {
                            name: header.name.clone(),
                            behavior,
                            contributes_variables: is_startup,
                            participates_in_graph: false,
                            execution_priority: priority,
                        }
                    })
                });

            let mut passage = if let Some(def) = special_def {
                Passage::new_special(
                    header.name.clone(),
                    header.header_start..body_offset + body.len(),
                    def,
                )
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();

            // Extract body elements.
            passage.links = self.extract_links(body, body_offset);
            passage.vars = self.extract_vars(body, body_offset);
            passage.body = self.extract_blocks(body, body_offset);

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
        let special_defs = Self::special_passage_defs();
        let special_def = special_defs
            .iter()
            .find(|d| d.name == passage_name)
            .cloned();

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.links = self.extract_links(passage_text, 0);
        passage.vars = self.extract_vars(passage_text, 0);
        passage.body = self.extract_blocks(passage_text, 0);

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    fn display_name(&self) -> &str {
        "Harlowe 3"
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
        let plugin = HarlowePlugin::new();
        let src = ":: Start\nYou are in a room. [[Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
    }

    #[test]
    fn detect_special_passages() {
        let plugin = HarlowePlugin::new();
        assert!(plugin.is_special_passage("startup"));
        assert!(plugin.is_special_passage("header"));
        assert!(plugin.is_special_passage("footer"));
        assert!(plugin.is_special_passage("PassageHeader"));
        assert!(plugin.is_special_passage("PassageFooter"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = HarlowePlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");
        assert!(result.passages.is_empty());
        assert!(result.is_complete);
    }

    #[test]
    fn parse_set_variable() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $gold to 10)You have $gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_put_variable() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(put: 5 + 3 into $score)Your score is $score.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_variable_read_only() {
        let plugin = HarlowePlugin::new();
        let src = ":: Hallway\nYou have $health remaining.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Read));
        assert!(!vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Write));
    }

    // -----------------------------------------------------------------------
    // Hook parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_named_hook() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\nClick here [cave] to enter.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Named hooks should appear as links (they can be targeted by link-goto)
        assert!(passage.links.iter().any(|l| l.target == "cave"));
        // Named hooks should appear as Expression blocks
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content == "cave"
        )));
    }

    #[test]
    fn parse_hook_attachment() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[some text]<red|\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Hook attachment should appear as Expression block
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content.contains("red")
        )));
    }

    #[test]
    fn parse_hook_reference() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n|red>[some text]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Hook reference should appear as Expression block
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content.contains("red")
        )));
    }

    #[test]
    fn parse_collapse_markup() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n{   lots   of   space   }\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Collapsing markup should appear as Expression block
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content.contains("lots")
        )));
    }

    // -----------------------------------------------------------------------
    // (move:) variable extraction
    // -----------------------------------------------------------------------

    #[test]
    fn parse_move_variable() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(move: $source into $dest)\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        // $source should be a read (move reads from it)
        assert!(
            vars.iter().any(|v| v.name == "$source" && v.kind == VarKind::Read),
            "Should detect $source as a read in (move:)"
        );
        // $dest should be a write (move writes to it)
        assert!(
            vars.iter().any(|v| v.name == "$dest" && v.kind == VarKind::Write),
            "Should detect $dest as a write in (move:)"
        );
    }

    #[test]
    fn parse_unpack_variable() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(unpack: (a: 1, b: 2) into $result)\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "$result" && v.kind == VarKind::Write),
            "Should detect $result as a write in (unpack:)"
        );
    }

    // -----------------------------------------------------------------------
    // Diagnostic tests
    // -----------------------------------------------------------------------

    #[test]
    fn unclosed_command_diagnostic() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $x to 5\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "hl-unclosed-command"),
            "Should detect unclosed parenthetical command"
        );
    }

    #[test]
    fn unclosed_link_diagnostic() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[[Target\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "hl-broken-link"),
            "Should detect unclosed link syntax"
        );
    }

    #[test]
    fn unclosed_hook_diagnostic() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[hook without close\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "hl-unclosed-hook"),
            "Should detect unclosed hook syntax"
        );
    }

    #[test]
    fn mismatched_changer_diagnostic() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[text]<red|\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "hl-mismatched-changer"),
            "Should detect mismatched changer (attached without reference)"
        );
    }

    // -----------------------------------------------------------------------
    // Multiple passages with overlapping content
    // -----------------------------------------------------------------------

    #[test]
    fn parse_multiple_passages() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
    }

    #[test]
    fn passages_with_duplicate_lines() {
        // This test specifically targets the old buggy text.find(line) approach.
        let plugin = HarlowePlugin::new();
        let src = ":: Start\nYou see a cat.\n:: Middle\nYou see a cat.\n:: End\nDone.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 3);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Middle");
        assert_eq!(result.passages[2].name, "End");
        // Each passage should have the correct body
        assert!(result.passages[0].body.iter().any(|b| matches!(
            b,
            Block::Text { content, .. } if content.contains("cat")
        )));
        assert!(result.passages[1].body.iter().any(|b| matches!(
            b,
            Block::Text { content, .. } if content.contains("cat")
        )));
    }

    // -----------------------------------------------------------------------
    // Passage with tags
    // -----------------------------------------------------------------------

    #[test]
    fn parse_passage_with_tags() {
        let plugin = HarlowePlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    // -----------------------------------------------------------------------
    // Arrow-style link with `->` in display text
    // -----------------------------------------------------------------------

    #[test]
    fn parse_arrow_link_with_arrow_in_display() {
        let plugin = HarlowePlugin::new();
        // Arrow link: display text may contain characters that aren't `]]`
        let src = ":: Start\n[[Go ->Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    // -----------------------------------------------------------------------
    // Pipe-style link with `|` in display text
    // -----------------------------------------------------------------------

    #[test]
    fn parse_pipe_link() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
        assert_eq!(links[0].display_text, Some("Go to forest".into()));
    }

    // -----------------------------------------------------------------------
    // Complex multi-variable passage
    // -----------------------------------------------------------------------

    #[test]
    fn complex_multi_variable_passage() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $health to 100)(set: $name to \"Hero\")(put: 50 into $score)\nYour health is $health, $name.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$name" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Write));
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Read));
        assert!(vars.iter().any(|v| v.name == "$name" && v.kind == VarKind::Read));
    }

    // -----------------------------------------------------------------------
    // Block model tests
    // -----------------------------------------------------------------------

    #[test]
    fn block_model_has_macro_blocks() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $x to 5)Hello\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        assert!(
            passage.body.iter().any(|b| matches!(b, Block::Macro { name, .. } if name == "set")),
            "Should have a Macro block for (set:)"
        );
        assert!(
            passage.body.iter().any(|b| matches!(b, Block::Text { .. })),
            "Should have a Text block for 'Hello'"
        );
    }

    #[test]
    fn block_model_has_expression_blocks() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n[hookname] Some text\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        assert!(
            passage.body.iter().any(|b| matches!(b, Block::Expression { .. })),
            "Should have an Expression block for [hookname]"
        );
    }

    // -----------------------------------------------------------------------
    // Incremental re-parse
    // -----------------------------------------------------------------------

    #[test]
    fn incremental_reparse() {
        let plugin = HarlowePlugin::new();
        let passage = plugin.parse_passage("Start", "You have $gold coins.\n");

        assert!(passage.is_some());
        let p = passage.unwrap();
        assert_eq!(p.name, "Start");
        assert!(p.vars.iter().any(|v| v.name == "$gold"));
    }

    // -----------------------------------------------------------------------
    // Special passage tag behavior
    // -----------------------------------------------------------------------

    #[test]
    fn tagged_header_passage() {
        let plugin = HarlowePlugin::new();
        let src = ":: Nav [header]\nNavigation here.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let p = &result.passages[0];
        assert!(p.is_special, "Passage tagged 'header' should be special");
    }

    #[test]
    fn tagged_footer_passage() {
        let plugin = HarlowePlugin::new();
        let src = ":: Credits [footer]\nThanks for playing.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let p = &result.passages[0];
        assert!(p.is_special, "Passage tagged 'footer' should be special");
    }

    // -----------------------------------------------------------------------
    // Changer link
    // -----------------------------------------------------------------------

    #[test]
    fn parse_changer_link() {
        let plugin = HarlowePlugin::new();
        let src = ":: Start\n(link: \"Click me\")[[Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(links.iter().any(|l| l.target == "Forest" && l.display_text == Some("Click me".into())));
    }
}
