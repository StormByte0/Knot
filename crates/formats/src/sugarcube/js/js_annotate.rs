//! JS annotation pass — Phase 2 of the unified AST pipeline.
//!
//! This module walks the SugarCube AST, finds nodes that contain JS content,
//! preprocesses + parses them with oxc, and attaches [`JsAnalysis`] to each
//! node. This replaces the old dual-path approach where both `scan_inline_vars`
//! and `js_walk` independently produced variable entries.
//!
//! ## Pipeline position
//!
//! 1. Phase 1 — Structural parse (SugarCube parser, produces AST with
//!    `js_analysis: None`)
//! 2. **Phase 2 — JS annotation** (this module, fills in `js_analysis`)
//! 3. Phase 3 — Registry population (single walk over unified AST)
//!
//! ## Script passages
//!
//! Script passages contain pure JS (no SugarCube syntax). Their AST is a
//! single Text node, which doesn't have a `js_analysis` field. Instead of
//! the old synthetic `__script_passage__` node hack, we store the analysis
//! on `PassageAst::script_js_analysis`. This avoids polluting every
//! downstream consumer (tokens, body blocks, widget detection) with a fake
//! macro node.

use crate::sugarcube::ast::{AnalyzedVarOp, AstNode, JsAnalysis, PassageAst};
use crate::sugarcube::js::js_preprocess;
use crate::sugarcube::js::js_walk;
use knot_core::oxc::{parse_js, ParseMode as JsParseMode};
use oxc_span::GetSpan;

/// Annotate AST nodes with JS analysis results (Phase 2).
///
/// Walks the AST, finds nodes that contain JS content, preprocesses + parses
/// with oxc, and attaches `JsAnalysis` to each node. This mutates the nodes
/// in place, filling in the `js_analysis` field.
///
/// For script passages, pass `is_script_passage = true` and the entire body
/// text is analyzed as a single JS module. The result is stored on
/// `PassageAst::script_js_analysis` (not on a synthetic node).
pub fn annotate_js(passage_ast: &mut PassageAst, body_text: &str, is_script_passage: bool) {
    if is_script_passage {
        annotate_script_passage(passage_ast, body_text);
    } else {
        annotate_inline_js(&mut passage_ast.nodes, body_text);
    }
}

/// Annotate a script passage's AST with JS analysis.
///
/// Script passages contain pure JS (no SugarCube syntax). The entire body
/// text is preprocessed and parsed as a JS module. The resulting `JsAnalysis`
/// is stored on `PassageAst::script_js_analysis` — NOT on a synthetic node.
fn annotate_script_passage(passage_ast: &mut PassageAst, body_text: &str) {
    if body_text.trim().is_empty() {
        return;
    }

    // Preprocess $var references for oxc
    let preprocessed = js_preprocess::preprocess_for_oxc(body_text);

    // Parse with oxc as a JS module.
    // oxc has error recovery — even when there are syntax errors, the AST
    // is usually still available (partial). We walk whatever AST we can get
    // so the user gets token highlighting for the valid parts while the
    // broken parts get precise error diagnostics via js_validate.
    //
    // If oxc panics (unrecoverable error), we leave script_js_analysis as
    // None — no tokens are emitted for the JS body. This is intentional:
    // a blank JS block is a clearer signal that "something is broken" than
    // a sea of approximate tokens from a fallback scanner. The diagnostic
    // from js_validate still shows the error location.
    let outcome = parse_js(&preprocessed.source, JsParseMode::Module);
    if let Some(analysis) = outcome.with_program(|program| {
        js_walk::walk_script_passage(program, &preprocessed)
    }) {
        passage_ast.script_js_analysis = Some(analysis);
    }
}

