//! Oxc AST walker for SugarCube JS analysis.
//!
//! This module walks parsed JavaScript ASTs to extract SugarCube-specific
//! information into [`JsAnalysis`] results that are attached to AST nodes:
//!
//! - `State.variables.x = value` → variable write
//! - `SugarCube.State.variables.x` → variable read
//! - `$var` / `_var` (after preprocessing) → variable read/write
//! - `Macro.add("name", {...})` → custom macro definition
//! - `Template.add("name", ...)` → template definition
//! - `function name()` → function definition
//!
//! ## New API (unified AST refactoring)
//!
//! The walker now **returns** `JsAnalysis` instead of mutating a registry.
//! The returned `JsAnalysis` is attached to the AST node during Phase 2
//! (JS annotation pass). Phase 3 (registry population) then walks the
//! unified AST and records from `JsAnalysis`.
//!
//! ## Preprocessing contract
//!
//! The JS source passed to oxc MUST be preprocessed by `js_preprocess`
//! first, so that `$var` references are replaced with `State_variables_varName`.

use oxc_ast::ast::Program;
use oxc_span::GetSpan;

use crate::sugarcube::ast::{
    AnalyzedVarOp, CommentKind, CommentSpan, FunctionCallInfo, FunctionDefInfo, JsAnalysis, KeywordSpan, LiteralKind, LiteralSpan, MacroAddInfo,
    NamespaceSpan, OperatorKind, OperatorSpan, PropertySpan, TemplateAddInfo,
};
use crate::sugarcube::js::js_annotate::compute_target_segment_spans;
use crate::sugarcube::registries::variable_tree::VarAccessKind;

// ---------------------------------------------------------------------------
// Script passage walker
// ---------------------------------------------------------------------------

/// Walk a script passage's JS AST and produce a `JsAnalysis`.
///
/// This is the main entry point for script passages (`[script]` tagged).
/// These passages contain full JS programs that can define macros, set
/// state variables, declare functions, and register templates.
///
/// Spans from oxc are mapped back to original body-relative coordinates
/// via `preprocessed.map_to_original()`.
pub fn walk_script_passage(
    program: &Program<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> JsAnalysis {
    let mut analysis = JsAnalysis::default();

    // Walk all top-level statements
    for stmt in &program.body {
        walk_statement(stmt, preprocessed, &mut analysis);
    }

    // Extract literal and operator spans from the substitution table and oxc AST.
    extract_substitution_operators(preprocessed, &mut analysis);

    // Scan for comments (/* */ and //) in the raw preprocessed source.
    // oxc strips comments from the AST, so we find them by source scanning.
    extract_comments(preprocessed, &mut analysis);

    analysis
}

// ---------------------------------------------------------------------------
// Inline JS snippet walker
// ---------------------------------------------------------------------------

/// Walk an inline JS snippet's AST and produce a `JsAnalysis`.
///
/// This is used for `<<set>>`, `<<run>>`, `<<script>>` blocks within
/// normal passages. The JS is typically an expression or a few statements.
///
/// Unlike the old implementation which only scanned the preprocessor
/// substitution table, this reuses the same full AST walking logic as
/// `walk_script_passage()` — so it detects:
/// - `State.variables.x = value` → variable writes
/// - `State.variables.x` (read) → variable reads
/// - `State_temporary_x` → temporary variable reads/writes
/// - `Macro.add()` calls, `Template.add()`, function declarations, etc.
pub fn walk_inline_js(
    program: &Program<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> JsAnalysis {
    let mut analysis = JsAnalysis::default();

    // Walk the AST using the same logic as script passages.
    // For expression-mode snippets, oxc wraps in `(expr)`, producing a
    // single ExpressionStatement in program.body — walk_statement handles that.
    for stmt in &program.body {
        walk_statement(stmt, preprocessed, &mut analysis);
    }

    // Extract literal and operator spans from the substitution table and oxc AST.
    extract_substitution_operators(preprocessed, &mut analysis);

    // Scan for comments (/* */ and //) in the raw preprocessed source.
    // oxc strips comments from the AST, so we find them by source scanning.
    extract_comments(preprocessed, &mut analysis);

    analysis
}

// ---------------------------------------------------------------------------
// Internal: statement-level walk
// ---------------------------------------------------------------------------

fn walk_statement(
    stmt: &oxc_ast::ast::Statement<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::Statement;

    match stmt {
        Statement::FunctionDeclaration(func) => {
            push_keyword(analysis, "function", func.span.start as usize, preprocessed);
            if let Some(id) = &func.id {
                let (name, name_offset) = demangle_function_name(
                    id.name.as_str(),
                    id.span.start as usize,
                    preprocessed,
                );
                let param_count = Some(func.params.items.len());
                analysis.function_defs.push(FunctionDefInfo {
                    name,
                    name_offset,
                    param_count,
                });
            }
            // Emit Variable+Definition tokens for function parameters
            emit_param_tokens(&func.params, preprocessed, analysis);
            // Recurse into the function body — Macro.add() / Template.add() /
            // nested function declarations inside a function body must be
            // discovered. Without this, macros registered inside a wrapper
            // function (e.g., `function registerMacros() { Macro.add(...) }`)
            // would be invisible to completion, hover, and goto-definition.
            walk_function_body(&func.body, preprocessed, analysis);
        }
        Statement::ExpressionStatement(expr_stmt) => {
            walk_expression(&expr_stmt.expression, preprocessed, analysis);
        }
        Statement::VariableDeclaration(var_decl) => {
            let kw = match var_decl.kind {
                oxc_ast::ast::VariableDeclarationKind::Var => "var",
                oxc_ast::ast::VariableDeclarationKind::Let => "let",
                oxc_ast::ast::VariableDeclarationKind::Const => "const",
                oxc_ast::ast::VariableDeclarationKind::Using => "using",
                oxc_ast::ast::VariableDeclarationKind::AwaitUsing => "using",
            };
            push_keyword(analysis, kw, var_decl.span.start as usize, preprocessed);
            for decl in &var_decl.declarations {
                // Emit a Variable+Definition span for the binding name (unless
                // it's a substituted SugarCube variable — those are handled by
                // the $var / _var mechanism).
                if let oxc_ast::ast::BindingPattern::BindingIdentifier(binding_name) = &decl.id {
                    let name = binding_name.name.as_str();
                    if !name.starts_with("State_variables_") && !name.starts_with("State_temporary_") {
                        let span = preprocessed.map_range_to_original(
                            binding_name.span.start as usize..binding_name.span.end as usize
                        );
                        analysis.js_var_def_spans.push(span);
                    }
                }
                // Track named function expressions and arrow functions
                if let Some(init) = &decl.init {
                    match init {
                        oxc_ast::ast::Expression::FunctionExpression(func_expr) => {
                            // var myFunc = function() {...} — name from the var binding
                            if let oxc_ast::ast::BindingPattern::BindingIdentifier(binding_name) = &decl.id {
                                let (name, name_offset) = demangle_function_name(
                                    binding_name.name.as_str(),
                                    binding_name.span.start as usize,
                                    preprocessed,
                                );
                                let param_count = Some(func_expr.params.items.len());
                                analysis.function_defs.push(FunctionDefInfo {
                                    name,
                                    name_offset,
                                    param_count,
                                });
                            }
                        }
                        oxc_ast::ast::Expression::ArrowFunctionExpression(_arrow) => {
                            if let oxc_ast::ast::BindingPattern::BindingIdentifier(binding_name) = &decl.id {
                                let (name, name_offset) = demangle_function_name(
                                    binding_name.name.as_str(),
                                    binding_name.span.start as usize,
                                    preprocessed,
                                );
                                analysis.function_defs.push(FunctionDefInfo {
                                    name,
                                    name_offset,
                                    param_count: None,
                                });
                            }
                        }
                        _ => {}
                    }
                    walk_expression(init, preprocessed, analysis);
                }
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                walk_statement(s, preprocessed, analysis);
            }
        }
        Statement::IfStatement(if_stmt) => {
            push_keyword(analysis, "if", if_stmt.span.start as usize, preprocessed);
            walk_expression(&if_stmt.test, preprocessed, analysis);
            walk_statement(&if_stmt.consequent, preprocessed, analysis);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement(alt, preprocessed, analysis);
            }
        }
        Statement::ForStatement(for_stmt) => {
            push_keyword(analysis, "for", for_stmt.span.start as usize, preprocessed);
            walk_statement(&for_stmt.body, preprocessed, analysis);
        }
        Statement::WhileStatement(while_stmt) => {
            push_keyword(analysis, "while", while_stmt.span.start as usize, preprocessed);
            walk_statement(&while_stmt.body, preprocessed, analysis);
        }
        Statement::ReturnStatement(ret) => {
            push_keyword(analysis, "return", ret.span.start as usize, preprocessed);
            if let Some(arg) = &ret.argument {
                walk_expression(arg, preprocessed, analysis);
            }
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                walk_statement(s, preprocessed, analysis);
            }
            if let Some(catch) = &try_stmt.handler {
                for s in &catch.body.body {
                    walk_statement(s, preprocessed, analysis);
                }
            }
            if let Some(finally) = &try_stmt.finalizer {
                for s in &finally.body {
                    walk_statement(s, preprocessed, analysis);
                }
            }
        }
        _ => {}
    }
}

/// Push a JS keyword span onto `analysis.keyword_spans`, mapping the start
/// position through the preprocessor's substitution table.
fn push_keyword(analysis: &mut JsAnalysis, kw: &'static str, start_offset: usize, preprocessed: &super::js_preprocess::PreprocessedJs) {
    let mapped_start = preprocessed.map_to_original(start_offset);
    let mapped_end = preprocessed.map_to_original(start_offset + kw.len());
    analysis.keyword_spans.push(KeywordSpan {
        text: kw,
        span: mapped_start..mapped_end,
    });
}

