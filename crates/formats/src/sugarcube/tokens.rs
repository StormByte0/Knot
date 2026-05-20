//! Semantic token generation for SugarCube.
//!
//! Produces semantic tokens for macro invocations, variable references,
//! passage links, and passage headers for use in LSP semantic highlighting.
//!
//! ## Hybrid highlighting strategy
//!
//! This module emits semantic tokens ONLY for Knot-specific constructs
//! that the TextMate grammar cannot handle (passage refs, variable
//! definitions/deprecation, macro names). The TextMate grammar provides
//! base highlighting for everything else: JS/CSS/HTML syntax inside
//! `<<script>>`/`<<style>>`/`[script]`/`[stylesheet]` passages, SugarCube
//! keywords, punctuation, link brackets, and parameter text.
//!
//! Semantic tokens "punch through" TextMate only where emitted. Uncovered
//! characters keep their TextMate scopes. This avoids reimplementing
//! JS/CSS/HTML tokenization in the LSP server.
//!
//! ## Token overlap avoidance
//!
//! Macro tokens cover ONLY the macro name (not `<<args>>`), so they
//! don't override TextMate punctuation/parameter highlighting and don't
//! overlap with `PassageRef` tokens inside macro arguments.
//!
//! ## Link highlighting
//!
//! For `[[links]]`, only the **passage name** is highlighted as a `Link` token,
//! not the surrounding `[[` and `]]` brackets or the display text:
//!
//! - `[[Target]]`           → highlight "Target"
//! - `[[Display->Target]]`  → highlight "Target" (the passage name after `->`)
//! - `[[Display|Target]]`   → highlight "Target" (the passage name after `|`)
//!
//! For implicit passage references (e.g., `Engine.play("name")`,
//! `data-passage="name"`), only the **passage name string** is highlighted
//! as a `PassageRef` token, not the surrounding API call syntax.
//!
//! For macro passage references (e.g., `<<goto "name">>`, `<<link "label" "name">>`)
//! the macro NAME gets a `Macro` token, and the passage name string inside
//! gets a `PassageRef` token. The `<<`, `>>`, and other arguments are left
//! to the TextMate grammar.

use std::ops::Range;

use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};
use super::links::{
    RE_LINK_ARROW, RE_LINK_PIPE, RE_LINK_SIMPLE,
    RE_DATA_PASSAGE, RE_ENGINE_PLAY, RE_ENGINE_GOTO,
    RE_STORY_GET, RE_STORY_PASSAGE, RE_STORY_HAS,
    RE_UI_GOTO, RE_UI_INCLUDE,
};
use super::vars::{RE_SET_MACRO, RE_VAR};
use super::blocks;
use super::lexer::ParsedHeader;
use super::macros;

/// Generate semantic tokens for a passage body.
///
/// Uses the string-aware macro scanner from `blocks.rs` instead of regex
/// to correctly handle `>` and `>>` inside macro conditions.
pub(crate) fn body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Macro tokens — highlight only the macro NAME, not the entire
    // <<name args>> span. This avoids:
    //   1. Overriding TextMate punctuation/parameter highlighting for
    //      <<, >>, and argument text (semantic tokens punch through TextMate)
    //   2. Overlapping with PassageRef tokens inside macro arguments
    //
    // The TextMate grammar provides punctuation and parameter scopes for
    // the parts we don't emit tokens for, so they still get colored.
    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        tokens.push(SemanticToken {
            start: body_offset + m.name_start,
            length: m.name_len,
            token_type: SemanticTokenType::Macro,
            modifier: None,
        });
    }

    // Variable tokens
    let mut init_spans: Vec<Range<usize>> = Vec::new();
    for caps in RE_SET_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        let var_start = body_offset + m.start() + m.as_str().find('$').unwrap_or(0);
        let var_name = format!("${}", caps.get(1).unwrap().as_str());
        let var_end = var_start + var_name.len();
        tokens.push(SemanticToken {
            start: var_start,
            length: var_name.len(),
            token_type: SemanticTokenType::Variable,
            modifier: Some(SemanticTokenModifier::Definition),
        });
        init_spans.push(var_start..var_end);
    }

    for caps in RE_VAR.captures_iter(body) {
        let full = caps.get(0).unwrap();
        let var_start = body_offset + full.start();
        let var_end = body_offset + full.end();
        let is_init = init_spans.iter().any(|s| {
            var_start >= s.start && var_end <= s.end
        });
        if !is_init {
            tokens.push(SemanticToken {
                start: var_start,
                length: full.end() - full.start(),
                token_type: SemanticTokenType::Variable,
                modifier: None,
            });
        }
    }

    // ── Link tokens: only highlight the passage name ────────────────────
    //
    // For [[Target]], highlight just "Target" (capture group 1).
    // For [[Display->Target]], highlight just "Target" (capture group 2).
    // For [[Display|Target]], highlight just "Target" (capture group 2).

    for caps in RE_LINK_ARROW.captures_iter(body) {
        // Capture group 2 is the target passage name after ->
        if let Some(target) = caps.get(2) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }
    for caps in RE_LINK_PIPE.captures_iter(body) {
        // Capture group 2 is the target passage name after |
        if let Some(target) = caps.get(2) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }
    for caps in RE_LINK_SIMPLE.captures_iter(body) {
        // Capture group 1 is the passage name
        if let Some(target) = caps.get(1) {
            tokens.push(SemanticToken {
                start: body_offset + target.start(),
                length: target.end() - target.start(),
                token_type: SemanticTokenType::Link,
                modifier: None,
            });
        }
    }

    tokens
}

