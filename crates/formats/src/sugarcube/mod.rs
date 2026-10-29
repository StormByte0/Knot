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
pub mod css;
pub mod graph;
pub mod js;
pub mod lsp;

// Expanded registry module (now includes variable_tree, custom_macros, etc.)
pub mod registries;

// Root-level modules (unchanged)
pub mod ast;
pub mod classifier;
pub mod formatter;
pub mod lexer;
pub mod macros;
pub mod parse_pipeline;
pub mod parser;
pub mod special_passages;

// Re-exports for backward compatibility
// These ensure that `super::passage_build` etc. from parser/ and macros/ still resolve
pub use graph::edge_classify;
pub use graph::nav_resolve;
pub use graph::passage_build;
pub use js::js_preprocess;
pub use js::js_validate;
pub use js::js_walk;
pub use lsp::syntax_detect;
pub use lsp::token_builder;
pub use registries::custom_macros;
pub use registries::registry_populate;
pub use registries::var_extract;
pub use registries::variable_tree;
pub use registries::variable_tree::VarAccessKind;

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

/// A zero-based cursor position (line, character) in a document, used by
/// completion handlers to build LSP `textEdit` ranges.
#[derive(Clone, Copy, Debug)]
struct CursorPos {
    line: u32,
    character: u32,
}

// ===========================================================================
// Module split (Fix 5b-2, plan §4.6) — pure move of the former 7,168-line
// sugarcube/mod.rs monolith. The four helper sections moved to sibling
// files; the plugin struct, its inherent impl, and the two trait impls
// stay here. Test modules moved to flat sibling files so their module
// paths (and any snapshot names) are unchanged.
// ===========================================================================

mod completion;
mod completion_zone;
mod cursor_utils;
mod macro_tags;

// Bring the moved helpers back into this module's namespace — the trait
// impls below reference them by their original (unqualified) names.
use completion::*;
use completion_zone::*;
use cursor_utils::*;
use macro_tags::*;

