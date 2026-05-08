//! Format Plugin Trait
//!
//! Defines the interface that all format plugins must implement. The core engine
//! is format-agnostic and consumes only normalized data exposed through this trait.

use knot_core::passage::{
    Passage, SpecialPassageDef, StoryFormat,
};
use serde::{Deserialize, Serialize};
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

/// The format plugin trait — all format parsers must implement this.
pub trait FormatPlugin: Send + Sync {
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
}

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
