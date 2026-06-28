//! Position / Range helpers (UTF-16 aware) and passage position parsing.

use knot_core::Workspace;
use lsp_types::*;
use url::Url;

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
    s.chars()
        .map(|c| if (c as u32) < 0x10000 { 1u32 } else { 2u32 })
        .sum()
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
        utf16_count += if (ch as u32) < 0x10000 {
            1usize
        } else {
            2usize
        };
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
            line_count // the \n itself is part of the previous line
        } else {
            line_count - 1 // we're on the last counted line
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
                if let Some(name_range) =
                    knot_formats::header::passage_name_range_in_header(after_colons)
                {
                    // The `::` prefix is 2 UTF-16 code units that must be
                    // included in the character offset — passage_name_range_in_header()
                    // returns offsets relative to after_colons, but LSP positions
                    // are relative to the full line.
                    let prefix_len = utf16_len(&line[..2]);
                    let start_char = prefix_len + utf16_len(&after_colons[..name_range.start]);
                    let end_char =
                        start_char + utf16_len(&after_colons[name_range.start..name_range.end]);
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

/// Parsed passage metadata from the header line's JSON block.
///
/// The JSON block can contain `position`, `group`, and `color` properties,
/// and is extensible for future metadata. Only the last `{...}` block on the
/// line is parsed (matching the Twee 3 specification).
pub(crate) struct PassageMetadata {
    pub position: Option<(f64, f64)>,
    pub group: Option<String>,
    pub color: Option<String>,
    pub size: Option<(f64, f64)>,
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

    // Extract size (Twine convention: "size":"w,h" string, or {"width":w,"height":h} object)
    let size = {
        // Try "size" as a string "w,h"
        if let Some(size_str) = json_val.get("size").and_then(|v| v.as_str()) {
            let parts: Vec<&str> = size_str.split(',').collect();
            if parts.len() == 2 {
                let w = parts[0].trim().parse::<f64>().ok()?;
                let h = parts[1].trim().parse::<f64>().ok()?;
                Some((w, h))
            } else {
                None
            }
        }
        // Try "size" as a JSON object {"width":...,"height":...}
        else if let Some(size_obj) = json_val.get("size").and_then(|v| v.as_object()) {
            let w = size_obj.get("width").and_then(|v| v.as_f64())?;
            let h = size_obj.get("height").and_then(|v| v.as_f64())?;
            Some((w, h))
        }
        // Also try separate "width"/"height" fields (some Twee editors use these)
        else if let (Some(w), Some(h)) = (
            json_val.get("width").and_then(|v| v.as_f64()),
            json_val.get("height").and_then(|v| v.as_f64()),
        ) {
            Some((w, h))
        } else {
            None
        }
    };

    Some(PassageMetadata {
        position,
        group,
        color,
        size,
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
        if let Some((rest, json_str)) =
            knot_formats::header::extract_json_block_public(stripped_line.trim_end())
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_str)
        {
            // Keep the last (rightmost) block's data — it has the
            // most recent position/group/color values.
            if existing_json.is_none() {
                existing_json = Some(parsed);
            }
            // Strip this block and continue scanning for more
            stripped_line = rest;
            continue;
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
        if let Some(obj) = json_val.as_object()
            && obj.is_empty()
        {
            return stripped_line.to_string();
        }

        if let Ok(new_json) = serde_json::to_string(&json_val) {
            return format!("{} {}", stripped_line, new_json);
        }
    }

    // No existing JSON block — create one with provided values
    let mut json_obj = serde_json::Map::new();

    if let Some((x, y)) = position {
        let pos_str = format!("{},{}", format_coord(x), format_coord(y));
        json_obj.insert("position".to_string(), serde_json::Value::String(pos_str));
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
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_buf)
                            {
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
            if let Some(name) = passage.get("name").and_then(|v| v.as_str())
                && let Some(pos) = passage.get("position").and_then(|v| v.as_array())
                && pos.len() >= 2
                && let (Some(x), Some(y)) = (pos[0].as_f64(), pos[1].as_f64())
            {
                positions.insert(name.to_string(), (x, y));
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
        let code_units = if (ch as u32) < 0x10000 {
            1usize
        } else {
            2usize
        };
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

/// Find the variable name at the given cursor position in the text.
///
/// Returns the variable name (with its sigil) if the cursor is on a
/// variable reference, or `None` otherwise. The `sigils` parameter
/// should come from the format plugin's `variable_sigils()` method.
///
/// For sigils that are valid identifier characters (e.g., `_`), a word
/// boundary is required before the sigil to avoid false matches (e.g.,
/// matching `_bar` inside `foo_bar`).
#[allow(dead_code)]
pub(crate) fn find_variable_at_position(
    text: &str,
    position: Position,
    sigils: &[char],
) -> Option<String> {
    let line_idx = position.line as usize;
    let line = text.lines().nth(line_idx)?;
    let char_idx = position.character as usize;

    if sigils.is_empty() {
        return None;
    }

    // Scan backwards from the cursor to find an identifier body
    let mut start = char_idx;
    while start > 0
        && line
            .as_bytes()
            .get(start - 1)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        start -= 1;
    }

    // Check if the character before the identifier is a variable sigil
    if start > 0 {
        let prev_byte = line.as_bytes().get(start - 1)?;
        let prev_char = *prev_byte as char;
        if sigils.contains(&prev_char) {
            // For sigils that are valid inside identifiers (like `_`),
            // require a word boundary before the sigil to avoid false
            // matches (e.g., matching `_bar` inside `foo_bar`).
            if (prev_char.is_alphanumeric() || prev_char == '_') && start >= 2 {
                let before_sigil = line.as_bytes().get(start - 2)?;
                if before_sigil.is_ascii_alphanumeric() || *before_sigil == b'_' {
                    return None; // No word boundary — skip
                }
            }
            let var_name = &line[start..char_idx.max(start)];
            if !var_name.is_empty() {
                return Some(format!("{}{}", prev_char, var_name));
            }
        }
    }

    None
}

/// Find the byte offset of the first line of a passage's body
/// (the line AFTER the `:: Name` header line).
/// Returns 0 if the passage header is not found.
#[allow(dead_code)]
pub(crate) fn find_passage_start_offset(text: &str, passage_name: &str) -> usize {
    let mut offset = 0;
    for line in text.lines() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == passage_name {
                // The header line itself — the body starts after this line
                return offset + line.len() + 1; // +1 for the newline
            }
        }
        offset += line.len() + 1; // +1 for the newline
    }
    0
}

/// Convert a byte offset to an LSP Position, returning None if the
/// offset is out of bounds (unlike the panicking version).
#[allow(dead_code)]
pub(crate) fn byte_offset_to_position_safe(text: &str, byte_offset: usize) -> Option<Position> {
    if byte_offset > text.len() {
        return None;
    }
    Some(byte_offset_to_position(text, byte_offset))
}

// ===========================================================================
// Span-based lookup helpers (use workspace passage data instead of
// re-scanning the source text). These are preferred over the line-based
// versions above because they avoid redundant parsing, correctly handle
// multi-byte characters, and work with arrow/pipe link syntax.
// ===========================================================================

/// Span-based version of [`find_passage_header_range`].
///
/// Uses `workspace.find_passage(name)` to locate the passage and computes
/// the full header line range from `passage.span.start` (start of `::` line
/// to the first newline). Falls back to the line-based implementation when
/// the workspace doesn't have passage data for the given name.
pub(crate) fn find_passage_header_range_span_based(
    text: &str,
    workspace: &Workspace,
    passage_name: &str,
) -> Range {
    if let Some((_doc, passage)) = workspace.find_passage(passage_name) {
        let span_start = passage.abs_offset(passage.span.start).min(text.len());
        let header_end = text[span_start..]
            .find('\n')
            .map(|n| span_start + n)
            .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));
        return byte_range_to_lsp_range(text, &(span_start..header_end));
    }
    // Fallback: line-based scan
    find_passage_header_range(text, passage_name)
}

/// Span-based version of [`find_passage_name_range`].
///
/// Uses `passage.header_name_span` when available (SugarCube), which
/// provides the byte range of just the passage name within the header line.
/// Falls back to the line-based implementation when `header_name_span` is
/// `None` or the passage isn't found in the workspace.
pub(crate) fn find_passage_name_range_span_based(
    text: &str,
    workspace: &Workspace,
    passage_name: &str,
) -> Range {
    if let Some((_doc, passage)) = workspace.find_passage(passage_name)
        && let Some(ref name_span) = passage.header_name_span
    {
        return byte_range_to_lsp_range(text, &passage.abs_range(name_span));
    }
    // header_name_span not available — fall through to line-based
    // Fallback: line-based scan
    find_passage_name_range(text, passage_name)
}

/// Span-based version of [`find_passage_at_position`].
///
/// Instead of checking `line.starts_with("::")`, this checks if the cursor's
/// byte offset falls within any `passage.span` AND is on the header line
/// (between `passage.span.start` and the first newline after it).
///
/// Falls back to the line-based implementation when the workspace doesn't
/// have document data for the given URI.
pub(crate) fn find_passage_at_position_span_based(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
) -> Option<String> {
    if let Some(doc) = workspace.get_document(uri) {
        let byte_offset = position_to_byte_offset(text, position);
        for passage in &doc.passages {
            let span_start = passage.abs_offset(passage.span.start).min(text.len());
            let header_end = text[span_start..]
                .find('\n')
                .map(|n| span_start + n)
                .unwrap_or(passage.abs_offset(passage.span.end).min(text.len()));
            if byte_offset >= span_start && byte_offset <= header_end {
                return Some(passage.name.clone());
            }
        }
        return None;
    }
    // Fallback: line-based scan
    find_passage_at_position(text, position)
}

/// Span-based version of [`find_link_target_at_position`].
///
/// Instead of scanning for `[[`/`]]`, iterates over `passage.links` and
/// checks if the cursor's byte offset falls within any `link.span`. Returns
/// the `link.target` passage name.
///
/// Falls back to the line-based implementation when the workspace doesn't
/// have document data for the given URI.
pub(crate) fn find_link_target_at_position_span_based(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
) -> Option<String> {
    if let Some(doc) = workspace.get_document(uri) {
        let byte_offset = position_to_byte_offset(text, position);
        for passage in &doc.passages {
            for link in &passage.links {
                if passage.span_contains_abs_offset(&link.span, byte_offset) {
                    let target = link.target.trim();
                    if !target.is_empty() {
                        return Some(target.to_string());
                    }
                }
            }
        }
        return None;
    }
    // Fallback: line-based scan
    find_link_target_at_position(text, position)
}

/// Span-based version of [`find_variable_at_position`].
///
/// Instead of scanning backwards for sigils, iterates over `passage.vars`
/// and checks if the cursor's byte offset falls within any `var.span`.
/// Returns the variable name (including its sigil).
///
/// Falls back to the line-based implementation when the workspace doesn't
/// have document data for the given URI.
#[allow(dead_code)]
pub(crate) fn find_variable_at_position_span_based(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
    _sigils: &[char],
) -> Option<String> {
    if let Some(doc) = workspace.get_document(uri) {
        let byte_offset = position_to_byte_offset(text, position);
        for passage in &doc.passages {
            for var in &passage.vars {
                if passage.span_contains_abs_offset(&var.span, byte_offset) {
                    return Some(var.name.clone());
                }
            }
        }
        return None;
    }
    // Fallback: line-based scan
    find_variable_at_position(text, position, _sigils)
}

/// Span-based version of [`find_passage_start_offset`].
///
/// Instead of scanning lines for `::`, uses `passage.span.start` directly
/// from the workspace to find the start of the header line, then computes
/// the body start offset (after the header line's newline).
///
/// Falls back to the line-based implementation when the workspace doesn't
/// have passage data for the given name.
#[allow(dead_code)]
pub(crate) fn find_passage_start_offset_span_based(
    text: &str,
    workspace: &Workspace,
    passage_name: &str,
) -> usize {
    if let Some((_doc, passage)) = workspace.find_passage(passage_name) {
        let span_start = passage.abs_offset(passage.span.start).min(text.len());
        // The body starts after the header line (after the first newline)
        return text[span_start..]
            .find('\n')
            .map(|n| span_start + n + 1)
            .unwrap_or(span_start);
    }
    // Fallback: line-based scan
    find_passage_start_offset(text, passage_name)
}

// ===========================================================================
// Span-based macro & token context helpers
// ===========================================================================
//
// NOTE: The `CompletionContext` enum and `resolve_completion_context_span_based()`
// have been moved to the format plugin (`knot_formats::types::CompletionContext`
// and `FormatPlugin::resolve_completion_context()`). The format plugin owns all
// completion context detection — the handler is just a thin dispatcher.
//
// The remaining functions here are used by hover, go-to-definition, and other
// handlers that need span-based position lookups.

/// Find the `MacroArgRef` at a cursor position, if any.
///
/// Returns the full `MacroArgRef` struct (passage-relative spans) from the
/// passage that contains the cursor, or `None` if the cursor isn't inside
/// any passage-ref arg. This is a focused version of
/// `resolve_completion_context_span_based` that returns the raw data for
/// callers that need the full span details.
#[allow(dead_code)]
pub(crate) fn find_macro_arg_ref_at_position_span_based(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
) -> Option<knot_core::passage::MacroArgRef> {
    let doc = workspace.get_document(uri)?;
    let byte_offset = position_to_byte_offset(text, position);

    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                return Some(arg_ref.clone());
            }
        }
    }
    None
}

