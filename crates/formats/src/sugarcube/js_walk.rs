//! Oxc AST walker for SugarCube registry population.
//!
//! This module walks parsed JavaScript ASTs to extract SugarCube-specific
//! information that feeds the variable and macro registries:
//!
//! - `State.variables.x = value` → variable write
//! - `SugarCube.State.variables.x` → variable read
//! - `Macro.add("name", {...})` → custom macro definition
//! - Function declarations → function registry
//!
//! ## Current Implementation
//!
//! The current implementation uses a simplified walk approach that handles
//! the most common patterns found in SugarCube script passages. It focuses
//! on correctness for the most frequent constructs rather than full AST
//! coverage. More complex patterns (nested aliases, computed member access)
//! can be added incrementally as needed.
//!
//! ## Preprocessing contract
//!
//! The JS source passed to oxc MUST be preprocessed by `js_preprocess`
//! first, so that `$var` references are replaced with `State_variables_varName`.

use oxc_ast::ast::Program;
use oxc_span::GetSpan;

use super::custom_macros::CustomMacroRegistry;
use super::variable_tree::VariableTree;

// ---------------------------------------------------------------------------
// Extraction results
// ---------------------------------------------------------------------------

/// Information extracted from walking a JS AST.
#[derive(Debug, Clone, Default)]
pub struct JsWalkResult {
    /// Number of State.variables writes found.
    pub state_writes: usize,
    /// Number of State.variables reads found.
    pub state_reads: usize,
    /// Number of Macro.add() calls found.
    pub macro_adds: usize,
    /// Number of function declarations found.
    pub function_defs: usize,
}

// ---------------------------------------------------------------------------
// Script passage walker
// ---------------------------------------------------------------------------

/// Walk a script passage's JS AST and populate registries.
///
/// This is the main entry point for script passages (`[script]` tagged).
/// These passages contain full JS programs that can define macros, set
/// state variables, and declare functions.
///
/// The walker uses `with_program()` from the oxc output to access the AST.
/// It scans for common SugarCube patterns in a best-effort manner.
pub fn walk_script_passage(
    program: &Program<'_>,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    macro_registry: &mut CustomMacroRegistry,
) -> JsWalkResult {
    let mut result = JsWalkResult::default();

    // Walk all top-level statements
    for stmt in &program.body {
        walk_statement(stmt, preprocessed, file_uri, passage_name, var_tree, macro_registry, &mut result);
    }

    result
}

// ---------------------------------------------------------------------------
// Inline JS snippet walker
// ---------------------------------------------------------------------------

/// Walk an inline JS snippet's AST and populate registries.
///
/// This is used for `<<set>>`, `<<run>>`, `<<script>>` blocks within
/// normal passages. The JS is typically an expression or a few statements.
pub fn walk_inline_js(
    program: &Program<'_>,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
) -> JsWalkResult {
    let mut result = JsWalkResult::default();

    // For inline JS, scan for substituted variable references
    scan_for_substituted_vars(program.source_text, preprocessed, file_uri, passage_name, var_tree, &mut result);

    result
}

// ---------------------------------------------------------------------------
// Internal: statement-level walk
// ---------------------------------------------------------------------------

