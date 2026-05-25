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

/// Convert an LSP Position (line, UTF-16 character) to a byte offset in the text.
///
/// This is the reverse of [`byte_offset_to_position`]. It finds the start of
/// the given line, then converts the UTF-16 `character` offset to a byte
/// offset using [`utf16_to_byte_offset`].
pub(crate) fn position_to_byte_offset(text: &str, pos: Position) -> usize {
    let mut byte_offset = 0;
    let mut current_line = 0u32;

    // Advance to the start of the target line
    for ch in text.chars() {
        if current_line == pos.line {
            break;
        }
        byte_offset += ch.len_utf8();
        if ch == '\n' {
            current_line += 1;
        }
    }

    // If we didn't reach the target line, return end of text
    if current_line < pos.line {
        return text.len();
    }

    // Find the end of the current line (or end of text)
    let line_start = byte_offset;
    let line_end = text[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let line_text = &text[line_start..line_end];

    // Convert UTF-16 character offset to byte offset within the line
    let char_byte_offset = utf16_to_byte_offset(line_text, pos.character as usize);
    line_start + char_byte_offset
}

/// Convert an LSP Range to a byte range in the text.
///
/// Uses [`position_to_byte_offset`] to convert both the start and end
/// positions of the LSP range to byte offsets.
pub(crate) fn lsp_range_to_byte_range(text: &str, range: &Range) -> std::ops::Range<usize> {
    let start = position_to_byte_offset(text, range.start);
    let end = position_to_byte_offset(text, range.end);
    start..end
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

/// Find the LSP Range covering ONLY the passage name (stripped, without
/// tags, metadata, or the `::` prefix) for a given passage.
///
/// This is used by diagnostics to underline just the passage name rather
/// than the entire header line (which includes `[tags]` and `{metadata}`).
/// Underlining the full line makes diagnostics harder to read and can
/// mislead users into thinking the tags or metadata are the problem.
///
/// Falls back to the full header range if the name range cannot be
/// computed (e.g., malformed header).
pub(crate) fn find_passage_name_range(text: &str, passage_name: &str) -> Range {
    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == passage_name {
                let after_colons = &line[2..];
                if let Some(name_range) = knot_formats::header::passage_name_range_in_header(after_colons) {
                    let start_char = utf16_len(&after_colons[..name_range.start]);
                    let end_char = start_char + utf16_len(&after_colons[name_range.start..name_range.end]);
                    return Range {
                        start: Position {
                            line: line_idx as u32,
                            character: start_char,
                        },
                        end: Position {
                            line: line_idx as u32,
                            character: end_char,
                        },
                    };
                }
                // Fallback: return the full header range
                return find_passage_header_range(text, passage_name);
            }
        }
    }
    Range::default()
}

/// Parse just the passage name from a header (the part after `::`).
///
/// Handles the Twee 3 header format:
/// - JSON metadata: `:: Name [tags] {"position":"x,y"}`
/// - Multiple tag blocks: `:: Name [tag1] [tag2]`
/// - Bare headers: `:: Name`
///
/// Delegates to the unified `knot_formats::header::extract_passage_name()`
/// so that the server's header parsing always produces the same name
/// that the format plugin stored during workspace indexing.
pub(crate) fn parse_passage_name_from_header(header: &str) -> String {
    knot_formats::header::extract_passage_name(header)
}

/// Parse the position from a passage header line's JSON metadata block.
///
/// Twee 3 format supports position metadata as a JSON object after tags:
/// `:: Passage Name [tags] {"position":"100,200"}`
///
/// The position is stored in the "position" field as a string "x,y".
/// Some Twee compilers may emit a JSON object `{"x":100,"y":200}` instead.
/// Both formats are supported.
///
/// This is a convenience wrapper around [`parse_passage_metadata_from_header`]
/// that extracts only the position. Callers that also need `group` or
/// `color` should use `parse_passage_metadata_from_header` directly.
#[allow(dead_code)]
pub(crate) fn parse_passage_position_from_header(line: &str) -> Option<(f64, f64)> {
    parse_passage_metadata_from_header(line).and_then(|meta| meta.position)
}

