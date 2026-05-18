//! Passage splitting, header parsing, macro extraction, and body block building.
//!
//! This module contains the first-pass lexer (via `logos`) and the functions
//! that transform raw source text into structured passage data.

use knot_core::passage::Block;

use super::regexes::{RE_MACRO, RE_MACRO_CLOSE};

// ---------------------------------------------------------------------------
// Logos lexer — passage boundary detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, logos::Logos)]
pub(crate) enum TweeToken {
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
pub(crate) struct ParsedHeader {
    pub name: String,
    pub tags: Vec<String>,
    /// Byte offset where the header line starts.
    pub header_start: usize,
    /// Byte length of the header line (including trailing newline if present).
    pub header_len: usize,
    /// Byte offset where the passage name starts (after `::` and any whitespace).
    /// This is an absolute offset into the source text.
    pub name_start: usize,
    /// The (x, y) position of this passage on the Twine canvas, parsed from
    /// the header metadata JSON block (e.g., `{"position":"100,200"}`).
    pub position: Option<(f64, f64)>,
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Parse passage headers from the full source text.
///
/// Returns a list of `(ParsedHeader, body_text)` pairs. The body text is the
/// raw text between the end of this header line and the start of the next
/// header (or end of file).
pub(crate) fn split_passages(text: &str) -> Vec<(ParsedHeader, &str)> {
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
        let parsed = parse_header_line(header_line, header_start);

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

/// Parse a single `:: Name [tags] {"metadata"}` header line.
///
/// Supports the Twee 3 extended header format with optional JSON metadata
/// block after tags. The metadata can include a `"position"` field in the
/// format `"x,y"` (as Twine 2 serialises it) or as a JSON object
/// `{"x":100,"y":200}`.
pub(crate) fn parse_header_line(line: &str, offset: usize) -> Option<ParsedHeader> {
    // Strip the leading `::` and optional whitespace.
    let after_colons = line.strip_prefix("::")?;
    let whitespace_len = after_colons.len() - after_colons.trim_start().len();
    let rest = after_colons.trim_start();

    // The passage name starts at the absolute byte offset of `::` + 2 + whitespace
    let name_start = offset + 2 + whitespace_len;

    // Check for JSON metadata block at the end: `{"position":"100,200"}`
    // The metadata block must be the last thing on the line and start with `{`.
    let (rest_before_json, json_str) = if let Some(brace_start) = rest.rfind('{') {
        if rest.trim_end().ends_with('}') {
            let before = &rest[..brace_start].trim_end();
            let json = &rest[brace_start..];
            (before, Some(json))
        } else {
            (rest, None)
        }
    } else {
        (rest, None)
    };

    // Parse position from JSON metadata block
    let position = json_str.and_then(|json| parse_position_from_metadata(json));

    // Extract tags if present: `Name [tag1 tag2]`
    let (name, tags) = if let Some(bracket_start) = rest_before_json.rfind('[') {
        if rest_before_json.ends_with(']') {
            let name_part = rest_before_json[..bracket_start].trim();
            let tag_part = &rest_before_json[bracket_start + 1..rest_before_json.len() - 1];
            let tags = tag_part
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            (name_part.to_string(), tags)
        } else {
            (rest_before_json.trim().to_string(), Vec::new())
        }
    } else {
        (rest_before_json.trim().to_string(), Vec::new())
    };

    if name.is_empty() {
        return None;
    }

    Some(ParsedHeader {
        name,
        tags,
        header_start: offset,
        header_len: line.len(),
        name_start,
        position,
    })
}

/// Parse a `"position"` value from a passage header metadata JSON block.
///
/// Twine 2 serialises position as a string `"x,y"` (e.g., `"100,200"`).
/// Some Twee compilers may emit a JSON object `{"x":100,"y":200}` instead.
/// Both formats are supported.
fn parse_position_from_metadata(json: &str) -> Option<(f64, f64)> {
    // Try to parse as a JSON value (flexible approach)
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(pos_str) = val.get("position").and_then(|v| v.as_str()) {
            // Format: "position": "x,y"
            let parts: Vec<&str> = pos_str.split(',').collect();
            if parts.len() == 2 {
                let x = parts[0].trim().parse::<f64>().ok()?;
                let y = parts[1].trim().parse::<f64>().ok()?;
                return Some((x, y));
            }
        } else if let Some(pos_obj) = val.get("position").and_then(|v| v.as_object()) {
            // Format: "position": {"x": 100, "y": 200}
            let x = pos_obj.get("x").and_then(|v| v.as_f64())?;
            let y = pos_obj.get("y").and_then(|v| v.as_f64())?;
            return Some((x, y));
        }
    }
    None
}

/// Extract macros from a passage body and produce content blocks.
pub(crate) fn extract_macros(body: &str, body_offset: usize) -> Vec<Block> {
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

/// Build content blocks from the body text, interleaving text and macro
/// blocks without duplication.
///
/// Previous implementation added the entire body as a single `Block::Text`
/// PLUS all macros as `Block::Macro`, causing duplicate content. This
/// version collects macro spans, then creates text blocks only for the
/// gaps between macros (or the whole body if no macros are present).
pub(crate) fn build_body_blocks(body: &str, body_offset: usize, macros: &[Block]) -> Vec<Block> {
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
