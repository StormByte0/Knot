//! Legacy SugarCube macro catalog — DEPRECATED.
//!
//! All format-specific behavioral data has been moved to the format plugin
//! architecture. See:
//!
//! - `knot_formats::types` — shared types (MacroDef, MacroArgDef, etc.)
//! - `knot_formats::plugin::FormatPlugin` — trait with behavioral methods
//! - `knot_formats::sugarcube::macros` — SugarCube macro catalog data
//!
//! Handlers should query the active format plugin via
//! `inner.format_registry.get(&format)` instead of importing from this module.
//!
//! This file is kept temporarily for backward compatibility during the
//! migration. It re-exports from the format plugin for any remaining callers.

// Re-export types from the format plugin for any transition code that still
// references this module. New code should import directly from knot_formats.
pub use knot_formats::types::{
    MacroArgDef, MacroArgKind, MacroCategory, MacroDef,
};
