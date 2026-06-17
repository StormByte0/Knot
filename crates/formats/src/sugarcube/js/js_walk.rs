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
    AnalyzedVarOp, FunctionCallInfo, FunctionDefInfo, JsAnalysis, LiteralKind, LiteralSpan, MacroAddInfo,
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
            for decl in &var_decl.declarations {
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
            walk_expression(&if_stmt.test, preprocessed, analysis);
            walk_statement(&if_stmt.consequent, preprocessed, analysis);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement(alt, preprocessed, analysis);
            }
        }
        Statement::ForStatement(for_stmt) => {
            walk_statement(&for_stmt.body, preprocessed, analysis);
        }
        Statement::WhileStatement(while_stmt) => {
            walk_statement(&while_stmt.body, preprocessed, analysis);
        }
        Statement::ReturnStatement(ret) => {
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
                if let Some(call_info) = try_classify_as_function_call(id, preprocessed) {
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
                analysis.macro_adds.push(MacroAddInfo {
                    name,
                    name_offset,
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
                analysis.macro_adds.push(MacroAddInfo {
                    name,
                    name_offset,
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
            // Recurse into callee and arguments
            walk_expression(&call.callee, preprocessed, analysis);
            for arg in &call.arguments {
                walk_argument(arg, preprocessed, analysis);
            }
        }
        Expr::AssignmentExpression(assign) => {
            // Emit operator span for the assignment operator.
            emit_assignment_operator(&assign, preprocessed, analysis);
            // Check for State.variables.x = value
            check_assignment_for_state_var(&assign.left, preprocessed, analysis);
            // Check for object literal assignments to extract property paths
            check_object_literal_assignment(&assign.left, &assign.right, preprocessed, analysis);
            walk_expression(&assign.right, preprocessed, analysis);
        }
        Expr::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, analysis);
            // Note: We do NOT emit standalone NamespaceSpan for global identifiers
            // here because member expressions like `Engine.play()` are already
            // handled by `emit_namespace_for_member_expr`, which emits both
            // the Namespace and Property tokens. When we recurse into the
            // object of a member expression, we'd double-emit the Namespace.
            // Standalone global references (e.g., just `State` without `.x`)
            // are rare and not critical for highlighting.
        }
        Expr::BinaryExpression(bin) => {
            // Emit operator span for the binary operator.
            emit_binary_operator(&bin, preprocessed, analysis);
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
        Expr::NewExpression(new_expr) => {
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
            // Named function expression: `var f = function myFunc() { ... }`
            // The name (if present) is registered by the VariableDeclaration
            // handler above. Here we only need to recurse into the body.
            walk_function_body(&func_expr.body, preprocessed, analysis);
        }
        Expr::ArrowFunctionExpression(arrow) => {
            // Arrow function: `const f = () => { ... }` or `$(el).on('click', () => { ... })`
            // The name (if assigned to a variable) is registered by the
            // VariableDeclaration handler. Here we recurse into the body so
            // that Macro.add() / Template.add() / nested function definitions
            // inside arrow function bodies are discovered.
            walk_function_body(&arrow.body, preprocessed, analysis);
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
                });
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Internal: assignment target checker
// ---------------------------------------------------------------------------

fn check_assignment_for_state_var(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    use oxc_ast::ast::AssignmentTarget;

    match target {
        AssignmentTarget::StaticMemberExpression(member) => {
            // Check for State.variables.x = value
            if let oxc_ast::ast::Expression::StaticMemberExpression(inner) = &member.object
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
                });
            }
            // Check for SugarCube.State.variables.x = value
            if let oxc_ast::ast::Expression::StaticMemberExpression(state_access) = &member.object
                && state_access.property.name == "variables"
                && let oxc_ast::ast::Expression::StaticMemberExpression(sc_state) = &state_access.object
                && sc_state.property.name == "State"
                && let oxc_ast::ast::Expression::Identifier(id) = &sc_state.object
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
                    access_kind: VarAccessKind::Write,
                    span: span.clone(),
                    property_path: String::new(),
                    segment_spans: vec![span],
                    construct_span: None,
                });
            }
        }
        AssignmentTarget::ComputedMemberExpression(member) => {
            if let oxc_ast::ast::Expression::StaticMemberExpression(inner) = &member.object
                && inner.property.name == "variables"
                && let oxc_ast::ast::Expression::Identifier(id) = &inner.object
                && id.name == "State"
                && let oxc_ast::ast::Expression::StringLiteral(str_lit) = &member.expression
            {
                let prop_name = str_lit.value.as_str();
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
                });
            }
        }
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            check_identifier_for_substituted_var(id, true, preprocessed, analysis);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Internal: object literal assignment checker
// ---------------------------------------------------------------------------

