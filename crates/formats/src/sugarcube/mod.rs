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

pub mod macros;

use knot_core::passage::{
    Block, Link, Passage, SpecialPassageBehavior, SpecialPassageDef, StoryFormat, VarKind, VarOp,
};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use url::Url;

use crate::plugin::{
    FormatDiagnostic, FormatDiagnosticSeverity, FormatPlugin, ParseResult, SemanticToken,
    SemanticTokenModifier, SemanticTokenType,
};
use crate::types::{
    GlobalDef, ImplicitPassagePattern, MacroDef, OperatorNormalization, ResolvedNavLink,
    VariableSigilInfo,
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
///
/// Regexes are compiled once using `once_cell::sync::Lazy` rather than
/// per-instance, since they are immutable and identical across all instances.
pub struct SugarCubePlugin;

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Lazy-compiled regexes (compiled once, shared across all instances)
// ---------------------------------------------------------------------------

/// [[Target]] — simple passage link
static RE_LINK_SIMPLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]|>-]+?)\]\]").unwrap());

/// [[Display->Target]] — arrow-style link
static RE_LINK_ARROW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)->([^\]]+?)\]\]").unwrap());

/// [[Display|Target]] — pipe-style link
static RE_LINK_PIPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]]+?)\|([^\]]+?)\]\]").unwrap());

/// $variableName — SugarCube persistent variable reference
static RE_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

/// _variableName — SugarCube temporary/scratch variable reference
static RE_TEMP_VAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"_([A-Za-z][A-Za-z0-9_]*)").unwrap());

/// <<set $var to ...>> — write macro for persistent vars
static RE_SET_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+\$([A-Za-z_][A-Za-z0-9_]*)\s+to\b").unwrap());

/// <<set _var to ...>> — write macro for temporary vars
static RE_SET_TEMP_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<set\s+_([A-Za-z][A-Za-z0-9_]*)\s+to\b").unwrap());

/// <<name ...>> — any open macro
static RE_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+([^>]*?))?>>").unwrap());

/// <</name>> — closing macro tag
static RE_MACRO_CLOSE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap());

/// HTML data-passage attribute — implicit passage reference
static RE_DATA_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"data-passage\s*=\s*["']([^"']+)["']"#).unwrap());

/// Engine.play() — implicit passage reference
static RE_ENGINE_PLAY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*play\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Engine.goto() — implicit passage reference
static RE_ENGINE_GOTO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Engine\s*\.\s*goto\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.get() — implicit passage reference
static RE_STORY_GET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*get\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Story.passage() — implicit passage reference
static RE_STORY_PASSAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"Story\s*\.\s*passage\s*\(\s*["']([^"']+)["']"#).unwrap());

/// Navigation macros with a single passage argument: <<goto "target">>,
/// <<include "target">>, <<display "target">>, <<actions "target">>.
/// Supports both double and single quoted strings.
static RE_NAV_MACRO_SINGLE_ARG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"<<(?:goto|include|display|actions)\s+["']([^"']+)["']"#
    ).unwrap()
});

/// Link/button macros with label + passage arguments:
/// <<link "label" "target">>, <<button "label" "target">>,
/// <<linkappend "label" "target">>, <<linkprepend "label" "target">>,
/// <<linkreplace "label" "target">>, <<click "label" "target">>.
/// Supports both double and single quoted strings.
static RE_NAV_MACRO_LABEL_PASSAGE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"<<(?:link|button|linkappend|linkprepend|linkreplace|click)\s+["'][^"']*["']\s+["']([^"']+)["']"#
    ).unwrap()
});

