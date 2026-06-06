//! Pure transformation functions for building [`Passage`] from parsed AST.
//!
//! None of these functions access plugin state; they are pure conversions
//! from the SugarCube AST representation to the core `Passage` type.

use knot_core::passage::{Block, Passage, VarKind, VarOp};
use crate::sugarcube::ast::{self, PassageAst};
use crate::sugarcube::classifier::ClassifiedPassage;

/// Convert a `LinkSource` to the corresponding `EdgeType` hint.
///
/// This mapping is the single source of truth for how SugarCube link sources
/// map to graph edge types. By setting the hint at extraction time, we avoid
/// the post-hoc `classify_edge()` substring matching that can produce false
/// positives (e.g., `args.contains(target)` matching substrings).
pub fn link_source_to_edge_type(source: ast::LinkSource) -> Option<knot_core::graph::EdgeType> {
    Some(source.to_edge_type())
}

/// Build `Block` list from AST nodes (backward compatibility).
pub fn build_body_blocks(nodes: &[ast::AstNode], body_offset: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    for node in nodes {
        match node {
            ast::AstNode::Text { content, span, .. } => {
                if !content.is_empty() {
                    blocks.push(Block::Text {
                        content: content.clone(),
                        span: body_offset + span.start..body_offset + span.end,
                    });
                }
            }
            ast::AstNode::Macro { name, args, full_span, .. } => {
                blocks.push(Block::Macro {
                    name: name.clone(),
                    args: args.clone(),
                    span: body_offset + full_span.start..body_offset + full_span.end,
                });
            }
            ast::AstNode::Expression { content, span, .. } => {
                blocks.push(Block::Expression {
                    content: content.clone(),
                    span: body_offset + span.start..body_offset + span.end,
                });
            }
            ast::AstNode::Link { .. } => {
                // Links in body are represented as text blocks for backward compat
                // The actual Link data is in passage.links
            }
            ast::AstNode::Comment { .. } => {
                // Comments don't produce body blocks
            }
            ast::AstNode::Error { message, span } => {
                blocks.push(Block::Incomplete {
                    content: message.clone(),
                    span: body_offset + span.start..body_offset + span.end,
                });
            }
        }
    }
    blocks
}

/// Build a [`Passage`] from a classified passage and its AST.
///
/// This is a pure transformation — it does not read or write any plugin state.
pub fn build_passage(cp: &ClassifiedPassage, passage_ast: &PassageAst, body_offset: usize) -> Passage {
    let is_special = cp.special_def.is_some();
    let mut passage = if is_special {
        Passage::new_special(
            cp.header.name.clone(),
            cp.header.header_start..cp.header.header_start, // span computed in caller
            cp.special_def.clone().unwrap(),
        )
    } else {
        Passage::new(cp.header.name.clone(), cp.header.header_start..cp.header.header_start)
    };

    passage.tags = cp.header.tags.clone();

    // Build body blocks from AST (shift spans by body_offset)
    passage.body = build_body_blocks(&passage_ast.nodes, body_offset);

    // Build links from AST (shift spans by body_offset)
    // Skip links with empty targets — these are dynamic navigation macros
    // (e.g., <<back "Display">> or <<return "Display">>) where the target
    // is determined at runtime via browser history. An empty target would
    // create a false "BrokenLink" diagnostic.
    passage.links = passage_ast.links.iter()
        .filter(|link_info| !link_info.target.is_empty())
        .map(|link_info| {
            let edge_type_hint = link_source_to_edge_type(link_info.source);
            knot_core::passage::Link {
                display_text: link_info.display.clone(),
                target: link_info.target.clone(),
                span: body_offset + link_info.span.start..body_offset + link_info.span.end,
                edge_type_hint,
            }
        }).collect();

    // Build var ops from AST (shift spans by body_offset)
    passage.vars = passage_ast.var_ops.iter().map(|var_op| {
        VarOp {
            name: var_op.name.clone(),
            kind: if var_op.is_write { VarKind::Init } else { VarKind::Read },
            span: body_offset + var_op.span.start..body_offset + var_op.span.end,
            is_temporary: var_op.is_temporary,
        }
    }).collect();

    passage
}
