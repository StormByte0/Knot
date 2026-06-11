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
    Block, Link, MatchStrategy, Passage, SpecialPassageBehavior, SpecialPassageDef, SpecialPassageLayer,
    StoryFormat, VarKind, VarOp,
};
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;
use url::Url;


use crate::header::{self, TweeHeader};
use crate::plugin::{
    FormatDiagnostic, FormatDiagnosticSeverity, FormatPlugin, FormatPluginMut, ParseResult, SemanticToken,
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
// Regex statics
// ---------------------------------------------------------------------------

/// Regex for simple links: `[[Target]]`
static RE_LINK_SIMPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").expect("invalid regex for RE_LINK_SIMPLE"));
/// Regex for arrow links: `[[Display->Target]]`
static RE_LINK_ARROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").expect("invalid regex for RE_LINK_ARROW"));
/// Regex for pipe links: `[[Display|Target]]`
static RE_LINK_PIPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").expect("invalid regex for RE_LINK_PIPE"));
/// Regex for Harlowe link changer: `(link:"text")[[Target]]`
static RE_LINK_CHANGER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\(link:\s*"([^"]+)"\s*\)\[\[([^\]]+?)\]\]"#).expect("invalid regex for RE_LINK_CHANGER"));
/// Regex for Harlowe (set: $var to ...) variable write.
static RE_SET_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(set:\s*\$([A-Za-z_][A-Za-z0-9_]*)\s+to\b").expect("invalid regex for RE_SET_VAR"));
/// Regex for Harlowe (put: ... into $var) variable write.
static RE_PUT_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(put:[^)]*into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)").expect("invalid regex for RE_PUT_VAR"));
/// Regex for Harlowe (move: $var into $other) variable operation.
static RE_MOVE_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\(move:\s*\$([A-Za-z_][A-Za-z0-9_]*)\s+into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)")
        .expect("invalid regex for RE_MOVE_VAR")
});
/// Regex for Harlowe (unpack: ... into $var) variable write.
static RE_UNPACK_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(unpack:.*?into\s+\$([A-Za-z_][A-Za-z0-9_]*)\s*\)").expect("invalid regex for RE_UNPACK_VAR"));
/// Regex for all $variable references.
static RE_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").expect("invalid regex for RE_VAR"));
/// Regex for Harlowe macros: (name: ...)
static RE_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([A-Za-z_][A-Za-z0-9_]*:)" ).expect("invalid regex for RE_MACRO"));
/// Regex for named hooks: [hookname]
static RE_NAMED_HOOK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([A-Za-z_][A-Za-z0-9_-]*)\]").expect("invalid regex for RE_NAMED_HOOK"));
/// Regex for hook attachment: [text]<changer|
static RE_HOOK_ATTACH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*?)\]<([A-Za-z_][A-Za-z0-9_]*)\|").expect("invalid regex for RE_HOOK_ATTACH"));
/// Regex for hook reference: |changer>[text]
static RE_HOOK_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\|([A-Za-z_][A-Za-z0-9_]*)>\[([^\]]*?)\]").expect("invalid regex for RE_HOOK_REF"));
/// Regex for collapsing whitespace markup: {text}
static RE_COLLAPSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^}]*)\}").expect("invalid regex for RE_COLLAPSE"));
/// Regex for macro call detection in body (syntax handler dispatch).
static RE_MACRO_CALL_IN_BODY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([A-Za-z_][A-Za-z0-9_]*):").expect("invalid regex for RE_MACRO_CALL_IN_BODY"));

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// Harlowe 3.x format plugin.
pub struct HarlowePlugin;

impl Default for HarlowePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl HarlowePlugin {
    /// Create a new Harlowe plugin instance.
    pub fn new() -> Self {
        Self
    }

    // -----------------------------------------------------------------------
    // Pass 1: Split source into passage headers + bodies
    // -----------------------------------------------------------------------

