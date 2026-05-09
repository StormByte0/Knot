//! Semantic token generation for SugarCube.
//!
//! Produces semantic tokens for macro invocations, variable references,
//! passage links, and passage headers for use in LSP semantic highlighting.

use std::ops::Range;

use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};
use super::links::{RE_LINK_ARROW, RE_LINK_PIPE, RE_LINK_SIMPLE};
use super::vars::{RE_SET_MACRO, RE_VAR};
use super::blocks::{RE_MACRO, RE_MACRO_CLOSE};
use super::lexer::ParsedHeader;

/// Generate semantic tokens for a passage body.
pub(crate) fn body_tokens(body: &str, body_offset: usize) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Macro tokens
    for caps in RE_MACRO.captures_iter(body) {
        let m = caps.get(0).unwrap();
        tokens.push(SemanticToken {
            start: body_offset + m.start(),
            length: m.end() - m.start(),
            token_type: SemanticTokenType::Macro,
            modifier: None,
        });
    }
    for caps in RE_MACRO_CLOSE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        tokens.push(SemanticToken {
            start: body_offset + m.start(),
            length: m.end() - m.start(),
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

    // Link tokens
    for caps in RE_LINK_ARROW.captures_iter(body) {
        let m = caps.get(0).unwrap();
        tokens.push(SemanticToken {
            start: body_offset + m.start(),
            length: m.end() - m.start(),
            token_type: SemanticTokenType::Link,
            modifier: None,
        });
    }
    for caps in RE_LINK_PIPE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        tokens.push(SemanticToken {
            start: body_offset + m.start(),
            length: m.end() - m.start(),
            token_type: SemanticTokenType::Link,
            modifier: None,
        });
    }
    for caps in RE_LINK_SIMPLE.captures_iter(body) {
        let m = caps.get(0).unwrap();
        tokens.push(SemanticToken {
            start: body_offset + m.start(),
            length: m.end() - m.start(),
            token_type: SemanticTokenType::Link,
            modifier: None,
        });
    }

    tokens
}

/// Generate semantic tokens for passage headers.
pub(crate) fn header_tokens(header: &ParsedHeader) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // The `::` prefix is always 2 bytes.
    tokens.push(SemanticToken {
        start: header.header_start,
        length: 2,
        token_type: SemanticTokenType::PassageHeader,
        modifier: None,
    });

    // Passage name starts after `:: ` (2 for :: + however many spaces).
    let name_start = header.header_start + 2;
    tokens.push(SemanticToken {
        start: name_start,
        length: header.name.len(),
        token_type: SemanticTokenType::PassageHeader,
        modifier: None,
    });

    // Tags
    for (i, tag) in header.tags.iter().enumerate() {
        // Tags are inside [tag1 tag2], approximate their positions.
        // This is best-effort for semantic highlighting.
        tokens.push(SemanticToken {
            start: name_start + header.name.len() + 2 + i * (tag.len() + 1),
            length: tag.len(),
            token_type: SemanticTokenType::Tag,
            modifier: None,
        });
    }

    tokens
}