/// Annotate inline JS snippets in normal passage AST nodes.
///
/// Walks the AST, finds Macro and Expression nodes that contain JS,
/// preprocesses + parses each with oxc, and attaches `JsAnalysis`.
fn annotate_inline_js(nodes: &mut [AstNode], _body_text: &str) {
    for node in nodes.iter_mut() {
        match node {
            AstNode::Macro {
                name,
                args,
                open_span,
                children,
                set_assignment,
                js_analysis,
                ..
            } => {
                // <<script>> blocks: the body comes from child Text nodes.
                if name == "script" {
                    if let Some(ch) = children {
                        // Collect text content from children as JS source
                        let mut js_source = String::new();
                        let mut body_start = open_span.end;
                        for child in ch.iter() {
                            if let AstNode::Text { content, span, .. } = child {
                                if js_source.is_empty() {
                                    body_start = span.start;
                                }
                                js_source.push_str(content);
                            }
                        }
                        if !js_source.trim().is_empty() {
                            // Adjust body_start to account for leading whitespace
                            // stripped by trim(). The preprocessed source starts
                            // at the first non-whitespace char, so origin_offset
                            // must point there too.
                            let leading_ws = js_source.len() - js_source.trim_start().len();
                            let analysis = analyze_js_snippet(
                                js_source.trim(),
                                body_start + leading_ws,
                                true, // is_block = true for <<script>>
                            );
                            *js_analysis = Some(analysis);
                        }
                    }
                    // Still recurse into children for nested constructs
                    if let Some(ch) = children {
                        annotate_inline_js(ch, _body_text);
                    }
                    continue;
                }

                // For all other macros, determine if they contain JS that needs annotation
                let snippet = collect_macro_js_snippet(
                    name, args, open_span.clone(), set_assignment.as_ref(),
                );

                if let Some((source, body_offset, is_block)) = snippet {
                    let analysis = analyze_js_snippet(&source, body_offset, is_block);
                    *js_analysis = Some(analysis);
                }

                // For <<set>> with block literal RHS (object or array):
                // decompose into leaf writes per the propagation model
                // (see docs/variable-write-propagation-model.md).
                //
                // Block-assigned roots do NOT get a direct write — only
                // leaf scalar properties get direct writes, which then
                // propagate up. Each leaf write carries segment_construct_spans
                // for propagation.
                if name.eq_ignore_ascii_case("set") {
                    if let Some(sa) = set_assignment {
                        if let Some(expr) = &sa.expression {
                            let trimmed = expr.trim();
                            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                                let expr_span = sa.expression_span.as_ref().cloned()
                                    .unwrap_or_else(|| sa.target.span.clone());

                                let leading_ws = expr.len() - expr.trim_start().len();
                                let expr_body_offset = sa.expression_span.as_ref()
                                    .map(|s| s.start + leading_ws)
                                    .unwrap_or(sa.target.span.start);

                                // Compute the full assignment construct span
                                // (target + `to` + RHS). Used as the root
                                // construct span for propagation.
                                let assign_span = {
                                    let start = sa.target.span.start;
                                    let end = expr_span.end;
                                    start..end
                                };

                                let target_seg_spans = compute_target_segment_spans(
                                    &sa.target.name,
                                    &sa.target.property_path,
                                    &sa.target.span,
                                );

                                // Decompose the block literal into leaf writes.
                                let leaf_writes = decompose_block_literal_for_set(
                                    trimmed,
                                    expr_body_offset,
                                    &sa.target.name,
                                    sa.target.is_temporary,
                                    &sa.target.property_path,
                                    &target_seg_spans,
                                    assign_span,
                                );

                                if !leaf_writes.is_empty() {
                                    let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                                    for op in leaf_writes {
                                        analysis.var_ops.push(op);
                                    }
                                }
                            }
                        }
                    }
                }

                // Recurse into children
                if let Some(ch) = children {
                    annotate_inline_js(ch, _body_text);
                }
            }
            AstNode::Expression {
                content,
                span,
                js_analysis,
                ..
            } => {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    // Compute the body-relative byte offset of `trimmed`
                    // within the expression construct.
                    //
                    // `span.start` is the position of `<<` (the start of the
                    // full expression construct). `content` is the text
                    // between `<<=` (or `<<-`) and `>>`, so it starts at
                    // `span.start + 3` (3 = length of `<<=` / `<<-`).
                    // `trimmed` is `content.trim()`, so its start is
                    // `content_start + leading_whitespace_bytes`.
                    //
                    // Without this correction, `analyze_js_snippet` would
                    // compute variable spans as `span.start + pos_within_trimmed`,
                    // which is offset by `3 + leading_ws` bytes too early.
                    // That caused variable hover to fire when the cursor was
                    // on `<<=` or ` ` (inside the open tag) instead of on
                    // the actual variable — which in turn shadowed the
                    // macro hover for `<<=>>` / `<<->>`.
                    let leading_ws = content.len() - content.trim_start().len();
                    let trimmed_offset = span.start + 3 + leading_ws;
                    let analysis = analyze_js_snippet(trimmed, trimmed_offset, false);
                    *js_analysis = Some(analysis);
                }
            }
            _ => {}
        }
    }
}

