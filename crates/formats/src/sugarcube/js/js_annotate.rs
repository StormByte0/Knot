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
use crate::sugarcube::registries::variable_tree::VarAccessKind;
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

                // For <<set>> with object literal RHS: extract property paths
                if name.eq_ignore_ascii_case("set") {
                    if let Some(sa) = set_assignment {
                        if let Some(expr) = &sa.expression {
                            let trimmed = expr.trim();
                            if trimmed.starts_with('{') {
                                let expr_span = sa.expression_span.as_ref().cloned().unwrap_or_else(|| sa.target.span.clone());
                                let obj_literal_span = compute_object_literal_span(trimmed, &expr_span, expr);

                                let construct_span = obj_literal_span.clone()
                                    .or_else(|| sa.expression_span.clone());

                                let leading_ws = expr.len() - expr.trim_start().len();
                                let expr_body_offset = sa.expression_span.as_ref()
                                    .map(|s| s.start + leading_ws)
                                    .unwrap_or(sa.target.span.start);

                                let property_paths = extract_object_property_paths_detailed(trimmed, expr_body_offset);
                                if !property_paths.is_empty() {
                                    let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);

                                    let target_seg_spans = compute_target_segment_spans(
                                        &sa.target.name,
                                        &sa.target.property_path,
                                        &sa.target.span,
                                    );

                                    if let Some(full_span) = construct_span.clone() {
                                        analysis.var_ops.push(AnalyzedVarOp {
                                            name: sa.target.name.clone(),
                                            is_temporary: sa.target.is_temporary,
                                            access_kind: VarAccessKind::Write,
                                            span: full_span.clone(),
                                            property_path: sa.target.property_path.clone(),
                                            segment_spans: target_seg_spans,
                                            construct_span: Some(full_span),
                                        });
                                    }

                                    for (path, mut segment_spans) in property_paths {
                                        let target_seg_spans = compute_target_segment_spans(
                                            &sa.target.name,
                                            &sa.target.property_path,
                                            &sa.target.span,
                                        );
                                        segment_spans.splice(0..0, target_seg_spans);

                                        analysis.var_ops.push(AnalyzedVarOp {
                                            name: sa.target.name.clone(),
                                            is_temporary: sa.target.is_temporary,
                                            access_kind: VarAccessKind::Write,
                                            span: segment_spans.last().cloned().unwrap_or_else(|| sa.target.span.clone()),
                                            property_path: if sa.target.property_path.is_empty() {
                                                path
                                            } else {
                                                format!("{}.{}", sa.target.property_path, path)
                                            },
                                            segment_spans,
                                            construct_span: construct_span.clone(),
                                        });
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

// ---------------------------------------------------------------------------
// Object literal property path extraction
// ---------------------------------------------------------------------------

/// Extract property paths with per-segment spans from an object literal expression.
fn extract_object_property_paths_detailed(source: &str, body_offset: usize) -> Vec<(String, Vec<std::ops::Range<usize>>)> {
    let mut preprocessed = js_preprocess::preprocess_for_oxc(source);
    preprocessed.wrapping_offset = 1;
    preprocessed.origin_offset = body_offset;

    let outcome = parse_js(&preprocessed.source, JsParseMode::Expression);
    outcome.with_program(|program| {
        let mut results = Vec::new();
        for stmt in &program.body {
            if let oxc_ast::ast::Statement::ExpressionStatement(expr_stmt) = stmt {
                collect_object_paths_detailed_from_oxc_expr(
                    &expr_stmt.expression, &mut results, String::new(), &preprocessed,
                );
            }
        }
        results
    }).unwrap_or_default()
}

/// Compute the passage-body-relative span of the object literal portion `{...}`
fn compute_object_literal_span(
    trimmed: &str,
    expr_span: &std::ops::Range<usize>,
    original_expr: &str,
) -> Option<std::ops::Range<usize>> {
    let obj_start_in_trimmed = trimmed.find('{')?;
    let mut depth = 0i32;
    let mut obj_end_in_trimmed = obj_start_in_trimmed;
    let bytes = trimmed.as_bytes();
    let len = bytes.len();
    let mut i = obj_start_in_trimmed;
    while i < len {
        let b = bytes[i];
        match b {
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    obj_end_in_trimmed = i + 1;
                    break;
                }
                i += 1;
            }
            b'"' | b'\'' => {
                let quote = b;
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < len {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    if depth != 0 {
        return None;
    }

    let leading_ws = original_expr.len() - original_expr.trim_start().len();
    let obj_start_in_original = leading_ws + obj_start_in_trimmed;
    let obj_end_in_original = leading_ws + obj_end_in_trimmed;

    let offset = expr_span.start;
    Some((offset + obj_start_in_original)..(offset + obj_end_in_original))
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

/// Recursively collect property paths with per-segment spans from an oxc expression.
fn collect_object_paths_detailed_from_oxc_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    results: &mut Vec<(String, Vec<std::ops::Range<usize>>)>,
    prefix: String,
    preprocessed: &js_preprocess::PreprocessedJs,
) {
    use oxc_ast::ast::Expression;

    match expr {
        Expression::ParenthesizedExpression(pe) => {
            collect_object_paths_detailed_from_oxc_expr(
                &pe.expression, results, prefix, preprocessed,
            );
        }
        Expression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop {
                    let key_str = match &p.key {
                        oxc_ast::ast::PropertyKey::StaticIdentifier(id) => {
                            Some(id.name.to_string())
                        }
                        oxc_ast::ast::PropertyKey::StringLiteral(s) => {
                            Some(s.value.to_string())
                        }
                        oxc_ast::ast::PropertyKey::NumericLiteral(n) => {
                            Some(n.value.to_string())
                        }
                        _ => None,
                    };

                    if let Some(key) = key_str {
                        let path = if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{}.{}", prefix, key)
                        };

                        let key_span = p.key.span();
                        let start = preprocessed.map_to_original(key_span.start as usize);
                        let end = preprocessed.map_to_original(key_span.end as usize);
                        let key_range = start..end;

                        let segment_spans = build_segment_spans_for_path(
                            &path, results, &key_range,
                        );

                        results.push((path.clone(), segment_spans));

                        collect_object_paths_detailed_from_oxc_expr(
                            &p.value, results, path, preprocessed,
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

/// Build the segment_spans vector for a property path by looking up
/// the spans of each ancestor segment from previously collected results.
fn build_segment_spans_for_path(
    path: &str,
    previous_results: &[(String, Vec<std::ops::Range<usize>>)],
    leaf_span: &std::ops::Range<usize>,
) -> Vec<std::ops::Range<usize>> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return vec![leaf_span.clone()];
    }

    let mut segment_spans = Vec::with_capacity(segments.len());

    for (i, _seg) in segments.iter().enumerate() {
        if i < segments.len() - 1 {
            let prefix: String = segments[..=i].join(".");
            if let Some((_, spans)) = previous_results.iter().find(|(p, _)| p == &prefix) {
                if let Some(last) = spans.last() {
                    segment_spans.push(last.clone());
                }
            }
        }
    }

    segment_spans.push(leaf_span.clone());
    segment_spans
}
