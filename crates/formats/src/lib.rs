//! Knot Format Plugins
//!
//! Each story format is implemented as an isolated parser module. Format plugins
//! are responsible for parsing source text into the unified document model AND
//! providing format-specific behavioral data for IDE features.
//!
//! A plugin must:
//! - Parse passage boundaries
//! - Extract links
//! - Detect variable reads/writes
//! - Generate semantic tokens
//! - Produce format-specific diagnostics
//!
//! A plugin may also provide:
//! - Macro catalogs for completion, hover, and validation
//! - Variable sigil descriptions for hover
//! - Implicit passage reference patterns for graph building
//! - Dynamic navigation resolution for the passage graph
//! - Global object documentation for hover
//! - Operator normalization for macro expression handling
//!
//! All parsers must support incomplete and invalid syntax during live editing.
//!
//! ## Format Isolation
//!
//! **CRITICAL**: Format-specific behavior MUST live in the format directory.
//! Handlers must NEVER import format-specific data directly. They must always
//! query the active format plugin through `FormatRegistry::get()`. This ensures
//! that story formats can be hotswapped based on workspace configuration.

pub mod chapbook;
pub mod core_specials;
pub mod format_meta;
pub mod harlowe;
pub mod header;
pub mod plugin;
pub mod snowman;
pub mod sugarcube;
pub mod twine_core;
pub mod types;

pub use format_meta::{FormatMeta, InstalledFormat, parse_format_js, scan_storyformats_dir};
pub use plugin::{
    FormatDiagnostic, FormatPlugin, FormatPluginMut, MacroAtPosition, MacroBlockEvent,
    NoSourceText, ParseResult, SemanticToken, SourceTextProvider,
};
pub use types::{
    CompletionContext, GlobalDef, GlobalProperty, ImplicitPassagePattern, MacroArgDef,
    MacroArgKind, MacroCategory, MacroDef, MacroSignature, OperatorNormalization,
    PassageTempVarSummary, PassageVarRef, PropertyKind, PropertyMapEntry, ResolvedNavLink,
    VarStringMapResult, VariableSigilInfo,
};
// Workspace re-exports temporarily removed during ver_3 rewrite.
// These types will be re-introduced when the workspace module is rebuilt.

#[cfg(test)]
mod integration_tests;