    /// Parse passage headers from the full source text using a logos-based
    /// lexer for byte-offset accuracy.
    ///
    /// Returns a list of `(TweeHeader, body_text)` pairs. The body text is
    /// the raw text between the end of this header line and the start of the
    /// next header (or end of file).
    fn split_passages<'a>(&self, text: &'a str) -> Vec<(TweeHeader, &'a str)> {
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

        let mut results: Vec<(TweeHeader, &'a str)> = Vec::new();

        for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
            let mut header_line = &text[header_start..header_end];
            // The Logos regex `::[^\n]*` includes trailing \r on CRLF files.
            // Strip it so that parse_twee_header() receives clean content and
            // body_offset calculation is correct.
            let trailing_cr = header_line.ends_with('\r');
            if trailing_cr {
                header_line = &header_line[..header_line.len() - 1];
            }
            let adjusted_header_end = if trailing_cr { header_end - 1 } else { header_end };
            let parsed = header::parse_twee_header(header_line, header_start);

            // Body starts after the header line (skip trailing newline).
            let body_start = adjusted_header_end;
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
        for caps in RE_SET_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let var_name = format!("${}", match1.as_str());
            let var_start = body_offset + full.start() + full.as_str().find('$').unwrap_or(0);
            let var_end = var_start + var_name.len();
            vars.push(VarOp {
                name: var_name,
                kind: VarKind::Init,
                span: var_start..var_end,
                is_temporary: false,
            });
            write_spans.push(var_start..var_end);
        }

        // Detect writes via (put: ... into $var)
        for caps in RE_PUT_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let var_name = format!("${}", match1.as_str());
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: var_name,
                        kind: VarKind::Init,
                        span: var_start..var_end,
                        is_temporary: false,
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        // Detect (move: $src into $dst) — write to $dst, read from $src
        for caps in RE_MOVE_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let src_name = format!("${}", match1.as_str());
            let dst_name = format!("${}", match2.as_str());

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
                        kind: VarKind::Init,
                        span: dst_start..dst_end,
                        is_temporary: false,
                    });
                    write_spans.push(dst_start..dst_end);
                }
            }
        }

        // Detect (unpack: ... into $var) — write to $var
        for caps in RE_UNPACK_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let var_name = format!("${}", match1.as_str());
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_end = var_start + var_name.len();
                let already_covered = write_spans
                    .iter()
                    .any(|s| var_start >= s.start && var_end <= s.end);
                if !already_covered {
                    vars.push(VarOp {
                        name: var_name,
                        kind: VarKind::Init,
                        span: var_start..var_end,
                        is_temporary: false,
                    });
                    write_spans.push(var_start..var_end);
                }
            }
        }

        // Detect all $var references not already writes
        for caps in RE_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
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
        for caps in RE_LINK_CHANGER.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let display = match1.as_str().to_string();
            let target = match2.as_str().trim().to_string();
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

        // Arrow-style links: [[Display->Target]]
        for caps in RE_LINK_ARROW.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let display = match1.as_str().trim().to_string();
            let target = match2.as_str().trim().to_string();
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

        // Pipe-style links: [[Display|Target]]
        for caps in RE_LINK_PIPE.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let display = match1.as_str().trim().to_string();
            let target = match2.as_str().trim().to_string();
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

        // Simple links: [[Target]]
        // Skip overlaps with arrow/pipe/changer links.
        let known_spans: Vec<Range<usize>> = RE_LINK_ARROW
            .captures_iter(body)
            .chain(RE_LINK_PIPE.captures_iter(body))
            .chain(RE_LINK_CHANGER.captures_iter(body))
            .filter_map(|caps| {
                let m = caps.get(0)?;
                Some(m.start()..m.end())
            })
            .collect();

        for caps in RE_LINK_SIMPLE.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let span = m.start()..m.end();
            let overlaps = known_spans
                .iter()
                .any(|s| span.start >= s.start && span.end <= s.end);
            if !overlaps {
                let Some(match1) = caps.get(1) else { continue };
                let target = match1.as_str().trim().to_string();
                // Filter: skip targets containing "::" — this is JavaScript
                // namespace accessor syntax (e.g., Use::Operation), not a
                // Twine passage name.
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

        // Named hooks are also link targets: [hookname] can be reached via
        // (link-goto:) etc. Add them as links with target = hookname.
        // Collect all link spans to avoid overlaps.
        let all_link_spans: Vec<Range<usize>> = links
            .iter()
            .map(|l| (l.span.start - body_offset)..(l.span.end - body_offset))
            .collect();

        for caps in RE_NAMED_HOOK.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let hook_name = match1.as_str().to_string();
            let hook_span = m.start()..m.end();
            // Skip if this overlaps with an existing link (e.g., [Forest] inside [[Forest]])
            let overlaps_link = all_link_spans
                .iter()
                .any(|s| hook_span.start >= s.start && hook_span.end <= s.end);
            // Skip if this is actually part of a hook attachment or reference
            let overlaps_attach = RE_HOOK_ATTACH.captures_iter(body).any(|ac| {
                let Some(am) = ac.get(0) else { return false };
                hook_span.start >= am.start() && hook_span.end <= am.end()
            });
            let overlaps_ref = RE_HOOK_REF.captures_iter(body).any(|rc| {
                let Some(rm) = rc.get(0) else { return false };
                hook_span.start >= rm.start() && hook_span.end <= rm.end()
            });
            // Named hooks: single-word inside brackets, no spaces
            if !hook_name.contains(' ') && !overlaps_link && !overlaps_attach && !overlaps_ref {
                links.push(Link {
                    display_text: None,
                    target: hook_name,
                    span: body_offset + m.start()..body_offset + m.end(),
                    edge_type_hint: None,
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
        for caps in RE_MACRO.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let macro_prefix = match1.as_str();
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
        for caps in RE_NAMED_HOOK.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let hook_name = match1.as_str().to_string();
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
        for caps in RE_HOOK_ATTACH.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let content = match1.as_str().to_string();
            let changer = match2.as_str().to_string();
            blocks.push(Block::Expression {
                content: format!("[{}]<{}|", content, changer),
                span: body_offset + m.start()..body_offset + m.end(),
            });
            non_text_spans.push(m.start()..m.end());
        }

        // Hook reference: |changer>[text] — Expression block
        for caps in RE_HOOK_REF.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let changer = match1.as_str().to_string();
            let content = match2.as_str().to_string();
            blocks.push(Block::Expression {
                content: format!("|{}>[{}]", changer, content),
                span: body_offset + m.start()..body_offset + m.end(),
            });
            non_text_spans.push(m.start()..m.end());
        }

        // Collapsing whitespace markup: {text} — Expression block
        for caps in RE_COLLAPSE.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let content = match1.as_str().to_string();
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
        for caps in RE_MACRO.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
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

        for caps in RE_SET_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let var_start = body_offset + full.start() + full.as_str().find('$').unwrap_or(0);
            let var_name = format!("${}", match1.as_str());
            let var_end = var_start + var_name.len();
            tokens.push(SemanticToken {
                start: var_start,
                length: var_name.len(),
                token_type: SemanticTokenType::Variable,
                modifier: Some(SemanticTokenModifier::Definition),
            });
            write_spans.push(var_start..var_end);
        }

        for caps in RE_PUT_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_name = format!("${}", match1.as_str());
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

        for caps in RE_MOVE_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let dollar_positions: Vec<usize> = full
                .as_str()
                .char_indices()
                .filter(|&(_, c)| c == '$')
                .map(|(i, _)| i)
                .collect();
            if dollar_positions.len() >= 2 {
                // Destination is a write
                let dst_name = format!("${}", match2.as_str());
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

        for caps in RE_UNPACK_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            if let Some(dollar_pos) = full.as_str().rfind('$') {
                let var_start = body_offset + full.start() + dollar_pos;
                let var_name = format!("${}", match1.as_str());
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
        for caps in RE_VAR.captures_iter(body) {
            let Some(full) = caps.get(0) else { continue };
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
        for caps in RE_NAMED_HOOK.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let hook_name = match1.as_str();
            if !hook_name.contains(' ') {
                tokens.push(SemanticToken {
                    start: body_offset + m.start(),
                    length: m.end() - m.start(),
                    token_type: SemanticTokenType::Keyword,
                    modifier: None,
                });
            }
        }

        for caps in RE_HOOK_ATTACH.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Keyword,
                modifier: None,
            });
        }

        for caps in RE_HOOK_REF.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
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
    /// Uses `PassageName` for regular passage names (distinct from `::` prefix)
    /// and `SpecialPassage`/`SpecialPassageHeader` for special passages.
    fn header_tokens(&self, header: &TweeHeader, is_special: bool) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();

        let (prefix_type, name_type) = if is_special {
            (SemanticTokenType::SpecialPassageHeader, SemanticTokenType::SpecialPassage)
        } else {
            (SemanticTokenType::PassageHeader, SemanticTokenType::PassageName)
        };

        // The `::` prefix is always 2 bytes.
        tokens.push(SemanticToken {
            start: header.header_start,
            length: 2,
            token_type: prefix_type,
            modifier: None,
        });

        // Passage name — use name_start for accurate positioning.
        tokens.push(SemanticToken {
            start: header.name_start,
            length: header.name.len(),
            token_type: name_type,
            modifier: None,
        });

        // Tags — compute positions from the raw text after colons (JSON-stripped).
        // Use `tags_raw` which preserves `[tag]` blocks (unlike `name_text_raw`
        // which strips them). This ensures `find('[')` actually finds the bracket,
        // giving correct tag positions regardless of whitespace between name and `[`.
        //
        // Each tag gets a modifier based on `classify_tag()`:
        // - Core tags ([script], [stylesheet]) → TwineCore
        // - Format tags ([startup], [header], [footer]) → StoryFormat
        // - Custom tags → None
        let bracket_start = if let Some(bs) = header.tags_raw.find('[') {
            header.name_start + bs
        } else {
            header.name_start + header.name_text_raw.len()
        };
        let tags_inner_start = bracket_start + 1; // after `[`
        let mut offset = tags_inner_start;
        for tag in &header.tags {
            let modifier = self.classify_tag(tag);
            if offset > tags_inner_start {
                offset += 1; // space between tags
            }
            tokens.push(SemanticToken {
                start: offset,
                length: tag.len(),
                token_type: SemanticTokenType::Tag,
                modifier,
            });
            offset += tag.len();
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
        for caps in RE_HOOK_ATTACH.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match2) = caps.get(2) else { continue };
            let name = match2.as_str().to_string();
            attached_changers.push((name, m.start(), m.end()));
        }

        // Collect all reference changers from |name>[text] patterns
        let mut ref_changers: Vec<(String, usize, usize)> = Vec::new();
        for caps in RE_HOOK_REF.captures_iter(body) {
            let Some(m) = caps.get(0) else { continue };
            let Some(match1) = caps.get(1) else { continue };
            let name = match1.as_str().to_string();
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

    /// Harlowe name-matched special passage definitions.
    ///
    /// These are passages identified by their exact passage name.
    /// Only `PassageHeader` and `PassageFooter` are name-matched in Harlowe.
    /// The `[startup]`, `[header]`, and `[footer]` tag-matched definitions
    /// live in `tag_matched_special_passages()`.
    fn special_passage_defs() -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "PassageHeader".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::ChromeInterceptor,
                contributes_variables: true,
                participates_in_graph: true,
                execution_priority: Some(91),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "PassageFooter".into(),
                match_strategy: MatchStrategy::Name,
                behavior: SpecialPassageBehavior::ChromeInterceptor,
                contributes_variables: true,
                participates_in_graph: true,
                execution_priority: Some(111),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
        ]
    }

}

