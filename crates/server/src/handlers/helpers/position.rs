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
    // Clamp to the text, then snap DOWN to the nearest char boundary.
    // Callers derive byte offsets from several sources (UTF-16 conversions,
    // cached spans, test harnesses); a mid-UTF-8-sequence offset would
    // panic the slices below. Flooring is the safe, monotonic choice —
    // the position of the character that *contains* the offset (mirrors
    // the nightly-only `str::floor_char_boundary`). Note `len()` is
    // always a boundary, so the loop terminates at 0 in the worst case.
    let mut safe_offset = offset.min(text.len());
    while !text.is_char_boundary(safe_offset) {
        safe_offset -= 1;
    }
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

/// Build or update the JSON metadata block in a passage header line with
/// position, group, and/or color values.
///
/// If the header already has a JSON metadata block, the "position" field
/// is updated. If not, a new `{"position":"x,y"}` block is appended.
///
/// Returns the new header line with the updated position metadata.
pub(crate) fn update_passage_position_in_header(line: &str, x: f64, y: f64) -> String {
    update_passage_metadata_in_header(line, Some((x, y)), None, None)
}

/// Legacy three-field metadata writer — see
/// [`update_passage_header`] for the full editor. Kept for the position
/// drag-save flow (`knot/updatePositions`); delegates with only the
/// requested fields so tags/size/reachable are preserved untouched.
pub(crate) fn update_passage_metadata_in_header(
    line: &str,
    position: Option<(f64, f64)>,
    group: Option<Option<&str>>,
    color: Option<Option<&str>>,
) -> String {
    update_passage_header(
        line,
        HeaderEdits {
            position,
            group,
            color,
            ..Default::default()
        },
    )
    .unwrap_or_else(|_| line.to_string())
}

/// Field-level edits for [`update_passage_header`].
///
/// Every field uses the same three-state semantics as the JSON writer:
/// - `None` → leave unchanged
/// - `Some(None)` → remove the field
/// - `Some(Some(v))` → set/replace the field
///
/// `tags` is two-state: `None` keeps the existing tag blocks verbatim,
/// `Some(list)` replaces them wholesale (`Some([])` removes all tags).
#[derive(Default)]
pub(crate) struct HeaderEdits<'a> {
    /// Canvas position `{"position":"x,y"}`.
    pub position: Option<(f64, f64)>,
    /// Story Map group assignment.
    pub group: Option<Option<&'a str>>,
    /// Story Map node color.
    pub color: Option<Option<&'a str>>,
    /// Node size (width, height) — Twine `"size":"w,h"` convention.
    pub size: Option<Option<(f64, f64)>>,
    /// Manual reachability link source (metadata `reachable`).
    pub reachable: Option<Option<&'a str>>,
    /// Replacement tag list (whole-block rewrite).
    pub tags: Option<&'a [String]>,
}

/// Format a coordinate: integer if whole number, otherwise up to 2 decimal places.
fn format_coord(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{:.2}", v)
    }
}

