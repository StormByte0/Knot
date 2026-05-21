//! Twee document formatting helpers.

use lsp_types::*;

use super::position::utf16_len;

/// Format a Twee document: normalize headers, trim trailing whitespace,
/// ensure blank lines between passages.
pub(crate) fn format_twee_text(text: &str) -> Vec<TextEdit> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut edits = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        // Trim trailing whitespace
        let trimmed_end = line.trim_end();
        if trimmed_end.len() != line.len() {
            edits.push(TextEdit {
                range: Range {
                    start: Position { line: i as u32, character: utf16_len(trimmed_end) },
                    end: Position { line: i as u32, character: utf16_len(line) },
                },
                new_text: String::new(),
            });
        }

        // Normalize passage header spacing: ensure exactly one space after "::"
        if let Some(rest) = line.strip_prefix("::") {
            if rest.starts_with(|c: char| c != ' ' && c != '[' && c != '\t') && !rest.is_empty() {
                // Missing space after "::", add one.
                // "::" is always 2 UTF-16 code units (ASCII), so character=2 is correct.
                edits.push(TextEdit {
                    range: Range {
                        start: Position { line: i as u32, character: 2 },
                        end: Position { line: i as u32, character: 2 },
                    },
                    new_text: " ".to_string(),
                });
            }
        }
    }

    // Ensure blank lines between passages — done as a full replacement if needed
    let mut formatted_lines: Vec<String> = Vec::new();
    let mut prev_was_blank = true; // start with blank to avoid blank line at top

    for line in &lines {
        if line.starts_with("::") {
            if !prev_was_blank && !formatted_lines.is_empty() {
                formatted_lines.push(String::new());
            }
            formatted_lines.push(line.trim_end().to_string());
            prev_was_blank = false;
        } else {
            let trimmed = line.trim_end().to_string();
            prev_was_blank = trimmed.is_empty();
            formatted_lines.push(trimmed);
        }
    }

    let formatted_text = formatted_lines.join("\n");
    let original_text = text.to_string();

    if formatted_text != original_text {
        // Return a single edit replacing the entire document
        let line_count = lines.len() as u32;
        let last_line_utf16_len = lines.last().map(|l| utf16_len(l)).unwrap_or(0);
        vec![TextEdit {
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: line_count.saturating_sub(1), character: last_line_utf16_len },
            },
            new_text: formatted_text,
        }]
    } else {
        edits
    }
}
