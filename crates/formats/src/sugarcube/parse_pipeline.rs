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

use crate::plugin::{FormatPlugin, ParseResult};
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
    let mut all_tokens = Vec::new();
    let mut all_diagnostics = Vec::new();

    for cp in &classified {
        let mode = SugarCubePlugin::parse_mode_for(cp);

        // Compute where the body starts in the document (after header line + newline)
        let header_line_end = text[cp.header.header_start..]
            .find('\n')
            .map_or(text.len(), |pos| cp.header.header_start + pos + 1);
        let body_offset = header_line_end;

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
        );
        pipeline_log::parse_phase3_exit(
            &cp.header.name,
            registry.variables().path_index_len(),
            registry.custom_macros().len(),
            registry.functions().len(),
            registry.templates().len(),
        );

        // Build the Passage struct (shift all AST spans by body_offset)
        let mut passage = passage_build::build_passage(cp, &passage_ast, body_offset);
        passage.span = cp.header.header_start..header_line_end + cp.body_text.len();

        // Build passage.vars from the unified AST (including js_analysis + script_js_analysis)
        passage.vars = passage_build::build_vars_from_unified_ast(&passage_ast, body_offset);

        // Build semantic tokens for the header
        let is_special = cp.special_def.is_some();
        let header_tokens = super::token_builder::build_header_tokens(&cp.header, is_special);
        all_tokens.extend(header_tokens);

        // Build semantic tokens from the body AST (shift spans by body_offset)
        if matches!(mode, ParseMode::Minimal) {
            let json_tokens = super::token_builder::build_json_body_tokens(&cp.body_text, body_offset);
            all_tokens.extend(json_tokens);
        } else {
            super::token_builder::build_semantic_tokens(&passage_ast.nodes, &mut all_tokens, body_offset);
        }

        // Build diagnostics from the body AST (shift spans by body_offset)
        super::token_builder::build_diagnostics(&passage_ast.nodes, &mut all_diagnostics, body_offset);

        // Validate inline JS snippets via oxc (for diagnostics only)
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            let js_diagnostics = super::js_validate::validate_inline_js(
                &passage_ast.nodes,
                body_offset,
            );
            all_diagnostics.extend(js_diagnostics);
        }

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
        all_tokens.len(),
        all_diagnostics.len(),
    );

    ParseResult {
        passages,
        tokens: all_tokens,
        diagnostics: all_diagnostics,
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
        Some(plugin.classify_passage(passage_name, passage_tags))
    } else {
        None
    };

    let mut passage = if let Some(def) = &special_def {
        Passage::new_special(
            passage_name.to_string(),
            0..passage_text.len(),
            def.clone()?,
        )
    } else {
        Passage::new(passage_name.to_string(), 0..passage_text.len())
    };

    passage.tags = passage_tags.to_vec();
    passage.body = passage_build::build_body_blocks(&passage_ast.nodes, 0);
    passage.links = passage_ast.links.iter()
        .filter(|li| !li.target.is_empty())
        .map(|li| {
            let edge_type_hint = passage_build::link_source_to_edge_type(li.source);
            knot_core::passage::Link {
                display_text: li.display.clone(),
                target: li.target.clone(),
                span: li.span.start..li.span.end,
                edge_type_hint,
            }
        }).collect();

    // Build passage.vars from the unified AST
    passage.vars = passage_build::build_vars_from_unified_ast(&passage_ast, 0);

    // Phase 3: Registry mutation — clear old passage data, then populate.
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
        special_def: None,
        processing_priority: 40,
    };

    {
        let registry = plugin.registry_mut();
        registry.remove_passage(passage_name);

        registry_populate::populate_registries_from_unified_ast(
            registry,
            &passage_ast,
            &cp,
            file_uri,
        );
    }

    Some(passage)
}
