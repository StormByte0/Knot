//! CSS parsing via cssparser (Mozilla's crate, used in Firefox/Servo).
//!
//! This module is the CSS equivalent of [`crate::oxc`] for JS.

pub mod types;
pub mod parser;

pub use types::{CssToken, CssTokenKind, CssDiagnostic, CssParseOutcome};
pub use parser::parse_css;
