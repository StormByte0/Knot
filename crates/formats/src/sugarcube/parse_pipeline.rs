//! Full parse pipeline orchestration.
//!
//! Contains the two main entry points that were previously the bodies of
//! `FormatPluginMut::parse_mut()` and `FormatPluginMut::parse_passage_mut()`. The trait
//! impl in `mod.rs` delegates to the free functions here.
//!
//! ## 3-Phase Pipeline
//!
//! Each passage goes through three phases:
//!
//! 1. **Phase 1 — Structural parse**: SugarCube parser produces AST with
//!    `js_analysis: None` on all nodes.
//!
//! 2. **Phase 2 — JS annotation**: `js_annotate::annotate_js()` walks the AST,
//!    finds nodes with JS content, preprocesses + parses with oxc, and attaches
//!    `JsAnalysis` to each node. For script passages, analysis is stored on
//!    `PassageAst::script_js_analysis`.
//!
//! 3. **Phase 3 — Unified registry population**: `populate_registries_from_unified_ast()`
//!    walks the enriched AST once and populates all registries from `JsAnalysis`
//!    (with SugarCube semantic overrides applied).

use knot_core::passage::PassageCategory as CorePassageCategory;
use url::Url;

use super::SugarCubePlugin;
use super::ast::ParseMode;
use super::classifier::{self, ClassifiedPassage, is_script_passage};
use super::lsp::pipeline_log;
use super::passage_build;
use super::registry_populate;
use crate::plugin::{FormatPlugin, ParseResult, PassageDiagnosticGroup, PassageTokenGroup};

