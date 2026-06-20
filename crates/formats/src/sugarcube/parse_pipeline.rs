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

use knot_core::passage::{Passage, PassageCategory as CorePassageCategory};
use url::Url;

use crate::plugin::{FormatPlugin, ParseResult, PassageDiagnosticGroup, PassageTokenGroup};
use super::SugarCubePlugin;
use super::ast::ParseMode;
use super::classifier::{self, ClassifiedPassage, is_script_passage};
use super::lsp::pipeline_log;
use super::passage_build;
use super::registry_populate;

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

        // Phase 2: JS annotation pass — attach JsAnalysis to AST nodes
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            let js_node_count = count_js_nodes(&passage_ast.nodes);
            pipeline_log::parse_phase2_enter(&cp.header.name, js_node_count);
            super::js::js_annotate::annotate_js(
                &mut passage_ast,
                &cp.body_text,
                is_script_passage(cp),
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
        let mut passage = passage_build::build_passage(cp, &passage_ast, body_offset_in_passage, passage_head);
        passage.span = 0..(header_line_end - passage_head + cp.body_text.len());
        passage.passage_offset = passage_head;

        // Build passage.vars from the unified AST (including js_analysis + script_js_analysis)
        passage.vars = passage_build::build_vars_from_unified_ast(&passage_ast, body_offset_in_passage);

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
            let json_tokens = super::token_builder::build_json_body_tokens(&cp.body_text, body_offset_in_passage);
            passage_tokens.extend(json_tokens);
        } else if matches!(mode, ParseMode::Stylesheet) {
            // Stylesheet passages are pure CSS. CSS parsing is currently
            // unserved (see `knot_core::css`) — no tokens emitted.
            // No diagnostic is raised: this is an internal limitation,
            // not a user-visible problem.
        } else if matches!(mode, ParseMode::Interface) {
            // StoryInterface body is HTML. HTML parsing is currently
            // unserved — no tokens emitted, no diagnostic raised.
        } else {
            // Collect custom macro names for Function token differentiation
            let custom_names: std::collections::HashSet<String> =
                registry.custom_macros().names().cloned().collect();
            super::token_builder::build_semantic_tokens(
                &passage_ast.nodes,
                &mut passage_tokens,
                body_offset_in_passage,
                &custom_names,
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
        super::token_builder::build_diagnostics(&passage_ast.nodes, &mut passage_diagnostics, body_offset_in_passage);

        // Validate inline JS snippets via oxc (for diagnostics only)
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            if matches!(mode, ParseMode::Script) {
                // [script] passages: validate the entire body as a JS module.
                // validate_inline_js only walks AST nodes for <<script>> block
                // macros, which doesn't cover [script] passages (their entire
                // body is JS, no macro wrapper).
                let js_diagnostics = super::js_validate::validate_script_passage(
                    &cp.body_text,
                    body_offset_in_passage,
                );
                passage_diagnostics.extend(js_diagnostics);
            } else {
                // Normal passages: validate inline <<script>>/<<set>>/<<run>> snippets
                let js_diagnostics = super::js_validate::validate_inline_js(
                    &passage_ast.nodes,
                    body_offset_in_passage,
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

    // 6. Post-pass: inject Call edges for widget invocations.
    {
        let custom_macros = &plugin.registry().custom_macros();
        for passage in &mut passages {
            for block in &passage.body {
                if let knot_core::passage::Block::Macro { name, .. } = block {
                    if custom_macros.contains(name) {
                        let edge_type_hint = Some(knot_core::graph::EdgeType::Call);
                        let already_has = passage.links.iter().any(|l| {
                            l.target == *name && l.edge_type_hint == edge_type_hint
                        });
                        if !already_has {
                            passage.links.push(knot_core::passage::Link {
                                display_text: Some(format!("<<{}>>", name)),
                                target: name.clone(),
                                span: 0..0,
                                edge_type_hint,
                            });
                        }
                    }
                }
            }
        }
    }

    pipeline_log::parse_full_summary(
        uri.as_ref(),
        passages.len(),
        all_token_groups.iter().map(|g| g.tokens.len()).sum::<usize>(),
        all_diagnostic_groups.iter().map(|g| g.diagnostics.len()).sum::<usize>(),
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
                super::ast::AstNode::Macro { js_analysis, children, .. } => {
                    if let Some(analysis) = js_analysis {
                        n += analysis.var_ops.len();
                    }
                    if let Some(ch) = children {
                        n += count_in_nodes(ch);
                    }
                }
                super::ast::AstNode::Expression { js_analysis, .. } => {
                    if let Some(analysis) = js_analysis {
                        n += analysis.var_ops.len();
                    }
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
pub(super) fn parse_single(
    plugin: &mut SugarCubePlugin,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
    file_uri: &str,
) -> Option<Passage> {
    // Determine the parse mode from the tags
    let mode = if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("script")) {
        ParseMode::Script
    } else if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("stylesheet") || t.eq_ignore_ascii_case("style")) {
        ParseMode::Stylesheet
    } else if passage_tags.iter().any(|t| t.eq_ignore_ascii_case("widget")) {
        ParseMode::Widget
    } else if passage_name == "StoryInterface" {
        ParseMode::Interface
    } else if passage_name == "StoryData" {
        ParseMode::Minimal
    } else {
        ParseMode::Normal
    };

    // Phase 1: Structural parse
    let mut passage_ast = super::parser::parse_passage_body(passage_text, 0, mode);

    // Phase 2: JS annotation pass
    if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
        super::js::js_annotate::annotate_js(
            &mut passage_ast,
            passage_text,
            mode == ParseMode::Script,
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
    // parse_single works with isolated passage text (no document context),
    // so header_start = 0, name_start = 0.
    let header = crate::header::TweeHeader {
        name: passage_name.to_string(),
        tags: passage_tags.to_vec(),
        header_start: 0,
        name_start: 0,
        metadata_json: None,
        name_text_raw: passage_name.to_string(),
        tags_raw: String::new(),
    };
    let cp = ClassifiedPassage {
        header,
        body_text: passage_text.to_string(),
        file_uri: file_uri.to_string(),
        category: classifier::PassageCategory::Regular,
        special_def,
        processing_priority: 40,
    };

    // Build the Passage struct (passage-relative spans, passage_head = 0).
    // body_offset_in_passage = 0 because parse_single has no document context.
    let mut passage = passage_build::build_passage(&cp, &passage_ast, 0, 0);
    passage.span = 0..passage_text.len();
    passage.passage_offset = 0;

    // header_name_span: in parse_single, header_start=0, name_start=0,
    // so the span is already passage-relative.
    passage.header_name_span = Some(
        cp.header.name_start..cp.header.name_start + cp.header.name.len()
    );

    // Override vars with unified AST vars (includes js_analysis + script_js_analysis)
    passage.vars = passage_build::build_vars_from_unified_ast(&passage_ast, 0);

    // Phase 3: Registry mutation — clear old passage data, then populate.
    {
        let registry = plugin.registry_mut();
        registry.remove_passage(passage_name);

        registry_populate::populate_registries_from_unified_ast(
            registry,
            &passage_ast,
            &cp,
            file_uri,
            0, // body_offset_in_passage = 0 (parse_single has no document context)
        );
    }

    Some(passage)
}
