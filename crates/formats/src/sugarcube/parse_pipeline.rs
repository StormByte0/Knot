//! Full parse pipeline orchestration.
//!
//! Contains the two main entry points that were previously the bodies of
//! `FormatPlugin::parse()` and `FormatPlugin::parse_passage()`. The trait
//! impl in `mod.rs` delegates to the free functions here.

use knot_core::passage::{Passage, VarKind, VarOp, PassageCategory as CorePassageCategory};
use url::Url;

use crate::plugin::{FormatPlugin, ParseResult};
use super::SugarCubePlugin;
use super::ast::ParseMode;
use super::classifier::{self, ClassifiedPassage, is_script_passage};
use super::passage_build;
use super::registry_populate;

/// Full parse: split → classify → sort → per-passage dispatch.
///
/// This is the body of `FormatPlugin::parse()` for `SugarCubePlugin`.
pub(super) fn parse_full(plugin: &SugarCubePlugin, uri: &Url, text: &str) -> ParseResult {
    // 1. Split into raw passages
    let raw_passages = super::lexer::split_passages(text);

    // 2. Classify each passage
    let mut classified = classifier::classify_all(&raw_passages, uri.as_ref());

    // 3. Sort by processing priority (scripts first, normal last)
    classifier::sort_for_processing(&mut classified);

    // 4. Clear registries for this file before re-populating
    {
        let mut var_tree = plugin.variable_tree.write();
        var_tree.remove_file(uri.as_ref());
        let mut macro_reg = plugin.custom_macros.write();
        macro_reg.remove_file(uri.as_ref());
    }

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

        // Parse the body (parser returns offsets relative to body text start)
        let passage_ast = super::parser::parse_passage_body(&cp.body_text, body_offset, mode);

        // Populate registries from the AST
        {
            let mut var_tree = plugin.variable_tree.write();
            let mut macro_reg = plugin.custom_macros.write();
            registry_populate::populate_registries_from_ast(
                &mut var_tree,
                &mut macro_reg,
                &passage_ast,
                cp,
                uri.as_ref(),
                body_offset,
            );
        }

        // For script passages, also do oxc walk for State.variables / Macro.add
        if is_script_passage(cp) {
            let mut var_tree = plugin.variable_tree.write();
            let mut macro_reg = plugin.custom_macros.write();
            registry_populate::walk_script_js(
                &mut var_tree,
                &mut macro_reg,
                &cp.body_text,
                cp,
                uri.as_ref(),
            );
        }

        // Build the Passage struct (shift all AST spans by body_offset)
        let mut passage = passage_build::build_passage(cp, &passage_ast, body_offset);
        passage.span = cp.header.header_start..header_line_end + cp.body_text.len();

        // Build semantic tokens for the header
        let is_special = cp.special_def.is_some();
        let header_tokens = super::token_builder::build_header_tokens(&cp.header, is_special);
        all_tokens.extend(header_tokens);

        // Build semantic tokens from the body AST (shift spans by body_offset)
        super::token_builder::build_semantic_tokens(&passage_ast.nodes, &mut all_tokens, body_offset);

        // Build diagnostics from the body AST (shift spans by body_offset)
        super::token_builder::build_diagnostics(&passage_ast.nodes, &mut all_diagnostics, body_offset);

        // Validate inline JS snippets via oxc (Phase D)
        // Only for passages that can contain JS (not stylesheets, not minimal)
        if !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            let js_diagnostics = super::js_validate::validate_inline_js(
                &passage_ast.nodes,
                body_offset,
            );
            all_diagnostics.extend(js_diagnostics);
        }

        passages.push(passage);
    }

    ParseResult {
        passages,
        tokens: all_tokens,
        diagnostics: all_diagnostics,
        is_complete: true,
    }
}

/// Incremental re-parse of a single passage.
///
/// This is the body of `FormatPlugin::parse_passage()` for `SugarCubePlugin`.
pub(super) fn parse_single(
    plugin: &SugarCubePlugin,
    passage_name: &str,
    passage_tags: &[String],
    passage_text: &str,
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

    let passage_ast = super::parser::parse_passage_body(passage_text, 0, mode);

    // Phase H: Incremental registry update for single-passage re-parse.
    // Remove old entries for this passage from the registries before
    // adding new ones, so we don't accumulate stale data.
    {
        let mut var_tree = plugin.variable_tree.write();
        var_tree.remove_passage(passage_name);
        let mut macro_reg = plugin.custom_macros.write();
        macro_reg.remove_passage(passage_name);
    }

    // Classify the passage (using the FormatPlugin default impl)
    let (_, category) = plugin.classify_passage_category(passage_name, passage_tags);
    let is_special = category != CorePassageCategory::Regular;

    let mut passage = if is_special {
        let def = plugin.classify_passage(passage_name, passage_tags);
        Passage::new_special(
            passage_name.to_string(),
            0..passage_text.len(),
            def?,
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
    passage.vars = passage_ast.var_ops.iter().map(|vo| {
        VarOp {
            name: vo.name.clone(),
            kind: if vo.is_write { VarKind::Init } else { VarKind::Read },
            span: vo.span.start..vo.span.end,
            is_temporary: vo.is_temporary,
        }
    }).collect();

    // Phase H: Update registries from the freshly parsed AST
    // Build a minimal ClassifiedPassage for the registry population methods
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
        file_uri: String::new(),
        category: classifier::PassageCategory::Regular,
        special_def: None,
        processing_priority: 40, // Normal passage priority
    };

    {
        let mut var_tree = plugin.variable_tree.write();
        let mut macro_reg = plugin.custom_macros.write();
        registry_populate::populate_registries_from_ast(
            &mut var_tree,
            &mut macro_reg,
            &passage_ast,
            &cp,
            "",
            0,
        );
    }

    // For script passages, also do oxc walk
    if mode == ParseMode::Script {
        let mut var_tree = plugin.variable_tree.write();
        let mut macro_reg = plugin.custom_macros.write();
        registry_populate::walk_script_js(
            &mut var_tree,
            &mut macro_reg,
            passage_text,
            &cp,
            "",
        );
    }

    Some(passage)
}
