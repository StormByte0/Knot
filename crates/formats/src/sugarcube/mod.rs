//! SugarCube Format Plugin
//!
//! SugarCube 2.x is the most popular Twine story format, providing a rich macro
//! system and variable tracking via `$variable` syntax.
//!
//! This module implements a fault-tolerant, two-pass parser:
//!
//! 1. **Pass 1 — Passage boundaries**: A [`logos`]-based lexer splits the source
//!    into passage header regions and their body text.
//! 2. **Pass 2 — Body analysis**: Regex-based extractors detect links, variable
//!    operations, and macro invocations within each passage body.
//!
//! The parser never hard-fails on invalid input. Malformed constructs are captured
//! as [`Block::Incomplete`] and reported as diagnostics rather than causing panics.

pub mod macros;
pub mod lexer;
pub mod links;
pub mod vars;
pub mod tokens;
pub mod validation;
pub mod blocks;
pub mod special_passages;
pub mod comments;
pub mod passage_tree;
pub mod variable_tree;
pub mod custom_macros;
pub mod macro_scan;
pub mod navigation;
pub mod workspace;
#[cfg(test)] mod tests;

use knot_core::passage::{Passage, SpecialPassageDef, StoryFormat, VarOp};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::RwLock;

use url::Url;

use crate::plugin::{FormatDiagnosticSeverity, FormatPlugin, ParseResult};
use crate::types::{
    GlobalDef, ImplicitPassagePattern, MacroDef, OperatorNormalization, ResolvedNavLink,
    VariableSigilInfo,
};

// ---------------------------------------------------------------------------
// Plugin struct
// ---------------------------------------------------------------------------

/// SugarCube 2.x format plugin.
///
/// Regexes are compiled once using `std::sync::LazyLock` in the submodule
/// statics rather than per-instance, since they are immutable and identical
/// across all instances.
///
/// ## Side tables
///
/// The plugin holds side tables for variable tracking. These are
/// SugarCube-internal and NOT exposed through the `FormatPlugin` trait:
///
/// - `variable_tree`: Workspace-wide variable aggregation from VarEncounters
///
/// Both use `RwLock` for interior mutability since `FormatPlugin` requires
/// `Send + Sync` and its methods take `&self` (not `&mut self`). The side
/// tables are populated during `parse()` and `parse_passage()`.
pub struct SugarCubePlugin {
    /// Workspace-wide variable tree.
    /// Aggregates VarEncounter entries from all passages for completions,
    /// hover info, and tree panel navigation.
    variable_tree: RwLock<variable_tree::VariableTree>,

    /// Workspace-wide custom macro registry.
    /// Accumulates Macro.add definitions from [script] passages across
    /// all files so that walk_encounters() can identify user-defined
    /// callables during variable encounter extraction.
    custom_macros: RwLock<custom_macros::CustomMacroRegistry>,
}

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)] // Side table accessors used by Phase C (VSCode wiring)
impl SugarCubePlugin {
    /// Create a new SugarCube plugin instance.
    ///
    /// Regexes are pre-compiled as `LazyLock` statics, so this is essentially free.
    /// Side tables start empty and are populated during parsing.
    pub fn new() -> Self {
        Self {
            variable_tree: RwLock::new(variable_tree::VariableTree::new()),
            custom_macros: RwLock::new(custom_macros::CustomMacroRegistry::new()),
        }
    }

