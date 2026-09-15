//! CSS module — type definitions + parse entry point.
//!
//! This module is the CSS equivalent of [`crate::oxc`] for JS: a shared
//! parsing service formats access on demand. [`parse_css()`] runs a
//! hand-rolled, tolerant, highlighter-grade tokenizer (plan.md Phase 4.2) —
//! it never panics and classifies the token families themes need.
//!
//! The types in [`types`] are kept stable; downstream callers
//! (`sugarcube::css::analyze_css`, the token builder, the parse pipeline)
//! map them to semantic tokens for `[stylesheet]` passages, `<style>`
//! bodies, `<<style>>`/`<<css>>` blocks and `style="…"` attribute values.

pub mod parser;
pub mod types;

pub use parser::{parse_css, parse_css_declarations};
pub use types::{CssDiagnostic, CssParseOutcome, CssToken, CssTokenKind};
