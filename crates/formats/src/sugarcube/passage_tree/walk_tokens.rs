//! Semantic token walks for the passage tree.
//!
//! Contains `walk_tokens()` and its inner recursive walk that produces
//! semantic tokens from the tree structure.

use super::PassageNode;

// ---------------------------------------------------------------------------
// walk_tokens() — Tree-based semantic tokens (replaces tokens::body_tokens)
// ---------------------------------------------------------------------------

/// Walk the tree and produce semantic tokens for the passage body.
///
/// Replaces `tokens::body_tokens()` which re-scans with `blocks::scan_macros()`
/// and multiple regex passes. This walk uses the tree's structural information
/// directly:
///
/// - **Macro tokens**: Name position from `ParsedMacro.name_start/name_len`
/// - **Variable tokens**: From `VarRef` entries on tree nodes (write = Definition)
/// - **Link tokens**: From `Link` entries on Text nodes
///
/// Additional token types (keyword, boolean, namespace, number, string, operator,
/// property, PassageRef) still use the existing `tokens.rs` helpers because they
/// require text-level scanning that doesn't benefit from tree structure. These
/// are called as augmentation passes after the tree-based core tokens.
pub(crate) fn walk_tokens(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
) -> Vec<crate::plugin::SemanticToken> {
    let mut tokens = Vec::new();
    walk_tokens_inner(nodes, body, body_offset, &mut tokens);

    tokens
}

fn walk_tokens_inner(
    nodes: &[PassageNode],
    body: &str,
    body_offset: usize,
    tokens: &mut Vec<crate::plugin::SemanticToken>,
) {
    use crate::plugin::{SemanticToken, SemanticTokenModifier, SemanticTokenType};

    for node in nodes {
        match node {
            PassageNode::Text { links, var_refs, .. } => {
                // Variable tokens from text nodes (reads)
                for vr in var_refs {
                    let is_init = vr.is_write;
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if is_init {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }

                // Link tokens — highlight the passage name portion of [[...]] links.
                // The tree builder computes link spans covering the full [[...]]
                // construct. We extract the target name's byte position by
                // slicing the body text within the link span.
                for link in links {
                    let link_body_start = link.span.start.saturating_sub(body_offset);
                    let link_body_end = link.span.end.saturating_sub(body_offset);
                    if link_body_start >= body.len() || link_body_end > body.len() {
                        continue;
                    }
                    let link_text = &body[link_body_start..link_body_end];
                    // Find the target portion within the [[...]] construct:
                    // [[Target]] → "Target"
                    // [[Display->Target]] → "Target" (after ->)
                    // [[Display|Target]] → "Target" (after |)
                    let target_start_in_link = if let Some(pos) = link_text.find("->") {
                        // Arrow-style: highlight after "->"
                        pos + 2
                    } else if let Some(pos) = link_text.find('|') {
                        // Pipe-style: highlight after "|"
                        pos + 1
                    } else {
                        // Simple: highlight after "[["
                        2
                    };
                    let target_end_in_link = link_text.len().saturating_sub(2); // before "]]"
                    if target_start_in_link < target_end_in_link {
                        tokens.push(SemanticToken {
                            start: link.span.start + target_start_in_link,
                            length: target_end_in_link - target_start_in_link,
                            token_type: SemanticTokenType::Link,
                            modifier: None,
                        });
                    }
                }
            }

            PassageNode::Macro {
                parsed,
                var_refs,
                children,
                ..
            } => {
                // Macro name token — highlight only the name, not <<args>>
                tokens.push(SemanticToken {
                    start: body_offset + parsed.name_start,
                    length: parsed.name_len,
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });

                // Variable tokens from macro args
                for vr in var_refs {
                    let is_init = vr.is_write;
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if is_init {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }

                // Recurse into children
                if let Some(children) = children {
                    walk_tokens_inner(children, body, body_offset, tokens);
                }
            }

            PassageNode::Expression { name, var_refs, span, .. } => {
                // Expression macros (<<=>> / <<->>) get a Macro token for
                // the name. The name position can be derived from the span:
                // <<name>> → name is at span.start + 2, length = name.len()
                tokens.push(SemanticToken {
                    start: span.start + 2, // skip "<<"
                    length: name.len(),
                    token_type: SemanticTokenType::Macro,
                    modifier: None,
                });

                // Variable tokens from expression args
                for vr in var_refs {
                    tokens.push(SemanticToken {
                        start: vr.span.start,
                        length: vr.span.end - vr.span.start,
                        token_type: SemanticTokenType::Variable,
                        modifier: if vr.is_write {
                            Some(SemanticTokenModifier::Definition)
                        } else {
                            None
                        },
                    });
                }
            }

            PassageNode::Heading { .. } | PassageNode::Error { .. } => {
                // No semantic tokens for headings or errors
            }
        }
    }
}