impl SugarCubePlugin {
    /// Create a new SugarCube plugin instance.
    ///
    /// Regexes are pre-compiled as `Lazy` statics, so this is essentially free.
    pub fn new() -> Self {
        Self
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
        for caps in RE_LINK_ARROW.captures_iter(body) {
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
        for caps in RE_LINK_PIPE.captures_iter(body) {
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
        let arrow_pipe_spans: Vec<Range<usize>> = RE_LINK_ARROW
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
        for caps in RE_SET_MACRO.captures_iter(body) {
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
        for caps in RE_SET_TEMP_MACRO.captures_iter(body) {
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
        for caps in RE_VAR.captures_iter(body) {
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
        for caps in RE_TEMP_VAR.captures_iter(body) {
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
        for caps in RE_MACRO.captures_iter(body) {
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
        for caps in RE_MACRO_CLOSE.captures_iter(body) {
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
        for caps in RE_MACRO.captures_iter(body) {
            let m = caps.get(0).unwrap();
            tokens.push(SemanticToken {
                start: body_offset + m.start(),
                length: m.end() - m.start(),
                token_type: SemanticTokenType::Macro,
                modifier: None,
            });
        }
        for caps in RE_MACRO_CLOSE.captures_iter(body) {
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
        for caps in RE_SET_MACRO.captures_iter(body) {
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

        for caps in RE_VAR.captures_iter(body) {
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
    ///
    /// These passages are invoked automatically by the SugarCube engine at
    /// specific lifecycle points — they are never reached via normal
    /// `[[links]]`, so they must be exempted from reachability analysis.
    ///
    /// The definitions here must cover **every** name listed in
    /// `special_passage_names()` and `system_passage_names()` (in
    /// `macros.rs`) to ensure consistent `is_special` flagging during
    /// parsing. Any name that appears in those sets but not here will
    /// be treated as a normal passage and flagged as unreachable.
    fn special_passage_defs() -> Vec<SpecialPassageDef> {
        vec![
            // ── Startup ────────────────────────────────────────────────
            // StoryInit runs before the first passage is rendered.
            // Most variable initialisation happens here ($var = …).
            SpecialPassageDef {
                name: "StoryInit".into(),
                behavior: SpecialPassageBehavior::Startup,
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: Some(0),
            },
            // ── Metadata ───────────────────────────────────────────────
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
            // ── Chrome / UI passages ───────────────────────────────────
            // These render as part of the story interface, not the narrative.
            SpecialPassageDef {
                name: "StoryCaption".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(100),
            },
            SpecialPassageDef {
                name: "StoryBanner".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(95),
            },
            SpecialPassageDef {
                name: "StorySubtitle".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(96),
            },
            SpecialPassageDef {
                name: "StoryAuthor".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(97),
            },
            SpecialPassageDef {
                name: "StoryDisplayTitle".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(98),
            },
            SpecialPassageDef {
                name: "StoryMenu".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(101),
            },
            SpecialPassageDef {
                name: "StoryShare".into(),
                behavior: SpecialPassageBehavior::Chrome,
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: Some(102),
            },
            // StoryInterface replaces the default UI layout entirely.
            SpecialPassageDef {
                name: "StoryInterface".into(),
                behavior: SpecialPassageBehavior::Custom("StoryInterface".into()),
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
            },
            // ── System passages (script / style) ───────────────────────
            // "Story JavaScript" and "Story Stylesheet" are system-level
            // passages that inject JS/CSS into the compiled story. They
            // never participate in narrative flow.
            SpecialPassageDef {
                name: "Story JavaScript".into(),
                behavior: SpecialPassageBehavior::Custom("StoryJavaScript".into()),
                contributes_variables: true,
                participates_in_graph: false,
                execution_priority: None,
            },
            SpecialPassageDef {
                name: "Story Stylesheet".into(),
                behavior: SpecialPassageBehavior::Custom("StoryStylesheet".into()),
                contributes_variables: false,
                participates_in_graph: false,
                execution_priority: None,
            },
            // ── Passage lifecycle ──────────────────────────────────────
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

    /// Comprehensive validation: check for common SugarCube errors.
    ///
    /// This includes:
    /// - Unclosed macro brackets
    /// - Unclosed link brackets
    /// - Structural validation (macro parent constraints)
    /// - Unknown macro detection
    /// - Deprecated macro warnings
    /// - Implicit passage reference detection
    fn validate(&self, body: &str, body_offset: usize) -> Vec<FormatDiagnostic> {
        let mut diagnostics = Vec::new();

        // ── Unclosed macro brackets ──────────────────────────────────────
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

        if depth > 0
            && let Some(pos) = open_pos {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed macro `<<` — missing `>>`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-unclosed-macro".into(),
                });
            }

        // ── Unclosed link brackets ──────────────────────────────────────
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

        if link_depth > 0
            && let Some(pos) = link_open {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + pos..body_offset + pos + 2,
                    message: "Unclosed link `[[` — missing `]]`".into(),
                    severity: FormatDiagnosticSeverity::Warning,
                    code: "sc-broken-link".into(),
                });
            }

        // ── Structural validation: macro parent constraints ──────────────
        // Build a stack of open macros and validate that child macros
        // appear only inside their valid parent containers.
        let constraints = macros::structural_constraints();
        let deprecated = macros::deprecated_macros();
        let known_macros = macros::known_macro_names();

        let mut open_stack: Vec<(&str, usize)> = Vec::new(); // (name, byte_offset_of_<<)

        // Process open macros: <<name ...>> or <<name>>
        for caps in RE_MACRO.captures_iter(body) {
            let m = caps.get(0).unwrap();
            let name = caps.get(1).unwrap().as_str();

            // Check for deprecated macros
            if let Some(msg) = deprecated.get(name) {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + m.start()..body_offset + m.end(),
                    message: format!("Deprecated macro: {}", msg),
                    severity: FormatDiagnosticSeverity::Info,
                    code: "sc-deprecated-macro".into(),
                });
            }

            // Check for unknown macros
            if !known_macros.contains(name) && !name.starts_with('/') {
                diagnostics.push(FormatDiagnostic {
                    range: body_offset + m.start()..body_offset + m.end(),
                    message: format!("Unknown SugarCube macro `<<{}>>`", name),
                    severity: FormatDiagnosticSeverity::Hint,
                    code: "sc-unknown-macro".into(),
                });
            }

            // Validate structural constraints
            if let Some(valid_parents) = constraints.get(name) {
                let has_valid_parent = open_stack.iter().rev().any(|(parent, _)| {
                    valid_parents.contains(parent)
                });
                if !has_valid_parent {
                    let parent_list: Vec<String> = valid_parents
                        .iter()
                        .map(|p| format!("`<<{}>>`", p))
                        .collect();
                    diagnostics.push(FormatDiagnostic {
                        range: body_offset + m.start()..body_offset + m.end(),
                        message: format!(
                            "`<<{}>>` must be inside {}",
                            name,
                            parent_list.join(" or ")
                        ),
                        severity: FormatDiagnosticSeverity::Error,
                        code: "sc-container-structure".into(),
                    });
                }
            }

            // Push to open stack (only for block macros that can contain children)
            let is_block = macros::is_block_macro(name);
            if is_block {
                open_stack.push((name, m.start()));
            }
        }

        // Process close macros: <</name>> — pop from open stack
        for caps in RE_MACRO_CLOSE.captures_iter(body) {
            let name = caps.get(1).unwrap().as_str();

            // Find and pop the matching open tag from the stack
            for idx in (0..open_stack.len()).rev() {
                if open_stack[idx].0 == name {
                    open_stack.remove(idx);
                    break;
                }
            }
        }

        diagnostics
    }

    /// Extract implicit passage references from raw text/HTML/JS.
    ///
    /// Detects patterns like `data-passage="..."`, `Engine.play("...")`,
    /// `Story.get("...")` that reference passages but aren't standard
    /// `[[links]]` or `<<macro>>` passage-args.
    fn extract_implicit_passage_refs(&self, body: &str, body_offset: usize) -> Vec<Link> {
        let mut links = Vec::new();

        // All regexes are Lazy statics — compiled once, reused across all calls.
        let patterns: &[&Lazy<Regex>] = &[
            &RE_DATA_PASSAGE,
            &RE_ENGINE_PLAY,
            &RE_ENGINE_GOTO,
            &RE_STORY_GET,
            &RE_STORY_PASSAGE,
        ];

        for re in patterns {
            for caps in re.captures_iter(body) {
                if let Some(target_match) = caps.get(1) {
                    let full_match = caps.get(0).unwrap();
                    let target = target_match.as_str().trim().to_string();
                    if !target.is_empty() {
                        links.push(Link {
                            display_text: None,
                            target,
                            span: body_offset + full_match.start()..body_offset + full_match.end(),
                        });
                    }
                }
            }
        }

        links
    }

    /// Extract passage references from navigation macros with literal
    /// string arguments.
    ///
    /// SugarCube has two categories of macro-based navigation:
    ///
    /// 1. **Single-arg navigation** — the first (and only) quoted string
    ///    is the passage name:
    ///    - `<<goto "target">>`
    ///    - `<<include "target">>`
    ///    - `<<display "target">>`  (deprecated)
    ///    - `<<actions "target">>`
    ///
    /// 2. **Label-then-passage navigation** — the first quoted string is
    ///    a display label, the second is the passage name:
    ///    - `<<link "label" "target">>`
    ///    - `<<button "label" "target">>`
    ///    - `<<linkappend "label" "target">>`
    ///    - `<<linkprepend "label" "target">>`
    ///    - `<<linkreplace "label" "target">>`
    ///    - `<<click "label" "target">>`  (deprecated)
    ///
    /// Variable-based navigation (e.g., `<<goto $var>>`) is handled
    /// separately by `resolve_dynamic_navigation_links()` during graph
    /// building.
    fn extract_macro_passage_refs(&self, body: &str, body_offset: usize) -> Vec<Link> {
        let mut links = Vec::new();

        // Single-arg navigation macros: <<goto "target">>, etc.
        for caps in RE_NAV_MACRO_SINGLE_ARG.captures_iter(body) {
            if let Some(target_match) = caps.get(1) {
                let full_match = caps.get(0).unwrap();
                let target = target_match.as_str().trim().to_string();
                if !target.is_empty() {
                    links.push(Link {
                        display_text: None,
                        target,
                        span: body_offset + full_match.start()..body_offset + full_match.end(),
                    });
                }
            }
        }

        // Label-then-passage macros: <<link "label" "target">>, etc.
        for caps in RE_NAV_MACRO_LABEL_PASSAGE.captures_iter(body) {
            if let Some(target_match) = caps.get(1) {
                let full_match = caps.get(0).unwrap();
                let target = target_match.as_str().trim().to_string();
                if !target.is_empty() {
                    links.push(Link {
                        display_text: None,
                        target,
                        span: body_offset + full_match.start()..body_offset + full_match.end(),
                    });
                }
            }
        }

        links
    }

    /// Build content blocks from the body text, interleaving text and macro
    /// blocks without duplication.
    ///
    /// Previous implementation added the entire body as a single `Block::Text`
    /// PLUS all macros as `Block::Macro`, causing duplicate content. This
    /// version collects macro spans, then creates text blocks only for the
    /// gaps between macros (or the whole body if no macros are present).
    fn build_body_blocks(&self, body: &str, body_offset: usize, macros: &[Block]) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();

        if macros.is_empty() {
            // No macros — the entire body is a single text block
            if !body.trim().is_empty() {
                blocks.push(Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                });
            }
            return blocks;
        }

        // Collect macro spans so we can identify the gaps (non-macro text)
        let mut macro_spans: Vec<std::ops::Range<usize>> = macros
            .iter()
            .filter_map(|m| match m {
                Block::Macro { span, .. } => Some(span.start - body_offset..span.end - body_offset),
                _ => None,
            })
            .collect();

        // Sort by start position
        macro_spans.sort_by_key(|s| s.start);

        // Build blocks: text gaps + macros, in source order
        let mut cursor: usize = 0;
        let mut macro_idx: usize = 0;

        while macro_idx < macro_spans.len() {
            let mspan = &macro_spans[macro_idx];

            // Add text block for the gap before this macro (if non-empty)
            if cursor < mspan.start {
                let gap = &body[cursor..mspan.start];
                if !gap.trim().is_empty() {
                    blocks.push(Block::Text {
                        content: gap.to_string(),
                        span: body_offset + cursor..body_offset + mspan.start,
                    });
                }
            }

            // Add the macro block itself
            if let Some(macro_block) = macros.get(macro_idx) {
                blocks.push(macro_block.clone());
            }

            cursor = mspan.end;
            macro_idx += 1;
        }

        // Add trailing text after the last macro
        if cursor < body.len() {
            let trailing = &body[cursor..];
            if !trailing.trim().is_empty() {
                blocks.push(Block::Text {
                    content: trailing.to_string(),
                    span: body_offset + cursor..body_offset + body.len(),
                });
            }
        }

        // If no blocks were created (all macros but no text gaps), just add macros
        if blocks.is_empty() && !macros.is_empty() {
            blocks.extend_from_slice(macros);
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
            // Also extract implicit passage references (data-passage, Engine.play, etc.)
            passage.links.extend(self.extract_implicit_passage_refs(body, body_offset));
            // Also extract passage references from navigation macros
            // (<<link "label" "target">>, <<goto "target">>, etc.)
            passage.links.extend(self.extract_macro_passage_refs(body, body_offset));
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

    // -------------------------------------------------------------------
    // Macro catalog (behavioral overrides)
    // -------------------------------------------------------------------

    fn builtin_macros(&self) -> &'static [MacroDef] {
        macros::builtin_macros()
    }

    fn block_macro_names(&self) -> HashSet<&'static str> {
        macros::block_macro_names()
    }

    fn passage_arg_macro_names(&self) -> HashSet<&'static str> {
        macros::passage_arg_macro_names()
    }

    fn label_then_passage_macros(&self) -> HashSet<&'static str> {
        macros::label_then_passage_macros()
    }

    fn variable_assignment_macros(&self) -> HashSet<&'static str> {
        macros::variable_assignment_macros()
    }

    fn macro_definition_macros(&self) -> HashSet<&'static str> {
        macros::macro_definition_macros()
    }

    fn inline_script_macros(&self) -> HashSet<&'static str> {
        macros::inline_script_macros()
    }

    fn dynamic_navigation_macros(&self) -> HashSet<&'static str> {
        macros::dynamic_navigation_macros()
    }

