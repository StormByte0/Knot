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
use knot_core::oxc::{ParseMode as JsParseMode, parse_js};
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
///
/// `sugarcube_syntax` controls whether the SugarCube preprocessor runs
/// (`$var` → `State.variables.var`, keyword operators `to`/`is`/`eq` → JS).
/// Pass `true` for `[script]`-tagged Twee passages (SugarCube syntax is
/// allowed). Pass `false` for standalone `.js` files (pure JS, `$` is a
/// valid identifier char, no SugarCube keyword operators).
pub fn annotate_js(
    passage_ast: &mut PassageAst,
    body_text: &str,
    is_script_passage: bool,
    sugarcube_syntax: bool,
    known_macro_names: &std::collections::HashSet<String>,
) {
    if is_script_passage {
        annotate_script_passage(passage_ast, body_text, sugarcube_syntax);
    } else {
        // Inline JS inside Twee passages always uses SugarCube syntax
        // ($var, keyword operators). The `sugarcube_syntax` flag is
        // irrelevant for inline JS.
        annotate_inline_js(&mut passage_ast.nodes, body_text, known_macro_names);
    }
}

/// Annotate a script passage's AST with JS analysis.
///
/// Script passages contain pure JS (no SugarCube syntax). The entire body
/// text is preprocessed and parsed as a JS module. The resulting `JsAnalysis`
/// is stored on `PassageAst::script_js_analysis` — NOT on a synthetic node.
///
/// `sugarcube_syntax` controls whether the SugarCube preprocessor runs.
/// See [`annotate_js`] for details.
fn annotate_script_passage(passage_ast: &mut PassageAst, body_text: &str, sugarcube_syntax: bool) {
    if body_text.trim().is_empty() {
        return;
    }

    // Preprocess $var references for oxc (only when sugarcube_syntax is true)
    let preprocessed = js_preprocess::preprocess_for_oxc(body_text, sugarcube_syntax);

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
    if let Some(analysis) =
        outcome.with_program(|program| js_walk::walk_script_passage(program, &preprocessed))
    {
        passage_ast.script_js_analysis = Some(analysis);
    }
}

