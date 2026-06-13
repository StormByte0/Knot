//! Tree builder — pairs flat Macro/MacroClose nodes into a nested AST.
//!
//! The parser emits a flat list of AST nodes: `Macro` nodes with `children: None`
//! and `MacroClose` nodes for close tags. This module walks the flat list with a
//! stack, pairs open/close tags, and establishes parent-child relationships.
//!
//! After pairing, the tree builder also propagates **prose context**: Text nodes
//! inside non-rendering macros (`<<silently>>`, `<<script>>`, `<<style>>`) are
//! marked `is_prose = false`, while Text inside rendering macros (`<<if>>`,
//! `<<for>>`, `<<nobr>>`, etc.) remains `is_prose = true`.
//!
//! The result is the same nested AST structure that the old recursive parser
//! produced, but constructed from a clean separation of concerns:
//! - Parser: flat syntactic recognition (no block/inline awareness)
//! - Tree builder: structural pairing (uses catalog for BodyRequirement)
//! - JS annotation: oxc analysis on the nested AST

use std::ops::Range;

use crate::sugarcube::ast::AstNode;
use crate::types::BodyRequirement;
use crate::sugarcube::macros;

/// Build a nested AST from a flat list of AST nodes.
///
/// Walks the flat list, pairs `Macro` with `MacroClose`, and establishes
/// parent-child relationships. `MacroClose` nodes are consumed and their
/// span information is preserved on the parent `Macro`'s `close_span` and
/// `close_name_span` fields.
///
/// For macros without a matching `MacroClose`, the tree builder consults
/// the catalog's `BodyRequirement` to determine whether they're inline
/// or unclosed blocks.
///
/// After tree construction, prose context is propagated: Text nodes inside
/// non-rendering macros (`<<silently>>`, `<<script>>`, `<<style>>`) are
/// marked `is_prose = false`.
pub fn build_tree(flat: Vec<AstNode>) -> Vec<AstNode> {
    let mut stack: Vec<StackEntry> = Vec::new();
    let mut roots: Vec<AstNode> = Vec::new();

    for node in flat {
        match node {
            AstNode::MacroClose { name, name_span, span } => {
                // Find the matching open macro on the stack
                let match_idx = stack.iter().rposition(|entry| {
                    matches!(&entry.node, AstNode::Macro { name: n, .. } if n.eq_ignore_ascii_case(&name))
                        && entry.close_span.is_none() // not already closed
                });

                if let Some(idx) = match_idx {
                    // Pop everything above the match — these become the matched macro's children
                    let above: Vec<AstNode> = stack.drain(idx + 1..)
                        .flat_map(|e| e.into_nodes())
                        .collect();

                    let entry = &mut stack[idx];

                    // Collect children: the nodes that were between open and close
                    // (both the entry's pending children and the popped stack entries)
                    let mut children = std::mem::take(&mut entry.pending_children);
                    children.extend(above);

                    entry.children = Some(children);
                    entry.close_span = Some(span);
                    entry.close_name_span = Some(name_span);

                    // Update full_span to include the close tag
                    if let AstNode::Macro { full_span, .. } = &entry.node {
                        let new_end = entry.close_span.as_ref().unwrap().end;
                        // The full_span should cover from open to close
                        entry.full_span_end = Some(new_end.max(full_span.end));
                    }
                } else {
                    // Orphan close tag — no matching open macro.
                    // Emit it as an Error node so the user gets a diagnostic.
                    roots.push(AstNode::Error {
                        message: format!("Unexpected close tag: <</{}>>", name),
                        span: span.start..span.end,
                    });
                }
            }

            AstNode::Macro { .. } => {
                // Check if this macro already has children (script/style pre-nesting)
                let already_has_children = matches!(&node, AstNode::Macro { children: Some(_), .. });

                if already_has_children {
                    // Script/style macros are pre-nested by the parser.
                    // Just add to current context (don't push to stack).
                    let current = stack.last_mut();
                    if let Some(entry) = current {
                        entry.pending_children.push(node);
                    } else {
                        roots.push(node);
                    }
                } else {
                    // Push onto stack as a potential block parent
                    stack.push(StackEntry::from_macro(node));
                }
            }

            other => {
                // Text, Link, Expression, Comment, Error — add to current context
                let current = stack.last_mut();
                if let Some(entry) = current {
                    entry.pending_children.push(other);
                } else {
                    roots.push(other);
                }
            }
        }
    }

    // Process remaining stack entries — unmatched open macros
    for entry in stack {
        let node = entry.into_node_with_catalog();
        roots.push(node);
    }

    // Propagate prose context: mark Text nodes inside non-rendering macros
    propagate_prose_context(&mut roots);

    roots
}

// ---------------------------------------------------------------------------
// Stack entry — tracks an open macro awaiting its close tag
// ---------------------------------------------------------------------------

struct StackEntry {
    node: AstNode,
    pending_children: Vec<AstNode>,
    children: Option<Vec<AstNode>>,
    close_span: Option<Range<usize>>,
    close_name_span: Option<Range<usize>>,
    full_span_end: Option<usize>,
}

impl StackEntry {
    fn from_macro(node: AstNode) -> Self {
        StackEntry {
            node,
            pending_children: Vec::new(),
            children: None,
            close_span: None,
            close_name_span: None,
            full_span_end: None,
        }
    }

