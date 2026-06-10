//! Full parse pipeline orchestration.
//!
//! Contains the two main entry points that were previously the bodies of
//! `FormatPluginMut::parse_mut()` and `FormatPluginMut::parse_passage_mut()`. The trait
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

        // Parse the body (parser returns offsets relative to body text start)
        let passage_ast = super::parser::parse_passage_body(&cp.body_text, body_offset, mode);

        // Populate registries from the AST
        // Spans are passage-body-relative; no body_offset shifting needed.
        registry_populate::populate_registries_from_ast(
            registry,
            &passage_ast,
            cp,
            uri.as_ref(),
        );

        // For script passages, also do oxc walk for State.variables / Macro.add / functions / templates
        // Spans are passage-body-relative; no body_offset shifting needed.
        if is_script_passage(cp) {
            registry_populate::walk_script_js(
                registry,
                &cp.body_text,
                cp,
                uri.as_ref(),
            );
        }

        // For all passages with inline JS (<<run>>, <<set>>, <<if>>, <<script>> blocks, etc.),
        // walk the inline JS snippets with oxc to detect State.variables.x references,
        // SugarCube operator usage, and other JS patterns the SugarCube parser can't see.
        // Skip script passages (already handled above), stylesheets, and minimal passages.
        if !is_script_passage(cp) && !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            registry_populate::walk_inline_js_snippets(
                registry,
                &passage_ast.nodes,
                &cp.header.name,
                uri.as_ref(),
                &cp.body_text,
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

    // 6. Post-pass: inject Call edges for widget invocations.
    // After all passages are parsed and the custom macro registry is populated,
    // we do a second pass over normal passages to check if any of their
    // `<<macroName>>` invocations match a known widget. If so, we add a
    // Link with `LinkSource::WidgetCall` → `EdgeType::Call` so the graph
    // traces widget invocations as call edges.
    {
        let custom_macros = &plugin.registry().custom_macros();
        for passage in &mut passages {
            // Only check passages with body blocks that might contain macros
            for block in &passage.body {
                if let knot_core::passage::Block::Macro { name, .. } = block {
                    if custom_macros.contains(name) {
                        // Found a widget invocation — add a Call link to the
                        // widget's definition passage.
                        let edge_type_hint = Some(knot_core::graph::EdgeType::Call);
                        // Check if this link already exists (avoid duplicates
                        // from <<widget>> definitions themselves)
                        let already_has = passage.links.iter().any(|l| {
                            l.target == *name && l.edge_type_hint == edge_type_hint
                        });
                        if !already_has {
                            passage.links.push(knot_core::passage::Link {
                                display_text: Some(format!("<<{}>>", name)),
                                target: name.clone(),
                                span: 0..0, // Span not needed for dynamic links
                                edge_type_hint,
                            });
                        }
                    }
                }
            }
        }
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

    let passage_ast = super::parser::parse_passage_body(passage_text, 0, mode);

    // Classify the passage BEFORE mutably borrowing the registry.
    // The FormatPlugin classification methods take &self, which would
    // conflict with the mutable borrow from registry_mut().
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
    passage.vars = passage_ast.var_ops.iter().map(|vo| {
        VarOp {
            name: vo.name.clone(),
            kind: if vo.is_write { VarKind::Init } else { VarKind::Read },
            span: vo.span.start..vo.span.end,
            is_temporary: vo.is_temporary,
        }
    }).collect();

    // Phase H: Incremental registry update for single-passage re-parse.
    // Remove old entries for this passage from the registries before
    // adding new ones, so we don't accumulate stale data.
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
        file_uri: file_uri.to_string(),
        category: classifier::PassageCategory::Regular,
        special_def: None,
        processing_priority: 40, // Normal passage priority
    };

    // Now mutably borrow the registry for all write operations
    {
        let registry = plugin.registry_mut();
        registry.remove_passage(passage_name);

        registry_populate::populate_registries_from_ast(
            registry,
            &passage_ast,
            &cp,
            file_uri,
        );

        // For script passages, also do oxc walk
        if mode == ParseMode::Script {
            registry_populate::walk_script_js(
                registry,
                passage_text,
                &cp,
                file_uri,
            );
        }

        // For all passages with inline JS, walk the snippets for registry population
        if mode != ParseMode::Script && !matches!(mode, ParseMode::Stylesheet | ParseMode::Minimal) {
            registry_populate::walk_inline_js_snippets(
                registry,
                &passage_ast.nodes,
                passage_name,
                file_uri,
                passage_text,
            );
        }
    }

    Some(passage)
}