/// Full parse: split → classify → sort → per-passage dispatch.
///
/// This is the body of `FormatPluginMut::parse_mut()` for `SugarCubePlugin`.
pub(super) fn parse_full(plugin: &mut SugarCubePlugin, uri: &Url, text: &str) -> ParseResult {
    let registry = plugin.registry_mut();

    // 1. Split into raw passages
    let raw_passages = super::lexer::split_passages(text);

    // 2. Classify each passage
    let mut classified = classifier::classify_all(&raw_passages, uri.as_ref());

    // 3. Sort by processing priority (scripts first, normal last)
    classifier::sort_for_processing(&mut classified);

    // 4. Clear registries for this file before re-populating
    registry.remove_file(uri.as_ref());

    // 5. Process each passage in order
    let mut passages = Vec::new();
    let mut all_token_groups = Vec::new();
    let mut all_diagnostic_groups = Vec::new();

    for cp in &classified {
        let mode = SugarCubePlugin::parse_mode_for(cp);

        // Compute where the body starts in the document (after header line + newline)
        let header_line_end = text[cp.header.header_start..]
            .find('\n')
            .map_or(text.len(), |pos| cp.header.header_start + pos + 1);
        let body_offset = header_line_end;

        // The passage head is at header_start (the `::` prefix).
        // body_offset_in_passage is the offset from the passage head to the
        // body start — used to convert body-relative AST spans to
        // passage-relative token offsets.
        let passage_head = cp.header.header_start;
        let body_offset_in_passage = body_offset - passage_head;

        // Phase 1: Structural parse (SugarCube parser)
        pipeline_log::parse_phase1_enter(&cp.header.name, uri.as_ref());
        let mut passage_ast = super::parser::parse_passage_body(&cp.body_text, body_offset, mode);
        pipeline_log::parse_phase1_exit(&cp.header.name, passage_ast.nodes.len());

        // Phase 1b: Build the unified zone map (zoning engine).
        // All spans are passage-relative (shifted by body_offset_in_passage).
        // The custom macro registry is already populated from earlier [widget]
        // and [script] passages (sort-for-processing order).
        if matches!(
            mode,
            ParseMode::Normal | ParseMode::Widget | ParseMode::Interface
        ) {
            passage_ast.zones = crate::zoning::build_from_ast(
                &passage_ast.nodes,
                body_offset_in_passage,
                registry.custom_macros(),
            );
        }

        // Phase 2: JS annotation pass — attach JsAnalysis to AST nodes
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            let js_node_count = count_js_nodes(&passage_ast.nodes);
            pipeline_log::parse_phase2_enter(&cp.header.name, js_node_count);
            // Build the set of all known macro names (builtin + custom) so
            // the fallback in collect_macro_js_snippet doesn't send custom
            // widget args to oxc.
            let mut known_macro_names = registry
                .custom_macro_names()
                .into_iter()
                .collect::<std::collections::HashSet<String>>();
            for m in super::macros::builtin_macros() {
                known_macro_names.insert(m.name.to_string());
            }
            super::js::js_annotate::annotate_js(
                &mut passage_ast,
                &cp.body_text,
                is_script_passage(cp),
                // For Twee passages, SugarCube syntax ($var, keyword operators)
                // is always allowed — even in [script] passages, authors can
                // use $var references. The preprocessor handles the
                // $var → State.variables.var substitution.
                true,
                &known_macro_names,
            );
            let total_var_ops = count_total_var_ops(&passage_ast);
            pipeline_log::parse_phase2_exit(&cp.header.name, total_var_ops);
        }

        // Phase 3: Unified registry population (single walk over unified AST)
        pipeline_log::parse_phase3_enter(&cp.header.name, uri.as_ref());
        registry_populate::populate_registries_from_unified_ast(
            registry,
            &passage_ast,
            cp,
            uri.as_ref(),
            body_offset_in_passage,
        );
        pipeline_log::parse_phase3_exit(
            &cp.header.name,
            registry.variables().path_index_len(),
            registry.custom_macros().len(),
            registry.functions().len(),
            registry.templates().len(),
        );

        // Build the Passage struct (passage-relative spans via body_offset_in_passage)
        let mut passage =
            passage_build::build_passage(cp, &passage_ast, body_offset_in_passage, passage_head);
        passage.span = 0..(header_line_end - passage_head + cp.body_text.len());
        passage.passage_offset = passage_head;

        // Build passage.vars from the unified AST (including js_analysis + script_js_analysis)
        passage.vars =
            passage_build::build_vars_from_unified_ast(&passage_ast, body_offset_in_passage);

        // Build semantic tokens for this passage (passage-relative offsets).
        // The passage_offset is the document-absolute position of the `::`
        // prefix, applied at the LSP boundary to produce document-absolute
        // positions from the passage-relative token offsets.
        let mut passage_tokens = Vec::new();
        let is_special = cp.special_def.is_some();

        // Header tokens (already passage-relative from build_header_tokens)
        let header_tokens = super::token_builder::build_header_tokens(&cp.header, is_special);
        passage_tokens.extend(header_tokens);

        // Body tokens (shift body-relative AST spans by body_offset_in_passage)
        if matches!(mode, ParseMode::Minimal) {
            let json_tokens =
                super::token_builder::build_json_body_tokens(&cp.body_text, body_offset_in_passage);
            passage_tokens.extend(json_tokens);
        } else if matches!(mode, ParseMode::Stylesheet) {
            // Stylesheet passages are pure CSS (plan.md Phase 4.2): the
            // whole body tokenizes through the knot-core CSS service. CSS
            // diagnostics are intentionally NOT raised as LSP diagnostics —
            // upstream SugarCube is silent about broken CSS (styleTag hands
            // the text to the browser), so this is highlighting-only.
            let css_analysis = crate::sugarcube::css::analyze_css(&cp.body_text);
            for tok in css_analysis.tokens {
                passage_tokens.push(crate::plugin::SemanticToken {
                    start: body_offset_in_passage + tok.start,
                    length: tok.length,
                    token_type: tok.token_type,
                    modifier: tok.modifier,
                });
            }
        } else if matches!(mode, ParseMode::Interface) {
            // StoryInterface body is HTML. Token serving for it is still a
            // follow-up (plan.md 2.4/4.1 worklog note) — no tokens emitted,
            // no diagnostic raised.
        } else {
            // Collect custom macro names for Function token differentiation
            let custom_names: std::collections::HashSet<String> =
                registry.custom_macros().names().cloned().collect();
            super::token_builder::build_semantic_tokens(
                &passage_ast.nodes,
                &mut passage_tokens,
                body_offset_in_passage,
                &custom_names,
                &cp.body_text,
            );
            // For script passages, also emit tokens from script_js_analysis
            if let Some(ref analysis) = passage_ast.script_js_analysis {
                super::token_builder::build_script_passage_tokens(
                    analysis,
                    &mut passage_tokens,
                    body_offset_in_passage,
                );
            }
        }

        all_token_groups.push(PassageTokenGroup {
            passage_name: cp.header.name.clone(),
            passage_offset: passage_head,
            tokens: passage_tokens,
        });

        // Build diagnostics from the body AST (passage-relative offsets).
        // The passage_offset is applied at the LSP boundary to produce
        // document-absolute ranges — same pattern as semantic tokens.
        let mut passage_diagnostics = Vec::new();
        super::token_builder::build_diagnostics(
            &passage_ast.nodes,
            &mut passage_diagnostics,
            body_offset_in_passage,
            registry.custom_macros(),
        );

        // Validate inline JS snippets via oxc (for diagnostics only)
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            if matches!(mode, ParseMode::Script) {
                // [script] passages: validate the entire body as a JS module.
                // validate_inline_js only walks AST nodes for <<script>> block
                // macros, which doesn't cover [script] passages (their entire
                // body is JS, no macro wrapper).
                //
                // Task 1 (optimization): pass the pre-collected diagnostics
                // from `annotate_js` (stored on `script_js_analysis`) so
                // `validate_script_passage` doesn't re-parse the same source.
                let script_diagnostics = passage_ast
                    .script_js_analysis
                    .as_ref()
                    .map(|a| a.diagnostics.as_slice())
                    .unwrap_or(&[]);
                let js_diagnostics = super::js_validate::validate_script_passage(
                    &cp.body_text,
                    body_offset_in_passage,
                    // Twee [script] passages use SugarCube syntax
                    true,
                    script_diagnostics,
                );
                passage_diagnostics.extend(js_diagnostics);
            } else {
                // Normal passages: validate inline <<script>>/<<set>>/<<run>> snippets.
                //
                // Build the set of all known macro names (builtin catalog +
                // custom macros from the registry) so the fallback in
                // `collect_js_snippets` doesn't send custom widget args
                // (e.g. `<<statblock "Strength" $stats.strength>>`) to oxc.
                let mut known_macro_names = registry
                    .custom_macro_names()
                    .into_iter()
                    .collect::<std::collections::HashSet<String>>();
                for m in super::macros::builtin_macros() {
                    known_macro_names.insert(m.name.to_string());
                }
                let js_diagnostics = super::js_validate::validate_inline_js(
                    &passage_ast.nodes,
                    body_offset_in_passage,
                    &known_macro_names,
                );
                passage_diagnostics.extend(js_diagnostics);
            }
        }

        all_diagnostic_groups.push(PassageDiagnosticGroup {
            passage_name: cp.header.name.clone(),
            passage_offset: passage_head,
            diagnostics: passage_diagnostics,
        });

        passages.push(passage);
    }

    // Note: Custom macro (widget) invocations are NOT added as passage links.
    // They are function calls, not passage navigation — "go to definition"
    // works through the custom_macros registry, not through graph edges.
    // Adding them as links would cause false "BrokenLink" diagnostics since
    // the macro name is not a passage name.

    // Restore source order in the stored passages.
    //
    // `classifier::sort_for_processing` above reordered `classified` by
    // processing priority (scripts/init first, then specials, then regulars)
    // so that define-before-use dependencies are honored during registry
    // population. That order is no longer needed once parsing is complete —
    // the registries are already populated. Every downstream consumer of
    // `doc.passages` (document_symbol, folding_range, selection_range,
    // navigation, diagnostics, etc.) expects **source order**, and several
    // of them compute `passages[i].start .. passages[i+1].start` which
    // only produces well-formed ranges when the slice is in source order.
    //
    // `passage_offset` is the document-absolute byte offset of the `::`
    // header, so sorting by it restores on-disk order. The sort is stable,
    // so passages with identical offsets (shouldn't happen, but defensive)
    // retain their relative order.
    passages.sort_by_key(|p| p.passage_offset);

    pipeline_log::parse_full_summary(
        uri.as_ref(),
        passages.len(),
        all_token_groups
            .iter()
            .map(|g| g.tokens.len())
            .sum::<usize>(),
        all_diagnostic_groups
            .iter()
            .map(|g| g.diagnostics.len())
            .sum::<usize>(),
    );

    ParseResult {
        passages,
        token_groups: all_token_groups,
        diagnostic_groups: all_diagnostic_groups,
        is_complete: true,
    }
}

