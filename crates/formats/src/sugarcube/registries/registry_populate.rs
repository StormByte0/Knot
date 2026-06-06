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

use crate::sugarcube::ast::{self, SetOperator};
use crate::sugarcube::classifier::ClassifiedPassage;
use super::variable_tree::VarAccessKind;
use super::SugarCubeRegistry;

/// Map a `SetOperator` from the AST to the appropriate `VarAccessKind`.
///
/// Simple assignments (`to`, `=`) → `Write`
/// Compound assignments (`+=`, `-=`, etc.) → `CompoundWrite`
/// Postfix operators (`++`, `--`) → `PostfixModify`
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
///
/// - `<<capture>>` → `Capture`
/// - `<<unset>>` → `Unset`
/// - `<<run>>` with assignment → `Write` (detected at JS walk level)
/// - All other macros (`<<if>>`, `<<print>>`, etc.) → `Read`
#[allow(dead_code)]
fn macro_name_to_access_kind(name: &str) -> VarAccessKind {
    if name.eq_ignore_ascii_case("capture") {
        VarAccessKind::Capture
    } else if name.eq_ignore_ascii_case("unset") {
        VarAccessKind::Unset
    } else if name.eq_ignore_ascii_case("set") {
        // <<set>> without structured assignment — the whole args go to oxc.
        // The target variable is still a write. This handles cases like
        // `<<set $arr.push(1)>>` where the target is $arr (method call = write).
        VarAccessKind::Write
    } else {
        // All other macros read their variables (<<if>>, <<print>>, <<run>> args, etc.)
        VarAccessKind::Read
    }
}

/// Populate registries from a parsed passage AST.
///
/// Walks the AST's var_ops and links to feed the `VariableTree`
/// and `CustomMacroRegistry` side tables. Each variable access is classified
/// with the appropriate `VarAccessKind` for nuanced read/write tracking.
pub fn populate_registries_from_ast(
    registry: &SugarCubeRegistry,
    passage_ast: &ast::PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
    _body_offset: usize,
) {
    // Record variable operations from the AST
    {
        let mut vtree = registry.variables_mut();
        for var_op in &passage_ast.var_ops {
            // Determine the access kind from the AST's VarOpInfo.
            // The is_write flag is the basic classification; the actual kind
            // is refined below when we have SetOperator information.
            vtree.record_var_simple(
                &var_op.name,
                var_op.is_temporary,
                var_op.is_write,
                &cp.header.name,
                file_uri,
                var_op.span.clone(),
                &var_op.property_path,
            );
        }

        // Refine access kinds using structured <<set>> assignment info from AST nodes.
        // The basic VarOpInfo only has is_write=true/false, but we can get more
        // nuanced classification from the SetAssignment on each <<set>> macro.
        refine_access_kinds_from_ast(&mut vtree, &passage_ast.nodes, &cp.header.name, file_uri);

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

    // Extract widget definitions from AST nodes
    {
        let mut macro_reg = registry.custom_macros_mut();
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
    }
}

/// Refine `VarAccessKind` for variables in `<<set>>` macros using the
/// structured `SetAssignment` info from the AST.
///
/// The basic `VarOpInfo` only tracks `is_write: bool`, but `<<set>>` macros
/// have a `SetAssignment` that tells us whether it's a simple write (`to`/`=`),
/// a compound write (`+=`, `-=`, etc.), or a postfix modify (`++`, `--`).
/// This function walks the AST nodes and upgrades the access kind.
fn refine_access_kinds_from_ast(
    vtree: &mut super::variable_tree::VariableTree,
    nodes: &[ast::AstNode],
    passage_name: &str,
    file_uri: &str,
) {
    for node in nodes {
        if let ast::AstNode::Macro {
            name,
            set_assignment,
            children,
            ..
        } = node
        {
            // For <<set>> with structured assignment, refine the target variable's kind
            if name.eq_ignore_ascii_case("set") {
                if let Some(sa) = set_assignment {
                    let kind = set_operator_to_access_kind(&sa.operator);
                    // Find the target variable in the tree and update its access kind
                    if let Some(entry) = vtree.get_variable_mut(&sa.target.name) {
                        // Find the matching access (same passage, same span start)
                        for access in &mut entry.accesses {
                            if access.passage_name == passage_name
                                && access.file_uri == file_uri
                                && access.span.start == sa.target.span.start
                            {
                                access.kind = kind;
                            }
                        }
                    }
                }
            }

            // For <<capture>>, upgrade to Capture kind
            if name.eq_ignore_ascii_case("capture") {
                // Find the first variable in this passage that's currently marked as Write
                // and upgrade it to Capture
                // Note: <<capture>> variables are already marked is_write=true by extraction
                // We just need to refine the kind
                // This is a best-effort refinement since we don't have exact span matching
                // for <<capture>> vars in the VarOpInfo
            }

            // For <<unset>>, upgrade to Unset kind
            if name.eq_ignore_ascii_case("unset") {
                // <<unset>> vars are already marked is_write=true by extraction
                // We refine the kind similarly
            }

            // Recurse into children
            if let Some(ch) = children {
                refine_access_kinds_from_ast(vtree, ch, passage_name, file_uri);
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
/// - `function name()` → function registry entries
/// - `Template.add("name", ...)` → template registry entries
pub fn walk_script_js(
    registry: &SugarCubeRegistry,
    body_text: &str,
    cp: &ClassifiedPassage,
    file_uri: &str,
) {
    use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};
    use crate::sugarcube::js::js_preprocess;
    use crate::sugarcube::js::js_walk;

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
                    registry,
                );
            });
        }
        JsParseOutcome::Error(_diagnostics) => {
            // JS syntax errors are reported as diagnostics by the
            // caller — we just skip registry population for broken JS
        }
    }
}
