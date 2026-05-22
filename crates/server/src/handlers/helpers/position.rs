//! Position / Range helpers (UTF-16 aware) and passage position parsing.

use lsp_types::*;

// ===========================================================================
// UTF-16 helpers
// ===========================================================================

/// Count the number of UTF-16 code units in a string slice.
///
/// LSP specifies that `Position.character` is measured in UTF-16 code
/// units, not bytes or Unicode scalar values.  Basic Multilingual Plane
/// characters (U+0000–U+FFFF) occupy one UTF-16 code unit; supplementary
/// characters (U+10000–U+10FFFF) occupy two (a surrogate pair).
pub(crate) fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| {
        if (c as u32) < 0x10000 { 1u32 } else { 2u32 }
    }).sum()
}

/// Count UTF-16 code units in the first `byte_limit` bytes of `text`.
///
/// Used to convert byte offsets (from string slicing / regex matches) to
/// the UTF-16 character offsets the LSP requires.
pub(crate) fn utf16_len_up_to(text: &str, byte_limit: usize) -> u32 {
    let safe = byte_limit.min(text.len());
    let mut count = 0u32;
    for ch in text[..safe].chars() {
        count += if (ch as u32) < 0x10000 { 1u32 } else { 2u32 };
    }
    count
}

/// Convert a UTF-16 code unit offset on a single line to a byte offset.
///
/// The LSP sends `Position.character` as UTF-16 code units. Before slicing
/// a Rust `&str` (which is UTF-8), this offset must be converted back to
/// bytes.  Without this conversion, using `position.character as usize`
/// as a byte index will produce wrong positions — and can **panic** if
/// the offset falls inside a multi-byte UTF-8 sequence.
pub(crate) fn utf16_to_byte_offset(line: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0usize;
    let mut byte_offset = 0usize;
    for ch in line.chars() {
        if utf16_count >= utf16_offset {
            break;
        }
        utf16_count += if (ch as u32) < 0x10000 { 1usize } else { 2usize };
        byte_offset += ch.len_utf8();
    }
    byte_offset
}

// ===========================================================================
// Byte ↔ LSP Position conversions
// ===========================================================================

/// Convert a byte offset to an LSP Position (0-based line & UTF-16 character).
///
/// The LSP specification requires `character` to be measured in **UTF-16
/// code units**, not bytes. The previous implementation incorrectly used
/// byte offsets, which produced wrong positions for any non-ASCII text
/// (e.g., emoji, CJK characters, or other multi-byte UTF-8 sequences).
pub(crate) fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    let safe_offset = offset.min(text.len());
    let text_before = &text[..safe_offset];

    // Count lines (0-based)
    let line = if text_before.is_empty() {
        0u32
    } else {
        // `.lines()` does not count a trailing empty line after a final `\n`,
        // so we need to handle that case explicitly.
        let line_count = text_before.lines().count() as u32;
        if text_before.ends_with('\n') {
            line_count  // the \n itself is part of the previous line
        } else {
            line_count - 1  // we're on the last counted line
        }
    };

    // Extract the text on the current line up to the offset
    let last_newline = text_before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_text_before_offset = &text[last_newline..safe_offset];

    // Count UTF-16 code units on this line up to the offset
    let character = utf16_len(line_text_before_offset);

    Position { line, character }
}

/// Convert a byte range to an LSP Range.
pub(crate) fn byte_range_to_lsp_range(text: &str, range: &std::ops::Range<usize>) -> Range {
    let start = byte_offset_to_position(text, range.start);
    let end = byte_offset_to_position(text, range.end);
    Range { start, end }
}

// ===========================================================================
// Passage header / position helpers
// ===========================================================================

/// Find the LSP Range for a passage header line.
///
/// Returns a Range covering the full header line with `character` values
/// measured in UTF-16 code units (as required by the LSP specification).
pub(crate) fn find_passage_header_range(text: &str, passage_name: &str) -> Range {
    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == passage_name {
                return Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: utf16_len(line),
                    },
                };
            }
        }
    }
    Range::default()
}