impl FormatPluginMut for HarlowePlugin {
    fn parse_mut(&mut self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        let raw_passages = self.split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_start
                + text[header.header_start..]
                    .find('\n')
                    .unwrap_or(text[header.header_start..].len())
                + 1;

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
            let is_script = passage.is_script_passage();
            let is_stylesheet = passage.is_stylesheet_passage();

            if is_script {
                passage.body = crate::core_specials::raw_body_blocks(body, body_offset);
                tokens.extend(self.header_tokens(header, true));
            } else if is_stylesheet {
                passage.body = crate::core_specials::raw_body_blocks(body, body_offset);
                tokens.extend(self.header_tokens(header, true));
            } else {
                passage.links = self.extract_links(body, body_offset);
                passage.vars = self.extract_vars(body, body_offset);
                passage.body = self.extract_blocks(body, body_offset);

                tokens.extend(self.header_tokens(header, special_def.is_some()));
                tokens.extend(self.body_tokens(body, body_offset));

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

        ParseResult {
            passages,
            tokens,
            diagnostics,
            is_complete: !has_errors,
        }
    }

    fn parse_passage_mut(&mut self, passage_name: &str, passage_tags: &[String], passage_text: &str, _file_uri: &str) -> Option<Passage> {
        let special_def = self.classify_passage(passage_name, passage_tags);

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.tags = passage_tags.to_vec();

        let is_script = passage.is_script_passage();
        let is_stylesheet = passage.is_stylesheet_passage();

        if is_script || is_stylesheet {
            passage.body = crate::core_specials::raw_body_blocks(passage_text, 0);
        } else {
            passage.links = self.extract_links(passage_text, 0);
            passage.vars = self.extract_vars(passage_text, 0);
            passage.body = self.extract_blocks(passage_text, 0);
        }

        Some(passage)
    }

    fn remove_file_from_registries(&mut self, _file_uri: &str) {}
    fn remove_passage_from_registries(&mut self, _passage_name: &str, _file_uri: &str) {}
}

impl FormatPlugin for HarlowePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::Harlowe
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        Self::special_passage_defs()
    }

    /// Harlowe tag-matched special passage definitions.
    ///
    /// In Harlowe, `[startup]`, `[header]`, and `[footer]` are TAG-based
    /// special passages — the passage name is user-defined and irrelevant
    /// for classification. A passage like `:: Nav [header]` is classified
    /// as a ChromeInterceptor by its tag, not its name.
    ///
    /// This override ensures that `classify_passage()` (used by both
    /// `parse()` and `parse_passage()`) correctly identifies tag-matched
    /// special passages, fixing the incremental re-parse path that was
    /// previously broken because the default `tag_matched_special_passages()`
    /// returned an empty vec.
    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        vec![
            SpecialPassageDef {
                name: "startup".into(),
                match_strategy: MatchStrategy::Tag,
                behavior: SpecialPassageBehavior::Startup,
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "header".into(),
                match_strategy: MatchStrategy::Tag,
                behavior: SpecialPassageBehavior::ChromeInterceptor,
                contributes_variables: true,
                participates_in_graph: true,
                execution_priority: Some(90),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
            SpecialPassageDef {
                name: "footer".into(),
                match_strategy: MatchStrategy::Tag,
                behavior: SpecialPassageBehavior::ChromeInterceptor,
                contributes_variables: true,
                participates_in_graph: true,
                execution_priority: Some(110),
                layer: SpecialPassageLayer::StoryFormat,
                scaffold: None,
            },
        ]
    }

    fn display_name(&self) -> &str {
        "Harlowe 3"
    }

    // -------------------------------------------------------------------
    // Variable tracking capability
    // -------------------------------------------------------------------

    fn supports_partial_variable_tracking(&self) -> bool {
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

        // Harlowe uses (name:args) syntax.
        // Scan for opening ( followed by identifier: pattern.
        for caps in RE_MACRO_CALL_IN_BODY.captures_iter(line) {
            let Some(full_match) = caps.get(0) else { continue };
            let Some(name_match) = caps.get(1) else { continue };

            // Find the closing ) for this macro — scan forward from the opening (
            let open_paren = full_match.start();
            let name_start = name_match.start();
            let name_end = name_match.end();

            // Simple approach: find closing paren. Harlowe macros can be
            // nested, but for position detection we just need the first
            // matching close paren.
            let close_paren = line[open_paren..].find(')').map(|p| open_paren + p + 1).unwrap_or(line.len());

            if byte_pos >= open_paren && byte_pos <= close_paren {
                return Some(MacroAtPosition {
                    name: name_match.as_str().to_string(),
                    full_range: open_paren..close_paren,
                    name_range: name_start..name_end,
                    is_unclosed: close_paren == line.len(),
                });
            }
        }
        None
    }

    fn scan_line_for_macro_events(
        &self,
        _line: &str,
        _line_idx: u32,
    ) -> Vec<crate::plugin::MacroBlockEvent> {
        // Harlowe doesn't have block macros with close tags in the same
        // way SugarCube does. Harlowe uses hooks [text]<changer| and
        // |changer>[text] for block-like structure. For now, we don't
        // report macro block events since there are no close tags to pair.
        // Hook-based folding could be added later.
        Vec::new()
    }

    fn format_macro_label(&self, name: &str) -> String {
        format!("({}:)", name)
    }

    fn format_macro_signature_label(&self, name: &str, params: &str) -> String {
        if params.is_empty() {
            format!("({}:)", name)
        } else {
            format!("({}: {})", name, params)
        }
    }

    fn format_close_macro_label(&self, _name: &str) -> String {
        String::new() // Harlowe has no close tags
    }

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        if has_body {
            format!("({}: $1)\n$2\n(/{}:)", name, name)
        } else {
            format!("({}: $1)", name)
        }
    }

    fn detect_close_tag_context(&self, _before_cursor: &str) -> Option<String> {
        None // Harlowe has no close tags
    }

    fn has_block_macros_with_close_tags(&self) -> bool {
        false // Harlowe uses hooks, not close tags
    }

    fn variable_assignment_snippet(&self, var_name: &str, value: &str) -> Option<String> {
        Some(format!("(set: {} to {})", var_name, value))
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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\nYou are in a room. [[Forest]]\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
    }

    #[test]
    fn detect_special_passages() {
        let mut plugin = HarlowePlugin::new();
        // Name-matched passages: detected by passage name alone (no tags needed)
        assert!(plugin.is_special_passage("PassageHeader"));
        assert!(plugin.is_special_passage("PassageFooter"));
        assert!(!plugin.is_special_passage("MyRoom"));

        // Tag-matched passages: NOT detected by name alone — they require tags.
        // "startup", "header", "footer" are TAG-based in Harlowe, not name-based.
        // Use classify_passage() to detect them with tags.
        assert!(!plugin.is_special_passage("startup"),
            "startup is tag-matched, not name-matched");
        assert!(!plugin.is_special_passage("header"),
            "header is tag-matched, not name-matched");
        assert!(!plugin.is_special_passage("footer"),
            "footer is tag-matched, not name-matched");

        // Verify tag-matched detection works via classify_passage
        let startup_def = plugin.classify_passage("Init", &["startup".to_string()]);
        assert!(startup_def.is_some(), "startup tag should classify via classify_passage");
        assert!(matches!(startup_def.unwrap().behavior, SpecialPassageBehavior::Startup));

        let header_def = plugin.classify_passage("Nav", &["header".to_string()]);
        assert!(header_def.is_some(), "header tag should classify via classify_passage");
        assert!(matches!(header_def.unwrap().behavior, SpecialPassageBehavior::ChromeInterceptor));
    }

    #[test]
    fn empty_input_is_ok() {
        let mut plugin = HarlowePlugin::new();
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), "");
        assert!(result.passages.is_empty());
        assert!(result.is_complete);
    }

    #[test]
    fn parse_set_variable() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $gold to 10)You have $gold coins.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_put_variable() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(put: 5 + 3 into $score)Your score is $score.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_variable_read_only() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Hallway\nYou have $health remaining.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Read));
        assert!(!vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Init));
    }

    // -----------------------------------------------------------------------
    // Hook parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_named_hook() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\nClick here [cave] to enter.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[some text]<red|\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Hook attachment should appear as Expression block
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content.contains("red")
        )));
    }

    #[test]
    fn parse_hook_reference() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n|red>[some text]\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let passage = &result.passages[0];
        // Hook reference should appear as Expression block
        assert!(passage.body.iter().any(|b| matches!(
            b,
            Block::Expression { content, .. } if content.contains("red")
        )));
    }

    #[test]
    fn parse_collapse_markup() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n{   lots   of   space   }\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(move: $source into $dest)\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        // $source should be a read (move reads from it)
        assert!(
            vars.iter().any(|v| v.name == "$source" && v.kind == VarKind::Read),
            "Should detect $source as a read in (move:)"
        );
        // $dest should be a write (move writes to it)
        assert!(
            vars.iter().any(|v| v.name == "$dest" && v.kind == VarKind::Init),
            "Should detect $dest as a write in (move:)"
        );
    }

    #[test]
    fn parse_unpack_variable() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(unpack: (a: 1, b: 2) into $result)\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            vars.iter().any(|v| v.name == "$result" && v.kind == VarKind::Init),
            "Should detect $result as a write in (unpack:)"
        );
    }

    // -----------------------------------------------------------------------
    // Diagnostic tests
    // -----------------------------------------------------------------------

    #[test]
    fn unclosed_command_diagnostic() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $x to 5\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[[Target\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "hl-broken-link"),
            "Should detect unclosed link syntax"
        );
    }

    #[test]
    fn unclosed_hook_diagnostic() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[hook without close\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "hl-unclosed-hook"),
            "Should detect unclosed hook syntax"
        );
    }

    #[test]
    fn mismatched_changer_diagnostic() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[text]<red|\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
    }

    #[test]
    fn passages_with_duplicate_lines() {
        // This test specifically targets the old buggy text.find(line) approach.
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\nYou see a cat.\n:: Middle\nYou see a cat.\n:: End\nDone.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    // -----------------------------------------------------------------------
    // Arrow-style link with `->` in display text
    // -----------------------------------------------------------------------

    #[test]
    fn parse_arrow_link_with_arrow_in_display() {
        let mut plugin = HarlowePlugin::new();
        // Arrow link: display text may contain characters that aren't `]]`
        let src = ":: Start\n[[Go ->Forest]]\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Forest");
    }

    // -----------------------------------------------------------------------
    // Pipe-style link with `|` in display text
    // -----------------------------------------------------------------------

    #[test]
    fn parse_pipe_link() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $health to 100)(set: $name to \"Hero\")(put: 50 into $score)\nYour health is $health, $name.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$name" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$score" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$health" && v.kind == VarKind::Read));
        assert!(vars.iter().any(|v| v.name == "$name" && v.kind == VarKind::Read));
    }

    // -----------------------------------------------------------------------
    // Block model tests
    // -----------------------------------------------------------------------

    #[test]
    fn block_model_has_macro_blocks() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(set: $x to 5)Hello\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n[hookname] Some text\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

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
        let mut plugin = HarlowePlugin::new();
        let passage = plugin.parse_passage_mut("Start", &[], "You have $gold coins.\n", "");

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
        let mut plugin = HarlowePlugin::new();
        let src = ":: Nav [header]\nNavigation here.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let p = &result.passages[0];
        assert!(p.is_special, "Passage tagged 'header' should be special");
    }

    #[test]
    fn tagged_footer_passage() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Credits [footer]\nThanks for playing.\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let p = &result.passages[0];
        assert!(p.is_special, "Passage tagged 'footer' should be special");
    }

    // -----------------------------------------------------------------------
    // Incremental re-parse (parse_passage) with tag-matched passages
    // -----------------------------------------------------------------------

    #[test]
    fn parse_passage_tagged_header() {
        let mut plugin = HarlowePlugin::new();
        let result = plugin.parse_passage_mut(
            "Nav",
            &["header".to_string()],
            "Some header content\n",
            "",
        );
        let p = result.expect("tagged [header] passage should be classified as special");
        assert!(p.is_special, "Passage tagged 'header' should be special via classify_passage");
        assert!(p.special_def.is_some(), "special_def should be populated for tagged [header]");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::ChromeInterceptor));
    }

    #[test]
    fn parse_passage_tagged_footer() {
        let mut plugin = HarlowePlugin::new();
        let result = plugin.parse_passage_mut(
            "Credits",
            &["footer".to_string()],
            "Thanks for playing.\n",
            "",
        );
        let p = result.expect("tagged [footer] passage should be classified as special");
        assert!(p.is_special, "Passage tagged 'footer' should be special via classify_passage");
        assert!(p.special_def.is_some(), "special_def should be populated for tagged [footer]");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::ChromeInterceptor));
    }

    #[test]
    fn parse_passage_tagged_startup() {
        let mut plugin = HarlowePlugin::new();
        let result = plugin.parse_passage_mut(
            "Init",
            &["startup".to_string()],
            "(set: $x to 1)\n",
            "",
        );
        let p = result.expect("tagged [startup] passage should be classified as special");
        assert!(p.is_special, "Passage tagged 'startup' should be special via classify_passage");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::Startup));
        assert!(def.contributes_variables, "startup passages should contribute variables");
    }

    #[test]
    fn parse_passage_name_matched_passage_header() {
        let mut plugin = HarlowePlugin::new();
        let result = plugin.parse_passage_mut(
            "PassageHeader",
            &[],
            "Header content\n",
            "",
        );
        let p = result.expect("PassageHeader (name-matched) should be classified as special");
        assert!(p.is_special, "PassageHeader should be special via name matching");
        let def = p.special_def.as_ref().unwrap();
        assert!(matches!(def.behavior, SpecialPassageBehavior::ChromeInterceptor));
    }

    #[test]
    fn classify_passage_tag_takes_priority_over_name() {
        let mut plugin = HarlowePlugin::new();
        // A passage named "PassageHeader" but tagged [startup] should
        // be classified by its TAG (startup) first per the Twee 3 spec,
        // not by its name (PassageHeader).
        let def = plugin.classify_passage("PassageHeader", &["startup".to_string()]);
        assert!(def.is_some(), "Should classify PassageHeader with [startup] tag");
        let d = def.unwrap();
        assert!(matches!(d.behavior, SpecialPassageBehavior::Startup),
            "Tag-matched startup should take priority over name-matched PassageHeader");
    }

    // -----------------------------------------------------------------------
    // Changer link
    // -----------------------------------------------------------------------

    #[test]
    fn parse_changer_link() {
        let mut plugin = HarlowePlugin::new();
        let src = ":: Start\n(link: \"Click me\")[[Forest]]\n";
        let result = plugin.parse_mut(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(links.iter().any(|l| l.target == "Forest" && l.display_text == Some("Click me".into())));
    }
}
