//! Knot Core Engine
//!
//! This crate provides the unified document model, graph analysis engine,
//! workspace management, incremental editing pipeline, and JavaScript
//! parsing infrastructure for the Knot language server.

pub mod document;
pub mod passage;
pub mod graph;
pub mod workspace;
pub mod analysis;
pub mod editing;
pub mod oxc;

pub use document::{Document, DocumentSnapshot};
pub use passage::{Passage, Block, Link, VarOp, VarKind, SpecialPassageBehavior, PassageCategory};
pub use graph::PassageGraph;
pub use graph::EdgeType;
pub use graph::GameLoopInfo;
pub use workspace::Workspace;
pub use workspace::DocumentUpdateResult;
pub use analysis::AnalysisEngine;
pub use analysis::PassageFlowState;
pub use analysis::FormatVariableDiagnostic;

#[cfg(test)]
mod analysis_tests;
