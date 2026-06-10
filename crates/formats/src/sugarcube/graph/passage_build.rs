//! Pure transformation functions for building [`Passage`] from parsed AST.
//!
//! None of these functions access plugin state; they are pure conversions
//! from the SugarCube AST representation to the core `Passage` type.

use knot_core::passage::{Block, Passage, VarKind, VarOp};
use crate::sugarcube::ast::{self, PassageAst};
use crate::sugarcube::classifier::ClassifiedPassage;
use crate::sugarcube::parser::predicates::is_assignment_macro;

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

/// Build var ops from the unified AST (including js_analysis + script_js_analysis).
///
/// This walks the AST and collects variable operations from:
/// - `PassageAst::script_js_analysis` (for script passages)
/// - `AstNode::Macro { js_analysis }` (for macros with JS annotation)
/// - `AstNode::Expression { js_analysis }` (for expressions with JS annotation)
/// - `AstNode::Text { var_refs }` (prose variable references)
/// - `AstNode::Macro { var_refs }` (fallback when js_analysis is None)
/// - `AstNode::Expression { var_refs }` (fallback when js_analysis is None)
///
/// Falls back to the SugarCube parser's `var_ops` when no js_analysis is available.
pub fn build_vars_from_unified_ast(passage_ast: &PassageAst, body_offset: usize) -> Vec<VarOp> {
    let mut vars = Vec::new();

    // For script passages, collect from script_js_analysis
    if let Some(ref analysis) = passage_ast.script_js_analysis {
        for op in &analysis.var_ops {
            vars.push(VarOp {
                name: op.name.clone(),
                kind: if op.access_kind.is_write() { VarKind::Init } else { VarKind::Read },
                span: body_offset + op.span.start..body_offset + op.span.end,
                is_temporary: op.is_temporary,
            });
        }
    }

    // Walk AST nodes
    collect_vars_from_nodes(&passage_ast.nodes, &mut vars, body_offset);

    // If we got nothing from js_analysis, fall back to the parser's var_ops
    if vars.is_empty() && !passage_ast.var_ops.is_empty() {
        for var_op in &passage_ast.var_ops {
            vars.push(VarOp {
                name: var_op.name.clone(),
                kind: if var_op.is_write { VarKind::Init } else { VarKind::Read },
                span: body_offset + var_op.span.start..body_offset + var_op.span.end,
                is_temporary: var_op.is_temporary,
            });
        }
    }

    vars
}

fn collect_vars_from_nodes(nodes: &[ast::AstNode], vars: &mut Vec<VarOp>, body_offset: usize) {
    for node in nodes {
        match node {
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    vars.push(VarOp {
                        name: vr.name.clone(),
                        kind: VarKind::Read,
                        span: body_offset + vr.span.start..body_offset + vr.span.end,
                        is_temporary: vr.is_temporary,
                    });
                }
            }
            ast::AstNode::Macro { js_analysis, var_refs, children, set_assignment, name, .. } => {
                // Determine whether to use js_analysis or fall back to var_refs
                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());

                if has_js_analysis {
                    // Use oxc-derived var_ops (more accurate read/write classification)
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            vars.push(VarOp {
                                name: op.name.clone(),
                                kind: if op.access_kind.is_write() { VarKind::Init } else { VarKind::Read },
                                span: body_offset + op.span.start..body_offset + op.span.end,
                                is_temporary: op.is_temporary,
                            });
                        }
                    }
                    // Also emit the set_assignment target (SugarCube-owned, not in oxc output)
                    // UNLESS a block write from js_analysis already covers it
                    if let Some(sa) = set_assignment {
                        let block_write_covers = js_analysis.as_ref().is_some_and(|analysis| {
                            analysis.var_ops.iter().any(|op| {
                                op.name == sa.target.name
                                    && op.property_path == sa.target.property_path
                                    && op.construct_span.is_some()
                            })
                        });
                        if !block_write_covers {
                            vars.push(VarOp {
                                name: sa.target.name.clone(),
                                kind: VarKind::Init,
                                span: body_offset + sa.target.span.start..body_offset + sa.target.span.end,
                                is_temporary: sa.target.is_temporary,
                            });
                        }
                    }
                } else {
                    // Fall back to var_refs (from SugarCube parser's scan_inline_vars)
                    // For assignment macros, the target is in set_assignment, not var_refs
                    let is_assignment = is_assignment_macro(name);
                    for vr in var_refs {
                        vars.push(VarOp {
                            name: vr.name.clone(),
                            kind: if vr.is_write || is_assignment { VarKind::Init } else { VarKind::Read },
                            span: body_offset + vr.span.start..body_offset + vr.span.end,
                            is_temporary: vr.is_temporary,
                        });
                    }
                    // Emit set_assignment target if present (not covered by var_refs)
                    if let Some(sa) = set_assignment {
                        // Check if the target is already in vars from var_refs
                        let already_emitted = vars.iter().any(|v| {
                            v.name == sa.target.name && v.kind == VarKind::Init
                        });
                        if !already_emitted {
                            vars.push(VarOp {
                                name: sa.target.name.clone(),
                                kind: VarKind::Init,
                                span: body_offset + sa.target.span.start..body_offset + sa.target.span.end,
                                is_temporary: sa.target.is_temporary,
                            });
                        }
                    }
                }
                if let Some(ch) = children {
                    collect_vars_from_nodes(ch, vars, body_offset);
                }
            }
            ast::AstNode::Expression { js_analysis, var_refs, .. } => {
                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());
                if has_js_analysis {
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            vars.push(VarOp {
                                name: op.name.clone(),
                                kind: if op.access_kind.is_write() { VarKind::Init } else { VarKind::Read },
                                span: body_offset + op.span.start..body_offset + op.span.end,
                                is_temporary: op.is_temporary,
                            });
                        }
                    }
                } else {
                    for vr in var_refs {
                        vars.push(VarOp {
                            name: vr.name.clone(),
                            kind: VarKind::Read,
                            span: body_offset + vr.span.start..body_offset + vr.span.end,
                            is_temporary: vr.is_temporary,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
