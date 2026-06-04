//! Core virtual document construction — format-agnostic.
//!
//! This module provides the shared infrastructure for building sectioned virtual
//! JavaScript documents from Twine workspaces. The virtual document is the
//! foundation for cross-passage variable tracking, deep alias resolution, and
//! unified path-centric analysis.
//!
//! ## Why this is format-agnostic
//!
//! `[script]` passages are a **Twine core concept** defined by the Twee 3 spec,
//! not by any individual story format. Every format (SugarCube, Harlowe, Snowman,
//! Chapbook) has `[script]` passages that contain JavaScript executing at startup
//! in a shared scope. The core virtual document builder handles the parts that
//! are identical across all formats:
//!
//! - Collecting `[script]` passages from the workspace
//! - Concatenating them into a unified section with line mappings
//! - Scaffolding macro/template sections (one per passage)
//! - Line mapping from virtual document lines back to original source
//!
//! Format-specific behavior (alias regex patterns, macro→JS translation, variable
//! sigil resolution) is provided by the `FormatPlugin` trait hooks. The core
//! builder calls these hooks at the right points, so each format only needs to
//! implement a few focused methods instead of reimplementing the entire pipeline.
//!
//! ## Architecture
//!
//! The virtual document has two kinds of sections:
//!
//! 1. **Unified script section**: All `[script]` passage bodies concatenated
//!    in document order. Script passages execute at startup in a deterministic
//!    sequence, sharing a single JS scope. This section is where startup aliases
//!    are defined (e.g., `var g = gs()` in SugarCube, `var s = window.story.state`
//!    in Snowman, `var s = state` in Chapbook).
//!
//! 2. **Format-translated sections**: Each non-script passage that contains
//!    variable-affecting content is translated to JavaScript using the format
//!    plugin's `translate_passage_to_js()` hook. These are kept as individual
//!    sections — one per passage — because non-script passages execute
//!    non-deterministically based on player choices.
//!
//! ## Key Design Decisions
//!
//! - **Sectioned, not flat**: Format-translated sections are NOT concatenated
//!   with the script section. This avoids conflating "shares scope" (true for
//!   all JS across the session) with "shares execution flow" (only true for
//!   script passages at startup).
//!
//! - **Startup alias table**: Extracted from the unified script section via
//!   the format plugin's `extract_startup_aliases()` hook, and shared across
//!   all sections. This lets format-translated sections resolve aliases like
//!   `g.x` → `State.variables.x` (SugarCube) or `s.x` → `window.story.state.x`
//!   (Snowman) without re-deriving them.
//!
//! - **Line mapping**: Every virtual line maps back to the original passage
//!   and source line, enabling "go to definition" from analysis results.

use crate::plugin::SourceTextProvider;
use knot_core::passage::{Block, Passage};
use regex::Regex;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Comment stripping (shared across all formats — JS comments are universal)
// ---------------------------------------------------------------------------

static RE_LINE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"//[^\n]*").unwrap());
static RE_BLOCK_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/\*[\s\S]*?\*/").unwrap());

/// Strip JS comments from source text before alias extraction.
///
/// This is format-agnostic because all Twine formats use JavaScript in
/// `[script]` passages, and JS comments are the same regardless of the
/// story format layered on top.
pub fn strip_comments(src: &str) -> String {
    let no_block = RE_BLOCK_COMMENT.replace_all(src, "");
    let no_line = RE_LINE_COMMENT.replace_all(&no_block, "");
    no_line.to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute 0-based line number from a byte offset in a string.
pub fn line_from_offset(text: &str, offset: usize) -> u32 {
    text[..offset.min(text.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count() as u32
}

/// Extract the raw body text from a passage, combining all blocks.
///
/// This is format-agnostic because it works with the core `Block` model.
/// For script passages, the body is stored as `Block::Text` (raw JS).
/// For other passages, we reconstruct the full text including macros
/// from the block model so that the format plugin's translator can
/// process it.
pub fn extract_body_text(
    passage: &Passage,
    source_text: &dyn SourceTextProvider,
    file_uri: &str,
) -> String {
    let mut body = String::new();
    for block in &passage.body {
        match block {
            Block::Text { content, .. } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(content);
            }
            Block::Macro { name, args, .. } => {
                // Reconstruct the macro text for the format translator.
                // We don't know the delimiter syntax here (<<>>, (), []),
                // so we emit a neutral format that the format plugin can
                // parse in its translate_passage_to_js() hook. We use
                // the <<>> syntax as a default because the format plugin
                // will re-parse from the original passage body anyway.
                if !body.is_empty() {
                    body.push('\n');
                }
                if !args.is_empty() {
                    body.push_str(&format!("<<{} {}>>", name, args));
                } else {
                    body.push_str(&format!("<<{}>>", name));
                }
            }
            Block::Incomplete { content, .. } => {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(content);
            }
            _ => {}
        }
    }

    // If we couldn't get body from blocks, try source text
    if body.is_empty() {
        if let Some(text) = source_text.get_source_text(file_uri) {
            if passage.span.start < text.len() && passage.span.end <= text.len() {
                body = text[passage.span.start..passage.span.end].to_string();
                // Strip the header line
                if let Some(newline_pos) = body.find('\n') {
                    body = body[newline_pos + 1..].to_string();
                }
            }
        }
    }

    body
}