    /// Get a read lock on the variable tree.
    ///
    /// Used for completions, hover, and tree panel navigation.
    pub(crate) fn variable_tree(&self) -> std::sync::RwLockReadGuard<'_, variable_tree::VariableTree> {
        self.variable_tree.read().unwrap()
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::SugarCube
    }

    fn parse(&self, _uri: &Url, text: &str) -> ParseResult {
        let mut passages = Vec::new();
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut has_errors = false;

        // Clear variable tree entries for this file
        // before repopulating. This handles the full-file reparse case.

        let raw_passages = lexer::split_passages(text);

        // Pre-extract user callables for this file. We need them for
        // walk_encounters() (widget detection, callable names).
        let passage_infos: Vec<workspace::PassageInfo> = raw_passages
            .iter()
            .map(|(header, body)| workspace::PassageInfo {
                name: header.name.clone(),
                tags: header.tags.clone(),
                body_text: body.to_string(),
                file_uri: _uri.to_string(),
            })
            .collect();
        let callables = workspace::extract_user_callables(&passage_infos);

        // Update the workspace-wide custom macro registry with custom
        // macros from this file. This ensures cross-file macro definitions
        // are available when processing passages in other files.
        self.custom_macros.write().unwrap().update_file(_uri, &callables);

        // Merge workspace-wide custom macros with per-file callables
        // so that walk_encounters() sees ALL known callables, including
        // custom macros defined in other files' script passages.
        let merged_callables = self.custom_macros.read().unwrap().merge_with_callables(&callables);

        for (header, body) in &raw_passages {
            // Compute body_offset: after the header line's content.
            // The header line in source text ends at the next newline.
            // We find the end of the header line by searching for \n from header_start.
            let header_line_end = text[header.header_start..]
                .find('\n')
                .map(|i| header.header_start + i)
                .unwrap_or(text.len());
            // Body starts after the \n (1 byte) or \r\n (2 bytes)
            let newline_len = if text.get(header_line_end..header_line_end + 2) == Some("\r\n") { 2 } else if header_line_end < text.len() { 1 } else { 0 };
            let body_offset = header_line_end + newline_len;

            // Determine if this is a special passage using the unified
            // classification system. Tags are checked FIRST (per the
            // Twee 3 spec), then names. This replaces the old manual
            // three-stage lookup (format defs → core defs → tag fallback).
            let special_def = self.classify_passage(&header.name, &header.tags);

            let mut passage = if let Some(ref def) = special_def {
                Passage::new_special(header.name.clone(), header.header_start..body_offset + body.len(), def.clone())
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();
            passage.position = lexer::position_from_header(header);

            // ── Context-aware parsing ──────────────────────────────────────
            // Detect script and stylesheet passages. These contain non-Twine
            // content (JavaScript or CSS) and should NOT be parsed with
            // SugarCube regexes for links, variables, or macro structure.
            //
            // Script passages: tagged [script] or named "Story JavaScript"
            // Stylesheet passages: tagged [stylesheet] or named "Story Stylesheet"
            let is_script = passage.is_script_passage();
            let is_stylesheet = passage.is_stylesheet_passage();
            let is_interface = passage.is_interface_passage();

            if is_script {
                // Script passages: only extract implicit passage refs and
                // JS variable aliasing (Engine.play, State.variables, etc.)
                //
                // In script passages, // line comments are valid everywhere,
                // so we use find_all_comment_spans() with is_script_passage=true.
                //
                // Comment spans are body-relative; shift by body_offset to
                // match the document-absolute coordinates of link/var spans.
                let comment_spans: Vec<Range<usize>> = comments::find_all_comment_spans(body, true)
                    .into_iter()
                    .map(|s| (s.start + body_offset)..(s.end + body_offset))
                    .collect();

                let mut raw_links = links::extract_implicit_passage_refs(body, body_offset);
                // Filter links inside comments (including // line comments)
                raw_links.retain(|link| !comments::is_in_comment(&comment_spans, &link.span));
                passage.links = raw_links;

                passage.vars = vars::extract_vars(body, body_offset);
                // Filter vars inside comments
                passage.vars.retain(|var| !comments::is_in_comment(&comment_spans, &var.span));

                passage.body = vec![knot_core::passage::Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                }];

                // Semantic tokens: header + PassageRef tokens for implicit refs
                let layer = special_def.as_ref().map(|d| d.layer);
                let tag_mods = header.tags.iter().map(|t| self.classify_tag(t)).collect();
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
                    tag_modifiers: tag_mods,
                }));

                // PassageRef tokens for implicit passage references in script code
                let mut ref_tokens = tokens::script_passage_ref_tokens(body, body_offset);
                ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(ref_tokens);

                // Property tokens (State.variables, Engine.play, etc.)
                let mut prop_toks = tokens::property_tokens(body, body_offset);
                prop_toks.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(prop_toks);

                // Comment tokens for Twine-style comments
                // comment_spans are already doc-absolute (body-relative +
                // body_offset); comment_tokens() converts them back to
                // body-relative internally for body slicing.
                let comment_toks = tokens::comment_tokens(body, body_offset, &comment_spans);
                tokens.extend(comment_toks);

                // Validation: skip SugarCube-specific bracket checks
                // (no [[/]] or <</>> validation on JS content)

                // Script passages: JS content parsed above.
            } else if is_stylesheet {
                // Stylesheet passages: no link extraction, no variable
                // extraction — just store as a raw text block
                //
                // CSS only supports /* */ comments, which are already
                // covered by find_block_comment_spans(). We don't need
                // // line comment detection in stylesheets.
                passage.body = vec![knot_core::passage::Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                }];

                // Semantic tokens: header (SpecialPassage type)
                let layer = special_def.as_ref().map(|d| d.layer);
                let tag_mods = header.tags.iter().map(|t| self.classify_tag(t)).collect();
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
                    tag_modifiers: tag_mods,
                }));

                // No blanket body tokens for stylesheets — let TextMate
                // handle CSS highlighting. A blanket `String` token would
                // override TextMate scopes, making the body one uniform color.

                // No validation on CSS content
            } else if is_interface {
                // StoryInterface passage: HTML content, no SugarCube
                // link/variable extraction. Only extract implicit passage
                // refs from data-passage attributes and JS API calls
                // that may appear in inline <script> tags.
                let comment_spans: Vec<Range<usize>> = comments::find_all_comment_spans(body, false)
                    .into_iter()
                    .map(|s| (s.start + body_offset)..(s.end + body_offset))
                    .collect();

                let mut raw_links = links::extract_implicit_passage_refs(body, body_offset);
                raw_links.retain(|link| !comments::is_in_comment(&comment_spans, &link.span));
                passage.links = raw_links;

                passage.body = vec![knot_core::passage::Block::Text {
                    content: body.to_string(),
                    span: body_offset..body_offset + body.len(),
                }];

                // Semantic tokens: header (SpecialPassage type)
                let layer = special_def.as_ref().map(|d| d.layer);
                let tag_mods = header.tags.iter().map(|t| self.classify_tag(t)).collect();
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
                    tag_modifiers: tag_mods,
                }));

                // StoryInterface body tokens: emit PassageRef tokens for
                // data-passage attributes, but no blanket String token
                // (let TextMate handle HTML highlighting).
                // interface_body_tokens() delegates to script_passage_ref_tokens()
                // so we only call it once — no duplicate emission.
                let mut ref_tokens = tokens::interface_body_tokens(body, body_offset);
                ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(ref_tokens);
            } else {
                // Normal Twine passage: full SugarCube parsing

                // Find all comment spans (block + line comments within
                // <<script>> blocks). This detects:
                // - /* ... */ (C-style block comments)
                // - /% ... %/ (Twine-style block comments)
                // - <!-- ... --> (HTML block comments)
                // - // ... (line comments inside <<script>> blocks)
                //
                // Comment spans are body-relative; shift by body_offset to
                // match the document-absolute coordinates of link/var spans.
                let comment_spans: Vec<Range<usize>> = comments::find_all_comment_spans(body, false)
                    .into_iter()
                    .map(|s| (s.start + body_offset)..(s.end + body_offset))
                    .collect();

                // Extract body elements, filtering out comment-embedded matches
                // Build passage body from the tree (Phase 2: all walks from tree)
                let tree = passage_tree::parse_passage_body(body, body_offset);
                passage.body = passage_tree::walk_blocks(&tree);

                // Extract links from tree walk (replaces extract_links +
                // extract_implicit_passage_refs + extract_macro_passage_refs)
                let mut raw_links = passage_tree::walk_links(&tree, body, body_offset);
                // Filter links that fall inside comments
                raw_links.retain(|link| {
                    !comments::is_in_comment(&comment_spans, &link.span)
                });
                passage.links = raw_links;

                // Extract vars from tree walk (replaces extract_vars)
                passage.vars = passage_tree::walk_vars(&tree, body, body_offset);
                // Filter vars inside comments
                passage.vars.retain(|var| {
                    !comments::is_in_comment(&comment_spans, &var.span)
                });

                // ── Variable tree population ─────────────────────────────────
                // Walk the passage tree to collect variable encounters and
                // populate the variable tree side table.
                {
                    let callable_names: HashSet<&str> = merged_callables
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect();

                    let var_encounters = passage_tree::walk_encounters(
                        &tree, body, body_offset, &callable_names, &comment_spans,
                    );

                    // Update VariableTree
                    self.variable_tree.write().unwrap().update_passage(
                        &header.name,
                        &var_encounters,
                    );
                }

                // Semantic tokens for header. Use SpecialPassage type if this
                // is a format-defined special passage (e.g., StoryInit,
                // StoryCaption) even though it gets normal SugarCube parsing.
                // FIX: Also check is_script_passage()/is_stylesheet_passage()
                // for tag-classified passages that didn't match by name.
                let is_special_for_tokens = special_def.is_some()
                    || passage.is_script_passage()
                    || passage.is_stylesheet_passage();
                let layer = special_def.as_ref().map(|d| d.layer);
                let tag_mods = header.tags.iter().map(|t| self.classify_tag(t)).collect();
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: is_special_for_tokens,
                    layer,
                    tag_modifiers: tag_mods,
                }));

                // Semantic tokens for body: tree-based core tokens (Macro, Variable, Link)
                let mut body_tokens = passage_tree::walk_tokens(&tree, body, body_offset);
                body_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(body_tokens);

                // Tree-based augmentation tokens: keyword, boolean, namespace,
                // widget, number, string, operator, property, implicit passage
                // refs — all from the tree walk (zero redundant scan_macros()).
                let mut augment_toks = passage_tree::walk_augment_tokens(&tree, body, body_offset);
                augment_toks.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(augment_toks);

                // Comment tokens for Twine-style comments (/% ... %/, /%% ... %%/)
                // that the TextMate grammar doesn't recognize.
                // comment_spans are already doc-absolute; comment_tokens()
                // converts them back to body-relative internally.
                let comment_toks = tokens::comment_tokens(body, body_offset, &comment_spans);
                tokens.extend(comment_toks);

                // Tree-based macro passage-ref tokens (<<goto "name">>,
                // <<link "label" "name">>, etc.) — walks the tree instead of
                // re-scanning with scan_macros().
                let mut macro_ref_tokens = passage_tree::walk_macro_passage_ref_tokens(&tree, body, body_offset);
                macro_ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(macro_ref_tokens);

                // Validation diagnostics: tree-based structural checks (unknown,
                // deprecated, unclosed blocks, parent constraints) + bracket
                // validation (unclosed <</>>, unclosed [[]]) as augmentation
                let mut body_diags = passage_tree::walk_validate(&tree, body_offset);
                // Bracket validation: unclosed << / >> and [[ / ]]
                // These are character-level checks that don't map to tree nodes
                // (an unclosed << without >> never becomes a PassageNode), so
                // they run as augmentation passes on the raw body text.
                validation::validate_macro_brackets(body, body_offset, &mut body_diags);
                validation::validate_link_brackets(body, body_offset, &mut body_diags);
                let filtered_diags: Vec<_> = body_diags.into_iter().filter(|d| {
                    !comments::is_in_comment(&comment_spans, &d.range)
                }).collect();
                for d in &filtered_diags {
                    if matches!(d.severity, FormatDiagnosticSeverity::Error) {
                        has_errors = true;
                    }
                }
                diagnostics.extend(filtered_diags);
            }

            passages.push(passage);
        }

        ParseResult {
            passages,
            tokens,
            diagnostics,
            is_complete: !has_errors,
        }
    }

    fn parse_passage(&self, passage_name: &str, passage_tags: &[String], passage_text: &str) -> Option<Passage> {
        // For incremental re-parse: we receive the body text and tags.
        // Tags are now passed through, allowing tag-matched special passages
        // (e.g., [script], [stylesheet], [widget], [init]) to be correctly
        // classified during incremental updates.
        let special_def = self.classify_passage(passage_name, passage_tags);

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

        passage.tags = passage_tags.to_vec();

        // Context-aware: skip SugarCube regex on script/stylesheet passages
        let is_script = passage.is_script_passage();
        let is_stylesheet = passage.is_stylesheet_passage();
        let is_interface = passage.is_interface_passage();

        if is_script {
            // Script passages: // line comments are valid everywhere
            let comment_spans = comments::find_all_comment_spans(passage_text, true);

            let mut raw_links = links::extract_implicit_passage_refs(passage_text, 0);
            raw_links.retain(|link| !comments::is_in_comment(&comment_spans, &link.span));
            passage.links = raw_links;

            passage.vars = vars::extract_vars(passage_text, 0);
            passage.vars.retain(|var| !comments::is_in_comment(&comment_spans, &var.span));

            passage.body = vec![knot_core::passage::Block::Text {
                content: passage_text.to_string(),
                span: 0..passage_text.len(),
            }];

            // Script passages: custom macros are already registered during
            // the full parse() call. No additional processing needed here.
        } else if is_stylesheet {
            passage.body = vec![knot_core::passage::Block::Text {
                content: passage_text.to_string(),
                span: 0..passage_text.len(),
            }];
        } else if is_interface {
            // StoryInterface: HTML content, only implicit refs
            let comment_spans = comments::find_all_comment_spans(passage_text, false);
            let mut raw_links = links::extract_implicit_passage_refs(passage_text, 0);
            raw_links.retain(|link| !comments::is_in_comment(&comment_spans, &link.span));
            passage.links = raw_links;

            passage.body = vec![knot_core::passage::Block::Text {
                content: passage_text.to_string(),
                span: 0..passage_text.len(),
            }];
        } else {
            let comment_spans = comments::find_all_comment_spans(passage_text, false);

            // Build passage body from the tree (Phase 2: all walks from tree)
            let tree = passage_tree::parse_passage_body(passage_text, 0);
            passage.body = passage_tree::walk_blocks(&tree);

            // Extract links from tree walk
            passage.links = passage_tree::walk_links(&tree, passage_text, 0);
            // Filter links inside comments
            passage.links.retain(|link| {
                !comments::is_in_comment(&comment_spans, &link.span)
            });

            // Extract vars from tree walk
            passage.vars = passage_tree::walk_vars(&tree, passage_text, 0);
            passage.vars.retain(|var| {
                !comments::is_in_comment(&comment_spans, &var.span)
            });

            // ── Variable tree population (incremental) ──
            // For incremental updates, we use the workspace-wide custom macro
            // registry to provide cross-file callable names.
            {
                let registry_callables = self.custom_macros.read().unwrap()
                    .merge_with_callables(&[]);
                let callable_names: HashSet<&str> = registry_callables
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();

                let var_encounters = passage_tree::walk_encounters(
                    &tree, passage_text, 0, &callable_names, &comment_spans,
                );

                // Update VariableTree
                self.variable_tree.write().unwrap().update_passage(
                    passage_name,
                    &var_encounters,
                );
            }
        }

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::name_matched_special_passages()
    }

    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::tag_matched_special_passages()
    }

    fn display_name(&self) -> &str {
        "SugarCube 2"
    }

    // -------------------------------------------------------------------
    // Macro catalog (behavioral overrides)
    // -------------------------------------------------------------------

    fn builtin_macros(&self) -> &'static [MacroDef] {
        macros::builtin_macros()
    }

    fn block_macro_names(&self) -> HashSet<&'static str> {
        macros::block_macro_names()
    }

    fn folding_modifier_names(&self) -> HashSet<&'static str> {
        macros::folding_modifier_names()
    }

    fn passage_arg_macro_names(&self) -> HashSet<&'static str> {
        macros::passage_arg_macro_names()
    }

    fn label_then_passage_macros(&self) -> HashSet<&'static str> {
        macros::label_then_passage_macros()
    }

    fn variable_assignment_macros(&self) -> HashSet<&'static str> {
        macros::variable_assignment_macros()
    }

    fn macro_definition_macros(&self) -> HashSet<&'static str> {
        macros::macro_definition_macros()
    }

    fn inline_script_macros(&self) -> HashSet<&'static str> {
        macros::inline_script_macros()
    }

    fn dynamic_navigation_macros(&self) -> HashSet<&'static str> {
        macros::dynamic_navigation_macros()
    }

    fn find_macro(&self, name: &str) -> Option<&'static MacroDef> {
        macros::find_macro(name)
    }

    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        macros::macro_parent_constraints()
    }

    fn get_passage_arg_index(&self, macro_name: &str, arg_count: usize) -> i32 {
        macros::get_passage_arg_index(macro_name, arg_count)
    }

    // -------------------------------------------------------------------
    // Syntax detection (format-aware handler dispatch)
    // -------------------------------------------------------------------

    fn find_macro_at_position(
        &self,
        line: &str,
        byte_pos: usize,
    ) -> Option<crate::plugin::MacroAtPosition> {
        macro_scan::find_macro_at_position(line, byte_pos)
    }

    fn scan_line_for_macro_events(
        &self,
        line: &str,
        line_idx: u32,
    ) -> Vec<crate::plugin::MacroBlockEvent> {
        macro_scan::scan_line_for_macro_events(line, line_idx, &self.block_macro_names())
    }

    fn format_macro_label(&self, name: &str) -> String {
        format!("<<{}>>", name)
    }

    fn format_macro_signature_label(&self, name: &str, params: &str) -> String {
        if params.is_empty() {
            format!("<<{}>>", name)
        } else {
            format!("<<{} {}>>", name, params)
        }
    }

    fn format_close_macro_label(&self, name: &str) -> String {
        format!("<</{}>>", name)
    }

    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        macros::build_macro_snippet(name, has_body)
    }

    fn detect_close_tag_context(&self, before_cursor: &str) -> Option<String> {
        // Check for `<</` prefix — SugarCube close-tag context
        if let Some(pos) = before_cursor.rfind("<</") {
            let partial = &before_cursor[pos + 3..];
            // Partial should be alphanumeric (the partial macro name)
            if partial.is_empty() || partial.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Some(partial.to_string());
            }
        }
        // Also check for `<<` at the end (user just typed the open)
        if before_cursor.ends_with("<<") {
            return Some(String::new());
        }
        None
    }

    fn has_block_macros_with_close_tags(&self) -> bool {
        true // SugarCube has <<if>>...<</if>> block structure
    }

    // -------------------------------------------------------------------
    // Special passages (extended)
    // -------------------------------------------------------------------

    fn special_passage_names(&self) -> HashSet<&'static str> {
        macros::special_passage_names()
    }

    fn system_passage_names(&self) -> HashSet<&'static str> {
        macros::system_passage_names()
    }

    // -------------------------------------------------------------------
    // Variable tracking
    // -------------------------------------------------------------------

    fn variable_sigils(&self) -> Vec<VariableSigilInfo> {
        macros::variable_sigils()
    }

    fn describe_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::describe_variable_sigil(sigil)
    }

    fn resolve_variable_sigil(&self, sigil: char) -> Option<&'static str> {
        macros::resolve_variable_sigil(sigil)
    }

    fn assignment_operators(&self) -> Vec<&'static str> {
        macros::assignment_operators()
    }

    fn variable_assignment_snippet(&self, var_name: &str, value: &str) -> Option<String> {
        Some(format!("<<set {} to {}>>", var_name, value))
    }

    fn comparison_operators(&self) -> Vec<&'static str> {
        macros::comparison_operators()
    }

    // -------------------------------------------------------------------
    // Implicit passage references
    // -------------------------------------------------------------------

    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        macros::implicit_passage_patterns()
    }

    // -------------------------------------------------------------------
    // Dynamic navigation resolution
    // -------------------------------------------------------------------

    fn build_var_string_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        navigation::build_var_string_map(workspace)
    }

    fn resolve_dynamic_navigation_links(
        &self,
        passage: &Passage,
        var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        navigation::resolve_dynamic_navigation_links(passage, var_string_map)
    }

    // -------------------------------------------------------------------
    // Edge classification (format-aware edge typing)
    // -------------------------------------------------------------------

    fn classify_edge(
        &self,
        source_passage: &Passage,
        display_text: Option<&str>,
        target: &str,
    ) -> Option<knot_core::graph::EdgeType> {
        navigation::classify_edge(source_passage, display_text, target)
    }

    // -------------------------------------------------------------------
    // Hover / documentation
    // -------------------------------------------------------------------

    fn global_hover_text(&self, name: &str) -> Option<&'static str> {
        macros::global_hover_text(name)
    }

    fn builtin_globals(&self) -> &'static [GlobalDef] {
        macros::builtin_globals()
    }

    fn global_object_names(&self) -> HashSet<&'static str> {
        macros::builtin_globals().iter().map(|g| g.name).collect()
    }

    // -------------------------------------------------------------------
    // Operator normalization
    // -------------------------------------------------------------------

    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        macros::operator_normalization()
    }

    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        macros::operator_precedence()
    }

    // -------------------------------------------------------------------
    // Variable tracking capability
    // -------------------------------------------------------------------

    fn supports_full_variable_tracking(&self) -> bool {
        true
    }

    // -------------------------------------------------------------------
    // Macro snippet mapping
    // -------------------------------------------------------------------

    fn macro_snippet(&self, name: &str) -> Option<&'static str> {
        macros::macro_snippet(name)
    }

    // -------------------------------------------------------------------
    // Dot-notation object property map
    // -------------------------------------------------------------------

    fn build_object_property_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, HashSet<String>> {
        // Collect all variable operations across the workspace
        let vars_by_passage: Vec<Vec<&VarOp>> = workspace
            .documents()
            .flat_map(|doc| doc.passages.iter().map(|p| p.vars.iter().collect()))
            .collect();

        vars::extract_object_property_map(&vars_by_passage)
    }

    fn build_shape_aware_property_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, crate::types::PropertyMapEntry> {
        vars::build_shape_aware_property_map(workspace)
    }

    // -------------------------------------------------------------------
    // State variable registry & diagnostics
    // -------------------------------------------------------------------

    fn build_state_variable_registry(
        &self,
        workspace: &knot_core::Workspace,
    ) -> HashMap<String, crate::types::StateVariable> {
        vars::build_state_variable_registry(workspace)
    }

    fn compute_variable_diagnostics(
        &self,
        workspace: &knot_core::Workspace,
        start_passage: &str,
        registry: &HashMap<String, crate::types::StateVariable>,
    ) -> Vec<crate::types::VariableDiagnostic> {
        vars::compute_variable_diagnostics(workspace, start_passage, registry)
    }

    // -------------------------------------------------------------------
    // Variable tree (format-agnostic UI representation)
    // -------------------------------------------------------------------

    fn build_variable_tree(
        &self,
        workspace: &knot_core::Workspace,
        _source_text: &dyn crate::plugin::SourceTextProvider,
    ) -> Vec<crate::types::VariableTreeNode> {
        // Tree-only path: walk_vars() already handles all variable patterns
        // (JS aliases, State API, setter links, bracket props) that the
        // passage tree extraction covered.
        vars::build_variable_tree(workspace)
    }

    // -------------------------------------------------------------------
    // Passage variable references (passage diagnostics)
    // -------------------------------------------------------------------

    fn extract_passage_variable_refs(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn crate::plugin::SourceTextProvider,
        passage_name: &str,
    ) -> Vec<crate::types::PassageVarRef> {
        vars::extract_passage_variable_refs(workspace, source_text, passage_name)
    }
}