/// Emit Variable+Definition spans for function parameters.
fn emit_param_tokens(
    params: &oxc_ast::ast::FormalParameters<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    for item in &params.items {
        if let oxc_ast::ast::BindingPattern::BindingIdentifier(binding_name) = &item.pattern {
            let name = binding_name.name.as_str();
            if !name.starts_with("State_variables_") && !name.starts_with("State_temporary_") {
                let span = preprocessed.map_range_to_original(
                    binding_name.span.start as usize..binding_name.span.end as usize
                );
                analysis.js_var_def_spans.push(span);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: expression-level walk
// ---------------------------------------------------------------------------

fn walk_expression(
    expr: &oxc_ast::ast::Expression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::Expression as Expr;

    match expr {
        Expr::StaticMemberExpression(member) => {
            // Check for State.variables.x READ pattern
            if member.property.name != "variables"
                && let Expr::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let Expr::Identifier(id) = &inner.object
                && id.name == "State"
            {
                let prop_name = member.property.name.as_str();
                let var_name = format!("${}", prop_name);
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                let span = original_start..original_end;

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: span.clone(),
                    property_path: String::new(),
                    segment_spans: vec![span],
                    construct_span: None,
                            segment_construct_spans: Vec::new(),
                });
            }
            // Check for SugarCube.State.variables.x READ pattern
            if member.property.name != "variables"
                && let Expr::StaticMemberExpression(state_access) = &member.object
                && state_access.property.name == "variables"
                && let Expr::StaticMemberExpression(sc_state) = &state_access.object
                && sc_state.property.name == "State"
                && let Expr::Identifier(id) = &sc_state.object
                && id.name == "SugarCube"
            {
                let prop_name = member.property.name.as_str();
                let var_name = format!("${}", prop_name);
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                let span = original_start..original_end;

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: span.clone(),
                    property_path: String::new(),
                    segment_spans: vec![span],
                    construct_span: None,
                            segment_construct_spans: Vec::new(),
                });
            }
            // Check for State.variables pattern (intermediate — just recurse)
            if member.property.name == "variables"
                && let Expr::Identifier(id) = &member.object
                && id.name == "State"
            {
                // This is State.variables — the parent expression
                // will have the property access. We detect that at the
                // assignment/call level instead.
            }
            // Check for Macro.add pattern
            if member.property.name == "add"
                && let Expr::Identifier(id) = &member.object
                && id.name == "Macro"
            {
                // Found Macro.add — the parent call expression handles this
            }
            // Check for Template.add pattern
            if member.property.name == "add"
                && let Expr::Identifier(id) = &member.object
                && id.name == "Template"
            {
                // Found Template.add — the parent call expression handles this
            }
            // ── Namespace detection for SugarCube global objects ───────
            // After the specific pattern checks above, check if the object
            // of this member expression is a known SugarCube global. This
            // covers patterns like Engine.play(), Story.has(), Config.debug,
            // State.turns, State.passage, etc.
            //
            // IMPORTANT: We skip `State.variables.x` and `SugarCube.State.variables.x`
            // patterns here because those are already handled by var_ops above.
            // We also skip `Macro.add` and `Template.add` since those are handled
            // by the call expression handler.
            emit_namespace_for_member_expr(member, preprocessed, analysis);
            // If the object is NOT a known SugarCube global, emit a plain
            // JS property span for the property name.
            let object_is_sc_global = if let Expr::Identifier(id) = &member.object {
                is_known_global(id.name.as_str())
            } else {
                false
            };
            if !object_is_sc_global {
                let prop_span = preprocessed.map_range_to_original(
                    member.property.span.start as usize..member.property.span.end as usize
                );
                analysis.js_property_spans.push(prop_span);
            }
            // Recurse into object
            walk_expression(&member.object, preprocessed, analysis);
        }
        Expr::ComputedMemberExpression(member) => {
            // Check for State.variables["x"] READ pattern
            if let Expr::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let Expr::Identifier(id) = &inner.object
                && id.name == "State"
                && let Expr::StringLiteral(str_lit) = &member.expression
            {
                let prop_name = str_lit.value.as_str();
                let var_name = format!("${}", prop_name);
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                let span = original_start..original_end;

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: span.clone(),
                    property_path: String::new(),
                    segment_spans: vec![span],
                    construct_span: None,
                            segment_construct_spans: Vec::new(),
                });
            }
            walk_expression(&member.object, preprocessed, analysis);
            walk_expression(&member.expression, preprocessed, analysis);
        }
        Expr::CallExpression(call) => {
            // ── Detect preprocessed identifiers used as function call targets ──
            // When a SugarCube variable like `_myHelper` is used as a function
            // call target (`_myHelper()`), the preprocessor has already replaced
            // it with `State_temporary_myHelper`. We need to emit a FunctionCallInfo
            // instead of letting the recursive walk create a Variable var_op.
            if let Expr::Identifier(id) = &call.callee {
                if let Some(call_info) = try_classify_as_function_call(id, preprocessed, analysis) {
                    analysis.function_calls.push(call_info);
                    // Skip the normal recursive walk into the callee — we've
                    // already handled it as a function call. Just walk args.
                    for arg in &call.arguments {
                        walk_argument(arg, preprocessed, analysis);
                    }
                    return;
                }
            }
            // Check for Macro.add("name", ...) pattern
            if let Expr::StaticMemberExpression(member) = &call.callee
                && member.property.name == "add"
                && let Expr::Identifier(id) = &member.object
                && (id.name == "Macro" || id.name == "SugarCube")
                && let Some(arg) = call.arguments.first()
                && let Some(name) = extract_string_from_arg(arg)
            {
                let name_offset = preprocessed.map_to_original(arg.span().start as usize);
                // Inspect the config object (second argument) for the `tags`
                // field. SugarCube uses `tags` to signal body requirement:
                //   - `tags` omitted → inline (Never)
                //   - `tags: null` → container (Required)
                //   - `tags: ["a","b"]` → container with sub-tags (Required)
                let body = extract_body_requirement_from_macro_add_config(&call.arguments);
                analysis.macro_adds.push(MacroAddInfo {
                    name,
                    name_offset,
                    body,
                });
            }
            // Also handle SugarCube.Macro.add
            if let Expr::StaticMemberExpression(member) = &call.callee
                && member.property.name == "add"
                && let Expr::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "Macro"
                && let Expr::Identifier(id) = &inner.object
                && id.name == "SugarCube"
                && let Some(arg) = call.arguments.first()
                && let Some(name) = extract_string_from_arg(arg)
            {
                let name_offset = preprocessed.map_to_original(arg.span().start as usize);
                let body = extract_body_requirement_from_macro_add_config(&call.arguments);
                analysis.macro_adds.push(MacroAddInfo {
                    name,
                    name_offset,
                    body,
                });
            }
            // Check for Template.add("name", ...) pattern
            if let Expr::StaticMemberExpression(member) = &call.callee
                && member.property.name == "add"
                && let Expr::Identifier(id) = &member.object
                && id.name == "Template"
                && let Some(arg) = call.arguments.first()
                && let Some(name) = extract_string_from_arg(arg)
            {
                let name_offset = preprocessed.map_to_original(arg.span().start as usize);
                let is_string = if call.arguments.len() > 1 {
                    matches!(&call.arguments[1], oxc_ast::ast::Argument::StringLiteral(_))
                } else {
                    false
                };
                analysis.template_adds.push(TemplateAddInfo {
                    name,
                    name_offset,
                    is_string,
                });
            }
            // Also handle SugarCube.Template.add
            if let Expr::StaticMemberExpression(member) = &call.callee
                && member.property.name == "add"
                && let Expr::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "Template"
                && let Expr::Identifier(id) = &inner.object
                && id.name == "SugarCube"
                && let Some(arg) = call.arguments.first()
                && let Some(name) = extract_string_from_arg(arg)
            {
                let name_offset = preprocessed.map_to_original(arg.span().start as usize);
                let is_string = if call.arguments.len() > 1 {
                    matches!(&call.arguments[1], oxc_ast::ast::Argument::StringLiteral(_))
                } else {
                    false
                };
                analysis.template_adds.push(TemplateAddInfo {
                    name,
                    name_offset,
                    is_string,
                });
            }
            // ── Detect method calls: expr.method(...) ──────────────────
            if let Expr::StaticMemberExpression(member) = &call.callee {
                let prop_span = preprocessed.map_range_to_original(
                    member.property.span.start as usize..member.property.span.end as usize
                );
                analysis.js_method_spans.push(prop_span);
                walk_expression(&member.object, preprocessed, analysis);
            } else {
                walk_expression(&call.callee, preprocessed, analysis);
            }
            for arg in &call.arguments {
                walk_argument(arg, preprocessed, analysis);
            }
        }
        Expr::AssignmentExpression(assign) => {
            // Emit operator span for the assignment operator.
            emit_assignment_operator(&assign, preprocessed, analysis);
            // Check for variable writes on the assignment target.
            // `check_assignment_for_var_writes` handles both block-literal
            // RHS (decomposes into leaf writes) and scalar RHS (single
            // direct write on the target).
            check_assignment_for_var_writes(&assign.left, &assign.right, preprocessed, analysis);
            walk_expression(&assign.right, preprocessed, analysis);
        }
        Expr::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, analysis);
            let name = id.name.as_str();
            if !name.starts_with("State_variables_") && !name.starts_with("State_temporary_") {
                let span = preprocessed.map_range_to_original(
                    id.span.start as usize..id.span.end as usize
                );
                if is_js_global(name) {
                    analysis.js_global_spans.push(span);
                } else if is_sugarcube_global(name) {
                    analysis.namespace_spans.push(NamespaceSpan {
                        name: name.to_string(),
                        span,
                        property_spans: Vec::new(),
                    });
                } else {
                    analysis.js_var_spans.push(span);
                }
            }
        }
        Expr::BinaryExpression(bin) => {
            // Emit operator span for the binary operator.
            emit_binary_operator(&bin, preprocessed, analysis);
            // For keyword binary operators (in, instanceof), also emit a Keyword span.
            let kw = match bin.operator {
                oxc_ast::ast::BinaryOperator::In => Some("in"),
                oxc_ast::ast::BinaryOperator::Instanceof => Some("instanceof"),
                _ => None,
            };
            if let Some(kw) = kw {
                let gap_start = bin.left.span().end as usize;
                let gap_end = bin.right.span().start as usize;
                if let Some(kw_pos) = preprocessed.source[gap_start..gap_end].find(kw) {
                    let abs_pos = gap_start + kw_pos;
                    push_keyword(analysis, kw, abs_pos, preprocessed);
                }
            }
            walk_expression(&bin.left, preprocessed, analysis);
            walk_expression(&bin.right, preprocessed, analysis);
        }
        Expr::LogicalExpression(logic) => {
            // Emit operator span for the logical operator.
            emit_logical_operator(&logic, preprocessed, analysis);
            walk_expression(&logic.left, preprocessed, analysis);
            walk_expression(&logic.right, preprocessed, analysis);
        }
        Expr::SequenceExpression(seq) => {
            for e in &seq.expressions {
                walk_expression(e, preprocessed, analysis);
            }
        }
        Expr::ConditionalExpression(cond) => {
            walk_expression(&cond.test, preprocessed, analysis);
            walk_expression(&cond.consequent, preprocessed, analysis);
            walk_expression(&cond.alternate, preprocessed, analysis);
        }
        Expr::ParenthesizedExpression(pe) => {
            walk_expression(&pe.expression, preprocessed, analysis);
        }
        Expr::UnaryExpression(unary) => {
            // Emit operator span for unary operators (!, -, +, typeof, etc.)
            emit_unary_operator(&unary, preprocessed, analysis);
            let kw = match unary.operator {
                oxc_ast::ast::UnaryOperator::Typeof => Some("typeof"),
                oxc_ast::ast::UnaryOperator::Void => Some("void"),
                oxc_ast::ast::UnaryOperator::Delete => Some("delete"),
                _ => None,
            };
            if let Some(kw) = kw {
                push_keyword(analysis, kw, unary.span.start as usize, preprocessed);
            }
            walk_expression(&unary.argument, preprocessed, analysis);
        }
        Expr::UpdateExpression(update) => {
            // Emit operator span for update operators (++, --)
            emit_update_operator(&update, preprocessed, analysis);
            // UpdateExpression.argument is a SimpleAssignmentTarget, not an Expression.
            // Walk it for any variable references inside.
            walk_assignment_target_like(&update.argument, preprocessed, analysis);
        }
        Expr::ObjectExpression(obj) => {
            for prop in &obj.properties {
                if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop {
                    walk_expression(&p.value, preprocessed, analysis);
                }
            }
        }
        Expr::ArrayExpression(arr) => {
            use oxc_ast::ast::ArrayExpressionElement;
            for elem in &arr.elements {
                match elem {
                    ArrayExpressionElement::SpreadElement(spread) => { walk_expression(&spread.argument, preprocessed, analysis); }
                    ArrayExpressionElement::Elision(_) => {}
                    ArrayExpressionElement::NumericLiteral(n) => { let s = preprocessed.map_range_to_original(n.span.start as usize..n.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Number, span: s }); }
                    ArrayExpressionElement::StringLiteral(s) => { let sp = preprocessed.map_range_to_original(s.span.start as usize..s.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::String, span: sp }); }
                    ArrayExpressionElement::BooleanLiteral(b) => { let s = preprocessed.map_range_to_original(b.span.start as usize..b.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Boolean, span: s }); }
                    ArrayExpressionElement::NullLiteral(n) => { let s = preprocessed.map_range_to_original(n.span.start as usize..n.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Null, span: s }); }
                    ArrayExpressionElement::Identifier(id) => { check_identifier_for_substituted_var(id, false, preprocessed, analysis); }
                    ArrayExpressionElement::ObjectExpression(obj) => { for prop in &obj.properties { if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop { walk_expression(&p.value, preprocessed, analysis); } } }
                    ArrayExpressionElement::ArrayExpression(inner) => {
                        for e in &inner.elements {
                            if let ArrayExpressionElement::NumericLiteral(n) = e { let s = preprocessed.map_range_to_original(n.span.start as usize..n.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Number, span: s }); }
                            else if let ArrayExpressionElement::StringLiteral(s) = e { let sp = preprocessed.map_range_to_original(s.span.start as usize..s.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::String, span: sp }); }
                            else if let ArrayExpressionElement::BooleanLiteral(b) = e { let s = preprocessed.map_range_to_original(b.span.start as usize..b.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Boolean, span: s }); }
                            else if let ArrayExpressionElement::NullLiteral(n) = e { let s = preprocessed.map_range_to_original(n.span.start as usize..n.span.end as usize); analysis.literal_spans.push(LiteralSpan { kind: LiteralKind::Null, span: s }); }
                            else if let ArrayExpressionElement::Identifier(id) = e { check_identifier_for_substituted_var(id, false, preprocessed, analysis); }
                            else if let ArrayExpressionElement::ObjectExpression(obj) = e { for prop in &obj.properties { if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop { walk_expression(&p.value, preprocessed, analysis); } } }
                        }
                    }
                    ArrayExpressionElement::BinaryExpression(bin) => { walk_expression(&bin.left, preprocessed, analysis); walk_expression(&bin.right, preprocessed, analysis); }
                    ArrayExpressionElement::LogicalExpression(l) => { walk_expression(&l.left, preprocessed, analysis); walk_expression(&l.right, preprocessed, analysis); }
                    ArrayExpressionElement::CallExpression(c) => { walk_expression(&c.callee, preprocessed, analysis); for a in &c.arguments { walk_argument(a, preprocessed, analysis); } }
                    ArrayExpressionElement::ParenthesizedExpression(pe) => { walk_expression(&pe.expression, preprocessed, analysis); }
                    _ => {}
                }
            }
        }
        Expr::NewExpression(new_expr) => {
            push_keyword(analysis, "new", new_expr.span.start as usize, preprocessed);
            walk_expression(&new_expr.callee, preprocessed, analysis);
            for arg in &new_expr.arguments {
                walk_argument(arg, preprocessed, analysis);
            }
        }
        // ── Literal expressions: emit literal spans ──────────────────
        Expr::StringLiteral(str_lit) => {
            let span = preprocessed.map_range_to_original(
                str_lit.span.start as usize..str_lit.span.end as usize
            );
            analysis.literal_spans.push(LiteralSpan {
                kind: LiteralKind::String,
                span,
            });
        }
        Expr::NumericLiteral(num_lit) => {
            let span = preprocessed.map_range_to_original(
                num_lit.span.start as usize..num_lit.span.end as usize
            );
            analysis.literal_spans.push(LiteralSpan {
                kind: LiteralKind::Number,
                span,
            });
        }
        Expr::BooleanLiteral(bool_lit) => {
            let span = preprocessed.map_range_to_original(
                bool_lit.span.start as usize..bool_lit.span.end as usize
            );
            analysis.literal_spans.push(LiteralSpan {
                kind: LiteralKind::Boolean,
                span,
            });
        }
        Expr::NullLiteral(null_lit) => {
            let span = preprocessed.map_range_to_original(
                null_lit.span.start as usize..null_lit.span.end as usize
            );
            analysis.literal_spans.push(LiteralSpan {
                kind: LiteralKind::Null,
                span,
            });
        }
        Expr::RegExpLiteral(regex_lit) => {
            let span = preprocessed.map_range_to_original(
                regex_lit.span.start as usize..regex_lit.span.end as usize
            );
            analysis.regex_spans.push(span);
        }
        Expr::TemplateLiteral(tmpl) => {
            // Template literals with no expressions are effectively strings
            if tmpl.expressions.is_empty() && tmpl.quasis.len() == 1 {
                let span = preprocessed.map_range_to_original(
                    tmpl.span.start as usize..tmpl.span.end as usize
                );
                analysis.literal_spans.push(LiteralSpan {
                    kind: LiteralKind::String,
                    span,
                });
            } else {
                // Template literals with expressions: walk the expressions
                // and emit string spans for the quasi parts
                for quasi in &tmpl.quasis {
                    if !quasi.value.raw.is_empty() {
                        let span = preprocessed.map_range_to_original(
                            quasi.span.start as usize..quasi.span.end as usize
                        );
                        analysis.literal_spans.push(LiteralSpan {
                            kind: LiteralKind::String,
                            span,
                        });
                    }
                }
                for expr in &tmpl.expressions {
                    walk_expression(expr, preprocessed, analysis);
                }
            }
        }
        Expr::FunctionExpression(func_expr) => {
            push_keyword(analysis, "function", func_expr.span.start as usize, preprocessed);
            emit_param_tokens(&func_expr.params, preprocessed, analysis);
            walk_function_body(&func_expr.body, preprocessed, analysis);
        }
        Expr::ArrowFunctionExpression(arrow) => {
            emit_param_tokens(&arrow.params, preprocessed, analysis);
            walk_function_body(&arrow.body, preprocessed, analysis);
        }
        Expr::ChainExpression(chain) => {
            // Optional chaining (?.) — recurse into the inner expression.
            // The `?.` operator itself doesn't get a separate Operator token
            // (it's part of the member expression syntax), but the operands
            // (variables, properties) inside it still need to be walked.
            //
            // `chain.expression` is a `ChainElement` enum wrapping either a
            // `CallExpression` or a `MemberExpression`. We match on the
            // variants and walk the relevant sub-expressions directly
            // (object, property, arguments) rather than reconstructing an
            // `Expr` variant (which would require cloning arena-allocated
            // nodes).
            use oxc_ast::ast::ChainElement;
            match &chain.expression {
                ChainElement::CallExpression(call) => {
                    // Walk the callee and arguments (same as CallExpression handling above)
                    walk_expression(&call.callee, preprocessed, analysis);
                    for arg in &call.arguments {
                        walk_argument(arg, preprocessed, analysis);
                    }
                }
                ChainElement::StaticMemberExpression(member) => {
                    // Walk the object (same as StaticMemberExpression handling above)
                    walk_expression(&member.object, preprocessed, analysis);
                }
                ChainElement::ComputedMemberExpression(member) => {
                    // Walk the object and the expression inside []
                    walk_expression(&member.object, preprocessed, analysis);
                    walk_expression(&member.expression, preprocessed, analysis);
                }
                // TSNonNullExpression (TypeScript `expr!`) and
                // PrivateFieldExpression (`obj.#field`) are not relevant
                // for SugarCube/JS. Skip them — they're rare and would
                // require additional handling not worth the complexity.
                _ => {}
            }
        }
        Expr::AwaitExpression(await_expr) => {
            // `await expr` — emit `await` keyword, walk the argument.
            push_keyword(analysis, "await", await_expr.span.start as usize, preprocessed);
            walk_expression(&await_expr.argument, preprocessed, analysis);
        }
        Expr::YieldExpression(yield_expr) => {
            // `yield expr` — emit `yield` keyword, walk the argument if present.
            push_keyword(analysis, "yield", yield_expr.span.start as usize, preprocessed);
            if let Some(arg) = &yield_expr.argument {
                walk_expression(arg, preprocessed, analysis);
            }
        }
        _ => {}
    }
}

