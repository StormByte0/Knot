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
    // Strategy 1: Arena tree offset-based lookup
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

    // Strategy 2: Line-based scan for variable sigil + identifier before dot
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
                    return Some(path.to_string());
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
                        SemanticTokenType::Link | SemanticTokenType::PassageRef => {
                            let target = text[abs_start..abs_end].trim().to_string();
                            if !target.is_empty() {
                                return CompletionContext::Link { target };
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
        if trigger == Some('$') || trigger == Some('_') {
            return self.build_variable_completions(trigger == Some('_'));
        }

        // ── 2. . trigger → VariableDot or Namespace property ───────────
        if trigger == Some('.') {
            // Try variable dot-notation first (e.g., $player.)
            if let Some(var_path) = find_variable_path_before_dot(
                workspace, uri, text, byte_offset, before_cursor, &self.registry,
            ) {
                return self.build_variable_dot_completions(&var_path);
            }
            // Try namespace property (e.g., State.)
            if let Some(ns_name) = find_namespace_before_dot(before_cursor, self) {
                return self.build_global_property_completions(&ns_name);
            }
            return Vec::new();
        }

        // ── 3. " trigger → Passage name in macro string arg ────────────
        if trigger == Some('"') {
            if self.is_in_passage_arg_quote(before_cursor, byte_offset, workspace, uri) {
                return self.build_passage_name_completions(workspace, "", PassageCompletionKind::MacroArg);
            }
            return Vec::new();
        }

        // ── 4. < trigger → CloseTag or MacroName ──────────────────────
        if trigger == Some('<') {
            // Close-tag context (<</)
            if before_cursor.ends_with("<</") {
                let partial = before_cursor.rfind("<</")
                    .map(|pos| before_cursor[pos + 3..].to_string())
                    .unwrap_or_default();
                return self.build_close_tag_completions(&partial, workspace);
            }
            // Macro open context (<<)
            if before_cursor.ends_with("<<") {
                return self.build_macro_completions(workspace, "", line, character);
            }
            // Partial macro name after << (e.g., <<li)
            if let Some(delim_pos) = before_cursor.rfind("<<") {
                let after = &before_cursor[delim_pos + 2..];
                if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                    return self.build_macro_completions(workspace, after, line, character);
                }
            }
            // Default: try as macro name
            return self.build_macro_completions(workspace, "", line, character);
        }

        // ── 5. [ trigger → Link (passage names) ────────────────────────
        if trigger == Some('[') {
            return self.build_passage_name_completions(workspace, "", PassageCompletionKind::Link);
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
            return self.build_variable_completions(false);
        }
        if before_cursor.ends_with('_') && !before_cursor.ends_with("::_") {
            // _ at end but not in a passage header — likely temp var
            return self.build_variable_completions(true);
        }

        // Check if cursor is on an existing variable span
        if let Some(doc) = workspace.get_document(uri) {
            for passage in &doc.passages {
                for var in &passage.vars {
                    if passage.span_contains_abs_offset(&var.span, byte_offset) {
                        return self.build_variable_completions(var.is_temporary);
                    }
                }
            }
        }

        // Check if cursor is inside a passage-ref macro arg
        if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
            return self.build_passage_name_completions(
                workspace, &arg_ref.target, PassageCompletionKind::MacroArg,
            );
        }

        // Check if cursor is in a macro open context (no trigger, but text pattern matches)
        if let Some(delim_pos) = before_cursor.rfind("<<") {
            let after = &before_cursor[delim_pos + 2..];
            if after.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ' ') {
                let name = after.trim();
                return self.build_macro_completions(workspace, name, line, character);
            }
        }

        // Check semantic tokens for namespace or property
        for group in token_groups {
            let group_offset = group.passage_offset;
            for token in &group.tokens {
                let abs_start = token.start + group_offset;
                let abs_end = abs_start + token.length;
                if byte_offset >= abs_start && byte_offset < abs_end {
                    use crate::plugin::SemanticTokenType;
                    match token.token_type {
                        SemanticTokenType::Variable => {
                            let name = text[abs_start..abs_end].to_string();
                            let is_temp = name.starts_with('_');
                            return self.build_variable_completions(is_temp);
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
// Private completion builders (SugarCube-specific)
// ===========================================================================

/// Different contexts where passage names are offered as completions.
#[derive(Clone, Copy)]
enum PassageCompletionKind {
    /// Inside a `[[link]]` — inserts `[[name]]`
    Link,
    /// Inside a macro passage-arg — inserts just the name
    MacroArg,
}

/// Compute a `FormatTextEdit` that replaces the `<<prefix` text before the cursor
/// with the full macro snippet (including `<<`).
///
/// When `filter_prefix` is non-empty, the user typed something like `<<li`
/// and we need to replace from the `<<` to the cursor. When empty, the user
/// just typed `<<` and we replace from the `<<` position.
fn compute_macro_text_edit(
    filter_prefix: &str,
    line: u32,
    character: u32,
    snippet: &str,
) -> Option<crate::types::FormatTextEdit> {
    use crate::types::FormatTextEdit;

    let new_text = format!("<<{}", snippet);

    if !filter_prefix.is_empty() {
        let prefix_len = filter_prefix.len() as u32;
        let start_char = character.saturating_sub(prefix_len + 2);
        Some(FormatTextEdit {
            start_line: line,
            start_character: start_char,
            end_line: line,
            end_character: character,
            new_text,
        })
    } else {
        Some(FormatTextEdit {
            start_line: line,
            start_character: character.saturating_sub(2),
            end_line: line,
            end_character: character,
            new_text,
        })
    }
}

impl SugarCubePlugin {
    /// Build variable completions from the format plugin's variable registry.
    ///
    /// Uses `self.registry.variable_names()` which queries the VariableTree
    /// (the most accurate source), not `workspace.documents() → passage.vars`.
    fn build_variable_completions(&self, is_temp: bool) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

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
            .enumerate()
            .map(|(i, name)| {
                let display_name = if name.starts_with('$') || name.starts_with('_') {
                    name.clone()
                } else if is_temp {
                    format!("_{name}")
                } else {
                    format!("${name}")
                };
                let filter_name = name.trim_start_matches('$').trim_start_matches('_').to_string();

                FormatCompletionItem {
                    label: display_name.clone(),
                    kind: FormatCompletionKind::Variable,
                    detail: Some(if is_temp {
                        "Temp variable — scoped to current passage".to_string()
                    } else {
                        "Story variable — persists across passages".to_string()
                    }),
                    sort_text: Some(format!("1_{:06}", i)),
                    filter_text: Some(filter_name),
                    insert_text: Some(display_name),
                    insert_text_format: FormatInsertTextFormat::Snippet,
                    text_edit: None,
                    deprecated: false,
                    preselect: false,
                    data: Some(serde_json::json!({
                        "type": "variable",
                        "name": name,
                        "is_temp": is_temp
                    })),
                    commit_characters: vec![" ".to_string(), "\n".to_string()],
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
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{
            FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat,
        };

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Builtin macros ────────────────────────────────────────────
        for mdef in self.builtin_macros() {
            if !filter_prefix.is_empty() && !mdef.name.starts_with(filter_prefix) {
                continue;
            }
            seen.insert(mdef.name.to_string());

            let category = mdef.category.to_string();

            // Check for multi-form completions first
            if let Some(forms) = macros::macro_completion_forms(mdef.name) {
                for form in forms {
                    let snippet = macros::convert_snippet_newlines(form.snippet);
                    let text_edit = compute_macro_text_edit(
                        filter_prefix, line, character, &snippet,
                    );
                    items.push(FormatCompletionItem {
                        label: form.label.to_string(),
                        kind: FormatCompletionKind::Function,
                        detail: Some(format!("[{}] {}", category, form.detail)),
                        sort_text: Some(format!("1_{}_{:02}", mdef.name, form.sort_priority)),
                        filter_text: Some(mdef.name.to_string()),
                        insert_text: Some(snippet),
                        insert_text_format: FormatInsertTextFormat::Snippet,
                        text_edit,
                        deprecated: mdef.deprecated,
                        preselect: form.sort_priority == 0,
                        data: Some(serde_json::json!({"type": "macro", "name": mdef.name})),
                        commit_characters: Vec::new(),
                    });
                }
            } else {
                // Single-form macro: use the existing snippet system
                let snippet = self.build_macro_snippet(mdef.name, mdef.body);
                let text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &snippet,
                );
                items.push(FormatCompletionItem {
                    label: format!("<<{}>>", mdef.name),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("[{}] {}", category, mdef.description)),
                    sort_text: Some(format!("1_{}_00", mdef.name)),
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
            let arg_count = custom.and_then(|m| m.arg_count);
            let description = custom.and_then(|m| m.description.as_deref());

            // Build detail text
            let detail_base = if is_widget {
                format!("Custom widget — {}", name)
            } else {
                format!("Custom macro — {}", name)
            };

            // For widgets, offer both inline and block forms
            if is_widget {
                // Block form: <<name>>…<</name>>
                let block_snippet = macros::convert_snippet_newlines(
                    &format!("{} $1>>\\n$2\\n<</{}", name, name),
                );
                let block_text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &block_snippet,
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
                let inline_snippet = format!("{} {}", name, arg_placeholder);
                let inline_text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &inline_snippet,
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
                let snippet = format!("{} {}", name, arg_placeholder);
                let text_edit = compute_macro_text_edit(
                    filter_prefix, line, character, &snippet,
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

    /// Build passage name completions.
    fn build_passage_name_completions(
        &self,
        workspace: &knot_core::Workspace,
        target: &str,
        kind: PassageCompletionKind,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        let names = workspace.all_passage_names();
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let (insert_text, insert_format, commit_chars) = match kind {
                    PassageCompletionKind::Link => {
                        (format!("[[{}]]", name), FormatInsertTextFormat::Snippet, vec!["]".to_string()])
                    }
                    PassageCompletionKind::MacroArg => {
                        (name.clone(), FormatInsertTextFormat::PlainText, Vec::new())
                    }
                };

                FormatCompletionItem {
                    label: name.clone(),
                    kind: FormatCompletionKind::Module,
                    detail: Some("Passage".to_string()),
                    sort_text: Some(format!("0_{:06}", i)),
                    filter_text: Some(name.clone()),
                    insert_text: Some(insert_text),
                    insert_text_format: insert_format,
                    text_edit: None,
                    deprecated: false,
                    preselect: !target.is_empty() && name == target || name == "Start",
                    data: Some(serde_json::json!({"type": "passage", "name": name})),
                    commit_characters: commit_chars,
                }
            })
            .collect()
    }

    /// Build close-tag completions for unclosed block macros.
    fn build_close_tag_completions(
        &self,
        partial: &str,
        workspace: &knot_core::Workspace,
    ) -> Vec<crate::types::FormatCompletionItem> {
        use crate::types::{FormatCompletionItem, FormatCompletionKind, FormatInsertTextFormat};

        // Try to find unclosed block macros in the current file
        let unclosed = self.find_unclosed_block_macros(workspace);

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // First offer close tags for actually unclosed macros
        for name in unclosed.iter().rev() {
            if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                continue;
            }
            seen.insert(name.clone());
            items.push(FormatCompletionItem {
                label: format!("/{name}>>"),
                kind: FormatCompletionKind::Function,
                detail: Some(format!("Close <<{}>>", name)),
                sort_text: Some(format!("0_{}", name)),
                filter_text: Some(name.clone()),
                insert_text: Some(name.clone()),
                insert_text_format: FormatInsertTextFormat::PlainText,
                text_edit: None,
                deprecated: false,
                preselect: false,
                data: None,
                commit_characters: Vec::new(),
            });
        }

        // If no unclosed macros found, offer all block macro close tags
        if items.is_empty() {
            for name in self.body_macro_names() {
                if seen.contains(name) || (!partial.is_empty() && !name.starts_with(partial)) {
                    continue;
                }
                seen.insert(name.to_string());
                items.push(FormatCompletionItem {
                    label: format!("/{name}>>"),
                    kind: FormatCompletionKind::Function,
                    detail: Some(format!("Close <<{}>>", name)),
                    sort_text: Some(format!("1_{}", name)),
                    filter_text: Some(name.to_string()),
                    insert_text: Some(name.to_string()),
                    insert_text_format: FormatInsertTextFormat::PlainText,
                    text_edit: None,
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
    fn build_variable_dot_completions(
        &self,
        var_path: &str,
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
                    items.push(FormatCompletionItem {
                        label: method_name.to_string(),
                        kind: if *is_method { FormatCompletionKind::Method } else { FormatCompletionKind::Property },
                        detail: Some(format!("{} of {}", detail, var_path)),
                        sort_text: Some(format!("0_{:06}_{}", i, prop)),
                        filter_text: Some(method_name.to_string()),
                        insert_text: Some(method_name.to_string()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit: None,
                        deprecated: false,
                        preselect: false,
                        data: None,
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
                    items.push(FormatCompletionItem {
                        label: format!("[0].{}", child_name),
                        kind: FormatCompletionKind::Property,
                        detail: Some(detail),
                        sort_text: Some(format!("1_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(format!("[0].{}", child_name)),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit: None,
                        deprecated: false,
                        preselect: false,
                        data: None,
                        commit_characters: Vec::new(),
                    });
                }
            }
            PropertyKind::Object | PropertyKind::Unknown => {
                for (i, (child_name, child_kind)) in children.iter().enumerate() {
                    let kind = match child_kind {
                        PropertyKind::Object => FormatCompletionKind::Module,
                        PropertyKind::Array => FormatCompletionKind::Module,
                        _ => FormatCompletionKind::Field,
                    };
                    let detail = match child_kind {
                        PropertyKind::Object => format!("Object property of {}", var_path),
                        PropertyKind::Array => format!("Array property of {}", var_path),
                        _ => format!("Property of {}", var_path),
                    };
                    items.push(FormatCompletionItem {
                        label: child_name.clone(),
                        kind,
                        detail: Some(detail),
                        sort_text: Some(format!("0_{:06}_{}", i, child_name)),
                        filter_text: Some(child_name.clone()),
                        insert_text: Some(child_name.clone()),
                        insert_text_format: FormatInsertTextFormat::PlainText,
                        text_edit: None,
                        deprecated: false,
                        preselect: false,
                        data: None,
                        commit_characters: Vec::new(),
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

    /// Check if the cursor is inside a passage-ref macro's string argument.
    fn is_in_passage_arg_quote(
        &self,
        before_cursor: &str,
        byte_offset: usize,
        workspace: &knot_core::Workspace,
        uri: &url::Url,
    ) -> bool {
        // First try span-based detection
        if find_macro_arg_ref_at_offset(workspace, uri, byte_offset).is_some() {
            return true;
        }
        // Fallback to line-based detection
        detect_passage_in_quote(before_cursor, self).is_some()
    }

    /// Find unclosed block macros by scanning workspace passage data.
    fn find_unclosed_block_macros(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> Vec<String> {
        // For now, just return the block macro names.
        // A proper implementation would scan the document for open/close pairs.
        self.body_macro_names().into_iter().map(|s| s.to_string()).collect()
    }
}
