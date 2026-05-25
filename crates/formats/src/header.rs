//! Unified Twee passage header parser.
//!
//! All format plugins and the LSP server share this single implementation for
//! extracting the passage name, tags, and JSON metadata block from a Twee 3
//! header line.
//!
//! ## Twee 3 Header Format
//!
//! ```text
//! :: PassageName [tag1 tag2] {"position":"100,200","group":"Intro"}
//! ```
//!
//! The format is:
//! 1. `::` prefix (required)
//! 2. Optional whitespace
//! 3. Passage name (up to the first `[` or `{`, or end of line)
//! 4. Zero or more `[tag1 tag2]` tag blocks (space-separated)
//! 5. At most one `{...}` JSON metadata block (for tooling; Tweego ignores
//!    this when compiling)
//!
//! ## Multiple Tag Blocks
//!
//! Some authors write headers with multiple inline tag blocks:
//! ```text
//! :: PassageName [tag1] [tag2 tag3] {"position":"100,200"}
//! ```
//! This parser merges all `[...]` blocks into a single tag list.
//!
//! ## CRLF / LF Handling
//!
//! The parser strips trailing `\r` so it works correctly on both LF and CRLF
//! files, regardless of the editor's line-ending setting.

use std::ops::Range;

/// The result of parsing a Twee 3 passage header line.
///
/// All byte offsets are relative to the start of the full source text
/// (i.e., the `offset` parameter passed to `parse_twee_header`).
#[derive(Debug, Clone)]
pub struct TweeHeader {
    /// The passage name, stripped of tags and metadata.
    pub name: String,
    /// All tags from every `[...]` block on the header line.
    pub tags: Vec<String>,
    /// Byte offset where the header line starts in the source text.
    pub header_start: usize,
    /// Byte offset where the passage name starts (after `::` + whitespace).
    pub name_start: usize,
    /// The raw JSON metadata string (e.g., `{"position":"100,200"}`),
    /// if present. `None` if no `{...}` block was found.
    pub metadata_json: Option<String>,
    /// The text between `::` and the first `[` or `{`, preserving the
    /// original whitespace for accurate name-end offset computation.
    /// Useful for semantic token generation.
    ///
    /// Note: This field does NOT contain `[` or `{` brackets — it is the
    /// text *before* the first tag block. For tag position computation,
    /// use `tags_raw` which preserves the `[tag]` blocks.
    pub name_text_raw: String,
    /// The full text after `::` + whitespace with JSON metadata stripped
    /// but `[tag]` blocks intact. This is the primary field for computing
    /// tag bracket positions in semantic token generation.
    ///
    /// Example: For `:: Forest [dark scary] {"position":"100,200"}`,
    /// `tags_raw = "Forest [dark scary]"`.
    ///
    /// The byte offset of `tags_raw[0]` in the source text equals
    /// `name_start`, so `name_start + tags_raw.find('[')` gives the
    /// absolute byte position of the `[` bracket.
    pub tags_raw: String,
}

/// Parse a Twee 3 passage header line.
///
/// Returns `None` if the line doesn't start with `::` or the passage name
/// is empty.
///
/// `line` is the full header line (including the `::` prefix).
/// `offset` is the byte offset where `line` starts in the source text.
pub fn parse_twee_header(line: &str, offset: usize) -> Option<TweeHeader> {
    let after_colons = line.strip_prefix("::")?;
    let whitespace_len = after_colons.len() - after_colons.trim_start().len();
    // Strip trailing \r for CRLF robustness. VSCode may send either LF or
    // CRLF depending on the file's configured line ending.
    let rest = after_colons.trim_start().trim_end_matches('\r');

    let name_start = offset + 2 + whitespace_len;

    // ── Phase 1: Extract JSON metadata block (at most one) ────────────
    // The metadata block is the LAST `{...}` on the line. Per the Twee 3
    // spec, there is at most one JSON metadata block. Tweego ignores it
    // when compiling — it exists solely for tooling (Twine editor, LSP).
    //
    // We use bracket-counting to find the matching `}` so that nested
    // objects like `{"position":"100,200","data":{"x":1}}` are handled
    // correctly. A simple `rfind('}')` would match the inner `}`.
    let (rest_after_json, metadata_json) = extract_json_block(rest);

    // ── Phase 2: Extract tags from all `[...]` blocks ─────────────────
    // Multiple `[tag1] [tag2 tag3]` blocks are merged into a single list.
    let (name_text, tags) = extract_tags(&rest_after_json);

    let name = name_text.trim().to_string();

    if name.is_empty() {
        return None;
    }

    Some(TweeHeader {
        name,
        tags,
        header_start: offset,
        name_start,
        metadata_json,
        name_text_raw: name_text.to_string(),
        tags_raw: rest_after_json.to_string(),
    })
}