/// Find the semantic token at a cursor position.
///
/// Returns the token type and the text it covers, or `None` if no token
/// spans the cursor position. Useful for quick context checks without
/// the full `CompletionContext` enum.
///
/// Used by hover and go-to-definition handlers for span-based token lookup.
#[allow(dead_code)]
pub(crate) fn find_token_at_position_span_based(
    token_groups: &[knot_formats::plugin::PassageTokenGroup],
    text: &str,
    position: Position,
) -> Option<(knot_formats::plugin::SemanticTokenType, String)> {
    // We need the text to convert LSP position to byte offset.
    // If the position is beyond the text, return None.
    let byte_offset = position_to_byte_offset(text, position);

    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            let abs_start = token.start + group_offset;
            let abs_end = abs_start + token.length;
            if byte_offset >= abs_start
                && byte_offset < abs_end
                && abs_start < text.len()
                && abs_end <= text.len()
            {
                return Some((token.token_type, text[abs_start..abs_end].to_string()));
            }
        }
    }
    None
}

/// Build the stack of unclosed block macros at a cursor position using
/// passage data from the workspace.
///
/// This replaces the line-scanning approach in `try_close_tag_completion()`
/// that used `plugin.scan_line_for_macro_events()`. Instead of re-scanning
/// the source text, this function:
///
/// 1. Finds the passage containing the cursor
/// 2. Collects all `Block::Macro` entries from the passage body, sorted by
///    span position
/// 3. Uses the format plugin's `body_macro_names()` to classify which
///    macros are block macros (i.e., have open/close tag pairs)
/// 4. Builds a stack of unclosed open tags up to the cursor position
///
/// Returns the unclosed block macro names in innermost-first order.
#[allow(dead_code)]
pub(crate) fn find_unclosed_block_macros_span_based(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
    body_macro_names: &std::collections::HashSet<&'static str>,
    plugin: &dyn knot_formats::plugin::FormatPlugin,
) -> Vec<String> {
    let Some(doc) = workspace.get_document(uri) else {
        return Vec::new();
    };
    let byte_offset = position_to_byte_offset(text, position);

    for passage in &doc.passages {
        if !passage.contains_abs_offset(byte_offset) {
            continue;
        }

        // Collect macro events from the passage body blocks.
        // Block::Macro entries represent macro invocations with their spans.
        // We also check macro_arg_refs which have open/close information.
        let mut events: Vec<(String, bool, usize)> = Vec::new(); // (name, is_open, abs_byte)

        // Process body blocks for macro invocations
        for block in &passage.body {
            if let knot_core::passage::Block::Macro { name, span, .. } = block
                && body_macro_names.contains(name.as_str())
            {
                let abs_start = passage.abs_offset(span.start);
                // Only include events before the cursor
                if abs_start < byte_offset {
                    events.push((name.clone(), true, abs_start));
                }
            }
        }

        // Also use the format plugin's scan for close tags.
        // We need the text of the passage up to the cursor to find close tags.
        let passage_abs_start = passage.abs_offset(0);
        let passage_text_up_to_cursor =
            &text[passage_abs_start.min(text.len())..byte_offset.min(text.len())];

        // Scan for close tags using the plugin
        for (line_idx, line) in passage_text_up_to_cursor.lines().enumerate() {
            for event in plugin.scan_line_for_macro_events(line, line_idx as u32) {
                if body_macro_names.contains(event.name.as_str()) {
                    events.push((event.name, event.is_open, 0)); // position not needed for stack
                }
            }
        }

        // Sort events by position (for body block events) — close-tag
        // events from scan_line have position 0, which is a rough
        // approximation. For a fully precise implementation, we'd need
        // close-tag spans in the AST. This is good enough for completion.
        events.sort_by_key(|(_, _, pos)| *pos);

        // Build the stack of unclosed open tags
        let mut open_stack: Vec<String> = Vec::new();
        for (name, is_open, _) in &events {
            if *is_open {
                open_stack.push(name.clone());
            } else {
                // Close tag — remove the matching open tag (innermost first)
                for i in (0..open_stack.len()).rev() {
                    if open_stack[i] == *name {
                        open_stack.remove(i);
                        break;
                    }
                }
            }
        }

        // Return innermost-first
        open_stack.reverse();
        return open_stack;
    }

    Vec::new()
}

/// Compute the LSP Range for the passage name when `header_name_span` is
/// not available.
///
/// Extracts the header line from the passage span, then uses the unified
/// header parser (`passage_name_range_in_header`) to locate the name
/// portion. Falls back to the full header line range if the parser fails.
///
/// This avoids using `line_text.find(&name)`, which can match the wrong
/// occurrence when the name appears in tags or metadata.
pub(crate) fn compute_passage_name_range_fallback(
    text: &str,
    passage_span: &std::ops::Range<usize>,
) -> Range {
    let span_start = passage_span.start.min(text.len());
    let line_end = text[span_start..]
        .find('\n')
        .map(|n| span_start + n)
        .unwrap_or(text.len());
    let header_line = &text[span_start..line_end];
    let after_colons = header_line.strip_prefix("::").unwrap_or(header_line);

    if let Some(name_range) = knot_formats::header::passage_name_range_in_header(after_colons) {
        let abs_start = span_start + 2 + name_range.start;
        let abs_end = span_start + 2 + name_range.end;
        byte_range_to_lsp_range(text, &(abs_start..abs_end))
    } else {
        // Final fallback: return the full header line range
        byte_range_to_lsp_range(text, &(span_start..line_end))
    }
}
