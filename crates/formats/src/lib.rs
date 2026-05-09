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
//! - Operator normalization for virtual document generation
//!
//! All parsers must support incomplete and invalid syntax during live editing.
//!
//! ## Format Isolation
//!
//! **CRITICAL**: Format-specific behavior MUST live in the format directory.
//! Handlers must NEVER import format-specific data directly. They must always
//! query the active format plugin through `FormatRegistry::get()`. This ensures
//! that story formats can be hotswapped based on workspace configuration.

pub mod plugin;
pub mod types;
pub mod sugarcube;
pub mod harlowe;
pub mod chapbook;
pub mod snowman;

pub use plugin::{FormatPlugin, ParseResult, SemanticToken, FormatDiagnostic};
pub use types::{
    MacroDef, MacroArgDef, MacroArgKind, MacroCategory, GlobalDef, GlobalProperty, MacroSignature,
    ImplicitPassagePattern, VariableSigilInfo, OperatorNormalization, VarStringMapResult,
    ResolvedNavLink,
};

#[cfg(test)]
mod integration_tests;
