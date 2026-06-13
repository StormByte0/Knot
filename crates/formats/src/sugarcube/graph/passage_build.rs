//! Pure transformation functions for building [`Passage`] from parsed AST.
//!
//! None of these functions access plugin state; they are pure conversions
//! from the SugarCube AST representation to the core `Passage` type.

use knot_core::passage::{Block, MacroArgRef, Passage, VarKind, VarOp};
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
///
/// `body_offset_in_passage` is the passage-relative offset of the body region;
/// all spans produced here are passage-relative (0 = start of passage header `::`).
pub fn build_body_blocks(nodes: &[ast::AstNode], body_offset_in_passage: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    for node in nodes {
        match node {
            ast::AstNode::Text { content, span, .. } => {
                if !content.is_empty() {
                    blocks.push(Block::Text {
                        content: content.clone(),
                        span: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                    });
                }
            }
            ast::AstNode::Macro { name, args, full_span, .. } => {
                blocks.push(Block::Macro {
                    name: name.clone(),
                    args: args.clone(),
                    span: body_offset_in_passage + full_span.start..body_offset_in_passage + full_span.end,
                });
            }
            ast::AstNode::Expression { content, span, .. } => {
                blocks.push(Block::Expression {
                    content: content.clone(),
                    span: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                });
            }
            ast::AstNode::Link { .. } => {
                // Links in body are represented as text blocks for backward compat
                // The actual Link data is in passage.links
            }
            ast::AstNode::Comment { .. } => {
                // Comments don't produce body blocks
            }
            ast::AstNode::InlineStyle { class, span, .. } => {
                blocks.push(Block::Text {
                    content: class.clone(),
                    span: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                });
            }
            ast::AstNode::TextFormat { content, span, .. } => {
                blocks.push(Block::Text {
                    content: content.clone(),
                    span: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                });
            }
            ast::AstNode::Error { message, span } => {
                blocks.push(Block::Incomplete {
                    content: message.clone(),
                    span: body_offset_in_passage + span.start..body_offset_in_passage + span.end,
                });
            }
            // MacroClose nodes are consumed by the tree builder and should not
            // appear in the final AST. If one slips through, skip it.
            ast::AstNode::MacroClose { .. } => {}
        }
    }
    blocks
}

