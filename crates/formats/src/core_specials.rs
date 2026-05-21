//! Format-agnostic handling for Twine-core special passages.
//!
//! The `[script]` and `[stylesheet]` tags are defined by the Twee 3 spec and
//! are shared across ALL story formats. This module provides format-agnostic
//! body parsing for these passages so that every format plugin doesn't have
//! to reimplement the same logic.
//!
//! ## Usage
//!
//! Format plugins should check `passage.is_script_passage()` and
//! `passage.is_stylesheet_passage()` in their `parse()` method, and for these
//! passages skip format-specific body parsing (link extraction, variable
//! extraction, block building). The body should be stored as a single raw
//! text block. For stylesheet passages, no semantic tokens are generated for
//! the body. For script passages, format-specific implicit passage reference
//! extraction may be added on top.
//!
//! Each format plugin currently inlines this logic for simplicity, but the
//! shared types and functions here serve as the canonical reference and may
//! be used directly in the future.

#![allow(dead_code)] // Used as reference; format plugins currently inline this logic
//!
//! ## Format Isolation
//!
//! These functions handle the **core** behavior that is the same regardless of
//! the story format:
//!
//! - **Stylesheet passages**: No format-specific parsing. The body is stored as
//!   a raw text block. No link extraction, no variable extraction. TextMate
//!   grammar handles CSS highlighting.
//!
//! - **Script passages**: No format-specific markup parsing. The body is stored
//!   as a raw text block. Format plugins may optionally extract implicit passage
//!   references (e.g., SugarCube's `Engine.play()`) by calling
//!   `extract_script_implicit_refs()` with their format-specific patterns.
//!
//! Each format plugin should call these functions from their `parse()` method
//! when `passage.is_script_passage()` or `passage.is_stylesheet_passage()`
//! returns `true`, BEFORE attempting format-specific body parsing.

use knot_core::passage::{Block, SpecialPassageLayer};
use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};

/// Result of parsing a core special passage body.
pub struct CoreSpecialBodyResult {
    /// The body blocks to assign to the passage.
    pub blocks: Vec<Block>,
    /// Semantic tokens for the header (always produced for special passages).
    pub header_tokens: Vec<SemanticToken>,
}

/// Handle a stylesheet passage body in a format-agnostic way.
///
/// Stylesheet passages contain CSS and should NOT be parsed with any
/// format-specific markup. The body is stored as a single raw text block.
/// No semantic tokens are generated for the body — TextMate grammar handles
/// CSS highlighting.
///
/// # Arguments
///
/// * `body` - The raw body text of the passage
/// * `body_offset` - The byte offset where the body starts in the document
/// * `header_name` - The passage name (for token generation)
/// * `name_start` - The byte offset where the name starts in the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, LegacyCore, etc.)
///
/// # Returns
///
/// A `CoreSpecialBodyResult` with the body blocks and header semantic tokens.
pub fn parse_stylesheet_body(
    body: &str,
    body_offset: usize,
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> CoreSpecialBodyResult {
    let blocks = vec![Block::Text {
        content: body.to_string(),
        span: body_offset..body_offset + body.len(),
    }];

    let header_tokens = build_special_header_tokens(name_start, name_len, layer);

    CoreSpecialBodyResult { blocks, header_tokens }
}

/// Handle a script passage body in a format-agnostic way.
///
/// Script passages contain JavaScript (or another scripting language) and
/// should NOT be parsed with format-specific markup like links, macros,
/// or hooks. The body is stored as a single raw text block.
///
/// Format plugins that support implicit passage references in script code
/// (e.g., SugarCube's `Engine.play()`, Harlowe's `(goto:)`) should extract
/// those separately and add them to the passage's `links` field after calling
/// this function.
///
/// # Arguments
///
/// * `body` - The raw body text of the passage
/// * `body_offset` - The byte offset where the body starts in the document
/// * `name_start` - The byte offset where the name starts in the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, LegacyCore, etc.)
///
/// # Returns
///
/// A `CoreSpecialBodyResult` with the body blocks and header semantic tokens.
pub fn parse_script_body(
    body: &str,
    body_offset: usize,
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> CoreSpecialBodyResult {
    let blocks = vec![Block::Text {
        content: body.to_string(),
        span: body_offset..body_offset + body.len(),
    }];

    let header_tokens = build_special_header_tokens(name_start, name_len, layer);

    CoreSpecialBodyResult { blocks, header_tokens }
}

/// Build semantic tokens for a special passage header.
///
/// Generates:
/// - `SpecialPassageHeader` token for the `::` prefix
/// - `SpecialPassage` token for the passage name
/// - Layer modifier (`TwineCore`, `StoryFormat`, etc.) if applicable
fn build_special_header_tokens(
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> Vec<SemanticToken> {
    let layer_modifier = layer.map(|l| match l {
        SpecialPassageLayer::TwineCore => SemanticTokenModifier::TwineCore,
        SpecialPassageLayer::LegacyCore => SemanticTokenModifier::TwineCore,
        SpecialPassageLayer::StoryFormat => SemanticTokenModifier::StoryFormat,
        SpecialPassageLayer::UserDefined => SemanticTokenModifier::UserDefined,
    });

    let mut tokens = Vec::new();

    // `::` prefix token
    tokens.push(SemanticToken {
        start: name_start.saturating_sub(2),
        length: 2,
        token_type: SemanticTokenType::SpecialPassageHeader,
        modifier: layer_modifier,
    });

    // Passage name token
    tokens.push(SemanticToken {
        start: name_start,
        length: name_len,
        token_type: SemanticTokenType::SpecialPassage,
        modifier: layer_modifier,
    });

    tokens
}
