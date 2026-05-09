//! Format Plugin Trait
//!
//! Defines the interface that all format plugins must implement. The core engine
//! is format-agnostic and consumes only normalized data exposed through this trait.
//!
//! ## Architecture
//!
//! The trait has two categories of methods:
//!
//! 1. **Parsing methods** — Core parsing of source text into passages, tokens,
//!    and diagnostics. Every format must implement these.
//!
//! 2. **Behavioral methods** — Format-specific data for completion, hover,
//!    validation, dynamic navigation, and variable tracking. These have default
//!    (no-op) implementations so formats only need to override what they support.
//!
//! The behavioral methods are the Rust equivalent of the TypeScript
//! `StoryFormatAdapter` interface. They ensure format isolation: handlers
//! query the active format plugin instead of hardcoding format-specific logic.

use crate::types::*;
use knot_core::passage::{
    Passage, SpecialPassageDef, StoryFormat,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use url::Url;

/// A semantic token produced by a format plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticToken {
    /// The byte offset where the token starts.
    pub start: usize,
    /// The length of the token in bytes.
    pub length: usize,
    /// The token type (e.g., "macro", "variable", "link", "string").
    pub token_type: SemanticTokenType,
    /// Optional modifier (e.g., "deprecated", "definition").
    pub modifier: Option<SemanticTokenModifier>,
}

/// Types of semantic tokens a format plugin can produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticTokenType {
    /// A macro invocation.
    Macro,
    /// A variable reference.
    Variable,
    /// A passage link.
    Link,
    /// A string literal.
    String,
    /// A number literal.
    Number,
    /// A boolean literal.
    Boolean,
    /// A comment.
    Comment,
    /// A passage header (:: PassageName).
    PassageHeader,
    /// A tag in a passage header.
    Tag,
    /// A keyword specific to the format.
    Keyword,
}

/// Modifiers for semantic tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticTokenModifier {
    /// This token is a definition (not just a reference).
    Definition,
    /// This token is deprecated.
    Deprecated,
    /// This token is read-only.
    ReadOnly,
    /// This token is a control flow keyword.
    ControlFlow,
}

/// A diagnostic produced by a format plugin during parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatDiagnostic {
    /// The byte range of the issue.
    pub range: std::ops::Range<usize>,
    /// The diagnostic message.
    pub message: String,
    /// The severity.
    pub severity: FormatDiagnosticSeverity,
    /// The diagnostic code (for suppression).
    pub code: String,
}

/// Severity levels for format diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormatDiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// The result of parsing a document with a format plugin.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed passages.
    pub passages: Vec<Passage>,
    /// Semantic tokens for the document.
    pub tokens: Vec<SemanticToken>,
    /// Format-specific diagnostics.
    pub diagnostics: Vec<FormatDiagnostic>,
    /// Whether the parse was fully successful (no errors).
    pub is_complete: bool,
}

// ===========================================================================
// FormatPlugin trait
// ===========================================================================

/// The format plugin trait — all format parsers must implement this.
///
/// ## Parsing methods (required)
///
/// These methods handle source text parsing and must be implemented by every
/// format plugin.
///
/// ## Behavioral methods (optional, default no-ops)
///
/// These methods provide format-specific data for IDE features like completion,
/// hover, validation, and navigation. The default implementations return empty
/// collections or `None`, acting as safe no-ops for formats that don't support
/// a given feature. This is the same pattern as the TypeScript `FallbackAdapter`.
///
/// Handlers must always query these methods through the active format plugin
/// obtained from `FormatRegistry::get()`. Never import format-specific data
/// directly from a format module.
pub trait FormatPlugin: Send + Sync {
    // -----------------------------------------------------------------------
    // Parsing methods (required)
    // -----------------------------------------------------------------------

    /// Returns the story format this plugin handles.
    fn format(&self) -> StoryFormat;

    /// Parse a complete source file into passages.
    fn parse(&self, uri: &Url, text: &str) -> ParseResult;

    /// Re-parse only a single passage (for incremental updates).
    /// The `passage_text` is the body text of the passage (after the header line).
    fn parse_passage(&self, passage_name: &str, passage_text: &str) -> Option<Passage>;

    /// Returns the special passage definitions for this format.
    fn special_passages(&self) -> Vec<SpecialPassageDef>;