/// Rewrite a passage header line with the requested field edits.
///
/// The line is rebuilt from three parts — `:: Name`, the `[tag]` blocks,
/// and the `{...}` JSON metadata block — preserving every part that isn't
/// edited:
///
/// - Unedited tag blocks keep their original text (`[a] [b]` stays as-is).
/// - The JSON block is merged (existing unknown keys survive; setting a
///   value updates it; `Some(None)` removes it; an object that becomes
///   empty removes the whole block).
/// - Trailing duplicate JSON blocks (old race-condition damage) are
///   repaired by [`peel_trailing_json_blocks`].
///
/// # Errors
///
/// Returns an error when the line isn't a `::` passage header, or when a
/// tag in `edits.tags` fails [`validate_tag`]. On error the caller should
/// keep the original line.
pub(crate) fn update_passage_header(line: &str, edits: HeaderEdits) -> Result<String, String> {
    if !line.trim_start().starts_with("::") {
        return Err(format!("not a passage header line: {}", line));
    }
    // ── Part 1: peel the JSON metadata tail ────────────────────────────
    let (stripped, existing_json) = peel_trailing_json_blocks(line);
    let trimmed_stripped = stripped.trim_end();

    // ── Part 2: split ":: Name" from the "[tag] blocks" section ───────
    let (name_part, tag_section) = match trimmed_stripped.find('[') {
        Some(pos) => (&trimmed_stripped[..pos], &trimmed_stripped[pos..]),
        None => (trimmed_stripped, ""),
    };

    if name_part.trim_start_matches(':').trim().is_empty() {
        return Err(format!("passage header has no name: {}", line));
    }

    // ── Part 3: rebuild the tag section ────────────────────────────────
    let mut tag_section_owned: Option<String> = None;
    if let Some(tags) = edits.tags {
        for tag in tags {
            validate_tag(tag)?;
        }
        tag_section_owned = Some(if tags.is_empty() {
            String::new()
        } else {
            format!("[{}]", tags.join(" "))
        });
    }

    // ── Part 4: merge the JSON edits ───────────────────────────────────
    let mut json_val = existing_json.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    if let Some((x, y)) = edits.position {
        let pos_str = format!("{},{}", format_coord(x), format_coord(y));
        json_val["position"] = serde_json::Value::String(pos_str);
    }

    let set_or_remove = |json: &mut serde_json::Value, key: &str, value: Option<&str>| {
        if let Some(obj) = json.as_object_mut() {
            match value {
                Some(v) => {
                    obj.insert(key.to_string(), serde_json::Value::String(v.to_string()));
                }
                None => {
                    obj.remove(key);
                }
            }
        }
    };

    if let Some(group) = edits.group {
        set_or_remove(&mut json_val, "group", group);
    }
    if let Some(color) = edits.color {
        set_or_remove(&mut json_val, "color", color);
    }
    if let Some(size) = edits.size {
        match size {
            Some((w, h)) => {
                let size_str = format!("{},{}", format_coord(w), format_coord(h));
                json_val["size"] = serde_json::Value::String(size_str);
            }
            None => set_or_remove(&mut json_val, "size", None),
        }
    }
    if let Some(reachable) = edits.reachable {
        set_or_remove(&mut json_val, "reachable", reachable);
    }

    // ── Part 5: assemble ───────────────────────────────────────────────
    let mut result = String::from(name_part.trim_end());
    let final_tags = tag_section_owned.as_deref().unwrap_or(tag_section).trim();
    if !final_tags.is_empty() {
        result.push(' ');
        result.push_str(final_tags);
    }

    // An empty JSON object adds nothing — drop the block entirely. This
    // also covers "removed every field" edits.
    if let Some(obj) = json_val.as_object()
        && !obj.is_empty()
    {
        let new_json = serde_json::to_string(&json_val)
            .map_err(|e| format!("JSON re-encode failed: {}", e))?;
        result.push(' ');
        result.push_str(&new_json);
    }

    Ok(result)
}

/// Validate a passage name for header round-tripping.
///
/// The header grammar terminates the name at the first `[` or `{`, so a
/// name containing either bracket cannot survive a write/read cycle.
/// `::` inside a name would parse as a nested header marker.
pub(crate) fn validate_passage_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Passage name cannot be empty".to_string());
    }
    if name.trim() != name {
        return Err("Passage name cannot start or end with whitespace".to_string());
    }
    if name.contains("::") {
        return Err("Passage name cannot contain '::'".to_string());
    }
    if name.contains('[') || name.contains(']') {
        return Err("Passage name cannot contain '[' or ']'".to_string());
    }
    if name.contains('{') || name.contains('}') {
        return Err("Passage name cannot contain '{' or '}'".to_string());
    }
    if name.contains('\n') || name.contains('\r') {
        return Err("Passage name cannot contain line breaks".to_string());
    }
    Ok(())
}

/// Validate a tag for the whitespace-separated `[tag1 tag2]` block.
pub(crate) fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    if tag.chars().any(char::is_whitespace) {
        return Err(format!("Tag '{}' cannot contain whitespace", tag));
    }
    if tag.contains('[') || tag.contains(']') {
        return Err(format!("Tag '{}' cannot contain '[' or ']'", tag));
    }
    if tag.contains('{') || tag.contains('}') {
        return Err(format!("Tag '{}' cannot contain '{{' or '}}'", tag));
    }
    Ok(())
}

