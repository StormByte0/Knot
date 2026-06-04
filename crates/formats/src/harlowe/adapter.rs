//! Harlowe stub adapter — returns None for all methods.
//!
//! Harlowe uses a different variable model (no direct JS state access),
//! so the virtual doc pipeline is not yet applicable. This stub allows
//! the VirtualDocManager to work with Harlowe workspaces without panicking.

use knot_core::passage::Passage;
use knot_core::virtual_doc::{
    AdapterContext, JsDiagnostic, SourceLocation, SourceTextProvider,
    TranslatedBlock, TwDiagnostic, VirtualDocAdapter,
};
use std::ops::Range;

/// Harlowe's stub implementation of `VirtualDocAdapter`.
///
/// Returns `false` for `should_include_passage()`, `None` for
/// `translate_passage()`, and passes diagnostics through unchanged.
/// This is a placeholder until Harlowe-specific translation is implemented.
pub struct HarloweAdapter;

impl VirtualDocAdapter for HarloweAdapter {
    fn should_include_passage(&self, _passage: &Passage) -> bool {
        false
    }

    fn translate_passage(
        &self,
        _passage: &Passage,
        _source_text: &dyn SourceTextProvider,
        _context: &AdapterContext,
    ) -> Option<TranslatedBlock> {
        None
    }

    fn resolve_source_location(
        &self,
        _passage_name: &str,
        file_uri: &str,
        vdoc_byte_range: Range<usize>,
        _source_text: &str,
    ) -> SourceLocation {
        SourceLocation {
            file_uri: file_uri.to_string(),
            byte_range: vdoc_byte_range,
        }
    }

    fn interpret_diagnostic(
        &self,
        js_diagnostic: &JsDiagnostic,
        _passage_name: &str,
        file_uri: &str,
    ) -> Option<TwDiagnostic> {
        Some(TwDiagnostic {
            file_uri: file_uri.to_string(),
            byte_range: js_diagnostic.byte_range.clone(),
            message: js_diagnostic.message.clone(),
            severity: js_diagnostic.severity,
            code: js_diagnostic.code.clone(),
        })
    }

    fn clear_state(&self) {}

    fn invalidate_passage(&self, _passage_name: &str) {}
}