/// Walk the body of a function (declaration, expression, or arrow).
///
/// This is the shared recursion point for all function-body walking. It's
/// called from:
/// - `walk_statement` for `FunctionDeclaration` bodies (`Option<Box<FunctionBody>>`)
/// - `walk_expression` for `FunctionExpression` bodies (`Option<Box<FunctionBody>>`)
///   and `ArrowFunctionExpression` bodies (`Box<FunctionBody>` — arrows always
///   have a body)
/// - `walk_argument` for callback function/arrow expressions
///
/// Without this, `Macro.add()` / `Template.add()` / nested function definitions
/// inside ANY function body would be invisible — including the common pattern
/// of wrapping macro registrations in `function registerMacros() { ... }` or
/// passing callbacks like `$(document).one(':storyready', function() { ... })`.
fn walk_function_body(
    body: &impl WalkFunctionBody,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    if let Some(body) = body.as_function_body() {
        for stmt in &body.statements {
            walk_statement(stmt, preprocessed, analysis);
        }
    }
}

/// Helper trait to abstract over `Option<Box<FunctionBody>>` (function
/// declarations and function expressions) and `Box<FunctionBody>` (arrow
/// functions, which always have a body). This lets `walk_function_body` accept
/// both types without the caller having to unwrap.
trait WalkFunctionBody {
    fn as_function_body(&self) -> Option<&oxc_ast::ast::FunctionBody<'_>>;
}

impl WalkFunctionBody for Option<oxc_allocator::Box<'_, oxc_ast::ast::FunctionBody<'_>>> {
    fn as_function_body(&self) -> Option<&oxc_ast::ast::FunctionBody<'_>> {
        self.as_deref()
    }
}

impl WalkFunctionBody for oxc_allocator::Box<'_, oxc_ast::ast::FunctionBody<'_>> {
    fn as_function_body(&self) -> Option<&oxc_ast::ast::FunctionBody<'_>> {
        Some(self.as_ref())
    }
}