/// Count AST nodes that have JS content (Macro or Expression nodes).
fn count_js_nodes(nodes: &[super::ast::AstNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        match node {
            super::ast::AstNode::Macro { children, .. } => {
                count += 1;
                if let Some(ch) = children {
                    count += count_js_nodes(ch);
                }
            }
            super::ast::AstNode::Expression { .. } => {
                count += 1;
            }
            _ => {}
        }
    }
    count
}

/// Count total var_ops across all AST nodes that have js_analysis.
fn count_total_var_ops(ast: &super::ast::PassageAst) -> usize {
    let mut total = 0;
    // Script-level analysis
    if let Some(analysis) = &ast.script_js_analysis {
        total += analysis.var_ops.len();
    }
    // Node-level analysis
    fn count_in_nodes(nodes: &[super::ast::AstNode]) -> usize {
        let mut n = 0;
        for node in nodes {
            match node {
                super::ast::AstNode::Macro {
                    js_analysis,
                    children,
                    ..
                } => {
                    if let Some(analysis) = js_analysis {
                        n += analysis.var_ops.len();
                    }
                    if let Some(ch) = children {
                        n += count_in_nodes(ch);
                    }
                }
                super::ast::AstNode::Expression {
                    js_analysis: Some(analysis),
                    ..
                } => {
                    n += analysis.var_ops.len();
                }
                _ => {}
            }
        }
        n
    }
    total += count_in_nodes(&ast.nodes);
    total
}

