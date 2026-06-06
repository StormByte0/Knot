//! Registry population from parsed AST.
//!
//! These functions mutate the [`VariableTree`] and [`CustomMacroRegistry`]
//! side tables. They are called during the ordered parse pipeline so that
//! registries are warm for later passages.

use super::ast;
use super::classifier::ClassifiedPassage;
use super::variable_tree::VariableTree;
use super::custom_macros::CustomMacroRegistry;

/// Populate registries from a parsed passage AST.
///
/// Walks the AST's var_ops and links to feed the `VariableTree`
/// and `CustomMacroRegistry` side tables.
pub(super) fn populate_registries_from_ast(
    vtree: &mut VariableTree,
    macro_reg: &mut CustomMacroRegistry,
    passage_ast: &ast::PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
    _body_offset: usize,
) {
    // Record variable operations from the AST
    for var_op in &passage_ast.var_ops {
        vtree.record_var(
            &var_op.name,
            var_op.is_temporary,
            var_op.is_write,
            &cp.header.name,
            file_uri,
            var_op.span.clone(),
            &var_op.property_path,
        );
    }

    // Extract widget definitions from AST nodes
    for node in &passage_ast.nodes {
        if let ast::AstNode::Macro { name, args, open_span, .. } = node {
            // <<widget name>> definitions
            if name.eq_ignore_ascii_case("widget") {
                let widget_name = args.trim().to_string();
                if !widget_name.is_empty() {
                    macro_reg.register_widget(
                        &widget_name,
                        &cp.header.name,
                        file_uri,
                        open_span.start,
                        None,
                    );
                }
            }
        }
    }

    // Mark variables as seeded if this is a special passage
    if cp.special_def.as_ref().is_some_and(|d| {
        matches!(d.behavior, knot_core::passage::SpecialPassageBehavior::Startup)
    }) {
        for var_op in &passage_ast.var_ops {
            if var_op.is_write {
                vtree.mark_seeded(&var_op.name);
            }
        }
    }
}

/// Walk JS in a script passage using oxc for deep registry population.
///
/// Script passages contain full JS programs. We preprocess the `$var`
/// references, parse with oxc, and walk the AST to find:
/// - `State.variables.x = value` → variable writes
/// - `Macro.add("name", {...})` → custom macro definitions
/// - Function declarations → function registry
pub(super) fn walk_script_js(
    vtree: &mut VariableTree,
    macro_reg: &mut CustomMacroRegistry,
    body_text: &str,
    cp: &ClassifiedPassage,
    file_uri: &str,
) {
    use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};
    use super::js_preprocess;
    use super::js_walk;

    // Preprocess $var references for oxc
    let preprocessed = js_preprocess::preprocess_for_oxc(body_text);

    // Parse with oxc as a JS module
    match parse_js(&preprocessed.source, JsParseMode::Module) {
        JsParseOutcome::Success(output) => {
            output.with_program(|program| {
                js_walk::walk_script_passage(
                    program,
                    &preprocessed,
                    file_uri,
                    &cp.header.name,
                    vtree,
                    macro_reg,
                );
            });
        }
        JsParseOutcome::Error(_diagnostics) => {
            // JS syntax errors are reported as diagnostics by the
            // caller — we just skip registry population for broken JS
        }
    }
}