    fn find_macro(&self, name: &str) -> Option<&'static MacroDef> {
        macros::find_macro(name)
    }

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        macros::build_macro_snippet(name, has_body)
    }

    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        macros::macro_parent_constraints()
    }

    fn get_passage_arg_index(&self, macro_name: &str, arg_count: usize) -> i32 {
        macros::get_passage_arg_index(macro_name, arg_count)
    }

    // -------------------------------------------------------------------
    // Special passages (extended)
    // -------------------------------------------------------------------

    fn special_passage_names(&self) -> HashSet<&'static str> {
        macros::special_passage_names()
    }

    fn system_passage_names(&self) -> HashSet<&'static str> {
        macros::system_passage_names()
    }

    // -------------------------------------------------------------------
    // Variable tracking
    // -------------------------------------------------------------------

    fn variable_sigils(&self) -> Vec<VariableSigilInfo> {
        macros::variable_sigils()
    }

    fn describe_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::describe_variable_sigil(sigil)
    }

    fn resolve_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::resolve_variable_sigil(sigil)
    }

    fn assignment_operators(&self) -> Vec<&'static str> {
        macros::assignment_operators()
    }

    fn comparison_operators(&self) -> Vec<&'static str> {
        macros::comparison_operators()
    }

    // -------------------------------------------------------------------
    // Implicit passage references
    // -------------------------------------------------------------------

    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        macros::implicit_passage_patterns()
    }

    // -------------------------------------------------------------------
    // Dynamic navigation resolution
    // -------------------------------------------------------------------

    fn build_var_string_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        // SugarCube-specific: scan <<set $var to "literal">> patterns
        let re_set_string = regex::Regex::new(
            r#"<<set\s+([\$][A-Za-z_][A-Za-z0-9_]*)\s+to\s+"([^"]*)""#
        ).unwrap();

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for doc in workspace.documents() {
            for passage in &doc.passages {
                for block in &passage.body {
                    let content = match block {
                        knot_core::passage::Block::Text { content, .. } => content.as_str(),
                        knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                        _ => continue,
                    };
                    for caps in re_set_string.captures_iter(content) {
                        if let (Some(var_match), Some(val_match)) = (caps.get(1), caps.get(2)) {
                            let var_name = var_match.as_str().to_string();
                            let string_val = val_match.as_str().to_string();
                            map.entry(var_name).or_default().push(string_val);
                        }
                    }
                }
            }
        }
        for values in map.values_mut() {
            values.sort();
            values.dedup();
        }
        map
    }

    fn resolve_dynamic_navigation_links(
        &self,
        passage: &Passage,
        var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        // SugarCube-specific: resolve <<goto $var>>, <<include $var>>, <<link "label" $var>>, <<button "label" $var>>
        let re_nav_var = regex::Regex::new(
            r#"<<(?:goto|include|link|button)\s+(?:"[^"]*"\s+)?([\$][A-Za-z_][A-Za-z0-9_]*)"#
        ).unwrap();

        let mut links = Vec::new();
        for block in &passage.body {
            let content = match block {
                knot_core::passage::Block::Text { content, .. } => content.as_str(),
                knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                _ => continue,
            };
            for caps in re_nav_var.captures_iter(content) {
                if let Some(var_match) = caps.get(1) {
                    let var_name = var_match.as_str().to_string();
                    if let Some(known_values) = var_string_map.get(&var_name) {
                        for value in known_values {
                            links.push(ResolvedNavLink {
                                display_text: Some(format!("{} (via {})", value, var_name)),
                                target: value.clone(),
                            });
                        }
                    }
                }
            }
        }
        links
    }

    // -------------------------------------------------------------------
    // Hover / documentation
    // -------------------------------------------------------------------

    fn global_hover_text(&self, name: &str) -> Option<&'static str> {
        macros::global_hover_text(name)
    }

    fn builtin_globals(&self) -> &'static [GlobalDef] {
        macros::builtin_globals()
    }

    fn global_object_names(&self) -> HashSet<&'static str> {
        macros::builtin_globals().iter().map(|g| g.name).collect()
    }

    // -------------------------------------------------------------------
    // Operator normalization
    // -------------------------------------------------------------------

    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        macros::operator_normalization()
    }

    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        macros::operator_precedence()
    }

    // -------------------------------------------------------------------
    // Script/stylesheet tags
    // -------------------------------------------------------------------

    fn script_tags(&self) -> Vec<&'static str> {
        macros::script_tags()
    }

    fn stylesheet_tags(&self) -> Vec<&'static str> {
        macros::stylesheet_tags()
    }

    // -------------------------------------------------------------------
    // Macro snippet mapping
    // -------------------------------------------------------------------

    fn macro_snippet(&self, name: &str) -> Option<&'static str> {
        macros::macro_snippet(name)
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

    #[test]
    fn structural_validation_else_without_if() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<else>>Some text\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<else>> outside <<if>>"
        );
    }

    #[test]
    fn structural_validation_break_without_for() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<break>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<break>> outside <<for>>"
        );
    }

    #[test]
    fn structural_validation_else_inside_if_ok() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<if $x>><<else>>OK<</if>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "<<else>> inside <<if>> should not trigger structural validation"
        );
    }

    #[test]
    fn deprecated_macro_warning() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<click \"label\" \"target\">>Click<</click>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-deprecated-macro"),
            "Should detect deprecated <<click>> macro"
        );
    }

    #[test]
    fn unknown_macro_hint() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<foobar>>test<</foobar>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-unknown-macro"),
            "Should detect unknown <<foobar>> macro"
        );
    }

    #[test]
    fn implicit_passage_ref_data_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<a data-passage=\"Forest\">Go</a>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect data-passage implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_engine_play() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>Engine.play(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Engine.play() implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_story_get() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>var p = Story.get(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Story.get() implicit reference"
        );
    }
}