    /// Returns whether the given passage name is a known special passage.
    fn is_special_passage(&self, name: &str) -> bool {
        self.special_passages().iter().any(|d| d.name == name)
    }

    /// Returns the display name of this format plugin.
    fn display_name(&self) -> &str;

    // -----------------------------------------------------------------------
    // Macro catalog (optional)
    // -----------------------------------------------------------------------

    /// Returns the builtin macro definitions for this format.
    ///
    /// Used by completion, hover, validation, and signature help.
    fn builtin_macros(&self) -> &'static [MacroDef] {
        &[]
    }

    /// Returns the set of macro names that have closing tags (block macros).
    ///
    /// Used by close-tag completion and structural validation.
    fn block_macro_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that accept passage name arguments.
    ///
    /// Used by passage-in-quote completion and link extraction.
    fn passage_arg_macro_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names where the first string arg is a label
    /// and the second is a passage reference (e.g., `<<link "label" "passage">>`).
    fn label_then_passage_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that assign/write variables
    /// (e.g., `set`, `capture` in SugarCube).
    fn variable_assignment_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that define new macros
    /// (e.g., `widget` in SugarCube).
    fn macro_definition_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that contain inline scripts
    /// (e.g., `script` in SugarCube).
    fn inline_script_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of macro names that can navigate to other passages
    /// via variable arguments (e.g., `goto`, `include`, `link`, `button`).
    fn dynamic_navigation_macros(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Look up a macro definition by name.
    fn find_macro(&self, _name: &str) -> Option<&'static MacroDef> {
        None
    }

    /// Build an insertion snippet for a macro.
    fn build_macro_snippet(&self, name: &str, has_body: bool) -> String {
        if has_body {
            format!("{} $1>>\n$2\n<</{}>>", name, name)
        } else {
            format!("{} $1>>", name)
        }
    }

    /// Returns the structural parent constraints: maps child macro name →
    /// set of valid parent macro names.
    ///
    /// For example, in SugarCube: `elseif` must be inside `if` or `elseif`.
    fn macro_parent_constraints(&self) -> HashMap<&'static str, HashSet<&'static str>> {
        HashMap::new()
    }

    /// Given a macro name and the number of args provided so far, returns
    /// the 0-based index of the argument that is a passage reference.
    /// Returns -1 if no passage-ref arg at that position.
    fn get_passage_arg_index(&self, _macro_name: &str, _arg_count: usize) -> i32 {
        -1
    }

    // -----------------------------------------------------------------------
    // Special passages (extended)
    // -----------------------------------------------------------------------

    /// Returns the set of special passage names (e.g., "StoryInit", "PassageHeader").
    ///
    /// The default implementation returns an empty set. Format plugins that
    /// have special passages should override this method (and typically will,
    /// since they have `&'static str` names available at compile time).
    fn special_passage_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    /// Returns the set of system/metadata passage names
    /// (e.g., "StoryData", "StoryTitle", "Story JavaScript", "Story Stylesheet").
    fn system_passage_names(&self) -> HashSet<&'static str> {
        HashSet::new()
    }

    // -----------------------------------------------------------------------
    // Variable tracking (optional)
    // -----------------------------------------------------------------------

    /// Returns the variable sigils this format uses (e.g., `$` and `_` for SugarCube).
    fn variable_sigils(&self) -> Vec<VariableSigilInfo> {
        Vec::new()
    }

    /// Describe a variable sigil character (e.g., `$` → "SugarCube story variable").
    fn describe_variable_sigil(&self, _sigil: char) -> Option<&'static str> {
        None
    }

    /// Resolve a variable sigil character to a human-readable type name.
    fn resolve_variable_sigil(&self, _sigil: char) -> Option<&'static str> {
        None
    }

    /// Returns the assignment operators this format uses
    /// (e.g., `to`, `=` for SugarCube).
    fn assignment_operators(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Returns the comparison operators this format uses.
    fn comparison_operators(&self) -> Vec<&'static str> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Implicit passage references (optional)
    // -----------------------------------------------------------------------

    /// Returns the patterns for detecting implicit passage references in
    /// raw text/HTML/JS (e.g., `data-passage="..."`, `Engine.play("...")`).
    fn implicit_passage_patterns(&self) -> Vec<ImplicitPassagePattern> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Dynamic navigation resolution (optional)
    // -----------------------------------------------------------------------

    /// Build a map of variable name → set of known string literal values
    /// from format-specific assignment syntax.
    ///
    /// This is used to resolve dynamic passage references like
    /// `<<goto $dest>>` into concrete passage names.
    ///
    /// The default implementation returns an empty map. SugarCube overrides
    /// this to scan `<<set $var to "literal">>` patterns.
    fn build_var_string_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, Vec<String>> {
        HashMap::new()
    }

    /// Resolve dynamic navigation links from a passage using format-specific
    /// patterns and the variable string map.
    ///
    /// Returns a list of (display_text, target_passage) pairs for links that
    /// were resolved from variable references.
    fn resolve_dynamic_navigation_links(
        &self,
        _passage: &Passage,
        _var_string_map: &HashMap<String, Vec<String>>,
    ) -> Vec<ResolvedNavLink> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Hover / documentation (optional)
    // -----------------------------------------------------------------------

    /// Returns hover text for a global object name (e.g., "State", "Engine").
    fn global_hover_text(&self, _name: &str) -> Option<&'static str> {
        None
    }

    /// Returns the builtin global object definitions for this format.
    fn builtin_globals(&self) -> &'static [GlobalDef] {
        &[]
    }

    /// Returns the set of known global object names (e.g., "State", "Engine").
    fn global_object_names(&self) -> HashSet<&'static str> {
        self.builtin_globals().iter().map(|g| g.name).collect()
    }

    // -----------------------------------------------------------------------
    // Operator normalization (optional)
    // -----------------------------------------------------------------------

    /// Returns the operator normalization mappings for this format
    /// (e.g., SugarCube `to` → JS `=`, `is` → JS `===`).
    fn operator_normalization(&self) -> Vec<OperatorNormalization> {
        Vec::new()
    }

    /// Returns the operator precedence table for this format.
    fn operator_precedence(&self) -> Vec<(&'static str, u8)> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Script/stylesheet tags (optional)
    // -----------------------------------------------------------------------

    /// Returns the passage tag names that mark script passages.
    fn script_tags(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Returns the passage tag names that mark stylesheet passages.
    fn stylesheet_tags(&self) -> Vec<&'static str> {
        Vec::new()
    }

    // -----------------------------------------------------------------------
    // Macro snippet mapping (optional)
    // -----------------------------------------------------------------------

    /// Returns a per-macro snippet override for completion.
    /// If None is returned for a macro name, the default snippet is used.
    fn macro_snippet(&self, _name: &str) -> Option<&'static str> {
        None
    }

    // -----------------------------------------------------------------------
    // Dot-notation completion (optional)
    // -----------------------------------------------------------------------

    /// Build a map of variable dot-path → set of immediate child property names.
    ///
    /// Used for dot-notation completion (e.g., `$item.` → suggest "sword", "shield").
    /// The default implementation returns an empty map.
    fn build_object_property_map(&self, _workspace: &knot_core::Workspace) -> HashMap<String, HashSet<String>> {
        HashMap::new()
    }
}

// ===========================================================================
// FormatRegistry
// ===========================================================================

/// Registry of available format plugins.
pub struct FormatRegistry {
    plugins: Vec<Box<dyn FormatPlugin>>,
}

impl FormatRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a format plugin.
    pub fn register(&mut self, plugin: Box<dyn FormatPlugin>) {
        self.plugins.push(plugin);
    }

    /// Get the plugin for a given story format.
    pub fn get(&self, format: &StoryFormat) -> Option<&dyn FormatPlugin> {
        self.plugins
            .iter()
            .find(|p| &p.format() == format)
            .map(|p| p.as_ref())
    }

    /// Get all registered formats.
    pub fn formats(&self) -> Vec<StoryFormat> {
        self.plugins.iter().map(|p| p.format()).collect()
    }

    /// Create a registry with all built-in format plugins.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(crate::sugarcube::SugarCubePlugin::new()));
        registry.register(Box::new(crate::harlowe::HarlowePlugin::new()));
        registry.register(Box::new(crate::chapbook::ChapbookPlugin::new()));
        registry.register(Box::new(crate::snowman::SnowmanPlugin::new()));
        registry
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
