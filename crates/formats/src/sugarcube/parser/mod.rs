//! SugarCube recursive descent parser.
//!
//! This is the heart of the rewrite. A single parser replaces ~2500 lines of
//! regex code (vars/, links/, validation/, macro_scan/, workspace/, comments/,
//! passage_tree/). The parser handles SugarCube's delimiter-based syntax
//! natively, tracking nesting depth, string contexts, and the `>>` vs `>>>`
//! ambiguity.
//!
//! ## Delimiters
//!
//! | Sequence | Token |
//! |----------|-------|
//! | `<<`     | Macro open |
//! | `>>`     | Macro close (if inside `<<`) |
//! | `[[`     | Link open |
//! | `]]`     | Link close (if inside `[[`) |
//! | `$id`    | Story variable |
//! | `_id`    | Temporary variable (word boundary) |
//! | `/%`     | Twine block comment open |
//! | `%/`     | Twine block comment close |
//! | `/%%`    | SugarCube block comment open |
//! | `%%/`    | SugarCube block comment close |
//! | `<!--`   | HTML comment open (or conditional `<!--[if...]>`) |
//! | `-->`    | HTML comment close |
//! | `/*`     | C-style block comment open (CSS/JS) |
//! | `*/`     | C-style block comment close |
//! | `//`     | JS line comment (with context heuristics) |
//! | `$$`     | Escaped dollar (not a variable) |
//!
//! ## Algorithm
//!
//! The parser scans left-to-right, recognizing delimiters by their leading
//! character. When a delimiter is found, it dispatches to a specialized
//! handler that scans to the matching close delimiter, handling nesting
//! and string escaping along the way.

mod core;
mod macro_parser;
mod link_parser;
mod comment;
mod variable_scan;
mod extraction;
mod predicates;

// Re-export public API from sub-modules
pub use comment::strip_comments;
pub use extraction::{is_bare_passage_name, extract_bare_args_after_strings, extract_string_args};

use crate::sugarcube::ast::*;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a SugarCube passage body into an AST.
///
/// `body` is the raw text between the header line and the next passage header.
/// `_body_offset` is the byte offset where `body` starts in the document.
/// The parser produces body-relative spans internally; the caller shifts
/// them by `body_offset` to get document-absolute positions.
/// This parameter is kept for API consistency with the JS validation pipeline
/// (Phase D) which needs the offset for position mapping.
///
/// Returns a `PassageAst` with the node list, extracted links, and
/// variable operations.
pub fn parse_passage_body(body: &str, _body_offset: usize, mode: ParseMode) -> PassageAst {
    match mode {
        ParseMode::Normal | ParseMode::Widget => {
            let nodes = core::parse_body(body, 0);
            let links = extraction::extract_links_from_ast(&nodes);
            let var_ops = extraction::extract_var_ops_from_ast(&nodes);
            PassageAst {
                nodes,
                links,
                var_ops,
                mode,
            }
        }
        ParseMode::Interface => {
            // StoryInterface contains HTML with data-passage attributes.
            // We parse it as normal SC text (to get <<macros>> etc.) and
            // also extract data-passage attribute values as additional links.
            let nodes = core::parse_body(body, 0);
            let mut links = extraction::extract_links_from_ast(&nodes);
            // Extract data-passage="PassageName" attributes from HTML
            let data_passage_links = extraction::extract_data_passage_refs(body);
            links.extend(data_passage_links);
            let var_ops = extraction::extract_var_ops_from_ast(&nodes);
            PassageAst {
                nodes,
                links,
                var_ops,
                mode,
            }
        }
        ParseMode::Script | ParseMode::Stylesheet | ParseMode::Minimal => {
            PassageAst::empty(mode)
        }
    }
}
