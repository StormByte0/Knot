//! Property tests (plan.md Phase 5.1): arbitrary passage-like strings must
//! never panic the parser or the zone engine, zone invariants must hold
//! unconditionally, and per-byte language queries stay total.
//!
//! The string strategy draws from an alphabet weighted toward the markup
//! sigils that historically caused trouble (`''`, `@`, `<<`, `[[`, `;`,
//! quotes, braces) — the exact byte shapes from the Task 3/4 findings.

use crate::plugin::FormatRegistry;
use crate::sugarcube::ast::ParseMode;
use crate::sugarcube::parser::parse_passage_body;
use crate::sugarcube::registries::CustomMacroRegistry;
use crate::zoning::build_from_ast;
use knot_core::passage::StoryFormat;
use proptest::prelude::*;
use url::Url;

fn strategy_passage() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[\\[\\]<>{}`'\"@$;/_*=~^\n a-zA-Z0-9]{0,300}")
        .expect("valid regex strategy")
}

fn zones_for(src: &str) -> knot_core::zoning::ZoneMap {
    let ast = parse_passage_body(src, 0, ParseMode::Normal);
    build_from_ast(&ast.nodes, 0, &CustomMacroRegistry::new())
}

proptest! {
    /// The zone map must satisfy its invariants (non-empty, sorted,
    /// non-overlapping leaves) for ANY input — `ZoneMap::leaf_at`'s binary
    /// search depends on it.
    #[test]
    fn zone_invariants_hold(src in strategy_passage()) {
        let zones = zones_for(&src);
        zones.validate().expect("zone invariants must hold");
    }

    /// The full format pipeline (structural parse → annotation → validation)
    /// must never panic on arbitrary input.
    #[test]
    fn parse_pipeline_never_panics(src in strategy_passage()) {
        let mut registry = FormatRegistry::with_defaults();
        let plugin = registry
            .get_mut(&StoryFormat::SugarCube)
            .expect("sugarcube plugin");
        let uri = Url::parse("file:///proptest/story.tw").unwrap();
        let _ = plugin.parse_mut(&uri, &src);
    }

    /// Per-byte language queries are total: every offset answers, and the
    /// answer is one of the four languages (no panics, no holes).
    #[test]
    fn language_queries_are_total(src in strategy_passage()) {
        let zones = zones_for(&src);
        for offset in 0..=src.len() {
            let _ = zones.language_at(offset);
        }
    }
}