/// Build a [`Passage`] from a classified passage and its AST.
///
/// This is a pure transformation — it does not read or write any plugin state.
///
/// All spans in the returned `Passage` are **passage-relative**: offset 0
/// corresponds to the `::` prefix of the passage header. The caller must set
/// `passage.passage_offset = passage_head` so that LSP handlers can convert
/// back to document-absolute positions at the boundary.
///
/// * `body_offset_in_passage` — passage-relative offset of the body region.
/// * `passage_head` — document-absolute byte offset of the passage header `::`.
pub fn build_passage(
    cp: &ClassifiedPassage,
    passage_ast: &PassageAst,
    body_offset_in_passage: usize,
    passage_head: usize,
) -> Passage {
    let mut passage = if let Some(ref special_def) = cp.special_def {
        Passage::new_special(
            cp.header.name.clone(),
            0..0, // passage-relative: passage head is at 0; full span computed in caller
            special_def.clone(),
        )
    } else {
        Passage::new(cp.header.name.clone(), 0..0) // passage-relative: passage head is at 0
    };

    passage.tags = cp.header.tags.clone();

    // Store the header name span for fine-grained LSP position resolution.
    // The name starts at `name_start` (after `::` + whitespace) and extends
    // for `name.len()` bytes. `name_start` is document-absolute, so subtract
    // `passage_head` to make it passage-relative.
    passage.header_name_span = Some(
        (cp.header.name_start - passage_head)..(cp.header.name_start - passage_head + cp.header.name.len())
    );

    // Build body blocks from AST (shift spans by body_offset_in_passage)
    passage.body = build_body_blocks(&passage_ast.nodes, body_offset_in_passage);

    // Build links from AST (shift spans by body_offset_in_passage)
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
                span: body_offset_in_passage + link_info.span.start..body_offset_in_passage + link_info.span.end,
                edge_type_hint,
            }
        }).collect();

    // Build var ops from AST (shift spans by body_offset_in_passage)
    passage.vars = passage_ast.var_ops.iter().map(|var_op| {
        VarOp {
            name: var_op.name.clone(),
            kind: if var_op.is_write { VarKind::Init } else { VarKind::Read },
            span: body_offset_in_passage + var_op.span.start..body_offset_in_passage + var_op.span.end,
            is_temporary: var_op.is_temporary,
        }
    }).collect();

    // Build macro arg refs from AST (passage-ref args with individual spans
    // for layered hover). Only `PassageRef` args are stored — other arg
    // kinds don't need layering.
    passage.macro_arg_refs = build_macro_arg_refs(&passage_ast.nodes, body_offset_in_passage);

    // Narrow link spans: macro-based links from `extract_macro_passage_refs()`
    // use the entire `open_span` as the link span (e.g., the whole
    // `<<link "Talk" "Shop">>` range). This is too broad — hovering over
    // "Talk" (display text) would trigger link hover for "Shop" (target).
    //
    // We fix this by cross-referencing with `macro_arg_refs`: for each link
    // whose target matches a `MacroArgRef`, narrow the link's span to the
    // arg's individual span (just the passage name, e.g., just "Shop").
    // `[[passage]]` links are unaffected — they don't have `macro_arg_refs`.
    narrow_link_spans(&mut passage.links, &passage.macro_arg_refs);

    // Record the document-absolute offset of the passage head so that
    // handlers can convert passage-relative spans back to document-absolute
    // positions at the LSP boundary.
    passage.passage_offset = passage_head;

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
pub fn build_vars_from_unified_ast(passage_ast: &PassageAst, body_offset_in_passage: usize) -> Vec<VarOp> {
    let mut vars = Vec::new();

    // For script passages, collect from script_js_analysis
    if let Some(ref analysis) = passage_ast.script_js_analysis {
        for op in &analysis.var_ops {
            vars.push(VarOp {
                name: op.name.clone(),
                kind: if op.access_kind.is_write() { VarKind::Init } else { VarKind::Read },
                span: body_offset_in_passage + op.span.start..body_offset_in_passage + op.span.end,
                is_temporary: op.is_temporary,
            });
        }
    }

    // Walk AST nodes
    collect_vars_from_nodes(&passage_ast.nodes, &mut vars, body_offset_in_passage);

    // If we got nothing from js_analysis, fall back to the parser's var_ops
    if vars.is_empty() && !passage_ast.var_ops.is_empty() {
        for var_op in &passage_ast.var_ops {
            vars.push(VarOp {
                name: var_op.name.clone(),
                kind: if var_op.is_write { VarKind::Init } else { VarKind::Read },
                span: body_offset_in_passage + var_op.span.start..body_offset_in_passage + var_op.span.end,
                is_temporary: var_op.is_temporary,
            });
        }
    }

    vars
}

/// Narrow link spans to match individual passage-ref arg spans.
///
/// Macro-based links (e.g., from `<<link "Talk" "Shop">>`) currently use the
/// entire macro open span as the link span. This function cross-references
/// with `macro_arg_refs` to narrow each link's span to just the passage-ref
/// arg's individual span (e.g., just "Shop" instead of the whole `<<link ...>>`).
///
/// For each link, we look for a `MacroArgRef` with a matching `target` whose
/// `macro_open_span` overlaps with the link's span. If found, we replace the
/// link's span with the arg's narrower span.
///
/// `[[passage]]` links are unaffected — they have no matching `MacroArgRef`.
fn narrow_link_spans(links: &mut [knot_core::passage::Link], arg_refs: &[MacroArgRef]) {
    for link in links.iter_mut() {
        // Find a MacroArgRef that targets the same passage and whose macro
        // open span overlaps with this link's span. We check overlap rather
        // than equality because the link span may be the open_span itself
        // (for single-arg macros like <<goto "Passage">>) or may differ
        // slightly due to span computation differences.
        for arg_ref in arg_refs {
            if arg_ref.target == link.target {
                // Check if the link span overlaps with the macro_open_span.
                // A link from `<<link "Talk" "Shop">>` has span == open_span,
                // so the overlap is exact. A `[[Shop]]` link has no macro_open_span
                // and won't match any arg_ref.
                let link_overlaps_macro = link.span.start >= arg_ref.macro_open_span.start
                    && link.span.end <= arg_ref.macro_open_span.end;
                if link_overlaps_macro {
                    link.span = arg_ref.span.clone();
                    break; // One arg ref per link
                }
            }
        }
    }
}

