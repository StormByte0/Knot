//! Format-agnostic handling for Twine-core special passages.
//!
//! The `[script]` and `[stylesheet]` tags are defined by the Twee 3 spec and
//! are shared across ALL story formats. This module provides format-agnostic
//! body parsing and header token generation for these passages so that every
//! format plugin doesn't have to reimplement the same logic.
//!
//! ## Core vs Format-Specific Special Passages
//!
//! **Core special passages** (StoryTitle, StoryData, Start) are name-matched
//! and always recognized regardless of format. **Core tag-matched passages**
//! (`[script]`, `[stylesheet]`, `[style]`) are recognized by their tag, not
//! their name. Both categories are defined by `twine_core_special_passages()`
//! in `knot_core::passage` and share the same body-parsing rules.
//!
//! ## Usage
//!
//! Format plugins should call these helpers from their `parse()` method when
//! `passage.is_script_passage()` or `passage.is_stylesheet_passage()` returns
//! `true`, BEFORE attempting format-specific body parsing. The helpers produce:
//!
//! - A single raw `Block::Text` for the body (no link extraction, no variable
//!   extraction, no format-specific markup parsing).
//! - Correct semantic tokens for the header with the appropriate layer modifier.
//! - Optional tag tokens for the passage header.
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
//!   `extract_script_implicit_refs()` with their format-specific patterns and
//!   adding the results to the passage's `links` field after calling
//!   `parse_script_body()`.
//!
//! ## Why This Module Exists
//!
//! Without it, every format plugin inlines the same pattern:
//!
//! 1. Check `is_script_passage()` / `is_stylesheet_passage()`
//! 2. Create a raw `Block::Text { content, span }` for the body
//! 3. Build header tokens with `SpecialPassageHeader` / `SpecialPassage` types
//! 4. Build tag tokens with appropriate modifiers
//! 5. Skip format-specific body parsing
//!
//! This leads to bugs (Chapbook/Snowman miss the `is_special` classification
//! for tag-matched core passages because they only check `special_def.is_some()`)
//! and duplicated logic across 4+ plugins. Centralizing it here ensures
//! consistent, correct behavior.
//!
//! ## Passage-Relative Offsets
//!
//! All spans and token positions produced by this module are **passage-relative**:
//! offset 0 corresponds to the passage head `::`. The caller sets
//! `Passage.passage_offset` separately to the document-absolute position of the
//! passage head for LSP boundary conversion.

use knot_core::passage::{Block, SpecialPassageBehavior, SpecialPassageDef, SpecialPassageLayer};
use crate::header::TweeHeader;
use crate::plugin::{FormatPlugin, SemanticToken, SemanticTokenModifier, SemanticTokenType};

// ---------------------------------------------------------------------------
// Core tag constants
// ---------------------------------------------------------------------------

/// Tags that mark a passage as a script (JavaScript) passage.
/// These are Twine-core tags defined by the Twee 3 spec.
const CORE_SCRIPT_TAGS: &[&str] = &["script"];

/// Tags that mark a passage as a stylesheet (CSS) passage.
/// Both "stylesheet" and "style" are recognized per the Twee 3 spec.
const CORE_STYLESHEET_TAGS: &[&str] = &["stylesheet", "style"];

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of parsing a core special passage body.
pub struct CoreSpecialBodyResult {
    /// The body blocks to assign to the passage.
    pub blocks: Vec<Block>,
    /// Semantic tokens for the header (prefix + name + optional tag tokens).
    pub header_tokens: Vec<SemanticToken>,
}