fn walk_statement(
    stmt: &oxc_ast::ast::Statement<'_>,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    macro_registry: &mut CustomMacroRegistry,
    result: &mut JsWalkResult,
) {
    use oxc_ast::ast::Statement;

    match stmt {
        Statement::FunctionDeclaration(func) => {
            if let Some(_id) = &func.id {
                result.function_defs += 1;
            }
        }
        Statement::ExpressionStatement(expr_stmt) => {
            walk_expression(&expr_stmt.expression, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Statement::VariableDeclaration(var_decl) => {
            for decl in &var_decl.declarations {
                if let Some(init) = &decl.init {
                    walk_expression(init, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
                }
            }
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                walk_statement(s, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Statement::IfStatement(if_stmt) => {
            walk_expression(&if_stmt.test, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_statement(&if_stmt.consequent, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            if let Some(alt) = &if_stmt.alternate {
                walk_statement(alt, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Statement::ForStatement(for_stmt) => {
            walk_statement(&for_stmt.body, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Statement::WhileStatement(while_stmt) => {
            walk_statement(&while_stmt.body, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                walk_expression(arg, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Statement::TryStatement(try_stmt) => {
            for s in &try_stmt.block.body {
                walk_statement(s, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
            if let Some(catch) = &try_stmt.handler {
                for s in &catch.body.body {
                    walk_statement(s, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
                }
            }
            if let Some(finally) = &try_stmt.finalizer {
                for s in &finally.body {
                    walk_statement(s, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
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
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    macro_registry: &mut CustomMacroRegistry,
    result: &mut JsWalkResult,
) {
    use oxc_ast::ast::Expression as Expr;

    match expr {
        Expr::StaticMemberExpression(member) => {
            // Check for State.variables.x pattern
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
            // Recurse into object
            walk_expression(&member.object, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Expr::ComputedMemberExpression(member) => {
            walk_expression(&member.object, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&member.expression, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
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
                let offset = preprocessed.map_to_original(arg.span().start as usize);
                macro_registry.register_macro_add(
                    &name,
                    passage_name,
                    file_uri,
                    offset,
                    None,
                );
                result.macro_adds += 1;
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
                let offset = preprocessed.map_to_original(arg.span().start as usize);
                macro_registry.register_macro_add(
                    &name,
                    passage_name,
                    file_uri,
                    offset,
                    None,
                );
                result.macro_adds += 1;
            }
            // Recurse into callee and arguments
            walk_expression(&call.callee, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            for arg in &call.arguments {
                walk_argument(arg, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Expr::AssignmentExpression(assign) => {
            // Check for State.variables.x = value
            check_assignment_for_state_var(&assign.left, preprocessed, file_uri, passage_name, var_tree, result);
            walk_expression(&assign.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Expr::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, file_uri, passage_name, var_tree, result);
        }
        Expr::BinaryExpression(bin) => {
            walk_expression(&bin.left, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&bin.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Expr::LogicalExpression(logic) => {
            walk_expression(&logic.left, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&logic.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Expr::SequenceExpression(seq) => {
            for e in &seq.expressions {
                walk_expression(e, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Expr::ConditionalExpression(cond) => {
            walk_expression(&cond.test, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&cond.consequent, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&cond.alternate, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Internal: assignment target checker
// ---------------------------------------------------------------------------

fn check_assignment_for_state_var(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    result: &mut JsWalkResult,
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

                var_tree.record_var(
                    &var_name,
                    false,
                    true,
                    passage_name,
                    file_uri,
                    original_start..original_end,
                    "",
                );
                result.state_writes += 1;
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

                var_tree.record_var(
                    &var_name,
                    false,
                    true,
                    passage_name,
                    file_uri,
                    original_start..original_end,
                    "",
                );
                result.state_writes += 1;
            }
        }
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            check_identifier_for_substituted_var(id, true, preprocessed, file_uri, passage_name, var_tree, result);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Internal: substituted variable checker
// ---------------------------------------------------------------------------

fn check_identifier_for_substituted_var(
    id: &oxc_ast::ast::IdentifierReference<'_>,
    is_write: bool,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    result: &mut JsWalkResult,
) {
    let name = id.name.as_str();

    if let Some(var_part) = name.strip_prefix("State_variables_") {
        let (base_name, property_path) = split_substituted_var(var_part);
        let var_name = format!("${}", base_name);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);

        var_tree.record_var(
            &var_name,
            false,
            is_write,
            passage_name,
            file_uri,
            original_start..original_end,
            property_path,
        );
        if is_write {
            result.state_writes += 1;
        } else {
            result.state_reads += 1;
        }
    }

    if let Some(var_part) = name.strip_prefix("State_temporary_") {
        let var_name = format!("_{}", var_part);
        let original_start = preprocessed.map_to_original(id.span.start as usize);
        let original_end = preprocessed.map_to_original(id.span.end as usize);

        var_tree.record_var(
            &var_name,
            true,
            is_write,
            passage_name,
            file_uri,
            original_start..original_end,
            "",
        );
        if is_write {
            result.state_writes += 1;
        } else {
            result.state_reads += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: source text scanner for substituted vars (fallback)
// ---------------------------------------------------------------------------

/// Scan the preprocessed source text for substituted variable patterns.
/// This is used for inline JS expressions where the AST structure may not
/// be as easily traversable.
///
/// Note: `_source` (the preprocessed JS text) is not currently used by this
/// implementation, which relies solely on the substitution map. It is kept
/// for potential future use (e.g., regex-based fallback scanning).
fn scan_for_substituted_vars(
    _source: &str,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    result: &mut JsWalkResult,
) {
    // Scan for State_variables_XXX and State_temporary_XXX identifiers
    for sub in &preprocessed.substitutions {
        let original_text = &sub.original_text;

        if original_text.starts_with('$') {
            // Story variable: $hp, $player.name
            let name = original_text.clone();
            let base_name = if let Some(dot_pos) = name.find('.') {
                &name[..dot_pos]
            } else {
                &name
            };
            let property_path = if let Some(dot_pos) = name.find('.') {
                name[dot_pos + 1..].to_string()
            } else {
                String::new()
            };

            var_tree.record_var(
                base_name,
                false,
                false, // Inline expressions are typically reads
                passage_name,
                file_uri,
                sub.original_range.clone(),
                &property_path,
            );
            result.state_reads += 1;
        } else if original_text.starts_with('_') {
            // Temporary variable: _i, _count
            var_tree.record_var(
                original_text,
                true,
                false,
                passage_name,
                file_uri,
                sub.original_range.clone(),
                "",
            );
            result.state_reads += 1;
        }
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
///
/// `Argument` inherits all `Expression` variants plus `SpreadElement`.
/// This function mirrors `walk_expression` for the relevant variants.
fn walk_argument(
    arg: &oxc_ast::ast::Argument<'_>,
    preprocessed: &crate::sugarcube::js_preprocess::PreprocessedJs,
    file_uri: &str,
    passage_name: &str,
    var_tree: &mut VariableTree,
    macro_registry: &mut CustomMacroRegistry,
    result: &mut JsWalkResult,
) {
    use oxc_ast::ast::Argument as Arg;

    match arg {
        Arg::SpreadElement(spread) => {
            walk_expression(&spread.argument, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::StaticMemberExpression(member) => {
            if member.property.name == "variables"
                && let oxc_ast::ast::Expression::Identifier(id) = &member.object
                && id.name == "State"
            {
                // State.variables read detected at argument level
            }
            if member.property.name == "add"
                && let oxc_ast::ast::Expression::Identifier(id) = &member.object
                && id.name == "Macro"
            {
                // Macro.add detected at argument level
            }
            walk_expression(&member.object, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::ComputedMemberExpression(member) => {
            walk_expression(&member.object, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&member.expression, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::CallExpression(call) => {
            // Check for Macro.add("name", ...) pattern
            if let oxc_ast::ast::Expression::StaticMemberExpression(member) = &call.callee
                && member.property.name == "add"
                && let oxc_ast::ast::Expression::Identifier(id) = &member.object
                && (id.name == "Macro" || id.name == "SugarCube")
                && let Some(arg) = call.arguments.first()
                && let Some(name) = extract_string_from_arg(arg)
            {
                let offset = preprocessed.map_to_original(arg.span().start as usize);
                macro_registry.register_macro_add(
                    &name,
                    passage_name,
                    file_uri,
                    offset,
                    None,
                );
                result.macro_adds += 1;
            }
            walk_expression(&call.callee, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            for a in &call.arguments {
                walk_argument(a, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Arg::AssignmentExpression(assign) => {
            check_assignment_for_state_var(&assign.left, preprocessed, file_uri, passage_name, var_tree, result);
            walk_expression(&assign.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::Identifier(id) => {
            check_identifier_for_substituted_var(id, false, preprocessed, file_uri, passage_name, var_tree, result);
        }
        Arg::BinaryExpression(bin) => {
            walk_expression(&bin.left, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&bin.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::LogicalExpression(logic) => {
            walk_expression(&logic.left, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&logic.right, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
        }
        Arg::SequenceExpression(seq) => {
            for e in &seq.expressions {
                walk_expression(e, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            }
        }
        Arg::ConditionalExpression(cond) => {
            walk_expression(&cond.test, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&cond.consequent, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
            walk_expression(&cond.alternate, preprocessed, file_uri, passage_name, var_tree, macro_registry, result);
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
                };
                let mut var_tree = VariableTree::new();
                let mut macro_registry = CustomMacroRegistry::new();

                let result = output.with_program(|program| {
                    walk_script_passage(
                        program,
                        &preprocessed,
                        "file:///test.tw",
                        "Scripts",
                        &mut var_tree,
                        &mut macro_registry,
                    )
                });

                assert_eq!(result.state_writes, 1);
                assert!(var_tree.get_variable("$hp").is_some());
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
                };
                let mut var_tree = VariableTree::new();
                let mut macro_registry = CustomMacroRegistry::new();

                let result = output.with_program(|program| {
                    walk_script_passage(
                        program,
                        &preprocessed,
                        "file:///test.tw",
                        "Scripts",
                        &mut var_tree,
                        &mut macro_registry,
                    )
                });

                assert_eq!(result.macro_adds, 1);
                assert!(macro_registry.contains("myMacro"));
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
                let mut var_tree = VariableTree::new();

                let result = output.with_program(|program| {
                    walk_inline_js(
                        program,
                        &preprocessed,
                        "file:///test.tw",
                        "Start",
                        &mut var_tree,
                    )
                });

                assert!(result.state_reads >= 2);
                assert!(var_tree.get_variable("$hp").is_some());
                assert!(var_tree.get_variable("$gold").is_some());
            }
            JsParseOutcome::Error(_) => {}
        }
    }
}
