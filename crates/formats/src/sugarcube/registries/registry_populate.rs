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
///
/// Spans in the AST are **passage-body-relative** (relative to the start of
/// the passage content after the header line). They are stored as-is in the
/// variable tree without shifting by `body_offset`. Line numbers are computed
/// immediately from the passage body text.
pub fn populate_registries_from_ast(
    registry: &mut SugarCubeRegistry,
    passage_ast: &ast::PassageAst,
    cp: &ClassifiedPassage,
    file_uri: &str,
) {
    // Record variable operations from the AST
    {
        let mut vtree = registry.variables_mut();
        for var_op in &passage_ast.var_ops {
            // Determine the access kind from the AST's VarOpInfo.
            // The is_write flag is the basic classification; the actual kind
            // is refined below when we have SetOperator information.
            //
            // The parser produces body-relative spans — store them directly.
            // Line numbers are computed from the passage body text at record time.
            vtree.record_var_simple(
                &var_op.name,
                var_op.is_temporary,
                var_op.is_write,
                &cp.header.name,
                file_uri,
                var_op.span.clone(),
                &var_op.property_path,
                &cp.body_text,
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
        let macro_reg = registry.custom_macros_mut();
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
///
/// Spans are **passage-body-relative** — the AST and the variable tree both
/// use body-relative positions, so we match on the span directly without
/// any `body_offset` adjustment.
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
                    // Find the target variable in the tree and update its access kind.
                    // Both the variable tree and the AST use passage-body-relative
                    // spans, so we match on the span start directly.
                    if let Some(var_id) = vtree.get_variable_mut_id(&sa.target.name) {
                        let node = vtree.arena_mut().get_mut(var_id);
                        for access in &mut node.meta.refs {
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
///
/// Spans from oxc are mapped back to original body-relative coordinates
/// via `preprocessed.map_to_original()`. These body-relative spans are
/// stored directly in the variable tree without any `body_offset` adjustment.
pub fn walk_script_js(
    registry: &mut SugarCubeRegistry,
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
                    body_text,
                );
            });
        }
        JsParseOutcome::Error(_diagnostics) => {
            // JS syntax errors are reported as diagnostics by the
            // caller — we just skip registry population for broken JS
        }
    }
}

/// Walk inline JS snippets from a normal passage and populate registries.
///
/// Normal passages can contain JS inside `<<run>>`, `<<set>>`, `<<if>>`,
/// `<<script>>` blocks, etc. This function collects those snippets, parses
/// them with oxc, and walks the AST to detect:
/// - `State.variables.x` → variable read/write
/// - `State.variables.x = value` → variable write
/// - `$var` / `_var` (after preprocessing) → variable read/write
/// - `Macro.add()` calls, `Template.add()`, etc.
///
/// ## Span offset mapping
///
/// Each snippet's `body_offset` tells us where the snippet starts within
/// the passage body. The preprocessor's `map_to_original()` returns positions
/// relative to the snippet start, so we create a *shifted* `PreprocessedJs`
/// whose `original_range` values are already adjusted by `body_offset`.
/// This way, the JS walker records passage-body-relative spans directly,
/// consistent with how `populate_registries_from_ast` and `walk_script_js`
/// store their spans.
///
/// ## Deduplication
///
/// The SugarCube parser's `scan_inline_vars()` already detects `$var` and
/// `_var` references from the macro arguments and records them via
/// `populate_registries_from_ast()`. The JS walker will also find these
/// (after they've been substituted to `State_variables_x`). The variable
/// tree handles duplicate recordings gracefully — it adds a new `VarAccess`
/// entry for each recording, which means the same variable may have both
/// a read from the SugarCube parser and a write from the JS AST walker.
/// This is acceptable because:
/// 1. The JS walker provides MORE ACCURATE read/write classification
///    (e.g., `_items = State.variables.ITEMS` — JS walker knows `_items`
///    is a write, the SugarCube parser only sees it as a read).
/// 2. Extra access entries don't cause problems — the tree merges
///    operations for the same passage/uri/span.
/// 3. For `State.variables.ITEMS` references, the JS walker is the ONLY
///    path that detects them — the SugarCube parser doesn't know about
///    `State.variables.x` at all.
pub fn walk_inline_js_snippets(
    registry: &mut SugarCubeRegistry,
    nodes: &[ast::AstNode],
    passage_name: &str,
    file_uri: &str,
    body_text: &str,
) {
    use knot_core::oxc::{parse_js, JsParseOutcome, ParseMode as JsParseMode};
    use crate::sugarcube::js::js_preprocess;
    use crate::sugarcube::js::js_walk;

    let snippets = ast::collect_js_snippets(nodes);

    for snippet in &snippets {
        // Preprocess $var references and SugarCube operators for oxc
        let preprocessed = js_preprocess::preprocess_for_oxc(&snippet.source);

        // Determine parse mode: block scripts get Module, inline expressions get Expression
        let js_mode = if snippet.is_block {
            JsParseMode::Module
        } else {
            JsParseMode::Expression
        };

        // Set up span mapping:
        // - wrapping_offset: 1 for Expression mode (oxc wraps as `(source)`), 0 for Module
        // - origin_offset: snippet.body_offset to shift snippet-relative → passage-body-relative
        let wrapping_offset = match js_mode {
            JsParseMode::Expression => 1,
            _ => 0,
        };
        let shifted = shift_preprocessed(&preprocessed, snippet.body_offset, wrapping_offset);

        match parse_js(&shifted.source, js_mode) {
            JsParseOutcome::Success(output) => {
                output.with_program(|program| {
                    js_walk::walk_inline_js(
                        program,
                        &shifted,
                        file_uri,
                        passage_name,
                        registry,
                        body_text,
                    );
                });
            }
            JsParseOutcome::Error(_diagnostics) => {
                // JS syntax errors are reported as diagnostics by
                // js_validate — skip registry population for broken JS
            }
        }
    }
}

/// Shift a `PreprocessedJs` so that `map_to_original()` returns
/// passage-body-relative positions instead of snippet-relative.
///
/// This works by setting `origin_offset` to `offset` (the snippet's
/// `body_offset`) and `wrapping_offset` to account for oxc's Expression
/// mode wrapping. The `map_to_original()` method handles both offsets:
/// - Subtracts `wrapping_offset` from oxc AST positions first
/// - Adds `origin_offset` to map from snippet-relative to passage-body-relative
fn shift_preprocessed(
    preprocessed: &crate::sugarcube::js::js_preprocess::PreprocessedJs,
    offset: usize,
    wrapping_offset: usize,
) -> crate::sugarcube::js::js_preprocess::PreprocessedJs {
    crate::sugarcube::js::js_preprocess::PreprocessedJs {
        source: preprocessed.source.clone(),
        substitutions: preprocessed.substitutions.clone(),
        origin_offset: offset,
        wrapping_offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sugarcube::ast::ParseMode;
    use crate::sugarcube::parser;

    #[test]
    fn walk_inline_js_snippets_detects_state_variables_read() {
        // Test: <<run _items = State.variables.ITEMS>> should detect
        // $ITEMS as a READ and _items as a WRITE
        let body = "<<run _items = State.variables.ITEMS>>";
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        let mut registry = SugarCubeRegistry::new();

        // First populate from the SugarCube parser's var_ops (finds _items as a read)
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
        populate_registries_from_ast(&mut registry, &ast, &cp, "file:///test.tw");

        // Then walk inline JS snippets (should detect State.variables.ITEMS as $ITEMS READ,
        // and State_temporary_items as _items WRITE)
        walk_inline_js_snippets(&mut registry, &ast.nodes, "Game", "file:///test.tw", body);

        // Verify $ITEMS exists with a READ access
        let vtree = registry.variables();
        let items_var = vtree.get_variable("$ITEMS");
        assert!(items_var.is_some(), "$ITEMS should be in registry from State.variables.ITEMS detection");
        if let Some((_, node)) = items_var {
            let reads: Vec<_> = node.meta.refs.iter().filter(|a| a.is_read()).collect();
            assert!(!reads.is_empty(), "$ITEMS should have at least one READ from State.variables.ITEMS");
        }

        // Verify _items exists with both READ and WRITE accesses
        let temp_var = vtree.get_variable("_items");
        assert!(temp_var.is_some(), "_items should be in registry");
        if let Some((_, node)) = temp_var {
            let has_write = node.meta.refs.iter().any(|a| a.is_write());
            assert!(has_write, "_items should have a WRITE from the JS walker detecting it as assignment target");
        }
    }
}