/// Generate PassageRef semantic tokens for implicit passage references
/// in script passages (Engine.play, data-passage, etc.).
///
/// Unlike the `Link` type which highlights the passage name in `[[...]]`,
/// `PassageRef` highlights the passage name string inside API calls and
/// HTML attributes. Only the quoted passage name itself is highlighted,
/// not the surrounding syntax.
pub(crate) fn script_passage_ref_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    let patterns: &[&regex::Regex] = &[
        &RE_DATA_PASSAGE,
        &RE_ENGINE_PLAY,
        &RE_ENGINE_GOTO,
        &RE_STORY_GET,
        &RE_STORY_PASSAGE,
        &RE_STORY_HAS,
        &RE_UI_GOTO,
        &RE_UI_INCLUDE,
    ];

    for re in patterns {
        for caps in re.captures_iter(body) {
            // Capture group 1 is always the passage name string
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str().trim();
                if !name.is_empty() {
                    tokens.push(SemanticToken {
                        start: body_offset + name_match.start(),
                        length: name_match.end() - name_match.start(),
                        token_type: SemanticTokenType::PassageRef,
                        modifier: None,
                    });
                }
            }
        }
    }

    tokens
}

/// Generate PassageRef semantic tokens for macro passage references
/// (e.g., `<<goto "Passage">>`, `<<link "Label" "Passage">>`).
///
/// Only the passage name string is highlighted as PassageRef, not the
/// macro brackets or other arguments. The macro itself is already
/// highlighted by the `Macro` token from `body_tokens()`.
///
/// Uses the string-aware macro scanner from `blocks.rs` instead of regex
/// to correctly handle `>` and `>>` inside macro conditions.
pub(crate) fn macro_passage_ref_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let passage_arg_macros = macros::passage_arg_macro_names();

    let parsed_macros = blocks::scan_macros(body);
    for m in &parsed_macros {
        // Skip close tags
        if m.name.starts_with('/') {
            continue;
        }

        let macro_name = m.name.as_str();

        if !passage_arg_macros.contains(macro_name) {
            continue;
        }

        let args_str = m.args.as_str();
        if args_str.is_empty() {
            continue;
        }

        // Parse quoted string arguments
        let string_args = parse_quoted_args_with_spans(args_str);

        if string_args.is_empty() {
            continue;
        }

        // Determine which argument is the passage reference
        let arg_count = string_args.len();
        let passage_idx = macros::get_passage_arg_index(macro_name, arg_count);

        if passage_idx < 0 {
            continue;
        }

        let idx = passage_idx as usize;
        if idx < string_args.len() {
            let (content, rel_start, rel_end) = &string_args[idx];
            if !content.is_empty() {
                // rel_start/rel_end are relative to args_str, which starts at
                // name_end in the body. We need to find where args_str starts
                // in the body relative to the macro match.
                //
                // args_str = body[name_end..closing_gt_start].trim()
                // The trim might remove leading whitespace, so we need to find
                // the actual start of the trimmed args in the body.
                let name_end_in_body = m.name_start + m.name_len;
                let body_after_name = &body[name_end_in_body..m.end.saturating_sub(2)]; // before >>
                let trimmed_start = body_after_name.len() - body_after_name.trim_start().len();
                let args_offset_in_body = name_end_in_body + trimmed_start;

                tokens.push(SemanticToken {
                    start: body_offset + args_offset_in_body + *rel_start,
                    length: rel_end - rel_start,
                    token_type: SemanticTokenType::PassageRef,
                    modifier: None,
                });
            }
        }
    }

    tokens
}