/// Peel trailing JSON metadata blocks off a header line, right to left.
///
/// Returns the line with its trailing `{...}` blocks (and trailing
/// whitespace) removed, plus the parsed contents of the **last**
/// (rightmost) block — the one holding the most recently written
/// values. Each block is validated as proper JSON by the unified header
/// parser before it is stripped, so custom tags containing braces
/// (e.g., `[tag{stuff}]`) survive untouched.
///
/// Example: `:: Name [tags] {"position":"1,2"} {"position":"3,4"}`
/// peels to `:: Name [tags]` + the `{"position":"3,4"}` value.
fn peel_trailing_json_blocks(line: &str) -> (String, Option<serde_json::Value>) {
    let mut stripped = line;
    let mut json = None;
    loop {
        if let Some((rest, json_str)) =
            knot_formats::header::extract_json_block_public(stripped.trim_end())
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_str)
        {
            // Keep the last (rightmost) block's data — it has the most
            // recently written position/group/color/reachable values.
            if json.is_none() {
                json = Some(parsed);
            }
            stripped = rest;
            continue;
        }
        break;
    }
    (stripped.to_string(), json)
}

/// Insert or update the `"reachable"` manual link in a passage header
/// line's JSON metadata block, preserving every other field.
///
/// This is the quickfix writer for the unreachable-passage diagnostic:
/// `:: Hub` becomes `:: Hub {"reachable":"Start"}`, while
/// `:: Hub {"position":"1,2"}` becomes
/// `:: Hub {"position":"1,2","reachable":"Start"}`. The value names the
/// **source passage** of the manual link ("Hub is reachable from Start"),
/// which the graph represents as a `Manual` edge `Start → Hub`. The
/// declaration lands in the metadata block — tool state the engine never
/// reads — rather than the engine-facing tag bracket, and merges with any
/// existing position/group/color values the way Story Map position saves
/// do.
pub(crate) fn set_reachable_in_header(line: &str, source: &str) -> String {
    update_passage_header(
        line,
        HeaderEdits {
            reachable: Some(Some(source)),
            ..Default::default()
        },
    )
    .unwrap_or_else(|_| line.to_string())
}

// ===========================================================================
// StoryData position extraction
// ===========================================================================

/// Extract passage positions from StoryData JSON body.
///
/// Twine 2's StoryData can contain per-passage position information.
/// The StoryData JSON may include a "passages" array where each entry
/// has a "name" and "position" field (an array of [x, y]).
/// This function parses those positions and adds them to the map —
/// FILL-ONLY: a position already present in the map (parsed from the
/// passage's own header metadata, which is what the Story Map's drag
/// flow writes) is never overwritten. The old overwrite semantics made
/// a dragged node snap back to its stale StoryData coordinate on the
/// next graph refresh.
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
                // Fill-only: header metadata (written by the editor's
                // drag flow) wins over the legacy StoryData fallback.
                positions.entry(name.to_string()).or_insert((x, y));
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

