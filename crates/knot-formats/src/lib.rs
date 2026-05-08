//! Knot Format Plugins
//!
//! Each story format is implemented as an isolated parser module. Format plugins
//! are responsible for parsing source text into the unified document model.
//!
//! A plugin must:
//! - Parse passage boundaries
//! - Extract links
//! - Detect variable reads/writes
//! - Generate semantic tokens
//! - Produce format-specific diagnostics
//!
//! All parsers must support incomplete and invalid syntax during live editing.

pub mod plugin;
pub mod sugarcube;
pub mod harlowe;
pub mod chapbook;
pub mod snowman;

pub use plugin::{FormatPlugin, ParseResult, SemanticToken, FormatDiagnostic};

#[cfg(test)]
mod integration_tests;