/// Parse quoted string arguments from a macro's argument string, returning
/// both the content and the byte span (relative to the args string) of each
/// quoted argument (excluding the quote characters).
fn parse_quoted_args_with_spans(args: &str) -> Vec<(String, usize, usize)> {
    let mut result = Vec::new();
    let mut chars = args.char_indices().peekable();

    while let Some(&(_pos, c)) = chars.peek() {
        if c == '"' || c == '\'' {
            let quote = c;
            chars.next(); // consume opening quote
            let content_start = chars.peek().map(|&(i, _)| i).unwrap_or(args.len());
            let mut content = String::new();
            let mut content_end = content_start;
            while let Some(&(i, cc)) = chars.peek() {
                if cc == quote {
                    content_end = i;
                    chars.next(); // consume closing quote
                    break;
                }
                content.push(cc);
                content_end = i + cc.len_utf8();
                chars.next();
            }
            if !content.is_empty() {
                result.push((content, content_start, content_end));
            }
        } else {
            chars.next(); // skip non-quote characters
        }
    }

    result
}

/// Generate semantic tokens for the body of stylesheet passages.
///
/// Emits a `String`-type token covering the entire CSS body. This serves
/// as a fallback when the TextMate grammar doesn't activate CSS scopes
/// for stylesheet passages (tagged [stylesheet]).
/// The `String` type maps to the LSP `STRING` semantic token type, which
/// most themes color distinctly from plain text.
///
/// Note: If the TextMate grammar DOES provide CSS scopes, those will be
/// overridden by this semantic token. However, since the user reports CSS
/// is currently highlighted as plain text, this is a net improvement.
pub(crate) fn stylesheet_body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    if body.trim().is_empty() {
        return Vec::new();
    }
    vec![SemanticToken {
        start: body_offset,
        length: body.len(),
        token_type: SemanticTokenType::String,
        modifier: None,
    }]
}

/// Generate semantic tokens for the body of StoryInterface passages.
///
/// Emits a `String`-type token covering the entire HTML body. This serves
/// as a fallback when the TextMate grammar doesn't activate HTML scopes
/// for StoryInterface passages.
pub(crate) fn interface_body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    if body.trim().is_empty() {
        return Vec::new();
    }
    vec![SemanticToken {
        start: body_offset,
        length: body.len(),
        token_type: SemanticTokenType::String,
        modifier: None,
    }]
}

/// Generate semantic tokens for passage headers.
///
/// If `is_special` is true, the passage header tokens use the `SpecialPassage`
/// type instead of `PassageHeader`, giving special passages distinct visual
/// highlighting in the editor.
pub(crate) fn header_tokens(header: &ParsedHeader, is_special: bool) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    let header_type = if is_special {
        SemanticTokenType::SpecialPassage
    } else {
        SemanticTokenType::PassageHeader
    };

    // The `::` prefix is always 2 bytes.
    tokens.push(SemanticToken {
        start: header.header_start,
        length: 2,
        token_type: header_type.clone(),
        modifier: None,
    });

    // Passage name — use the pre-computed name_start which correctly accounts
    // for any whitespace between `::` and the name (e.g., `:: Start` has 1 space).
    tokens.push(SemanticToken {
        start: header.name_start,
        length: header.name.len(),
        token_type: header_type,
        modifier: None,
    });

    // Tags — positions are approximate since the header may have variable
    // whitespace between the name and the tag bracket.
    for (i, tag) in header.tags.iter().enumerate() {
        tokens.push(SemanticToken {
            start: header.name_start + header.name.len() + 2 + i * (tag.len() + 1),
            length: tag.len(),
            token_type: SemanticTokenType::Tag,
            modifier: None,
        });
    }

    tokens
}
