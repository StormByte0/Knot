//! Core module: Twine Core format plugin.
//!
//! ## Twine Core Format Plugin
//!
//! This plugin provides ONLY the base Twine/Twee engine behavior:
//! - Passage header parsing (`:: Name [tags] {metadata}`)
//! - Link extraction (`[[Target]]`, `[[Display->Target]]`, `[[Display|Target]]`)
//! - Core special passage classification (StoryTitle, StoryData, Start,
//!   [script], [stylesheet], [style])
//! - Basic semantic tokens for passage structure
//!
//! It does NOT provide:
//! - Macro catalogs, completion, or hover (no format-specific macros)
//! - Variable sigils or tracking (no format-specific variable syntax)
//! - Global objects or namespaces
//! - Format-specific diagnostics
//!
//! This ensures that the LSP never overfits to a specific format when the
//! actual story format cannot be determined. Users making new story formats
//! still get core Twine engine highlights and handlers.
//!
//! Link and header recognition go through the shared, regex-free scanners
//! ([`crate::core_links::scan_core_links`] and
//! [`crate::header::is_header_line`]) so the fallback plugin obeys the same
//! span-carrying-parse-result rule as the rest of the codebase.

use knot_core::passage::{Link, Passage, SpecialPassageDef, StoryFormat};
use url::Url;

use crate::core_links::scan_core_links;
use crate::header::{self, TweeHeader};
use crate::plugin::{
    FormatPlugin, FormatPluginMut, ParseResult, PassageTokenGroup, SemanticToken,
    SemanticTokenModifier, SemanticTokenType,
};

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// Twine Core format plugin — base engine fallback.
///
/// Provides passage parsing, link extraction, and core special passage
/// classification only. All format-specific features return empty/default
/// values, ensuring the LSP doesn't overfit to any specific story format.
pub struct TwineCorePlugin;

impl Default for TwineCorePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TwineCorePlugin {
    pub fn new() -> Self {
        Self
    }

    // -----------------------------------------------------------------------
    // Passage splitting (byte-offset tracking)
    // -----------------------------------------------------------------------