/// Parse just the passage name from a header (the part after `::`).
///
/// Handles the Twee 3 header format:
/// - JSON metadata: `:: Name [tags] {"position":"x,y"}`
/// - Bare headers: `:: Name`
///
/// The stripping order matches the format plugins' `parse_header_line`:
/// first strip `{...}` JSON metadata, then `[...]` tags.
/// This ensures the server's header parsing always produces the same name
/// that the format plugin stored during workspace indexing.
pub(crate) fn parse_passage_name_from_header(header: &str) -> String {
    // Trim trailing \r for CRLF robustness — mirrors the format plugins'
    // parse_header_line() CRLF fix.
    let header = header.trim().trim_end_matches('\r');

    // Strip JSON metadata block first: :: Name [tags] {"position":"100,200"}
    // The metadata must be the last thing on the line and start with '{'.
    // This mirrors the format plugins' parse_header_line() logic.
    let header = if let Some(brace_start) = header.rfind('{') {
        if header.ends_with('}') {
            header[..brace_start].trim()
        } else {
            header
        }
    } else {
        header
    };

    // Strip tag brackets: :: Name [tag1 tag2]
    // Use rfind('[') + ends_with(']') to match the lexer's tag detection,
    // avoiding false matches on '[' characters inside passage names.
    if let Some(bracket_start) = header.rfind('[') {
        if header.ends_with(']') {
            header[..bracket_start].trim().to_string()
        } else {
            header.to_string()
        }
    } else {
        header.to_string()
    }
}

/// Parse the position from a passage header line's JSON metadata block.
///
/// Twee 3 format supports position metadata as a JSON object after tags:
/// `:: Passage Name [tags] {"position":"100,200"}`
///
/// The position is stored in the "position" field as a string "x,y".
/// Some Twee compilers may emit a JSON object `{"x":100,"y":200}` instead.
/// Both formats are supported.
pub(crate) fn parse_passage_position_from_header(line: &str) -> Option<(f64, f64)> {
    // Find the JSON metadata block at the end of the line
    let brace_start = line.rfind('{')?;
    let after_brace = &line[brace_start..];
    if !after_brace.trim_end().ends_with('}') {
        return None;
    }

    // Try to parse as JSON
    let json_val: serde_json::Value = serde_json::from_str(after_brace).ok()?;

    // Try "position" as a string "x,y"
    if let Some(pos_str) = json_val.get("position").and_then(|v| v.as_str()) {
        let parts: Vec<&str> = pos_str.split(',').collect();
        if parts.len() == 2 {
            let x = parts[0].trim().parse::<f64>().ok()?;
            let y = parts[1].trim().parse::<f64>().ok()?;
            return Some((x, y));
        }
    }

    // Try "position" as a JSON object {"x":...,"y":...}
    if let Some(pos_obj) = json_val.get("position").and_then(|v| v.as_object()) {
        let x = pos_obj.get("x").and_then(|v| v.as_f64())?;
        let y = pos_obj.get("y").and_then(|v| v.as_f64())?;
        return Some((x, y));
    }

    None
}

/// Build or update the JSON metadata block in a passage header line with
/// a new position value.
///
/// If the header already has a JSON metadata block, the "position" field
/// is updated. If not, a new `{"position":"x,y"}` block is appended.
///
/// Returns the new header line with the updated position metadata.
pub(crate) fn update_passage_position_in_header(line: &str, x: f64, y: f64) -> String {
    /// Format a coordinate: integer if whole number, otherwise up to 2 decimal places.
    fn format_coord(v: f64) -> String {
        if v.fract() == 0.0 {
            format!("{}", v as i64)
        } else {
            format!("{:.2}", v)
        }
    }

    let pos_str = format!("{},{}", format_coord(x), format_coord(y));

    // Check if there's an existing JSON metadata block
    if let Some(brace_start) = line.rfind('{') {
        let after_brace = &line[brace_start..];
        if after_brace.trim_end().ends_with('}') {
            // Try to parse the existing JSON and update the position field
            if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(after_brace) {
                json_val["position"] = serde_json::Value::String(pos_str.clone());
                if let Ok(new_json) = serde_json::to_string(&json_val) {
                    let before = line[..brace_start].trim_end();
                    return format!("{} {}", before, new_json);
                }
            }
        }
    }

    // No existing JSON block — append a new one
    let trimmed = line.trim_end();
    format!("{} {{\"position\":\"{}\"}}", trimmed, pos_str)
}