/// SugarCube 2.x format plugin.
///
/// All runtime-populated registries are owned by the unified
/// [`SugarCubeRegistry`] hub, which implements [`FormatRegistry`] for
/// format-agnostic access. LSP handlers query registries through
/// `FormatPlugin` trait methods, never touching the hub directly.
pub struct SugarCubePlugin {
    /// The unified registry hub — owns all sub-registries.
    registry: SugarCubeRegistry,
    /// The story's pinned format version (from StoryData `format-version`),
    /// updated by the server via [`FormatPlugin::set_story_version`].
    /// `None` until StoryData is indexed — the effective version then
    /// falls back to [`macros::SUGARCUBE_LATEST`] (fail-open: no version
    /// must never mean false diagnostics).
    story_version: std::sync::RwLock<Option<crate::types::FormatVersion>>,
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
            story_version: std::sync::RwLock::new(None),
        }
    }

    /// The story's effective format version: the pinned StoryData
    /// `format-version` if known, else the catalog's latest (fail-open).
    pub fn effective_story_version(&self) -> crate::types::FormatVersion {
        self.story_version
            .read()
            .ok()
            .and_then(|v| *v)
            .unwrap_or(macros::SUGARCUBE_LATEST)
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

    fn parse_script_file_mut(&mut self, uri: &Url, text: &str) -> Option<ParseResult> {
        Some(parse_pipeline::parse_script_file(self, uri, text))
    }

    fn parse_passage_mut(
        &mut self,
        passage_name: &str,
        passage_tags: &[String],
        passage_text: &str,
        file_uri: &str,
        passage_offset: usize,
    ) -> Option<ParseResult> {
        parse_pipeline::parse_single(
            self,
            passage_name,
            passage_tags,
            passage_text,
            file_uri,
            passage_offset,
        )
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

    fn set_story_version(&self, version: Option<crate::types::FormatVersion>) {
        if let Ok(mut v) = self.story_version.write() {
            *v = version;
        }
    }

    fn story_version(&self) -> Option<crate::types::FormatVersion> {
        self.story_version.read().ok().and_then(|v| *v)
    }

    fn zone_analyze(&self, body: &str) -> knot_core::zoning::ZoneMap {
        // Phase 4.1 ZoneAnalyzer: the SugarCube strata producers
        // (prose/markup, htmlTag + raw-text script/style from Phases 1–3,
        // macro bodies) orchestrated into ONE ZoneMap — the per-byte
        // language authority. Uses the LIVE custom macro registry so widget
        // invocations classify with their registered kinds.
        let ast = parser::parse_passage_body(body, 0, ParseMode::Normal);
        crate::zoning::build_from_ast(&ast.nodes, 0, self.registry.custom_macros())
    }

    fn special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::name_matched_special_passages()
            .into_iter()
            .filter(|d| {
                special_passages::exists_at_version(&d.name, self.effective_story_version())
            })
            .collect()
    }

    fn tag_matched_special_passages(&self) -> Vec<SpecialPassageDef> {
        special_passages::tag_matched_special_passages()
            .into_iter()
            .filter(|d| {
                special_passages::exists_at_version(&d.name, self.effective_story_version())
            })
            .collect()
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

    fn describe_operator(&self, op: &str) -> Option<&'static str> {
        macros::describe_operator(op)
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

    fn build_var_string_map(
        &self,
        workspace: &knot_core::Workspace,
    ) -> HashMap<String, Vec<String>> {
        nav_resolve::build_var_string_map_impl(workspace, self.registry.variables())
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
        self.registry
            .variables()
            .path_at_offset(file_uri, passage_name, body_offset)
    }

    /// Get the children of a variable path with their inferred kinds.
    ///
    /// Delegates to the arena tree's `children_with_kind()` which queries
    /// the tree directly without building a full property map.
    ///
    /// **Note:** for temporary variable paths (`_x.y`), this returns an
    /// empty Vec because `path_index` keys for temps are namespaced by
    /// passage. Use [`variable_children_with_kind_for_passage`] in
    /// completion contexts where the enclosing passage is known.
    ///
    /// [`variable_children_with_kind_for_passage`]: FormatPlugin::variable_children_with_kind_for_passage
    fn variable_children_with_kind(&self, path: &str) -> Vec<(String, crate::types::PropertyKind)> {
        self.registry.variables().children_with_kind(path)
    }

    fn variable_kind_at_path(&self, path: &str) -> Option<crate::types::PropertyKind> {
        self.registry.variables().kind_at_path(path)
    }

    /// SugarCube override: resolves temporary variable paths against
    /// the declaring passage. Persistent paths ignore `passage_name`.
    fn variable_kind_at_path_for_passage(
        &self,
        path: &str,
        passage_name: Option<&str>,
    ) -> Option<crate::types::PropertyKind> {
        self.registry
            .variables()
            .kind_at_path_for_passage(path, passage_name)
    }

    /// SugarCube override: resolves temporary variable paths against
    /// the declaring passage. Persistent paths ignore `passage_name`.
    fn variable_children_with_kind_for_passage(
        &self,
        path: &str,
        passage_name: Option<&str>,
    ) -> Vec<(String, crate::types::PropertyKind)> {
        self.registry
            .variables()
            .children_with_kind_for_passage(path, passage_name)
    }

    /// Get all custom macro names for completion.
    fn custom_macro_names(&self) -> Vec<String> {
        self.registry.custom_macro_names()
    }

    /// Look up a custom macro definition for hover/go-to-def.
    fn find_custom_macro(&self, name: &str) -> Option<(String, String, usize)> {
        self.registry.custom_macros().get(name).map(|m| {
            (
                m.defined_in.clone(),
                m.file_uri.clone(),
                m.defined_at_offset,
            )
        })
    }

    /// Look up a custom macro with full detail for completion resolve.
    fn find_custom_macro_detail(&self, name: &str) -> Option<crate::plugin::CustomMacroDetail> {
        self.registry
            .custom_macros()
            .get(name)
            .map(|m| crate::plugin::CustomMacroDetail {
                defined_in: m.defined_in.clone(),
                file_uri: m.file_uri.clone(),
                is_widget: m.is_widget,
                is_container: m.is_container(),
                arg_count: m.arg_count,
                description: m.description.clone(),
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
            self.registry.variables(),
            workspace,
            source_text,
            passage_name,
            &passage_positions,
        )
    }

    fn extract_passage_temp_variables(
        &self,
        workspace: &knot_core::Workspace,
        source_text: &dyn crate::plugin::SourceTextProvider,
        passage_name: &str,
    ) -> Vec<crate::types::PassageTempVarSummary> {
        // Compute passage positions for relative→absolute line conversion.
        // Reuses the same map as the persistent-variable path so line
        // numbers are consistent between the two sections.
        let passage_positions = self.registry.compute_passage_positions(source_text);
        var_extract::extract_passage_temp_variables_impl(
            self.registry.variables(),
            workspace,
            source_text,
            passage_name,
            &passage_positions,
        )
    }

    fn build_object_property_map(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> HashMap<String, HashSet<String>> {
        self.registry.variables().property_map()
    }

    fn build_shape_aware_property_map(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> HashMap<String, crate::types::PropertyMapEntry> {
        var_extract::build_shape_aware_property_map_impl(self.registry.variables())
    }

    fn build_state_variable_registry(
        &self,
        _workspace: &knot_core::Workspace,
    ) -> HashMap<String, crate::types::StateVariable> {
        // For state variable registry, passage positions are not available
        // without source text. Use empty map — spans will stay passage-relative.
        // This is acceptable because the state variable registry is primarily
        // used for variable availability analysis, not for precise location reporting.
        let passage_positions =
            crate::sugarcube::registries::variable_tree::PassagePositionMap::new();
        var_extract::build_state_variable_registry_impl(
            self.registry.variables(),
            &passage_positions,
        )
    }

    // ── Function registry ─────────────────────────────────────────────

    fn function_names(&self) -> Vec<String> {
        self.registry.function_names()
    }

    fn find_function(&self, name: &str) -> Option<crate::types::FunctionDefInfo> {
        self.registry
            .functions()
            .get(name)
            .map(|f| crate::types::FunctionDefInfo {
                name: f.name.clone(),
                defined_in: f.defined_in.clone(),
                file_uri: f.file_uri.clone(),
                defined_at_offset: f.defined_at_offset,
                param_count: f.param_count,
            })
    }

    fn describe_builtin_method(&self, name: &str) -> Option<&'static str> {
        // Check SugarCube extension methods first (.pushUnique, .toUpperFirst, etc.)
        if let Some(desc) = macros::describe_builtin_method(name) {
            return Some(desc);
        }
        // Also check builtin standalone functions (random, either, visited, etc.)
        // — these can appear as Function tokens too.
        if let Some(desc) = macros::describe_builtin_function(name) {
            return Some(desc);
        }
        None
    }

    // ── Template registry ─────────────────────────────────────────────

    fn template_names(&self) -> Vec<String> {
        self.registry.template_completion_names()
    }

    fn find_template(&self, name: &str) -> Option<crate::types::TemplateDefInfo> {
        self.registry
            .templates()
            .get(name)
            .map(|t| crate::types::TemplateDefInfo {
                name: t.name.clone(),
                defined_in: t.defined_in.clone(),
                file_uri: t.file_uri.clone(),
                defined_at_offset: t.defined_at_offset,
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
                return CompletionContext::Variable {
                    name,
                    is_temporary: is_temp,
                };
            }

            // ── < → MacroName or CloseTag ───────────────────────────────
            Some('<') => {
                // Check for close-tag context first (<</)
                if before_cursor.ends_with("<</") {
                    let partial = before_cursor
                        .rfind("<</")
                        .map(|pos| before_cursor[pos + 3..].to_string())
                        .unwrap_or_default();
                    return CompletionContext::CloseTag { partial };
                }
                // Check for << (macro opening)
                if before_cursor.ends_with("<<") {
                    return CompletionContext::MacroName {
                        name: String::new(),
                    };
                }
                // Partial macro name after << (e.g., <<li)
                if let Some(delim_pos) = before_cursor.rfind("<<") {
                    let after = &before_cursor[delim_pos + 2..];
                    if after
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                    {
                        return CompletionContext::MacroName {
                            name: after.to_string(),
                        };
                    }
                }
                // Default: try span-based context, fall back to macro name
                return CompletionContext::MacroName {
                    name: String::new(),
                };
            }

            // ── [ → Link ────────────────────────────────────────────────
            Some('[') => {
                return CompletionContext::Link {
                    target: String::new(),
                };
            }

            // ── ? → Template (SugarCube template invocation in prose) ───
            Some('?') => {
                let q_pos = before_cursor.rfind('?').unwrap();
                // Suppress if `?` is preceded by a word char (JS optional
                // chaining `obj?.prop` or identifier-then-ternary).
                if q_pos > 0 {
                    let prev_byte = before_cursor.as_bytes()[q_pos - 1];
                    if prev_byte.is_ascii_alphanumeric() || prev_byte == b'_' {
                        return CompletionContext::Other;
                    }
                }
                let after_q = &before_cursor[q_pos + 1..];
                // Suppress if `?` is followed by a non-identifier char (JS
                // ternary `cond ? "x" : "y"`). Empty is OK (just typed `?`).
                if !after_q.is_empty()
                    && !after_q
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                {
                    return CompletionContext::Other;
                }
                let name = after_q.to_string();
                return CompletionContext::Template { name };
            }

            // ── . → VariableDot or Namespace/Property ───────────────────
            Some('.') => {
                // Try variable dot-notation first (e.g., $player.)
                if let Some(var_path) = find_variable_path_before_dot(
                    workspace,
                    uri,
                    text,
                    byte_offset,
                    before_cursor,
                    &self.registry,
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
                            let object_name =
                                find_preceding_namespace_token(token_groups, text, abs_start);
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
                                                before_cursor,
                                                byte_offset,
                                                self,
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

        // ── 0b. HTML-stratum guard (plan.md Phase 2.5) ─────────────
        //
        // The zone map, not raw-text scanning, decides what the cursor is
        // inside. In tag interiors and raw-text bodies the markup completion
        // contexts (macros, close tags, links, passage names, templates) are
        // inert bytes — only SugarCube variable sigils stay live, and only
        // where SugarCube actually evaluates variables: `<script>` bodies
        // (SugarCube syntax: $var → State.variables) and directive
        // attribute values (upstream `Scripting.evalTwineScript`).
        match self.completion_zone_at(text, byte_offset) {
            CompletionZone::Prose => {}
            CompletionZone::RawScript
            | CompletionZone::TagInterior {
                position: AttrPosition::DirectiveValue,
            } => {
                if trigger == Some('$') || trigger == Some('_') {
                    let is_temp = trigger == Some('_');
                    let sigil = if is_temp { '_' } else { '$' };
                    let partial = extract_partial_after_sigil(before_cursor, sigil);
                    let passage_name = find_enclosing_passage_name(text, line);
                    return self.build_variable_completions(
                        is_temp,
                        line,
                        character,
                        partial,
                        passage_name.as_deref(),
                    );
                }
                return Vec::new();
            }
            CompletionZone::TagInterior {
                position: AttrPosition::PlainValue | AttrPosition::TagName,
            } => return Vec::new(),
            CompletionZone::TagInterior {
                position: AttrPosition::AttrName,
            } => {
                // Attribute-NAME position — the HTML attribute completions
                // (plain names + the `@` / `sc-eval:` directive families,
                // version-gated: the directives exist only from 2.21.0).
                return lsp::html_completions::build_html_attr_completions(
                    before_cursor,
                    self.effective_story_version(),
                );
            }
            CompletionZone::RawStyle => return Vec::new(),
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
            // Resolve the enclosing passage so SugarCube `_` temps from
            // other passages don't bleed into this completion list.
            let passage_name = find_enclosing_passage_name(text, line);
            return self.build_variable_completions(
                is_temp,
                line,
                character,
                partial,
                passage_name.as_deref(),
            );
        }

        // ── 2. . trigger → VariableDot or Namespace property ───────────
        if trigger == Some('.') {
            // Try variable dot-notation first (e.g., $player.)
            if let Some(var_path) = find_variable_path_before_dot(
                workspace,
                uri,
                text,
                byte_offset,
                before_cursor,
                &self.registry,
            ) {
                // No partial after the dot — the `.` trigger fires right
                // after the dot is typed, so there's nothing to replace.
                // Resolve enclosing passage so temp-var dot-completion
                // (`_x.`) is scoped to the current passage.
                let passage_name = find_enclosing_passage_name(text, line);
                return self.build_variable_dot_completions(
                    &var_path,
                    line,
                    character,
                    "",
                    passage_name.as_deref(),
                );
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
            if let Some(macro_ctx) =
                self.resolve_passage_arg_context(before_cursor, byte_offset, workspace, uri)
            {
                // Extract partial passage name typed after the opening quote
                // e.g., <<goto "Gar → partial = "Gar"
                let partial = extract_partial_in_quote(before_cursor);
                return self.build_passage_name_completions(workspace, &partial, macro_ctx);
            }
            return Vec::new();
        }

        // ── 3b. ? trigger → Template completions ──────────────────────
        //
        // SugarCube templates are invoked with `?name` in prose. When the
        // user types `?`, offer all known template names. When they type
        // `?par`, filter to templates starting with "par".
        //
        // SUPPRESSION: `?` is also a JS operator (ternary `cond ? a : b`
        // and optional chaining `obj?.prop`). To avoid bleeding template
        // completions into JS expression contexts, we replicate the
        // SugarCube grammar's `(?<!\w)\?[A-Za-z_]` rule:
        //   - If `?` is preceded by a word char (`[A-Za-z0-9_]`), suppress
        //     (likely JS optional chaining or identifier-then-ternary).
        //   - If `?` is followed by a non-identifier char (and not end of
        //     input), suppress (likely JS ternary like `cond ? "x" : "y"`).
        if trigger == Some('?') {
            let q_pos = before_cursor.rfind('?').unwrap();
            // Check the char before `?` (must NOT be a word char).
            if q_pos > 0 {
                let prev_byte = before_cursor.as_bytes()[q_pos - 1];
                if prev_byte.is_ascii_alphanumeric() || prev_byte == b'_' {
                    return Vec::new();
                }
            }
            let after_q = &before_cursor[q_pos + 1..];
            // If there's text after `?`, it must look like an identifier
            // (alphanumeric/underscore/hyphen). Empty (just typed `?`) is
            // also valid — the user is about to type a template name.
            if !after_q.is_empty()
                && !after_q
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Vec::new();
            }
            let partial = after_q.to_string();
            return self.build_template_completions(line, character, &partial);
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
                let partial = before_cursor
                    .rfind("<</")
                    .map(|pos| before_cursor[pos + 3..].to_string())
                    .unwrap_or_default();
                return self.build_close_tag_completions(
                    &partial,
                    text,
                    byte_offset,
                    line,
                    character,
                );
            }
            // Macro open context (<<)
            if before_cursor.ends_with("<<") {
                return self.build_macro_completions(
                    workspace,
                    uri,
                    "",
                    CursorPos { line, character },
                    text,
                    byte_offset,
                );
            }
            // Partial macro name after << (e.g., <<li)
            if let Some(delim_pos) = before_cursor.rfind("<<") {
                let after = &before_cursor[delim_pos + 2..];
                if after
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                {
                    return self.build_macro_completions(
                        workspace,
                        uri,
                        after,
                        CursorPos { line, character },
                        text,
                        byte_offset,
                    );
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
                return self.build_passage_name_completions(
                    workspace,
                    "",
                    PassageCompletionKind::Link,
                );
            }
            // Partial passage name after [[ (e.g., [[Gar)
            if let Some(delim_pos) = before_cursor.rfind("[[") {
                let after = &before_cursor[delim_pos + 2..];
                // Only trigger if the text after [[ looks like a passage name
                // (alphanumeric, spaces, underscores, hyphens — no | yet)
                if !after.is_empty()
                    && after
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-')
                {
                    return self.build_passage_name_completions(
                        workspace,
                        after,
                        PassageCompletionKind::Link,
                    );
                }
            }
            // Pipe-link context: [[display|PassageName — after the pipe
            if let Some(pipe_pos) = before_cursor.rfind('|') {
                // Check if there's a [[ before the pipe
                if before_cursor[..pipe_pos].rfind("[[").is_some() {
                    let after_pipe = &before_cursor[pipe_pos + 1..];
                    if after_pipe
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-')
                    {
                        return self.build_passage_name_completions(
                            workspace,
                            after_pipe,
                            PassageCompletionKind::Link,
                        );
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

        // Resolve the enclosing passage once for both branches below —
        // SugarCube `_` temps are passage-scoped, so the completion list
        // must be filtered to the declaring passage. `$` vars are global
        // and unaffected by this resolution.
        let enclosing_passage = find_enclosing_passage_name(text, line);

        // Check if we're inside a variable sigil context (user typed $name and hit Ctrl+Space)
        if before_cursor.ends_with('$') || before_cursor.ends_with('$') {
            return self.build_variable_completions(
                false,
                line,
                character,
                "",
                enclosing_passage.as_deref(),
            );
        }
        if before_cursor.ends_with('_') && !before_cursor.ends_with("::_") {
            // _ at end but not in a passage header — likely temp var
            return self.build_variable_completions(
                true,
                line,
                character,
                "",
                enclosing_passage.as_deref(),
            );
        }

        // ── No-trigger partial-identifier completion ───────────────────
        // Detect `$ident` or `_ident` (sigil + identifier chars, no
        // whitespace or other punctuation between sigil and cursor) when
        // the user has typed a partial variable name and hit Ctrl+Space.
        // This is the prose / macro-arg / any-variable-eligible-context
        // path — variables are not restricted to parsed VarOp spans the
        // way passage names are restricted to link/macro-arg spans.
        //
        // Examples:
        //   `<<set _star|`        → partial="star", is_temp=true
        //   `You have _startT|`   → partial="startT", is_temp=true  (prose)
        //   `<<print $play|`      → partial="play", is_temp=false
        //   `State.variables.$pl|`→ partial="pl", is_temp=false
        //
        // The check verifies the partial extends to the cursor (no
        // whitespace between sigil and cursor), so `<<set _x to |` (cursor
        // after space) does NOT trigger — the partial would be "x" but
        // the cursor is past whitespace, so we skip.
        for (sigil, is_temp) in &[('$', false), ('_', true)] {
            if let Some(sigil_pos) = before_cursor.rfind(*sigil) {
                // Skip passage headers — `:: _passage` is not a temp var.
                if *sigil == '_' && before_cursor[..sigil_pos].ends_with("::") {
                    continue;
                }
                let after_sigil = &before_cursor[sigil_pos + 1..];
                // Partial = contiguous identifier chars after sigil.
                let end = after_sigil
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(after_sigil.len());
                let partial = &after_sigil[..end];
                // Partial must be non-empty AND extend to the cursor —
                // otherwise we'd fire on `<<set _x |` (cursor past space)
                // or `<<set _|` (cursor right after sigil, handled above).
                if !partial.is_empty() && end == after_sigil.len() {
                    return self.build_variable_completions(
                        *is_temp,
                        line,
                        character,
                        partial,
                        enclosing_passage.as_deref(),
                    );
                }
            }
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
                    if after
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                        && let Some(dot_pos) = after.rfind('.')
                    {
                        let var_path = &before_cursor[sigil_pos..sigil_pos + 1 + dot_pos];
                        let partial = &after[dot_pos + 1..];
                        // Verify the path exists in the arena tree.
                        // Use the passage-aware lookup so temp paths
                        // (`_x.y`) resolve against the enclosing passage.
                        if self
                            .variable_kind_at_path_for_passage(
                                var_path,
                                enclosing_passage.as_deref(),
                            )
                            .is_some()
                        {
                            found_dot_ctx = Some((var_path.to_string(), partial.to_string()));
                            break;
                        }
                        // Also try arena offset-based resolution
                        if let Some(resolved) = find_variable_path_before_dot(
                            workspace,
                            uri,
                            text,
                            byte_offset,
                            before_cursor,
                            &self.registry,
                        ) {
                            found_dot_ctx = Some((resolved, partial.to_string()));
                            break;
                        }
                    }
                }
            }
            if let Some((var_path, partial)) = found_dot_ctx {
                // `enclosing_passage` is in scope from the top of the
                // no-trigger branch — temp-var dot-continuation uses
                // it to scope `_x.y` resolution.
                return self.build_variable_dot_completions(
                    &var_path,
                    line,
                    character,
                    &partial,
                    enclosing_passage.as_deref(),
                );
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
                        // We're inside `passage` (the loop variable) — use
                        // its name directly as the scope, no text scan needed.
                        return self.build_variable_completions(
                            is_temp,
                            line,
                            character,
                            partial,
                            Some(&passage.name),
                        );
                    }
                }
            }
        }

        // Check if cursor is inside a passage-ref macro arg
        if let Some(arg_ref) = find_macro_arg_ref_at_offset(workspace, uri, byte_offset) {
            return self.build_passage_name_completions(
                workspace,
                &arg_ref.target,
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
                crate::types::CompletionContext::MacroPassageRef {
                    macro_name,
                    has_body,
                    ..
                } => (macro_name, has_body),
                _ => (String::new(), false),
            };
            return self.build_passage_name_completions(
                workspace,
                &partial,
                PassageCompletionKind::MacroArg {
                    macro_name,
                    has_body,
                },
            );
        }

        // Check if cursor is in a link context ([[PassageName or [[display|PassageName)
        if let Some(delim_pos) = before_cursor.rfind("[[") {
            let after = &before_cursor[delim_pos + 2..];
            // Check for pipe-link syntax: [[display|PassageName
            if let Some(pipe_pos) = after.rfind('|') {
                let after_pipe = &after[pipe_pos + 1..];
                if after_pipe
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-')
                {
                    return self.build_passage_name_completions(
                        workspace,
                        after_pipe,
                        PassageCompletionKind::Link,
                    );
                }
            } else if after
                .chars()
                .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-')
            {
                return self.build_passage_name_completions(
                    workspace,
                    after,
                    PassageCompletionKind::Link,
                );
            }
        }

        // Check if cursor is in a macro open context (no trigger, but text pattern matches)
        if let Some(delim_pos) = before_cursor.rfind("<<") {
            let after = &before_cursor[delim_pos + 2..];
            if after
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ' ')
            {
                let name = after.trim();
                return self.build_macro_completions(
                    workspace,
                    uri,
                    name,
                    CursorPos { line, character },
                    text,
                    byte_offset,
                );
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
                                workspace,
                                &name,
                                PassageCompletionKind::Link,
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
                                            before_cursor,
                                            byte_offset,
                                            self,
                                        )
                                    });
                            return self.build_passage_name_completions(
                                workspace,
                                &name,
                                PassageCompletionKind::MacroArg {
                                    macro_name,
                                    has_body,
                                },
                            );
                        }
                        SemanticTokenType::Variable => {
                            let name = text[abs_start..abs_end].to_string();
                            let is_temp = name.starts_with('_');
                            let sigil = if is_temp { '_' } else { '$' };
                            let partial = extract_partial_after_sigil(before_cursor, sigil);
                            // Resolve enclosing passage for scope-correct
                            // temp-var completion.
                            let passage_name = find_enclosing_passage_name(text, line);
                            return self.build_variable_completions(
                                is_temp,
                                line,
                                character,
                                partial,
                                passage_name.as_deref(),
                            );
                        }
                        SemanticTokenType::Namespace => {
                            let name = text[abs_start..abs_end].to_string();
                            return self.build_global_property_completions(&name);
                        }
                        SemanticTokenType::Property => {
                            let object_name =
                                find_preceding_namespace_token(token_groups, text, abs_start);
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

    /// SugarCube zone-aware passage formatter (Phase 10).
    ///
    /// Delegates to [`formatter::format_passage`]. Returns `None` if the
    /// formatter refuses (Error leaves present) — the caller falls back to
    /// the default whitespace-only formatter in that case.
    fn format_passage(
        &self,
        body_text: &str,
        zones: &knot_core::zoning::ZoneMap,
    ) -> Option<String> {
        formatter::format_passage(body_text, zones)
    }
}

/// Helper: create a workspace with named passages for completion testing.
#[cfg(test)]
fn make_workspace_with_passages(uri: &Url, names: &[&str]) -> knot_core::Workspace {
    use knot_core::passage::StoryFormat;
    use knot_core::{Document, Passage};

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
            macro_invocations: Vec::new(),
            is_special: false,
            special_def: None,
            position: None,
            group: None,
            color: None,
            size: None,
            line: 0,
            manual_reachable: None,
            passage_offset: offset,
            zones: knot_core::zoning::ZoneMap::default(),
        });
    }
    workspace.insert_document(doc);
    workspace
}

// Tests for Phase 2 scanning infrastructure (section banner preserved
// from the monolith's tests region).
#[cfg(test)]
mod completion_debug_tests;
#[cfg(test)]
mod html_zone_guard_tests;
#[cfg(test)]
mod phase2_tests;
#[cfg(test)]
mod phase4_zone_completion_tests;
#[cfg(test)]
mod storyinterface_incremental_tests;
#[cfg(test)]
mod temp_var_scope_tests;
#[cfg(test)]
mod zone_analyzer_tests;
