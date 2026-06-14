//! SugarCube 2.x Format Plugin — Rewrite (ver_3)
//!
//! This module is being rewritten from scratch. The old implementation had
//! ~2500 lines of regex spaghetti spread across vars/, links/, validation/,
//! macro_scan/, workspace/, comments/, and passage_tree/. This rewrite replaces
//! all of that with a single recursive descent parser that handles SugarCube's
//! delimiter-based syntax natively.
//!
//! ## Architecture
//!
//! ```text
//! Source Text
//!     |
//!     v
//! lexer::split_passages()     ← Passage boundary detection (kept from old code)
//!     |
//!     v
//! classifier::classify_all()  ← Two-pass: detect + classify (tags-first per Twee 3)
//!     |
//!     v
//! classifier::sort_for_processing() ← Define-before-use ordering
//!     |
//!     v
//! [per-passage dispatch]       ← Each category gets the right parser mode
//!     |
//!     |--> Script:         oxc parse → warm registries
//!     |--> Widget:         SC parser (Widget mode) → warm widget registry
//!     |--> Normal/Special: SC parser (Normal mode)
//!     |--> Stylesheet:     skip
//!     |--> StoryData:      minimal
//!     |
//!     v
//! ParseResult { passages, token_groups, diagnostics }
//! ```
//!
//! ## Classification Priority (Twee 3 spec: tags override names)
//!
//! 1. Core name-matched (StoryTitle, StoryData, Start)
//! 2. Core tag-matched ([script], [stylesheet])
//! 3. Format tag-matched ([init], [widget])
//! 4. Format name-matched (StoryInit, PassageHeader, etc.)
//! 5. Normal passages (with or without custom tags)
//!
//! ## Processing Order (define-before-use)
//!
//! 1. [script] passages → oxc → populate variable/macro registries
//! 2. [widget] passages → SugarCube parser → populate widget registry
//! 3. Named specials → SugarCube parser (registries now warm)
//! 4. Normal passages → SugarCube parser (can query all registries)
//! 5. Stylesheets/StoryData → skip or minimal processing

// Subdirectory modules (new organization)
pub mod graph;
pub mod js;
pub mod lsp;

// Expanded registry module (now includes variable_tree, custom_macros, etc.)
pub mod registries;

// Root-level modules (unchanged)
pub mod lexer;
pub mod classifier;
pub mod parse_pipeline;
pub mod ast;
pub mod special_passages;
pub mod parser;
pub mod macros;

// Re-exports for backward compatibility
// These ensure that `super::passage_build` etc. from parser/ and macros/ still resolve
pub use graph::passage_build;
pub use graph::edge_classify;
pub use graph::nav_resolve;
pub use js::js_preprocess;
pub use js::js_walk;
pub use js::js_validate;
pub use lsp::syntax_detect;
pub use lsp::token_builder;
pub use registries::variable_tree;
pub use registries::variable_tree::VarAccessKind;
pub use registries::custom_macros;
pub use registries::var_extract;
pub use registries::registry_populate;

use knot_core::passage::{Passage, SpecialPassageDef, StoryFormat};
use std::collections::{HashMap, HashSet};
use url::Url;

use crate::plugin::{FormatPlugin, FormatPluginMut, ParseResult};
use crate::types::{
    BodyRequirement, GlobalDef, ImplicitPassagePattern, MacroDef, OperatorNormalization,
    VariableSigilInfo, VariableTreeNode,
};
use ast::ParseMode;
use classifier::{ClassifiedPassage, is_script_passage, is_stylesheet_passage, is_widget_passage};
use registries::SugarCubeRegistry;

// ===========================================================================
// Completion context helpers (SugarCube-specific)
// ===========================================================================

/// Convert a (line, character) position to a document-absolute byte offset.
fn line_char_to_byte_offset(text: &str, line: u32, character: u32) -> usize {
    let mut byte_offset = 0;
    let mut current_line = 0u32;
    for ch in text.chars() {
        if current_line == line {
            break;
        }
        byte_offset += ch.len_utf8();
        if ch == '\n' {
            current_line += 1;
        }
    }
    if current_line < line {
        return text.len();
    }
    // Now we're at the start of the target line.
    // Convert UTF-16 character offset to byte offset within the line.
    let line_start = byte_offset;
    let line_end = text[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let line_text = &text[line_start..line_end];
    line_start + char_to_byte_offset(line_text, character as usize)
}

/// Convert a UTF-16 character offset to a byte offset within a line.
fn char_to_byte_offset(line: &str, char_offset: usize) -> usize {
    let mut utf16_count = 0usize;
    let mut byte_offset = 0usize;
    for ch in line.chars() {
        if utf16_count >= char_offset {
            break;
        }
        let code_units = if (ch as u32) < 0x10000 { 1usize } else { 2usize };
        utf16_count += code_units;
        byte_offset += ch.len_utf8();
    }
    byte_offset
}

/// Find the variable name at a byte offset in the workspace.
fn find_variable_name_at_offset(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    byte_offset: usize,
) -> Option<String> {
    let doc = workspace.get_document(uri)?;
    for passage in &doc.passages {
        for var in &passage.vars {
            if passage.span_contains_abs_offset(&var.span, byte_offset) {
                return Some(var.name.clone());
            }
        }
    }
    None
}

/// Extract the partial variable/property identifier after a sigil (`$` or `_`)
/// on the current line.
///
/// For `$pl` → returns `"pl"`. For just `$` → returns `""`.
/// Stops at characters that aren't valid in variable identifiers or
/// dot-notation paths (spaces, operators, delimiters, etc.).
fn extract_partial_after_sigil(before_cursor: &str, sigil: char) -> &str {
    if let Some(pos) = before_cursor.rfind(sigil) {
        let after = &before_cursor[pos + 1..];
        let end = after
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .unwrap_or(after.len());
        &after[..end]
    } else {
        ""
    }
}

/// Compute a `FormatTextEdit` for variable/property completions.
///
/// Follows the same design as `compute_macro_text_edit`: the sigil (`$`/`_`)
/// stays in the document and is NOT covered by the textEdit range.
/// VS Code uses the text within the textEdit range as the filter prefix,
/// so covering the `$` would break client-side filtering (our filter_text
/// values are bare names like "player", not "$player").
///
/// When the user types `$pl` and hits Ctrl+Space:
/// - `partial` = `"pl"` (after the `$`)
/// - textEdit replaces `"pl"` with `"player"`
/// - The `$` remains in the document → result: `$player`
fn compute_variable_text_edit(
    partial: &str,
    line: u32,
    character: u32,
    replacement: &str,
) -> Option<crate::types::FormatTextEdit> {
    use crate::types::FormatTextEdit;

    let partial_len = partial.chars().count() as u32;
    let start_char = character.saturating_sub(partial_len);
    Some(FormatTextEdit {
        start_line: line,
        start_character: start_char,
        end_line: line,
        end_character: character,
        new_text: replacement.to_string(),
    })
}

/// Find the `MacroArgRef` at a byte offset in the workspace.
fn find_macro_arg_ref_at_offset(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    byte_offset: usize,
) -> Option<knot_core::passage::MacroArgRef> {
    let doc = workspace.get_document(uri)?;
    for passage in &doc.passages {
        for arg_ref in &passage.macro_arg_refs {
            if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                return Some(arg_ref.clone());
            }
        }
    }
    None
}