/// Collect a JS snippet from a macro node for annotation.
///
/// Returns `Some((source, body_offset, is_block))` if the macro contains
/// JS that should be analyzed, or `None` otherwise.
fn collect_macro_js_snippet(
    name: &str,
    args: &str,
    open_span: std::ops::Range<usize>,
    set_assignment: Option<&crate::sugarcube::ast::SetAssignment>,
) -> Option<(String, usize, bool)> {
    use crate::sugarcube::macros::inline_js_macro_names;

    if name == "script" {
        None
    } else if name == "for" {
        // <<for>> has SugarCube-specific syntax (not JS): C-style, range,
        // simple iteration, and for-in forms. Don't send to oxc.
        None
    } else if name == "set" {
        if let Some(sa) = set_assignment {
            if let Some(expr) = &sa.expression {
                let trimmed = expr.trim();
                if !trimmed.is_empty() {
                    let leading_ws = expr.len() - expr.trim_start().len();
                    let expr_body_offset = sa.expression_span.as_ref()
                        .map(|s| s.start + leading_ws)
                        .unwrap_or(open_span.start);
                    Some((trimmed.to_string(), expr_body_offset, false))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            let trimmed = args.trim();
            if !trimmed.is_empty() {
                let leading_ws = args.len() - args.trim_start().len();
                let args_body_start = open_span.end - 2 - args.len() + leading_ws;
                Some((trimmed.to_string(), args_body_start, false))
            } else {
                None
            }
        }
    } else if inline_js_macro_names().contains(name) {
        let trimmed = args.trim();
        if !trimmed.is_empty() {
            let leading_ws = args.len() - args.trim_start().len();
            let args_body_start = open_span.end - 2 - args.len() + leading_ws;
            Some((trimmed.to_string(), args_body_start, false))
        } else {
            None
        }
    } else if args.contains('$') || args.contains('_') {
        let trimmed = args.trim();
        if !trimmed.is_empty() {
            let leading_ws = args.len() - args.trim_start().len();
            let args_body_start = open_span.end - 2 - args.len() + leading_ws;
            Some((trimmed.to_string(), args_body_start, false))
        } else {
            None
        }
    } else {
        None
    }
}

/// Analyze a single JS snippet and return a JsAnalysis.
fn analyze_js_snippet(source: &str, body_offset: usize, is_block: bool) -> JsAnalysis {
    let preprocessed = js_preprocess::preprocess_for_oxc(source);

    let js_mode = if is_block {
        JsParseMode::Module
    } else {
        JsParseMode::Expression
    };

    let wrapping_offset = match js_mode {
        JsParseMode::Expression => 1,
        _ => 0,
    };
    let shifted = js_preprocess::PreprocessedJs {
        source: preprocessed.source,
        substitutions: preprocessed.substitutions,
        origin_offset: body_offset,
        wrapping_offset,
    };

    let outcome = parse_js(&shifted.source, js_mode);
    // oxc has error recovery — walk whatever AST we can get, even if there
    // are syntax errors. The valid parts get token highlighting; the broken
    // parts get precise diagnostics via js_validate.
    //
    // If oxc panics (unrecoverable error), return empty analysis — no tokens.
    // The diagnostic from js_validate still shows the error location.
    outcome.with_program(|program| {
        if is_block {
            js_walk::walk_script_passage(program, &shifted)
        } else {
            js_walk::walk_inline_js(program, &shifted)
        }
    }).unwrap_or_default()
}

/// Decompose a block literal (object or array) into leaf writes for a
/// `<<set>>` macro.
///
/// This is the `<<set>>`-specific entry point that mirrors what
/// `check_assignment_for_var_writes` does in js_walk for `<<run>>` and
/// script passages. It parses the block literal expression string with oxc,
/// then walks the AST to emit leaf writes with per-segment construct spans.
///
/// Block-assigned roots do NOT get a direct write — only leaf scalar
/// properties get direct writes, which then propagate up.
fn decompose_block_literal_for_set(
    expr_src: &str,
    expr_body_offset: usize,
    var_name: &str,
    is_temporary: bool,
    target_property_path: &str,
    target_seg_spans: &[std::ops::Range<usize>],
    assign_span: std::ops::Range<usize>,
) -> Vec<AnalyzedVarOp> {
    let preprocessed = js_preprocess::preprocess_for_oxc(expr_src);
    let shifted = js_preprocess::PreprocessedJs {
        source: preprocessed.source,
        substitutions: preprocessed.substitutions,
        origin_offset: expr_body_offset,
        wrapping_offset: 1, // Expression mode
    };

    let outcome = parse_js(&shifted.source, JsParseMode::Expression);
    let mut result = Vec::new();

    outcome.with_program(|program| {
        for stmt in &program.body {
            if let oxc_ast::ast::Statement::ExpressionStatement(expr_stmt) = stmt {
                let expr = &expr_stmt.expression;
                // root_construct_spans has ONE entry per path depth.
                // Depth 0 = the root variable ($ITEMS). Its construct span
                // is the full assignment span (target + `=` + RHS).
                // This must stay aligned with segment_spans (which also
                // has one entry per depth, starting with the root token).
                let root_construct_spans = vec![assign_span.clone()];
                decompose_expr_for_set(
                    expr,
                    var_name,
                    is_temporary,
                    target_property_path,
                    target_seg_spans.to_vec(),
                    root_construct_spans,
                    &shifted,
                    &mut result,
                );
                break;
            }
        }
    });

    result
}

/// Recursively decompose an oxc ObjectExpression into leaf writes.
fn decompose_object_expr_for_set(
    obj: &oxc_ast::ast::ObjectExpression<'_>,
    var_name: &str,
    is_temporary: bool,
    prefix: &str,
    parent_segments: Vec<std::ops::Range<usize>>,
    parent_construct_spans: Vec<std::ops::Range<usize>>,
    preprocessed: &js_preprocess::PreprocessedJs,
    result: &mut Vec<AnalyzedVarOp>,
) {
    use crate::sugarcube::registries::variable_tree::VarAccessKind;
    use oxc_ast::ast::Expression;

    for prop in &obj.properties {
        if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop {
            let key_str = match &p.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                oxc_ast::ast::PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
                oxc_ast::ast::PropertyKey::NumericLiteral(n) => Some(n.value.to_string()),
                _ => None,
            };
            let Some(key) = key_str else { continue };

            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };

            let key_span = p.key.span();
            let key_start = preprocessed.map_to_original(key_span.start as usize);
            let key_end = preprocessed.map_to_original(key_span.end as usize);
            let key_range = key_start..key_end;

            let value_span = p.value.span();
            let value_end = preprocessed.map_to_original(value_span.end as usize);
            let prop_construct_span = key_start..value_end;

            let mut segment_spans = parent_segments.clone();
            segment_spans.push(key_range.clone());
            let mut segment_construct_spans = parent_construct_spans.clone();
            segment_construct_spans.push(prop_construct_span.clone());

            match &p.value {
                Expression::ObjectExpression(inner) => {
                    decompose_object_expr_for_set(
                        inner, var_name, is_temporary, &path,
                        segment_spans, segment_construct_spans,
                        preprocessed, result,
                    );
                }
                Expression::ArrayExpression(inner) => {
                    decompose_array_expr_for_set(
                        inner, var_name, is_temporary, &path,
                        segment_spans, segment_construct_spans,
                        preprocessed, result,
                    );
                }
                _ => {
                    result.push(AnalyzedVarOp {
                        name: var_name.to_string(),
                        is_temporary,
                        access_kind: VarAccessKind::Write,
                        span: prop_construct_span.clone(),
                        property_path: path,
                        segment_spans,
                        construct_span: Some(prop_construct_span),
                        segment_construct_spans,
                    });
                }
            }
        }
    }
}

