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

    // Compute the 0-based line index of every header in one pass, so
    // each TweeHeader can carry its line for the Passage model (Story Map
    // navigation). Headers are already in document order.
    let header_lines = header::header_line_indices(text, &header_spans);

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
        let adjusted_header_end = if trailing_cr {
            header_end - 1
        } else {
            header_end
        };
        let mut parsed = header::parse_twee_header(header_line, header_start);
        if let Some(hdr) = parsed.as_mut() {
            hdr.line = header_lines[i];
        }

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

// header_line_indices lives in crate::header (shared with the Harlowe
// tokenizer splitter).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_passages_carries_line_indices_and_metadata() {
        // LF file:
        //   line 0: :: A {"position":"1,2"}
        //   line 1: body
        //   line 2: :: B
        //   line 3: more
        //   line 4: :: C           (last line, no trailing newline)
        let text = ":: A {\"position\":\"1,2\"}\nbody\n:: B\nmore\n:: C";
        let parts = split_passages(text);

        let lines: Vec<u32> = parts.iter().map(|(h, _)| h.line).collect();
        assert_eq!(lines, vec![0, 2, 4]);
        assert_eq!(parts[0].0.name, "A");
        assert_eq!(
            parts[0].0.metadata_json.as_deref(),
            Some(r#"{"position":"1,2"}"#)
        );
        assert_eq!(parts[1].0.name, "B");
        assert_eq!(parts[2].0.name, "C");
    }

    #[test]
    fn split_passages_line_indices_crlf() {
        // CRLF file: every header line still gets its 0-based index.
        let text = ":: A\r\nbody\r\n:: B\r\nmore\r\n";
        let parts = split_passages(text);
        let lines: Vec<u32> = parts.iter().map(|(h, _)| h.line).collect();
        assert_eq!(lines, vec![0, 2]);
    }

    #[test]
    fn split_passages_body_slices_unchanged() {
        // The line-index addition must not perturb body slicing.
        let text = ":: A\nhello\n:: B\nworld\n";
        let parts = split_passages(text);
        assert_eq!(parts[0].1, "hello\n");
        assert_eq!(parts[1].1, "world\n");
    }
}
