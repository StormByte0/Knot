//! Navigation helpers — finding passages that link to a target and link ranges.
//!
//! Uses workspace passage data for span-based resolution instead of
//! re-scanning source text for `[[`/`]]`.

use knot_core::Workspace;
use lsp_types::*;

use super::position::byte_range_to_lsp_range;

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
///
/// Uses workspace passage data to locate links by `link.target` match,
/// then converts each `link.span` to an LSP Range via
/// `byte_range_to_lsp_range()`. Falls back to line-based scanning when
/// the workspace doesn't have document data for the given URI.
pub(crate) fn find_link_ranges_for_target(
    text: &str,
    workspace: &Workspace,
    uri: &url::Url,
    target: &str,
) -> Vec<Range> {
    if let Some(doc) = workspace.get_document(uri) {
        let mut ranges = Vec::new();
        for passage in &doc.passages {
            for link in &passage.links {
                if link.target.trim() == target {
                    ranges.push(byte_range_to_lsp_range(text, &passage.abs_range(&link.span)));
                }
            }
        }
        return ranges;
    }

    // Fallback: line-based scanning when workspace doesn't have the document
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
                        start: Position { line: line_idx as u32, character: super::position::utf16_len_up_to(line, content_start) },
                        end: Position { line: line_idx as u32, character: super::position::utf16_len_up_to(line, content_end) },
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
