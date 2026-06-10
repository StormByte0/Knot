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
    AnalyzedVarOp, FunctionDefInfo, JsAnalysis, MacroAddInfo, TemplateAddInfo,
};
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
                let name = id.name.to_string();
                let name_offset = preprocessed.map_to_original(id.span.start as usize);
                let param_count = Some(func.params.items.len());
                analysis.function_defs.push(FunctionDefInfo {
                    name,
                    name_offset,
                    param_count,
                });
            }
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
                                let name = binding_name.name.to_string();
                                let name_offset = preprocessed.map_to_original(binding_name.span.start as usize);
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
                                let name = binding_name.name.to_string();
                                let name_offset = preprocessed.map_to_original(binding_name.span.start as usize);
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Read,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
                    construct_span: None,
                });
            }
            walk_expression(&member.object, preprocessed, analysis);
            walk_expression(&member.expression, preprocessed, analysis);
        }
        Expr::CallExpression(call) => {
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
            // Check for State.variables.x = value
            check_assignment_for_state_var(&assign.left, preprocessed, analysis);
            // Check for object literal assignments to extract property paths
            check_object_literal_assignment(&assign.left, &assign.right, preprocessed, analysis);
            walk_expression(&assign.right, preprocessed, analysis);
        }
        Expr::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, analysis);
        }
        Expr::BinaryExpression(bin) => {
            walk_expression(&bin.left, preprocessed, analysis);
            walk_expression(&bin.right, preprocessed, analysis);
        }
        Expr::LogicalExpression(logic) => {
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
            walk_expression(&unary.argument, preprocessed, analysis);
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Write,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Write,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
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

                analysis.var_ops.push(AnalyzedVarOp {
                    name: var_name,
                    is_temporary: false,
                    access_kind: VarAccessKind::Write,
                    span: original_start..original_end,
                    property_path: String::new(),
                    segment_spans: Vec::new(),
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

        analysis.var_ops.push(AnalyzedVarOp {
            name: var_name,
            is_temporary: false,
            access_kind: if is_write { VarAccessKind::Write } else { VarAccessKind::Read },
            span: original_start..original_end,
            property_path: property_path.to_string(),
            segment_spans: Vec::new(),
            construct_span: None,
        });
    }

    if let Some(var_part) = name.strip_prefix("State_temporary_") {
        let var_name = format!("_{}", var_part);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);

        analysis.var_ops.push(AnalyzedVarOp {
            name: var_name,
            is_temporary: true,
            access_kind: if is_write { VarAccessKind::Write } else { VarAccessKind::Read },
            span: original_start..original_end,
            property_path: String::new(),
            segment_spans: Vec::new(),
            construct_span: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Internal: helpers
// ---------------------------------------------------------------------------

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
}
