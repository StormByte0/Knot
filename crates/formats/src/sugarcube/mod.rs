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
//! ParseResult { passages, tokens, diagnostics }
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

}
