//! Knot Core Engine
//!
//! This crate provides the unified document model, graph analysis engine,
//! workspace management, incremental editing pipeline, and JavaScript
//! parsing infrastructure for the Knot language server.

pub mod analysis;
pub mod css;
pub mod document;
pub mod editing;
pub mod graph;
pub mod html;
pub mod oxc;
pub mod passage;
pub mod types;
pub mod workspace;
pub mod zoning;

pub use analysis::AnalysisEngine;
pub use analysis::FormatVariableDiagnostic;
pub use analysis::PassageFlowState;
pub use document::{Document, DocumentSnapshot};
pub use graph::EdgeType;
pub use graph::GameLoopInfo;
pub use graph::PassageGraph;
pub use html::{HtmlCst, parse_html_fragment};
pub use passage::{Block, Link, Passage, PassageCategory, SpecialPassageBehavior, VarKind, VarOp};
pub use types::{BodyRequirement, MacroKind};
pub use workspace::DocumentUpdateResult;
pub use workspace::Workspace;
pub use zoning::{
    ErrorKind, LeafKind, LeafZone, MacroBody, MarkupKind, RawLanguage, TagPart, ZoneMap,
};

#[cfg(test)]
mod analysis_tests;