/// Parsed passage metadata from the header line's JSON block.
///
/// The JSON block can contain `position`, `group`, and `color` properties,
/// and is extensible for future metadata. Only the last `{...}` block on the
/// line is parsed (matching the Twee 3 specification).
pub(crate) struct PassageMetadata {
    pub position: Option<(f64, f64)>,
    pub group: Option<String>,
    pub color: Option<String>,
}

/// Parse all known metadata properties from a passage header line's JSON block.
///
/// Twee 3 format: `:: Passage Name [tags] {"position":"100,200","group":"Intro","color":"#ff6600"}`
///
/// Returns `None` if no valid JSON metadata block is found on the line.
pub(crate) fn parse_passage_metadata_from_header(line: &str) -> Option<PassageMetadata> {
    // Use the unified header parser's JSON block extraction. This correctly
    // handles: nested JSON, multiple `[tag]` blocks, custom tags with braces,
    // CRLF line endings, and avoids matching `{...}` that isn't valid JSON.
    //
    // The old `rfind('{')` approach could match braces inside tag text or
    // passage names, and couldn't handle nested objects like
    // `{"position":"1,2","data":{"x":1}}`.
    let (_rest, json_str) = knot_formats::header::extract_json_block_public(line.trim_end())?;
    let json_val: serde_json::Value = serde_json::from_str(&json_str).ok()?;

    // Extract position
    let position = {
        // Try "position" as a string "x,y"
        if let Some(pos_str) = json_val.get("position").and_then(|v| v.as_str()) {
            let parts: Vec<&str> = pos_str.split(',').collect();
            if parts.len() == 2 {
                let x = parts[0].trim().parse::<f64>().ok()?;
                let y = parts[1].trim().parse::<f64>().ok()?;
                Some((x, y))
            } else {
                None
            }
        }
        // Try "position" as a JSON object {"x":...,"y":...}
        else if let Some(pos_obj) = json_val.get("position").and_then(|v| v.as_object()) {
            let x = pos_obj.get("x").and_then(|v| v.as_f64())?;
            let y = pos_obj.get("y").and_then(|v| v.as_f64())?;
            Some((x, y))
        } else {
            None
        }
    };

    // Extract group
    let group = json_val
        .get("group")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract color
    let color = json_val
        .get("color")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(PassageMetadata {
        position,
        group,
        color,
    })
}

/// Build or update the JSON metadata block in a passage header line with
/// a new position value.
///
/// If the header already has a JSON metadata block, the "position" field
/// is updated. If not, a new `{"position":"x,y"}` block is appended.
///
/// Returns the new header line with the updated position metadata.
pub(crate) fn update_passage_position_in_header(line: &str, x: f64, y: f64) -> String {
    update_passage_metadata_in_header(line, Some((x, y)), None, None)
}

