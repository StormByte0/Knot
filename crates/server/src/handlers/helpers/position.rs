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
pub(crate) fn parse_passage_name_from_header(header: &str) -> String {
    let header = header.trim();
    // Strip angle-bracket position metadata: :: Name [tags] <x,y>
    let header = if let Some(angle_start) = header.find('<') {
        header[..angle_start].trim()
    } else {
        header.trim()
    };
    if let Some(bracket_start) = header.find('[') {
        header[..bracket_start].trim().to_string()
    } else {
        header.to_string()
    }
}

/// Parse the position from a passage header line.
///
/// Twee 3 format supports position metadata in angle brackets:
/// `:: Passage Name [tags] <100,200>`
///
/// The angle brackets appear after the tags (if any) and contain
/// the x,y coordinates separated by a comma.
pub(crate) fn parse_passage_position(line: &str) -> Option<(f64, f64)> {
    // Find the last <...> on the line (position comes after tags)
    let last_angle_start = line.rfind('<')?;
    let after_angle = &line[last_angle_start + 1..];

    // Find the closing >
    let angle_end = after_angle.find('>')?;
    let content = &after_angle[..angle_end];

    // Parse "x,y" or "x, y"
    let parts: Vec<&str> = content.split(',').collect();
    if parts.len() != 2 {
        return None;
    }

    let x = parts[0].trim().parse::<f64>().ok()?;
    let y = parts[1].trim().parse::<f64>().ok()?;
    Some((x, y))
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