    fn split_passages<'a>(&self, text: &'a str) -> Vec<(TweeHeader, &'a str)> {
        let mut results = Vec::new();
        let mut header_spans: Vec<(usize, usize)> = Vec::new();
        let mut byte_offset = 0;

        for line in text.lines() {
            let line_start = byte_offset;
            let line_end = line_start + line.len();

            if header::is_header_line(line) {
                header_spans.push((line_start, line_end));
            }

            // Detect actual newline length: CRLF is 2 bytes, LF is 1 byte.
            // Rust's str::lines() strips both \n and \r\n, so we must check
            // the raw text to know which one was present.
            let newline_len = if text.get(line_end..line_end + 2) == Some("\r\n") {
                2
            } else if line_end < text.len() {
                1
            } else {
                0
            };
            byte_offset = line_end + newline_len;
        }

        for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
            let header_line = &text[header_start..header_end];
            let parsed = header::parse_twee_header(header_line, header_start);

            // Body starts after the header line's newline (CRLF = 2, LF = 1).
            let newline_len = if text.get(header_end..header_end + 2) == Some("\r\n") {
                2
            } else if header_end < text.len() {
                1
            } else {
                0
            };
            let body_start = header_end + newline_len;
            let body_end = if i + 1 < header_spans.len() {
                header_spans[i + 1].0
            } else {
                text.len()
            };
            let body_text = text
                .get(body_start.min(text.len())..body_end.min(text.len()))
                .unwrap_or("");

            if let Some(hdr) = parsed {
                results.push((hdr, body_text));
            }
        }

        results
    }

    // -----------------------------------------------------------------------
    // Link extraction
    // -----------------------------------------------------------------------

    fn extract_links(body_text: &str, body_offset: usize) -> Vec<Link> {
        // The shared span-carrying scanner (see the module docs) — the
        // simple form has no display part; arrow/pipe forms carry one.
        // Span/coverage quirks of the previous regex trio are pinned by
        // the core_links tests.
        scan_core_links(body_text)
            .into_iter()
            .map(|l| Link {
                display_text: l.display(body_text).map(str::to_string),
                target_span: None,
                target: l.target(body_text).to_string(),
                span: (body_offset + l.span.start)..(body_offset + l.span.end),
                edge_type_hint: None,
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Semantic tokens
    // -----------------------------------------------------------------------

    /// Build semantic tokens for a passage header with passage-relative offsets.
    ///
    /// All returned `start` values are relative to the passage head (the `::`
    /// prefix at offset 0 within the passage).
    fn build_passage_tokens(header_line: &str, is_special: bool) -> Vec<SemanticToken> {
        let mut tokens = Vec::new();

        // "::" prefix token — always at passage-relative offset 0
        let prefix_len = if header_line.starts_with("::") { 2 } else { 0 };
        let prefix_type = if is_special {
            SemanticTokenType::SpecialPassageHeader
        } else {
            SemanticTokenType::PassageHeader
        };
        let prefix_modifier = if is_special {
            Some(SemanticTokenModifier::TwineCore)
        } else {
            None
        };
        tokens.push(SemanticToken {
            start: 0,
            length: prefix_len,
            token_type: prefix_type,
            modifier: prefix_modifier,
        });

        // Passage name token — use the unified header parser to find the
        // exact name span. This correctly handles multiple [tags] and
        // {} metadata blocks.
        if let Some(after_colons) = header_line.strip_prefix("::")
            && let Some(name_range) = header::passage_name_range_in_header(after_colons)
        {
            let name_type = if is_special {
                SemanticTokenType::SpecialPassage
            } else {
                SemanticTokenType::PassageName
            };
            tokens.push(SemanticToken {
                start: 2 + name_range.start,
                length: name_range.end - name_range.start,
                token_type: name_type,
                modifier: prefix_modifier,
            });
        }

        tokens
    }
}

// ---------------------------------------------------------------------------
// FormatPlugin implementation
// ---------------------------------------------------------------------------

impl FormatPluginMut for TwineCorePlugin {
    fn parse_mut(&mut self, _uri: &Url, text: &str) -> ParseResult {
        // Twine Core has no mutable registries — body moved to parse_mut below
        let mut passages = Vec::new();
        let mut token_groups = Vec::new();

        let passages_raw = self.split_passages(text);

        for (header, body_text) in &passages_raw {
            let special_def = self.classify_passage(&header.name, &header.tags);
            let is_special = special_def.is_some();
            let passage_head = header.header_start;

            let header_line_end = header.header_start
                + text[header.header_start..]
                    .find('\n')
                    .unwrap_or(text[header.header_start..].len());
            let header_line = &text[header.header_start..header_line_end];

            let mut passage_tokens = Vec::new();
            passage_tokens.extend(Self::build_passage_tokens(header_line, is_special));

            if !header.tags.is_empty() {
                // Compute tag positions relative to the passage head.
                // tags_raw is aligned with name_start, so
                // name_start + find('[') gives the document-absolute bracket
                // position; subtract passage_head for passage-relative.
                let bracket_start_rel = header
                    .tags_raw
                    .find('[')
                    .map(|bs| header.name_start - passage_head + bs)
                    .unwrap_or(header.name_start - passage_head + header.name_text_raw.len());
                let tags_inner_start_rel = bracket_start_rel + 1;
                let mut offset = tags_inner_start_rel;
                for tag in &header.tags {
                    let modifier = self.classify_tag(tag);
                    if offset > tags_inner_start_rel {
                        offset += 1;
                    }
                    passage_tokens.push(SemanticToken {
                        start: offset,
                        length: tag.len(),
                        token_type: SemanticTokenType::Tag,
                        modifier,
                    });
                    offset += tag.len();
                }
            }

            let body_offset = header.header_start
                + text[header.header_start..]
                    .find('\n')
                    .unwrap_or(text[header.header_start..].len())
                + 1;
            for link in Self::extract_links(body_text, body_offset) {
                passage_tokens.push(SemanticToken {
                    start: link.span.start - passage_head,
                    length: link.span.len(),
                    token_type: SemanticTokenType::Link,
                    modifier: None,
                });
            }

            let body_offset_in_passage = body_offset - passage_head;
            let body_blocks =
                crate::core_specials::raw_body_blocks(body_text, body_offset_in_passage);
            let links = Self::extract_links(body_text, body_offset);

            let mut passage = Passage::new(
                header.name.clone(),
                header.header_start
                    ..(header.header_start
                        + text[header.header_start..]
                            .find('\n')
                            .unwrap_or(text[header.header_start..].len())),
            );
            passage.tags = header.tags.clone();
            passage.body = body_blocks;
            passage.links = links;
            passage.vars = Vec::new();
            passage.is_special = is_special;
            passage.special_def = special_def;

            passages.push(passage);
            token_groups.push(PassageTokenGroup {
                passage_name: header.name.clone(),
                passage_offset: passage_head,
                tokens: passage_tokens,
            });
        }

        ParseResult {
            passages,
            token_groups,
            diagnostic_groups: Vec::new(),
            is_complete: true,
        }
    }

    fn parse_passage_mut(
        &mut self,
        passage_name: &str,
        passage_tags: &[String],
        passage_text: &str,
        _file_uri: &str,
        passage_offset: usize,
    ) -> Option<crate::plugin::ParseResult> {
        let special_def = self.classify_passage(passage_name, passage_tags);
        let is_special = special_def.is_some();

        let links = Self::extract_links(passage_text, 0);

        let mut passage = Passage::new(passage_name.to_string(), 0..passage_text.len());
        passage.tags = passage_tags.to_vec();
        passage.body = crate::core_specials::raw_body_blocks(passage_text, 0);
        passage.links = links;
        passage.vars = Vec::new();
        passage.is_special = is_special;
        passage.special_def = special_def;
        passage.passage_offset = passage_offset;

        Some(crate::plugin::ParseResult {
            passages: vec![passage],
            token_groups: Vec::new(),
            diagnostic_groups: Vec::new(),
            is_complete: true,
        })
    }

    fn remove_file_from_registries(&mut self, _file_uri: &str) {}
    fn remove_passage_from_registries(&mut self, _passage_name: &str, _file_uri: &str) {}
}

impl FormatPlugin for TwineCorePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::Core
    }

    fn display_name(&self) -> &str {
        "Twine Core"
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        // Core plugin does NOT define its own special passages —
        // the core special passages (StoryTitle, StoryData, Start, etc.)
        // are provided by `twine_core_special_passages()` which is merged
        // automatically by `all_name_matched_passages()`.
        Vec::new()
    }

    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        // Same — core tags ([script], [stylesheet], [style]) are provided
        // by `twine_core_special_passages()`.
        Vec::new()
    }

    // All behavioral methods use the default (no-op) implementations from
    // FormatPlugin. The Core plugin provides:
    // - No macros → completion, hover, validation return empty
    // - No variable sigils → variable completion returns empty
    // - No global objects → global hover returns None
    // - No syntax detection → find_macro_at_position returns None
    // - No close tags → detect_close_tag_context returns None
    // - No operator normalization
    // - No implicit passage patterns
    // - No dynamic navigation resolution
}
