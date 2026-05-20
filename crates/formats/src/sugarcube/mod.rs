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

use knot_core::passage::{Passage, SpecialPassageDef, StoryFormat, VarOp};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
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
/// Regexes are compiled once using `once_cell::sync::Lazy` in the submodule
/// statics rather than per-instance, since they are immutable and identical
/// across all instances.
pub struct SugarCubePlugin;

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SugarCubePlugin {
    /// Create a new SugarCube plugin instance.
    ///
    /// Regexes are pre-compiled as `Lazy` statics, so this is essentially free.
    pub fn new() -> Self {
        Self
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

        let raw_passages = lexer::split_passages(text);

        for (header, body) in &raw_passages {
            let body_offset = header.header_start + header.header_len;

            // Determine if this is a special passage.
            // Check format-specific defs first, then fall back to TwineCore/LegacyCore.
            let format_defs = special_passages::special_passage_defs();
            let special_def = format_defs.iter().find(|d| d.name == header.name).cloned()
                .or_else(|| {
                    knot_core::passage::twine_core_special_passages().iter()
                        .chain(knot_core::passage::legacy_core_special_passages().iter())
                        .find(|d| d.name == header.name).cloned()
                });

            let mut passage = if let Some(ref def) = special_def {
                Passage::new_special(header.name.clone(), header.header_start..body_offset + body.len(), def.clone())
            } else {
                Passage::new(header.name.clone(), header.header_start..body_offset + body.len())
            };

            passage.tags = header.tags.clone();
            passage.position = header.position;

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
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
                }));

                // PassageRef tokens for implicit passage references in script code
                let mut ref_tokens = tokens::script_passage_ref_tokens(body, body_offset);
                ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(ref_tokens);

                // Validation: skip SugarCube-specific bracket checks
                // (no [[/]] or <</>> validation on JS content)
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
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
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
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: true,
                    layer,
                }));

                // StoryInterface body tokens: emit PassageRef tokens for
                // data-passage attributes, but no blanket String token
                // (let TextMate handle HTML highlighting).
                tokens.extend(tokens::interface_body_tokens(body, body_offset));

                let mut ref_tokens = tokens::script_passage_ref_tokens(body, body_offset);
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
                let mut raw_links = links::extract_links(body, body_offset);
                raw_links.extend(links::extract_implicit_passage_refs(body, body_offset));
                raw_links.extend(links::extract_macro_passage_refs(body, body_offset));

                // Filter links that fall inside comments
                passage.links = raw_links.into_iter().filter(|link| {
                    !comments::is_in_comment(&comment_spans, &link.span)
                }).collect();

                // Deduplicate links by (display_text, target) — the same
                // passage reference should not appear multiple times
                {
                    let mut seen = HashSet::new();
                    passage.links.retain(|link| {
                        let key = (link.display_text.clone(), link.target.clone());
                        seen.insert(key)
                    });
                }

                passage.vars = vars::extract_vars(body, body_offset);
                // Filter vars inside comments
                passage.vars.retain(|var| {
                    !comments::is_in_comment(&comment_spans, &var.span)
                });

                let macros = blocks::extract_macros(body, body_offset);
                passage.body = blocks::build_body_blocks(body, body_offset, &macros);

                // Semantic tokens for header. Use SpecialPassage type if this
                // is a format-defined special passage (e.g., StoryInit,
                // StoryCaption) even though it gets normal SugarCube parsing.
                let layer = special_def.as_ref().map(|d| d.layer);
                tokens.extend(tokens::header_tokens(header, &tokens::HeaderTokenContext {
                    is_special: special_def.is_some(),
                    layer,
                }));

                // Semantic tokens for body (filter comment-embedded tokens)
                let mut body_tokens = tokens::body_tokens(body, body_offset);
                body_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(body_tokens);

                // PassageRef tokens for implicit passage references
                // (Engine.play, data-passage, etc.)
                let mut implicit_ref_tokens = tokens::script_passage_ref_tokens(body, body_offset);
                implicit_ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(implicit_ref_tokens);

                // PassageRef tokens for macro passage references
                // (<<goto "name">>, <<link "label" "name">>, etc.)
                let mut macro_ref_tokens = tokens::macro_passage_ref_tokens(body, body_offset);
                macro_ref_tokens.retain(|tok| {
                    let tok_span = tok.start..tok.start + tok.length;
                    !comments::is_in_comment(&comment_spans, &tok_span)
                });
                tokens.extend(macro_ref_tokens);

                // Validation diagnostics (filter comment-embedded ranges)
                let body_diags = validation::validate(body, body_offset);
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

    fn parse_passage(&self, passage_name: &str, passage_text: &str) -> Option<Passage> {
        // For incremental re-parse: we receive just the body text.
        let format_defs = special_passages::special_passage_defs();
        let special_def = format_defs.iter().find(|d| d.name == passage_name).cloned()
            .or_else(|| {
                knot_core::passage::twine_core_special_passages().iter()
                    .chain(knot_core::passage::legacy_core_special_passages().iter())
                    .find(|d| d.name == passage_name).cloned()
            });

        let mut passage = if let Some(def) = special_def {
            Passage::new_special(passage_name.to_string(), 0..passage_text.len(), def)
        } else {
            Passage::new(passage_name.to_string(), 0..passage_text.len())
        };

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

            passage.links = links::extract_links(passage_text, 0);
            passage.links.extend(links::extract_implicit_passage_refs(passage_text, 0));
            passage.links.extend(links::extract_macro_passage_refs(passage_text, 0));

            // Filter links inside comments
            passage.links.retain(|link| {
                !comments::is_in_comment(&comment_spans, &link.span)
            });

            // Deduplicate
            {
                let mut seen = HashSet::new();
                passage.links.retain(|link| {
                    let key = (link.display_text.clone(), link.target.clone());
                    seen.insert(key)
                });
            }

            passage.vars = vars::extract_vars(passage_text, 0);
            passage.vars.retain(|var| {
                !comments::is_in_comment(&comment_spans, &var.span)
            });

            let macros = blocks::extract_macros(passage_text, 0);
            passage.body = blocks::build_body_blocks(passage_text, 0, &macros);
        }

        Some(passage)
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::special_passage_defs()
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
        use crate::plugin::MacroAtPosition;

        // Search for `<<name ...>>` constructs on this line.
        // byte_pos is a UTF-16 code unit offset (from LSP). We convert
        // to a byte offset for string comparison, then return byte ranges
        // that the handler converts back to UTF-16 for LSP responses.
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("<<") {
            let abs_start = search_from + rel_start;

            // Check for close-tag: <</name>>
            if line[abs_start..].starts_with("<</") {
                if let Some(rel_end) = line[abs_start..].find(">>") {
                    let abs_end = abs_start + rel_end + 2;

                    if byte_pos >= abs_start && byte_pos <= abs_end {
                        let inner = &line[abs_start + 3..abs_end - 2];
                        let name = inner.split_whitespace().next().unwrap_or(inner).trim();
                        let name_byte_start = abs_start + 3;
                        let name_byte_end = name_byte_start + name.len();
                        return Some(MacroAtPosition {
                            name: name.to_string(),
                            full_range: abs_start..abs_end,
                            name_range: name_byte_start..name_byte_end,
                            is_unclosed: false,
                        });
                    }
                    search_from = abs_end;
                    continue;
                }
            }

            // Open tag: <<name args>>
            if let Some(rel_end) = line[abs_start..].find(">>") {
                let abs_end = abs_start + rel_end + 2;

                if byte_pos >= abs_start && byte_pos <= abs_end {
                    let content_start = abs_start + 2;
                    let content_end = abs_end - 2;
                    let content = &line[content_start..content_end];
                    let macro_name = content.split_whitespace().next().unwrap_or(content).trim();
                    let name_byte_start = content_start;
                    let name_byte_end = content_start + macro_name.len();
                    return Some(MacroAtPosition {
                        name: macro_name.to_string(),
                        full_range: abs_start..abs_end,
                        name_range: name_byte_start..name_byte_end,
                        is_unclosed: false,
                    });
                }
                search_from = abs_end;
            } else {
                // Unclosed macro — cursor might be inside
                if byte_pos >= abs_start {
                    let content_start = abs_start + 2;
                    let content = &line[content_start..];
                    let macro_name = content.split_whitespace().next().unwrap_or(content).trim();
                    let name_byte_start = content_start;
                    let name_byte_end = content_start + macro_name.len();
                    return Some(MacroAtPosition {
                        name: macro_name.to_string(),
                        full_range: abs_start..line.len(),
                        name_range: name_byte_start..name_byte_end,
                        is_unclosed: true,
                    });
                }
                break;
            }
        }
        None
    }

    fn scan_line_for_macro_events(
        &self,
        line: &str,
        line_idx: u32,
    ) -> Vec<crate::plugin::MacroBlockEvent> {
        use crate::plugin::MacroBlockEvent;

        let block_names = self.block_macro_names();
        let mut events = Vec::new();

        // Open macros: <<name ...>> — use the same regex as the TextMate grammar
        let re_open = regex::Regex::new(r"<<([A-Za-z_][A-Za-z0-9_]*)(?:\s+((?:[^>]|>[^>])*?))?>>").unwrap();
        for caps in re_open.captures_iter(line) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                if block_names.contains(name) {
                    events.push(MacroBlockEvent {
                        name: name.to_string(),
                        line: line_idx,
                        is_open: true,
                    });
                }
            }
        }

        // Close macros: <</name>>
        let re_close = regex::Regex::new(r"<</([A-Za-z_][A-Za-z0-9_]*)>>").unwrap();
        for caps in re_close.captures_iter(line) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                events.push(MacroBlockEvent {
                    name: name.to_string(),
                    line: line_idx,
                    is_open: false,
                });
            }
        }

        events
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
        // SugarCube-specific: scan <<set $var to "literal">> patterns
        let re_set_string = regex::Regex::new(
            r#"<<set\s+([\$][A-Za-z_][A-Za-z0-9_]*)\s+to\s+"([^"]*)""#
        ).unwrap();

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for doc in workspace.documents() {
            for passage in &doc.passages {
                for block in &passage.body {
                    let content = match block {
                        knot_core::passage::Block::Text { content, .. } => content.as_str(),
                        knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                        _ => continue,
                    };
                    for caps in re_set_string.captures_iter(content) {
                        if let (Some(var_match), Some(val_match)) = (caps.get(1), caps.get(2)) {
                            let var_name = var_match.as_str().to_string();
                            let string_val = val_match.as_str().to_string();
                            map.entry(var_name).or_default().push(string_val);
                        }
                    }
                }
            }
        }
        for values in map.values_mut() {
            values.sort();
            values.dedup();
        }
        map
    }

    fn resolve_dynamic_navigation_links(
        &self,
        passage: &Passage,
        var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        // SugarCube-specific: resolve <<goto $var>>, <<include $var>>, <<link "label" $var>>, <<button "label" $var>>
        let re_nav_var = regex::Regex::new(
            r#"<<(?:goto|include|link|button)\s+(?:"[^"]*"\s+)?([\$][A-Za-z_][A-Za-z0-9_]*)"#
        ).unwrap();

        let mut links = Vec::new();
        for block in &passage.body {
            let content = match block {
                knot_core::passage::Block::Text { content, .. } => content.as_str(),
                knot_core::passage::Block::Macro { args, .. } => args.as_str(),
                _ => continue,
            };
            for caps in re_nav_var.captures_iter(content) {
                if let Some(var_match) = caps.get(1) {
                    let var_name = var_match.as_str().to_string();
                    if let Some(known_values) = var_string_map.get(&var_name) {
                        for value in known_values {
                            links.push(ResolvedNavLink {
                                display_text: Some(format!("{} (via {})", value, var_name)),
                                target: value.clone(),
                            });
                        }
                    }
                }
            }
        }
        links
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
    // Script/stylesheet tags
    // -------------------------------------------------------------------

    fn script_tags(&self) -> Vec<&'static str> {
        macros::script_tags()
    }

    fn stylesheet_tags(&self) -> Vec<&'static str> {
        macros::stylesheet_tags()
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
        vars::build_variable_tree(workspace)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use knot_core::passage::VarKind;

    #[test]
    fn parse_simple_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nYou are in a room. [[Go north->Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go north".into())
        );
        assert!(result.is_complete);
    }

    #[test]
    fn parse_multiple_passages() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\nHello [[Forest]]\n:: Forest\nYou are in a forest.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 2);
        assert_eq!(result.passages[0].name, "Start");
        assert_eq!(result.passages[1].name, "Forest");
    }

    #[test]
    fn parse_passage_with_tags() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Dark Room [dark interior]\nIt is very dark.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        assert_eq!(result.passages[0].name, "Dark Room");
        assert_eq!(result.passages[0].tags, vec!["dark", "interior"]);
    }

    #[test]
    fn parse_variable_operations() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>>You have $gold coins.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Init));
        assert!(vars.iter().any(|v| v.name == "$gold" && v.kind == VarKind::Read));
    }

    #[test]
    fn parse_pipe_link() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n[[Go to forest|Forest]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages[0].links.len(), 1);
        assert_eq!(result.passages[0].links[0].target, "Forest");
        assert_eq!(
            result.passages[0].links[0].display_text,
            Some("Go to forest".into())
        );
    }

    #[test]
    fn detect_special_passages() {
        let plugin = SugarCubePlugin::new();
        assert!(plugin.is_special_passage("StoryInit"));
        assert!(plugin.is_special_passage("StoryCaption"));
        assert!(!plugin.is_special_passage("MyRoom"));
    }

    #[test]
    fn unclosed_macro_diagnostic() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $x to 5\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"));
    }

    #[test]
    fn empty_input_is_ok() {
        let plugin = SugarCubePlugin::new();
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), "");

        assert!(result.passages.is_empty());
        assert!(result.is_complete);
    }

    #[test]
    fn incremental_reparse() {
        let plugin = SugarCubePlugin::new();
        let passage = plugin.parse_passage("Start", "You have $gold coins.\n");

        assert!(passage.is_some());
        let p = passage.unwrap();
        assert_eq!(p.name, "Start");
        assert!(p.vars.iter().any(|v| v.name == "$gold"));
    }

    #[test]
    fn parse_temporary_variable() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set _temp to 5>>You see _temp items.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let vars = &result.passages[0].vars;

        // Should detect _temp as a temporary init
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Init && v.is_temporary),
            "Should detect _temp as a temporary init"
        );

        // Should detect _temp as a temporary read
        assert!(
            vars.iter().any(|v| v.name == "_temp" && v.kind == VarKind::Read && v.is_temporary),
            "Should detect _temp as a temporary read"
        );
    }

    #[test]
    fn persistent_and_temp_vars_separate() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<set $gold to 10>><<set _temp to 5>>You have $gold and _temp.\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;

        // $gold should be persistent
        let gold_inits: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "$gold" && v.kind == VarKind::Init)
            .collect();
        assert_eq!(gold_inits.len(), 1);
        assert!(!gold_inits[0].is_temporary);

        // _temp should be temporary
        let temp_inits: Vec<_> = vars
            .iter()
            .filter(|v| v.name == "_temp" && v.kind == VarKind::Init)
            .collect();
        assert_eq!(temp_inits.len(), 1);
        assert!(temp_inits[0].is_temporary);
    }

    #[test]
    fn structural_validation_else_without_if() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<else>>Some text\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<else>> outside <<if>>"
        );
    }

    #[test]
    fn structural_validation_break_without_for() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<break>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "Should detect <<break>> outside <<for>>"
        );
    }

    #[test]
    fn structural_validation_else_inside_if_ok() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<if $x>><<else>>OK<</if>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "<<else>> inside <<if>> should not trigger structural validation"
        );
    }

    #[test]
    fn gt_in_condition_else_not_flagged() {
        // The critical bug: <<if _parts.length > 0>> should NOT cause
        // <<else>> to be flagged as a structural error. The `>` in the
        // condition must not break macro delimiter parsing.
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<if _parts.length > 0>>\n  <<= _parts[0] >>\n  <<if _parts.length > 1>> +<<= _parts.length - 1 >><</if>>\n<<else>>\n  &mdash;\n<</if>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
            "<<else>> inside <<if _parts.length > 0>> should NOT be flagged — the > in the condition should not break delimiter parsing"
        );

        // Also verify no unclosed-macro diagnostics from the > in condition
        assert!(
            !result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"),
            "<<if _parts.length > 0>> should not produce unclosed-macro warnings"
        );
    }

    #[test]
    fn deprecated_macro_warning() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<click \"label\" \"target\">>Click<</click>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-deprecated-macro"),
            "Should detect deprecated <<click>> macro"
        );
    }

    #[test]
    fn unknown_macro_hint() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<foobar>>test<</foobar>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert!(
            result.diagnostics.iter().any(|d| d.code == "sc-unknown-macro"),
            "Should detect unknown <<foobar>> macro"
        );
    }

    #[test]
    fn implicit_passage_ref_data_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<a data-passage=\"Forest\">Go</a>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect data-passage implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_engine_play() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>Engine.play(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Engine.play() implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_story_get() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>var p = Story.get(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Story.get() implicit reference"
        );
    }

    #[test]
    fn macro_passage_ref_goto() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<goto \"Forest\">>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect <<goto>> macro passage reference"
        );
    }

    #[test]
    fn macro_passage_ref_link() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<link \"Click\" \"Forest\">>Go<</link>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect <<link>> macro passage reference"
        );
    }

    #[test]
    fn macro_passage_ref_include() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<include \"Sidebar\">>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Sidebar"),
            "Should detect <<include>> macro passage reference"
        );
    }

    #[test]
    fn special_passage_defs_complete() {
        // SugarCube's own special passage definitions (StoryFormat layer only)
        let sc_defs = special_passages::special_passage_defs();
        let sc_names: Vec<&str> = sc_defs.iter().map(|d| d.name.as_str()).collect();

        // StoryFormat-layer passages owned by SugarCube
        assert!(sc_names.contains(&"StoryInit"));
        assert!(sc_names.contains(&"StoryCaption"));
        assert!(sc_names.contains(&"StoryMenu"));
        assert!(sc_names.contains(&"StoryBanner"));
        assert!(sc_names.contains(&"StorySubtitle"));
        assert!(sc_names.contains(&"StoryAuthor"));
        assert!(sc_names.contains(&"StoryDisplayTitle"));
        assert!(sc_names.contains(&"StoryShare"));
        assert!(sc_names.contains(&"StoryInterface"));
        assert!(sc_names.contains(&"PassageReady"));
        assert!(sc_names.contains(&"PassageDone"));
        assert!(sc_names.contains(&"PassageHeader"));
        assert!(sc_names.contains(&"PassageFooter"));

        // TwineCore passages should NOT be in SugarCube's own list
        assert!(!sc_names.contains(&"StoryTitle"), "StoryTitle is TwineCore, not SugarCube");
        assert!(!sc_names.contains(&"StoryData"), "StoryData is TwineCore, not SugarCube");
        assert!(!sc_names.contains(&"Story JavaScript"), "Story JavaScript is TwineCore, not SugarCube");
        assert!(!sc_names.contains(&"Story Stylesheet"), "Story Stylesheet is TwineCore, not SugarCube");

        // But they should be available through all_special_passages()
        let plugin = SugarCubePlugin::new();
        let all_defs = plugin.all_special_passages();
        let all_names: Vec<&str> = all_defs.iter().map(|d| d.name.as_str()).collect();

        assert!(all_names.contains(&"StoryTitle"), "StoryTitle should be in merged registry");
        assert!(all_names.contains(&"StoryData"), "StoryData should be in merged registry");
        assert!(all_names.contains(&"Story JavaScript"), "Story JavaScript should be in merged registry");
        assert!(all_names.contains(&"Story Stylesheet"), "Story Stylesheet should be in merged registry");
        assert!(all_names.contains(&"StoryInit"), "StoryInit should be in merged registry");
    }

    // ── Comment filtering tests ───────────────────────────────────────

    #[test]
    fn twine_comment_skips_links() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n/% [[HiddenLink]] %/ visible [[RealLink]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            !links.iter().any(|l| l.target == "HiddenLink"),
            "Links inside /% %/ comments should be filtered out"
        );
        assert!(
            links.iter().any(|l| l.target == "RealLink"),
            "Links outside /% %/ comments should be detected"
        );
    }

    #[test]
    fn html_comment_skips_links() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<!-- [[HiddenLink]] --> visible [[RealLink]]\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            !links.iter().any(|l| l.target == "HiddenLink"),
            "Links inside <!-- --> comments should be filtered out"
        );
        assert!(
            links.iter().any(|l| l.target == "RealLink"),
            "Links outside <!-- --> comments should be detected"
        );
    }

    #[test]
    fn line_comment_skips_refs_in_script_block() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>\n// Engine.play(\"Hidden\");\nEngine.play(\"Visible\");\n<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            !links.iter().any(|l| l.target == "Hidden"),
            "Engine.play inside // line comment should be filtered out"
        );
        assert!(
            links.iter().any(|l| l.target == "Visible"),
            "Engine.play outside line comment should be detected"
        );
    }

    #[test]
    fn line_comment_in_script_passage() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Story JavaScript [script]\n// Engine.play(\"Hidden\");\nEngine.play(\"Visible\");\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            !links.iter().any(|l| l.target == "Hidden"),
            "Engine.play inside // line comment in script passage should be filtered out"
        );
        assert!(
            links.iter().any(|l| l.target == "Visible"),
            "Engine.play outside line comment in script passage should be detected"
        );
    }

    #[test]
    fn twine_comment_skips_vars() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n/% <<set $hidden to 5>> %/ <<set $visible to 10>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let vars = &result.passages[0].vars;
        assert!(
            !vars.iter().any(|v| v.name == "$hidden"),
            "Variables inside /% %/ comments should be filtered out"
        );
        assert!(
            vars.iter().any(|v| v.name == "$visible"),
            "Variables outside /% %/ comments should be detected"
        );
    }

    // ── New implicit passage reference tests ──────────────────────────

    #[test]
    fn implicit_passage_ref_ui_goto() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>UI.goto(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect UI.goto() implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_ui_include() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>UI.include(\"Sidebar\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Sidebar"),
            "Should detect UI.include() implicit reference"
        );
    }

    #[test]
    fn implicit_passage_ref_story_has() {
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<script>>Story.has(\"Forest\");<</script>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        let links = &result.passages[0].links;
        assert!(
            links.iter().any(|l| l.target == "Forest"),
            "Should detect Story.has() implicit reference"
        );
    }

    // ── Macro block ordering and > in condition tests ────────────────

    #[test]
    fn extract_macros_sorted_by_position() {
        // Verify that extract_macros returns blocks in source order,
        // not open-then-close order. This is critical for build_body_blocks()
        // which assumes sorted input.
        use super::blocks;
        let body = "<<if $x>>yes<</if>>";
        let macros = blocks::extract_macros(body, 0);

        // Should be: open "if", close "/if" — in source order
        assert_eq!(macros.len(), 2, "Should find 2 macros");
        match &macros[0] {
            knot_core::passage::Block::Macro { name, .. } => {
                assert_eq!(name, "if", "First macro should be open 'if'");
            }
            _ => panic!("Expected Macro block"),
        }
        match &macros[1] {
            knot_core::passage::Block::Macro { name, .. } => {
                assert_eq!(name, "/if", "Second macro should be close '/if'");
            }
            _ => panic!("Expected Macro block"),
        }
    }

    #[test]
    fn extract_macros_nested_sorted() {
        // Nested macros with close tags between open tags — must be sorted
        // Source order: <<if>>, <<if>>, <</if>>, <<else>>, <</if>>
        use super::blocks;
        let body = "<<if $a>><<if $b>>yes<</if>><<else>>no<</if>>";
        let macros = blocks::extract_macros(body, 0);

        assert_eq!(macros.len(), 5, "Should find 5 macros");

        // Verify source order: open if, open if, close if, open else, close if
        let names: Vec<&str> = macros.iter().filter_map(|m| match m {
            knot_core::passage::Block::Macro { name, .. } => Some(name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(names, &["if", "if", "/if", "else", "/if"],
            "Macros must be in source order, not open-then-close order");

        // Verify spans are monotonically increasing
        let spans: Vec<usize> = macros.iter().filter_map(|m| match m {
            knot_core::passage::Block::Macro { span, .. } => Some(span.start),
            _ => None,
        }).collect();
        for i in 1..spans.len() {
            assert!(spans[i] > spans[i - 1],
                "Macro spans must be in increasing order: {:?} at index {}", spans, i);
        }
    }

    #[test]
    fn gt_in_condition_exhaustive() {
        // Exhaustive test for > in macro conditions — multiple patterns
        let plugin = SugarCubePlugin::new();

        let test_cases = vec![
            // (description, source)
            ("simple gt", ":: Start\n<<if $x > 0>>yes<</if>>\n"),
            ("gt with else", ":: Start\n<<if $x > 0>>yes<<else>>no<</if>>\n"),
            ("nested gt", ":: Start\n<<if $a > 1>>\n  <<if $b > 2>>inner<</if>>\n<<else>>\n  outer\n<</if>>\n"),
            ("gt with print shorthand", ":: Start\n<<if _parts.length > 0>>\n  <<= _parts[0] >>\n  <<if _parts.length > 1>> +<<= _parts.length - 1 >><</if>>\n<<else>>\n  &mdash;\n<</if>>\n"),
            ("multiple gt conditions", ":: Start\n<<if $x > 0>><<elseif $x > -1>>zero<<else>>neg<</if>>\n"),
            ("gte operator", ":: Start\n<<if $x >= 0>>yes<</if>>\n"),
        ];

        for (desc, src) in test_cases {
            let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

            assert!(
                !result.diagnostics.iter().any(|d| d.code == "sc-container-structure"),
                "[{}] <<else>>/<<elseif>> should NOT be flagged — > in condition should not break delimiter parsing. Diagnostics: {:?}",
                desc,
                result.diagnostics.iter().map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
            );

            assert!(
                !result.diagnostics.iter().any(|d| d.code == "sc-unclosed-macro"),
                "[{}] > in condition should not produce unclosed-macro warnings. Diagnostics: {:?}",
                desc,
                result.diagnostics.iter().map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn body_blocks_correct_order_with_close_tags() {
        // Verify that body blocks are in correct source order even when
        // close tags appear between open tags. This tests the sorting fix
        // in extract_macros().
        let plugin = SugarCubePlugin::new();
        let src = ":: Start\n<<if $a>>\n  <<if $b>>inner<</if>>\n<<else>>no\n<</if>>\n";
        let result = plugin.parse(&Url::parse("file:///test.twee").unwrap(), src);

        assert_eq!(result.passages.len(), 1);
        let passage = &result.passages[0];

        // Collect macro names and their spans from body blocks
        let macro_info: Vec<(&str, usize)> = passage.body.iter().filter_map(|b| match b {
            knot_core::passage::Block::Macro { name, span, .. } => Some((name.as_str(), span.start)),
            _ => None,
        }).collect();

        // Macros should appear in source order:
        // 1. open "if" (outer)
        // 2. open "if" (inner)
        // 3. close "/if" (inner)
        // 4. open "else"
        // 5. close "/if" (outer)
        assert!(macro_info.len() >= 5,
            "Expected at least 5 macro blocks, got {}: {:?}", macro_info.len(), macro_info);

        // Verify spans are in increasing order
        for i in 1..macro_info.len() {
            assert!(macro_info[i].1 > macro_info[i - 1].1,
                "Body blocks must be in source order: {:?}", macro_info);
        }
    }
}