/// Build or update the JSON metadata block in a passage header line with
/// position, group, and/or color values.
///
/// This writes the **entire** JSON metadata block, preserving existing
/// properties and updating/adding the ones provided. Per the design spec,
/// every metadata write rewrites the whole block to avoid partial updates.
///
/// - `position`: If provided, sets or updates the "position" field.
/// - `group`: If `Some(Some(v))`, sets the "group" field. If `Some(None)`,
///   removes the "group" field. If `None`, leaves the existing value unchanged.
/// - `color`: Same semantics as `group`.
///
/// Returns the new header line with the updated metadata.
pub(crate) fn update_passage_metadata_in_header(
    line: &str,
    position: Option<(f64, f64)>,
    group: Option<Option<&str>>,
    color: Option<Option<&str>>,
) -> String {
    /// Format a coordinate: integer if whole number, otherwise up to 2 decimal places.
    fn format_coord(v: f64) -> String {
        if v.fract() == 0.0 {
            format!("{}", v as i64)
        } else {
            format!("{:.2}", v)
        }
    }

    // Strip trailing JSON metadata blocks using the unified header parser.
    // This repairs duplication damage from the old race condition where
    // multiple position updates could pile up JSON blocks before
    // open_documents was updated. We keep only the last (most recent)
    // block's data and merge it with the new values being written.
    //
    // Example input:  `:: Name [tags] {"position":"1,2"} {"position":"3,4"}`
    // After stripping: `:: Name [tags]`  +  merged JSON with latest values
    //
    // Using the unified parser ensures we correctly handle:
    // - Nested JSON objects
    // - Custom tags with braces in them (e.g., [tag{stuff}])
    // - Multiple `[tag1] [tag2]` blocks before the JSON
    let mut stripped_line = line;
    let mut existing_json: Option<serde_json::Value> = None;

    // Peel off JSON blocks from right to left, keeping only the last one.
    // The unified parser validates each block as proper JSON before stripping.
    loop {
        if let Some((rest, json_str)) = knot_formats::header::extract_json_block_public(stripped_line.trim_end()) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_str) {
                // Keep the last (rightmost) block's data — it has the
                // most recent position/group/color values.
                if existing_json.is_none() {
                    existing_json = Some(parsed);
                }
                // Strip this block and continue scanning for more
                stripped_line = rest;
                continue;
            }
        }
        break;
    }

    // If we found an existing JSON block, merge the new values into it
    if let Some(mut json_val) = existing_json {
        // Update position if provided
        if let Some((x, y)) = position {
            let pos_str = format!("{},{}", format_coord(x), format_coord(y));
            json_val["position"] = serde_json::Value::String(pos_str);
        }

        // Update group if provided
        if let Some(group_opt) = group {
            match group_opt {
                Some(v) => {
                    json_val["group"] = serde_json::Value::String(v.to_string());
                }
                None => {
                    // Remove group field
                    if let Some(obj) = json_val.as_object_mut() {
                        obj.remove("group");
                    }
                }
            }
        }

        // Update color if provided
        if let Some(color_opt) = color {
            match color_opt {
                Some(v) => {
                    json_val["color"] = serde_json::Value::String(v.to_string());
                }
                None => {
                    // Remove color field
                    if let Some(obj) = json_val.as_object_mut() {
                        obj.remove("color");
                    }
                }
            }
        }

        // Remove empty object — if only "position" remains and it was the
        // only field, we still keep it. But if position is also None and the
        // object is empty, don't write an empty {} block.
        if let Some(obj) = json_val.as_object() {
            if obj.is_empty() {
                return stripped_line.to_string();
            }
        }

        if let Ok(new_json) = serde_json::to_string(&json_val) {
            return format!("{} {}", stripped_line, new_json);
        }
    }

    // No existing JSON block — create one with provided values
    let mut json_obj = serde_json::Map::new();

    if let Some((x, y)) = position {
        let pos_str = format!("{},{}", format_coord(x), format_coord(y));
        json_obj.insert(
            "position".to_string(),
            serde_json::Value::String(pos_str),
        );
    }

    if let Some(Some(v)) = group {
        json_obj.insert(
            "group".to_string(),
            serde_json::Value::String(v.to_string()),
        );
    }

    if let Some(Some(v)) = color {
        json_obj.insert(
            "color".to_string(),
            serde_json::Value::String(v.to_string()),
        );
    }

    if json_obj.is_empty() {
        // Nothing to write — return the line unchanged
        return line.to_string();
    }

    let new_json = serde_json::to_string(&serde_json::Value::Object(json_obj))
        .unwrap_or_else(|_| "{}".to_string());
    let trimmed = line.trim_end();
    format!("{} {}", trimmed, new_json)
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
