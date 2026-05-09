//! Block extraction and building for SugarCube.
//!
//! Contains regexes and functions for extracting macro invocations
//! and building interleaved text/macro content blocks from passage bodies.

use knot_core::passage::Block;
use once_cell::sync::Lazy;
use regex::Regex;

/// <<name ...>> — any open macro
pub(crate) static RE_MACRO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+([^>]*?))?>>").unwrap());

/// <</name>> — closing macro tag
pub(crate) static RE_MACRO_CLOSE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap());

/// Extract macros from a passage body and produce content blocks.
pub(crate) fn extract_macros(body: &str, body_offset: usize) -> Vec<Block> {
    let mut blocks = Vec::new();

    // Open macros: <<name args>>
    for caps in RE_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let name = caps.get(1).unwrap().as_str().to_string();
        let args = caps.get(2).map(|a| a.as_str().to_string()).unwrap_or_default();
        blocks.push(Block::Macro {
            name,
            args,
            span: body_offset + m.start()..body_offset + m.end(),
        });
    }

    // Close macros: <</name>>
    for caps in RE_MACRO_CLOSE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let name = caps.get(1).unwrap().as_str().to_string();
        blocks.push(Block::Macro {
            name: format!("/{}", name),
            args: String::new(),
            span: body_offset + m.start()..body_offset + m.end(),
        });
    }

    blocks
}

/// Build content blocks from the body text, interleaving text and macro
/// blocks without duplication.
///
/// Collects macro spans, then creates text blocks only for the gaps
/// between macros (or the whole body if no macros are present).
pub(crate) fn build_body_blocks(body: &str, body_offset: usize, macros: &[Block]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();

    if macros.is_empty() {
        // No macros — the entire body is a single text block
        if !body.trim().is_empty() {
            blocks.push(Block::Text {
                content: body.to_string(),
                span: body_offset..body_offset + body.len(),
            });
        }
        return blocks;
    }

    // Collect macro spans so we can identify the gaps (non-macro text)
    let mut macro_spans: Vec<std::ops::Range<usize>> = macros
        .iter()
        .filter_map(|m| match m {
            Block::Macro { span, .. } => Some(span.start - body_offset..span.end - body_offset),
            _ => None,
        })
        .collect();

    // Sort by start position
    macro_spans.sort_by_key(|s| s.start);

    // Build blocks: text gaps + macros, in source order
    let mut cursor: usize = 0;
    let mut macro_idx: usize = 0;

    while macro_idx < macro_spans.len() {
        let mspan = &macro_spans[macro_idx];

        // Add text block for the gap before this macro (if non-empty)
        if cursor < mspan.start {
            let gap = &body[cursor..mspan.start];
            if !gap.trim().is_empty() {
                blocks.push(Block::Text {
                    content: gap.to_string(),
                    span: body_offset + cursor..body_offset + mspan.start,
                });
            }
        }

        // Add the macro block itself
        if let Some(macro_block) = macros.get(macro_idx) {
            blocks.push(macro_block.clone());
        }

        cursor = mspan.end;
        macro_idx += 1;
    }

    // Add trailing text after the last macro
    if cursor < body.len() {
        let trailing = &body[cursor..];
        if !trailing.trim().is_empty() {
            blocks.push(Block::Text {
                content: trailing.to_string(),
                span: body_offset + cursor..body_offset + body.len(),
            });
        }
    }

    // If no blocks were created (all macros but no text gaps), just add macros
    if blocks.is_empty() && !macros.is_empty() {
        blocks.extend_from_slice(macros);
    }

    blocks
}