/// Check if an assignment target is a `State.variables.x` pattern and
/// extract property paths from the RHS if it's an `ObjectExpression`.
fn check_object_literal_assignment(
    left: &oxc_ast::ast::AssignmentTarget<'_>,
    right: &oxc_ast::ast::Expression<'_>,
    preprocessed: &super::js_preprocess::PreprocessedJs,
    analysis: &mut JsAnalysis,
) {
    let var_info = extract_var_info_from_assignment_target(left, preprocessed);

    if let Some((var_name, is_temporary, target_span)) = var_info {
        if let oxc_ast::ast::Expression::ObjectExpression(obj) = right {
            // Emit a root-level write with the full object literal span
            let obj_span = obj.span();
            let obj_start = preprocessed.map_to_original(obj_span.start as usize);
            let obj_end = preprocessed.map_to_original(obj_span.end as usize);
            analysis.var_ops.push(AnalyzedVarOp {
                name: var_name.clone(),
                is_temporary,
                access_kind: VarAccessKind::Write,
                span: obj_start..obj_end,
                property_path: String::new(),
                segment_spans: vec![target_span.clone()],
                construct_span: None,
            });

            extract_object_property_paths_recursive(
                obj, &var_name, is_temporary, target_span.clone(), String::new(), vec![target_span.clone()], preprocessed, analysis,
            );
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

/// Recursively extract property paths from an oxc `ObjectExpression`.
fn extract_object_property_paths_recursive(
    obj: &oxc_ast::ast::ObjectExpression<'_>,
    var_name: &str,
    is_temporary: bool,
    _target_span: std::ops::Range<usize>,
    prefix: String,
    parent_segments: Vec<std::ops::Range<usize>>,
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

            if let Some(key) = key_str {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };

                let key_span = p.key.span();
                let original_start = preprocessed.map_to_original(key_span.start as usize);
                let original_end = preprocessed.map_to_original(key_span.end as usize);
                let key_range = original_start..original_end;

                let mut segment_spans = parent_segments.clone();
                segment_spans.push(key_range.clone());

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name.to_string(),
                    is_temporary,
                    access_kind: VarAccessKind::Write,
                    span: key_range.clone(),
                    property_path: path.clone(),
                    segment_spans,
                    construct_span: None,
                });

                if let oxc_ast::ast::Expression::ObjectExpression(inner_obj) = &p.value {
                    let mut child_segments = parent_segments.clone();
                    child_segments.push(key_range);
                    extract_object_property_paths_recursive(
                        inner_obj, var_name, is_temporary, _target_span.clone(),
                        path, child_segments, preprocessed, analysis,
                    );
                }
            }
        }
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
) -> Option<FunctionCallInfo> {
    let name = id.name.as_str();

    if let Some(var_part) = name.strip_prefix("State_temporary_") {
        let func_name = format!("_{}", var_part);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);
        Some(FunctionCallInfo {
            name: func_name,
            span: original_start..original_end,
        })
    } else if let Some(var_part) = name.strip_prefix("State_variables_") {
        let (base_name, _property_path) = split_substituted_var(var_part);
        let func_name = format!("${}", base_name);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);
        Some(FunctionCallInfo {
            name: func_name,
            span: original_start..original_end,
        })
    } else {
        None
    }
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
            check_assignment_for_state_var(&assign.left, preprocessed, analysis);
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
            // Callback function: e.g., `$(el).on('click', function() { ... })`.
            // Walk the body so Macro.add() / nested definitions inside
            // callbacks are discovered.
            walk_function_body(&func_expr.body, preprocessed, analysis);
        }
        Arg::ArrowFunctionExpression(arrow) => {
            // Arrow callback: e.g., `$(el).on('click', () => { ... })`.
            walk_function_body(&arrow.body, preprocessed, analysis);
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

    for sub in &preprocessed.substitutions {
        // $var substitutions start with '$' or '_' — skip those.
        // Operator normalizations have alphabetic original_text (like "to", "eq").
        if let Some(kind) = classify_keyword(&sub.original_text) {
            // The substitution's original_range is relative to the preprocessed
            // source (the snippet), so we need to map it through origin_offset.
            let span = (sub.original_range.start + preprocessed.origin_offset)
                ..(sub.original_range.end + preprocessed.origin_offset);
            analysis.operator_spans.push(OperatorSpan { kind, span });
        }
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
    use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};

    #[test]
    fn walk_script_state_variables_write() {
        let source = "State.variables.hp = 100;";
        match parse_js(source, JsParseMode::Module) {
            JsParseOutcome::Success(output) => {
                let preprocessed = js_preprocess::PreprocessedJs {
                    source: source.to_string(),
                    substitutions: Vec::new(),
                    origin_offset: 0,
                    wrapping_offset: 0,
                };

                let analysis = output.with_program(|program| {
                    walk_script_passage(program, &preprocessed)
                });

                assert_eq!(analysis.var_ops.len(), 1);
                assert_eq!(analysis.var_ops[0].name, "$hp");
                assert_eq!(analysis.var_ops[0].access_kind, VarAccessKind::Write);
            }
            JsParseOutcome::Error(_) => {}
        }
    }

    #[test]
    fn walk_script_macro_add() {
        let source = r#"Macro.add("myMacro", {});"#;
        match parse_js(source, JsParseMode::Module) {
            JsParseOutcome::Success(output) => {
                let preprocessed = js_preprocess::PreprocessedJs {
                    source: source.to_string(),
                    substitutions: Vec::new(),
                    origin_offset: 0,
                    wrapping_offset: 0,
                };

                let analysis = output.with_program(|program| {
                    walk_script_passage(program, &preprocessed)
                });

                assert_eq!(analysis.macro_adds.len(), 1);
                assert_eq!(analysis.macro_adds[0].name, "myMacro");
            }
            JsParseOutcome::Error(_) => {}
        }
    }

    #[test]
    fn walk_script_function_declaration() {
        let source = "function calculateScore(points) { return points * 2; }";
        match parse_js(source, JsParseMode::Module) {
            JsParseOutcome::Success(output) => {
                let preprocessed = js_preprocess::PreprocessedJs {
                    source: source.to_string(),
                    substitutions: Vec::new(),
                    origin_offset: 0,
                    wrapping_offset: 0,
                };

                let analysis = output.with_program(|program| {
                    walk_script_passage(program, &preprocessed)
                });

                assert_eq!(analysis.function_defs.len(), 1);
                assert_eq!(analysis.function_defs[0].name, "calculateScore");
                assert_eq!(analysis.function_defs[0].param_count, Some(1));
            }
            JsParseOutcome::Error(_) => {}
        }
    }

    #[test]
    fn walk_script_template_add() {
        let source = r#"Template.add("heal", function () { return "<<link 'Heal'>>...<</link>>"; });"#;
        match parse_js(source, JsParseMode::Module) {
            JsParseOutcome::Success(output) => {
                let preprocessed = js_preprocess::PreprocessedJs {
                    source: source.to_string(),
                    substitutions: Vec::new(),
                    origin_offset: 0,
                    wrapping_offset: 0,
                };

                let analysis = output.with_program(|program| {
                    walk_script_passage(program, &preprocessed)
                });

                assert_eq!(analysis.template_adds.len(), 1);
                assert_eq!(analysis.template_adds[0].name, "heal");
            }
            JsParseOutcome::Error(_) => {}
        }
    }

    #[test]
    fn walk_inline_substituted_var() {
        let original = "$hp + $gold";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                assert!(analysis.var_ops.len() >= 2, "Expected at least 2 var_ops, got {}", analysis.var_ops.len());
                let names: Vec<&str> = analysis.var_ops.iter().map(|op| op.name.as_str()).collect();
                assert!(names.contains(&"$hp"), "Expected $hp in var_ops");
                assert!(names.contains(&"$gold"), "Expected $gold in var_ops");
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_state_variables_read() {
        let source = "State_temporary_items = State.variables.ITEMS";
        match parse_js(source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let preprocessed = js_preprocess::PreprocessedJs {
                    source: source.to_string(),
                    substitutions: Vec::new(),
                    origin_offset: 0,
                    wrapping_offset: 0,
                };

                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                // State.variables.ITEMS should be detected as a READ of $ITEMS
                let items_reads: Vec<_> = analysis.var_ops.iter()
                    .filter(|op| op.name == "$ITEMS" && op.access_kind == VarAccessKind::Read)
                    .collect();
                assert!(!items_reads.is_empty(), "Expected $ITEMS READ, got {:?}", analysis.var_ops);

                // _items should be detected as a WRITE
                let items_writes: Vec<_> = analysis.var_ops.iter()
                    .filter(|op| op.name == "_items" && op.access_kind == VarAccessKind::Write)
                    .collect();
                assert!(!items_writes.is_empty(), "Expected _items WRITE, got {:?}", analysis.var_ops);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    // ── Literal span tests ────────────────────────────────────────────

    #[test]
    fn walk_inline_string_literal() {
        let original = r#"$name eq "hello""#;
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let strings: Vec<_> = analysis.literal_spans.iter()
                    .filter(|l| l.kind == LiteralKind::String)
                    .collect();
                assert!(!strings.is_empty(), "Expected at least one String literal, got {:?}", analysis.literal_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_number_literal() {
        let original = "$hp gte 50";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let numbers: Vec<_> = analysis.literal_spans.iter()
                    .filter(|l| l.kind == LiteralKind::Number)
                    .collect();
                assert!(!numbers.is_empty(), "Expected at least one Number literal, got {:?}", analysis.literal_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_boolean_literal() {
        let original = "$alive eq true";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let booleans: Vec<_> = analysis.literal_spans.iter()
                    .filter(|l| l.kind == LiteralKind::Boolean)
                    .collect();
                assert!(!booleans.is_empty(), "Expected at least one Boolean literal, got {:?}", analysis.literal_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    // ── Operator span tests ───────────────────────────────────────────

    #[test]
    fn walk_inline_sugarcube_comparison_operator() {
        let original = "$hp gte 50";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let comparisons: Vec<_> = analysis.operator_spans.iter()
                    .filter(|op| op.kind == OperatorKind::Comparison)
                    .collect();
                assert!(!comparisons.is_empty(), "Expected at least one Comparison operator (gte), got {:?}", analysis.operator_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_sugarcube_logical_operator() {
        let original = "$hasKey and $doorOpen";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let logicals: Vec<_> = analysis.operator_spans.iter()
                    .filter(|op| op.kind == OperatorKind::Logical)
                    .collect();
                assert!(!logicals.is_empty(), "Expected at least one Logical operator (and), got {:?}", analysis.operator_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_sugarcube_assignment_operator() {
        let original = "$name to \"hello\"";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let assignments: Vec<_> = analysis.operator_spans.iter()
                    .filter(|op| op.kind == OperatorKind::Assignment)
                    .collect();
                assert!(!assignments.is_empty(), "Expected at least one Assignment operator (to), got {:?}", analysis.operator_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }

    #[test]
    fn walk_inline_js_arithmetic_operator() {
        // Test a standard JS operator that doesn't get substituted
        let original = "$x + $y";
        let preprocessed = js_preprocess::preprocess_for_oxc(original);

        match parse_js(&preprocessed.source, JsParseMode::Expression) {
            JsParseOutcome::Success(output) => {
                let analysis = output.with_program(|program| {
                    walk_inline_js(program, &preprocessed)
                });

                let arithmetic: Vec<_> = analysis.operator_spans.iter()
                    .filter(|op| op.kind == OperatorKind::Arithmetic)
                    .collect();
                assert!(!arithmetic.is_empty(), "Expected at least one Arithmetic operator (+), got {:?}", analysis.operator_spans);
            }
            JsParseOutcome::Error(diags) => {
                panic!("Parse failed: {:?}", diags);
            }
        }
    }
}