/// Whether a passage is a core special passage that should skip
/// format-specific body parsing. This is the unified check that
/// accounts for both name-matched passages (StoryTitle, StoryData) and
/// tag-matched passages ([script], [stylesheet], [style]).
pub enum CoreSpecialKind {
    /// A script passage tagged `[script]`. Body is raw JavaScript.
    Script,
    /// A stylesheet passage tagged `[stylesheet]` or `[style]`. Body is raw CSS.
    Stylesheet,
    /// A core metadata passage (StoryTitle, StoryData). Body has no
    /// format-specific markup but may contain structured data (JSON for StoryData).
    Metadata,
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Determine if a passage is a core special passage that should skip
/// format-specific body parsing.
///
/// This combines the `special_def` from `FormatPlugin::classify_passage()`
/// with the tag-based checks `is_script_passage()` / `is_stylesheet_passage()`
/// into a single classification. This fixes the bug where plugins only check
/// `special_def.is_some()` and miss tag-matched core passages like
/// `:: MyScript [script]` (where `special_def` is `None` because `[script]`
/// is a TwineCore tag, not a format-specific name-match).
///
/// # Arguments
///
/// * `plugin` - The format plugin (for `classify_passage()` and `classify_tag()`)
/// * `passage_name` - The passage name
/// * `passage_tags` - The passage tags
/// * `special_def` - The result of `plugin.classify_passage()` (may be `None`
///   for tag-matched core passages)
pub fn classify_core_special(
    _plugin: &dyn FormatPlugin,
    _passage_name: &str,
    passage_tags: &[String],
    special_def: Option<&SpecialPassageDef>,
) -> Option<CoreSpecialKind> {
    // Check tag-matched core passages first — these are TwineCore tag-matched
    // passages like [script], [stylesheet], [style]. The special_def from
    // classify_passage() may be None for these because classify_passage()
    // only checks name-matched core passages in its first pass.
    //
    // We check the tag strings directly against the known core tags, matching
    // the same logic as Passage::is_script_passage() / is_stylesheet_passage()
    // in knot_core. The classify_tag() method returns a SemanticTokenModifier
    // (not a SpecialPassageDef), so we can't use it for behavior classification.
    for tag in passage_tags {
        if CORE_SCRIPT_TAGS.iter().any(|core_tag| tag.eq_ignore_ascii_case(core_tag)) {
            return Some(CoreSpecialKind::Script);
        }
        if CORE_STYLESHEET_TAGS.iter().any(|core_tag| tag.eq_ignore_ascii_case(core_tag)) {
            return Some(CoreSpecialKind::Stylesheet);
        }
    }

    // Check name-matched core passages — use the correct variant names from
    // SpecialPassageBehavior: ScriptInjection, StyleInjection, Metadata.
    if let Some(def) = special_def {
        match def.behavior {
            SpecialPassageBehavior::ScriptInjection => {
                return Some(CoreSpecialKind::Script);
            }
            SpecialPassageBehavior::StyleInjection => {
                return Some(CoreSpecialKind::Stylesheet);
            }
            SpecialPassageBehavior::Metadata => {
                return Some(CoreSpecialKind::Metadata);
            }
            _ => {}
        }
    }

    None
}

/// Determine whether a passage should receive `SpecialPassageHeader` /
/// `SpecialPassage` semantic token types (rather than `PassageHeader` /
/// `PassageName`). This accounts for both name-matched and tag-matched
/// core special passages.
///
/// This is the correct replacement for the buggy pattern of checking only
/// `special_def.is_some()` which misses tag-matched core passages.
pub fn is_special_for_tokens(
    plugin: &dyn FormatPlugin,
    passage_name: &str,
    passage_tags: &[String],
    special_def: Option<&SpecialPassageDef>,
) -> bool {
    classify_core_special(plugin, passage_name, passage_tags, special_def).is_some()
}

// ---------------------------------------------------------------------------
// Body parsing
// ---------------------------------------------------------------------------

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
/// * `body_offset_in_passage` - The passage-relative byte offset where the body
///   starts (i.e., relative to the passage head `::`)
/// * `passage_head` - The document-absolute byte offset of the passage head
///   `::`. Used to convert document-absolute header positions (e.g.
///   `name_start`) into passage-relative positions for semantic tokens.
/// * `name_start` - The document-absolute byte offset where the name starts in
///   the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, StoryFormat, etc.)
///
/// # Returns
///
/// A `CoreSpecialBodyResult` with the body blocks and header semantic tokens.
pub fn parse_stylesheet_body(
    body: &str,
    body_offset_in_passage: usize,
    passage_head: usize,
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> CoreSpecialBodyResult {
    let blocks = raw_body_blocks(body, body_offset_in_passage);
    let header_tokens = build_special_header_tokens(passage_head, name_start, name_len, layer);

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
/// * `body_offset_in_passage` - The passage-relative byte offset where the body
///   starts (i.e., relative to the passage head `::`)
/// * `passage_head` - The document-absolute byte offset of the passage head
///   `::`. Used to convert document-absolute header positions (e.g.
///   `name_start`) into passage-relative positions for semantic tokens.
/// * `name_start` - The document-absolute byte offset where the name starts in
///   the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, StoryFormat, etc.)
///
/// # Returns
///
/// A `CoreSpecialBodyResult` with the body blocks and header semantic tokens.
pub fn parse_script_body(
    body: &str,
    body_offset_in_passage: usize,
    passage_head: usize,
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> CoreSpecialBodyResult {
    let blocks = raw_body_blocks(body, body_offset_in_passage);
    let header_tokens = build_special_header_tokens(passage_head, name_start, name_len, layer);

    CoreSpecialBodyResult { blocks, header_tokens }
}

/// Handle a core metadata passage body (StoryTitle, StoryData) in a
/// format-agnostic way. The body is stored as a single raw text block.
///
/// # Arguments
///
/// * `body` - The raw body text of the passage
/// * `body_offset_in_passage` - The passage-relative byte offset where the body
///   starts (i.e., relative to the passage head `::`)
/// * `passage_head` - The document-absolute byte offset of the passage head
///   `::`. Used to convert document-absolute header positions (e.g.
///   `name_start`) into passage-relative positions for semantic tokens.
/// * `name_start` - The document-absolute byte offset where the name starts in
///   the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, StoryFormat, etc.)
pub fn parse_metadata_body(
    body: &str,
    body_offset_in_passage: usize,
    passage_head: usize,
    name_start: usize,
    name_len: usize,
    layer: Option<SpecialPassageLayer>,
) -> CoreSpecialBodyResult {
    let blocks = raw_body_blocks(body, body_offset_in_passage);
    let header_tokens = build_special_header_tokens(passage_head, name_start, name_len, layer);

    CoreSpecialBodyResult { blocks, header_tokens }
}

/// Create raw body blocks for any core special passage.
///
/// The returned span is **passage-relative**: offset 0 corresponds to the
/// passage head `::`. Callers that still have a document-absolute body offset
/// should subtract `passage_offset` before passing it here.
///
/// Used by both `parse()` (which computes a passage-relative body offset)
/// and `parse_passage()` (which uses offset 0).
pub fn raw_body_blocks(body: &str, body_offset_in_passage: usize) -> Vec<Block> {
    vec![Block::Text {
        content: body.to_string(),
        span: body_offset_in_passage..body_offset_in_passage + body.len(),
    }]
}

// ---------------------------------------------------------------------------
// Semantic token generation
// ---------------------------------------------------------------------------

/// Build semantic tokens for a special passage header.
///
/// Generates:
/// - `SpecialPassageHeader` token for the `::` prefix (always at
///   passage-relative offset 0, length 2)
/// - `SpecialPassage` token for the passage name (passage-relative)
/// - Layer modifier (`TwineCore`, `StoryFormat`, etc.) if applicable
///
/// # Arguments
///
/// * `passage_head` - The document-absolute byte offset of the passage head
///   `::`. Used to convert `name_start` from document-absolute to
///   passage-relative.
/// * `name_start` - The document-absolute byte offset where the name starts in
///   the header
/// * `name_len` - The byte length of the passage name
/// * `layer` - The special passage layer (TwineCore, StoryFormat, etc.)
pub fn build_special_header_tokens(
    passage_head: usize,
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

    // `::` prefix token — always at passage-relative offset 0
    tokens.push(SemanticToken {
        start: 0,
        length: 2,
        token_type: SemanticTokenType::SpecialPassageHeader,
        modifier: layer_modifier,
    });

    // Passage name token — passage-relative
    tokens.push(SemanticToken {
        start: name_start - passage_head,
        length: name_len,
        token_type: SemanticTokenType::SpecialPassage,
        modifier: layer_modifier,
    });

    tokens
}

/// Build semantic tokens for tags in a passage header, using the format
/// plugin's `classify_tag()` to determine modifiers for special core tags
/// like `[script]`, `[stylesheet]`, `[style]`.
///
/// All token positions are **passage-relative**: offset 0 corresponds to the
/// passage head `::`.
///
/// # Arguments
///
/// * `header` - The parsed TweeHeader (contains tag positions; `name_start` is
///   document-absolute)
/// * `passage_head` - The document-absolute byte offset of the passage head
///   `::`. Used to convert document-absolute header positions into
///   passage-relative positions.
/// * `plugin` - The format plugin (for `classify_tag()`)
pub fn build_tag_tokens(
    header: &TweeHeader,
    passage_head: usize,
    plugin: &dyn FormatPlugin,
) -> Vec<SemanticToken> {
    if header.tags.is_empty() {
        return Vec::new();
    }

    let bracket_start = header.tags_raw.find('[')
        .map(|bs| header.name_start - passage_head + bs)
        .unwrap_or(header.name_start - passage_head + header.name_text_raw.len());
    let tags_inner_start = bracket_start + 1; // after `[`
    let mut offset = tags_inner_start;
    let mut tokens = Vec::new();

    for tag in &header.tags {
        let modifier = plugin.classify_tag(tag);
        if offset > tags_inner_start {
            offset += 1; // space between tags
        }
        tokens.push(SemanticToken {
            start: offset,
            length: tag.len(),
            token_type: SemanticTokenType::Tag,
            modifier,
        });
        offset += tag.len();
    }

    tokens
}

/// Determine the layer for a core special passage based on its `SpecialPassageDef`.
/// Returns `None` if the passage is not a core special passage.
pub fn layer_from_special_def(def: Option<&SpecialPassageDef>) -> Option<SpecialPassageLayer> {
    def.map(|d| d.layer)
}
