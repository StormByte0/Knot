//! Logos lexer and header parsing for SugarCube.
//!
//! Contains the [`TweeToken`] enum and the passage-splitting logic that uses
//! the Logos lexer to detect passage boundaries in twee source files.
//!
//! Header parsing (name, tags, metadata extraction) delegates to the unified
//! `crate::header::parse_twee_header()` so that all format plugins share the
//! same parsing logic for the Twee 3 header format.

use crate::header::{self, TweeHeader};

/// A token produced by the Logos lexer for twee source.
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

/// Parse passage headers from the full source text.
///
/// Returns a list of `(TweeHeader, body_text)` pairs. The body text is the
/// raw text between the end of this header line and the start of the next
/// header (or end of file).
pub(crate) fn split_passages(text: &str) -> Vec<(TweeHeader, &str)> {
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

    let mut results: Vec<(TweeHeader, &str)> = Vec::new();

    for (i, &(header_start, header_end)) in header_spans.iter().enumerate() {
        let mut header_line = &text[header_start..header_end];
        // The Logos regex `::[^\n]*` includes trailing \r on CRLF files.
        // Strip it so that parse_twee_header() receives clean content and
        // body_offset calculation is correct.
        let trailing_cr = header_line.ends_with('\r');
        if trailing_cr {
            header_line = &header_line[..header_line.len() - 1];
        }
        // Adjust header_end to exclude the \r for correct body_offset
        let adjusted_header_end = if trailing_cr { header_end - 1 } else { header_end };
        let parsed = header::parse_twee_header(header_line, header_start);

        // Body starts after the header line (skip trailing newline).
        //
        // The Logos regex `::[^\n]*` matches up to (but not including) `\n`.
        // For LF files, `adjusted_header_end` points at the `\n` character.
        // For CRLF files, it points at the `\r` (since we stripped it above).
        //
        // We must skip past the newline sequence so that `body_text` does NOT
        // include the leading newline. This is critical because `body_offset`
        // in the format plugin's `parse()` method is computed as the position
        // AFTER the newline. If `body_text` includes the newline, every body
        // token's byte offset will be shifted by +1 (LF) or +2 (CRLF).
        let body_start = adjusted_header_end;
        let newline_skip = if text.get(body_start..body_start + 2) == Some("\r\n") {
            2
        } else if body_start < text.len() && text.as_bytes()[body_start] == b'\n' {
            1
        } else {
            0
        };
        let body_content_start = body_start + newline_skip;
        let body_end = if i + 1 < header_spans.len() {
            header_spans[i + 1].0
        } else {
            text.len()
        };
        let body_text = text.get(body_content_start..body_end).unwrap_or("");

        if let Some(hdr) = parsed {
            results.push((hdr, body_text));
        }
    }

    results
}

/// Extract the position from a `TweeHeader`'s metadata JSON block, if present.
///
/// Twine 2 serialises position as a string `"x,y"` (e.g., `"100,200"`).
/// Some Twee compilers may emit a JSON object `{"x":100,"y":200}` instead.
/// Both formats are supported.
///
/// This function is currently unused but retained for the story map
/// visualization feature (passage position in the Twine graph UI).
#[allow(dead_code)]
pub(crate) fn position_from_header(header: &TweeHeader) -> Option<(f64, f64)> {
    let json_str = header.metadata_json.as_ref()?;
    parse_position_from_metadata(json_str)
}

/// Parse a `"position"` value from a passage header metadata JSON block.
///
/// See [`position_from_header`] for the intended use case.
#[allow(dead_code)]
fn parse_position_from_metadata(json: &str) -> Option<(f64, f64)> {
    let val = serde_json::from_str::<serde_json::Value>(json).ok()?;
    if let Some(pos_str) = val.get("position").and_then(|v| v.as_str()) {
        let parts: Vec<&str> = pos_str.split(',').collect();
        if parts.len() == 2 {
            let x = parts[0].trim().parse::<f64>().ok()?;
            let y = parts[1].trim().parse::<f64>().ok()?;
            return Some((x, y));
        }
    } else if let Some(pos_obj) = val.get("position").and_then(|v| v.as_object()) {
        let x = pos_obj.get("x").and_then(|v| v.as_f64())?;
        let y = pos_obj.get("y").and_then(|v| v.as_f64())?;
        return Some((x, y));
    }
    None
}