/// Build `MacroArgRef` entries from the AST for layered hover.
///
/// Walks all `AstNode::Macro` nodes and extracts `StructuredMacroArg` entries
/// where `kind == ParsedArgKind::PassageRef`. Each produces a `MacroArgRef`
/// with:
/// - The passage name as `target`
/// - The arg's individual span as `span` (shifted by `body_offset_in_passage`)
/// - The macro's `name_span` as `macro_name_span` (shifted)
/// - The macro's `open_span` as `macro_open_span` (shifted)
///
/// Recurses into block macro children.
pub fn build_macro_arg_refs(nodes: &[ast::AstNode], body_offset_in_passage: usize) -> Vec<MacroArgRef> {
    let mut refs = Vec::new();
    collect_macro_arg_refs(nodes, &mut refs, body_offset_in_passage);
    refs
}

fn collect_macro_arg_refs(nodes: &[ast::AstNode], refs: &mut Vec<MacroArgRef>, body_offset_in_passage: usize) {
    let label_then_passage: std::collections::HashSet<&str> =
        crate::sugarcube::macros::label_then_passage_macros();

    for node in nodes {
        if let ast::AstNode::Macro {
            name,
            name_span,
            open_span,
            children,
            structured_args,
            ..
        } = node {
            // `children: Some(_)` means the macro has a body (block variant with
            // close tag). `None` means inline (no body, no close tag). Container
            // macros like <<link>> always have children; Inline macros never do.
            let has_body = children.is_some();

            if let Some(sargs) = structured_args {
                for sarg in sargs {
                    let is_passage_ref = matches!(sarg.kind, ast::ParsedArgKind::PassageRef);
                    // For label_then_passage macros (e.g., <<link "Talk">>), when the
                    // single arg is classified as Label, it doubles as the passage target
                    // (equivalent to [[Talk]]). Treat it as a PassageRef for hover layering.
                    let is_label_as_passage = !is_passage_ref
                        && matches!(sarg.kind, ast::ParsedArgKind::Label)
                        && label_then_passage.contains(name.as_str())
                        && sargs.len() == 1;

                    if is_passage_ref || is_label_as_passage {
                        refs.push(MacroArgRef {
                            target: sarg.value.clone(),
                            span: body_offset_in_passage + sarg.span.start..body_offset_in_passage + sarg.span.end,
                            macro_name: name.clone(),
                            macro_name_span: body_offset_in_passage + name_span.start..body_offset_in_passage + name_span.end,
                            macro_open_span: body_offset_in_passage + open_span.start..body_offset_in_passage + open_span.end,
                            has_body,
                        });
                    }
                }
            }
            // Recurse into block macro children
            if let Some(ch) = children {
                collect_macro_arg_refs(ch, refs, body_offset_in_passage);
            }
        }
    }
}

fn collect_vars_from_nodes(nodes: &[ast::AstNode], vars: &mut Vec<VarOp>, body_offset_in_passage: usize) {
    for node in nodes {
        match node {
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    vars.push(VarOp {
                        name: vr.name.clone(),
                        kind: VarKind::Read,
                        span: body_offset_in_passage + vr.span.start..body_offset_in_passage + vr.span.end,
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
                                span: body_offset_in_passage + op.span.start..body_offset_in_passage + op.span.end,
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
                                span: body_offset_in_passage + sa.target.span.start..body_offset_in_passage + sa.target.span.end,
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
                            span: body_offset_in_passage + vr.span.start..body_offset_in_passage + vr.span.end,
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
                                span: body_offset_in_passage + sa.target.span.start..body_offset_in_passage + sa.target.span.end,
                                is_temporary: sa.target.is_temporary,
                            });
                        }
                    }
                }
                if let Some(ch) = children {
                    collect_vars_from_nodes(ch, vars, body_offset_in_passage);
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
                                span: body_offset_in_passage + op.span.start..body_offset_in_passage + op.span.end,
                                is_temporary: op.is_temporary,
                            });
                        }
                    }
                } else {
                    for vr in var_refs {
                        vars.push(VarOp {
                            name: vr.name.clone(),
                            kind: VarKind::Read,
                            span: body_offset_in_passage + vr.span.start..body_offset_in_passage + vr.span.end,
                            is_temporary: vr.is_temporary,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
