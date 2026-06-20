//! CSS module — type definitions + parse entry point.
//!
//! This module is the CSS equivalent of [`crate::oxc`] for JS. It exposes
//! the stable types a future CSS parser will populate, plus a [`parse_css()`]
//! entry point.
//!
//! ## Current status: unserved
//!
//! CSS parsing is **not yet implemented**. [`parse_css()`] returns an empty
//! [`CssParseOutcome`] (no tokens, no diagnostics). See [`parser`] for the
//! re-integration plan.
//!
//! The types in [`types`] are kept stable so a future CSS crate can plug
//! in without breaking downstream callers.

pub mod types;
pub mod parser;

pub use types::{CssToken, CssTokenKind, CssDiagnostic, CssParseOutcome};
pub use parser::parse_css;
