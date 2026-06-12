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

use crate::sugarcube::ast::{self, AnalyzedVarOp, SetOperator};
use crate::sugarcube::classifier::ClassifiedPassage;
use crate::sugarcube::js::js_annotate::compute_target_segment_spans;
use crate::sugarcube::parser::predicates::is_assignment_macro;
use crate::sugarcube::registries::function_registry::FunctionKind;
use crate::sugarcube::registries::template_registry::TemplateKind;
use super::variable_tree::VarAccessKind;
use super::SugarCubeRegistry;

/// Map a `SetOperator` from the AST to the appropriate `VarAccessKind`.
fn set_operator_to_access_kind(op: &SetOperator) -> VarAccessKind {
    match op {
        SetOperator::To | SetOperator::Eq => VarAccessKind::Write,
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
) {
    // Record variable operations from the unified AST
    {
        let vtree = registry.variables_mut();

        let mut all_var_ops = Vec::new();

        // For script passages, collect from script_js_analysis first
        if let Some(ref analysis) = passage_ast.script_js_analysis {
            for op in &analysis.var_ops {
                all_var_ops.push((op.clone(), None));
            }
        }

        // Walk the AST nodes for inline var ops
        collect_var_ops_from_nodes(&passage_ast.nodes, &mut all_var_ops, cp, file_uri);

        // Record each variable operation
        for (op, kind_override) in &all_var_ops {
            let final_kind = kind_override.unwrap_or(op.access_kind);
            vtree.record_var(
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
            );
        }

        // Mark variables as seeded if this is a special passage
        if cp.special_def.as_ref().is_some_and(|d| {
            matches!(d.behavior, knot_core::passage::SpecialPassageBehavior::Startup)
        }) {
            for (op, _) in &all_var_ops {
                if op.access_kind.is_write() {
                    vtree.mark_seeded(&op.name);
                }
            }
        }
    }

    // Extract widget definitions and register macro_adds/template_adds/function_defs
    {
        let (macro_reg, func_reg, template_reg) = registry.definition_registries_mut();

        // For script passages, register definitions from script_js_analysis
        if let Some(ref analysis) = passage_ast.script_js_analysis {
            for macro_add in &analysis.macro_adds {
                macro_reg.register_macro_add(
                    &macro_add.name,
                    &cp.header.name,
                    file_uri,
                    macro_add.name_offset,
                    None,
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
        }

        register_definitions_from_nodes(
            &passage_ast.nodes,
            &cp.header.name,
            file_uri,
            macro_reg,
            func_reg,
            template_reg,
        );
    }
}

/// Collect all variable operations from AST nodes, applying SugarCube
/// semantic overrides.
fn collect_var_ops_from_nodes(
    nodes: &[ast::AstNode],
    result: &mut Vec<(AnalyzedVarOp, Option<VarAccessKind>)>,
    cp: &ClassifiedPassage,
    _file_uri: &str,
) {
    for node in nodes {
        match node {
            ast::AstNode::Text { var_refs, .. } => {
                for vr in var_refs {
                    let segment_spans = compute_target_segment_spans(
                        &vr.name,
                        &vr.property_path,
                        &vr.span,
                    );

                    result.push((
                        AnalyzedVarOp {
                            name: vr.name.clone(),
                            is_temporary: vr.is_temporary,
                            access_kind: VarAccessKind::Read,
                            span: vr.span.clone(),
                            property_path: vr.property_path.clone(),
                            segment_spans,
                            construct_span: None,
                        },
                        None,
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
                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());

                if has_js_analysis {
                    // Use oxc-derived var_ops (more accurate read/write classification)
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            let kind_override = determine_macro_override(name, op, set_assignment.as_ref(), capture_target.as_ref());
                            result.push((op.clone(), kind_override));
                        }
                    }
                } else {
                    // Fall back to var_refs from SugarCube parser's scan_inline_vars
                    let is_assignment = is_assignment_macro(name);
                    for vr in var_refs {
                        let segment_spans = compute_target_segment_spans(
                            &vr.name,
                            &vr.property_path,
                            &vr.span,
                        );
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
                            },
                            None,
                        ));
                    }
                }

                // For <<set>> macros with set_assignment: emit the target variable
                // UNLESS a block write from js_analysis already covers it.
                if let Some(sa) = set_assignment {
                    let block_write_covers_target = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis.var_ops.iter().any(|op| {
                            op.name == sa.target.name
                                && op.property_path == sa.target.property_path
                                && op.construct_span.is_some()
                        })
                    });

                    if !block_write_covers_target {
                        let kind = set_operator_to_access_kind(&sa.operator);

                        let segment_spans = compute_target_segment_spans(
                            &sa.target.name,
                            &sa.target.property_path,
                            &sa.target.span,
                        );

                        result.push((
                            AnalyzedVarOp {
                                name: sa.target.name.clone(),
                                is_temporary: sa.target.is_temporary,
                                access_kind: kind,
                                span: sa.target.span.clone(),
                                property_path: sa.target.property_path.clone(),
                                segment_spans,
                                construct_span: Some(full_span.clone()),
                            },
                            None,
                        ));
                    }
                }

                // For <<capture>> macros with capture_target: emit the captured variable
                // as VarAccessKind::Capture. This provides AST-level capture tracking that
                // complements the JS annotation pass.
                if let Some(ct) = capture_target {
                    // Only emit if not already covered by js_analysis
                    let already_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis.var_ops.iter().any(|op| {
                            op.name == ct.name && op.access_kind.is_write()
                        })
                    });

                    if !already_covered {
                        let segment_spans = compute_target_segment_spans(
                            &ct.name,
                            &ct.property_path,
                            &ct.span,
                        );

                        result.push((
                            AnalyzedVarOp {
                                name: ct.name.clone(),
                                is_temporary: ct.is_temporary,
                                access_kind: VarAccessKind::Capture,
                                span: ct.span.clone(),
                                property_path: ct.property_path.clone(),
                                segment_spans,
                                construct_span: Some(full_span.clone()),
                            },
                            None,
                        ));
                    }
                }

                // For <<for>> macros with for_loop_vars: emit the loop variables.
                // The index variable (_i) is a write (receives each element).
                // The iterated variable ($array) is a read.
                if let Some(fl) = for_loop_vars {
                    // Emit index var as Write (it receives each element during iteration)
                    let index_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis.var_ops.iter().any(|op| {
                            op.name == fl.index_var.name && op.access_kind.is_write()
                        })
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
                            },
                            None,
                        ));
                    }

                    // Emit iterated var as Read
                    let iter_covered = js_analysis.as_ref().is_some_and(|analysis| {
                        analysis.var_ops.iter().any(|op| {
                            op.name == fl.iterated_var.name
                        })
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
                            },
                            None,
                        ));
                    }
                }

                // Recurse into children
                if let Some(ch) = children {
                    collect_var_ops_from_nodes(ch, result, cp, _file_uri);
                }
            }
            ast::AstNode::Expression { js_analysis, var_refs, .. } => {
                let has_js_analysis = js_analysis.as_ref().is_some_and(|a| !a.var_ops.is_empty());
                if has_js_analysis {
                    if let Some(analysis) = js_analysis {
                        for op in &analysis.var_ops {
                            result.push((op.clone(), None));
                        }
                    }
                } else {
                    // Fall back to var_refs
                    for vr in var_refs {
                        let segment_spans = compute_target_segment_spans(
                            &vr.name,
                            &vr.property_path,
                            &vr.span,
                        );
                        result.push((
                            AnalyzedVarOp {
                                name: vr.name.clone(),
                                is_temporary: vr.is_temporary,
                                access_kind: VarAccessKind::Read,
                                span: vr.span.clone(),
                                property_path: vr.property_path.clone(),
                                segment_spans,
                                construct_span: None,
                            },
                            None,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
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
        let is_capture_target = capture_target.map_or(false, |ct| {
            ct.name == op.name && ct.span.start == op.span.start
        });

        if is_capture_target || (capture_target.is_none() && op.access_kind.is_write()) {
            return Some(VarAccessKind::Capture);
        }
    }

    if macro_name.eq_ignore_ascii_case("unset") {
        if op.access_kind.is_write() {
            return Some(VarAccessKind::Unset);
        }
    }

    if macro_name.eq_ignore_ascii_case("set") {
        if let Some(sa) = set_assignment {
            if op.name == sa.target.name && op.span.start == sa.target.span.start {
                let kind = set_operator_to_access_kind(&sa.operator);
                if op.access_kind != kind {
                    return Some(kind);
                }
            }
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
                        macro_reg.register_widget(
                            &widget_name,
                            passage_name,
                            file_uri,
                            definition_name_span.as_ref().map_or(open_span.start, |dns| dns.start),
                            None,
                        );
                    }
                }

                // Register Macro.add(), Template.add(), function definitions from js_analysis
                if let Some(analysis) = js_analysis {
                    for macro_add in &analysis.macro_adds {
                        macro_reg.register_macro_add(
                            &macro_add.name,
                            passage_name,
                            file_uri,
                            macro_add.name_offset,
                            None,
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
                            template_add.name_offset,
                        );
                    }
                    for func_def in &analysis.function_defs {
                        func_reg.register_function(
                            &func_def.name,
                            FunctionKind::Declaration,
                            passage_name,
                            file_uri,
                            func_def.name_offset,
                            func_def.param_count,
                        );
                    }
                }

                // Recurse into children
                if let Some(ch) = children {
                    register_definitions_from_nodes(
                        ch, passage_name, file_uri,
                        macro_reg, func_reg, template_reg,
                    );
                }
            }
            ast::AstNode::Expression { js_analysis, .. } => {
                if let Some(analysis) = js_analysis {
                    for macro_add in &analysis.macro_adds {
                        macro_reg.register_macro_add(
                            &macro_add.name,
                            passage_name,
                            file_uri,
                            macro_add.name_offset,
                            None,
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
                            template_add.name_offset,
                        );
                    }
                    for func_def in &analysis.function_defs {
                        func_reg.register_function(
                            &func_def.name,
                            FunctionKind::Declaration,
                            passage_name,
                            file_uri,
                            func_def.name_offset,
                            func_def.param_count,
                        );
                    }
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
) {
    populate_registries_from_unified_ast(registry, passage_ast, cp, file_uri);
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
    use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};
    use crate::sugarcube::js::js_preprocess;
    use crate::sugarcube::js::js_walk;

    let preprocessed = js_preprocess::preprocess_for_oxc(body_text);

    match parse_js(&preprocessed.source, JsParseMode::Module) {
        JsParseOutcome::Success(output) => {
            output.with_program(|program| {
                let analysis = js_walk::walk_script_passage(program, &preprocessed);

                // Record variable operations
                let vtree = registry.variables_mut();
                for op in &analysis.var_ops {
                    vtree.record_var(
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
        JsParseOutcome::Error(_diagnostics) => {}
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

        // Phase 2: JS annotation
        js_annotate::annotate_js(&mut ast, body, false);

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
        populate_registries_from_unified_ast(&mut registry, &ast, &cp, "file:///test.tw");

        // Verify $ITEMS exists with a READ access
        let vtree = registry.variables();
        let items_var = vtree.get_variable("$ITEMS");
        assert!(items_var.is_some(), "$ITEMS should be in registry from State.variables.ITEMS detection");
        if let Some((_, node)) = items_var {
            let reads: Vec<_> = node.meta.refs.iter().filter(|a| a.is_read()).collect();
            assert!(!reads.is_empty(), "$ITEMS should have at least one READ");
        }
    }
}