/// Recursively decompose an oxc ArrayExpression into leaf writes.
fn decompose_array_expr_for_set(
    arr: &oxc_ast::ast::ArrayExpression<'_>,
    var_name: &str,
    is_temporary: bool,
    prefix: &str,
    parent_segments: Vec<std::ops::Range<usize>>,
    parent_construct_spans: Vec<std::ops::Range<usize>>,
    preprocessed: &js_preprocess::PreprocessedJs,
    result: &mut Vec<AnalyzedVarOp>,
) {
    use crate::sugarcube::registries::variable_tree::VarAccessKind;
    use oxc_ast::ast::ArrayExpressionElement;

    for (idx, elem) in arr.elements.iter().enumerate() {
        let key = idx.to_string();
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };

        match elem {
            ArrayExpressionElement::ObjectExpression(obj) => {
                let obj = obj.as_ref();
                let obj_span = obj.span();
                let s = preprocessed.map_to_original(obj_span.start as usize);
                let e = preprocessed.map_to_original(obj_span.end as usize);
                let elem_range = s..e;
                let mut ss = parent_segments.clone();
                ss.push(elem_range.clone());
                let mut scs = parent_construct_spans.clone();
                scs.push(elem_range.clone());
                decompose_object_expr_for_set(
                    obj, var_name, is_temporary, &path,
                    ss, scs, preprocessed, result,
                );
            }
            ArrayExpressionElement::ArrayExpression(inner) => {
                let inner = inner.as_ref();
                let arr_span = inner.span();
                let s = preprocessed.map_to_original(arr_span.start as usize);
                let e = preprocessed.map_to_original(arr_span.end as usize);
                let elem_range = s..e;
                let mut ss = parent_segments.clone();
                ss.push(elem_range.clone());
                let mut scs = parent_construct_spans.clone();
                scs.push(elem_range.clone());
                decompose_array_expr_for_set(
                    inner, var_name, is_temporary, &path,
                    ss, scs, preprocessed, result,
                );
            }
            ArrayExpressionElement::SpreadElement(_) | ArrayExpressionElement::Elision(_) => {
                continue;
            }
            _ => {
                let elem_span = elem.span();
                let s = preprocessed.map_to_original(elem_span.start as usize);
                let e = preprocessed.map_to_original(elem_span.end as usize);
                let elem_range = s..e;
                let mut ss = parent_segments.clone();
                ss.push(elem_range.clone());
                let mut scs = parent_construct_spans.clone();
                scs.push(elem_range.clone());
                result.push(AnalyzedVarOp {
                    name: var_name.to_string(),
                    is_temporary,
                    access_kind: VarAccessKind::Write,
                    span: elem_range.clone(),
                    property_path: path,
                    segment_spans: ss,
                    construct_span: Some(elem_range),
                    segment_construct_spans: scs,
                });
            }
        }
    }
}