// ===========================================================================
// StoryData position extraction
// ===========================================================================

/// Extract passage positions from StoryData JSON body.
///
/// Twine 2's StoryData can contain per-passage position information.
/// The StoryData JSON may include a "passages" array where each entry
/// has a "name" and "position" field (an array of [x, y]).
/// This function parses those positions and adds them to the map.
pub(crate) fn extract_positions_from_storydata(
    text: &str,
    positions: &mut std::collections::HashMap<String, (f64, f64)>,
) {
    // Find the StoryData passage in the text
    let mut in_story_data = false;
    let mut json_start: Option<usize> = None;
    let mut brace_depth: i32 = 0;
    let mut json_buf = String::new();

    for line in text.lines() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == "StoryData" {
                in_story_data = true;
                continue;
            } else if in_story_data {
                // End of StoryData passage
                break;
            }
        } else if in_story_data {
            let trimmed = line.trim();
            if !trimmed.is_empty() && json_start.is_none() {
                json_start = Some(0);
            }
            json_buf.push_str(line);
            json_buf.push('\n');

            // Count braces to detect end of JSON
            for ch in trimmed.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            // Try to parse the JSON
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_buf) {
                                extract_positions_from_storydata_value(&value, positions);
                            }
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Extract positions from a parsed StoryData JSON value.
fn extract_positions_from_storydata_value(
    value: &serde_json::Value,
    positions: &mut std::collections::HashMap<String, (f64, f64)>,
) {
    // StoryData may contain a "passages" array with per-passage metadata
    if let Some(passages) = value.get("passages").and_then(|v| v.as_array()) {
        for passage in passages {
            if let Some(name) = passage.get("name").and_then(|v| v.as_str()) {
                if let Some(pos) = passage.get("position").and_then(|v| v.as_array()) {
                    if pos.len() >= 2 {
                        if let (Some(x), Some(y)) = (
                            pos[0].as_f64(),
                            pos[1].as_f64(),
                        ) {
                            positions.insert(name.to_string(), (x, y));
                        }
                    }
                }
            }
        }
    }
}

// ===========================================================================
// Position-based lookup helpers
// ===========================================================================

/// Find the passage name at a given LSP position.
pub(crate) fn find_passage_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;
    if line_text.starts_with("::") {
        let name = parse_passage_name_from_header(&line_text[2..]);
        Some(name)
    } else {
        None
    }
}

/// Find a link target at a given LSP position.
///
/// The `position.character` is in UTF-16 code units (LSP spec). This
/// function converts the UTF-16 character offset to a byte offset for
/// string slicing, then searches for `[[...]]` links on the line.
pub(crate) fn find_link_target_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;

    // Convert UTF-16 character offset to a byte offset on this line
    let utf16_offset = position.character as usize;
    let mut utf16_count = 0usize;
    let mut byte_offset = 0usize;
    for ch in line_text.chars() {
        if utf16_count >= utf16_offset {
            break;
        }
        let code_units = if (ch as u32) < 0x10000 { 1usize } else { 2usize };
        utf16_count += code_units;
        byte_offset += ch.len_utf8();
    }

    let char_offset = byte_offset;

    let mut search_from = 0;
    while let Some(rel_start) = line_text[search_from..].find("[[") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line_text[abs_start..].find("]]") {
            let content_start = abs_start + 2;
            let content_end = abs_start + rel_end;

            if char_offset >= content_start && char_offset <= content_end {
                let link_text = &line_text[content_start..content_end];
                // Handle both arrow (->) and pipe (|) link syntax
                let target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                let target = target.trim();
                if !target.is_empty() {
                    return Some(target.to_string());
                }
            }
            search_from = abs_start + rel_end + 2;
        } else {
            break;
        }
    }
    None
}
