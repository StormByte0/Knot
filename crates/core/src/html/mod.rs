//! html5gum-based HTML parsing for Knot.
//!
//! This module is the HTML side of the two-parser model — the "like oxc"
//! integration (plan.md Phase 2.1 / decision D3). It is a pure parsing
//! service: it takes HTML source text and returns a **span-carrying CST**
//! ([`HtmlCst`]), tolerant by design (malformed input never fails).
//!
//! ## Design
//!
//! This module knows nothing about SugarCube, Harlowe, or any story format.
//! It is format-agnostic HTML infrastructure, like [`crate::oxc`] is for JS:
//!
//! - `knot-core` owns the parser (this module);
//! - format plugins access it on demand while parsing (the Phase 2.2
//!   htmlTag stratum carves atomic tag extents; Phase 2.3 carves raw-text
//!   zones);
//! - core's token/zoning pipelines consume the CST's byte spans for
//!   semantic tokens and theming (Phase 2.4);
//! - diagnostics and LSP handlers consult the resulting zones/CST instead
//!   of re-scanning text (Phase 2.5).
//!
//! ## Why html5gum
//!
//! WHATWG-compliant tokenizer (passes the html5lib tokenizer suite), byte
//! spans on every token, never fails on malformed input, no unsafe code.
//! It does NOT build trees — this crate layers a tolerant CST builder on
//! top ([`parser`] docs). If real tree construction is ever needed, the
//! upgrade path is html5gum's `tree-builder` feature (html5ever).
//!
//! ## Dependency flow
//!
//! `knot-formats → knot-core → html5gum`: formats depend only on
//! `knot-core` (re-exports below), never on html5gum directly.

pub mod parser;
pub mod types;

pub use parser::{parse_html_fragment, scan_leading_tag};
pub use types::{
    HtmlAttr, HtmlCst, HtmlDiagnostic, HtmlElement, HtmlElementKind, HtmlNode, HtmlNodeKind,
    HtmlRawKind, ScannedTag,
};

#[cfg(test)]
mod tests;