/// Annotate inline JS snippets in normal passage AST nodes.
///
/// Walks the AST, finds Macro and Expression nodes that contain JS,
/// preprocesses + parses each with oxc, and attaches `JsAnalysis`.
fn annotate_inline_js(
    nodes: &mut [AstNode],
    _body_text: &str,
    known_macro_names: &std::collections::HashSet<String>,
) {
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
                        annotate_inline_js(ch, _body_text, known_macro_names);
                    }
                    continue;
                }

                // For all other macros, determine if they contain JS that needs annotation
                let snippet = collect_macro_js_snippet(
                    name,
                    args,
                    open_span.clone(),
                    set_assignment.as_ref(),
                    known_macro_names,
                );

                if let Some((source, body_offset, is_block)) = snippet {
                    let analysis = analyze_js_snippet(&source, body_offset, is_block);
                    *js_analysis = Some(analysis);
                }

                // ── <<for>> manual annotation ──────────────────────────────
                // <<for>> is NOT sent to oxc because its SugarCube-specific
                // syntax (range form `from...to`, simplified `_i, $array`)
                // can't be reliably substituted into valid JS. We manually
                // scan the args string for SugarCube keyword operators and
                // numeric literals, emitting Operator and Number spans.
                //
                // Variable tokens for $var/_var references are already
                // emitted by the `var_refs` scanner in `extraction.rs`,
                // so we only handle operators and literals here.
                if name.eq_ignore_ascii_case("for") {
                    let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                    annotate_for_macro_args(args, open_span.clone(), analysis);
                }

                // For <<set>> with block literal RHS (object or array):
                // decompose into leaf writes per the propagation model
                // (see docs/variable-write-propagation-model.md).
                //
                // Block-assigned roots do NOT get a direct write — only
                // leaf scalar properties get direct writes, which then
                // propagate up. Each leaf write carries segment_construct_spans
                // for propagation.
                //
                // For <<set>> with SCALAR RHS (string, number, bool, etc.):
                // emit a direct Write var_op on the target. The oxc analysis
                // path only sees the RHS expression (not the full assignment),
                // so without this, scalar-assigned variables get NO variable
                // token — inconsistent with block-assigned variables which
                // get tokens via leaf write decomposition.
                if name.eq_ignore_ascii_case("set")
                    && let Some(sa) = set_assignment
                {
                    if let Some(expr) = &sa.expression {
                        let trimmed = expr.trim();
                        if trimmed.starts_with('{') || trimmed.starts_with('[') {
                            let expr_span = sa
                                .expression_span
                                .as_ref()
                                .cloned()
                                .unwrap_or_else(|| sa.target.span.clone());

                            let leading_ws = expr.len() - expr.trim_start().len();
                            let expr_body_offset = sa
                                .expression_span
                                .as_ref()
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
                            } else {
                                // Empty block literal (e.g., `<<set $x to []>>`
                                // or `<<set $x to {}>>`) — no leaf writes to
                                // decompose. Emit a direct Write on the root
                                // so the variable still gets a token, matching
                                // the behavior for scalar assignments and
                                // non-empty block assignments.
                                let target_span = sa.target.span.clone();
                                let segment_spans = compute_target_segment_spans(
                                    &sa.target.name,
                                    &sa.target.property_path,
                                    &target_span,
                                );
                                let var_name = sa.target.name.clone();
                                let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                                analysis.var_ops.push(AnalyzedVarOp {
                                    name: var_name,
                                    is_temporary: sa.target.is_temporary,
                                    access_kind: VarAccessKind::Write,
                                    span: target_span.clone(),
                                    property_path: sa.target.property_path.clone(),
                                    segment_spans,
                                    construct_span: Some(target_span.clone()),
                                    segment_construct_spans: vec![target_span],
                                });
                            }
                        } else {
                            // Scalar RHS — emit a direct Write var_op on
                            // the target variable. This ensures every
                            // `<<set $var to <scalar>>>` produces a
                            // Variable+Definition token, matching the
                            // behavior for block-literal assignments.
                            let target_span = sa.target.span.clone();
                            let segment_spans = compute_target_segment_spans(
                                &sa.target.name,
                                &sa.target.property_path,
                                &target_span,
                            );
                            let var_name = sa.target.name.clone();
                            let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                            analysis.var_ops.push(AnalyzedVarOp {
                                name: var_name,
                                is_temporary: sa.target.is_temporary,
                                access_kind: VarAccessKind::Write,
                                span: target_span.clone(),
                                property_path: sa.target.property_path.clone(),
                                segment_spans,
                                construct_span: Some(target_span.clone()),
                                segment_construct_spans: vec![target_span],
                            });
                        }
                    } else {
                        // Postfix ++ / -- (expression is None).
                        // Emit a Write var_op on the target variable so it
                        // gets a Variable(Definition) token. Without this,
                        // `<<set $a++>>` produces the `++` Operator token
                        // but NO Variable token for `$a` — the variable
                        // appears uncolored.
                        let target_span = sa.target.span.clone();
                        let segment_spans = compute_target_segment_spans(
                            &sa.target.name,
                            &sa.target.property_path,
                            &target_span,
                        );
                        let var_name = sa.target.name.clone();
                        let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                        analysis.var_ops.push(AnalyzedVarOp {
                            name: var_name,
                            is_temporary: sa.target.is_temporary,
                            access_kind: VarAccessKind::Write,
                            span: target_span.clone(),
                            property_path: sa.target.property_path.clone(),
                            segment_spans,
                            construct_span: Some(target_span.clone()),
                            segment_construct_spans: vec![target_span],
                        });
                    }
                }

                // ── Emit operator token for <<set>> assignment operators ──
                // oxc only sees the RHS expression (never the operator), so
                // the standard `emit_*_operator` paths in `js_walk` never
                // fire for `<<set>>`. We emit the operator span directly
                // here, using the `operator_span` captured by the parser.
                //
                // This covers: `to`, `into`, `=`, `+=`, `-=`, `*=`, `/=`,
                // `%=`, `++` (postfix), `--` (postfix).
                if name.eq_ignore_ascii_case("set")
                    && let Some(sa) = set_assignment
                    && let Some(op_span) = &sa.operator_span
                {
                    let kind = match sa.operator {
                        crate::sugarcube::ast::SetOperator::To
                        | crate::sugarcube::ast::SetOperator::Into
                        | crate::sugarcube::ast::SetOperator::Eq => {
                            crate::sugarcube::ast::OperatorKind::Assignment
                        }
                        crate::sugarcube::ast::SetOperator::PlusEq
                        | crate::sugarcube::ast::SetOperator::MinusEq
                        | crate::sugarcube::ast::SetOperator::StarEq
                        | crate::sugarcube::ast::SetOperator::SlashEq
                        | crate::sugarcube::ast::SetOperator::PercentEq => {
                            crate::sugarcube::ast::OperatorKind::CompoundAssign
                        }
                        crate::sugarcube::ast::SetOperator::PostfixPlus
                        | crate::sugarcube::ast::SetOperator::PostfixMinus => {
                            crate::sugarcube::ast::OperatorKind::Arithmetic
                        }
                    };
                    let analysis = js_analysis.get_or_insert_with(JsAnalysis::default);
                    analysis
                        .operator_spans
                        .push(crate::sugarcube::ast::OperatorSpan {
                            kind,
                            span: op_span.clone(),
                        });
                }

                // Recurse into children
                if let Some(ch) = children {
                    annotate_inline_js(ch, _body_text, known_macro_names);
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
    known_macro_names: &std::collections::HashSet<String>,
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
                    let expr_body_offset = sa
                        .expression_span
                        .as_ref()
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
    } else if (args.contains('$') || args.contains('_')) && !known_macro_names.contains(name) {
        // Fallback: unknown macros whose args contain $var/_var may contain
        // JS expressions. But ONLY for macros not in the known set — custom
        // widgets like <<statblock "Strength" $stats.strength>> use
        // SugarCube discrete-argument syntax (not JS), and sending their
        // args to oxc produces false "Expected `,` or `)`" errors.
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

/// Manually annotate `<<for>>` macro args with Operator and Number tokens.
///
/// `<<for>>` is NOT sent to oxc because its SugarCube-specific syntax
/// (range form `from...to`, simplified `_i, $array`, C-style with
/// SugarCube keywords) can't be reliably substituted into valid JS.
/// This function manually scans the args string for:
///
/// - SugarCube keyword operators (`from`, `to`, `and`, `or`, `not`,
///   `lt`, `gt`, `lte`, `gte`, `eq`, `neq`, `is`, `isnot`, `def`, `ndef`)
/// - Numeric literals (`42`, `3.14`)
/// - Symbolic operators that might appear in C-style form (`++`, `--`,
///   `<`, `>`, `=`, `;`)
///
/// Variable tokens for `$var`/`_var` references are already emitted by
/// the `var_refs` scanner in `extraction.rs`, so we don't duplicate that
/// here.
///
/// `args` is the raw args string (between `<<for ` and `>>`).
/// `open_span` is the body-relative span of `<<for args` (from `<<` to
/// just before `>>`). We compute the args offset from this.
fn annotate_for_macro_args(
    args: &str,
    open_span: std::ops::Range<usize>,
    analysis: &mut JsAnalysis,
) {
    if args.is_empty() {
        return;
    }

    // Compute the body-relative offset of the args string.
    //
    // `open_span` covers `<<name args>>` — from `<<` to PAST `>>`.
    // So `open_span.end` is 2 bytes past the `>>` delimiter.
    // The args string (stored on the Macro node) does NOT include `>>`
    // or the leading space after the macro name — it starts at the first
    // non-space char after `<<name ` and ends at the last char before `>>`.
    //
    // Therefore: args_offset = open_span.end - 2 (for `>>`) - args.len()
    let args_offset = open_span.end.saturating_sub(2 + args.len());

    let bytes = args.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // SugarCube keyword operators that can appear in <<for>> args.
    // Sorted by length (longest first) to ensure longest-match.
    const FOR_KEYWORDS: &[&str] = &[
        "isnot", "ndef", "from", "and", "gte", "lte", "neq", "def", "eq", "gt", "is", "lt", "or",
        "to", "not",
    ];

    // Symbolic operator chars that can appear in C-style <<for>>.
    fn is_sym_op_char(b: u8) -> bool {
        matches!(
            b,
            b'+' | b'-' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' | b'*' | b'/' | b'%'
        )
    }

    fn is_ident_char(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    while i < len {
        let b = bytes[i];

        // Skip whitespace and semicolons (C-style for separator)
        if b == b' ' || b == b'\t' || b == b';' || b == b',' {
            i += 1;
            continue;
        }

        // Check for SugarCube keyword operators (alphabetic words at word boundaries)
        if b.is_ascii_alphabetic() {
            // Word boundary before: previous char must NOT be an ident char
            let word_boundary_before = i == 0 || !is_ident_char(bytes[i - 1]);

            if word_boundary_before {
                // Try to match a keyword (longest first)
                let mut matched = None;
                for &kw in FOR_KEYWORDS {
                    if args[i..].starts_with(kw) {
                        // Word boundary after: next char must NOT be an ident char
                        let after_pos = i + kw.len();
                        let word_boundary_after =
                            after_pos >= len || !is_ident_char(bytes[after_pos]);
                        if word_boundary_after {
                            matched = Some(kw);
                            break;
                        }
                    }
                }

                if let Some(kw) = matched {
                    let kind = match kw {
                        "to" => crate::sugarcube::ast::OperatorKind::Assignment,
                        "from" => crate::sugarcube::ast::OperatorKind::Assignment,
                        "and" | "or" | "not" => crate::sugarcube::ast::OperatorKind::Logical,
                        "eq" | "neq" | "is" | "isnot" | "gt" | "gte" | "lt" | "lte" => {
                            crate::sugarcube::ast::OperatorKind::Comparison
                        }
                        "def" | "ndef" => crate::sugarcube::ast::OperatorKind::Comparison,
                        _ => unreachable!(),
                    };
                    let span = (args_offset + i)..(args_offset + i + kw.len());
                    analysis
                        .operator_spans
                        .push(crate::sugarcube::ast::OperatorSpan { kind, span });
                    i += kw.len();
                    continue;
                }
            }

            // Not a keyword — skip the rest of this word
            while i < len && is_ident_char(bytes[i]) {
                i += 1;
            }
            continue;
        }

        // Check for numeric literals
        if b.is_ascii_digit() {
            let start = i;
            while i < len && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let span = (args_offset + start)..(args_offset + i);
            analysis
                .literal_spans
                .push(crate::sugarcube::ast::LiteralSpan {
                    kind: crate::sugarcube::ast::LiteralKind::Number,
                    span,
                });
            continue;
        }

        // Check for symbolic operators (C-style form: ++, --, <, >, =, etc.)
        if is_sym_op_char(b) {
            let start = i;
            while i < len && is_sym_op_char(bytes[i]) {
                i += 1;
            }
            // Only emit if this looks like an operator, not a lone `-` in
            // a variable name (already handled by the ident scanner above).
            let op_text = &args[start..i];
            let kind = match op_text {
                "++" | "--" => crate::sugarcube::ast::OperatorKind::Arithmetic,
                "=" | "+=" | "-=" | "*=" | "/=" | "%=" => {
                    crate::sugarcube::ast::OperatorKind::Assignment
                }
                "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=" => {
                    crate::sugarcube::ast::OperatorKind::Comparison
                }
                "&&" | "||" | "!" => crate::sugarcube::ast::OperatorKind::Logical,
                "+" | "-" | "*" | "/" | "%" => crate::sugarcube::ast::OperatorKind::Arithmetic,
                _ => {
                    // Unknown symbolic sequence — skip without emitting
                    continue;
                }
            };
            let span = (args_offset + start)..(args_offset + i);
            analysis
                .operator_spans
                .push(crate::sugarcube::ast::OperatorSpan { kind, span });
            continue;
        }

        // Skip any other character (parens, dots in property paths, etc.)
        i += 1;
    }
}

/// Analyze a single JS snippet and return a JsAnalysis.
fn analyze_js_snippet(source: &str, body_offset: usize, is_block: bool) -> JsAnalysis {
    let preprocessed = js_preprocess::preprocess_for_oxc(source, true);

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
    outcome
        .with_program(|program| {
            if is_block {
                js_walk::walk_script_passage(program, &shifted)
            } else {
                js_walk::walk_inline_js(program, &shifted)
            }
        })
        .unwrap_or_default()
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
    let preprocessed = js_preprocess::preprocess_for_oxc(expr_src, true);
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
#[allow(clippy::too_many_arguments)]
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
                        inner,
                        var_name,
                        is_temporary,
                        &path,
                        segment_spans,
                        segment_construct_spans,
                        preprocessed,
                        result,
                    );
                }
                Expression::ArrayExpression(inner) => {
                    decompose_array_expr_for_set(
                        inner,
                        var_name,
                        is_temporary,
                        &path,
                        segment_spans,
                        segment_construct_spans,
                        preprocessed,
                        result,
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
#[allow(clippy::too_many_arguments)]
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
                    obj,
                    var_name,
                    is_temporary,
                    &path,
                    ss,
                    scs,
                    preprocessed,
                    result,
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
                    inner,
                    var_name,
                    is_temporary,
                    &path,
                    ss,
                    scs,
                    preprocessed,
                    result,
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
#[allow(clippy::too_many_arguments)]
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
                &pe.expression,
                var_name,
                is_temporary,
                prefix,
                parent_segments,
                parent_construct_spans,
                preprocessed,
                result,
            );
        }
        Expression::ObjectExpression(obj) => {
            decompose_object_expr_for_set(
                obj,
                var_name,
                is_temporary,
                prefix,
                parent_segments,
                parent_construct_spans,
                preprocessed,
                result,
            );
        }
        Expression::ArrayExpression(arr) => {
            decompose_array_expr_for_set(
                arr,
                var_name,
                is_temporary,
                prefix,
                parent_segments,
                parent_construct_spans,
                preprocessed,
                result,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use crate::plugin::{FormatPluginMut, SemanticTokenType};
    use crate::sugarcube::SugarCubePlugin;
    use url::Url;

    /// Helper: parse a source and return all semantic tokens of the given type.
    fn tokens_of_type(src: &str, token_type: SemanticTokenType) -> Vec<(usize, usize, String)> {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.tw").unwrap();
        let result = plugin.parse_mut(&uri, src);
        let mut found = Vec::new();
        for group in &result.token_groups {
            for token in &group.tokens {
                if token.token_type == token_type {
                    let start = (token.start + group.passage_offset).min(src.len());
                    let end = (start + token.length).min(src.len());
                    let text = src[start..end].to_string();
                    found.push((start, token.length, text));
                }
            }
        }
        found
    }

    #[test]
    fn scalar_set_emits_variable_token() {
        // `<<set $playerName to "Alex">>` — scalar RHS (string).
        // Before the fix, this produced NO variable token because oxc
        // only saw `"Alex"` (not the full assignment).
        let src = ":: Start\n<<set $playerName to \"Alex\">>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            !var_tokens.is_empty(),
            "scalar <<set>> should emit a Variable token for the target, got: {:?}",
            var_tokens
        );
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$playerName"),
            "Variable token should cover $playerName, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn number_set_emits_variable_token() {
        // `<<set $playerLevel to 1>>` — scalar RHS (number).
        let src = ":: Start\n<<set $playerLevel to 1>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$playerLevel"),
            "Variable token should cover $playerLevel, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn empty_array_set_emits_variable_token() {
        // `<<set $visitedRooms to []>>` — empty array RHS.
        // Before the fix, decompose_block_literal_for_set returned an
        // empty vec (no leaf writes), so no token was emitted.
        let src = ":: Start\n<<set $visitedRooms to []>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens
                .iter()
                .any(|(_, _, text)| text == "$visitedRooms"),
            "empty array <<set>> should emit a Variable token for the target, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn empty_object_set_emits_variable_token() {
        // `<<set $empty to {}>>` — empty object RHS.
        let src = ":: Start\n<<set $empty to {}>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$empty"),
            "empty object <<set>> should emit a Variable token, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn nonempty_array_set_emits_variable_token() {
        // `<<set $inventory to ["Lantern", "Rope"]>>` — non-empty array.
        // This already worked via leaf write decomposition, but verify
        // the fix didn't break it.
        let src = ":: Start\n<<set $inventory to [\"Lantern\", \"Rope\"]>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$inventory"),
            "non-empty array <<set>> should emit a Variable token, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn nonempty_object_set_emits_variable_token() {
        // `<<set $stats to { strength: 10 }>>` — non-empty object.
        let src = ":: Start\n<<set $stats to { strength: 10 }>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$stats"),
            "non-empty object <<set>> should emit a Variable token, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn new_expression_set_emits_variable_token() {
        // `<<set $npcs to new Map([...])>>` — NewExpression RHS.
        // This falls to the scalar branch (not `{` or `[`), so should
        // emit a direct Write token.
        let src = ":: Start\n<<set $npcs to new Map([[\"bard\", { name: \"Lila\" }]])>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$npcs"),
            "new-expression <<set>> should emit a Variable token, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn temp_var_set_emits_variable_token() {
        // `<<set _count to 0>>` — temporary variable, scalar RHS.
        let src = ":: Start\n<<set _count to 0>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "_count"),
            "temp var <<set>> should emit a Variable token, got: {:?}",
            var_tokens
        );
    }

    #[test]
    fn all_storyinit_vars_get_tokens() {
        // Reproduces the exact inconsistency from the testbed's StoryInit:
        // all of these should get Variable tokens, not just the block ones.
        let src = "\
:: StoryInit
<<set $playerName to \"Alex\">>
<<set $playerLevel to 1>>
<<set $playerHP to 100>>
<<set $playerMaxHP to 100>>
<<set $playerGold to 50>>
<<set $inventory to [\"Lantern\", \"Rope\"]>>
<<set $stats to { strength: 10, dexterity: 12 }>>
<<set $flags to { metKing: false, hasKey: false }>>
<<set $visitedRooms to []>>
<<set $questLog to []>>
";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        let names: Vec<&str> = var_tokens.iter().map(|(_, _, t)| t.as_str()).collect();
        for expected in &[
            "$playerName",
            "$playerLevel",
            "$playerHP",
            "$playerMaxHP",
            "$playerGold",
            "$inventory",
            "$stats",
            "$flags",
            "$visitedRooms",
            "$questLog",
        ] {
            assert!(
                names.contains(expected),
                "Variable token missing for {}: got {:?}",
                expected,
                names
            );
        }
    }

    // ── Operator token tests ────────────────────────────────────────────

    #[test]
    fn nullish_coalescing_in_print_macro_gets_operator_token() {
        // `<<= $playerClass ?? "Undecided">>` — the `??` (nullish coalescing)
        // operator should get an Operator semantic token.
        //
        // `??` is a LogicalExpression in oxc, handled by `emit_logical_operator`.
        let src = ":: Start\n<<= $playerClass ?? \"Undecided\">>\n";
        let op_tokens = tokens_of_type(src, SemanticTokenType::Operator);
        assert!(
            !op_tokens.is_empty(),
            "?? should get an Operator token, got: {:?}",
            op_tokens
        );
        // Verify the token text contains `??`
        assert!(
            op_tokens.iter().any(|(_, _, text)| text.contains("??")),
            "Operator token should contain '??', got: {:?}",
            op_tokens
        );
    }

    #[test]
    fn nullish_coalescing_in_set_macro_gets_operator_token() {
        // `<<set _counter to (_counter ?? 0) + 1>>` — from testbed 26-misc.twee:12
        let src = ":: Start\n<<set _counter to (_counter ?? 0) + 1>>\n";
        let op_tokens = tokens_of_type(src, SemanticTokenType::Operator);
        assert!(
            op_tokens.iter().any(|(_, _, text)| text.contains("??")),
            "?? in <<set>> should get an Operator token, got: {:?}",
            op_tokens
        );
    }

    #[test]
    fn logical_and_or_get_operator_tokens() {
        // `<<if $x and $y or $z>>` — `and` and `or` are SugarCube operators
        // that get substituted to `&&` and `||`. They should get Operator tokens.
        let src = ":: Start\n<<if $x and $y or $z>><</if>>\n";
        let op_tokens = tokens_of_type(src, SemanticTokenType::Operator);
        assert!(
            op_tokens.len() >= 2,
            "and + or should produce 2+ Operator tokens, got: {:?}",
            op_tokens
        );
    }

    #[test]
    fn optional_chaining_in_set_macro_walks_operands() {
        // `<<set _mood to $npcs.get("fisherman")?.mood ?? 0>>` — from testbed
        // 50-story.twee:167. The `?.` (optional chaining) is a ChainExpression
        // in oxc. The operands ($npcs, .mood) should still get tokens.
        let src = ":: Start\n<<set _mood to $npcs.get(\"fisherman\")?.mood ?? 0>>\n";
        let var_tokens = tokens_of_type(src, SemanticTokenType::Variable);
        assert!(
            var_tokens.iter().any(|(_, _, text)| text == "$npcs"),
            "$npcs should get a Variable token even with optional chaining, got: {:?}",
            var_tokens
        );
    }

    // ── Dead-end / link extraction tests ────────────────────────────────

    #[test]
    fn return_macro_produces_link_with_empty_target() {
        // `<<return "Return to start">>` — produces a link with empty target
        // (history-based navigation). The link should be in passage.links
        // so the passage is NOT flagged as a dead end.
        //
        // Before the fix, the link was filtered out by `.filter(|l| !l.target.is_empty())`
        // in passage_build.rs, causing false dead-end diagnostics.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = ":: SomePassage\nYou are here.\n<<return \"Go back\">>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        // Find the passage and check its links
        let passage = result
            .passages
            .iter()
            .find(|p| p.name == "SomePassage")
            .expect("SomePassage should exist");
        assert!(
            !passage.links.is_empty(),
            "Passage with <<return>> should have at least 1 link (for dead-end detection), got 0 links"
        );
        // The link should have an empty target (dynamic navigation)
        assert!(
            passage.links.iter().any(|l| l.target.is_empty()),
            "<<return>> link should have empty target (history-based nav), got: {:?}",
            passage.links.iter().map(|l| &l.target).collect::<Vec<_>>()
        );
    }

    #[test]
    fn back_macro_produces_link_with_empty_target() {
        // Same as return — <<back "Display">> produces a link with empty target.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = ":: SomePassage\n<<back \"Go back\">>\n";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        let passage = result
            .passages
            .iter()
            .find(|p| p.name == "SomePassage")
            .expect("SomePassage should exist");
        assert!(
            !passage.links.is_empty(),
            "Passage with <<back>> should have at least 1 link, got 0"
        );
    }

    #[test]
    fn passage_with_only_return_is_not_dead_end() {
        // Integration test: a passage whose only outgoing navigation is
        // <<return>> should NOT be flagged as a dead end. The <<return>> link
        // (with empty target) is kept in passage.links, so dead-end detection
        // sees it as having outgoing navigation.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = "\
:: Start
You begin your adventure.
[[Go north|North]]

:: North
You are in the north.
<<return \"Go back south\">>
";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        // The "North" passage has <<return>> as its only outgoing navigation.
        // It should NOT produce a DeadEndPassage diagnostic.
        let dead_end_diags: Vec<_> = result
            .diagnostic_groups
            .iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "dead-end" || d.message.contains("dead end"))
            .collect();
        assert!(
            dead_end_diags.is_empty(),
            "Passage with <<return>> should NOT be flagged as dead end, got: {:?}",
            dead_end_diags
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn passage_with_only_zero_arg_return_is_not_dead_end() {
        // J3 regression: <<return>> with NO args (just <<return>>) must
        // still count as outgoing navigation. Without the J3 fix, zero-arg
        // <<return>> produced no link entry, causing the passage to be
        // flagged as a dead-end.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = "\
:: Start
You begin your adventure.
[[Go north|North]]

:: North
You are in the north.
<<return>>
";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        let dead_end_diags: Vec<_> = result
            .diagnostic_groups
            .iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "dead-end" || d.message.contains("dead end"))
            .collect();
        assert!(
            dead_end_diags.is_empty(),
            "Passage with zero-arg <<return>> should NOT be flagged as dead end, got: {:?}",
            dead_end_diags
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn link_single_arg_does_not_create_macro_arg_ref() {
        // J1/J2 regression: <<link "Forest">> (single arg) should NOT
        // create a MacroArgRef with target="Forest". The single arg is
        // a click handler label, not a passage target. Only
        // <<link "Display" "Passage">> (two args) creates a PassageRef.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = "\
:: Start
<<link \"Forest\">>Click<</link>>
";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        // Check that no passage has a macro_arg_ref with target "Forest"
        for passage in &result.passages {
            for arg_ref in &passage.macro_arg_refs {
                assert_ne!(
                    arg_ref.target, "Forest",
                    "Single-arg <<link \"Forest\">> should NOT create a MacroArgRef with target \"Forest\". \
                     The arg is a click handler label, not a passage target. Got: {:?}",
                    arg_ref
                );
            }
        }
    }

    #[test]
    fn return_label_arg_does_not_create_macro_arg_ref() {
        // J2 regression: <<return "Return to start">> should NOT create
        // a MacroArgRef with target="Return to start". The arg is display
        // text, not a passage target.
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        let src = "\
:: Start
<<return \"Return to start\">>
";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        for passage in &result.passages {
            for arg_ref in &passage.macro_arg_refs {
                assert_ne!(
                    arg_ref.target, "Return to start",
                    "<<return \"Return to start\">> should NOT create a MacroArgRef with the label as target. \
                     Got: {:?}",
                    arg_ref
                );
            }
        }
    }

    #[test]
    fn link_single_arg_does_not_produce_broken_link_diagnostic() {
        // Regression: <<link "Forest">> (single arg) is a click handler,
        // NOT passage navigation. It must NOT produce a "Link target
        // 'Forest' not found" broken-link diagnostic, even if no passage
        // named "Forest" exists.
        //
        // Per SugarCube docs: https://www.motoslave.net/sugarcube/2/docs/#macros-macro-link
        //   "May be called with either the link text and passage name as
        //    separate arguments, a link markup, or an image markup."
        //   - 1 arg = link text only (click handler with body)
        //   - 2 args = link text + passage name (navigation)
        use crate::plugin::FormatPluginMut;
        use crate::sugarcube::SugarCubePlugin;

        let mut plugin = SugarCubePlugin::new();
        // Note: NO passage named "Forest" exists in this story.
        let src = "\
:: Start
<<link \"Forest\">><<set $x to 1>><</link>>
";
        let result = plugin.parse_mut(&url::Url::parse("file:///test.tw").unwrap(), src);

        let broken_diags: Vec<_> = result
            .diagnostic_groups
            .iter()
            .flat_map(|g| g.diagnostics.iter())
            .filter(|d| d.code == "broken-link" || d.message.contains("not found"))
            .collect();
        assert!(
            broken_diags.is_empty(),
            "Single-arg <<link \"Forest\">> should NOT produce broken-link diagnostics. \
             Got: {:?}",
            broken_diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}
