//! Edge classification for SugarCube passage links.
//!
//! This module contains the fallback edge classification logic used when
//! `edge_type_hint` is not set at extraction time.

use knot_core::passage::{Block, Passage};
use crate::sugarcube::parser;

/// Classify the edge type for a link from a SugarCube source passage.
///
/// This is now a **fallback** — most edges have their type set via
/// `edge_type_hint` at extraction time (see `link_source_to_edge_type`).
/// This function is only called by the graph handler when `edge_type_hint`
/// is `None` (which should be rare after the rewrite).
///
/// SugarCube-specific edge classification rules:
/// - `<<widget_name>>` → `Call` (widget invocation — checks CustomMacroRegistry)
/// - `<<include>>` → `Include` (passage inclusion)
/// - `<<goto>>` → `Jump` (unconditional navigation)
/// - `<<link>>` / `<<button>>` → `Navigation` (player choice)
/// - `[[link]]` → `Navigation` (default — player choice)
pub fn classify_edge_impl(
    source_passage: &Passage,
    _display_text: Option<&str>,
    target: &str,
) -> Option<knot_core::graph::EdgeType> {
    // Check if the target is a known custom widget (→ Call edge)
    // This handles the case where a widget invocation like <<myWidget>>
    // doesn't produce a LinkInfo at parse time but the graph needs a
    // Call edge classification.
    // NOTE: The custom_macros registry is not accessible here (static function),
    // so we check the passage body for widget-like macro invocations.
    // The primary mechanism for widget Call edges should be added to
    // the parse pipeline via the custom macro registry.

    // Check if the target is referenced by a navigation macro
    for block in &source_passage.body {
        if let Block::Macro { name, args, .. } = block {
            // Use proper string arg extraction instead of substring matching
            // to avoid false positives (e.g., args.contains("Forest") matching
            // "NotTheForest" or "ForestPath")
            let string_args = parser::extract_string_args(args);
            let string_match = string_args.iter().any(|a| a == target);

            // Also check for bare (unquoted) passage name args
            let trimmed = args.trim();
            let bare_sole_match = parser::is_bare_passage_name(trimmed) && trimmed == target;

            // Check for bare args after string args (e.g., <<link "Display" Forest>>)
            let bare_after_strings = parser::extract_bare_args_after_strings(args, string_args.len());
            let bare_after_match = bare_after_strings.iter().any(|a| a == target);

            let args_match = string_match || bare_sole_match || bare_after_match;

            match name.as_str() {
                "goto" if args_match => {
                    return Some(knot_core::graph::EdgeType::Jump);
                }
                "include" if args_match => {
                    return Some(knot_core::graph::EdgeType::Include);
                }
                "link" | "button" if args_match => {
                    return Some(knot_core::graph::EdgeType::Navigation);
                }
                "actions" if args_match => {
                    return Some(knot_core::graph::EdgeType::Navigation);
                }
                "return" | "back" if args_match => {
                    // For <<back>>/<<return>> with a single string arg, that arg
                    // is display text (NOT a passage target). Only treat it as a
                    // navigation edge if we have 2+ string args (display + target).
                    // This fallback is only called when edge_type_hint is None,
                    // which should be rare after the extraction rewrite.
                    let string_args = parser::extract_string_args(args);
                    if string_args.len() >= 2 {
                        return Some(knot_core::graph::EdgeType::Navigation);
                    }
                    // Single string arg = display text only, no navigation edge
                    return None;
                }
                _ => {}
            }
        }
    }

    // Default: no special classification (the graph engine will use Navigation)
    None
}