/// Extract the single JSON metadata block from the end of a header line.
///
/// Returns `(rest, Some(json_string))` if a valid `{...}` block was found
/// at the end, or `(rest, None)` otherwise.
///
/// Uses bracket-counting to correctly handle nested braces in the JSON.
///
/// This is the public entry point used by the LSP server's position helpers
/// for consistent JSON block extraction across header parsing and metadata
/// read/write operations.
pub fn extract_json_block_public(text: &str) -> Option<(&str, String)> {
    let (rest, opt) = extract_json_block(text);
    opt.map(|json| (rest, json))
}

/// Internal implementation — returns `(rest, Option<json_string>)`.
fn extract_json_block(text: &str) -> (&str, Option<String>) {
    let trimmed = text.trim_end();

    // Fast path: no `{` at all
    if !trimmed.contains('{') {
        return (text, None);
    }

    // Find the rightmost `{` that starts a balanced JSON block ending at
    // the end of the trimmed text.
    //
    // We scan backwards from the end to find the `{` whose matching `}`
    // reaches the end of the line. This handles nested objects correctly.
    let bytes = trimmed.as_bytes();
    let len = bytes.len();

    // The line must end with `}`
    if bytes.last() != Some(&b'}') {
        return (text, None);
    }

    // Scan backwards from the end, counting brace depth.
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let mut brace_start: Option<usize> = None;

    // Walk from the end looking for the matching `{`
    for i in (0..len).rev() {
        let ch = bytes[i];

        if escape_next {
            escape_next = false;
            continue;
        }

        if in_string {
            if ch == b'\\' {
                escape_next = true;
            } else if ch == b'"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            b'"' => in_string = true,
            b'}' => depth += 1,
            b'{' => {
                depth -= 1;
                if depth == 0 {
                    brace_start = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }

    let Some(start) = brace_start else {
        return (text, None);
    };

    // Validate that it parses as JSON (or at least looks like it could)
    let json_str = &trimmed[start..];

    // Quick validation: try to parse it. If it fails, don't treat it as
    // metadata — it might be content that happens to have braces.
    if serde_json::from_str::<serde_json::Value>(json_str).is_ok() {
        let before = text[..start].trim_end();
        (before, Some(json_str.to_string()))
    } else {
        // Not valid JSON — leave the text as-is
        (text, None)
    }
}

/// Extract all tags from `[...]` blocks in a header line (after JSON
/// metadata has been stripped).
///
/// Multiple tag blocks are supported: `[tag1] [tag2 tag3]` → `["tag1", "tag2", "tag3"]`.
///
/// Returns `(name_text, tags)` where `name_text` is everything before the
/// first `[` (or the whole text if no tags).
fn extract_tags(text: &str) -> (&str, Vec<String>) {
    let mut tags = Vec::new();
    let mut name_end = text.len(); // end of name portion (before first `[`)

    // Scan for all `[...]` blocks. We track the position of the first `[`
    // so we can split the name from the tags.
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut found_first_bracket = false;

    while i < len {
        if bytes[i] == b'[' {
            if !found_first_bracket {
                name_end = i;
                found_first_bracket = true;
            }

            // Find the matching `]`
            let bracket_start = i;
            let mut j = i + 1;
            while j < len && bytes[j] != b']' {
                j += 1;
            }

            if j < len {
                // Extract tags from inside the brackets
                let inner = &text[bracket_start + 1..j];
                for tag in inner.split_whitespace() {
                    let t = tag.to_string();
                    if !t.is_empty() && !tags.contains(&t) {
                        tags.push(t);
                    }
                }
                i = j + 1;
            } else {
                // Unclosed `[` — treat the rest as name
                if !found_first_bracket {
                    name_end = bracket_start;
                }
                break;
            }
        } else {
            i += 1;
        }
    }

    let name_text = &text[..name_end];
    (name_text, tags)
}

/// Lightweight function to extract just the passage name from a header line.
///
/// This is used by the LSP server handlers that don't need tags or metadata.
/// It mirrors the logic of `parse_twee_header` but returns only the name.
pub fn extract_passage_name(header_after_colons: &str) -> String {
    let header = header_after_colons.trim().trim_end_matches('\r');

    // Strip JSON metadata block
    let header = match extract_json_block(header) {
        (rest, Some(_)) => rest,
        (rest, None) => rest,
    };

    // Strip all tag blocks and extract just the name
    let (name_text, _) = extract_tags(header);
    name_text.trim().to_string()
}

/// Find the byte range of the passage name within a header line, for
/// computing `selectionRange` in `documentSymbol`.
///
/// Returns `(name_start_in_line, name_end_in_line)` as byte offsets
/// within the line (after the `::` prefix has been stripped), or `None`
/// if the header can't be parsed.
///
/// The caller is responsible for adding the `::` prefix length and any
/// whitespace offset to get absolute offsets.
pub fn passage_name_range_in_header(after_colons: &str) -> Option<Range<usize>> {
    let rest = after_colons.trim_start().trim_end_matches('\r');
    let ws_len = after_colons.len() - after_colons.trim_start().len();

    // Strip JSON metadata
    let (rest_after_json, _) = extract_json_block(rest);

    // Find where the name ends (before the first `[`)
    let name_end = rest_after_json
        .find('[')
        .unwrap_or(rest_after_json.len());
    let name_text = rest_after_json[..name_end].trim_end();

    if name_text.is_empty() {
        return None;
    }

    // The name starts after whitespace in the after_colons string.
    // after_colons = [whitespace][rest]  where rest = after_colons.trim_start()
    // rest_after_json is rest with metadata stripped from end.
    // The name starts at the trim_start offset within rest_after_json.
    let name_start_in_rest = rest_after_json.len() - rest_after_json.trim_start().len();

    let start = ws_len + name_start_in_rest;
    let end = start + name_text.len();

    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_header() {
        let result = parse_twee_header(":: Start", 0).unwrap();
        assert_eq!(result.name, "Start");
        assert!(result.tags.is_empty());
        assert!(result.metadata_json.is_none());
    }

    #[test]
    fn test_header_with_tags() {
        let result = parse_twee_header(":: Forest [dark scary]", 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["dark", "scary"]);
        assert!(result.metadata_json.is_none());
    }

    #[test]
    fn test_header_with_metadata() {
        let result = parse_twee_header(r#":: Start {"position":"100,200"}"#, 0).unwrap();
        assert_eq!(result.name, "Start");
        assert!(result.tags.is_empty());
        assert_eq!(result.metadata_json.as_deref(), Some(r#"{"position":"100,200"}"#));
    }

    #[test]
    fn test_header_with_tags_and_metadata() {
        let result = parse_twee_header(r#":: Forest [dark scary] {"position":"100,200"}"#, 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["dark", "scary"]);
        assert_eq!(result.metadata_json.as_deref(), Some(r#"{"position":"100,200"}"#));
    }

    #[test]
    fn test_header_with_multiple_tag_blocks() {
        let result = parse_twee_header(":: Forest [dark] [scary cold]", 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["dark", "scary", "cold"]);
    }

    #[test]
    fn test_header_multiple_tags_with_metadata() {
        let result = parse_twee_header(r#":: Forest [dark] [scary] {"position":"100,200","group":"Intro"}"#, 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["dark", "scary"]);
        assert_eq!(result.metadata_json.as_deref(), Some(r#"{"position":"100,200","group":"Intro"}"#));
    }

    #[test]
    fn test_header_crlf() {
        let result = parse_twee_header(":: Start\r", 0).unwrap();
        assert_eq!(result.name, "Start");
    }

    #[test]
    fn test_header_crlf_with_metadata() {
        let result = parse_twee_header(":: Start {\"position\":\"100,200\"}\r", 0).unwrap();
        assert_eq!(result.name, "Start");
        assert!(result.metadata_json.is_some());
    }

    #[test]
    fn test_header_no_colons() {
        assert!(parse_twee_header("Start", 0).is_none());
    }

    #[test]
    fn test_header_empty_name() {
        assert!(parse_twee_header(":: ", 0).is_none());
    }

    #[test]
    fn test_header_metadata_only_no_tags() {
        let result = parse_twee_header(r#":: Coworker {"position":"40,660"}"#, 0).unwrap();
        assert_eq!(result.name, "Coworker");
        assert_eq!(result.metadata_json.as_deref(), Some(r#"{"position":"40,660"}"#));
    }

    #[test]
    fn test_header_metadata_not_json_ignored() {
        // If the braces don't contain valid JSON, they should NOT be stripped
        // (might be content like {invalid)
        let result = parse_twee_header(":: Name {not json}", 0).unwrap();
        // Since {not json} isn't valid JSON, the parser should NOT strip it
        // and instead treat the whole thing as the name
        assert_eq!(result.name, "Name {not json}");
    }

    #[test]
    fn test_header_nested_json_metadata() {
        let result = parse_twee_header(
            r#":: Test {"position":"100,200","data":{"x":1}}"#,
            0,
        ).unwrap();
        assert_eq!(result.name, "Test");
        assert!(result.metadata_json.is_some());
    }

    #[test]
    fn test_extract_passage_name_simple() {
        assert_eq!(extract_passage_name(" Start"), "Start");
    }

    #[test]
    fn test_extract_passage_name_with_tags_and_metadata() {
        assert_eq!(
            extract_passage_name(r#" Forest [dark scary] {"position":"100,200"}"#),
            "Forest"
        );
    }

    #[test]
    fn test_extract_passage_name_crlf() {
        assert_eq!(extract_passage_name(" Start\r"), "Start");
    }

    #[test]
    fn test_passage_name_range() {
        let after = " Start [tag] {}";
        let range = passage_name_range_in_header(after).unwrap();
        assert_eq!(&after[range.clone()], "Start");
    }

    // Regression tests for the specific bugs from the user's report
    #[test]
    fn test_regression_coworker_with_metadata() {
        // Previously parsed as: "::Coworker {\"position\":\"40,660\"}<<setSceneLoc \"hallway\">>"
        // The header line is just: ::Coworker {"position":"40,660"}
        let result = parse_twee_header(r#"::Coworker {"position":"40,660"}"#, 0).unwrap();
        assert_eq!(result.name, "Coworker");
        assert!(result.tags.is_empty());
    }

    #[test]
    fn test_regression_story_data_with_metadata() {
        // Was: "StoryData0\"}009EE-E3E8-4FDB-B87C-2C299B557C78\","
        // This was likely from metadata JSON being included in the name
        let result = parse_twee_header(r#"::StoryData {"position":"0,0"}"#, 0).unwrap();
        assert_eq!(result.name, "StoryData");
    }

    #[test]
    fn test_regression_story_init_with_metadata() {
        // Was: "StoryIni::StoryInit {\"position\":\"40,300\"}===========..."
        let result = parse_twee_header(r#"::StoryInit {"position":"40,300"}"#, 0).unwrap();
        assert_eq!(result.name, "StoryInit");
    }

    // ── Custom tag tests ────────────────────────────────────────────────
    // Custom tags (user-defined tags not in any format's special passage
    // list) must be correctly extracted and preserved through the entire
    // parsing pipeline.

    #[test]
    fn test_custom_tag_alone() {
        let result = parse_twee_header(":: Forest [mysterious]", 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["mysterious"]);
    }

    #[test]
    fn test_custom_tag_mixed_with_special() {
        // [widget] is a SugarCube special tag; [custom] is user-defined
        let result = parse_twee_header(":: MyWidget [widget custom]", 0).unwrap();
        assert_eq!(result.name, "MyWidget");
        assert_eq!(result.tags, vec!["widget", "custom"]);
    }

    #[test]
    fn test_custom_tag_with_metadata() {
        let result = parse_twee_header(
            r#":: Forest [mysterious dark] {"position":"100,200"}"#,
            0,
        ).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["mysterious", "dark"]);
        assert_eq!(result.metadata_json.as_deref(), Some(r#"{"position":"100,200"}"#));
    }

    #[test]
    fn test_custom_tag_multiple_blocks() {
        // [custom1] and [custom2 special] in separate bracket groups
        let result = parse_twee_header(":: Cave [custom1] [custom2 special]", 0).unwrap();
        assert_eq!(result.name, "Cave");
        assert_eq!(result.tags, vec!["custom1", "custom2", "special"]);
    }

    #[test]
    fn test_special_and_custom_tags_multiple_blocks_with_metadata() {
        // Mix of special tags, custom tags, and JSON metadata
        let result = parse_twee_header(
            r#":: Helpers [widget] [custom helpers] {"position":"300,400","group":"Code"}"#,
            0,
        ).unwrap();
        assert_eq!(result.name, "Helpers");
        assert_eq!(result.tags, vec!["widget", "custom", "helpers"]);
        assert!(result.metadata_json.is_some());
    }

    #[test]
    fn test_custom_tag_with_nobr() {
        // [nobr] is a rendering hint, NOT a special tag. It should be
        // preserved as a regular custom tag (not classified as special).
        let result = parse_twee_header(":: Forest [nobr dark]", 0).unwrap();
        assert_eq!(result.name, "Forest");
        assert_eq!(result.tags, vec!["nobr", "dark"]);
    }

    #[test]
    fn test_extract_json_block_public_with_custom_tags() {
        // Ensure the public JSON extraction function works correctly
        // with custom tags present before the JSON block
        let line = r#":: Name [custom1 custom2] {"position":"100,200"}"#;
        let (rest, json_str) = extract_json_block_public(line.trim_end()).unwrap();
        assert_eq!(json_str, r#"{"position":"100,200"}"#);
        assert_eq!(rest, r#":: Name [custom1 custom2]"#);
    }

    #[test]
    fn test_extract_json_block_public_no_json() {
        let line = ":: Name [custom tag]";
        assert!(extract_json_block_public(line.trim_end()).is_none());
    }

    #[test]
    fn test_extract_json_block_public_nested_json() {
        let line = r#":: Name [tag] {"position":"1,2","data":{"nested":true}}"#;
        let (rest, json_str) = extract_json_block_public(line.trim_end()).unwrap();
        assert!(json_str.contains("nested"));
        assert_eq!(rest, ":: Name [tag]");
    }
}