/// Walk a `SimpleAssignmentTarget` for variable detection.
///
/// `UpdateExpression` (like `$i++`) uses `SimpleAssignmentTarget` as its
/// argument type, not `Expression`. This function handles the common cases
/// to detect substituted variable references.
fn walk_assignment_target_like(
    target: &oxc_ast::ast::SimpleAssignmentTarget<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::SimpleAssignmentTarget;

    match target {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
            check_identifier_for_substituted_var(id, true, preprocessed, analysis);
        }
        SimpleAssignmentTarget::StaticMemberExpression(member) => {
            // Check for State.variables.x pattern directly
            if member.property.name != "variables"
                && let oxc_ast::ast::Expression::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let oxc_ast::ast::Expression::Identifier(id) = &inner.object
                && id.name == "State"
            {
                let prop_name = member.property.name.as_str();
                let var_name = format!("${}", prop_name);
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                let span = original_start..original_end;
                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Write,
                    span: span.clone(),
                    property_path: String::new(),
                    segment_spans: vec![span],
                    construct_span: None,
                            segment_construct_spans: Vec::new(),
                });
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Internal: assignment target checker (unified)
// ---------------------------------------------------------------------------

/// Check an assignment for variable writes. This is the single entry point
/// for assignment-target var_ops — handles both block-literal RHS and
/// scalar/non-block RHS in one pass.
///
/// Behavior:
/// - Extract var info (name, is_temporary, target_span) from the LHS.
/// - If RHS is ObjectExpression or ArrayExpression: decompose into leaf
///   writes (no direct write on the target — leaves propagate up).
/// - If RHS is anything else (scalar, expression): emit a single direct
///   write on the target with construct_span = full assignment span and
///   segment_construct_spans populated for propagation.
fn check_assignment_for_var_writes(
    left: &oxc_ast::ast::AssignmentTarget<'_>,
    right: &oxc_ast::ast::Expression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    let var_info = extract_var_info_from_assignment_target(left, preprocessed);
    let Some((var_name, is_temporary, target_span)) = var_info else {
        return;
    };

    // Full assignment construct span (LHS + `=` + RHS).
    let assign_span = {
        let left_span = left.span();
        let right_span = right.span();
        let start = preprocessed.map_to_original(left_span.start as usize);
        let end = preprocessed.map_to_original(right_span.end as usize);
        start..end
    };

    match right {
        oxc_ast::ast::Expression::ObjectExpression(obj) => {
            // Block literal — decompose into leaf writes. No direct write
            // on the target (leaves propagate up).
            let root_construct_spans = vec![assign_span];
            decompose_object_write(
                obj,
                &var_name,
                is_temporary,
                &target_span,
                String::new(),
                vec![target_span.clone()],
                root_construct_spans,
                preprocessed,
                analysis,
            );
        }
        oxc_ast::ast::Expression::ArrayExpression(arr) => {
            let root_construct_spans = vec![assign_span];
            decompose_array_write(
                arr,
                &var_name,
                is_temporary,
                &target_span,
                String::new(),
                vec![target_span.clone()],
                root_construct_spans,
                preprocessed,
                analysis,
            );
        }
        _ => {
            // Scalar / non-block expression — emit a single direct write
            // on the target (leaf scalar). The construct_span is the full
            // assignment span, and segment_construct_spans is populated
            // so propagation uses the same span at every depth.
            let segment_spans = vec![target_span.clone()];
            let segment_construct_spans = vec![assign_span.clone()];
            analysis.var_ops.push(AnalyzedVarOp {
                name: var_name,
                is_temporary,
                access_kind: VarAccessKind::Write,
                span: target_span,
                property_path: String::new(),
                segment_spans,
                construct_span: Some(assign_span),
                segment_construct_spans,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: block literal decomposition helpers (objects + arrays)
// ---------------------------------------------------------------------------

/// Decompose an object literal into leaf writes.
///
/// For each property:
/// - If the value is a leaf scalar (literal or non-block expression), emit
///   a direct write on this property.
/// - If the value is a nested ObjectExpression or ArrayExpression, recurse
///   (no direct write on this property — it gets writes from leaf propagation).
///
/// `prefix` is the dot-path prefix up to this point (e.g., "n1" for `$a.n1`).
/// `parent_segments` is the per-segment token spans (for nav).
/// `parent_construct_spans` is the per-segment construct spans (for propagation).
fn decompose_object_write(
    obj: &oxc_ast::ast::ObjectExpression<'_>,
    var_name: &str,
    is_temporary: bool,
    _target_span: &std::ops::Range<usize>,
    prefix: String,
    parent_segments: Vec<std::ops::Range<usize>>,
    parent_construct_spans: Vec<std::ops::Range<usize>>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
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

            let Some(key) = key_str else { continue };

            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };

            // Key token span (for nav pointing)
            let key_span = p.key.span();
            let key_start = preprocessed.map_to_original(key_span.start as usize);
            let key_end = preprocessed.map_to_original(key_span.end as usize);
            let key_range = key_start..key_end;

            // Property construct span = `key: value` (from key start to value end).
            let value_span = p.value.span();
            let value_end = preprocessed.map_to_original(value_span.end as usize);
            let prop_construct_span = key_start..value_end;

            let mut segment_spans = parent_segments.clone();
            segment_spans.push(key_range.clone());

            let mut segment_construct_spans = parent_construct_spans.clone();
            segment_construct_spans.push(prop_construct_span.clone());

            // Check if the value is a leaf scalar or a nested block.
            match &p.value {
                oxc_ast::ast::Expression::ObjectExpression(inner_obj) => {
                    // Nested object — recurse, no direct write on this property.
                    decompose_object_write(
                        inner_obj,
                        var_name,
                        is_temporary,
                        _target_span,
                        path,
                        segment_spans,
                        segment_construct_spans,
                        preprocessed,
                        analysis,
                    );
                }
                oxc_ast::ast::Expression::ArrayExpression(inner_arr) => {
                    // Nested array — recurse.
                    decompose_array_write(
                        inner_arr,
                        var_name,
                        is_temporary,
                        _target_span,
                        path,
                        segment_spans,
                        segment_construct_spans,
                        preprocessed,
                        analysis,
                    );
                }
                _ => {
                    // Leaf scalar (literal, identifier, expression) — emit direct write.
                    analysis.var_ops.push(AnalyzedVarOp {
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

/// Decompose an array literal into leaf writes.
///
/// Array elements are identified by their numeric index (0, 1, 2, ...).
/// Each element's construct span = the element's value expression span.
fn decompose_array_write(
    arr: &oxc_ast::ast::ArrayExpression<'_>,
    var_name: &str,
    is_temporary: bool,
    _target_span: &std::ops::Range<usize>,
    prefix: String,
    parent_segments: Vec<std::ops::Range<usize>>,
    parent_construct_spans: Vec<std::ops::Range<usize>>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::ArrayExpressionElement;

    for (idx, elem) in arr.elements.iter().enumerate() {
        // Match on the element type. ObjectExpression and ArrayExpression
        // are nested blocks (recurse). Everything else (literals,
        // identifiers, expressions) is a leaf scalar.
        match elem {
            ArrayExpressionElement::ObjectExpression(obj) => {
                let key = idx.to_string();
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                let obj_span = obj.span();
                let elem_start = preprocessed.map_to_original(obj_span.start as usize);
                let elem_end = preprocessed.map_to_original(obj_span.end as usize);
                let elem_range = elem_start..elem_end;

                let mut segment_spans = parent_segments.clone();
                segment_spans.push(elem_range.clone());
                let mut segment_construct_spans = parent_construct_spans.clone();
                segment_construct_spans.push(elem_range.clone());

                decompose_object_write(
                    obj, var_name, is_temporary, _target_span,
                    path, segment_spans, segment_construct_spans,
                    preprocessed, analysis,
                );
            }
            ArrayExpressionElement::ArrayExpression(inner_arr) => {
                let key = idx.to_string();
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                let arr_span = inner_arr.span();
                let elem_start = preprocessed.map_to_original(arr_span.start as usize);
                let elem_end = preprocessed.map_to_original(arr_span.end as usize);
                let elem_range = elem_start..elem_end;

                let mut segment_spans = parent_segments.clone();
                segment_spans.push(elem_range.clone());
                let mut segment_construct_spans = parent_construct_spans.clone();
                segment_construct_spans.push(elem_range.clone());

                decompose_array_write(
                    inner_arr, var_name, is_temporary, _target_span,
                    path, segment_spans, segment_construct_spans,
                    preprocessed, analysis,
                );
            }
            ArrayExpressionElement::SpreadElement(_)
            | ArrayExpressionElement::Elision(_) => {
                // Skip — spread elements and elisions don't produce writes.
                continue;
            }
            // All literal/identifier variants are leaf scalars.
            _ => {
                let elem_span = elem.span();
                let elem_start = preprocessed.map_to_original(elem_span.start as usize);
                let elem_end = preprocessed.map_to_original(elem_span.end as usize);
                let elem_range = elem_start..elem_end;

                let key = idx.to_string();
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };

                let mut segment_spans = parent_segments.clone();
                segment_spans.push(elem_range.clone());
                let mut segment_construct_spans = parent_construct_spans.clone();
                segment_construct_spans.push(elem_range.clone());

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name.to_string(),
                    is_temporary,
                    access_kind: VarAccessKind::Write,
                    span: elem_range.clone(),
                    property_path: path,
                    segment_spans,
                    construct_span: Some(elem_range),
                    segment_construct_spans,
                });
            }
        }
    }
}

/// Extract variable name, is_temporary flag, and span from an assignment target
/// if it matches a `State.variables.x`, `SugarCube.State.variables.x`,
/// `State_variables_x`, or `State_temporary_x` pattern.
fn extract_var_info_from_assignment_target(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> Option<(String, bool, std::ops::Range<usize>)> {
    use oxc_ast::ast::AssignmentTarget;

    match target {
        AssignmentTarget::StaticMemberExpression(member) => {
            if let oxc_ast::ast::Expression::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let oxc_ast::ast::Expression::Identifier(id) = &inner.object
                && id.name == "State"
            {
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                Some((format!("${}", member.property.name.as_str()), false, original_start..original_end))
            }
            else if let oxc_ast::ast::Expression::StaticMemberExpression(state_access) = &member.object
                && state_access.property.name == "variables"
                && let oxc_ast::ast::Expression::StaticMemberExpression(sc_state) = &state_access.object
                && sc_state.property.name == "State"
                && let oxc_ast::ast::Expression::Identifier(id) = &sc_state.object
                && id.name == "SugarCube"
            {
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                Some((format!("${}", member.property.name.as_str()), false, original_start..original_end))
            } else {
                None
            }
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            if let oxc_ast::ast::Expression::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let oxc_ast::ast::Expression::Identifier(id) = &inner.object
                && id.name == "State"
                && let oxc_ast::ast::Expression::StringLiteral(str_lit) = &member.expression
            {
                let original_start = preprocessed.map_to_original(member.span.start as usize);
                let original_end = preprocessed.map_to_original(member.span.end as usize);
                Some((format!("${}", str_lit.value.as_str()), false, original_start..original_end))
            } else {
                None
            }
        }
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            if let Some(var_part) = id.name.as_str().strip_prefix("State_variables_") {
                let (base_name, _) = split_substituted_var(var_part);
                let original_start = preprocessed.map_to_original(id.span.start as usize);
                let original_end = preprocessed.map_to_original(id.span.end as usize);
                Some((format!("${}", base_name), false, original_start..original_end))
            } else if let Some(var_part) = id.name.as_str().strip_prefix("State_temporary_") {
                let original_start = preprocessed.map_to_original(id.span.start as usize);
                let original_end = preprocessed.map_to_original(id.span.end as usize);
                Some((format!("_{}", var_part), true, original_start..original_end))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Internal: substituted variable checker
// ---------------------------------------------------------------------------

fn check_identifier_for_substituted_var(
    id: &oxc_ast::ast::IdentifierReference<'_>,
    is_write: bool,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    let name = id.name.as_str();

    // Guard: skip identifiers that are part of a def/ndef substitution.
    // These are already handled by extract_substitution_operators which
    // emits both the Operator token (for "def"/"ndef") and the Variable
    // token (for the operand) with correct spans. If we let this function
    // also emit a var_op, it produces a DUPLICATE Variable token at the
    // wrong position (mapped to the start of the substitution instead of
    // the variable's actual position).
    //
    // We detect def/ndef substitutions by checking if the identifier's
    // processed position falls within any substitution whose original_text
    // starts with "def " or "ndef ".
    let id_processed_start = id.span.start as usize;
    for sub in &preprocessed.substitutions {
        if (sub.original_text.starts_with("def ") || sub.original_text.starts_with("ndef "))
            && id_processed_start >= sub.processed_range.start
            && id_processed_start < sub.processed_range.end
        {
            return; // Skip — already handled by extract_substitution_operators
        }
    }

    if let Some(_var_part) = name.strip_prefix("State_variables_") {
        let (base_name, property_path) = split_substituted_var(_var_part);
        let var_name = format!("${}", base_name);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);
        let span = original_start..original_end;
        let segment_spans = compute_target_segment_spans(&var_name, property_path, &span);

        analysis.var_ops.push(AnalyzedVarOp {
            name: var_name,
            is_temporary: false,
            access_kind: if is_write { VarAccessKind::Write } else { VarAccessKind::Read },
            span,
            property_path: property_path.to_string(),
            segment_spans,
            construct_span: None,
                            segment_construct_spans: Vec::new(),
        });
    }

    if let Some(var_part) = name.strip_prefix("State_temporary_") {
        let var_name = format!("_{}", var_part);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);
        let span = original_start..original_end;
        let segment_spans = compute_target_segment_spans(&var_name, "", &span);

        analysis.var_ops.push(AnalyzedVarOp {
            name: var_name,
            is_temporary: true,
            access_kind: if is_write { VarAccessKind::Write } else { VarAccessKind::Read },
            span,
            property_path: String::new(),
            segment_spans,
            construct_span: None,
                            segment_construct_spans: Vec::new(),
        });
    }
}

// ---------------------------------------------------------------------------
// Internal: helpers
// ---------------------------------------------------------------------------

/// Demangle a function name that was preprocessed by the SugarCube variable
/// substitutor.
///
/// The preprocessor replaces `$var` → `State_variables_varName` and
/// `_var` → `State_temporary_varName`. When these appear as function
/// declaration names or variable binding names, we need to restore the
/// original name and compute the correct offset for the original source
/// position.
///
/// Returns `(demangled_name, original_name_offset)`.
fn demangle_function_name(
    preprocessed_name: &str,
    span_start: usize,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> (String, usize) {
    if let Some(var_part) = preprocessed_name.strip_prefix("State_temporary_") {
        let original_name = format!("_{}", var_part);
        let name_offset = preprocessed.map_to_original(span_start);
        (original_name, name_offset)
    } else if let Some(var_part) = preprocessed_name.strip_prefix("State_variables_") {
        let (base_name, _property_path) = split_substituted_var(var_part);
        let original_name = format!("${}", base_name);
        let name_offset = preprocessed.map_to_original(span_start);
        (original_name, name_offset)
    } else {
        let name_offset = preprocessed.map_to_original(span_start);
        (preprocessed_name.to_string(), name_offset)
    }
}

/// Try to classify an identifier reference as a function call target.
///
/// When a SugarCube variable like `_myHelper` is used as a function call
/// target (`_myHelper()`), the preprocessor has already replaced it with
/// `State_temporary_myHelper`. This function detects that case and returns
/// a `FunctionCallInfo` so the token builder emits a `Function` token
/// instead of a `Variable` token.
///
/// Returns `None` if the identifier is not a preprocessed SugarCube variable
/// (i.e., it's a regular JS identifier and should be handled normally).
fn try_classify_as_function_call(
    id: &oxc_ast::ast::IdentifierReference<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) -> Option<FunctionCallInfo> {
    let name = id.name.as_str();

    if let Some(var_part) = name.strip_prefix("State_temporary_") {
        // Check if this substituted variable has a property path.
        // The preprocessor replaces `$var.prop` → `State_temporary_var_prop`,
        // so underscores in var_part after the base name represent property
        // accesses. If there's a property path, the last segment is the
        // method being called — emit a Variable token for the root and a
        // Function token for just the method name.
        let (base_name, property_path) = split_substituted_var(var_part);
        if property_path.is_empty() {
            // Simple _var() call — no property path
            let func_name = format!("_{}", base_name);
            let original_start = preprocessed.map_to_original(id.span.start as usize);
            let original_end = preprocessed.map_to_original(id.span.end as usize);
            Some(FunctionCallInfo {
                name: func_name,
                span: original_start..original_end,
            })
        } else {
            // Method call on _var: _var.prop()
            // Emit Variable token for the root, Function token for the method
            emit_substituted_var_method_call(
                &format!("_{}", base_name),
                base_name,
                property_path,
                false, // is_temporary
                id,
                preprocessed,
                analysis,
            )
        }
    } else if let Some(var_part) = name.strip_prefix("State_variables_") {
        let (base_name, property_path) = split_substituted_var(var_part);
        if property_path.is_empty() {
            // Simple $var() call — no property path
            let func_name = format!("${}", base_name);
            let original_start = preprocessed.map_to_original(id.span.start as usize);
            let original_end = preprocessed.map_to_original(id.span.end as usize);
            Some(FunctionCallInfo {
                name: func_name,
                span: original_start..original_end,
            })
        } else {
            // Method call on $var: $var.prop()
            // Emit Variable token for the root, Function token for the method
            emit_substituted_var_method_call(
                &format!("${}", base_name),
                base_name,
                property_path,
                false, // is_temporary
                id,
                preprocessed,
                analysis,
            )
        }
    } else {
        // ── SugarCube builtin functions ───────────────────────────────
        if crate::sugarcube::macros::is_builtin_function(name) {
            let original_start = preprocessed.map_to_original(id.span.start as usize);
            let original_end = preprocessed.map_to_original(id.span.end as usize);
            Some(FunctionCallInfo {
                name: name.to_string(),
                span: original_start..original_end,
            })
        } else {
            None
        }
    }
}

/// Emit a Variable token for the root and a Function token for the method
/// when a substituted variable with a property path is called as a function.
///
/// Example: `$arr.last()` → preprocessed to `State_variables_arr_last()`.
/// oxc sees this as Identifier("State_variables_arr_last") called as function.
/// We emit:
/// - Variable token for `$arr` (the root variable)
/// - Property tokens for intermediate properties (if any)
/// - Function token for `last` (the method being called)
///
/// Returns a FunctionCallInfo with the span of just the method name (the
/// last segment), so the caller can push it to `function_calls`.
fn emit_substituted_var_method_call(
    var_name: &str,
    _base_name: &str,
    property_path: &str,
    is_temporary: bool,
    id: &oxc_ast::ast::IdentifierReference<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) -> Option<FunctionCallInfo> {
    // The full span of the substituted identifier in original source coords
    let original_start = preprocessed.map_to_original(id.span.start as usize);
    let original_end = preprocessed.map_to_original(id.span.end as usize);
    let full_span = original_start..original_end;

    // Compute segment spans for the root variable and each property.
    let all_segments = crate::sugarcube::js::js_annotate::compute_target_segment_spans(
        var_name,
        property_path,
        &full_span,
    );

    // The last segment is the method name being called. We must NOT emit
    // a Property token for it — it gets a Function token from
    // `function_calls` instead. Overlapping Property + Function tokens
    // at the same span causes VS Code to show "Property" instead of
    // "Function" (undefined behavior for overlapping semantic tokens).
    //
    // So we split: all_segments except the last → var_op's segment_spans
    // (emits Variable for root + Property for intermediate props).
    // Last segment → FunctionCallInfo (emits Function token).
    let (var_op_segments, method_segment) = if all_segments.len() > 1 {
        let split_pos = all_segments.len() - 1;
        (all_segments[..split_pos].to_vec(), all_segments[split_pos].clone())
    } else {
        // Only one segment (shouldn't happen for method calls, but handle
        // gracefully) — use it for the Function token, emit no var_op.
        (Vec::new(), all_segments.first().cloned().unwrap_or(full_span.clone()))
    };

    // Emit Variable token for the root + Property tokens for intermediate
    // segments (everything EXCEPT the method name).
    //
    // IMPORTANT: The var_op's property_path must NOT include the method
    // name. Only intermediate properties (e.g., `name` in `$arr.name.first()`)
    // should be registered. The method name is handled via FunctionCallInfo
    // and must NOT appear in the variable tree as a property — otherwise
    // the VSCode variable panel shows methods like `.last()`, `.pushUnique()`
    // mixed in with real properties like `.length`.
    if let Some(first_seg) = var_op_segments.first() {
        // Strip the last segment (method name) from property_path.
        // E.g., "name.first" → "name", "last" → "" (no intermediate props).
        let var_op_property_path = if let Some(last_dot) = property_path.rfind('.') {
            property_path[..last_dot].to_string()
        } else {
            String::new() // No intermediate properties — just the root variable
        };

        analysis.var_ops.push(AnalyzedVarOp {
            name: var_name.to_string(),
            is_temporary,
            access_kind: VarAccessKind::Read,
            span: first_seg.clone(),
            property_path: var_op_property_path,
            segment_spans: var_op_segments,
            construct_span: None,
            segment_construct_spans: Vec::new(),
        });
    }

    // Return the method segment as a FunctionCallInfo so the caller
    // pushes a Function token for it.
    let method_name = property_path.rsplit('.').next().unwrap_or(property_path);
    Some(FunctionCallInfo {
        name: method_name.to_string(),
        span: method_segment,
    })
}

/// Extract a string value from a function argument.
fn extract_string_from_arg(arg: &oxc_ast::ast::Argument<'_>) -> Option<String> {
    use oxc_ast::ast::Argument as Arg;
    match arg {
        Arg::StringLiteral(str_lit) => {
            Some(str_lit.value.to_string())
        }
        Arg::TemplateLiteral(tmpl) => {
            if tmpl.expressions.is_empty() && tmpl.quasis.len() == 1 {
                Some(tmpl.quasis[0].value.raw.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract the `BodyRequirement` from a `Macro.add("name", config)` call.
///
/// SugarCube's `Macro.add()` accepts a config object as the second argument.
/// The presence of the `tags` field on that object signals body requirement:
///
/// - `tags` omitted → inline macro (`Never`) — no body, no close tag.
///   Example: `Macro.add("emojify", { handler() {...} })` → `<<emojify "x">>`
///
/// - `tags: null` → container macro (`Required`) — body required, close tag
///   expected, no named sub-tags.
///   Example: `Macro.add("banner", { tags: null, handler() {...} })`
///   → `<<banner>>...<</banner>>`
///
/// - `tags: ["a", "b"]` → container macro (`Required`) with named sub-tags
///   (like `<<if>>`/`<<elseif>>`/`<<else>>`).
///   Example: `Macro.add("switch", { tags: ["case", "default"], ... })`
///
/// If the config object is missing or malformed, defaults to `Never` (the
/// most common case for inline macros — false positives on `Required` are
/// worse than false negatives on `Never` because unclosed-block diagnostics
/// are noisy).
fn extract_body_requirement_from_macro_add_config(
    args: &oxc_allocator::Vec<'_, oxc_ast::ast::Argument<'_>>,
) -> crate::types::BodyRequirement {
    use crate::types::BodyRequirement;
    use oxc_ast::ast::Argument as Arg;

    // The config object is the second argument (index 1).
    let Some(config_arg) = args.get(1) else {
        return BodyRequirement::Never;
    };

    let Arg::ObjectExpression(obj) = config_arg else {
        return BodyRequirement::Never;
    };

    // Scan properties for `tags`.
    for prop in &obj.properties {
        if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(p) = prop {
            // Property name must be `tags`. In oxc, identifier property keys
            // are `PropertyKey::StaticIdentifier(IdentifierName)` — NOT
            // `Identifier` (which is a different type for variable references).
            let is_tags = match &p.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(ident) => ident.name == "tags",
                oxc_ast::ast::PropertyKey::StringLiteral(str_lit) => str_lit.value == "tags",
                _ => false,
            };
            if !is_tags {
                continue;
            }
            // Found `tags` — its value determines container vs sub-tagged
            // container, but both map to `Required` for our purposes.
            // We only need to distinguish "has body" from "no body".
            //
            // Note: `p.value` is an `oxc_ast::ast::Expression`, not an
            // `Argument`. Object property values are always expressions.
            use oxc_ast::ast::Expression as Expr;
            return match &p.value {
                // `tags: null` → container
                Expr::NullLiteral(_) => BodyRequirement::Required,
                // `tags: ["a", "b"]` → container with sub-tags
                Expr::ArrayExpression(_) => BodyRequirement::Required,
                // `tags: undefined` or `tags: someVar` — can't statically
                // determine. Default to `Never` (inline) since explicit
                // `null`/array is the documented way to opt into container.
                _ => BodyRequirement::Never,
            };
        }
    }

    // No `tags` property found → inline macro.
    BodyRequirement::Never
}

/// Walk a call argument, recursing into nested expressions.
fn walk_argument(
    arg: &oxc_ast::ast::Argument<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::Argument as Arg;

    match arg {
        Arg::SpreadElement(spread) => {
            walk_expression(&spread.argument, preprocessed, analysis);
        }
        Arg::StaticMemberExpression(member) => {
            walk_expression(&member.object, preprocessed, analysis);
        }
        Arg::ComputedMemberExpression(member) => {
            walk_expression(&member.object, preprocessed, analysis);
            walk_expression(&member.expression, preprocessed, analysis);
        }
        Arg::CallExpression(call) => {
            walk_expression(&call.callee, preprocessed, analysis);
            for a in &call.arguments {
                walk_argument(a, preprocessed, analysis);
            }
        }
        Arg::AssignmentExpression(assign) => {
            check_assignment_for_var_writes(&assign.left, &assign.right, preprocessed, analysis);
            walk_expression(&assign.right, preprocessed, analysis);
        }
        Arg::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, analysis);
        }
        Arg::BinaryExpression(bin) => {
            walk_expression(&bin.left, preprocessed, analysis);
            walk_expression(&bin.right, preprocessed, analysis);
        }
        Arg::LogicalExpression(logic) => {
            walk_expression(&logic.left, preprocessed, analysis);
            walk_expression(&logic.right, preprocessed, analysis);
        }
        Arg::SequenceExpression(seq) => {
            for e in &seq.expressions {
                walk_expression(e, preprocessed, analysis);
            }
        }
        Arg::ConditionalExpression(cond) => {
            walk_expression(&cond.test, preprocessed, analysis);
            walk_expression(&cond.consequent, preprocessed, analysis);
            walk_expression(&cond.alternate, preprocessed, analysis);
        }
        Arg::ParenthesizedExpression(pe) => {
            walk_expression(&pe.expression, preprocessed, analysis);
        }
        Arg::FunctionExpression(func_expr) => {
            push_keyword(analysis, "function", func_expr.span.start as usize, preprocessed);
            emit_param_tokens(&func_expr.params, preprocessed, analysis);
            walk_function_body(&func_expr.body, preprocessed, analysis);
        }
        Arg::ArrowFunctionExpression(arrow) => {
            emit_param_tokens(&arrow.params, preprocessed, analysis);
            walk_function_body(&arrow.body, preprocessed, analysis);
        }
        Arg::UnaryExpression(unary) => {
            emit_unary_operator(unary, preprocessed, analysis);
            let kw = match unary.operator {
                oxc_ast::ast::UnaryOperator::Typeof => Some("typeof"),
                oxc_ast::ast::UnaryOperator::Void => Some("void"),
                oxc_ast::ast::UnaryOperator::Delete => Some("delete"),
                _ => None,
            };
            if let Some(kw) = kw {
                push_keyword(analysis, kw, unary.span.start as usize, preprocessed);
            }
            walk_expression(&unary.argument, preprocessed, analysis);
        }
        Arg::NewExpression(new_expr) => {
            push_keyword(analysis, "new", new_expr.span.start as usize, preprocessed);
            walk_expression(&new_expr.callee, preprocessed, analysis);
            for arg in &new_expr.arguments {
                walk_argument(arg, preprocessed, analysis);
            }
        }
        Arg::RegExpLiteral(regex_lit) => {
            let span = preprocessed.map_range_to_original(
                regex_lit.span.start as usize..regex_lit.span.end as usize
            );
            analysis.regex_spans.push(span);
        }
        _ => {}
    }
}

/// Split a substituted variable name into base name and property path.
///
/// `State_variables_player_name` → ("player", "name")
/// `State_variables_hp` → ("hp", "")
fn split_substituted_var(var_part: &str) -> (&str, &str) {
    if let Some(underscore_pos) = var_part.find('_') {
        let base = &var_part[..underscore_pos];
        let prop = &var_part[underscore_pos + 1..];
        (base, prop)
    } else {
        (var_part, "")
    }
}

// ---------------------------------------------------------------------------
// Operator extraction from substitution table and oxc AST
// ---------------------------------------------------------------------------

/// Extract operator spans from the preprocessor's substitution table.
///
/// SugarCube keyword operators (`to`, `eq`, `and`, etc.) are normalized to JS
/// equivalents by the preprocessor before oxc sees them. The substitution table
/// records the original position and text of each normalization. This function
/// walks those substitutions and emits `OperatorSpan` entries for each one that
/// represents an operator normalization (skipping `$var` and `_var` substitutions).
///
/// This is the primary mechanism for SugarCube keyword operator tokens. Standard
/// JS operators (like `+`, `>`, `===` that appear directly in the source without
/// normalization) are emitted by the `emit_*_operator` functions during the
/// oxc AST walk.
fn extract_substitution_operators(
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    /// SugarCube keyword operator → operator kind classification.
    /// Returns `None` for non-operator substitutions (like $var replacements).
    fn classify_keyword(keyword: &str) -> Option<OperatorKind> {
        match keyword {
            "to" => Some(OperatorKind::Assignment),
            "eq" | "neq" | "is" | "isnot" | "gt" | "gte" | "lt" | "lte" => {
                Some(OperatorKind::Comparison)
            }
            "and" | "or" | "not" => Some(OperatorKind::Logical),
            _ => None,
        }
    }

    /// Detect `def` / `ndef` substitutions and return the keyword length.
    ///
    /// Unlike simple keyword operators (`to`, `eq`, etc.) whose
    /// `original_text` is just the keyword itself, `def`/`ndef`
    /// substitutions consume the entire `def $var` expression — so
    /// `original_text` is e.g. `"def $hp"` or `"ndef _temp"`. We detect
    /// these by prefix and return the keyword length (3 for `def`,
    /// 4 for `ndef`) so the caller can emit an Operator span for just
    /// the keyword portion (NOT the whole expression — the variable
    /// portion gets its own Variable token via `check_identifier_for_substituted_var`).
    fn def_ndef_keyword_len(original_text: &str) -> Option<usize> {
        if original_text.starts_with("ndef") {
            // Ensure the char after "ndef" is NOT an ident char —
            // prevents matching "ndefinable". The substitution always
            // has whitespace after the keyword (e.g., "ndef $x"), so
            // this check is belt-and-suspenders.
            let after = original_text.as_bytes().get(4).copied();
            match after {
                None => Some(4),
                Some(b) if !b.is_ascii_alphanumeric() && b != b'_' => Some(4),
                _ => None,
            }
        } else if original_text.starts_with("def") {
            let after = original_text.as_bytes().get(3).copied();
            match after {
                None => Some(3),
                Some(b) if !b.is_ascii_alphanumeric() && b != b'_' => Some(3),
                _ => None,
            }
        } else {
            None
        }
    }

    for sub in &preprocessed.substitutions {
        // $var substitutions start with '$' or '_' — skip those.
        // Operator normalizations have alphabetic original_text (like "to", "eq").

        // ── def / ndef ──────────────────────────────────────────────
        // These are unary prefix operators whose substitution covers the
        // whole `def $var` / `ndef $var` expression (because the variable
        // must be consumed and wrapped in a typeof check). We emit an
        // Operator span for just the keyword portion — the variable
        // portion gets its own Variable token via the regular
        // `check_identifier_for_substituted_var` path in walk_expression.
        //
        // Classify as Comparison because `def`/`ndef` are semantically
        // comparison-like (they check equality with "undefined").
        if let Some(kw_len) = def_ndef_keyword_len(&sub.original_text) {
            let op_span = (sub.original_range.start + preprocessed.origin_offset)
                ..(sub.original_range.start + kw_len + preprocessed.origin_offset);
            analysis.operator_spans.push(OperatorSpan {
                kind: OperatorKind::Comparison,
                span: op_span,
            });

            // Also emit a var_op for the variable operand with the CORRECT
            // span. The substitution's original_text is like "def $hp" or
            // "ndef $obj.prop". The variable starts after the keyword + any
            // whitespace. We find the `$` or `_` sigil in the original_text
            // and compute the variable span relative to original_range.
            //
            // Without this, the var_op from check_identifier_for_substituted_var
            // maps the State_variables_hp identifier back through the
            // preprocessor, which clamps to the START of the substitution
            // (position of "def"), producing a Variable token at the wrong
            // position (on "def" instead of "$hp").
            let orig_text = &sub.original_text;
            let orig_base = sub.original_range.start + preprocessed.origin_offset;
            // Find the sigil position within original_text (after the keyword)
            if let Some(sigil_rel) = orig_text[kw_len..].find(|c| c == '$' || c == '_') {
                let var_start_rel = kw_len + sigil_rel;
                let var_start = orig_base + var_start_rel;
                let var_end = orig_base + orig_text.len();
                let var_span = var_start..var_end;

                // Extract the variable name (sigil + identifier + dot-path)
                let var_text = &orig_text[var_start_rel..];
                let is_temp = var_text.starts_with('_');
                // Name is sigil + identifier (no dot-path)
                let var_name: String = var_text.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                // Property path is everything after the first identifier
                let property_path = if let Some(dot_pos) = var_text.find('.') {
                    var_text[dot_pos..].to_string()
                } else {
                    String::new()
                };

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: is_temp,
                    access_kind: VarAccessKind::Read,
                    span: var_span.clone(),
                    property_path: property_path.clone(),
                    segment_spans: vec![var_span],
                    construct_span: None,
                    segment_construct_spans: Vec::new(),
                });
            }
            continue;
        }

        if let Some(kind) = classify_keyword(&sub.original_text) {
            // The substitution's original_range is relative to the preprocessed
            // source (the snippet), so we need to map it through origin_offset.
            let span = (sub.original_range.start + preprocessed.origin_offset)
                ..(sub.original_range.end + preprocessed.origin_offset);
            analysis.operator_spans.push(OperatorSpan { kind, span });
        }
    }
}



fn extract_comments(preprocessed: &super::js_preprocess::PreprocessedJs, analysis: &mut JsAnalysis) {
    let src = preprocessed.source.as_bytes();
    let len = src.len();
    let mut i = 0usize;
    let regex_spans: Vec<_> = analysis.regex_spans.iter().cloned().collect();
    let is_in_regex = |preprocessed_pos: usize| -> bool {
        let orig_pos = preprocessed.map_to_original(preprocessed_pos + preprocessed.wrapping_offset);
        regex_spans.iter().any(|r| orig_pos >= r.start && orig_pos < r.end)
    };
    while i < len {
        let b = src[i];
        if is_in_regex(i) {
            let orig_pos = preprocessed.map_to_original(i + preprocessed.wrapping_offset);
            if let Some(r) = regex_spans.iter().find(|r| orig_pos >= r.start && orig_pos < r.end) {
                let mut j = i;
                while j < len {
                    let orig_j = preprocessed.map_to_original(j + preprocessed.wrapping_offset);
                    if orig_j >= r.end { break; }
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        if b == b'"' || b == b'\'' || b == b'`' {
            let q = b; i += 1;
            while i < len { if src[i] == b'\\' { i += 2; continue; } if src[i] == q { i += 1; break; } i += 1; }
            continue;
        }
        if b == b'/' && i + 1 < len && src[i + 1] == b'*' {
            let start = i; i += 2;
            while i + 1 < len { if src[i] == b'*' && src[i + 1] == b'/' { i += 2; break; } i += 1; }
            let end = i.min(len);
            let os = preprocessed.map_to_original(start + preprocessed.wrapping_offset);
            let oe = preprocessed.map_to_original(end + preprocessed.wrapping_offset);
            if oe > os { analysis.comment_spans.push(CommentSpan { kind: CommentKind::CStyle, span: os..oe }); }
            continue;
        }
        if b == b'/' && i + 1 < len && src[i + 1] == b'/' {
            let start = i; i += 2;
            while i < len && src[i] != b'\n' { i += 1; }
            let end = i;
            let os = preprocessed.map_to_original(start + preprocessed.wrapping_offset);
            let oe = preprocessed.map_to_original(end + preprocessed.wrapping_offset);
            if oe > os { analysis.comment_spans.push(CommentSpan { kind: CommentKind::JsLine, span: os..oe }); }
            continue;
        }
        i += 1;
    }
}

/// Emit an operator span for a binary expression from the oxc AST.
///
/// Computes the operator's span from the gap between the left and right operand
/// spans, then maps it to original source coordinates. Only emits for operators
/// that were NOT already covered by the substitution table (i.e., standard JS
/// operators like `+`, `-`, `>`, `<`, `===`, `!==`).
fn emit_binary_operator(
    bin: &oxc_ast::ast::BinaryExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::BinaryOperator;

    let kind = match bin.operator {
        BinaryOperator::Equality
        | BinaryOperator::Inequality
        | BinaryOperator::StrictEquality
        | BinaryOperator::StrictInequality
        | BinaryOperator::GreaterThan
        | BinaryOperator::GreaterEqualThan
        | BinaryOperator::LessThan
        | BinaryOperator::LessEqualThan
        | BinaryOperator::In
        | BinaryOperator::Instanceof => OperatorKind::Comparison,
        BinaryOperator::Addition | BinaryOperator::Subtraction => OperatorKind::Arithmetic,
        BinaryOperator::Multiplication
        | BinaryOperator::Division
        | BinaryOperator::Remainder
        | BinaryOperator::Exponential => OperatorKind::Arithmetic,
        BinaryOperator::BitwiseAnd
        | BinaryOperator::BitwiseOR
        | BinaryOperator::BitwiseXOR
        | BinaryOperator::ShiftLeft
        | BinaryOperator::ShiftRight
        | BinaryOperator::ShiftRightZeroFill => OperatorKind::Other,
    };

    let span = compute_operator_span_between(
        bin.left.span(),
        bin.right.span(),
        preprocessed,
    );

    // Only emit if this operator wasn't already captured by the substitution
    // table (which handles SugarCube keyword operators like `eq`, `gt`, etc.)
    if !is_covered_by_substitution(span.clone(), preprocessed) {
        analysis.operator_spans.push(OperatorSpan { kind, span });
    }
}

/// Emit an operator span for a logical expression from the oxc AST.
fn emit_logical_operator(
    logic: &oxc_ast::ast::LogicalExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    let kind = OperatorKind::Logical;

    let span = compute_operator_span_between(
        logic.left.span(),
        logic.right.span(),
        preprocessed,
    );

    // Only emit if not already covered by substitution (e.g., `and` → `&&`)
    if !is_covered_by_substitution(span.clone(), preprocessed) {
        analysis.operator_spans.push(OperatorSpan { kind, span });
    }
}

/// Emit an operator span for an assignment expression from the oxc AST.
fn emit_assignment_operator(
    assign: &oxc_ast::ast::AssignmentExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::AssignmentOperator;

    let kind = match assign.operator {
        AssignmentOperator::Assign => OperatorKind::Assignment,
        AssignmentOperator::Addition
        | AssignmentOperator::Subtraction
        | AssignmentOperator::Multiplication
        | AssignmentOperator::Division
        | AssignmentOperator::Remainder
        | AssignmentOperator::Exponential
        | AssignmentOperator::BitwiseAnd
        | AssignmentOperator::BitwiseOR
        | AssignmentOperator::BitwiseXOR
        | AssignmentOperator::ShiftLeft
        | AssignmentOperator::ShiftRight
        | AssignmentOperator::ShiftRightZeroFill
        | AssignmentOperator::LogicalAnd
        | AssignmentOperator::LogicalOr
        | AssignmentOperator::LogicalNullish => OperatorKind::CompoundAssign,
    };

    let span = compute_operator_span_between(
        assign.left.span(),
        assign.right.span(),
        preprocessed,
    );

    // Only emit if not already covered by substitution (e.g., `to` → `=`)
    if !is_covered_by_substitution(span.clone(), preprocessed) {
        analysis.operator_spans.push(OperatorSpan { kind, span });
    }
}

/// Emit an operator span for a unary expression from the oxc AST.
fn emit_unary_operator(
    unary: &oxc_ast::ast::UnaryExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::UnaryOperator;

    let kind = match unary.operator {
        UnaryOperator::LogicalNot => OperatorKind::Logical,
        UnaryOperator::Typeof | UnaryOperator::Void | UnaryOperator::Delete => OperatorKind::Other,
        UnaryOperator::UnaryPlus | UnaryOperator::UnaryNegation => OperatorKind::Arithmetic,
        UnaryOperator::BitwiseNot => OperatorKind::Other,
    };

    // For prefix unary operators, the operator is at the start of the expression.
    // Compute span from expression start to argument start.
    let op_start = preprocessed.map_to_original(unary.span.start as usize);
    let op_end = preprocessed.map_to_original(unary.argument.span().start as usize);
    if op_start < op_end {
        let span = op_start..op_end;
        if !is_covered_by_substitution(span.clone(), preprocessed) {
            analysis.operator_spans.push(OperatorSpan { kind, span });
        }
    }
}

/// Emit an operator span for an update expression from the oxc AST.
fn emit_update_operator(
    update: &oxc_ast::ast::UpdateExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    // Update operators (++, --) are compound assignments semantically.
    let kind = OperatorKind::CompoundAssign;

    if update.prefix {
        // Prefix: operator is before the argument
        let op_start = preprocessed.map_to_original(update.span.start as usize);
        let op_end = preprocessed.map_to_original(update.argument.span().start as usize);
        if op_start < op_end {
            let span = op_start..op_end;
            if !is_covered_by_substitution(span.clone(), preprocessed) {
                analysis.operator_spans.push(OperatorSpan { kind, span });
            }
        }
    } else {
        // Postfix: operator is after the argument
        let op_start = preprocessed.map_to_original(update.argument.span().end as usize);
        let op_end = preprocessed.map_to_original(update.span.end as usize);
        if op_start < op_end {
            let span = op_start..op_end;
            if !is_covered_by_substitution(span.clone(), preprocessed) {
                analysis.operator_spans.push(OperatorSpan { kind, span });
            }
        }
    }
}

/// Compute the operator span in original source coordinates from the gap
/// between two operand spans.
///
/// The operator sits between `left_span.end` and `right_span.start` in the
/// oxc AST. We map both boundaries back to original coordinates to get the
/// operator span. This may include surrounding whitespace, which is acceptable
/// for semantic token purposes — LSP clients render only the non-whitespace
/// portion visually.
fn compute_operator_span_between(
    left_span: oxc_span::Span,
    right_span: oxc_span::Span,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> std::ops::Range<usize> {
    let op_start = preprocessed.map_to_original(left_span.end as usize);
    let op_end = preprocessed.map_to_original(right_span.start as usize);
    op_start..op_end
}

/// Check whether a span in original source coordinates overlaps with any
/// substitution's original range.
///
/// This is used to avoid double-emitting operator tokens: the substitution
/// table already captures SugarCube keyword operators (like `eq`, `and`, `to`),
/// so we skip standard JS operator emission for those positions.
fn is_covered_by_substitution(
    span: std::ops::Range<usize>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
) -> bool {
    // Adjust span from passage-body-relative back to snippet-relative
    // for comparison against substitution original_ranges.
    let snippet_start = span.start.saturating_sub(preprocessed.origin_offset);
    let snippet_end = span.end.saturating_sub(preprocessed.origin_offset);
    let snippet_range = snippet_start..snippet_end;

    for sub in &preprocessed.substitutions {
        // Only check operator substitutions (skip $var substitutions)
        if sub.original_text.starts_with('$') || sub.original_text.starts_with('_') {
            continue;
        }
        // Check for overlap
        if snippet_range.start < sub.original_range.end
            && snippet_range.end > sub.original_range.start
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Namespace detection for SugarCube global objects
// ---------------------------------------------------------------------------

/// Check if an identifier name is a known SugarCube global object.
///
/// Uses the `builtin_globals()` catalog from `macros/globals.rs`. This
/// includes objects like `State`, `Engine`, `Story`, `Save`, `Config`,
/// `UI`, `Dialog`, `Macro`, `Template`, `SugarCube`, `setup`, etc.
fn is_known_global(name: &str) -> bool {
    crate::sugarcube::macros::builtin_globals()
        .iter()
        .any(|g| g.name == name)
}

fn is_sugarcube_global(name: &str) -> bool {
    is_known_global(name)
}

fn is_js_global(name: &str) -> bool {
    matches!(name,
        "document" | "window" | "console" | "navigator" | "location" |
        "history" | "screen" | "localStorage" | "sessionStorage" |
        "Array" | "Object" | "Math" | "JSON" | "Number" | "String" |
        "Boolean" | "Date" | "RegExp" | "Error" | "TypeError" |
        "RangeError" | "ReferenceError" | "SyntaxError" | "URIError" |
        "Promise" | "Set" | "Map" | "WeakMap" | "WeakSet" | "Symbol" |
        "Proxy" | "Reflect" | "Intl" | "BigInt" | "BigInt64Array" |
        "BigUint64Array" | "Float32Array" | "Float64Array" |
        "Int8Array" | "Int16Array" | "Int32Array" |
        "Uint8Array" | "Uint8ClampedArray" | "Uint16Array" | "Uint32Array" |
        "ArrayBuffer" | "SharedArrayBuffer" | "DataView" |
        "encodeURI" | "decodeURI" | "encodeURIComponent" | "decodeURIComponent" |
        "parseInt" | "parseFloat" | "isNaN" | "isFinite" |
        "eval" | "undefined" | "NaN" | "Infinity" |
        "setTimeout" | "setInterval" | "clearTimeout" | "clearInterval" |
        "requestAnimationFrame" | "cancelAnimationFrame" |
        "queueMicrotask"
    )
}

/// Emit a `NamespaceSpan` for a `StaticMemberExpression` whose object is
/// a known SugarCube global.
///
/// This function handles several patterns:
///
/// - `Engine.play()` → Namespace("Engine") + Property("play")
/// - `Story.has("Cave")` → Namespace("Story") + Property("has")
/// - `Config.debug` → Namespace("Config") + Property("debug")
/// - `State.turns` → Namespace("State") + Property("turns")
/// - `SugarCube.State` → Namespace("SugarCube") + Property("State")
/// - `SugarCube.Macro.add(...)` → Namespace("SugarCube") + Property("Macro")
///
/// **Skipped patterns** (already handled by other emission paths):
/// - `State.variables.x` → handled by `var_ops` (Variable + Property tokens)
/// - `SugarCube.State.variables.x` → handled by `var_ops`
/// - `Macro.add("name", ...)` → handled by call expression for macro_adds
/// - `Template.add("name", ...)` → handled by call expression for template_adds
fn emit_namespace_for_member_expr(
    member: &oxc_ast::ast::StaticMemberExpression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::Expression as Expr;

    // Only emit when the object of the member expression is a direct
    // identifier that's a known SugarCube global. For chained member
    // expressions like `SugarCube.Engine.play()`, the recursion will
    // walk into `SugarCube.Engine` and emit a namespace span at that
    // level (Namespace("SugarCube") + Property("Engine")). The outer
    // level's property ("play") is not emitted as a separate Property
    // token since "Engine" is a property of SugarCube, not a namespace.
    if let Expr::Identifier(id) = &member.object {
        let global_name = id.name.as_str();
        if is_known_global(global_name) {
            // Skip State.variables and State.temporary — they're the prefix
            // of a variable access that's already handled by var_ops.
            if global_name == "State"
                && matches!(member.property.name.as_str(), "variables" | "temporary")
            {
                return;
            }
            // Skip Macro.add and Template.add — handled by the call expression
            // handler for macro_adds/template_adds registration.
            if (global_name == "Macro" || global_name == "Template")
                && member.property.name == "add"
            {
                return;
            }
            // Skip SugarCube.State — it's always the prefix of a
            // SugarCube.State.variables.x chain that's already handled by
            // var_ops. Emitting Namespace("SugarCube") + Property("State")
            // would overlap with the Variable token for the entire chain.
            if global_name == "SugarCube"
                && member.property.name == "State"
            {
                return;
            }
            // Skip SugarCube.Macro and SugarCube.Template — they're prefixes
            // for Macro.add/Template.add patterns handled by the call handler.
            if global_name == "SugarCube"
                && matches!(member.property.name.as_str(), "Macro" | "Template")
            {
                return;
            }

            let ns_span = preprocessed.map_range_to_original(
                id.span.start as usize..id.span.end as usize
            );
            let prop_span = preprocessed.map_range_to_original(
                member.property.span.start as usize..member.property.span.end as usize
            );

            analysis.namespace_spans.push(NamespaceSpan {
                name: global_name.to_string(),
                span: ns_span,
                property_spans: vec![PropertySpan {
                    name: member.property.name.to_string(),
                    span: prop_span,
                }],
            });
        }
    }
    // For SugarCube-prefixed chains (e.g., SugarCube.Engine.play()):
    // The recursion will process the inner StaticMemberExpression
    // (SugarCube.Engine), where the object is an Identifier "SugarCube"
    // that IS a known global. At that level, we emit
    // Namespace("SugarCube") + Property("Engine").
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::js_preprocess;
    use knot_core::oxc::{parse_js, ParseMode as JsParseMode};

    /// Helper: parse JS and walk as script passage, returning the analysis.
    fn walk_script(source: &str) -> JsAnalysis {
        let preprocessed = js_preprocess::PreprocessedJs {
            source: source.to_string(),
            substitutions: Vec::new(),
            origin_offset: 0,
            wrapping_offset: 0,
        };
        let outcome = parse_js(source, JsParseMode::Module);
        outcome.with_program(|program| walk_script_passage(program, &preprocessed)).unwrap_or_default()
    }

    /// Helper: parse JS and walk as inline expression, returning the analysis.
    fn walk_inline(source: &str) -> JsAnalysis {
        let preprocessed = js_preprocess::preprocess_for_oxc(source, true);
        let outcome = parse_js(&preprocessed.source, JsParseMode::Expression);
        outcome.with_program(|program| walk_inline_js(program, &preprocessed)).unwrap_or_default()
    }

    #[test]
    fn walk_script_state_variables_write() {
        let analysis = walk_script("State.variables.hp = 100;");
        assert_eq!(analysis.var_ops.len(), 1);
        assert_eq!(analysis.var_ops[0].name, "$hp");
        assert_eq!(analysis.var_ops[0].access_kind, VarAccessKind::Write);
    }

    #[test]
    fn walk_script_macro_add() {
        let analysis = walk_script(r#"Macro.add("myMacro", {});"#);
        assert_eq!(analysis.macro_adds.len(), 1);
        assert_eq!(analysis.macro_adds[0].name, "myMacro");
    }

    #[test]
    fn walk_script_function_declaration() {
        let analysis = walk_script("function calculateScore(x) { return x * 2; }");
        assert_eq!(analysis.function_defs.len(), 1);
        assert_eq!(analysis.function_defs[0].name, "calculateScore");
    }

    #[test]
    fn walk_script_template_add() {
        let analysis = walk_script(r#"Template.add("myTemplate", "hello");"#);
        assert_eq!(analysis.template_adds.len(), 1);
        assert_eq!(analysis.template_adds[0].name, "myTemplate");
    }

    #[test]
    fn walk_inline_substituted_var() {
        let analysis = walk_inline("$hp + $gold");
        assert!(analysis.var_ops.len() >= 2, "Expected at least 2 var_ops, got {}", analysis.var_ops.len());
        let names: Vec<&str> = analysis.var_ops.iter().map(|op| op.name.as_str()).collect();
        assert!(names.contains(&"$hp"), "Expected $hp in var_ops");
        assert!(names.contains(&"$gold"), "Expected $gold in var_ops");
    }

    #[test]
    fn walk_inline_state_variables_read() {
        let analysis = walk_inline("State_temporary_items = State.variables.ITEMS");
        assert!(analysis.var_ops.len() >= 2, "Expected at least 2 var_ops");
    }

    #[test]
    fn walk_inline_string_literal() {
        let analysis = walk_inline(r#"$name = "hello""#);
        let strings: Vec<_> = analysis.literal_spans.iter()
            .filter(|l| l.kind == LiteralKind::String)
            .collect();
        assert!(!strings.is_empty(), "Expected at least one String literal");
    }

    #[test]
    fn walk_inline_number_literal() {
        let analysis = walk_inline("$hp = 100");
        let numbers: Vec<_> = analysis.literal_spans.iter()
            .filter(|l| l.kind == LiteralKind::Number)
            .collect();
        assert!(!numbers.is_empty(), "Expected at least one Number literal");
    }

    #[test]
    fn walk_inline_boolean_literal() {
        let analysis = walk_inline("$alive = true");
        let booleans: Vec<_> = analysis.literal_spans.iter()
            .filter(|l| l.kind == LiteralKind::Boolean)
            .collect();
        assert!(!booleans.is_empty(), "Expected at least one Boolean literal");
    }

    #[test]
    fn walk_inline_sugarcube_comparison_operator() {
        let analysis = walk_inline("$hp gte 50");
        let comparisons: Vec<_> = analysis.operator_spans.iter()
            .filter(|op| op.kind == OperatorKind::Comparison)
            .collect();
        assert!(!comparisons.is_empty(), "Expected at least one Comparison operator (gte)");
    }

    #[test]
    fn walk_inline_sugarcube_logical_operator() {
        let analysis = walk_inline("$alive and $awake");
        let logicals: Vec<_> = analysis.operator_spans.iter()
            .filter(|op| op.kind == OperatorKind::Logical)
            .collect();
        assert!(!logicals.is_empty(), "Expected at least one Logical operator (and)");
    }

    #[test]
    fn walk_inline_sugarcube_assignment_operator() {
        let analysis = walk_inline("$hp to 100");
        let assignments: Vec<_> = analysis.operator_spans.iter()
            .filter(|op| op.kind == OperatorKind::Assignment)
            .collect();
        assert!(!assignments.is_empty(), "Expected at least one Assignment operator (to)");
    }

    #[test]
    fn walk_inline_js_arithmetic_operator() {
        let analysis = walk_inline("$hp + 10");
        let arithmetic: Vec<_> = analysis.operator_spans.iter()
            .filter(|op| op.kind == OperatorKind::Arithmetic)
            .collect();
        assert!(!arithmetic.is_empty(), "Expected at least one Arithmetic operator (+)");
    }
}