/// Find the passage containing the cursor **anywhere** — header line or
/// body — via span containment.
///
/// [`find_passage_at_position_span_based`] only matches when the cursor is
/// on the header line, but graph diagnostics (like UnreachablePassage)
/// squiggle only the passage NAME in the header, while authors spend
/// their time editing in the body. This lookup covers the whole passage
/// span, letting code actions surface quickfixes from wherever in the
/// passage the cursor happens to be.
///
/// Returns `None` when the workspace has no document for the URI or the
/// position falls between passages (e.g. a blank line outside any span).
pub(crate) fn find_passage_containing_position(
    text: &str,
    workspace: &Workspace,
    uri: &Url,
    position: Position,
) -> Option<String> {
    let doc = workspace.get_document(uri)?;
    let byte_offset = position_to_byte_offset(text, position).min(text.len());
    doc.passages
        .iter()
        .find(|p| p.contains_abs_offset(byte_offset))
        .map(|p| p.name.clone())
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

#[cfg(test)]
mod byte_offset_position_tests {
    use super::byte_offset_to_position;

    // Fix (round 1.1 / DT3): the converter used to panic on offsets that
    // land inside a multi-byte UTF-8 sequence (only `.min(len)` was
    // clamped, never boundary-snapped). Production offsets always come
    // from UTF-16 conversions that snap, so this was folklore-safe, not
    // enforced — and the delta-equivalence proptest in `sync.rs` had to
    // snap its own offsets to avoid tripping it. These tests pin the
    // tolerant contract: ANY `0..=len` offset is accepted and floors to
    // the enclosing character's start.

    #[test]
    fn every_offset_in_cjk_text_is_accepted_and_floors_to_char_start() {
        let text = "a中\nb🎉end"; // 3-byte CJK + 4-byte emoji, multi-line
        // 中 occupies bytes 1..4, 🎉 occupies bytes 6..10.
        for offset in 0..=text.len() {
            let pos = byte_offset_to_position(text, offset); // must not panic
            // The floor of any offset inside 中 (1..4) is 中's start (line 0, char 1).
            if (1..4).contains(&offset) {
                assert_eq!((pos.line, pos.character), (0, 1), "offset {offset}");
            }
            // The floor of any offset inside 🎉 (6..10) is 🎉's start
            // (line 1, char 1 — after `b`).
            if (6..10).contains(&offset) {
                assert_eq!((pos.line, pos.character), (1, 1), "offset {offset}");
            }
        }
    }

    #[test]
    fn out_of_range_offset_clamps_to_end_of_text() {
        let text = ":: Start\n你好\n";
        let pos = byte_offset_to_position(text, usize::MAX);
        // End of text = start of the (empty) line after the final \n.
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn empty_text_accepts_any_offset() {
        let pos = byte_offset_to_position("", 42);
        assert_eq!((pos.line, pos.character), (0, 0));
    }

    #[test]
    fn offsets_are_monotonic_across_boundaries() {
        // Flooring must never make positions go backwards as the offset
        // grows — the worker's versioned publishes and the client's
        // ordering both rely on position monotonicity within a document.
        let text = "中a\n🎉b中c";
        let mut prev = (0u32, 0u32);
        for offset in 0..=text.len() {
            let p = byte_offset_to_position(text, offset);
            assert!((p.line, p.character) >= prev, "regression at {offset}");
            prev = (p.line, p.character);
        }
    }
}

#[cfg(test)]
mod storydata_position_tests {
    use super::extract_positions_from_storydata;
    use std::collections::HashMap;

    fn storydata_text(json: &str) -> String {
        format!(":: StoryData\n{}\n\n:: Start\nHello\n", json)
    }

    #[test]
    fn storydata_positions_fill_passages_without_header_metadata() {
        let text = storydata_text(
            r#"{"start":"Start","passages":[{"name":"Start","position":[10,20]},{"name":"Other","position":[30,40]}]}"#,
        );
        let mut positions = HashMap::new();
        extract_positions_from_storydata(&text, &mut positions);
        assert_eq!(positions.get("Start"), Some(&(10.0, 20.0)));
        assert_eq!(positions.get("Other"), Some(&(30.0, 40.0)));
    }

    #[test]
    fn storydata_positions_do_not_overwrite_header_metadata() {
        // A dragged node gets `{"position":"x,y"}` written into its own
        // header; the StoryData fallback must not yank it back to the
        // stale archived coordinate on the next graph export.
        let text =
            storydata_text(r#"{"start":"Start","passages":[{"name":"Start","position":[10,20]}]}"#);
        let mut positions = HashMap::from([("Start".to_string(), (500.0, 250.0))]);
        extract_positions_from_storydata(&text, &mut positions);
        assert_eq!(positions.get("Start"), Some(&(500.0, 250.0)));
    }

    #[test]
    fn storydata_without_passages_array_is_ignored() {
        let text = storydata_text(r#"{"start":"Start","format":"Harlowe"}"#);
        let mut positions = HashMap::new();
        extract_positions_from_storydata(&text, &mut positions);
        assert!(positions.is_empty());
    }
}
