//! Registry population from parsed AST.
//!
//! These functions mutate the sub-registries inside [`SugarCubeRegistry`]
//! during the ordered parse pipeline so that registries are warm for later
//! passages. The hub provides coordinated access to all sub-registries:
//!
//! - **VariableTree** — `$var` / `_var` references with nuanced read/write classification
//! - **CustomMacroRegistry** — `<<widget>>` and `Macro.add()` definitions
//! - **FunctionRegistry** — JS function declarations in `[script]` passages
//! - **TemplateRegistry** — `Template.add()` definitions in `[script]` passages
//!
//! ## Unified AST pipeline (Phase 6)
//!
//! After the unified AST refactoring, this module provides two population paths:
//!
//! 1. `populate_registries_from_unified_ast()` — Phase 3 of the 3-phase pipeline.
//!    Walks the enriched AST (with `js_analysis` attached to nodes) and populates
//!    registries from `JsAnalysis`. This is the preferred path.
//!
//! 2. `populate_registries_from_ast()` — Backward-compatible wrapper that delegates
//!    to the unified path. After Phase 6, this is a thin wrapper.
//!
//! 3. `walk_script_js()` — Kept temporarily for backward compat during migration.

use super::SugarCubeRegistry;
use super::variable_tree::{VarAccessKind, VarOrigin};
use crate::sugarcube::ast::{self, AnalyzedVarOp, SetOperator};
use crate::sugarcube::classifier::ClassifiedPassage;
use crate::sugarcube::js::js_annotate::compute_target_segment_spans;
use crate::sugarcube::macros::find_macro;
use crate::sugarcube::parser::predicates::is_assignment_macro;
use crate::sugarcube::registries::function_registry::FunctionKind;
use crate::sugarcube::registries::template_registry::TemplateKind;
use knot_core::zoning::ZoneMap;

/// Map a `SetOperator` from the AST to the appropriate `VarAccessKind`.
fn set_operator_to_access_kind(op: &SetOperator) -> VarAccessKind {
    match op {
        SetOperator::To | SetOperator::Eq | SetOperator::Into => VarAccessKind::Write,
        SetOperator::PlusEq
        | SetOperator::MinusEq
        | SetOperator::StarEq
        | SetOperator::SlashEq
        | SetOperator::PercentEq => VarAccessKind::CompoundWrite,
        SetOperator::PostfixPlus | SetOperator::PostfixMinus => VarAccessKind::PostfixModify,
    }
}

/// Determine the `VarAccessKind` for a macro that isn't `<<set>>`.
#[allow(dead_code)]
fn macro_name_to_access_kind(name: &str) -> VarAccessKind {
    if name.eq_ignore_ascii_case("capture") {
        VarAccessKind::Capture
    } else if name.eq_ignore_ascii_case("unset") {
        VarAccessKind::Unset
    } else if name.eq_ignore_ascii_case("set") {
        VarAccessKind::Write
    } else {
        VarAccessKind::Read
    }
}

// ---------------------------------------------------------------------------
// Unified registry population (Phase 3 of 3-phase pipeline)
// ---------------------------------------------------------------------------

/// Populate registries from the unified AST (Phase 3).
///
/// Walks the AST once. For each node:
///
/// | Node type | Source | Action |
/// |---|---|---|
/// | `PassageAst::script_js_analysis` | oxc (script passage) | Record var_ops directly |
/// | `AstNode::Text { var_refs }` | Custom scanner (prose) | Record var_refs as Read |
/// | `AstNode::Macro { js_analysis, name, set_assignment }` | oxc (macro args) | Record js_analysis.var_ops, apply SugarCube semantic overrides |
/// | `AstNode::Expression { js_analysis }` | oxc (expression) | Record js_analysis.var_ops |
/// | `AstNode::Link { ... }` | — | Already handled by link extraction |
///
/// Spans in the AST are **passage-body-relative**. They are stored as-is in the
/// variable tree without shifting by `body_offset`.
pub fn populate_registries_from_unified_ast(
    registry: &mut SugarCubeRegistry,
    passage_ast: &ast::PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
    body_offset_in_passage: usize,
) {
    // Record variable operations from the unified AST
    {
        let vtree = registry.variables_mut();

        // Triple: (op, kind_override, origin) — origin is the zone context
        // (Phase 9). Computed in `collect_var_ops_from_nodes` via `ZoneMap`
        // lookups; defaults to `RawScript` for top-level script passages.
        let mut all_var_ops: Vec<(AnalyzedVarOp, Option<VarAccessKind>, VarOrigin)> = Vec::new();

        // For script passages, collect from script_js_analysis first.
        // These are top-level JS var ops (not inside any macro) — their
        // origin is `RawScript` since they come from oxc analysis of the
        // passage's raw JS body.
        if let Some(ref analysis) = passage_ast.script_js_analysis {
            for op in &analysis.var_ops {
                all_var_ops.push((op.clone(), None, VarOrigin::RawScript));
            }
        }

        // Walk the AST nodes for inline var ops. Thread the zone map so
        // `collect_var_ops_from_nodes` can query the enclosing macro body
        // for Text-node var refs (Phase 9 origin classification).
        collect_var_ops_from_nodes(
            &passage_ast.nodes,
            &mut all_var_ops,
            cp,
            file_uri,
            &passage_ast.zones,
            body_offset_in_passage,
        );

        // Record each variable operation
        for (op, kind_override, origin) in &all_var_ops {
            let final_kind = kind_override.unwrap_or(op.access_kind);
            vtree.record_var_with_origin(
                &op.name,
                op.is_temporary,
                final_kind,
                &cp.header.name,
                file_uri,
                op.span.clone(),
                &op.property_path,
                &cp.body_text,
                &op.segment_spans,
                op.construct_span.clone(),
                &op.segment_construct_spans,
                origin.clone(),
            );
        }

        // Mark variables as seeded if this is a special passage
        if cp.special_def.as_ref().is_some_and(|d| {
            matches!(
                d.behavior,
                knot_core::passage::SpecialPassageBehavior::Startup
            )
        }) {
            for (op, _, _) in &all_var_ops {
                if op.access_kind.is_write() {
                    vtree.mark_seeded(&op.name);
                }
            }
        }
    }

    // Extract widget definitions and register macro_adds/template_adds/function_defs
    {
        let (macro_reg, func_reg, template_reg) = registry.definition_registries_mut();

        // For script passages, register definitions from script_js_analysis.
        // The offsets in script_js_analysis are body-relative (relative to the
        // passage body text, 0 = first byte after the header newline). We add
        // `body_offset_in_passage` to convert them to passage-relative (0 = `::`
        // head), matching the convention used by the `Passage` struct. This
        // keeps all internal offsets passage-relative so that cross-document
        // passage moves and incremental re-parsing don't require offset
        // recomputation — only `passage_offset` (document-absolute position of
        // the passage head) changes, and that's applied at the LSP boundary.
        if let Some(ref analysis) = passage_ast.script_js_analysis {
            for macro_add in &analysis.macro_adds {
                macro_reg.register_macro_add(
                    &macro_add.name,
                    &cp.header.name,
                    file_uri,
                    macro_add.name_offset + body_offset_in_passage,
                    None,
                    macro_add.body,
                );
            }
            for template_add in &analysis.template_adds {
                let kind = if template_add.is_string {
                    TemplateKind::String
                } else {
                    TemplateKind::Function
                };
                template_reg.register_template(
                    &template_add.name,
                    kind,
                    &cp.header.name,
                    file_uri,
                    template_add.name_offset + body_offset_in_passage,
                );
            }
            for func_def in &analysis.function_defs {
                func_reg.register_function(
                    &func_def.name,
                    FunctionKind::Declaration,
                    &cp.header.name,
                    file_uri,
                    func_def.name_offset + body_offset_in_passage,
                    func_def.param_count,
                );
            }
        }

        register_definitions_from_nodes(
            &passage_ast.nodes,
            &cp.header.name,
            file_uri,
            body_offset_in_passage,
            macro_reg,
            func_reg,
            template_reg,
        );
    }
}