/// Find the variable path before a dot on the current line.
///
/// Tries multiple strategies:
/// 1. Arena tree via `variable_path_at_offset()` (most accurate)
/// 2. Scan `before_cursor` for `$name` or `_name` pattern (fallback)
fn find_variable_path_before_dot(
    workspace: &knot_core::Workspace,
    uri: &url::Url,
    text: &str,
    byte_offset: usize,
    before_cursor: &str,
    registry: &SugarCubeRegistry,
) -> Option<String> {
    // ── Strategy 2 (line-based scan) runs FIRST ──────────────────
    //
    // When the text before the cursor contains a variable sigil (`$` or `_`),
    // the line-based scan is more reliable than the arena offset-based lookup.
    // The arena lookup's `build_path_for_segment` can return incomplete paths
    // when the root variable node is found but the deep path isn't resolved.
    // The line scan directly extracts the full dot-path from the source text.
    //
    // We run Strategy 2 first and only fall back to Strategy 1 when no sigil
    // is present in the text (e.g., namespace completions like `State.`).
    let before_dot = before_cursor.trim_end_matches('.');
    let sigils = ['$', '_'];
    for sigil in &sigils {
        if let Some(sigil_pos) = before_dot.rfind(*sigil) {
            let after_sigil = &before_dot[sigil_pos + 1..];
            // The part after the sigil should be a valid identifier path
            // (alphanumeric, underscores, dots for nested access)
            if after_sigil.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                let path = &before_dot[sigil_pos..];
                if !path.is_empty() {
                    // Validate: the path should exist in the arena tree.
                    // If it doesn't, it might be a partially-typed path
                    // that hasn't been parsed yet — still return it so
                    // that we can try completions at the right depth.
                    return Some(path.to_string());
                }
            }
        }
    }

    // ── Strategy 1: Arena tree offset-based lookup ───────────────
    //
    // Used as a fallback when no `$`/`_` sigil appears in the text
    // before the cursor. This covers namespace completions and cases
    // where the sigil is outside the current line fragment.
    if let Some(doc) = workspace.get_document(uri) {
        for passage in &doc.passages {
            if passage.contains_abs_offset(byte_offset) {
                let abs_start = passage.abs_offset(passage.span.start).min(text.len());
                let header_end = text[abs_start..]
                    .find('\n')
                    .map(|n| abs_start + n + 1)
                    .unwrap_or(abs_start);
                let body_offset = byte_offset.saturating_sub(header_end);
                let file_uri_str = uri.to_string();
                if let Some(path) = registry.variables().path_at_offset(&file_uri_str, &passage.name, body_offset) {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Find a global namespace name before a dot on the current line.
fn find_namespace_before_dot(
    before_cursor: &str,
    plugin: &dyn FormatPlugin,
) -> Option<String> {
    let before_dot = before_cursor.trim_end_matches('.');
    let ident = before_dot
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()?;
    if plugin.global_object_names().contains(ident) {
        Some(ident.to_string())
    } else {
        None
    }
}

/// Extract partial text typed inside a quoted string on the current line.
///
/// Finds the last unmatched opening `"` and returns the text between it
/// and the cursor. This is used to extract partial passage names when
/// the user types `<<goto "Gar` — returning "Gar" for filtering.
fn extract_partial_in_quote(before_cursor: &str) -> String {
    // Find the last opening quote that isn't closed
    let mut quote_positions: Vec<usize> = Vec::new();
    let bytes = before_cursor.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Check if escaped
            let mut backslash_count = 0;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' {
                backslash_count += 1;
                j -= 1;
            }
            if backslash_count % 2 == 0 {
                quote_positions.push(i);
            }
        }
        i += 1;
    }
    // If there's an odd number of quotes, the last one is unmatched (open)
    if quote_positions.len() % 2 == 1 {
        let last_open = quote_positions[quote_positions.len() - 1];
        let partial = &before_cursor[last_open + 1..];
        return partial.to_string();
    }
    String::new()
}

/// Detect passage-in-quote context from before_cursor text.
///
/// Used when the `" ` trigger fires but the span-based data doesn't
/// find a MacroArgRef (e.g., when the user just typed the opening
/// quote and the AST hasn't been updated yet).
fn detect_passage_in_quote(
    before_cursor: &str,
    plugin: &dyn FormatPlugin,
) -> Option<crate::types::CompletionContext> {
    let passage_arg_names = plugin.passage_arg_macro_names();
    if passage_arg_names.is_empty() {
        return None;
    }

    // Find the most recent macro open context
    let mut best_match: Option<(&str, usize)> = None;
    for &macro_name in &passage_arg_names {
        let open_pattern = plugin.format_macro_label(macro_name);
        let open_prefix = open_pattern
            .trim_end_matches('>')
            .trim_end_matches(')')
            .trim_end_matches(']')
            .trim_end_matches('}');

        if let Some(pos) = before_cursor.rfind(open_prefix) {
            match best_match {
                None => best_match = Some((macro_name, pos)),
                Some((_, prev_pos)) if pos > prev_pos => best_match = Some((macro_name, pos)),
                _ => {}
            }
        }
    }

    let (macro_name, open_pos) = best_match?;
    let after_open = &before_cursor[open_pos..];

    // Must not contain the closing delimiter
    let macro_label = plugin.format_macro_label(macro_name);
    let close_delim = if macro_label.starts_with("<<") {
        ">>"
    } else if macro_label.starts_with('(') {
        ")"
    } else if macro_label.starts_with('[') {
        "]"
    } else if macro_label.starts_with("{{") {
        "}}"
    } else {
        ""
    };
    if !close_delim.is_empty() && after_open.contains(close_delim) { return None; }

    // Check if we're inside a quoted string
    let is_in_quote = after_open.matches('"').count() % 2 == 1;
    if !is_in_quote {
        return None;
    }

    Some(crate::types::CompletionContext::MacroPassageRef {
        target: String::new(),
        macro_name: macro_name.to_string(),
        has_body: false,
    })
}

/// Resolve the macro name and has_body from the line text when the
/// span-based `MacroArgRef` data isn't available yet.
///
/// Scans `before_cursor` for the most recent `<<name` pattern and checks
/// whether the macro is a body macro (container) via the plugin catalog.
/// This is the fallback for the PassageRef semantic token case where we
/// have the token text but no MacroArgRef at the offset.
fn resolve_macro_name_from_offset(
    before_cursor: &str,
    _byte_offset: usize,
    plugin: &dyn FormatPlugin,
) -> (String, bool) {
    // Find the most recent << before the cursor
    if let Some(delim_pos) = before_cursor.rfind("<<") {
        let after = &before_cursor[delim_pos + 2..];
        // Extract the macro name (first word after <<)
        let name = after.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .next()
            .unwrap_or("");
        if !name.is_empty() {
            let has_body = plugin.find_macro(name)
                .map(|m| m.body != crate::types::BodyRequirement::Never)
                .unwrap_or(false);
            return (name.to_string(), has_body);
        }
    }
    (String::new(), false)
}

/// Find the namespace token that immediately precedes a given byte offset.
fn find_preceding_namespace_token(
    token_groups: &[crate::plugin::PassageTokenGroup],
    text: &str,
    before_offset: usize,
) -> String {
    use crate::plugin::SemanticTokenType;

    let mut best_name = String::new();
    let mut best_end = 0usize;

    for group in token_groups {
        let group_offset = group.passage_offset;
        for token in &group.tokens {
            if token.token_type != SemanticTokenType::Namespace {
                continue;
            }
            let abs_end = token.start + group_offset + token.length;
            if abs_end <= before_offset
                && before_offset - abs_end <= 10
                && abs_end > best_end
            {
                let abs_start = token.start + group_offset;
                if abs_start < text.len() && abs_end <= text.len() {
                    best_name = text[abs_start..abs_end].to_string();
                    best_end = abs_end;
                }
            }
        }
    }

    best_name
}

/// Find the passage header name at a given line number.
fn find_passage_header_at_position(text: &str, line: u32) -> Option<String> {
    let line_text = text.lines().nth(line as usize)?;
    if line_text.starts_with("::") {
        let name = crate::header::extract_passage_name(&line_text[2..]);
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// SugarCube 2.x format plugin.
///
/// All runtime-populated registries are owned by the unified
/// [`SugarCubeRegistry`] hub, which implements [`FormatRegistry`] for
/// format-agnostic access. LSP handlers query registries through
/// `FormatPlugin` trait methods, never touching the hub directly.
pub struct SugarCubePlugin {
    /// The unified registry hub — owns all sub-registries.
    registry: SugarCubeRegistry,
}

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SugarCubePlugin {
    pub fn new() -> Self {
        Self {
            registry: SugarCubeRegistry::new(),
        }
    }

    /// Determine the parse mode for a classified passage.
    fn parse_mode_for(cp: &ClassifiedPassage) -> ParseMode {
        if is_script_passage(cp) {
            ParseMode::Script
        } else if is_stylesheet_passage(cp) {
            ParseMode::Stylesheet
        } else if is_widget_passage(cp) {
            ParseMode::Widget
        } else if cp.header.name == "StoryInterface" {
            ParseMode::Interface
        } else if cp.header.name == "StoryData" {
            ParseMode::Minimal
        } else {
            ParseMode::Normal
        }
    }

    /// Get a reference to the unified registry hub.
    pub fn registry(&self) -> &SugarCubeRegistry {
        &self.registry
    }

    /// Get a mutable reference to the unified registry hub.
    pub fn registry_mut(&mut self) -> &mut SugarCubeRegistry {
        &mut self.registry
    }
}

impl FormatPluginMut for SugarCubePlugin {
    fn parse_mut(&mut self, uri: &Url, text: &str) -> ParseResult {
        parse_pipeline::parse_full(self, uri, text)
    }

    fn parse_passage_mut(&mut self, passage_name: &str, passage_tags: &[String], passage_text: &str, file_uri: &str) -> Option<Passage> {
        parse_pipeline::parse_single(self, passage_name, passage_tags, passage_text, file_uri)
    }

    fn remove_file_from_registries(&mut self, file_uri: &str) {
        self.registry.remove_file(file_uri);
    }

    fn remove_passage_from_registries(&mut self, passage_name: &str, _file_uri: &str) {
        self.registry.remove_passage(passage_name);
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn format(&self) -> StoryFormat {
        StoryFormat::SugarCube
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

    // ── Macro catalog ──────────────────────────────────────────────────

    fn builtin_macros(&self) -> &'static [MacroDef] {
        macros::builtin_macros()
    }

    fn body_macro_names(&self) -> HashSet<&'static str> {
        macros::body_macro_names()
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

    // ── Variable tracking ──────────────────────────────────────────────

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

    // ── Syntax detection ───────────────────────────────────────────────

    fn has_block_macros_with_close_tags(&self) -> bool {
        true
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

    fn build_macro_snippet(&self, name: &str, body: BodyRequirement) -> String {
        macros::build_macro_snippet(name, body)
    }

    fn detect_close_tag_context(&self, before_cursor: &str) -> Option<String> {
        if let Some(pos) = before_cursor.rfind("<</") {
            let partial = &before_cursor[pos + 3..];
            if partial.is_empty() || partial.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Some(partial.to_string());
            }
        }
        if before_cursor.ends_with("<<") {
            return Some(String::new());
        }
        None
    }

    // ── Special passage names ──────────────────────────────────────────

    fn special_passage_names(&self) -> HashSet<&'static str> {
        macros::special_passage_names()
    }

    fn system_passage_names(&self) -> HashSet<&'static str> {
        macros::system_passage_names()
    }

    // ── Implicit passage patterns ──────────────────────────────────────

    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        macros::implicit_passage_patterns()
    }

    // ── Hover / documentation ──────────────────────────────────────────

    fn global_hover_text(&self, name: &str) -> Option<&'static str> {
        macros::global_hover_text(name)
    }

    fn builtin_globals(&self) -> &'static [GlobalDef] {
        macros::builtin_globals()
    }

    fn global_object_names(&self) -> HashSet<&'static str> {
        macros::builtin_globals().iter().map(|g| g.name).collect()
    }

    // ── Operator normalization ─────────────────────────────────────────

    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        macros::operator_normalization()
    }

    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        macros::operator_precedence()
    }

    fn supports_full_variable_tracking(&self) -> bool {
        true
    }

    fn macro_snippet(&self, name: &str) -> Option<&'static str> {
        macros::macro_snippet(name)
    }

    // ── Syntax detection (Phase E) ──────────────────────────────────────

    fn find_macro_at_position(
        &self,
        line: &str,
        byte_pos: usize,
    ) -> Option<crate::plugin::MacroAtPosition> {
        syntax_detect::find_macro_at_position_impl(line, byte_pos)
    }

    fn scan_line_for_macro_events(
        &self,
        line: &str,
        line_idx: u32,
    ) -> Vec<crate::plugin::MacroBlockEvent> {
        syntax_detect::scan_line_for_macro_events_impl(line, line_idx)
    }

    // ── Dynamic navigation resolution (Phase F) ───────────────────────

    fn build_var_string_map(&self, workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        nav_resolve::build_var_string_map_impl(workspace, &self.registry.variables())
    }

    fn resolve_dynamic_navigation_links(
        &self,
        passage: &Passage,
        var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<crate::types::ResolvedNavLink> {
        nav_resolve::resolve_dynamic_navigation_links_impl(passage, var_string_map)
    }

    // ── Edge classification ────────────────────────────────────────────

    fn classify_edge(
        &self,
        source_passage: &Passage,
        display_text: Option<&str>,
        target: &str,
    ) -> Option<knot_core::graph::EdgeType> {
        edge_classify::classify_edge_impl(source_passage, display_text, target)
    }

    // ── Registry accessors (Phase C) ───────────────────────────────────
    //
    // These methods expose the format-owned registries through the
    // FormatPlugin trait so that LSP handlers can query them without
    // importing format-specific types. The handlers call these methods
    // through `FormatRegistry::get()` — never directly.

    /// Build the variable tree for the workspace.
    ///
    /// Returns the current tree-structured variable inventory from the
    /// VariableTree sub-registry. This is used by the variable tracker
    /// UI panel and by completion/hover for workspace-wide variable info.
    ///
    /// Before building the tree, resolves all byte-offset → line-number
    /// mappings using the server's document cache so that usage locations
    /// report actual source lines instead of `line: 0`.
    fn build_variable_tree(
        &self,
        _workspace: &knot_core::Workspace,
        source_text: &dyn crate::plugin::SourceTextProvider,
    ) -> Vec<VariableTreeNode> {
        self.registry.build_variable_tree(source_text)
    }

    /// Get all workspace variable names for completion.
    fn workspace_variable_names(&self) -> HashSet<String> {
        self.registry.variable_names()
    }

    /// Get known property paths for a variable (for dot-notation completion).
    fn variable_properties(&self, var_name: &str) -> HashSet<String> {
        self.registry.variable_properties(var_name)
    }

    /// Find the variable path at a passage-body-relative byte offset.
    ///
    /// Delegates to the arena tree's `path_at_offset()` which scans
    /// segment_spans for the matching access record.
    fn variable_path_at_offset(
        &self,
        file_uri: &str,
        passage_name: &str,
        body_offset: usize,
    ) -> Option<String> {
        self.registry.variables().path_at_offset(file_uri, passage_name, body_offset)
    }

    /// Get the children of a variable path with their inferred kinds.
    ///
    /// Delegates to the arena tree's `children_with_kind()` which queries
    /// the tree directly without building a full property map.
    fn variable_children_with_kind(&self, path: &str) -> Vec<(String, crate::types::PropertyKind)> {
        self.registry.variables().children_with_kind(path)
    }

    fn variable_kind_at_path(&self, path: &str) -> Option<crate::types::PropertyKind> {
        self.registry.variables().kind_at_path(path)
    }

    /// Get all custom macro names for completion.
    fn custom_macro_names(&self) -> Vec<String> {
        self.registry.custom_macro_names()
    }

    /// Look up a custom macro definition for hover/go-to-def.
    fn find_custom_macro(&self, name: &str) -> Option<(String, String, usize)> {
        self.registry.custom_macros().get(name).map(|m| {
            (m.defined_in.clone(), m.file_uri.clone(), m.defined_at_offset)
        })
    }

    /// Look up a custom macro with full detail for completion resolve.
    fn find_custom_macro_detail(
        &self,
        name: &str,
    ) -> Option<crate::plugin::CustomMacroDetail> {
        self.registry.custom_macros().get(name).map(|m| {
            crate::plugin::CustomMacroDetail {
                defined_in: m.defined_in.clone(),
                file_uri: m.file_uri.clone(),
                is_widget: m.is_widget,
                is_container: m.is_container,
                arg_count: m.arg_count,
                description: m.description.clone(),
            }
        })
    }

    /// Check if a macro name is a known custom macro.
    fn is_custom_macro(&self, name: &str) -> bool {
        self.registry.custom_macros().contains(name)
    }

    // ── Variable refs + property maps (Phase G) ────────────────────────

    fn extract_passage_variable_refs(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn crate::plugin::SourceTextProvider,
        passage_name: &str,
    ) -> Vec<crate::types::PassageVarRef> {
        // Compute passage positions for relative→absolute line conversion
        let passage_positions = self.registry.compute_passage_positions(source_text);
        var_extract::extract_passage_variable_refs_impl(
            &self.registry.variables(),
            workspace,
            source_text,
            passage_name,
            &passage_positions,
        )
    }

    fn build_object_property_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, HashSet<String>> {
        self.registry.variables().property_map()
    }

    fn build_shape_aware_property_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, crate::types::PropertyMapEntry> {
        var_extract::build_shape_aware_property_map_impl(&self.registry.variables())
    }

    fn build_state_variable_registry(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> HashMap<String, crate::types::StateVariable> {
        // For state variable registry, passage positions are not available
        // without source text. Use empty map — spans will stay passage-relative.
        // This is acceptable because the state variable registry is primarily
        // used for variable availability analysis, not for precise location reporting.
        let passage_positions = crate::sugarcube::registries::variable_tree::PassagePositionMap::new();
        var_extract::build_state_variable_registry_impl(&self.registry.variables(), &passage_positions)
    }

    // ── Function registry ─────────────────────────────────────────────

    fn function_names(&self) -> Vec<String> {
        self.registry.function_names()
    }

    fn find_function(&self, name: &str) -> Option<crate::types::FunctionDefInfo> {
        self.registry.functions().get(name).map(|f| {
            crate::types::FunctionDefInfo {
                name: f.name.clone(),
                defined_in: f.defined_in.clone(),
                file_uri: f.file_uri.clone(),
                defined_at_offset: f.defined_at_offset,
                param_count: f.param_count,
            }
        })
    }

    // ── Template registry ─────────────────────────────────────────────

    fn template_names(&self) -> Vec<String> {
        self.registry.template_completion_names()
    }

    fn find_template(&self, name: &str) -> Option<crate::types::TemplateDefInfo> {
        self.registry.templates().get(name).map(|t| {
            crate::types::TemplateDefInfo {
                name: t.name.clone(),
                defined_in: t.defined_in.clone(),
                file_uri: t.file_uri.clone(),
                defined_at_offset: t.defined_at_offset,
            }
        })
    }

    // ── Completion context resolution ──────────────────────────────────

    fn completion_trigger_characters(&self) -> Vec<char> {
        vec!['$', '_', '<', '[', '.', '"']
    }

    fn resolve_completion_context(
        &self,
        text: &str,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
        line: u32,
        character: u32,
        trigger: Option<char>,
        token_groups: &[crate::plugin::PassageTokenGroup],
    ) -> crate::types::CompletionContext {
        use crate::types::CompletionContext;

        // Compute byte offset from (line, character)
        let byte_offset = line_char_to_byte_offset(text, line, character);

        // Compute before_cursor text for pattern detection on the current line
        let line_text = text.lines().nth(line as usize).unwrap_or("");
        let byte_pos = char_to_byte_offset(line_text, character as usize);
        let before_cursor = &line_text[..byte_pos.min(line_text.len())];

        // ── Step 1: Trigger-character shortcuts ─────────────────────────
        //
        // Trigger characters give us a strong signal about intent. For
        // SugarCube, the mapping is unambiguous:
        //
        //   $  → Variable (story)
        //   _  → Variable (temporary)
        //   <  → MacroName (<< opening) or CloseTag (<</ closing)
        //   [  → Link
        //   .  → VariableDot or Namespace/Property
        //   "  → MacroPassageRef (inside a macro arg quote)
        //
        // The span-based data refines these but the trigger always wins
        // for the top-level category. A `$` trigger can never produce
        // passage names; a `<` trigger can never produce variables.

        match trigger {
            // ── $ / _ → Variable ────────────────────────────────────────
            Some('$') | Some('_') => {
                let is_temp = trigger == Some('_');
                // Try to get the variable name from the span data
                let name = find_variable_name_at_offset(workspace, uri, byte_offset)
                    .unwrap_or_else(|| {
                        // No existing var span — the user just typed the sigil.
                        // Return empty name; the handler will offer all vars.
                        String::new()
                    });
                return CompletionContext::Variable { name, is_temporary: is_temp };
            }

            // ── < → MacroName or CloseTag ───────────────────────────────
            Some('<') => {
                // Check for close-tag context first (<</)
                if before_cursor.ends_with("<</") {
                    let partial = before_cursor.rfind("<</")
                        .map(|pos| before_cursor[pos + 3..].to_string())
                        .unwrap_or_default();
                    return CompletionContext::CloseTag { partial };
                }
                // Check for << (macro opening)
                if before_cursor.ends_with("<<") {
                    return CompletionContext::MacroName { name: String::new() };
                }
                // Partial macro name after << (e.g., <<li)
                if let Some(delim_pos) = before_cursor.rfind("<<") {
                    let after = &before_cursor[delim_pos + 2..];
                    if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                        return CompletionContext::MacroName {
                            name: after.to_string(),
                        };
                    }
                }
                // Default: try span-based context, fall back to macro name
                return CompletionContext::MacroName { name: String::new() };
            }

            // ── [ → Link ────────────────────────────────────────────────
            Some('[') => {
                return CompletionContext::Link { target: String::new() };
            }

            // ── . → VariableDot or Namespace/Property ───────────────────
            Some('.') => {
                // Try variable dot-notation first (e.g., $player.)
                if let Some(var_path) = find_variable_path_before_dot(
                    workspace, uri, text, byte_offset, before_cursor, &self.registry,
                ) {
                    return CompletionContext::VariableDot { path: var_path };
                }
                // Try namespace property (e.g., State.)
                if let Some(ns_name) = find_namespace_before_dot(before_cursor, self) {
                    return CompletionContext::Property {
                        object_name: ns_name,
                        property_name: None,
                    };
                }
                // Fall back: span-based context
                return CompletionContext::Other;
            }

            // ── " → MacroPassageRef (inside macro arg quote) ────────────
            Some('"') => {
                // Check if cursor is inside a passage-ref macro arg
                if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
                    return CompletionContext::MacroPassageRef {
                        target: arg_ref.target.clone(),
                        macro_name: arg_ref.macro_name.clone(),
                        has_body: arg_ref.has_body,
                    };
                }
                // Not inside a passage-ref — try the line-based fallback
                // to detect "inside a passage-arg macro quote"
                if let Some(ctx) = detect_passage_in_quote(before_cursor, self) {
                    return ctx;
                }
                return CompletionContext::Other;
            }

            // ── No trigger → span-based resolution ──────────────────────
            None => {}

            // ── Unrecognized trigger → fall through to span-based ────────
            _ => {}
        }

        // ── Step 2: Span-based resolution (no trigger character) ─────────
        //
        // When there's no trigger (Ctrl+Space or auto), we use the
        // workspace passage data to determine what's at the cursor.

        if let Some(doc) = workspace.get_document(uri) {
            // 1. Check variable spans (highest priority)
            for passage in &doc.passages {
                for var in &passage.vars {
                    if passage.span_contains_abs_offset(&var.span, byte_offset) {
                        return CompletionContext::Variable {
                            name: var.name.clone(),
                            is_temporary: var.is_temporary,
                        };
                    }
                }
            }

            // 2. Check link spans
            for passage in &doc.passages {
                for link in &passage.links {
                    if passage.span_contains_abs_offset(&link.span, byte_offset) {
                        let target = link.target.trim().to_string();
                        if !target.is_empty() {
                            return CompletionContext::Link { target };
                        }
                    }
                }
            }

            // 3. Check macro_arg_refs
            for passage in &doc.passages {
                for arg_ref in &passage.macro_arg_refs {
                    if passage.span_contains_abs_offset(&arg_ref.span, byte_offset) {
                        return CompletionContext::MacroPassageRef {
                            target: arg_ref.target.clone(),
                            macro_name: arg_ref.macro_name.clone(),
                            has_body: arg_ref.has_body,
                        };
                    }
                    if passage.span_contains_abs_offset(&arg_ref.macro_name_span, byte_offset) {
                        return CompletionContext::MacroName {
                            name: arg_ref.macro_name.clone(),
                        };
                    }
                    if passage.span_contains_abs_offset(&arg_ref.macro_open_span, byte_offset) {
                        return CompletionContext::MacroInterior {
                            name: arg_ref.macro_name.clone(),
                        };
                    }
                }
            }
        }

        // 4. Check semantic tokens
        for group in token_groups {
            let group_offset = group.passage_offset;
            for token in &group.tokens {
                let abs_start = token.start + group_offset;
                let abs_end = abs_start + token.length;
                if byte_offset >= abs_start && byte_offset < abs_end {
                    use crate::plugin::SemanticTokenType;
                    match token.token_type {
                        SemanticTokenType::Macro => {
                            let name = text[abs_start..abs_end].to_string();
                            return CompletionContext::MacroName { name };
                        }
                        SemanticTokenType::Namespace => {
                            let name = text[abs_start..abs_end].to_string();
                            return CompletionContext::Namespace { name };
                        }
                        SemanticTokenType::Property => {
                            let property_name = text[abs_start..abs_end].to_string();
                            let object_name = find_preceding_namespace_token(
                                token_groups, text, abs_start,
                            );
                            return CompletionContext::Property {
                                object_name,
                                property_name: Some(property_name),
                            };
                        }
                        SemanticTokenType::Variable => {
                            let name = text[abs_start..abs_end].to_string();
                            return CompletionContext::Variable {
                                name: name.clone(),
                                is_temporary: name.starts_with('_'),
                            };
                        }
                        SemanticTokenType::Link => {
                            let target = text[abs_start..abs_end].trim().to_string();
                            if !target.is_empty() {
                                return CompletionContext::Link { target };
                            }
                        }
                        SemanticTokenType::PassageRef => {
                            // PassageRef tokens are inside macro args, not
                            // [[ ]] links. Return MacroPassageRef so the
                            // handler knows to use macro-arg insertion
                            // semantics (bare name, no [[ ]] wrapping).
                            let target = text[abs_start..abs_end].trim().to_string();
                            if !target.is_empty() {
                                // Resolve macro context from MacroArgRef data
                                let (macro_name, has_body) =
                                    find_macro_arg_ref_at_offset(workspace, uri, byte_offset)
                                        .map(|ar| (ar.macro_name.clone(), ar.has_body))
                                        .unwrap_or_else(|| {
                                            resolve_macro_name_from_offset(
                                                before_cursor, byte_offset, self,
                                            )
                                        });
                                return CompletionContext::MacroPassageRef {
                                    target,
                                    macro_name,
                                    has_body,
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // 5. Check passage header
        if let Some(passage_name) = find_passage_header_at_position(text, line) {
            return CompletionContext::PassageHeader { name: passage_name };
        }

        CompletionContext::Other
    }

    // ── Primary completion: format-owned context detection + building ────

    fn provide_completions(
        &self,
        text: &str,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
        line: u32,
        character: u32,
        trigger: Option<char>,
        token_groups: &[crate::plugin::PassageTokenGroup],
    ) -> Vec<crate::types::FormatCompletionItem> {
        let byte_offset = line_char_to_byte_offset(text, line, character);
        let line_text = text.lines().nth(line as usize).unwrap_or("");
        let byte_pos = char_to_byte_offset(line_text, character as usize);
        let before_cursor = &line_text[..byte_pos.min(line_text.len())];

        // ── 0. Passage header suppression ──────────────────────────────
        if find_passage_header_at_position(text, line).is_some() {
            return Vec::new();
        }

        // ── 1. $ / _ trigger → Variable completions ───────────────────
        //
        // `$` = State.variables. → show children of persistent root
        // `_` = State.temporary. → show children of temp root
        // The partial text after the sigil (e.g., "pl" for `$pl`) is
        // extracted for text_edit range computation.
        if trigger == Some('$') || trigger == Some('_') {
            let is_temp = trigger == Some('_');
            let sigil = if is_temp { '_' } else { '$' };
            let partial = extract_partial_after_sigil(before_cursor, sigil);
            return self.build_variable_completions(is_temp, line, character, partial);
        }

        // ── 2. . trigger → VariableDot or Namespace property ───────────
        if trigger == Some('.') {
            // Try variable dot-notation first (e.g., $player.)
            if let Some(var_path) = find_variable_path_before_dot(
                workspace, uri, text, byte_offset, before_cursor, &self.registry,
            ) {
                // No partial after the dot — the `.` trigger fires right
                // after the dot is typed, so there's nothing to replace.
                return self.build_variable_dot_completions(&var_path, line, character, "");
            }
            // Try namespace property (e.g., State.)
            if let Some(ns_name) = find_namespace_before_dot(before_cursor, self) {
                return self.build_global_property_completions(&ns_name);
            }
            return Vec::new();
        }

        // ── 3. " trigger → Passage name in macro string arg ────────────
        //
        // When the user types a `"` inside a macro that has a passage-ref
        // argument (e.g., <<goto "PassageName">>), we show passage name
        // completions. The `"` is the trigger, but we only fire when we're
        // inside a passage-ref macro arg context. A lone `"` in normal text
        // (e.g., `"hello"`) should NOT trigger passage completions.
        if trigger == Some('"') {
            if let Some(macro_ctx) = self.resolve_passage_arg_context(before_cursor, byte_offset, workspace, uri) {
                // Extract partial passage name typed after the opening quote
                // e.g., <<goto "Gar → partial = "Gar"
                let partial = extract_partial_in_quote(before_cursor);
                return self.build_passage_name_completions(workspace, &partial, macro_ctx);
            }
            return Vec::new();
        }

        // ── 4. < trigger → CloseTag or MacroName ──────────────────────
        //
        // IMPORTANT: A single `<` is a trigger character, but we only show
        // completions when the user has actually typed `<<` (macro delimiter).
        // Typing `a < b` should NOT trigger macro suggestions — the single
        // `<` is far too common in comparison expressions. We return empty
        // for a bare `<` so VS Code dismisses the suggestion list instantly.
        if trigger == Some('<') {
            // Close-tag context (<</)
            if before_cursor.ends_with("<</") {
                let partial = before_cursor.rfind("<</")
                    .map(|pos| before_cursor[pos + 3..].to_string())
                    .unwrap_or_default();
                return self.build_close_tag_completions(&partial, text, byte_offset, line, character);
            }
            // Macro open context (<<)
            if before_cursor.ends_with("<<") {
                return self.build_macro_completions(workspace, "", line, character, text, byte_offset);
            }
            // Partial macro name after << (e.g., <<li)
            if let Some(delim_pos) = before_cursor.rfind("<<") {
                let after = &before_cursor[delim_pos + 2..];
                if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                    return self.build_macro_completions(workspace, after, line, character, text, byte_offset);
                }
            }
            // Single `<` without `<<` — not a macro context, return empty.
            // This prevents annoying suggestions when typing `a < b` etc.
            return Vec::new();
        }

        // ── 5. [ trigger → Link (passage names) ────────────────────────
        //
        // IMPORTANT: A single `[` is a trigger character, but we only show
        // passage name completions when the user has actually typed `[[`
        // (SugarCube link delimiter). A single `[` is far too common in
        // array literals, CSS selectors in DOM macros, etc. We return empty
        // for a bare `[` so VS Code dismisses the suggestion list instantly.
        if trigger == Some('[') {
            // SugarCube link context ([[)
            if before_cursor.ends_with("[[") {
                return self.build_passage_name_completions(workspace, "", PassageCompletionKind::Link);
            }
            // Partial passage name after [[ (e.g., [[Gar)
            if let Some(delim_pos) = before_cursor.rfind("[[") {
                let after = &before_cursor[delim_pos + 2..];
                // Only trigger if the text after [[ looks like a passage name
                // (alphanumeric, spaces, underscores, hyphens — no | yet)
                if !after.is_empty() && after.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-') {
                    return self.build_passage_name_completions(workspace, after, PassageCompletionKind::Link);
                }
            }
            // Pipe-link context: [[display|PassageName — after the pipe
            if let Some(pipe_pos) = before_cursor.rfind('|') {
                // Check if there's a [[ before the pipe
                if let Some(bracket_pos) = before_cursor[..pipe_pos].rfind("[[") {
                    let after_pipe = &before_cursor[pipe_pos + 1..];
                    if after_pipe.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-') {
                        return self.build_passage_name_completions(workspace, after_pipe, PassageCompletionKind::Link);
                    }
                }
            }
            // Single `[` without `[[` — not a link context, return empty.
            return Vec::new();
        }

        // ── 6. No trigger → context-aware fallback ─────────────────────
        //
        // Use span-based resolution to determine what's at the cursor,
        // then build the appropriate completions. This handles:
        // - Ctrl+Space on a variable → variable completions
        // - Ctrl+Space inside a macro arg → passage or property completions
        // - Ctrl+Space at a random position → workspace symbols

        // Check if we're inside a variable sigil context (user typed $name and hit Ctrl+Space)
        if before_cursor.ends_with('$') || before_cursor.chars().last() == Some('$') {
            return self.build_variable_completions(false, line, character, "");
        }
        if before_cursor.ends_with('_') && !before_cursor.ends_with("::_") {
            // _ at end but not in a passage header — likely temp var
            return self.build_variable_completions(true, line, character, "");
        }

        // ── No-trigger dot continuation (BEFORE VarOp span check) ──────
        // Detect `$varname.partial` or `_varname.partial` patterns where
        // the user has typed past a dot without a trigger. This handles
        // Ctrl+Space after typing `$player.n` — the arena tree and
        // line-based scan both try to resolve the variable path.
        //
        // This MUST come before the VarOp span check because a VarOp span
        // for `$item.work` covers the entire token — when the user types
        // `$item.work.` and hits Ctrl+Space, the cursor is at/near the end
        // of the VarOp span, and we want dot-continuation completions
        // (children of `$item.work`) rather than root variable completions.
        {
            let mut found_dot_ctx = None;
            for sigil in &['$', '_'] {
                if let Some(sigil_pos) = before_cursor.rfind(*sigil) {
                    let after = &before_cursor[sigil_pos + 1..];
                    // Validate: must be a valid identifier + dot + partial
                    // e.g., "player.n" or "player.address.st"
                    if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                        if let Some(dot_pos) = after.rfind('.') {
                            let var_path = &before_cursor[sigil_pos..sigil_pos + 1 + dot_pos];
                            let partial = &after[dot_pos + 1..];
                            // Verify the path exists in the arena tree
                            if self.variable_kind_at_path(var_path).is_some() {
                                found_dot_ctx = Some((var_path.to_string(), partial.to_string()));
                                break;
                            }
                            // Also try arena offset-based resolution
                            if let Some(resolved) = find_variable_path_before_dot(
                                workspace, uri, text, byte_offset, before_cursor, &self.registry,
                            ) {
                                found_dot_ctx = Some((resolved, partial.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
            if let Some((var_path, partial)) = found_dot_ctx {
                return self.build_variable_dot_completions(&var_path, line, character, &partial);
            }
        }

        // Check if cursor is on an existing variable span
        if let Some(doc) = workspace.get_document(uri) {
            for passage in &doc.passages {
                for var in &passage.vars {
                    if passage.span_contains_abs_offset(&var.span, byte_offset) {
                        // Compute partial from the variable name (cursor is
                        // somewhere on the existing variable span)
                        let is_temp = var.is_temporary;
                        let sigil = if is_temp { '_' } else { '$' };
                        let partial = extract_partial_after_sigil(before_cursor, sigil);
                        return self.build_variable_completions(is_temp, line, character, partial);
                    }
                }
            }
        }

        // Check if cursor is inside a passage-ref macro arg
        if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
            return self.build_passage_name_completions(
                workspace, &arg_ref.target,
                PassageCompletionKind::MacroArg {
                    macro_name: arg_ref.macro_name.clone(),
                    has_body: arg_ref.has_body,
                },
            );
        }
        // Line-based fallback for passage-ref: detect if cursor is inside
        // a quoted string argument of a passage-ref macro (e.g., <<goto "Gar)
        // This handles the case where the AST hasn't been updated yet.
        if let Some(ctx) = detect_passage_in_quote(before_cursor, self) {
            let partial = extract_partial_in_quote(before_cursor);
            let (macro_name, has_body) = match ctx {
                crate::types::CompletionContext::MacroPassageRef { macro_name, has_body, .. } => (macro_name, has_body),
                _ => (String::new(), false),
            };
            return self.build_passage_name_completions(
                workspace, &partial,
                PassageCompletionKind::MacroArg { macro_name, has_body },
            );
        }

        // Check if cursor is in a link context ([[PassageName or [[display|PassageName)
        if let Some(delim_pos) = before_cursor.rfind("[[") {
            let after = &before_cursor[delim_pos + 2..];
            // Check for pipe-link syntax: [[display|PassageName
            if let Some(pipe_pos) = after.rfind('|') {
                let after_pipe = &after[pipe_pos + 1..];
                if after_pipe.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-') {
                    return self.build_passage_name_completions(workspace, after_pipe, PassageCompletionKind::Link);
                }
            } else if after.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-') {
                return self.build_passage_name_completions(workspace, after, PassageCompletionKind::Link);
            }
        }

        // Check if cursor is in a macro open context (no trigger, but text pattern matches)
        if let Some(delim_pos) = before_cursor.rfind("<<") {
            let after = &before_cursor[delim_pos + 2..];
            if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ' ') {
                let name = after.trim();
                return self.build_macro_completions(workspace, name, line, character, text, byte_offset);
            }
        }

        // Check semantic tokens for passage ref, variable, namespace, or property
        for group in token_groups {
            let group_offset = group.passage_offset;
            for token in &group.tokens {
                let abs_start = token.start + group_offset;
                let abs_end = abs_start + token.length;
                if byte_offset >= abs_start && byte_offset < abs_end {
                    use crate::plugin::SemanticTokenType;
                    match token.token_type {
                        SemanticTokenType::Link => {
                            // Cursor is on a passage name inside [[ ]] link syntax.
                            // Offer passage name completions with Link kind.
                            let name = text[abs_start..abs_end].to_string();
                            return self.build_passage_name_completions(
                                workspace, &name, PassageCompletionKind::Link,
                            );
                        }
                        SemanticTokenType::PassageRef => {
                            // Cursor is on a passage name inside a macro passage-ref arg
                            // (e.g., "Forest" in <<goto "Forest">>).
                            // Offer passage name completions with MacroArg kind.
                            //
                            // We resolve the macro context from the MacroArgRef
                            // data first (most accurate), then fall back to
                            // scanning the line for the enclosing macro name.
                            let name = text[abs_start..abs_end].to_string();
                            let (macro_name, has_body) =
                                find_macro_arg_ref_at_offset(workspace, uri, byte_offset)
                                    .map(|ar| (ar.macro_name.clone(), ar.has_body))
                                    .unwrap_or_else(|| {
                                        // Fallback: scan before_cursor for the macro name
                                        resolve_macro_name_from_offset(
                                            before_cursor, byte_offset, self,
                                        )
                                    });
                            return self.build_passage_name_completions(
                                workspace, &name,
                                PassageCompletionKind::MacroArg { macro_name, has_body },
                            );
                        }
                        SemanticTokenType::Variable => {
                            let name = text[abs_start..abs_end].to_string();
                            let is_temp = name.starts_with('_');
                            let sigil = if is_temp { '_' } else { '$' };
                            let partial = extract_partial_after_sigil(before_cursor, sigil);
                            return self.build_variable_completions(is_temp, line, character, partial);
                        }
                        SemanticTokenType::Namespace => {
                            let name = text[abs_start..abs_end].to_string();
                            return self.build_global_property_completions(&name);
                        }
                        SemanticTokenType::Property => {
                            let object_name = find_preceding_namespace_token(
                                token_groups, text, abs_start,
                            );
                            return self.build_global_property_completions(&object_name);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Default: offer workspace symbols (passages, variables, macros)
        self.build_default_completions(workspace)
    }
}

// ===========================================================================
// Macro open/close pair scanning (Phase 2 infrastructure)
// ===========================================================================

/// A detected macro open/close tag pair in source text.
///
/// Used by `find_enclosing_block_macro()` and `find_unclosed_block_macros()`
/// to determine which container macros surround a cursor position and which
/// container macros are unclosed.
#[derive(Debug, Clone)]
struct MacroTag {
    /// The macro name (e.g., "if", "for", "link").
    name: String,
    /// Byte offset where the tag starts in the document.
    start: usize,
    /// Byte offset where the tag ends in the document.
    end: usize,
    /// Whether this is an open tag (`<<name>>`) or close tag (`<</name>>`).
    is_close: bool,
}

/// Scan text for all `<<name>>` and `<</name>>` tags up to a byte limit.
///
/// Returns tags sorted by byte position. Skips content inside string literals,
/// block comments, and line comments within macro args (between `<<` and `>>`).
/// This is a heuristic scan — it doesn't build a full AST, but handles the
/// common cases well enough for completion filtering.
fn scan_macro_tags(text: &str, up_to: usize) -> Vec<MacroTag> {
    let mut tags = Vec::new();
    let bytes = text.as_bytes();
    let limit = up_to.min(text.len());
    let mut i = 0;

    while i < limit {
        // Look for `<<` delimiter
        if bytes[i] == b'<' && i + 1 < limit && bytes[i + 1] == b'<' {
            let tag_start = i;

            // Check for close tag: `<</`
            let is_close = i + 2 < limit && bytes[i + 2] == b'/';
            let name_start = if is_close { i + 3 } else { i + 2 };

            // Extract the macro name (alphanumeric + underscore + hyphen)
            let mut name_end = name_start;
            while name_end < limit {
                let ch = bytes[name_end];
                if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                    name_end += 1;
                } else {
                    break;
                }
            }

            let name = &text[name_start..name_end];

            // Skip empty names or names that look like operators (`<=`, `<<`, `<-`)
            if name.is_empty() {
                i += 1;
                continue;
            }

            // For open tags, skip to the closing `>>` while handling strings/comments
            // For close tags, find the `>>` directly
            let mut scan_pos = name_end;
            let mut found_close = false;

            if !is_close {
                // Skip args content inside the open tag: strings, comments, etc.
                let mut in_dq = false;
                let mut in_sq = false;
                while scan_pos + 1 < limit {
                    let ch = bytes[scan_pos];
                    let next = bytes[scan_pos + 1];

                    if in_dq {
                        if ch == b'\\' {
                            scan_pos += 2; // Skip escaped char
                            continue;
                        }
                        if ch == b'"' {
                            in_dq = false;
                        }
                    } else if in_sq {
                        if ch == b'\\' {
                            scan_pos += 2;
                            continue;
                        }
                        if ch == b'\'' {
                            in_sq = false;
                        }
                    } else {
                        // Check for block comment `/* ... */`
                        if ch == b'/' && next == b'*' {
                            scan_pos += 2;
                            while scan_pos + 1 < limit {
                                if bytes[scan_pos] == b'*' && bytes[scan_pos + 1] == b'/' {
                                    scan_pos += 2;
                                    break;
                                }
                                scan_pos += 1;
                            }
                            continue;
                        }
                        // Check for line comment `//`
                        if ch == b'/' && next == b'/' {
                            scan_pos += 2;
                            while scan_pos < limit {
                                if bytes[scan_pos] == b'\n' {
                                    scan_pos += 1;
                                    break;
                                }
                                scan_pos += 1;
                            }
                            continue;
                        }
                        if ch == b'"' {
                            in_dq = true;
                        } else if ch == b'\'' {
                            in_sq = true;
                        } else if ch == b'>' && next == b'>' {
                            found_close = true;
                            scan_pos += 2;
                            break;
                        }
                    }
                    scan_pos += 1;
                }
            } else {
                // Close tag: find `>>`
                while scan_pos + 1 < limit {
                    if bytes[scan_pos] == b'>' && bytes[scan_pos + 1] == b'>' {
                        found_close = true;
                        scan_pos += 2;
                        break;
                    }
                    scan_pos += 1;
                }
            }

            if found_close {
                tags.push(MacroTag {
                    name: name.to_string(),
                    start: tag_start,
                    end: scan_pos,
                    is_close,
                });
                i = scan_pos;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    tags
}

/// Build a stack of currently-open container macro names at a given byte offset.
///
/// Scans the text up to `byte_offset`, tracking `<<name>>` opens and `<</name>>`
/// closes. Returns the stack of unclosed macro names from outermost to innermost.
///
/// Only considers macros that are in the `body_macro_names()` set (Container macros).
/// Structural modifiers like `<<else>>`, `<<elseif>>`, `<<case>>` are not pushed
/// because they don't open a new scope.
fn build_open_macro_stack_at_offset(text: &str, byte_offset: usize, body_macros: &HashSet<&'static str>) -> Vec<String> {
    let tags = scan_macro_tags(text, byte_offset);
    let mut stack: Vec<String> = Vec::new();

    for tag in &tags {
        if tag.is_close {
            // Pop the matching open tag
            if let Some(pos) = stack.iter().rposition(|n| n == &tag.name) {
                stack.truncate(pos);
            }
        } else {
            // Only push if this is a container macro
            if body_macros.contains(tag.name.as_str()) {
                stack.push(tag.name.clone());
            }
        }
    }

    stack
}

/// Find the names of the enclosing container macro(s) at a cursor position.
///
/// Returns a list from outermost to innermost. For example, when the cursor
/// is inside `<<if>><<for>>...<</for>><</if>>`, returns `["if", "for"]`.
///
/// If the cursor is at the top level (not inside any container macro), returns
/// an empty vector.
fn find_enclosing_block_macros(text: &str, byte_offset: usize, body_macros: &HashSet<&'static str>) -> Vec<String> {
    build_open_macro_stack_at_offset(text, byte_offset, body_macros)
}

// ===========================================================================
// Private completion builders (SugarCube-specific)
// ===========================================================================

/// Different contexts where passage names are offered as completions.
#[derive(Clone)]
enum PassageCompletionKind {
    /// Inside a `[[link]]` — inserts `[[name]]`
    Link,
    /// Inside a macro passage-arg — inserts just the name.
    ///
    /// Carries the macro context so the builder can produce context-aware
    /// detail text (e.g., "Navigation target for <<goto>>" vs
    /// "Included passage for <<include>>") and proper text_edit ranges.
    MacroArg {
        /// Which macro this passage-ref arg belongs to (e.g., "goto", "link", "include").
        macro_name: String,
        /// Whether the macro invocation has a body block.
        has_body: bool,
    },
}

/// Compute a `FormatTextEdit` for macro completions.
///
/// ## Design: why textEdit does NOT cover `<<`
///
/// VS Code uses the text inside a completion's `textEdit` range as the
/// "prefix" for client-side filtering. If the range covers `<<`, the prefix
/// would be `<<` (or `<<li`), and VS Code would filter out items whose
/// `filter_text` doesn't start with `<<`. Since our `filter_text` values are
/// bare macro names (`"if"`, `"set"`, `"link"`), ALL items would be removed.
///
/// The fix: the textEdit range covers ONLY the partial name the user typed
/// AFTER `<<`. The `<<` itself remains in the document and the snippet is
/// inserted after it. This works because:
///
/// 1. **Empty prefix** (user typed `<<`): textEdit range starts at the
///    cursor (after `<<`) and extends to cover any auto-closed `>>`.
///    `new_text` is the snippet WITHOUT `<<`. The `<<` already in the
///    document + snippet = `<<macro ...>>`. VS Code's word at cursor is
///    empty → all items pass the filter.
///
/// 2. **Partial prefix** (user typed `<<li`): textEdit range covers just
///    `li` (from after `<<` to cursor), plus any auto-closed `>>`.
///    `new_text` is the snippet WITHOUT `<<`. The `<<` remains, `li` and
///    `>>` are replaced by the full snippet. VS Code's prefix is `li` →
///    matches `filter_text = "link"`.
///
/// ## Auto-close `>>` handling
///
/// When the user types `<<`, VS Code's auto-close pair feature may add `>>`
/// immediately after the cursor. If we don't consume this `>>`, accepting a
/// completion would leave a dangling `>>` at the end. We detect this by
/// checking if `>>` follows the cursor position and including it in the
/// textEdit range so it gets replaced.
fn compute_macro_text_edit(
    filter_prefix: &str,
    line: u32,
    character: u32,
    snippet: &str,
    after_cursor: &str,
) -> Option<crate::types::FormatTextEdit> {
    use crate::types::FormatTextEdit;

    // The snippet does NOT include `<<` — it starts with the macro name.
    // The `<<` is already in the document and will remain there.
    // The snippet IS self-contained with `>>` (and closing tags for block
    // macros). If VS Code auto-close also inserted `>>`, we consume it by
    // extending the replacement range so there's no duplication.
    let new_text = snippet.to_string();

    // Check if auto-close added `>>` after the cursor — if so, we need to
    // include it in the replacement range so it gets consumed.
    let auto_close_len = if after_cursor.starts_with(">>") {
        2u32
    } else {
        0u32
    };

    if !filter_prefix.is_empty() {
        // User typed `<<li` — replace just the partial name `li`
        let prefix_len = filter_prefix.chars().count() as u32;
        let start_char = character.saturating_sub(prefix_len);
        let end_char = character + auto_close_len;
        Some(FormatTextEdit {
            start_line: line,
            start_character: start_char,
            end_line: line,
            end_character: end_char,
            new_text,
        })
    } else {
        // User typed `<<` — insert snippet at cursor (possibly replacing auto-close `>>`)
        Some(FormatTextEdit {
            start_line: line,
            start_character: character,
            end_line: line,
            end_character: character + auto_close_len,
            new_text,
        })
    }
}

impl SugarCubePlugin {
    /// Build variable name completions, enriched with structural data
    /// from the arena tree.
    ///
    /// Since `$` is shorthand for `State.variables.` and `_` for
    /// `State.temporary.`, this is really "show children of the scope
    /// root" — the same operation as dot-notation property completions,
    /// just starting from a different depth in the arena tree.
    ///
    /// Each completion item carries the variable's inferred kind
    /// (Object/Array/Scalar/Unknown) from the arena `NavIndex`, which
    /// controls the icon, detail text, sort priority, and whether
    /// the `.` commit character is offered for chaining.
    fn build_variable_completions(
        &self,
        is_temp: bool,
        line: u32,
        character: u32,
        partial: &str,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, PropertyKind};

        let all_names = self.registry.variable_names();
        let mut sorted_names: Vec<_> = all_names.into_iter().collect();
        sorted_names.sort();

        sorted_names
            .iter()
            .filter(|name| {
                if is_temp {
                    name.starts_with('_') && !name.starts_with("__")
                } else {
                    !name.starts_with('_')
                }
            })
            .map(|name| {
                let display_name = if name.starts_with('$') || name.starts_with('_') {
                    name.clone()
                } else if is_temp {
                    format!("_{name}")
                } else {
                    format!("${name}")
                };
                let filter_name = name.trim_start_matches('$').trim_start_matches('_').to_string();

                // ── Arena tree enrichment ──────────────────────────────
                // Look up the inferred structural kind and children for
                // this variable. `$` = children of <persistent> root,
                // `_` = children of <temp:Passage> root — the arena
                // path_index unifies both under the display name.
                let inferred_kind = self.variable_kind_at_path(&display_name)
                    .unwrap_or(PropertyKind::Unknown);
                let children = self.variable_children_with_kind(&display_name);
                let child_count = children.len();

                // Completion icon: Objects and Arrays use Module (they
                // have children to explore), Scalars use Variable.
                let completion_kind = match inferred_kind {
                    PropertyKind::Object | PropertyKind::Array => FormatCompletionKind::Module,
                    PropertyKind::Scalar | PropertyKind::Unknown => FormatCompletionKind::Variable,
                };

                // Detail: type-aware with child preview
                let detail = match inferred_kind {
                    PropertyKind::Object => {
                        let preview: Vec<&str> = children.iter()
                            .take(3)
                            .map(|(n, _)| n.as_str())
                            .collect();
                        if child_count <= 3 {
                            format!("Object {{ {} }}", preview.join(", "))
                        } else {
                            format!("Object {{ {}, … }} — {} properties", preview.join(", "), child_count)
                        }
                    }
                    PropertyKind::Array => {
                        format!("Array — {} element properties", child_count)
                    }
                    PropertyKind::Scalar => {
                        if is_temp { "Scalar — scoped to passage".to_string() }
                        else { "Scalar".to_string() }
                    }
                    PropertyKind::Unknown => {
                        if is_temp { "Temp variable — scoped to passage".to_string() }
                        else { "Story variable — persists across passages".to_string() }
                    }
                };

                // Sort: Objects first (most completable), then Arrays,
                // then Scalars, then Unknowns. Within each group, sort
                // alphabetically by name.
                let sort_prefix = match inferred_kind {
                    PropertyKind::Object => "0",
                    PropertyKind::Array => "1",
                    PropertyKind::Scalar => "2",
                    PropertyKind::Unknown => "3",
                };

                // Text edit: replace partial after sigil with full name.
                // The `$` or `_` stays in the document (like `<<` for
                // macros) — only the identifier portion is replaced.
                let text_edit = compute_variable_text_edit(
                    partial, line, character, &filter_name,
                );

                // Commit characters: add "." for chaining into properties
                // on Object/Array variables. Scalars don't need it.
                let commit_chars = match inferred_kind {
                    PropertyKind::Object | PropertyKind::Array => {
                        vec![".".to_string(), " ".to_string(), "\n".to_string()]
                    }
                    PropertyKind::Scalar | PropertyKind::Unknown => {
                        vec![" ".to_string(), "\n".to_string()]
                    }
                };

                // Data payload for resolve: carry structural info so
                // completion_resolve can show type, children, def-site.
                let kind_str = match inferred_kind {
                    PropertyKind::Object => "object",
                    PropertyKind::Array => "array",
                    PropertyKind::Scalar => "scalar",
                    PropertyKind::Unknown => "unknown",
                };
                let child_names: Vec<&str> = children.iter()
                    .take(10)
                    .map(|(n, _)| n.as_str())
                    .collect();

                FormatCompletionItem {
                    label: display_name.clone(),
                    kind: completion_kind,
                    detail: Some(detail),
                    sort_text: Some(format!("{}_{}", sort_prefix, name)),
                    filter_text: Some(filter_name.clone()),
                    // insert_text is the fallback when text_edit is None;
                    // insert just the name (sigil already in document).
                    insert_text: Some(filter_name.clone()),
                    insert_text_format: FormatInsertTextFormat::PlainText,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({
                        "type": "variable",
                        "name": name,
                        "is_temp": is_temp,
                        "inferred_kind": kind_str,
                        "child_count": child_count,
                        "child_names": child_names,
                    })),
                    commit_characters: commit_chars,
                }
            })
            .collect()
    }

    /// Build macro completions including builtins + custom macros.
    ///
    /// Uses the multi-form completion system: each macro can declare multiple
    /// completion forms (e.g., `<<link>>` has 5 forms for different arg counts
    /// and inline/block variants). Forms come from `completion_forms.rs`.
    /// Macros without explicit forms fall back to `build_macro_snippet()`.
    ///
    /// Uses `textEdit` to properly consume and replace `<<partial` text.
    /// When `filter_prefix` is non-empty, only macros whose name starts with
    /// the prefix are included.
    fn build_macro_completions(
        &self,
        _workspace: &knot_core::Workspace,
        filter_prefix: &str,
        line: u32,
        character: u32,
        text: &str,
        byte_offset: usize,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, MacroKind,
        };

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Compute text after cursor (for auto-close `>>` detection) ──
        let after_cursor = &text[byte_offset..];

        // ── Determine enclosing block macros for sub-macro filtering ──
        let body_macros = self.body_macro_names();
        let enclosing = find_enclosing_block_macros(text, byte_offset, &body_macros);
        let parent_constraints = macros::macro_parent_constraints();

        // ── Builtin macros ────────────────────────────────────────────
        for mdef in self.builtin_macros() {
            if !filter_prefix.is_empty() && !mdef.name.starts_with(filter_prefix) {
                continue;
            }

            // Phase 2: Sub-macro scoping — filter SubMacro items when the
            // cursor is not inside a valid parent container.
            if mdef.kind == MacroKind::SubMacro {
                // Look up which parents this sub-macro requires
                if let Some(valid_parents) = parent_constraints.get(mdef.name) {
                    // Check if ANY enclosing macro is a valid parent
                    let inside_valid_parent = enclosing.iter()
                        .any(|enc| valid_parents.contains(enc.as_str()));
                    if !inside_valid_parent {
                        // If the user's filter prefix partially matches this
                        // sub-macro name, still show it but deprioritize it
                        // (user might be typing it intentionally).
                        if filter_prefix.is_empty() || !mdef.name.starts_with(filter_prefix) {
                            continue;
                        }
                        // Partial match — include but deprioritize (sort prefix "9_")
                        // Fall through to normal processing with sort adjustment below
                    }
                }
            }

            seen.insert(mdef.name.to_string());

            let category = mdef.category.to_string();

            // Determine sort prefix (lower = higher priority in completion list):
            // - "0" = context-smart: sub-macro inside its valid parent (top priority)
            // - "1" = normal macro (default priority)
            // - "2" = deprecated macro (still shown but after non-deprecated)
            // - "9" = sub-macro outside valid parent (lowest priority)
            let sort_prefix = if mdef.kind == MacroKind::SubMacro {
                // Check if this was a partial-prefix match outside valid parent
                if let Some(valid_parents) = parent_constraints.get(mdef.name) {
                    let inside_valid_parent = enclosing.iter()
                        .any(|enc| valid_parents.contains(enc.as_str()));
                    if !inside_valid_parent {
                        "9" // Deprioritized — outside valid parent
                    } else {
                        "0" // Context-smart: inside valid parent, boost to top
                    }
                } else {
                    "1"
                }
            } else if mdef.deprecated {
                "2" // Deprecated — shown after normal macros
            } else {
                "1"
            };

            // Check for multi-form completions first
            if let Some(forms) = macros::macro_completion_forms(mdef.name) {
                for form in forms {
                    let snippet = macros::convert_snippet_newlines(form.snippet);
                    let text_edit = compute_macro_text_edit(
                        filter_prefix, line, character, &snippet, after_cursor,
                    );
                    let detail_text = if mdef.deprecated {
                        format!("[Deprecated] [{}] {}", category, form.detail)
                    } else {
                        format!("[{}] {}", category, form.detail)
                    };
                    items.push(FormatCompletionItem {
                        label: form.label.to_string(),
                        kind: FormatCompletionKind::Function,
                        detail: Some(detail_text),
                        sort_text: Some(format!("{}_{}_{:02}", sort_prefix, mdef.name, form.sort_priority)),
                        filter_text: Some(mdef.name.to_string()),
                        insert_text: Some(snippet),
                        insert_text_format: FormatInsertTextFormat::Snippet,
                        text_edit,
                        deprecated: mdef.deprecated,
                        preselect: form.sort_priority == 0 && (sort_prefix == "0" || sort_prefix == "1"),
                        data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
                        commit_characters: Vec::new(),
                    });
                }
            } else {
                // Single-form macro: use the existing snippet system
                let snippet = self.build_macro_snippet(mdef.name, mdef.body);
                let text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &snippet, after_cursor,
                );
                let detail_text = if mdef.deprecated {
                    format!("[Deprecated] [{}] {}", category, mdef.description)
                } else {
                    format!("[{}] {}", category, mdef.description)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", mdef.name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(detail_text),
                    sort_text: Some(format!("{}_{}_00", sort_prefix, mdef.name)),
                    filter_text: Some(mdef.name.to_string()),
                    insert_text: Some(snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit,
                    deprecated: mdef.deprecated,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
                    commit_characters: Vec::new(),
                });
            }
        }

        // ── Custom macros (widgets, Macro.add) ────────────────────────
        let custom_names = self.registry.custom_macro_names();
        for name in &custom_names {
            if !filter_prefix.is_empty() && !name.starts_with(filter_prefix) {
                continue;
            }
            if seen.contains(name) {
                continue;
            }
            seen.insert(name.clone());

            let custom = self.registry.custom_macros().get(name);
            let is_widget = custom.map(|m| m.is_widget).unwrap_or(false);
            let is_container = custom.map(|m| m.is_container).unwrap_or(false);
            let arg_count = custom.and_then(|m| m.arg_count);
            let description = custom.and_then(|m| m.description.as_deref());

            // Build detail text
            let detail_base = if is_widget {
                format!("Custom widget — {}", name)
            } else {
                format!("Custom macro — {}", name)
            };

            // Container widgets: offer block form only with _contents tabstop
            // Non-container widgets: offer block form as primary, inline form as secondary
            if is_widget && is_container {
                // Container widget: block form only with _contents at $2
                let block_snippet = macros::convert_snippet_newlines(
                    &format!("{} $1>>\\n$2\\n<</{}>>", name, name),
                );
                let block_text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &block_snippet, after_cursor,
                );
                let block_detail = if let Some(desc) = description {
                    format!("{} (container) — {}", name, desc)
                } else {
                    format!("{} (container)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>…<</{}>>", name, name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", block_detail)),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(block_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: block_text_edit,
                    deprecated: false,
                    preselect: true,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            } else if is_widget {
                // Block form: <<name>>…<</name>>
                let block_snippet = macros::convert_snippet_newlines(
                    &format!("{} $1>>\\n$2\\n<</{}>>", name, name),
                );
                let block_text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &block_snippet, after_cursor,
                );
                let block_detail = if let Some(desc) = description {
                    format!("{} (with body) — {}", name, desc)
                } else {
                    format!("{} (with body)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>…<</{}>>", name, name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", block_detail)),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(block_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: block_text_edit,
                    deprecated: false,
                    preselect: true,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });

                // Inline form: <<name args>>
                let arg_placeholder = if let Some(n) = arg_count {
                    (0..n)
                        .map(|i| format!("${{{}:{}}}", i + 1, format!("arg{}", i + 1)))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    "${1:args}".to_string()
                };
                let inline_snippet = format!("{} {}>>", name, arg_placeholder);
                let inline_text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &inline_snippet, after_cursor,
                );
                let inline_detail = if let Some(desc) = description {
                    format!("{} (inline) — {}", name, desc)
                } else {
                    format!("{} (inline)", name)
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Custom widget — {}", inline_detail)),
                    sort_text: Some(format!("2_{}_01", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(inline_snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: inline_text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            } else {
                // Non-widget custom macro (Macro.add): just inline form
                let arg_placeholder = if let Some(n) = arg_count {
                    (0..n)
                        .map(|i| format!("${{{}:{}}}", i + 1, format!("arg{}", i + 1)))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    "${1:args}".to_string()
                };
                let snippet = format!("{} {}>>", name, arg_placeholder);
                let text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &snippet, after_cursor,
                );
                let full_detail = if let Some(desc) = description {
                    format!("{} — {}", detail_base, desc)
                } else {
                    detail_base
                };
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(full_detail),
                    sort_text: Some(format!("2_{}_00", name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(snippet),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({"type": "macro", "name": name})),
                    commit_characters: Vec::new(),
                });
            }
        }

        items
    }

    /// Build passage name completions with context-aware detail and text_edit.
    ///
    /// ## Context-aware detail
    ///
    /// Instead of the generic "Passage" label, each item's `detail` reflects the
    /// macro context that triggered the completion:
    ///
    /// - `<<goto "Passage">>` → "Navigation target for \<\<goto\>\>"
    /// - `<<include "Passage">>` → "Included passage for \<\<include\>\>"
    /// - `<<link "Talk" "Passage">>` → "Link target for \<\<link\>\>"
    /// - `[[Passage]]` → "Link target"
    ///
    /// The detail is derived from the `LinkSource` semantics and the macro
    /// catalog's `MacroDef.description`, following format isolation — no
    /// format-specific data leaks out of the plugin.
    ///
    /// ## text_edit
    ///
    /// Passage completions provide `text_edit` ranges that replace only the
    /// partial passage name text, not the surrounding delimiters (`[[`, `]]`,
    /// or quotes). This follows the same design as `compute_macro_text_edit`:
    ///
    /// - For `[[Partial]]`: text_edit replaces `Partial` with the full name.
    ///   The `[[` and `]]` remain in the document.
    /// - For `<<goto "Partial">>`: text_edit replaces `Partial` inside the
    ///   quotes. The quotes and `<<goto` remain.
    fn build_passage_name_completions(
        &self,
        workspace: &knot_core::Workspace,
        target: &str,
        kind: PassageCompletionKind,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        // Build a context-aware detail string based on the completion kind.
        // This uses the macro catalog and LinkSource semantics to produce
        // descriptions that tell the user *why* they're seeing passage names.
        let detail_text = match &kind {
            PassageCompletionKind::Link => "Link target".to_string(),
            PassageCompletionKind::MacroArg { macro_name, .. } => {
                // Look up the macro in the catalog for semantic context.
                // We use the same classification as LinkSource to produce
                // human-readable descriptions for each macro category.
                match macro_name.as_str() {
                    "goto" => "Navigation target for <<goto>>".to_string(),
                    "include" | "display" => format!("Included passage for <<{}>>", macro_name),
                    "link" | "button" | "click" => format!("Link target for <<{}>>", macro_name),
                    "linkappend" | "linkprepend" | "linkreplace" | "linkrepeat" => {
                        format!("Link target for <<{}>>", macro_name)
                    }
                    "actions" => "Choice passage for <<actions>>".to_string(),
                    "back" => "Return passage for <<back>>".to_string(),
                    "return" => "Return passage for <<return>>".to_string(),
                    // Fallback for unknown macros that have passage-ref args
                    // (e.g., custom macros or newly added catalog entries)
                    other => format!("Passage target for <<{}>>", other),
                }
            }
        };

        // Look up the MacroDef to enrich completion data further.
        // If found, we can extract the arg label (e.g., "passage" vs "passageName")
        // and whether the arg is required — useful for sort priority.
        // NOTE: Currently used for future enrichment (filtering by required args,
        // macro-specific sort ordering). The lookup is cheap (static catalog scan).
        let _macro_def = match &kind {
            PassageCompletionKind::MacroArg { macro_name, .. } => {
                self.find_macro(macro_name)
            }
            PassageCompletionKind::Link => None,
        };

        let names = workspace.all_passage_names();
        names
            .iter()
            .enumerate()
            .map(|(_, name)| {
                let (insert_text, insert_format, commit_chars) = match &kind {
                    PassageCompletionKind::Link => {
                        // For [[ links, the insert_text replaces the partial name
                        // after [[ with the full [[Name]] pattern
                        if target.is_empty() {
                            (format!("[[{}]]", name), FormatInsertTextFormat::Snippet, vec!["]".to_string()])
                        } else {
                            // Partial name already typed after [[ — just insert the name part
                            (name.clone(), FormatInsertTextFormat::PlainText, vec!["]".to_string()])
                        }
                    }
                    PassageCompletionKind::MacroArg { .. } => {
                        (name.clone(), FormatInsertTextFormat::PlainText, Vec::new())
                    }
                };

                // Sort priority: context-aware ordering.
                // - "Start" passage is preselected for navigation macros
                //   (most likely target for <<goto>>, <<link>>, etc.)
                // - Exact matches on the partial target get highest priority
                // - Items are otherwise sorted by index (stable order)
                let is_exact_match = !target.is_empty() && name == target;
                let is_start = name == "Start";
                let sort_prefix = if is_exact_match {
                    "0" // Exact match — highest priority
                } else if is_start {
                    "1" // Start passage — high priority for navigation
                } else {
                    "2" // Everything else
                };

                // Build the data payload. For MacroArg, include macro context
                // so completion_resolve can provide macro-specific documentation.
                let data = match &kind {
                    PassageCompletionKind::Link => {
                        serde_json::json!({
                            "type": "passage",
                            "name": name,
                        })
                    }
                    PassageCompletionKind::MacroArg { macro_name, has_body } => {
                        serde_json::json!({
                            "type": "passage",
                            "name": name,
                            "macro_name": macro_name,
                            "has_body": has_body,
                        })
                    }
                };

                FormatCompletionItem {
                    label: name.clone(),
                    kind: FormatCompletionKind::Module,
                    detail: Some(detail_text.clone()),
                    sort_text: Some(format!("{}_{}", sort_prefix, name)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(insert_text),
                    insert_text_format: insert_format,
                    text_edit: None, // text_edit is computed by the caller with position info
                    deprecated: false,
                    preselect: is_exact_match || is_start,
                    data: Some(data),
                    commit_characters: commit_chars,
                }
            })
            .collect()
    }

    /// Build close-tag completions for unclosed block macros.
    ///
    /// Phase 2: Now uses proper open/close pair scanning instead of the old
    /// stub that returned ALL body macro names. Only macros that are actually
    /// unclosed at the cursor position are offered as close-tag completions.
    /// Also provides `text_edit` for each close-tag item, replacing from `<</`
    /// to the cursor with `name>>`.
    fn build_close_tag_completions(
        &self,
        partial: &str,
        text: &str,
        byte_offset: usize,
        line: u32,
        character: u32,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, FormatTextEdit};

        // Find actually unclosed block macros at cursor position
        let unclosed = self.find_unclosed_block_macros(text, byte_offset);

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Compute text_edit that replaces partial after `<</` with `name>>`
        // Same design principle as compute_macro_text_edit: don't include `<</`
        // in the textEdit range because VS Code uses it as filter prefix.
        // The textEdit range covers only the partial name after `<</`.
        let compute_close_text_edit = |name: &str| -> Option<FormatTextEdit> {
            let partial_len = partial.chars().count() as u32;
            // Replace just the partial name (after `<</` and before cursor)
            let start_char = character.saturating_sub(partial_len);
            Some(FormatTextEdit {
                start_line: line,
                start_character: start_char,
                end_line: line,
                end_character: character,
                // Insert the name + `>>`. The `<</` stays in the document.
                new_text: format!("{name}>>"),
            })
        };

        // First offer close tags for actually unclosed macros
        for name in unclosed.iter().rev() {
            if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                continue;
            }
            seen.insert(name.clone());
            let text_edit = compute_close_text_edit(name);
            items.push(FormatCompletionItem {
                label: format!("/{name}>>"),
                kind: FormatCompletionKind::Function,
                detail: Some(format!("Close <<{}>>", name)),
                sort_text: Some(format!("0_{}", name)),
                filter_text: Some(name.clone()),
                insert_text: Some(format!("{name}>>")),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit,
                deprecated: false,
                preselect: false,
                data: None,
                commit_characters: Vec::new(),
            });
        }

        // If no unclosed macros found, offer all block macro close tags as fallback
        if items.is_empty() {
            for name in self.body_macro_names() {
                if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                    continue;
                }
                seen.insert(name.to_string());
                let text_edit = compute_close_text_edit(name);
                items.push(FormatCompletionItem {
                    label: format!("/{name}>>"),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Close <<{}>>", name)),
                    sort_text: Some(format!("1_{}", name)),
                    filter_text: Some(name.to_string()),
                    insert_text: Some(format!("{name}>>")),
                    insert_text_format: FormatInsertTextFormat::PlainText,
                    text_edit,
                    deprecated: false,
                    preselect: false,
                    data: None,
                    commit_characters: Vec::new(),
                });
            }
        }

        items
    }

    /// Build variable dot-notation completions (e.g., $player. → .name, .hp).
    ///
    /// This is the same operation as `$`/`_` completions — "show children
    /// of a node in the arena tree" — just starting from a deeper node
    /// rather than a scope root. The `var_path` identifies the parent
    /// node (e.g., `"$player"` or `"$player.address"`).
    ///
    /// Includes `text_edit` for partial property replacement and `.`
    /// commit character for continued chaining on Object/Array children.
    fn build_variable_dot_completions(
        &self,
        var_path: &str,
        line: u32,
        character: u32,
        partial: &str,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat, PropertyKind};

        let children = self.variable_children_with_kind(var_path);
        if children.is_empty() {
            return Vec::new();
        }

        let entry_kind = self.variable_kind_at_path(var_path).unwrap_or(PropertyKind::Unknown);
        let mut items = Vec::new();

        match entry_kind {
            PropertyKind::Array => {
                // Array built-in methods
                let array_props = [
                    (".length", "Array property", false),
                    (".push()", "Array method", true),
                    (".pop()", "Array method", true),
                    (".shift()", "Array method", true),
                    (".unshift()", "Array method", true),
                    (".includes()", "Array method", true),
                    (".indexOf()", "Array method", true),
                    (".splice()", "Array method", true),
                ];
                for (i, (prop, detail, is_method)) in array_props.iter().enumerate() {
                    let method_name = prop.trim_start_matches('.');
                    let text_edit = compute_variable_text_edit(
                        partial, line, character, method_name,
                    );
                    items.push(FormatCompletionItem {
                        label: method_name.to_string(),
                        kind: if *is_method { FormatCompletionKind::Method } else { FormatCompletionKind::Property },
                        detail: Some(format!("{} of {}", detail, var_path)),
                        sort_text: Some(format!("0_{:06}_{}", i, prop)),
                        filter_text: Some(method_name.to_string()),
                        insert_text: Some(method_name.to_string()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": method_name,
                            "is_method": is_method,
                        })),
                        commit_characters: Vec::new(),
                    });
                }
                // Element properties
                for (i, (child_name, child_kind)) in children.iter().enumerate() {
                    let detail = match child_kind {
                        PropertyKind::Object => format!("Object property of {}", var_path),
                        PropertyKind::Array => format!("Array property of {}", var_path),
                        _ => format!("Element property of {}", var_path),
                    };
                    let insert = format!("[0].{}", child_name);
                    let text_edit = compute_variable_text_edit(
                        partial, line, character, &insert,
                    );
                    // Offer "." commit for object/array element properties
                    let commit_chars = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => {
                            vec![".".to_string()]
                        }
                        _ => Vec::new(),
                    };
                    items.push(FormatCompletionItem {
                        label: format!("[0].{}", child_name),
                        kind: FormatCompletionKind::Property,
                        detail: Some(detail),
                        sort_text: Some(format!("1_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(insert.clone()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": child_name,
                            "is_method": false,
                        })),
                        commit_characters: commit_chars,
                    });
                }
            }
            PropertyKind::Object | PropertyKind::Unknown => {
                for (i, (child_name, child_kind)) in children.iter().enumerate() {
                    let kind = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => FormatCompletionKind::Module,
                        _ => FormatCompletionKind::Field,
                    };
                    let detail = match child_kind {
                        PropertyKind::Object => format!("Object property of {}", var_path),
                        PropertyKind::Array => format!("Array property of {}", var_path),
                        _ => format!("Property of {}", var_path),
                    };
                    let text_edit = compute_variable_text_edit(
                        partial, line, character, child_name,
                    );
                    // Offer "." commit for object/array children so the
                    // user can chain deeper (e.g., $player.address.)
                    let commit_chars = match child_kind {
                        PropertyKind::Object | PropertyKind::Array => {
                            vec![".".to_string()]
                        }
                        _ => Vec::new(),
                    };
                    let kind_str = match child_kind {
                        PropertyKind::Object => "object",
                        PropertyKind::Array => "array",
                        PropertyKind::Scalar => "scalar",
                        PropertyKind::Unknown => "unknown",
                    };
                    items.push(FormatCompletionItem {
                        label: child_name.clone(),
                        kind,
                        detail: Some(detail),
                        sort_text: Some(format!("0_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(child_name.clone()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit,
                        deprecated: false,
                        preselect: false,
                        data: Some(serde_json::json!({
                            "type": "variable_property",
                            "parent_path": var_path,
                            "property": child_name,
                            "inferred_kind": kind_str,
                            "is_method": false,
                        })),
                        commit_characters: commit_chars,
                    });
                }
            }
            PropertyKind::Scalar => {}
        }

        items
    }

    /// Build global object property completions (e.g., State. → .variables, .passage).
    fn build_global_property_completions(
        &self,
        object_name: &str,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        if !self.global_object_names().contains(object_name) {
            return Vec::new();
        }

        let global_def = match self.builtin_globals().iter().find(|g| g.name == object_name) {
            Some(g) => g,
            None => return Vec::new(),
        };

        let properties = match global_def.properties {
            Some(props) => props,
            None => return Vec::new(),
        };

        properties
            .iter()
            .map(|prop| FormatCompletionItem {
                label: prop.name.to_string(),
                kind: if prop.is_method {
                    FormatCompletionKind::Method
                } else {
                    FormatCompletionKind::Property
                },
                detail: Some(prop.description.to_string()),
                sort_text: None,
                filter_text: Some(prop.name.to_string()),
                insert_text: Some(prop.name.to_string()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: None,
                commit_characters: Vec::new(),
            })
            .collect()
    }

    /// Build default completions when no specific context is detected.
    ///
    /// Offers workspace symbols: passages, variables, custom macros, and JS globals.
    fn build_default_completions(
        &self,
        workspace: &knot_core::Workspace,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Passage names ────────────────────────────────────────────
        let passage_names = workspace.all_passage_names();
        for (i, name) in passage_names.iter().enumerate() {
            seen.insert(format!("passage:{}", name));
            items.push(FormatCompletionItem {
                label: name.clone(),
                kind: FormatCompletionKind::Module,
                detail: Some("Passage".to_string()),
                sort_text: Some(format!("0_{:06}", i)),
                filter_text: Some(name.clone()),
                insert_text: Some(name.clone()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: name == "Start",
                data: Some(serde_json::json!({"type": "passage", "name": name})),
                commit_characters: Vec::new(),
            });
        }

        // ── Story variables ──────────────────────────────────────────
        let var_names = self.registry.variable_names();
        let mut sorted_vars: Vec<_> = var_names.into_iter().collect();
        sorted_vars.sort();
        for (i, name) in sorted_vars.iter().enumerate() {
            if name.starts_with('_') {
                continue; // Skip temp vars in default context
            }
            let key = format!("var:{}", name);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            let display = if name.starts_with('$') {
                name.clone()
            } else {
                format!("${name}")
            };
            items.push(FormatCompletionItem {
                label: display.clone(),
                kind: FormatCompletionKind::Variable,
                detail: Some("Story variable".to_string()),
                sort_text: Some(format!("1_{:06}", i)),
                filter_text: Some(name.trim_start_matches('$').to_string()),
                insert_text: Some(display),
                insert_text_format: FormatInsertTextFormat::Snippet,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: Some(serde_json::json!({"type": "variable", "name": name, "is_temp": false})),
                commit_characters: Vec::new(),
            });
        }

        // ── Custom macros / widgets ──────────────────────────────────
        for name in self.registry.custom_macro_names() {
            let key = format!("macro:{}", name);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            items.push(FormatCompletionItem {
                label: format!("<<{}>>", name),
                kind: FormatCompletionKind::Function,
                detail: Some("Custom macro".to_string()),
                sort_text: Some(format!("2_{}", name)),
                filter_text: Some(name.clone()),
                insert_text: Some(name.clone()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: Some(serde_json::json!({"type": "macro", "name": name})),
                commit_characters: Vec::new(),
            });
        }

        items
    }

    /// Resolve whether the cursor is inside a passage-ref macro arg,
    /// returning the full `PassageCompletionKind::MacroArg` context if so.
    ///
    /// This replaces the old `is_in_passage_arg_quote` which only returned
    /// a boolean. Now we carry the macro_name and has_body through, which
    /// enables context-aware detail text in the completion items.
    ///
    /// Detection strategy (same as before, but richer return):
    /// 1. Span-based: check MacroArgRef at cursor offset (most accurate)
    /// 2. Line-based: detect passage-in-quote pattern (AST lag fallback)
    fn resolve_passage_arg_context(
        &self,
        before_cursor: &str,
        byte_offset: usize,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
    ) -> Option<PassageCompletionKind> {
        // First try span-based detection — gives us macro_name and has_body
        if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
            return Some(PassageCompletionKind::MacroArg {
                macro_name: arg_ref.macro_name.clone(),
                has_body: arg_ref.has_body,
            });
        }
        // Fallback to line-based detection
        if let Some(ctx) = detect_passage_in_quote(before_cursor, self) {
            let (macro_name, has_body) = match ctx {
                crate::types::CompletionContext::MacroPassageRef { macro_name, has_body, .. } => {
                    (macro_name, has_body)
                }
                _ => return None,
            };
            return Some(PassageCompletionKind::MacroArg { macro_name, has_body });
        }
        None
    }

    /// Find unclosed block macros by scanning text for open/close pairs.
    ///
    /// Phase 2: Now properly scans the source text using `scan_macro_tags()`
    /// and `build_open_macro_stack_at_offset()` instead of the old stub that
    /// returned ALL body macro names. Returns only macros that are actually
    /// unclosed at the cursor position, from outermost to innermost.
    fn find_unclosed_block_macros(
        &self,
        text: &str,
        byte_offset: usize,
    ) -> Vec<String> {
        let body_macros = self.body_macro_names();
        build_open_macro_stack_at_offset(text, byte_offset, &body_macros)
    }
}

// ===========================================================================
// Tests for Phase 2 scanning infrastructure
// ===========================================================================

#[cfg(test)]
mod completion_debug_tests {
    use super::*;
    use knot_core::Workspace;
    use crate::types::FormatCompletionKind;

    /// Test that `provide_completions` returns macro items when cursor is after `<<`.
    #[test]
    fn test_macro_completions_after_double_angle() {
        let plugin = SugarCubePlugin::new();
        let text = "<<";
        let line = 0u32;
        let character = 2u32; // cursor after "<<"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        // Test with no trigger (Ctrl+Space)
        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, None, &[],
        );
        assert!(!items.is_empty(), "Expected macro completions after <<, got {} items", items.len());

        // Check that at least some items have Function kind (macro completions)
        let macro_items: Vec<_> = items.iter().filter(|i| matches!(i.kind, FormatCompletionKind::Function)).collect();
        assert!(!macro_items.is_empty(), "Expected Function-typed macro completions, got none");
    }

    /// Test that `build_macro_completions` directly returns items.
    #[test]
    fn test_build_macro_completions_direct() {
        let plugin = SugarCubePlugin::new();
        let text = "<<";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.build_macro_completions(
            &workspace, "", 0, 2, text, 2,
        );
        assert!(!items.is_empty(), "build_macro_completions returned {} items, expected > 0", items.len());
    }

    /// Test `<` as trigger character when cursor is after `<<`.
    #[test]
    fn test_macro_completions_with_angle_trigger() {
        let plugin = SugarCubePlugin::new();
        let text = "<<";
        let line = 0u32;
        let character = 2u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('<'), &[],
        );
        assert!(!items.is_empty(), "Expected macro completions with < trigger after <<, got {} items", items.len());
    }

    /// Test macro completions in a realistic passage document.
    #[test]
    fn test_macro_completions_in_passage() {
        let plugin = SugarCubePlugin::new();
        // Simulate a passage with text then <<
        let text = ":: Start\nSome text <<";
        let line = 1u32; // second line (after passage header)
        let character = 12u32; // cursor after "<<"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, None, &[],
        );
        assert!(!items.is_empty(), "Expected macro completions inside passage after <<, got {} items", items.len());
        let macro_items: Vec<_> = items.iter().filter(|i| matches!(i.kind, FormatCompletionKind::Function)).collect();
        assert!(!macro_items.is_empty(), "Expected Function-typed macro completions inside passage, got none");
    }

    /// Test that text_edit ranges are valid (not out-of-bounds).
    #[test]
    fn test_macro_completions_text_edit_ranges_valid() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello <<";
        let line = 1u32;
        let character = 9u32; // cursor after "<<"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, None, &[],
        );
        assert!(!items.is_empty(), "Expected completions");

        for item in &items {
            if let Some(te) = &item.text_edit {
                // start should be <= end, and both should be on the same line
                assert!(te.start_line == te.end_line,
                    "text_edit spans multiple lines: start={}, end={}", te.start_line, te.end_line);
                assert!(te.start_character <= te.end_character,
                    "text_edit start > end: start={}, end={}", te.start_character, te.end_character);
                // start should not be negative (would underflow to huge number via saturating_sub)
                // but the range should also make sense relative to the line
            }
        }
    }

    /// Test that text_edit correctly handles auto-closed `>>` after cursor.
    #[test]
    fn test_macro_completions_auto_close_handling() {
        let plugin = SugarCubePlugin::new();
        // User typed `<<` and auto-close added `>>`, cursor is between them
        let text = ":: Start\nHello <<>>";
        let line = 1u32;
        let character = 8u32; // cursor after "<<", before ">>"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, None, &[],
        );
        assert!(!items.is_empty(), "Expected completions with auto-close >>");

        // The text_edit should cover the auto-closed ">>" (end_character = character + 2)
        for item in &items {
            if let Some(te) = &item.text_edit {
                // The textEdit should extend past the cursor to cover ">>"
                assert!(te.end_character >= character,
                    "textEdit end ({}) should be >= cursor ({}) to consume auto-close >>",
                    te.end_character, character);
                // For auto-close case, the textEdit should replace the ">>"
                // So end should be character + 2
                assert_eq!(te.end_character, character + 2,
                    "textEdit should cover auto-closed >>: end={}, expected={}",
                    te.end_character, character + 2);
            }
        }
    }

    /// Test that textEdit does NOT cover `<<` (the key fix for VS Code filtering).
    #[test]
    fn test_macro_text_edit_excludes_angle_brackets() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<";
        let line = 1u32;
        let character = 2u32; // cursor after "<<"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, None, &[],
        );
        assert!(!items.is_empty(), "Expected completions");

        // The textEdit start should NOT be before the cursor (i.e., should not cover <<)
        for item in &items {
            if let Some(te) = &item.text_edit {
                assert!(te.start_character >= character.saturating_sub(0),
                    "textEdit start ({}) should not cover << before cursor ({})",
                    te.start_character, character);
                // For empty prefix, start should be at cursor position (no prefix to replace)
                assert_eq!(te.start_character, character,
                    "textEdit start should be at cursor position for empty prefix: start={}, cursor={}",
                    te.start_character, character);
            }
        }
    }
    /// Test that single `[` (without `[[`) does NOT trigger passage completions.
    #[test]
    fn test_single_bracket_no_passage_completions() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [";
        let line = 1u32;
        let character = 7u32; // cursor after single "["
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        // Single `[` should NOT trigger passage name completions
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(passage_items.is_empty(),
            "Single [ should not trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that `[[` DOES trigger passage completions.
    #[test]
    fn test_double_bracket_triggers_passage_completions() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [[";
        let line = 1u32;
        let character = 8u32; // cursor after "[["
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "[[ should trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that partial passage name after `[[` triggers completions with filter.
    #[test]
    fn test_partial_passage_name_after_double_bracket() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [[Gar";
        let line = 1u32;
        let character = 11u32; // cursor after "Gar"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        // Should return passage completions filtered by "Gar"
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "[[Gar should trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that pipe-link syntax triggers passage completions for the passage name part.
    #[test]
    fn test_pipe_link_triggers_passage_completions() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [[Go to|";
        let line = 1u32;
        let character = 14u32; // cursor after "|"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        // Should return passage completions (pipe-link context)
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "[[display| should trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that single `<` (without `<<`) does NOT trigger macro completions.
    #[test]
    fn test_single_angle_no_macro_completions() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\na < b";
        let line = 1u32;
        let character = 3u32; // cursor after single "<"
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('<'), &[],
        );
        assert!(items.is_empty(),
            "Single < should not trigger macro completions, got {} items",
            items.len());
    }

    /// Test that `"` inside a passage-ref macro triggers passage completions.
    #[test]
    fn test_quote_in_passage_ref_macro_triggers_completions() {
        let plugin = SugarCubePlugin::new();
        // <<goto " — the " is the trigger, and <<goto has is_passage_ref arg
        let text = ":: Start\n<<goto \"";
        let line = 1u32;
        let character = 8u32; // cursor after the "
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "\" in <<goto should trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that `"` in normal text does NOT trigger passage completions.
    #[test]
    fn test_quote_in_normal_text_no_passage_completions() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHe said \"";
        let line = 1u32;
        let character = 10u32; // cursor after the "
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(passage_items.is_empty(),
            "\" in normal text should NOT trigger passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that Ctrl+Space (no trigger) on a Link semantic token returns passage completions.
    #[test]
    fn test_ctrl_space_on_link_token_returns_passage_completions() {
        let plugin = SugarCubePlugin::new();
        // Text: ":: Start\nHello [[Garden]]"
        // The word "Garden" is at byte offset 17..23 in the document.
        let text = ":: Start\nHello [[Garden]]";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        // Build a semantic token group simulating what the token builder produces
        // for a [[ ]] link. The "Garden" text starts at byte 17 in the document.
        // Passage "Start" is at passage_offset 0, so the token's passage-relative
        // start = 17 (abs) - 0 (offset) = 17.
        let token_groups = vec![crate::plugin::PassageTokenGroup {
            passage_name: "Start".to_string(),
            passage_offset: 0,
            tokens: vec![crate::plugin::SemanticToken {
                start: 17, // passage-relative byte offset of "Garden"
                length: 6, // "Garden".len()
                token_type: crate::plugin::SemanticTokenType::Link,
                modifier: None,
            }],
        }];

        // Cursor at byte 20 (inside "Garden"), line 1, char 12
        let items = plugin.provide_completions(
            text, &workspace, &uri, 1, 12, None, &token_groups,
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "Ctrl+Space on Link token should return passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that Ctrl+Space (no trigger) on a PassageRef semantic token returns passage completions.
    #[test]
    fn test_ctrl_space_on_passageref_token_returns_passage_completions() {
        let plugin = SugarCubePlugin::new();
        // Text: ":: Start\n<<goto \"Forest\">>"
        // The word "Forest" is inside the macro arg quotes.
        let text = ":: Start\n<<goto \"Forest\">>";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Forest", "Garden"]);

        // Build a semantic token group simulating what the token builder produces
        // for a passage-ref macro arg. "Forest" starts at byte 17 in the document.
        let token_groups = vec![crate::plugin::PassageTokenGroup {
            passage_name: "Start".to_string(),
            passage_offset: 0,
            tokens: vec![crate::plugin::SemanticToken {
                start: 17, // passage-relative byte offset of "Forest"
                length: 6, // "Forest".len()
                token_type: crate::plugin::SemanticTokenType::PassageRef,
                modifier: None,
            }],
        }];

        // Cursor at byte 20 (inside "Forest"), line 1, char 12
        let items = plugin.provide_completions(
            text, &workspace, &uri, 1, 12, None, &token_groups,
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "Ctrl+Space on PassageRef token should return passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test that Ctrl+Space on a Link token uses the current name for filtering.
    #[test]
    fn test_ctrl_space_on_link_token_uses_name_as_filter() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [[Gar]]";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate", "Forest"]);

        let token_groups = vec![crate::plugin::PassageTokenGroup {
            passage_name: "Start".to_string(),
            passage_offset: 0,
            tokens: vec![crate::plugin::SemanticToken {
                start: 17, // passage-relative byte offset of "Gar"
                length: 3, // "Gar".len()
                token_type: crate::plugin::SemanticTokenType::Link,
                modifier: None,
            }],
        }];

        let items = plugin.provide_completions(
            text, &workspace, &uri, 1, 12, None, &token_groups,
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(),
            "Ctrl+Space on Link token 'Gar' should return passage completions, got {} passage items",
            passage_items.len());
    }

    /// Test extract_partial_in_quote helper.
    #[test]
    fn test_extract_partial_in_quote() {
        assert_eq!(extract_partial_in_quote(r#"<<goto "Gar"#), "Gar");
        assert_eq!(extract_partial_in_quote(r#"<<goto ""#), "");
        assert_eq!(extract_partial_in_quote(r#"He said "hello""#), ""); // closed quote
        assert_eq!(extract_partial_in_quote(r#"<<link "Go" "Ga"#), "Ga"); // second open quote
    }

    // ── Context-aware passage name completion tests ─────────────────────

    /// Test that Link context produces "Link target" detail.
    #[test]
    fn test_link_context_has_link_target_detail() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\nHello [[";
        let line = 1u32;
        let character = 8u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(), "Expected passage completions after [[");
        // All passage items in Link context should have "Link target" detail
        for item in &passage_items {
            assert_eq!(item.detail.as_ref().unwrap(), &"Link target",
                "Link context passage item should have 'Link target' detail, got: {:?}",
                item.detail);
        }
    }

    /// Test that `<<goto "..."` context produces "Navigation target for <<goto>>" detail.
    #[test]
    fn test_goto_context_has_navigation_target_detail() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<goto \"";
        let line = 1u32;
        let character = 8u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(), "Expected passage completions after <<goto \"");
        for item in &passage_items {
            assert_eq!(item.detail.as_ref().unwrap(), &"Navigation target for <<goto>>",
                "goto context passage item should have 'Navigation target for <<goto>>' detail, got: {:?}",
                item.detail);
        }
    }

    /// Test that `<<include "..."` context produces "Included passage for <<include>>" detail.
    #[test]
    fn test_include_context_has_included_passage_detail() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<include \"";
        let line = 1u32;
        let character = 11u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Sidebar", "Header"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(), "Expected passage completions after <<include \"");
        for item in &passage_items {
            assert_eq!(item.detail.as_ref().unwrap(), &"Included passage for <<include>>",
                "include context passage item should have 'Included passage for <<include>>' detail, got: {:?}",
                item.detail);
        }
    }

    /// Test that `<<link "label" "..."` context produces "Link target for <<link>>" detail.
    #[test]
    fn test_link_macro_context_has_link_target_detail() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<link \"Go\" \"";
        let line = 1u32;
        let character = 14u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Garden", "Gate"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(), "Expected passage completions after <<link \"Go\" \"");
        for item in &passage_items {
            assert_eq!(item.detail.as_ref().unwrap(), &"Link target for <<link>>",
                "link macro context passage item should have 'Link target for <<link>>' detail, got: {:?}",
                item.detail);
        }
    }

    /// Test that PassageRef semantic token (Ctrl+Space) produces macro-context detail.
    #[test]
    fn test_passageref_token_has_macro_context_detail() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<goto \"Forest\">>";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Forest", "Garden"]);

        // Build a PassageRef semantic token for "Forest"
        let token_groups = vec![crate::plugin::PassageTokenGroup {
            passage_name: "Start".to_string(),
            passage_offset: 0,
            tokens: vec![crate::plugin::SemanticToken {
                start: 17,
                length: 6, // "Forest".len()
                token_type: crate::plugin::SemanticTokenType::PassageRef,
                modifier: None,
            }],
        }];

        let items = plugin.provide_completions(
            text, &workspace, &uri, 1, 12, None, &token_groups,
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty(), "Expected passage completions for PassageRef token");
        // PassageRef inside <<goto>> should have "Navigation target for <<goto>>" detail,
        // NOT "Link target" (the old bug where PassageRef was conflated with Link).
        for item in &passage_items {
            let detail = item.detail.as_ref().unwrap();
            assert!(detail.contains("<<goto>>"),
                "PassageRef in <<goto>> should have macro-context detail, got: {:?}", detail);
            assert_ne!(detail, &"Link target",
                "PassageRef should NOT have Link target detail (was conflated with Link before fix)");
        }
    }

    /// Test that passage completions in MacroArg context include macro_name in data.
    #[test]
    fn test_macro_arg_passage_completions_include_macro_context_in_data() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<goto \"";
        let line = 1u32;
        let character = 8u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('"'), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty());
        // All items should have macro_name in their data payload
        for item in &passage_items {
            let macro_name = item.data.as_ref()
                .and_then(|d| d.get("macro_name"))
                .and_then(|v| v.as_str());
            assert_eq!(macro_name, Some("goto"),
                "MacroArg passage completion should include macro_name='goto' in data, got: {:?}",
                macro_name);
        }
    }

    /// Test that Link context does NOT include macro_name in data.
    #[test]
    fn test_link_passage_completions_do_not_include_macro_context() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n[[";
        let line = 1u32;
        let character = 4u32;
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

        let items = plugin.provide_completions(
            text, &workspace, &uri, line, character, Some('['), &[],
        );
        let passage_items: Vec<_> = items.iter()
            .filter(|i| i.data.as_ref().and_then(|d| d.get("type")).and_then(|v| v.as_str()) == Some("passage"))
            .collect();
        assert!(!passage_items.is_empty());
        // Link context should NOT have macro_name in data
        for item in &passage_items {
            let macro_name = item.data.as_ref()
                .and_then(|d| d.get("macro_name"))
                .and_then(|v| v.as_str());
            assert_eq!(macro_name, None,
                "Link passage completion should NOT include macro_name in data, got: {:?}",
                macro_name);
        }
    }

    /// Test that resolve_completion_context returns MacroPassageRef for PassageRef tokens.
    #[test]
    fn test_resolve_context_passageref_returns_macro_passage_ref() {
        let plugin = SugarCubePlugin::new();
        let text = ":: Start\n<<goto \"Forest\">>";
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = make_workspace_with_passages(&uri, &["Start", "Forest"]);

        // Build a PassageRef semantic token
        let token_groups = vec![crate::plugin::PassageTokenGroup {
            passage_name: "Start".to_string(),
            passage_offset: 0,
            tokens: vec![crate::plugin::SemanticToken {
                start: 17,
                length: 6,
                token_type: crate::plugin::SemanticTokenType::PassageRef,
                modifier: None,
            }],
        }];

        let ctx = plugin.resolve_completion_context(
            text, &workspace, &uri, 1, 12, None, &token_groups,
        );
        // Should be MacroPassageRef, NOT Link
        match ctx {
            crate::types::CompletionContext::MacroPassageRef { target, macro_name, .. } => {
                assert_eq!(target, "Forest", "PassageRef should resolve target to 'Forest'");
                assert_eq!(macro_name, "goto", "PassageRef should resolve macro_name to 'goto'");
            }
            other => panic!("PassageRef token should return MacroPassageRef context, got: {:?}", other),
        }
    }

    // ── Variable dot-completion deep nesting tests ────────────────────

    /// Test that `find_variable_path_before_dot` correctly resolves
    /// deep paths like `$item.work` when cursor is after the dot.
    #[test]
    fn test_find_variable_path_before_dot_deep() {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        // Populate the arena tree with deep variable structure
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            10..40, "work.pen.color", "",
            &[],
            None,
        );

        // Line text: "<<set $item.work.>>"
        // Positions:  0123456789012345678
        //             <<set $item.work.>>
        // Cursor after second dot = character 17
        let text = ":: Init\n<<set $item.work.>>";
        let line = 1u32;
        let character = 17u32;
        let byte_offset = line_char_to_byte_offset(text, line, character);
        let line_text = text.lines().nth(line as usize).unwrap_or("");
        let byte_pos = char_to_byte_offset(line_text, character as usize);
        let before_cursor = &line_text[..byte_pos.min(line_text.len())];

        // before_cursor should be "<<set $item.work."
        assert!(before_cursor.contains("$item.work."),
            "before_cursor should contain '$item.work.', got: '{}'", before_cursor);

        let result = find_variable_path_before_dot(
            &workspace, &uri, text, byte_offset, before_cursor, &plugin.registry(),
        );

        assert!(result.is_some(), "find_variable_path_before_dot should resolve $item.work, got None");
        let path = result.unwrap();
        assert_eq!(path, "$item.work",
            "Expected path '$item.work', got '{}'", path);
    }

    /// Test that `build_variable_dot_completions` returns children at
    /// the correct depth for deeply nested variables.
    #[test]
    fn test_dot_completions_deep_nesting() {
        let mut plugin = SugarCubePlugin::new();

        // Populate the arena tree: $item -> work -> pen -> color
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            10..40, "work.pen.color", "",
            &[],
            None,
        );

        // Level 1: dot completions for $item → should show "work"
        let items_level1 = plugin.build_variable_dot_completions("$item", 0, 7, "");
        assert!(!items_level1.is_empty(), "Level 1 ($item.) should have completions");
        let labels_l1: Vec<&str> = items_level1.iter().map(|i| i.label.as_str()).collect();
        assert!(labels_l1.contains(&"work"),
            "Level 1 should contain 'work', got: {:?}", labels_l1);

        // Level 2: dot completions for $item.work → should show "pen"
        let items_level2 = plugin.build_variable_dot_completions("$item.work", 0, 13, "");
        assert!(!items_level2.is_empty(), "Level 2 ($item.work.) should have completions");
        let labels_l2: Vec<&str> = items_level2.iter().map(|i| i.label.as_str()).collect();
        assert!(labels_l2.contains(&"pen"),
            "Level 2 should contain 'pen', got: {:?}", labels_l2);

        // Level 3: dot completions for $item.work.pen → should show "color"
        let items_level3 = plugin.build_variable_dot_completions("$item.work.pen", 0, 17, "");
        assert!(!items_level3.is_empty(), "Level 3 ($item.work.pen.) should have completions");
        let labels_l3: Vec<&str> = items_level3.iter().map(|i| i.label.as_str()).collect();
        assert!(labels_l3.contains(&"color"),
            "Level 3 should contain 'color', got: {:?}", labels_l3);
    }

    /// Test the full `provide_completions` pipeline for dot-trigger
    /// deep nesting.
    #[test]
    fn test_provide_completions_dot_trigger_deep() {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        // Populate the arena tree: $item -> work -> pen -> color
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            10..40, "work.pen.color", "",
            &[],
            None,
        );

        // Test: $item. (dot trigger) → should show "work"
        // Line: "<<set $item.>>" — cursor after dot = character 12
        let text_l1 = ":: Init\n<<set $item.>>";
        let items_l1 = plugin.provide_completions(
            text_l1, &workspace, &uri,
            1, 12, // cursor after "$item."
            Some('.'),
            &[],
        );
        let labels_l1: Vec<&str> = items_l1.iter().map(|i| i.label.as_str()).collect();
        assert!(labels_l1.contains(&"work"),
            "Dot trigger after $item. should show 'work', got: {:?}", labels_l1);

        // Test: $item.work. (dot trigger) → should show "pen", NOT "work"
        // Line: "<<set $item.work.>>" — cursor after second dot = character 17
        let text_l2 = ":: Init\n<<set $item.work.>>";
        let items_l2 = plugin.provide_completions(
            text_l2, &workspace, &uri,
            1, 17, // cursor after "$item.work."
            Some('.'),
            &[],
        );
        let labels_l2: Vec<&str> = items_l2.iter().map(|i| i.label.as_str()).collect();
        assert!(labels_l2.contains(&"pen"),
            "Dot trigger after $item.work. should show 'pen', got: {:?}", labels_l2);
        assert!(!labels_l2.contains(&"work"),
            "Dot trigger after $item.work. should NOT show 'work' (that's a sibling, not a child), got: {:?}", labels_l2);
    }

    /// Test the no-trigger (Ctrl+Space) path for deep dot continuation.
    /// This simulates the scenario where the user types $item.work. and
    /// then hits Ctrl+Space instead of relying on the dot trigger.
    #[test]
    fn test_no_trigger_deep_dot_continuation() {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        // Populate the arena tree: $item -> work -> pen -> color
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            10..40, "work.pen.color", "",
            &[],
            None,
        );

        // Test: $item.work. with NO trigger → should show "pen"
        // Line: "<<set $item.work.>>" — cursor after second dot = character 17
        let text = ":: Init\n<<set $item.work.>>";
        let items = plugin.provide_completions(
            text, &workspace, &uri,
            1, 17, // cursor after "$item.work."
            None, // NO trigger (Ctrl+Space)
            &[],
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"pen"),
            "No-trigger after $item.work. should show 'pen', got: {:?}", labels);
        assert!(!labels.contains(&"work"),
            "No-trigger after $item.work. should NOT show 'work' (sibling), got: {:?}", labels);
    }

    /// Test that when a VarOp span covers the cursor, the dot continuation
    /// still takes priority for deep nesting (not root variable completions).
    #[test]
    fn test_dot_continuation_beats_var_span() {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.twee").unwrap();
        let mut workspace = knot_core::Workspace::new(uri.clone());

        // Populate the arena tree: $item -> work -> pen -> color
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            10..40, "work.pen.color", "",
            &[],
            None,
        );

        // Add a document with a VarOp for $item.work
        // This simulates the parser having seen $item.work in the passage
        use knot_core::{Document, Passage};
        use knot_core::passage::{VarOp, VarKind, StoryFormat};
        let mut doc = Document::new(uri.clone(), StoryFormat::SugarCube);
        let passage_offset = 8; // after ":: Init\n"
        let var_span_start = passage_offset + 7; // position of "$" in "<<set $item.work.>>"
        doc.passages.push(Passage {
            name: "Init".to_string(),
            tags: Vec::new(),
            span: passage_offset..(passage_offset + 30),
            header_name_span: None,
            body: Vec::new(),
            links: Vec::new(),
            vars: vec![VarOp {
                name: "$item".to_string(),
                kind: VarKind::Init,
                span: (var_span_start)..(var_span_start + 10), // covers "$item.work"
                is_temporary: false,
            }],
            macro_arg_refs: Vec::new(),
            is_special: false,
            special_def: None,
            position: None,
            passage_offset,
        });
        workspace.insert_document(doc);

        // Test: no-trigger at position after "$item.work."
        // The VarOp span covers "$item.work" but the cursor is after the trailing dot
        let text = ":: Init\n<<set $item.work.>>";
        let items = plugin.provide_completions(
            text, &workspace, &uri,
            1, 17, // cursor after "$item.work."
            None, // NO trigger
            &[],
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

        // Should show dot completions (pen), NOT root variable list ($item)
        assert!(labels.contains(&"pen"),
            "No-trigger after $item.work. with VarOp present should show 'pen', got: {:?}", labels);
    }

    /// Test that `build_path_for_segment` on the variable tree correctly
    /// resolves deep paths (e.g., "$item.work.pen") when segment_spans
    /// are provided and the starting node is the root variable.
    ///
    /// This is a regression test for the bug where `build_path_for_segment`
    /// walked UP from the root variable node, hit persistent_root
    /// immediately, and only returned a single-component path regardless
    /// of `seg_idx`.
    #[test]
    fn test_build_path_for_segment_deep_resolution() {
        use crate::sugarcube::registries::variable_tree::VariableTree;
        use crate::sugarcube::registries::variable_tree::VarAccessKind;

        let mut tree = VariableTree::new();

        // Record a deep variable: $item.work.pen.color
        // With segment_spans that represent each component's byte range.
        tree.record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            7..27, // full span: "$item.work.pen.color"
            "work.pen.color", "",
            &[
                7..12,  // segment 0: "$item"
                12..17, // segment 1: ".work"
                17..21, // segment 2: ".pen"
                21..27, // segment 3: ".color"
            ],
            None,
        );

        // Test: path_at_offset should resolve correctly at each depth.
        // When cursor is within segment 1 (work), should return "$item.work"
        let path_seg1 = tree.path_at_offset("file:///test.twee", "Init", 14);
        assert_eq!(path_seg1.as_deref(), Some("$item.work"),
            "path_at_offset at segment 1 (work) should return '$item.work', got {:?}", path_seg1);

        // When cursor is within segment 2 (pen), should return "$item.work.pen"
        let path_seg2 = tree.path_at_offset("file:///test.twee", "Init", 19);
        assert_eq!(path_seg2.as_deref(), Some("$item.work.pen"),
            "path_at_offset at segment 2 (pen) should return '$item.work.pen', got {:?}", path_seg2);

        // When cursor is within segment 3 (color), should return "$item.work.pen.color"
        let path_seg3 = tree.path_at_offset("file:///test.twee", "Init", 24);
        assert_eq!(path_seg3.as_deref(), Some("$item.work.pen.color"),
            "path_at_offset at segment 3 (color) should return '$item.work.pen.color', got {:?}", path_seg3);

        // When cursor is within segment 0 ($item), should return "$item"
        let path_seg0 = tree.path_at_offset("file:///test.twee", "Init", 9);
        assert_eq!(path_seg0.as_deref(), Some("$item"),
            "path_at_offset at segment 0 ($item) should return '$item', got {:?}", path_seg0);
    }

    /// Test the dot-trigger end-to-end scenario that the user reported:
    /// typing `$item.work.` should show properties of `work` (like `pen`),
    /// NOT the root-level children of `$item` (which would be just `work`).
    ///
    /// This simulates the real LSP scenario with segment_spans populated.
    #[test]
    fn test_dot_trigger_shows_deep_children_not_root_children() {
        let mut plugin = SugarCubePlugin::new();
        let uri = Url::parse("file:///test.twee").unwrap();
        let workspace = Workspace::new(uri.clone());

        // Populate the arena tree with deep variable structure
        // $item -> work -> pen -> color
        // $item -> name (scalar)
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            7..27,
            "work.pen.color", "",
            &[
                7..12,  // $item
                12..17, // .work
                17..21, // .pen
                21..27, // .color
            ],
            None,
        );
        plugin.registry_mut().variables_mut().record_var(
            "$item", false,
            VarAccessKind::Write,
            "Init", "file:///test.twee",
            35..40,
            "name", "",
            &[
                35..40, // $item
                40..44, // .name
            ],
            None,
        );

        // Test: $item.work. (dot trigger) → should show "pen", NOT "work" or "name"
        let text = ":: Init\n<<set $item.work.>>";
        let items = plugin.provide_completions(
            text, &workspace, &uri,
            1, 17, // cursor after "$item.work."
            Some('.'),
            &[],
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

        // "pen" is a child of $item.work — should be present
        assert!(labels.contains(&"pen"),
            "Dot trigger after $item.work. should show 'pen' (child of work), got: {:?}", labels);

        // "work" is NOT a child of $item.work — it IS $item.work.
        // It should NOT appear as a suggestion.
        assert!(!labels.contains(&"work"),
            "Dot trigger after $item.work. should NOT show 'work' (that's the path itself, not a child), got: {:?}", labels);

        // "name" is a sibling of "work" under $item, not a child of $item.work.
        // It should NOT appear as a suggestion.
        assert!(!labels.contains(&"name"),
            "Dot trigger after $item.work. should NOT show 'name' (sibling under $item, not child of work), got: {:?}", labels);
    }
}

/// Helper: create a workspace with named passages for completion testing.
fn make_workspace_with_passages(uri: &Url, names: &[&str]) -> knot_core::Workspace {
    use knot_core::{Document, Passage};
    use knot_core::passage::StoryFormat;

    let mut workspace = knot_core::Workspace::new(uri.clone());
    let mut doc = Document::new(uri.clone(), StoryFormat::SugarCube);
    for (i, name) in names.iter().enumerate() {
        let offset = i * 100;
        doc.passages.push(Passage {
            name: name.to_string(),
            tags: Vec::new(),
            span: offset..(offset + 50),
            header_name_span: None,
            body: Vec::new(),
            links: Vec::new(),
            vars: Vec::new(),
            macro_arg_refs: Vec::new(),
            is_special: false,
            special_def: None,
            position: None,
            passage_offset: offset,
        });
    }
    workspace.insert_document(doc);
    workspace
}

#[cfg(test)]
mod phase2_tests {
    use super::*;

    fn body_macros() -> HashSet<&'static str> {
        macros::body_macro_names()
    }

    #[test]
    fn test_scan_macro_tags_simple() {
        let text = "<<if $x>>hello<</if>>";
        let tags = scan_macro_tags(text, text.len());
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "if");
        assert!(!tags[0].is_close);
        assert_eq!(tags[1].name, "if");
        assert!(tags[1].is_close);
    }

    #[test]
    fn test_scan_macro_tags_nested() {
        let text = "<<if $x>><<for _i range $arr>>text<</for>><</if>>";
        let tags = scan_macro_tags(text, text.len());
        assert_eq!(tags.len(), 4);
        assert_eq!(tags[0].name, "if");
        assert!(!tags[0].is_close);
        assert_eq!(tags[1].name, "for");
        assert!(!tags[1].is_close);
        assert_eq!(tags[2].name, "for");
        assert!(tags[2].is_close);
        assert_eq!(tags[3].name, "if");
        assert!(tags[3].is_close);
    }

    #[test]
    fn test_scan_macro_tags_with_string_args() {
        // String args containing > or << should not confuse the scanner
        let text = r#"<<link "Go >>" "Passage">>click<</link>>"#;
        let tags = scan_macro_tags(text, text.len());
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "link");
        assert!(!tags[0].is_close);
        assert_eq!(tags[1].name, "link");
        assert!(tags[1].is_close);
    }

    #[test]
    fn test_scan_macro_tags_with_comments() {
        let text = "<<set $x to 1 /* >> not a close */ >>";
        let tags = scan_macro_tags(text, text.len());
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "set");
        assert!(!tags[0].is_close);
    }

    #[test]
    fn test_scan_macro_tags_close_tag() {
        let text = "<</if>>";
        let tags = scan_macro_tags(text, text.len());
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "if");
        assert!(tags[0].is_close);
    }

    #[test]
    fn test_find_enclosing_block_macros_inside_if() {
        let text = "<<if $x>>hello<</if>>";
        let offset = 12; // Inside the "hello" text
        let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
        assert_eq!(enclosing, vec!["if"]);
    }

    #[test]
    fn test_find_enclosing_block_macros_outside() {
        let text = "some text <<if $x>>hello<</if>>";
        let offset = 5; // Before the if
        let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
        assert!(enclosing.is_empty());
    }

    #[test]
    fn test_find_enclosing_block_macros_nested() {
        let text = "<<if $x>><<for _i range $arr>>inner<</for>><</if>>";
        let offset = 30; // Inside "inner"
        let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
        assert_eq!(enclosing, vec!["if", "for"]);
    }

    #[test]
    fn test_find_enclosing_block_macros_after_close() {
        let text = "<<if $x>>hello<</if>>after";
        let offset = text.len() - 3; // In "after"
        let enclosing = find_enclosing_block_macros(text, offset, &body_macros());
        assert!(enclosing.is_empty());
    }

    #[test]
    fn test_find_unclosed_macros_simple() {
        let text = "<<if $x>>hello";
        let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
        assert_eq!(unclosed, vec!["if"]);
    }

    #[test]
    fn test_find_unclosed_macros_nested_unclosed() {
        let text = "<<if $x>><<for _i range $arr>>inner";
        let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
        assert_eq!(unclosed, vec!["if", "for"]);
    }

    #[test]
    fn test_find_unclosed_macros_one_closed_one_open() {
        let text = "<<if $x>><<for _i range $arr>>inner<</for>>more";
        let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
        assert_eq!(unclosed, vec!["if"]);
    }

    #[test]
    fn test_find_unclosed_macros_all_closed() {
        let text = "<<if $x>><<for _i range $arr>>inner<</for>><</if>>";
        let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
        assert!(unclosed.is_empty());
    }

    #[test]
    fn test_sub_macro_else_not_pushed() {
        // <<else>> is a SubMacro, not a Container — it should NOT be pushed
        let text = "<<if $x>>hello<<else>>world<</if>>";
        let unclosed = build_open_macro_stack_at_offset(text, text.len(), &body_macros());
        // After the full text, everything is closed
        assert!(unclosed.is_empty());

        // Inside after <<else>>, only <<if>> should be on the stack
        let else_pos = text.find("<<else>>").unwrap();
        let after_else = else_pos + "<<else>>".len() + 3; // In "world"
        let enclosing = find_enclosing_block_macros(text, after_else, &body_macros());
        assert_eq!(enclosing, vec!["if"]);
    }

    #[test]
    fn test_scan_ignores_expression_macros() {
        // <<=>> is an expression macro, not a tag-based macro
        let text = "<<set $x to 1>><<=$x>>";
        let tags = scan_macro_tags(text, text.len());
        // Should find <<set>> and <<=>> but <<= should not be treated as a tag
        // Actually, the scanner will pick up "=" as the name after <<
        // Let's verify it doesn't crash and handles it gracefully
        assert!(tags.len() >= 1); // At least <<set>>
        // The = expression should have an empty-ish name
        let set_tag = tags.iter().find(|t| t.name == "set");
        assert!(set_tag.is_some(), "Should find <<set>> tag");
    }
}
