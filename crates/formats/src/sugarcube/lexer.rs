//! Logos lexer and header parsing for SugarCube.
//!
//! Contains the [`TweeToken`] enum, [`ParsedHeader`] struct, and the
//! passage-splitting logic that uses the Logos lexer to detect passage
//! boundaries in twee source files.

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

/// The result of parsing a single passage header line.
pub(crate) struct ParsedHeader {
    /// The passage name extracted from the header.
    pub name: String,
    /// Tags parsed from the `[tag1 tag2]` suffix, if any.
    pub tags: Vec<String>,
    /// Byte offset where the header line starts.
    pub header_start: usize,
    /// Byte length of the header line (including trailing newline if present).
    pub header_len: usize,
    /// Byte offset where the passage name starts (after `::` and any whitespace).
    /// This is an absolute offset into the source text.
    pub name_start: usize,
}

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

/// Parse a single `:: Name [tags]` header line.
pub(crate) fn parse_header_line(line: &str, offset: usize) -> Option<ParsedHeader> {
    // Strip the leading `::` and optional whitespace.
    let after_colons = line.strip_prefix("::")?;
    let whitespace_len = after_colons.len() - after_colons.trim_start().len();
    let rest = after_colons.trim_start();

    // The passage name starts at the absolute byte offset of `::` + 2 + whitespace
    let name_start = offset + 2 + whitespace_len;

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
        name_start,
    })
}
