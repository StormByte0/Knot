//! Navigation helpers — finding passages that link to a target and link ranges.

use knot_core::Workspace;
use lsp_types::*;

use super::position::utf16_len_up_to;

/// Find passages that link TO a given passage name.
pub(crate) fn find_passages_linking_to(workspace: &Workspace, passage_name: &str) -> Vec<String> {
    let mut result = Vec::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                result.push(passage.name.clone());
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

/// Find all [[target]] link ranges for a specific target in the text.
pub(crate) fn find_link_ranges_for_target(text: &str, target: &str) -> Vec<Range> {
    let mut ranges = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("[[") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find("]]") {
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;
                let link_text = &line[content_start..content_end];
                let link_target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                if link_target.trim() == target {
                    ranges.push(Range {
                        start: Position { line: line_idx as u32, character: utf16_len_up_to(line, content_start) },
                        end: Position { line: line_idx as u32, character: utf16_len_up_to(line, content_end) },
                    });
                }
                search_from = abs_start + rel_end + 2;
            } else {
                break;
            }
        }
    }
    ranges
}