/// Incremental re-parse of a single passage.
///
/// This is the body of `FormatPluginMut::parse_passage_mut()` for `SugarCubePlugin`.
pub fn parse_single(
    plugin: &mut SugarCubePlugin,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
    file_uri: &str,
    passage_offset: usize,
) -> Option<crate::plugin::ParseResult> {
    // Determine the parse mode from the tags
    let mode = if passage_tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("script"))
    {
        ParseMode::Script
    } else if passage_tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("stylesheet") || t.eq_ignore_ascii_case("style"))
    {
        ParseMode::Stylesheet
    } else if passage_tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("widget"))
    {
        ParseMode::Widget
    } else if passage_name == "StoryInterface" {
        ParseMode::Interface
    } else if passage_name == "StoryData" {
        ParseMode::Minimal
    } else {
        ParseMode::Normal
    };

    // Strip the passage header line before parsing.
    //
    // `passage_text` (from `did_change_incremental`) starts at the passage's
    // `::` prefix and includes the header line (e.g. `:: DomMacros [docs dom]\n`).
    // But `parse_passage_body` expects BODY text only (after the header).
    //
    // If we pass the full text including `::`, the parser treats `::` as the
    // first 2 bytes of prose, shifting ALL zone spans by the header length.
    // This causes the formatter to slice wrong portions of the body, producing
    // cascading corruption on repeated format calls.
    //
    // We find the header line end (the first `\n`) and pass only the body.
    // The `body_offset_in_passage` is set to the header length so zone spans
    // are correctly relative to the passage head (`::`), matching what
    // `parse_full` produces.
    //
    // **Body-only fallback**: If `passage_text` does NOT start with `::`,
    // treat the entire text as body (body_offset = 0). This supports callers
    // that pass body-only text (e.g. unit tests, direct API usage).
    let (body_text, body_offset_in_passage) = if passage_text.starts_with("::") {
        let header_line_end = passage_text
            .find('\n')
            .map(|pos| pos + 1)
            .unwrap_or(passage_text.len());
        (&passage_text[header_line_end..], header_line_end)
    } else {
        (passage_text, 0)
    };

    // Phase 1: Structural parse — parse BODY text only (no :: header)
    let mut passage_ast =
        super::parser::parse_passage_body(body_text, body_offset_in_passage, mode);

    // Phase 1b: Build the unified zone map (zoning engine).
    // body_offset_in_passage shifts zone spans from body-relative to
    // passage-relative (0 = `::`), matching what `parse_full` produces.
    if matches!(
        mode,
        ParseMode::Normal | ParseMode::Widget | ParseMode::Interface
    ) {
        let registry = plugin.registry();
        passage_ast.zones = crate::zoning::build_from_ast(
            &passage_ast.nodes,
            body_offset_in_passage,
            registry.custom_macros(),
        );
    }

    // Phase 2: JS annotation pass
    if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
        let registry = plugin.registry();
        let mut known_macro_names = registry
            .custom_macro_names()
            .into_iter()
            .collect::<std::collections::HashSet<String>>();
        for m in super::macros::builtin_macros() {
            known_macro_names.insert(m.name.to_string());
        }
        super::js::js_annotate::annotate_js(
            &mut passage_ast,
            body_text,
            mode == ParseMode::Script,
            // Incremental re-parse of a single Twee passage — SugarCube syntax
            // is always allowed.
            true,
            &known_macro_names,
        );
    }

    // Classify the passage BEFORE mutably borrowing the registry.
    let (_, category) = plugin.classify_passage_category(passage_name, passage_tags);
    let is_special = category != CorePassageCategory::Regular;

    let special_def = if is_special {
        plugin.classify_passage(passage_name, passage_tags)
    } else {
        None
    };

    // If this was supposed to be special but classify_passage returned None, bail.
    if is_special && special_def.is_none() {
        return None;
    }

    // Build ClassifiedPassage for build_passage and registry population.
    //
    // parse_single works with isolated passage text (no document context),
    // so header_start = 0. But name_start MUST be re-parsed from the actual
    // header line — it determines where the passage-name semantic token is
    // placed. For `::Name` (no space), name_start = 2; for `:: Name` (with
    // space), name_start = 3.
    //
    // Previously this was hardcoded to 0, which placed the name token at the
    // same offset as the `::` prefix token (offset 0). The two overlapping
    // tokens caused VS Code to swallow the name token — the passage name
    // lost its coloring during incremental edits (WithinPassage edits that
    // don't trigger full re-parse). On server restart, the full parse path
    // (parse_full) correctly set name_start via parse_twee_header, which is
    // why coloring worked on restart but broke on edit.
    let header = if passage_text.starts_with("::") {
        // Re-parse the header line to get accurate name_start, tags, and
        // metadata. passage_text starts at the passage head (::), so
        // header_start = 0 (passage-relative).
        let header_line_end = passage_text.find('\n').unwrap_or(passage_text.len());
        let header_line = &passage_text[..header_line_end];
        match crate::header::parse_twee_header(header_line, 0) {
            Some(parsed) => parsed,
            None => {
                // Header didn't parse — fall back to the old hardcoded
                // values. This is defensive; parse_twee_header should
                // always succeed for a valid passage (the caller already
                // verified the header via did_change_incremental's
                // header-stability check).
                crate::header::TweeHeader {
                    name: passage_name.to_string(),
                    tags: passage_tags.to_vec(),
                    header_start: 0,
                    name_start: 0,
                    metadata_json: None,
                    name_text_raw: passage_name.to_string(),
                    tags_raw: String::new(),
                }
            }
        }
    } else {
        // Body-only text (no :: prefix) — caller passed body text directly.
        // Use the old hardcoded values.
        crate::header::TweeHeader {
            name: passage_name.to_string(),
            tags: passage_tags.to_vec(),
            header_start: 0,
            name_start: 0,
            metadata_json: None,
            name_text_raw: passage_name.to_string(),
            tags_raw: String::new(),
        }
    };
    let cp = ClassifiedPassage {
        header,
        body_text: body_text.to_string(),
        file_uri: file_uri.to_string(),
        category: classifier::PassageCategory::Regular,
        special_def,
        processing_priority: 40,
    };

    // Build the Passage struct (passage-relative spans, passage_head = 0).
    // body_offset_in_passage = header_line_end (the offset of the body
    // within the passage, after the `:: Name` header line).
    let mut passage = passage_build::build_passage(&cp, &passage_ast, body_offset_in_passage, 0);
    passage.span = 0..passage_text.len();
    // Set the document-absolute passage_offset from the caller's param.
    // All other spans (links, vars, blocks) remain passage-relative (0 = `::`).
    passage.passage_offset = passage_offset;

    // header_name_span: in parse_single, header_start=0, name_start=0,
    // so the span is already passage-relative.
    passage.header_name_span =
        Some(cp.header.name_start..cp.header.name_start + cp.header.name.len());

    // Override vars with unified AST vars (includes js_analysis + script_js_analysis)
    passage.vars = passage_build::build_vars_from_unified_ast(&passage_ast, body_offset_in_passage);

    // Phase 3: Registry mutation — clear old passage data, then populate.
    {
        let registry = plugin.registry_mut();
        registry.remove_passage(passage_name);

        registry_populate::populate_registries_from_unified_ast(
            registry,
            &passage_ast,
            &cp,
            file_uri,
            body_offset_in_passage,
        );
    }

    // ── Build semantic tokens (same per-passage logic as parse_full) ──
    let mut passage_tokens = Vec::new();
    let is_special = cp.special_def.is_some();

    // Header tokens (already passage-relative from build_header_tokens)
    let header_tokens = super::token_builder::build_header_tokens(&cp.header, is_special);
    passage_tokens.extend(header_tokens);

    // Body tokens (shift body-relative AST spans by body_offset_in_passage)
    if matches!(mode, ParseMode::Minimal) {
        let json_tokens =
            super::token_builder::build_json_body_tokens(&cp.body_text, body_offset_in_passage);
        passage_tokens.extend(json_tokens);
    } else if matches!(mode, ParseMode::Stylesheet) {
        // Stylesheet passages are pure CSS (plan.md Phase 4.2) — same as
        // the full-parse path above: highlighting-only, no diagnostics.
        let css_analysis = crate::sugarcube::css::analyze_css(&cp.body_text);
        for tok in css_analysis.tokens {
            passage_tokens.push(crate::plugin::SemanticToken {
                start: body_offset_in_passage + tok.start,
                length: tok.length,
                token_type: tok.token_type,
                modifier: tok.modifier,
            });
        }
    } else if matches!(mode, ParseMode::Interface) {
        // StoryInterface body is HTML. Token serving still a follow-up —
        // no tokens emitted.
    } else {
        // Collect custom macro names for Function token differentiation
        let registry = plugin.registry();
        let custom_names: std::collections::HashSet<String> =
            registry.custom_macros().names().cloned().collect();
        super::token_builder::build_semantic_tokens(
            &passage_ast.nodes,
            &mut passage_tokens,
            body_offset_in_passage,
            &custom_names,
            &cp.body_text,
        );
        // For script passages, also emit tokens from script_js_analysis
        if let Some(ref analysis) = passage_ast.script_js_analysis {
            super::token_builder::build_script_passage_tokens(
                analysis,
                &mut passage_tokens,
                body_offset_in_passage,
            );
        }
    }

    let token_group = crate::plugin::PassageTokenGroup {
        passage_name: passage_name.to_string(),
        passage_offset,
        tokens: passage_tokens,
    };

    // ── Build diagnostics (same per-passage logic as parse_full) ──
    let mut passage_diagnostics = Vec::new();
    {
        let registry = plugin.registry();
        super::token_builder::build_diagnostics(
            &passage_ast.nodes,
            &mut passage_diagnostics,
            body_offset_in_passage,
            registry.custom_macros(),
        );
    }

    // Validate inline JS snippets via oxc (for diagnostics only)
    if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
        if matches!(mode, ParseMode::Script) {
            // Task 1 (optimization): pass pre-collected diagnostics from
            // `annotate_js` to avoid re-parsing the same JS source.
            let script_diagnostics = passage_ast
                .script_js_analysis
                .as_ref()
                .map(|a| a.diagnostics.as_slice())
                .unwrap_or(&[]);
            let js_diagnostics = super::js_validate::validate_script_passage(
                &cp.body_text,
                body_offset_in_passage,
                // Twee [script] passages use SugarCube syntax
                true,
                script_diagnostics,
            );
            passage_diagnostics.extend(js_diagnostics);
        } else {
            let known_macro_names = {
                let registry = plugin.registry();
                let mut names: std::collections::HashSet<String> =
                    registry.custom_macro_names().into_iter().collect();
                for m in super::macros::builtin_macros() {
                    names.insert(m.name.to_string());
                }
                names
            };
            let js_diagnostics = super::js_validate::validate_inline_js(
                &passage_ast.nodes,
                body_offset_in_passage,
                &known_macro_names,
            );
            passage_diagnostics.extend(js_diagnostics);
        }
    }

    let diagnostic_group = crate::plugin::PassageDiagnosticGroup {
        passage_name: passage_name.to_string(),
        passage_offset,
        diagnostics: passage_diagnostics,
    };

    Some(crate::plugin::ParseResult {
        passages: vec![passage],
        token_groups: vec![token_group],
        diagnostic_groups: vec![diagnostic_group],
        is_complete: true,
    })
}