/// Collect all variable operations from AST nodes, applying SugarCube
/// semantic overrides.
///
/// **Phase 9**: also computes a [`VarOrigin`] for each op by querying the
/// `ZoneMap` at the variable's span. The origin records the zone context
/// (prose / macro arg / macro body / expression / raw script) in which the
/// variable access occurs. The zone map's spans are passage-relative, so
/// `body_offset_in_passage` is added to the AST's body-relative offsets
/// before querying.
fn collect_var_ops_from_nodes(
    nodes: &[ast::AstNode],
    result: &mut Vec<(AnalyzedVarOp, Option<VarAccessKind>, VarOrigin)>,
    _cp: &ClassifiedPassage,
    _file_uri: &str,
    zones: &ZoneMap,
    body_offset_in_passage: usize,
) {
    for node in nodes {
        match node {
            ast::AstNode::Text { var_refs, .. } => {
                // Text node — var refs here are in prose/markup. The origin
                // depends on whether we're inside a macro body (use the zone
                // map to find out) or at the top level (Prose).
                for vr in var_refs {
                    let segment_spans =
                        compute_target_segment_spans(&vr.name, &vr.property_path, &vr.span);
                    let origin = compute_text_origin(zones, vr.span.start, body_offset_in_passage);

                    result.push((
                        AnalyzedVarOp {
                            name: vr.name.clone(),
                            is_temporary: vr.is_temporary,
                            access_kind: VarAccessKind::Read,
                            span: vr.span.clone(),
                            property_path: vr.property_path.clone(),
                            segment_spans,
                            construct_span: None,
                            segment_construct_spans: Vec::new(),
                        },
                        None,
                        origin,
                    ));
                }
            }
            ast::AstNode::Macro {
                name,
                js_analysis,
                var_refs,
                set_assignment,
                capture_target,
                for_loop_vars,
                children,
                full_span,
                ..
            } => {
                // Determine if this is a raw-body macro (e.g. <<script>>).
                // Raw-body macros have their body processed by an external
                // parser (oxc for JS); vars from their js_analysis get the
                // RawScript origin. Non-raw macros (e.g. <<set>>) also have
                // js_analysis for their args, but those args are SugarCube
                // expressions → MacroArg.
                let is_raw_body = is_raw_body_macro(name);
                let macro_arg_origin = if is_raw_body {
                    VarOrigin::RawScript
                } else {
                    VarOrigin::MacroArg {
                        macro_name: name.clone(),
                    }
                };

                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());

                if has_js_analysis {
                    // Use oxc-derived var_ops (more accurate read/write classification)
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            let kind_override = determine_macro_override(
                                name,
                                op,
                                set_assignment.as_ref(),
                                capture_target.as_ref(),
                            );
                            result.push((op.clone(), kind_override, macro_arg_origin.clone()));
                        }
                    }
                } else {
                    // Fall back to var_refs from SugarCube parser's scan_inline_vars.
                    // Skip the set_assignment TARGET if present — Path B handles it
                    // with better construct_span info. Other var_refs (e.g., `$bar`
                    // in `<<set $foo to $bar + 1>>`) are still emitted here.
                    //
                    // Note: scan_inline_vars may classify the target as a read
                    // (is_write=false) even though it's the assignment target,
                    // because the scanner doesn't understand assignment context.
                    // We skip it regardless of is_write — Path B always emits
                    // the target with the correct write kind.
                    let is_assignment = is_assignment_macro(name);
                    let sa_target_name = set_assignment.as_ref().map(|sa| sa.target.name.as_str());
                    for vr in var_refs {
                        // Skip the set_assignment target — Path B emits it.
                        if let Some(target) = sa_target_name
                            && vr.name == target
                        {
                            continue;
                        }
                        let segment_spans =
                            compute_target_segment_spans(&vr.name, &vr.property_path, &vr.span);
                        let kind = if vr.is_write || is_assignment {
                            VarAccessKind::Write
                        } else {
                            VarAccessKind::Read
                        };
                        result.push((
                            AnalyzedVarOp {
                                name: vr.name.clone(),
                                is_temporary: vr.is_temporary,
                                access_kind: kind,
                                span: vr.span.clone(),
                                property_path: vr.property_path.clone(),
                                segment_spans,
                                construct_span: None,
                                segment_construct_spans: Vec::new(),
                            },
                            None,
                            macro_arg_origin.clone(),
                        ));
                    }
                }

                // For <<set>> macros with set_assignment: emit the target variable
                // ONLY IF js_analysis didn't already cover it. js_analysis covers
                // the target in two cases:
                //   1. Block literal RHS (object/array) — js_walk decomposes it
                //      into leaf writes. The target itself gets no direct write
                //      (per the propagation model — it gets writes from leaf
                //      propagation instead).
                //   2. Scalar RHS — `check_assignment_for_state_var` emits the
                //      target as a direct write.
                // In both cases, Path B is redundant and would cause a
                // duplicate write on the target node.
                if let Some(sa) = set_assignment {
                    // Check if js_analysis already produced writes for this
                    // variable. For scalar RHS, js_analysis emits a direct
                    // write on the target (property_path = ""). For block
                    // literal RHS, js_analysis emits leaf writes
                    // (property_path = "child.grandchild"). In both cases,
                    // Path B is redundant — either the direct write already
                    // exists, or leaf writes will propagate up to the root.
                    let js_analysis_covers_target = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis
                            .var_ops
                            .iter()
                            .any(|op| op.name == sa.target.name && op.access_kind.is_write())
                    });

                    if !js_analysis_covers_target {
                        let kind = set_operator_to_access_kind(&sa.operator);

                        let segment_spans = compute_target_segment_spans(
                            &sa.target.name,
                            &sa.target.property_path,
                            &sa.target.span,
                        );

                        // For non-block assignments, the construct span is the
                        // full `<<set>>` macro span. This is used by propagation
                        // as the construct span at every depth (since there are
                        // no intermediate block literals to aggregate).
                        result.push((
                            AnalyzedVarOp {
                                name: sa.target.name.clone(),
                                is_temporary: sa.target.is_temporary,
                                access_kind: kind,
                                span: sa.target.span.clone(),
                                property_path: sa.target.property_path.clone(),
                                segment_spans: segment_spans.clone(),
                                construct_span: Some(full_span.clone()),
                                segment_construct_spans: {
                                    // For non-block assignments, all segments
                                    // share the full assignment span. The leaf
                                    // itself + every ancestor gets the same span.
                                    let mut scs = Vec::new();
                                    for _ in 0..segment_spans.len() {
                                        scs.push(full_span.clone());
                                    }
                                    scs
                                },
                            },
                            None,
                            macro_arg_origin.clone(),
                        ));
                    }
                }

                // For <<capture>> macros with capture_target: emit the captured variable
                // as VarAccessKind::Capture. This provides AST-level capture tracking that
                // complements the JS annotation pass.
                if let Some(ct) = capture_target {
                    // Only emit if not already covered by js_analysis
                    let already_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis
                            .var_ops
                            .iter()
                            .any(|op| op.name == ct.name && op.access_kind.is_write())
                    });

                    if !already_covered {
                        let segment_spans =
                            compute_target_segment_spans(&ct.name, &ct.property_path, &ct.span);

                        result.push((
                            AnalyzedVarOp {
                                name: ct.name.clone(),
                                is_temporary: ct.is_temporary,
                                access_kind: VarAccessKind::Capture,
                                span: ct.span.clone(),
                                property_path: ct.property_path.clone(),
                                segment_spans,
                                construct_span: Some(full_span.clone()),
                                segment_construct_spans: Vec::new(),
                            },
                            None,
                            macro_arg_origin.clone(),
                        ));
                    }
                }

                // For <<for>> macros with for_loop_vars: emit the loop variables.
                // The index variable (_i) is a write (receives each element).
                // The iterated variable ($array) is a read.
                if let Some(fl) = for_loop_vars {
                    // Emit index var as Write (it receives each element during iteration)
                    let index_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis
                            .var_ops
                            .iter()
                            .any(|op| op.name == fl.index_var.name && op.access_kind.is_write())
                    });

                    if !index_covered {
                        let segment_spans = compute_target_segment_spans(
                            &fl.index_var.name,
                            &fl.index_var.property_path,
                            &fl.index_var.span,
                        );

                        result.push((
                            AnalyzedVarOp {
                                name: fl.index_var.name.clone(),
                                is_temporary: true,
                                access_kind: VarAccessKind::Write,
                                span: fl.index_var.span.clone(),
                                property_path: fl.index_var.property_path.clone(),
                                segment_spans,
                                construct_span: None,
                                segment_construct_spans: Vec::new(),
                            },
                            None,
                            macro_arg_origin.clone(),
                        ));
                    }

                    // Emit iterated var as Read
                    let iter_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis
                            .var_ops
                            .iter()
                            .any(|op| op.name == fl.iterated_var.name)
                    });

                    if !iter_covered {
                        let segment_spans = compute_target_segment_spans(
                            &fl.iterated_var.name,
                            &fl.iterated_var.property_path,
                            &fl.iterated_var.span,
                        );

                        result.push((
                            AnalyzedVarOp {
                                name: fl.iterated_var.name.clone(),
                                is_temporary: fl.iterated_var.is_temporary,
                                access_kind: VarAccessKind::Read,
                                span: fl.iterated_var.span.clone(),
                                property_path: fl.iterated_var.property_path.clone(),
                                segment_spans,
                                construct_span: None,
                                segment_construct_spans: Vec::new(),
                            },
                            None,
                            macro_arg_origin.clone(),
                        ));
                    }
                }

                // Recurse into children (the macro's body). The zone map is
                // threaded through so that Text nodes inside the body can
                // query their enclosing macro context.
                if let Some(ch) = children {
                    collect_var_ops_from_nodes(
                        ch,
                        result,
                        _cp,
                        _file_uri,
                        zones,
                        body_offset_in_passage,
                    );
                }
            }
            ast::AstNode::Expression {
                js_analysis,
                var_refs,
                ..
            } => {
                // Expression macros (<<=>>expr>>, <<->>expr>>) — their args
                // are JS expressions. All var refs here get the Expression
                // origin (Phase 9).
                let expression_origin = VarOrigin::Expression;
                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());
                if has_js_analysis {
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            result.push((op.clone(), None, expression_origin.clone()));
                        }
                    }
                } else {
                    // Fall back to var_refs
                    for vr in var_refs {
                        let segment_spans =
                            compute_target_segment_spans(&vr.name, &vr.property_path, &vr.span);
                        result.push((
                            AnalyzedVarOp {
                                name: vr.name.clone(),
                                is_temporary: vr.is_temporary,
                                access_kind: VarAccessKind::Read,
                                span: vr.span.clone(),
                                property_path: vr.property_path.clone(),
                                segment_spans,
                                construct_span: None,
                                segment_construct_spans: Vec::new(),
                            },
                            None,
                            expression_origin.clone(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Compute the [`VarOrigin`] for a variable reference in a Text node (Phase 9).
///
/// Text-node var refs are in prose/markup. Their origin depends on whether
/// they're inside a macro body (→ `MacroBody { enclosing_macro, depth }`)
/// or at the top level (→ `Prose`).
///
/// The zone map is queried at the var's span (shifted from body-relative to
/// passage-relative via `body_offset_in_passage`). If the zone map is empty
/// (e.g., script passages don't have zones populated), falls back to `Prose`.
fn compute_text_origin(
    zones: &ZoneMap,
    body_offset: usize,
    body_offset_in_passage: usize,
) -> VarOrigin {
    let passage_offset = body_offset.saturating_add(body_offset_in_passage);
    if let Some(leaf) = zones.leaf_at(passage_offset) {
        // The leaf's body_idx tells us if we're inside a macro body.
        // body_stack_at walks the parent chain to get the full ancestor
        // stack; the last element is the innermost (immediate) enclosing body.
        let stack = zones.body_stack_at(passage_offset);
        if let Some(innermost) = stack.last() {
            return VarOrigin::MacroBody {
                enclosing_macro: innermost.macro_name.clone(),
                depth: innermost.depth,
            };
        }
        // Leaf exists but no enclosing body → top-level prose/markup.
        // (leaf.body_idx is None, meaning we're at the passage top level.)
        let _ = leaf; // suppress unused warning
    }
    // Zone map empty or no enclosing body → top-level prose.
    VarOrigin::Prose
}

/// Check if a macro name corresponds to a raw-body macro (Phase 9).
///
/// Raw-body macros (currently only `<<script>>`) have their body processed by
/// an external parser (oxc for JS) rather than the SugarCube parser. Var ops
/// from their `js_analysis` get the `RawScript` origin.
fn is_raw_body_macro(name: &str) -> bool {
    find_macro(name).is_some_and(|d| d.body_is_raw)
}

/// Determine SugarCube semantic overrides for a variable operation within
/// a macro context.
fn determine_macro_override(
    macro_name: &str,
    op: &AnalyzedVarOp,
    set_assignment: Option<&ast::SetAssignment>,
    capture_target: Option<&ast::VarRef>,
) -> Option<VarAccessKind> {
    if macro_name.eq_ignore_ascii_case("capture") {
        // If capture_target is available, use it for precise matching.
        // Otherwise fall back to the heuristic of upgrading any write to Capture.
        let is_capture_target =
            capture_target.is_some_and(|ct| ct.name == op.name && ct.span.start == op.span.start);

        if is_capture_target || (capture_target.is_none() && op.access_kind.is_write()) {
            return Some(VarAccessKind::Capture);
        }
    }

    if macro_name.eq_ignore_ascii_case("unset") && op.access_kind.is_write() {
        return Some(VarAccessKind::Unset);
    }

    if macro_name.eq_ignore_ascii_case("set")
        && let Some(sa) = set_assignment
        && op.name == sa.target.name
        && op.span.start == sa.target.span.start
    {
        let kind = set_operator_to_access_kind(&sa.operator);
        if op.access_kind != kind {
            return Some(kind);
        }
    }

    None
}

/// Register widget definitions, Macro.add(), Template.add(), and function
/// definitions from the js_analysis on AST nodes.
fn register_definitions_from_nodes(
    nodes: &[ast::AstNode],
    passage_name: &str,
    file_uri: &str,
    body_offset_in_passage: usize,
    macro_reg: &mut crate::sugarcube::registries::custom_macros::CustomMacroRegistry,
    func_reg: &mut crate::sugarcube::registries::function_registry::FunctionRegistry,
    template_reg: &mut crate::sugarcube::registries::template_registry::TemplateRegistry,
) {
    for node in nodes {
        match node {
            ast::AstNode::Macro {
                name,
                args,
                open_span,
                definition_name_span,
                children,
                js_analysis,
                ..
            } => {
                // <<widget name>> definitions
                // Use definition_name_span for precise name extraction when available,
                // falling back to args.trim() for backward compatibility.
                if name.eq_ignore_ascii_case("widget") {
                    let widget_name = if definition_name_span.is_some() {
                        // Extract the name from args using the span offset.
                        // definition_name_span is in passage-body coords;
                        // open_span.start is the position of << in passage-body coords.
                        // The args start after << + name + space, so args_start_in_body ≈
                        // name_span.end + 1. We can derive the name offset within args:
                        //   dns.start - args_offset, where args_offset = name_span.end + 1 (approx)
                        // But since we don't have name_span here, we use a simpler approach:
                        // the first whitespace-delimited token in args is the widget name.
                        // This matches the span-based extraction for all well-formed inputs.
                        args.split_whitespace().next().unwrap_or("").to_string()
                    } else {
                        args.trim().to_string()
                    };
                    if !widget_name.is_empty() {
                        // Detect the `container` keyword in widget args.
                        // SugarCube syntax: <<widget "name" container>>
                        // The word "container" must appear after the name token,
                        // outside of any quoted string.
                        let is_container = detect_widget_container_keyword(args);
                        // Map the boolean to BodyRequirement: container widgets
                        // require a close tag (Required), non-container widgets
                        // are inline (Never).
                        let body = if is_container {
                            crate::types::BodyRequirement::Required
                        } else {
                            crate::types::BodyRequirement::Never
                        };
                        // Extract arg_count from _args[N] / $args[N] references in the widget body
                        let arg_count = children
                            .as_ref()
                            .and_then(|ch| extract_widget_arg_count(ch));
                        // definition_name_span and open_span are body-relative
                        // (0 = body start). Add body_offset_in_passage to convert
                        // to passage-relative (0 = `::` head).
                        let name_offset = definition_name_span
                            .as_ref()
                            .map_or(open_span.start, |dns| dns.start)
                            + body_offset_in_passage;
                        macro_reg.register_widget(
                            &widget_name,
                            passage_name,
                            file_uri,
                            name_offset,
                            arg_count,
                            body,
                        );
                    }
                }

                // Register Macro.add(), Template.add(), function definitions from js_analysis.
                // The offsets in js_analysis are body-relative; add
                // body_offset_in_passage to convert to passage-relative.
                if let Some(analysis) = js_analysis {
                    for macro_add in &analysis.macro_adds {
                        macro_reg.register_macro_add(
                            &macro_add.name,
                            passage_name,
                            file_uri,
                            macro_add.name_offset + body_offset_in_passage,
                            None,
                            macro_add.body,
                        );
                    }
                    for template_add in &analysis.template_adds {
                        let kind = if template_add.is_string {
                            TemplateKind::String
                        } else {
                            TemplateKind::Function
                        };
                        template_reg.register_template(
                            &template_add.name,
                            kind,
                            passage_name,
                            file_uri,
                            template_add.name_offset + body_offset_in_passage,
                        );
                    }
                    for func_def in &analysis.function_defs {
                        func_reg.register_function(
                            &func_def.name,
                            FunctionKind::Declaration,
                            passage_name,
                            file_uri,
                            func_def.name_offset + body_offset_in_passage,
                            func_def.param_count,
                        );
                    }
                }

                // Recurse into children
                if let Some(ch) = children {
                    register_definitions_from_nodes(
                        ch,
                        passage_name,
                        file_uri,
                        body_offset_in_passage,
                        macro_reg,
                        func_reg,
                        template_reg,
                    );
                }
            }
            ast::AstNode::Expression {
                js_analysis: Some(analysis),
                ..
            } => {
                // Same body-relative → passage-relative conversion as the Macro
                // branch above. Expression macros (<<=>>, <<->>) can contain
                // Macro.add() / function definitions in rare cases (e.g.,
                // <<= Macro.add("x", {}) >>).
                for macro_add in &analysis.macro_adds {
                    macro_reg.register_macro_add(
                        &macro_add.name,
                        passage_name,
                        file_uri,
                        macro_add.name_offset + body_offset_in_passage,
                        None,
                        macro_add.body,
                    );
                }
                for template_add in &analysis.template_adds {
                    let kind = if template_add.is_string {
                        TemplateKind::String
                    } else {
                        TemplateKind::Function
                    };
                    template_reg.register_template(
                        &template_add.name,
                        kind,
                        passage_name,
                        file_uri,
                        template_add.name_offset + body_offset_in_passage,
                    );
                }
                for func_def in &analysis.function_defs {
                    func_reg.register_function(
                        &func_def.name,
                        FunctionKind::Declaration,
                        passage_name,
                        file_uri,
                        func_def.name_offset + body_offset_in_passage,
                        func_def.param_count,
                    );
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Backward-compatible wrapper: populate from AST (old API, uses unified path)
// ---------------------------------------------------------------------------

/// Populate registries from a parsed passage AST.
///
/// This is the backward-compatible entry point. After the unified AST
/// refactoring, this delegates to `populate_registries_from_unified_ast`.
pub fn populate_registries_from_ast(
    registry: &mut SugarCubeRegistry,
    passage_ast: &ast::PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
    body_offset_in_passage: usize,
) {
    populate_registries_from_unified_ast(
        registry,
        passage_ast,
        cp,
        file_uri,
        body_offset_in_passage,
    );
}

// ---------------------------------------------------------------------------
// Walk JS for script passages (kept temporarily for backward compat)
// ---------------------------------------------------------------------------

/// Walk JS in a script passage using oxc for deep registry population.
///
/// **Note**: This is kept temporarily for backward compat during migration.
/// The preferred path is through `populate_registries_from_unified_ast()`
/// which reads from `PassageAst::script_js_analysis`.
pub fn walk_script_js(
    registry: &mut SugarCubeRegistry,
    body_text: &str,
    cp: &ClassifiedPassage,
    file_uri: &str,
) {
    use crate::sugarcube::js::js_preprocess;
    use crate::sugarcube::js::js_walk;
    use knot_core::oxc::{ParseMode as JsParseMode, parse_and_visit};

    let preprocessed = js_preprocess::preprocess_for_oxc(body_text, true);

    // Walk the AST whenever the parse did not panic. NOTE that oxc's error
    // recovery is narrow (verified empirically against oxc 0.134): common
    // syntax errors — missing operand, unclosed `{`, unterminated string —
    // panic the parser and yield an EMPTY AST, in which case the visitor
    // does not run and no var ops are recorded from this body. When oxc
    // DOES recover, the valid parts still contribute to the registries.
    let (_outcome, _) = parse_and_visit(&preprocessed.source, JsParseMode::Module, |program| {
        let analysis = js_walk::walk_script_passage(program, &preprocessed);

        // Record variable operations. `walk_script_js` handles script
        // passages — all var ops here originate from raw JS, so their
        // `VarOrigin` is `RawScript` (Phase 9).
        let vtree = registry.variables_mut();
        for op in &analysis.var_ops {
            vtree.record_var_with_origin(
                &op.name,
                op.is_temporary,
                op.access_kind,
                &cp.header.name,
                file_uri,
                op.span.clone(),
                &op.property_path,
                body_text,
                &op.segment_spans,
                op.construct_span.clone(),
                &op.segment_construct_spans,
                VarOrigin::RawScript,
            );
        }

        // Record definitions
        let (macro_reg, func_reg, template_reg) = registry.definition_registries_mut();
        for macro_add in &analysis.macro_adds {
            macro_reg.register_macro_add(
                &macro_add.name,
                &cp.header.name,
                file_uri,
                macro_add.name_offset,
                None,
                macro_add.body,
            );
        }
        for template_add in &analysis.template_adds {
            let kind = if template_add.is_string {
                TemplateKind::String
            } else {
                TemplateKind::Function
            };
            template_reg.register_template(
                &template_add.name,
                kind,
                &cp.header.name,
                file_uri,
                template_add.name_offset,
            );
        }
        for func_def in &analysis.function_defs {
            func_reg.register_function(
                &func_def.name,
                FunctionKind::Declaration,
                &cp.header.name,
                file_uri,
                func_def.name_offset,
                func_def.param_count,
            );
        }
    });
}

/// Detect whether the `container` keyword appears in widget args.
///
/// SugarCube widget syntax: `<<widget "name" container>>` or `<<widget 'name' container>>`.
/// The keyword must appear as a bare token (not inside quotes) after the name.
/// This function skips quoted strings and checks for the bare word "container".
fn detect_widget_container_keyword(args: &str) -> bool {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut i = 0;
    let bytes = args.as_bytes();
    let len = bytes.len();

    while i < len {
        let ch = bytes[i];
        if in_double_quote {
            if ch == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_double_quote = false;
            }
        } else if in_single_quote {
            if ch == b'\'' && (i == 0 || bytes[i - 1] != b'\\') {
                in_single_quote = false;
            }
        } else {
            if ch == b'"' {
                in_double_quote = true;
            } else if ch == b'\'' {
                in_single_quote = true;
            } else if ch == b'c' {
                // Check for "container" keyword outside quotes
                if args[i..].starts_with("container") {
                    let end = i + 9; // "container".len()
                    // Must be a word boundary: preceded by whitespace and followed
                    // by end-of-string or whitespace
                    let prev_ok = i == 0 || args.as_bytes()[i - 1].is_ascii_whitespace();
                    let next_ok = end >= len || args.as_bytes()[end].is_ascii_whitespace();
                    if prev_ok && next_ok {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

/// Extract the number of arguments a widget accepts by scanning its children
/// for `_args[N]` or `$args[N]` references.
///
/// Returns `Some(max_index + 1)` if any `_args[N]` / `$args[N]` patterns are
/// found, where `max_index` is the highest array index referenced. Returns
/// `None` if no such patterns exist (the widget doesn't reference its args).
///
/// This walks all text and expression nodes recursively within the widget body.
fn extract_widget_arg_count(children: &[ast::AstNode]) -> Option<usize> {
    let mut max_index: Option<usize> = None;

    fn scan_node(node: &ast::AstNode, max_index: &mut Option<usize>) {
        match node {
            ast::AstNode::Text { content, .. } => {
                scan_for_args_index(content, max_index);
            }
            ast::AstNode::Expression { content, .. } => {
                scan_for_args_index(content, max_index);
            }
            ast::AstNode::Macro { args, children, .. } => {
                scan_for_args_index(args, max_index);
                if let Some(ch) = children {
                    for child in ch {
                        scan_node(child, max_index);
                    }
                }
            }
            // Links can contain text that references _args
            ast::AstNode::Link { .. } => {}
            // These node types don't contain _args references
            ast::AstNode::Comment { .. }
            | ast::AstNode::MacroClose { .. }
            | ast::AstNode::Error { .. } => {}
            // Containers with parsed children: recurse. TextFormat (and
            // InlineStyle) children were skipped before plan.md Phase 1.3,
            // which hid `_args[N]` references inside formatted text
            // (`''_args[0]''`) in widget bodies.
            ast::AstNode::InlineStyle { children, .. }
            | ast::AstNode::TextFormat { children, .. } => {
                for child in children {
                    scan_node(child, max_index);
                }
            }
            // ── Block-level markup (Phase 1 scaffolding) ──
            // Not yet emitted by the parser. When Phase 3+ adds Heading,
            // ListItem, Blockquote, BlockquoteBlock, TableCell content,
            // those arms will need to recurse into their `children` to
            // scan for `_args[N]` references inside widget bodies. For
            // now these arms never fire.
            //
            // NOTE: CodeBlock, InlineCode, and Verbatim content is raw (no
            // macro processing), so even when emitted they should NOT be
            // recursed into here — `_args` inside `{{{...}}}` or `"""..."""`
            // is literal.
            ast::AstNode::Heading { .. }
            | ast::AstNode::HorizontalRule { .. }
            | ast::AstNode::ListItem { .. }
            | ast::AstNode::Blockquote { .. }
            | ast::AstNode::BlockquoteBlock { .. }
            | ast::AstNode::Table { .. }
            | ast::AstNode::CodeBlock { .. }
            | ast::AstNode::InlineCode { .. }
            | ast::AstNode::Verbatim { .. } => {}
        }
    }

    for child in children {
        scan_node(child, &mut max_index);
    }

    max_index.map(|mi| mi + 1)
}

/// Scan a string for `_args[N]` or `$args[N]` patterns and update `max_index`
/// if a higher index is found.
fn scan_for_args_index(text: &str, max_index: &mut Option<usize>) {
    // Match patterns like _args[0], _args[1], $args[5], etc.
    // Hand-written scanner to avoid regex overhead on hot paths.
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for _args[ or $args[
        if (bytes[i] == b'_' || bytes[i] == b'$')
            && i + 5 < len
            && &text[i + 1..i + 5] == "args"
            && text.as_bytes()[i + 5] == b'['
        {
            // Found _args[ or $args[ — extract the index
            let bracket_start = i + 6;
            let mut bracket_end = bracket_start;
            while bracket_end < len && bytes[bracket_end].is_ascii_digit() {
                bracket_end += 1;
            }
            if bracket_end > bracket_start
                && bracket_end < len
                && bytes[bracket_end] == b']'
                && let Ok(idx) = text[bracket_start..bracket_end].parse::<usize>()
            {
                *max_index = Some(max_index.map_or(idx, |mi| mi.max(idx)));
            }
            i = bracket_end + 1;
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::js::js_annotate;
    use crate::sugarcube::parser;

    #[test]
    fn unified_ast_detects_state_variables_read() {
        let body = "<<run _items = State.variables.ITEMS>>";
        let mut ast = parser::parse_passage_body(body, 0, ParseMode::Normal);

        // Phase 2: JS annotation (sugarcube_syntax = true for Twee passages)
        js_annotate::annotate_js(
            &mut ast,
            body,
            false,
            true,
            &std::collections::HashSet::new(),
        );

        let mut registry = SugarCubeRegistry::new();

        let header = crate::header::TweeHeader {
            name: "Game".to_string(),
            tags: Vec::new(),
            header_start: 0,
            name_start: 0,
            metadata_json: None,
            name_text_raw: "Game".to_string(),
            tags_raw: String::new(),
        };
        let cp = ClassifiedPassage {
            header,
            body_text: body.to_string(),
            file_uri: "file:///test.tw".to_string(),
            category: crate::sugarcube::classifier::PassageCategory::Regular,
            special_def: None,
            processing_priority: 40,
        };
        populate_registries_from_unified_ast(&mut registry, &ast, &cp, "file:///test.tw", 0);

        // Verify $ITEMS exists with a READ access
        let vtree = registry.variables();
        let items_var = vtree.get_variable("$ITEMS");
        assert!(
            items_var.is_some(),
            "$ITEMS should be in registry from State.variables.ITEMS detection"
        );
        if let Some((_, node)) = items_var {
            let reads: Vec<_> = node.meta.refs.iter().filter(|a| a.is_read()).collect();
            assert!(!reads.is_empty(), "$ITEMS should have at least one READ");
        }
    }

    #[test]
    fn test_detect_widget_container_keyword() {
        // Double-quoted name with container
        assert!(detect_widget_container_keyword(r#""myWidget" container"#));
        // Single-quoted name with container
        assert!(detect_widget_container_keyword(r#"'myWidget' container"#));
        // Container at the end
        assert!(detect_widget_container_keyword(r#""myWidget" container"#));
        // No container keyword
        assert!(!detect_widget_container_keyword(r#""myWidget""#));
        assert!(!detect_widget_container_keyword(r#""myWidget" "#));
        // "container" inside quotes should NOT be detected
        assert!(!detect_widget_container_keyword(r#""container""#));
        assert!(!detect_widget_container_keyword(r#""myContainer""#));
        // "container" as part of another word should NOT be detected
        assert!(!detect_widget_container_keyword(r#""myWidget" containers"#));
    }

    #[test]
    fn test_scan_for_args_index() {
        let mut max_index = None;
        scan_for_args_index("_args[0]", &mut max_index);
        assert_eq!(max_index, Some(0));

        let mut max_index = None;
        scan_for_args_index("_args[2]", &mut max_index);
        assert_eq!(max_index, Some(2));

        let mut max_index = None;
        scan_for_args_index("$args[5]", &mut max_index);
        assert_eq!(max_index, Some(5));

        // Multiple references — should pick the highest
        let mut max_index = None;
        scan_for_args_index("<<print _args[0]>> <<print _args[3]>>", &mut max_index);
        assert_eq!(max_index, Some(3));

        // No _args references
        let mut max_index = None;
        scan_for_args_index("Hello world", &mut max_index);
        assert_eq!(max_index, None);

        // $args mixed with _args
        let mut max_index = None;
        scan_for_args_index("$args[1] and _args[4]", &mut max_index);
        assert_eq!(max_index, Some(4));
    }

    #[test]
    fn test_extract_widget_arg_count_from_ast() {
        // Parse a widget that uses _args
        let body = r#"<<widget "greet">><<print _args[0]>> says <<print _args[1]>><</widget>>"#;
        let ast = parser::parse_passage_body(body, 0, ParseMode::Widget);

        // Find the widget macro node
        if let Some(ast::AstNode::Macro { children, .. }) = ast.nodes.first() {
            let arg_count = children
                .as_ref()
                .and_then(|ch| extract_widget_arg_count(ch));
            // _args[0] and _args[1] means 2 args
            assert_eq!(
                arg_count,
                Some(2),
                "Expected arg_count=2 from _args[0] and _args[1]"
            );
        } else {
            panic!("Expected a Macro node as the first AST node");
        }
    }

    // ===================================================================
    // Phase 9 — VarOrigin tests
    // ===================================================================
    //
    // These tests verify that `VarAccess.origin` is correctly populated
    // based on the zone context (prose / macro arg / macro body /
    // expression / raw script). They parse a body, build zones (simulating
    // what `parse_pipeline::parse_full` does), run `js_annotate::annotate_js`
    // for JS analysis, then call `populate_registries_from_unified_ast` and
    // inspect the resulting `VarAccess` records.

    use crate::header::TweeHeader;
    use crate::sugarcube::classifier::PassageCategory;
    use crate::sugarcube::registries::CustomMacroRegistry;
    use crate::zoning::build_from_ast as build_zones;

    /// Build a minimal `ClassifiedPassage` for testing.
    fn make_cp(body: &str) -> ClassifiedPassage {
        ClassifiedPassage {
            header: TweeHeader {
                name: "Test".to_string(),
                tags: Vec::new(),
                header_start: 0,
                name_start: 3,
                metadata_json: None,
                name_text_raw: "Test".to_string(),
                tags_raw: String::new(),
            },
            body_text: body.to_string(),
            file_uri: "file:///test.tw".to_string(),
            category: PassageCategory::Regular,
            special_def: None,
            processing_priority: 40,
        }
    }

    /// Parse a body, annotate JS, build zones, and populate registries.
    /// Returns the `SugarCubeRegistry` for inspection.
    fn parse_and_populate(body: &str) -> SugarCubeRegistry {
        let mut ast = parser::parse_passage_body(body, 0, ParseMode::Normal);

        // Phase 2: JS annotation (sugarcube_syntax = true for Twee passages)
        js_annotate::annotate_js(
            &mut ast,
            body,
            false,
            true,
            &std::collections::HashSet::new(),
        );

        // Phase 1b: Build zones (simulating what parse_pipeline does).
        ast.zones = build_zones(&ast.nodes, 0, &CustomMacroRegistry::new());

        let cp = make_cp(body);
        let mut registry = SugarCubeRegistry::new();
        populate_registries_from_unified_ast(&mut registry, &ast, &cp, "file:///test.tw", 0);
        registry
    }

    /// Collect all `VarAccess` records for a given variable name from the
    /// registry's variable tree. Uses `get_variable` to find the root node,
    /// then collects all accesses from its `meta.refs` (includes both direct
    /// and propagated accesses).
    fn collect_accesses(
        registry: &SugarCubeRegistry,
        var_name: &str,
    ) -> Vec<super::super::variable_tree::VarAccess> {
        let vtree = registry.variables();
        let mut accesses = Vec::new();
        if let Some((_, node)) = vtree.get_variable(var_name) {
            accesses.extend(node.meta.refs.iter().cloned());
        }
        accesses
    }

    /// `$gold` in top-level prose → `VarOrigin::Prose`.
    #[test]
    fn phase9_origin_prose() {
        let body = "You have $gold gold pieces.";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record"
        );
        for a in &accesses {
            assert!(
                matches!(a.origin, super::super::variable_tree::VarOrigin::Prose),
                "$gold in prose should have origin Prose, got {:?}",
                a.origin
            );
        }
    }

    /// `$gold` in `<<print $gold>>` args → `VarOrigin::MacroArg { "print" }`.
    #[test]
    fn phase9_origin_macro_arg() {
        let body = "<<print $gold>>";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record"
        );
        for a in &accesses {
            match &a.origin {
                super::super::variable_tree::VarOrigin::MacroArg { macro_name } => {
                    assert_eq!(
                        macro_name, "print",
                        "$gold in <<print $gold>> should have MacroArg {{ \"print\" }}, got MacroArg with {:?}",
                        macro_name
                    );
                }
                other => panic!(
                    "$gold in <<print $gold>> should have MacroArg origin, got {:?}",
                    other
                ),
            }
        }
    }

    /// `$gold` in `<<link>>` body → `VarOrigin::MacroBody { "link", depth: 1 }`.
    #[test]
    fn phase9_origin_macro_body() {
        let body = "<<link \"Go\">>You find $gold.<</link>>";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record (from inside <<link>> body)"
        );
        for a in &accesses {
            match &a.origin {
                super::super::variable_tree::VarOrigin::MacroBody {
                    enclosing_macro,
                    depth,
                } => {
                    assert_eq!(
                        enclosing_macro, "link",
                        "$gold in <<link>> body should have enclosing_macro \"link\", got {:?}",
                        enclosing_macro
                    );
                    assert_eq!(
                        *depth, 0,
                        "$gold in <<link>> body should have depth 0 (outermost body = depth 0), got {}",
                        depth
                    );
                }
                other => panic!(
                    "$gold in <<link>> body should have MacroBody origin, got {:?}",
                    other
                ),
            }
        }
    }

    /// `$gold` in nested macro body (`<<link>><<if>>$gold<</if>><</link>>`)
    /// → `VarOrigin::MacroBody { "if", depth: 2 }`.
    #[test]
    fn phase9_origin_nested_macro_body() {
        let body = "<<link \"Go\">><<if $x>>You find $gold.<</if>><</link>>";
        let registry = parse_and_populate(body);
        let gold_accesses = collect_accesses(&registry, "$gold");
        assert!(
            !gold_accesses.is_empty(),
            "$gold should have at least one access record"
        );
        for a in &gold_accesses {
            match &a.origin {
                super::super::variable_tree::VarOrigin::MacroBody {
                    enclosing_macro,
                    depth,
                } => {
                    assert_eq!(
                        enclosing_macro, "if",
                        "$gold inside <<link>><<if>> should have enclosing_macro \"if\" (innermost), got {:?}",
                        enclosing_macro
                    );
                    assert_eq!(
                        *depth, 1,
                        "$gold inside <<link>><<if>> should have depth 1 (link=0, if=1), got {}",
                        depth
                    );
                }
                other => panic!(
                    "$gold inside <<link>><<if>> should have MacroBody origin, got {:?}",
                    other
                ),
            }
        }
    }

    /// `$gold` in `<<set $gold to 1>>` → `VarOrigin::MacroArg { "set" }`.
    #[test]
    fn phase9_origin_set_macro_arg() {
        let body = "<<set $gold to 1>>";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record"
        );
        for a in &accesses {
            match &a.origin {
                super::super::variable_tree::VarOrigin::MacroArg { macro_name } => {
                    assert_eq!(
                        macro_name, "set",
                        "$gold in <<set $gold to 1>> should have MacroArg {{ \"set\" }}, got {:?}",
                        macro_name
                    );
                }
                other => panic!(
                    "$gold in <<set $gold to 1>> should have MacroArg origin, got {:?}",
                    other
                ),
            }
        }
    }

    /// `$gold` in `<<script>>` body → `VarOrigin::RawScript`.
    #[test]
    fn phase9_origin_raw_script() {
        let body = "<<script>>\nState.variables.gold = 100;\n<</script>>";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record from <<script>> body"
        );
        for a in &accesses {
            assert!(
                matches!(a.origin, super::super::variable_tree::VarOrigin::RawScript),
                "$gold in <<script>> body should have origin RawScript, got {:?}",
                a.origin
            );
        }
    }

    /// `$gold` in `<<= $gold>>` expression → `VarOrigin::Expression`.
    #[test]
    fn phase9_origin_expression() {
        // Note: `<<= $gold>>` (with space after `=`) is the correct SugarCube
        // expression-macro syntax. `<<=>>$gold>>` (no space) would parse as
        // an empty `<<=>>` expression followed by text `$gold>>`.
        let body = "<<= $gold>>";
        let registry = parse_and_populate(body);
        let accesses = collect_accesses(&registry, "$gold");
        assert!(
            !accesses.is_empty(),
            "$gold should have at least one access record from <<=>> expression"
        );
        for a in &accesses {
            assert!(
                matches!(a.origin, super::super::variable_tree::VarOrigin::Expression),
                "$gold in <<= $gold>> should have origin Expression, got {:?}",
                a.origin
            );
        }
    }
}
