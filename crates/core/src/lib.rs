//! Knot Core Engine
//!
//! This crate provides the unified document model, graph analysis engine,
//! workspace management, and incremental editing pipeline for the Knot
//! language server.

pub mod document;
pub mod passage;
pub mod graph;
pub mod workspace;
pub mod analysis;
pub mod editing;

pub use document::Document;
pub use passage::{Passage, Block, Link, VarOp, VarKind, SpecialPassageBehavior};
pub use graph::PassageGraph;
pub use workspace::Workspace;
pub use analysis::AnalysisEngine;
pub use analysis::PassageFlowState;
pub use analysis::FormatVariableDiagnostic;

#[cfg(test)]
mod analysis_tests;