/// Parse a standalone `.js` file as a synthetic script passage.
///
/// Tweego bundles `.js` files from the source directory into the compiled
/// HTML as `<script>` tags — they run at startup, before any passage. This
/// function gives Knot the same view of those files: it builds a synthetic
/// `ClassifiedPassage` with the `[script]` special-def, runs the full
/// JS analysis pipeline (annotate + validate + registry populate), and
/// returns a `ParseResult` with a single `Passage`.
///
/// ## Identical to `[script]` passages
///
/// The JS analysis behavior is **identical** to `[script]`-tagged passages.
/// The only difference is where the text comes from:
///
/// - `[script]` passage: the passage body (after the `:: Name [script]\n`
///   header line) is passed to oxc
/// - `.js` file: the entire file contents are passed to oxc
///
/// This means:
/// - The SugarCube preprocessor runs (`$var` → `State.variables.var`,
///   keyword operators `to`/`is`/`eq` → JS equivalents) — same as
///   `[script]` passages
/// - `Macro.add()`, `Template.add()`, `function` declarations, and
///   `State.variables` writes are registered in the workspace registries
/// - JS tokens are emitted for syntax highlighting
/// - JS diagnostics are produced from oxc
///
/// ## Differences from `parse_full` (structural only)
///
/// - No `lexer::split_passages()` (no `::` headers in a `.js` file)
/// - No `classifier::classify_all()` (we hard-code the script category)
/// - `body_offset_in_passage = 0` and `passage_offset = 0` (the entire
///   file IS the passage body — no `::` header)
/// - `header_name_span = None` (no header to select)
///
/// ## Passage name
///
/// The passage name is the file stem (e.g., `story` for `story.js`). This
/// matches how Tweego names the injected script tag. It's only used for
/// display in the outline and for registry origin tracking — it's not a
/// link target.
pub(super) fn parse_script_file(
    plugin: &mut SugarCubePlugin,
    uri: &Url,
    text: &str,
) -> ParseResult {
    let registry = plugin.registry_mut();

    // Derive a passage name from the file stem.
    let passage_name = uri
        .path_segments()
        .and_then(|mut s| s.next_back())
        .map(|filename| {
            filename
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(filename)
        })
        .unwrap_or("script")
        .to_string();

    // Build the script SpecialPassageDef — same one used for [script]-tagged
    // passages. We look it up from the core specials so we stay in sync with
    // any future changes to the def.
    let special_def = knot_core::passage::twine_core_special_passages()
        .into_iter()
        .find(|d| d.name == "script" && d.match_strategy == knot_core::passage::MatchStrategy::Tag)
        .expect("core specials must contain a [script] tag def");

    // Build a synthetic ClassifiedPassage. The header fields are minimal
    // (no `::` prefix, no tags block, no metadata) — just enough for the
    // downstream pipeline to work.
    let cp = ClassifiedPassage {
        header: crate::header::TweeHeader {
            name: passage_name.clone(),
            tags: vec!["script".to_string()],
            header_start: 0,
            name_start: 0,
            metadata_json: None,
            name_text_raw: passage_name.clone(),
            tags_raw: String::new(),
        },
        body_text: text.to_string(),
        file_uri: uri.to_string(),
        special_def: Some(special_def),
        category: classifier::PassageCategory::CoreTagged,
        processing_priority: classifier::PROCESSING_SCRIPT,
    };

    // Clear registries for this file before re-populating
    registry.remove_file(uri.as_ref());

    // Phase 1: empty AST (Script mode doesn't parse SugarCube syntax)
    let mut passage_ast = super::ast::PassageAst::empty(ParseMode::Script);

    // Phase 2: JS annotation — sugarcube_syntax = true (identical to [script]
    // passages). The SugarCube preprocessor runs ($var → State.variables.var,
    // keyword operators → JS equivalents) so that .js files have the same JS
    // analysis behavior as [script]-tagged passages.
    //
    // `known_macro_names` is empty here — standalone .js files have no
    // inline macro calls, so the fallback path is never reached.
    let empty_set = std::collections::HashSet::new();
    super::js::js_annotate::annotate_js(&mut passage_ast, text, true, true, &empty_set);

    // Phase 3: Registry population
    registry_populate::populate_registries_from_unified_ast(
        registry,
        &passage_ast,
        &cp,
        uri.as_ref(),
        0, // body_offset_in_passage = 0 (no header)
    );

    // Build the Passage struct
    let mut passage = passage_build::build_passage(&cp, &passage_ast, 0, 0);
    passage.span = 0..text.len();
    passage.passage_offset = 0;
    // header_name_span = None (no `::` header in a .js file)
    passage.header_name_span = None;

    // Build tokens from script_js_analysis
    let mut passage_tokens = Vec::new();
    if let Some(ref analysis) = passage_ast.script_js_analysis {
        super::token_builder::build_script_passage_tokens(analysis, &mut passage_tokens, 0);
    }

    let token_group = PassageTokenGroup {
        passage_name: passage_name.clone(),
        passage_offset: 0,
        tokens: passage_tokens,
    };

    // Build diagnostics — validate the entire file as a JS module.
    // sugarcube_syntax = true (identical to [script] passages).
    //
    // Task 1 (optimization): pass pre-collected diagnostics from
    // `annotate_js` to avoid re-parsing the same JS source.
    let mut passage_diagnostics = Vec::new();
    let script_diagnostics = passage_ast
        .script_js_analysis
        .as_ref()
        .map(|a| a.diagnostics.as_slice())
        .unwrap_or(&[]);
    let js_diagnostics =
        super::js_validate::validate_script_passage(text, 0, true, script_diagnostics);
    passage_diagnostics.extend(js_diagnostics);

    let diagnostic_group = PassageDiagnosticGroup {
        passage_name: passage_name.clone(),
        passage_offset: 0,
        diagnostics: passage_diagnostics,
    };

    pipeline_log::parse_full_summary(
        uri.as_ref(),
        1,
        token_group.tokens.len(),
        diagnostic_group.diagnostics.len(),
    );

    // Sort passages by passage_offset (source order) — consistent with parse_full
    let mut passages = vec![passage];
    passages.sort_by_key(|p| p.passage_offset);

    ParseResult {
        passages,
        token_groups: vec![token_group],
        diagnostic_groups: vec![diagnostic_group],
        is_complete: true,
    }
}
