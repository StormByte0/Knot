//! CSS module — type definitions + parse entry point.
//!
//! This module is the CSS equivalent of [`crate::oxc`] for JS: a shared
//! parsing service formats access on demand. [`parse_css()`] parses with
//! `oxc-css-parser` (plan.md Phase 6 — the raffia-based parser maintained
//! by the oxc-project org, the same ecosystem as our JS parser), producing
//! a span-carrying AST internally and mapping it to the stable token
//! stream + relayed diagnostics (severity-carrying — the user-facing
//! policy is "show breaking code when written", never hide).
//!
//! The types in [`types`] are kept stable and are the isolation boundary
//! for the pinned crate (`=0.0.15`); downstream callers
//! (`sugarcube::css::analyze_css`, the token builder, the parse pipeline)
//! never see crate types. Future handlers (go-to-definition on custom
//! properties, variable references) extend the stable outcome rather than
//! leaking the crate's AST.
//!
//! Two entry points:
//! - [`parse_css()`] — stylesheet-shaped regions (`[stylesheet]` passages,
//!   `<style>` bodies, `<<style>>`/`<<css>>` blocks). Tolerant: statement-
//!   level recovery, and a hard failure falls back to the scanner in
//!   [`fallback`] so highlighting never goes dark.
//! - [`fallback::parse_css_declarations()`] — the `style="…"` attribute
//!   microsyntax (a declaration LIST, not a stylesheet; browsers drop
//!   invalid declarations silently, so no diagnostics for that shape).

pub mod fallback;
pub mod parser;
pub mod types;

pub use fallback::parse_css_declarations;
pub use parser::parse_css;
pub use types::{CssDiagnostic, CssDiagnosticSeverity, CssParseOutcome, CssToken, CssTokenKind};