/// Recursively decompose an oxc expression into leaf writes.
/// Delegates to `decompose_object_expr_for_set` or `decompose_array_expr_for_set`
/// based on the expression type.
fn decompose_expr_for_set(
    expr: &oxc_ast::ast::Expression<'_>,
    var_name: &str,
    is_temporary: bool,
    prefix: &str,
    parent_segments: Vec<std::ops::Range<usize>>,
    parent_construct_spans: Vec<std::ops::Range<usize>>,
    preprocessed: &js_preprocess::PreprocessedJs,
    result: &mut Vec<AnalyzedVarOp>,
) {
    use crate::sugarcube::registries::variable_tree::VarAccessKind;
    use oxc_ast::ast::Expression;

    match expr {
        Expression::ParenthesizedExpression(pe) => {
            // Unwrap parens — the inner expression is what matters.
            decompose_expr_for_set(
                &pe.expression, var_name, is_temporary, prefix,
                parent_segments, parent_construct_spans,
                preprocessed, result,
            );
        }
        Expression::ObjectExpression(obj) => {
            decompose_object_expr_for_set(
                obj, var_name, is_temporary, prefix,
                parent_segments, parent_construct_spans,
                preprocessed, result,
            );
        }
        Expression::ArrayExpression(arr) => {
            decompose_array_expr_for_set(
                arr, var_name, is_temporary, prefix,
                parent_segments, parent_construct_spans,
                preprocessed, result,
            );
        }
        _ => {
            // Non-block expression at top level — defensive fallback.
            let expr_span = expr.span();
            let s = preprocessed.map_to_original(expr_span.start as usize);
            let e = preprocessed.map_to_original(expr_span.end as usize);
            let span = s..e;
            let mut scs = parent_construct_spans;
            scs.push(span.clone());
            result.push(AnalyzedVarOp {
                name: var_name.to_string(),
                is_temporary,
                access_kind: VarAccessKind::Write,
                span: span.clone(),
                property_path: prefix.to_string(),
                segment_spans: parent_segments,
                construct_span: Some(span),
                segment_construct_spans: scs,
            });
        }
    }
}


/// Compute segment spans from a target's name, property_path, and overall span.
pub fn compute_target_segment_spans(
    name: &str,
    property_path: &str,
    target_span: &std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let base = target_span.start;

    spans.push(base..(base + name.len()));

    if property_path.is_empty() {
        return spans;
    }

    let mut offset = name.len();
    for segment in property_path.split('.') {
        offset += 1; // Skip the '.' separator
        let seg_start = base + offset;
        let seg_end = base + offset + segment.len();
        spans.push(seg_start..seg_end);
        offset += segment.len();
    }

    spans
}
