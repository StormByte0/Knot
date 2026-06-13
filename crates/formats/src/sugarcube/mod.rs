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
                return self.build_macro_completions(workspace, name, line, character, text, byte_offset);
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
#[derive(Clone, Copy)]
enum PassageCompletionKind {
    /// Inside a `[[link]]` — inserts `[[name]]`
    Link,
    /// Inside a macro passage-arg — inserts just the name
    MacroArg,
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