    /// Convert to a final AstNode, consulting the catalog for BodyRequirement
    /// to determine how to handle unmatched open macros.
    fn into_node_with_catalog(self) -> AstNode {
        match self.node {
            AstNode::Macro {
                name,
                args,
                var_refs,
                js_analysis,
                name_span,
                open_span,
                full_span,
                set_assignment,
                definition_name_span,
                capture_target,
                for_loop_vars,
                structured_args,
                ..
            } => {
                let body_req = lookup_body_requirement(&name);

                match (self.close_span.is_some(), body_req) {
                    // Close tag found — it's a block macro with children
                    (true, _) => {
                        let close_span = self.close_span.unwrap();
                        let close_name_span = self.close_name_span;
                        let full_end = self.full_span_end.unwrap_or(full_span.end);
                        AstNode::Macro {
                            name,
                            args,
                            var_refs,
                            js_analysis,
                            children: self.children,
                            name_span,
                            open_span,
                            close_span: Some(close_span),
                            full_span: full_span.start..full_end,
                            set_assignment,
                            definition_name_span,
                            close_name_span,
                            capture_target,
                            for_loop_vars,
                            structured_args,
                        }
                    }

                    // No close tag, Never body — inline macro (correct)
                    (false, BodyRequirement::Never) => {
                        AstNode::Macro {
                            name,
                            args,
                            var_refs,
                            js_analysis,
                            children: None,
                            name_span,
                            open_span,
                            close_span: None,
                            full_span,
                            set_assignment,
                            definition_name_span,
                            close_name_span: None,
                            capture_target,
                            for_loop_vars,
                            structured_args,
                        }
                    }

                    // No close tag, Optional body — inline form (correct, no error)
                    (false, BodyRequirement::Optional) => {
                        AstNode::Macro {
                            name,
                            args,
                            var_refs,
                            js_analysis,
                            children: None,
                            name_span,
                            open_span,
                            close_span: None,
                            full_span,
                            set_assignment,
                            definition_name_span,
                            close_name_span: None,
                            capture_target,
                            for_loop_vars,
                            structured_args,
                        }
                    }

                    // No close tag, Required body — unclosed block (error)
                    // The pending children become the macro's body (same as old parser)
                    (false, BodyRequirement::Required) => {
                        let children = if self.pending_children.is_empty() {
                            None
                        } else {
                            Some(self.pending_children)
                        };
                        AstNode::Macro {
                            name,
                            args,
                            var_refs,
                            js_analysis,
                            children,
                            name_span,
                            open_span,
                            close_span: None, // None signals "unclosed" → diagnostic
                            full_span,
                            set_assignment,
                            definition_name_span,
                            close_name_span: None,
                            capture_target,
                            for_loop_vars,
                            structured_args,
                        }
                    }
                }
            }

            // Non-macro nodes should never be on the stack
            other => other,
        }
    }

    /// Convert into a list of nodes (for when this entry is popped without
    /// a matching close tag and needs to be re-emitted).
    fn into_nodes(self) -> Vec<AstNode> {
        let node = self.into_node_with_catalog();
        let result = vec![node];
        // Pending children that were collected while this was on the stack
        // are already handled by into_node_with_catalog
        result
    }
}

/// Look up a macro's BodyRequirement from the catalog.
///
/// Returns `BodyRequirement::Never` for unknown macros (treat as inline
/// unless we find a close tag for them).
fn lookup_body_requirement(name: &str) -> BodyRequirement {
    if let Some(mdef) = macros::find_macro(name) {
        mdef.body
    } else {
        // Unknown macro — assume inline. If a close tag is found,
        // the tree builder will pair it regardless.
        BodyRequirement::Never
    }
}

// ---------------------------------------------------------------------------
// Prose context propagation
// ---------------------------------------------------------------------------

/// Walk the AST and set `is_prose = false` on Text nodes inside non-rendering
/// macros. Top-level Text nodes and Text inside rendering macros retain their
/// default `is_prose = true`.
///
/// A **non-rendering macro** is one whose body content is NOT displayed to the
/// player. In SugarCube, this is:
/// - `<<silently>>` — executes code but produces no output
/// - `<<script>>` — opaque JS (already handled with `is_prose = false` at
///   parse time, but included here for completeness)
/// - `<<style>>` / `<<css>>` — opaque CSS (same as script)
///
/// All other block macros render their body content (even `<<if>>`, `<<for>>`,
/// `<<nobr>>`, `<<capture>>`, `<<type>>`, `<<widget>>`, `<<link>>`,
/// `<<button>>`, `<<click>>`, `<<switch>>`, `<<timed>>`, `<<repeat>>`, etc.)
/// — their Text children are prose.
fn propagate_prose_context(nodes: &mut [AstNode]) {
    for node in nodes.iter_mut() {
        if let AstNode::Macro { name, children, .. } = node {
            if !is_prose_rendering_macro(name) {
                // Non-rendering macro: mark all descendant Text nodes as non-prose
                if let Some(ch) = children {
                    mark_non_prose(ch);
                }
            } else if let Some(ch) = children {
                // Rendering macro: recurse to check nested macros
                propagate_prose_context(ch);
            }
        }
    }
}

/// Mark all Text nodes in the given list (and their descendants) as non-prose.
fn mark_non_prose(nodes: &mut [AstNode]) {
    for node in nodes.iter_mut() {
        match node {
            AstNode::Text { is_prose, .. } => {
                *is_prose = false;
            }
            AstNode::Macro { children, .. } => {
                if let Some(ch) = children {
                    mark_non_prose(ch);
                }
            }
            _ => {}
        }
    }
}

/// Returns `true` if the named macro renders its body content as narrative
/// output (prose). Returns `false` for macros whose body is code or is
/// suppressed.
///
/// The default is `true` — most block macros render their content. Only
/// `<<silently>>`, `<<done>>`, `<<script>>`, and `<<style>>`/`<<css>>` are
/// non-rendering. `<<done>>` executes code after rendering, so its body is
/// imperative code rather than narrative prose.
fn is_prose_rendering_macro(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !matches!(lower.as_str(), "silently" | "done" | "script" | "style" | "css")
}
