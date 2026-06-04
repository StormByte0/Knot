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
pub mod virtual_doc;

pub use document::{Document, DocumentSnapshot};
pub use passage::{Passage, Block, Link, VarOp, VarKind, SpecialPassageBehavior, PassageCategory};
pub use graph::PassageGraph;
pub use graph::EdgeType;
pub use graph::GameLoopInfo;
pub use workspace::Workspace;
pub use analysis::AnalysisEngine;
pub use analysis::PassageFlowState;
pub use analysis::FormatVariableDiagnostic;
pub use virtual_doc::{
    VirtualDocAdapter, VirtualDocManager, PassageEntry,
    TranslatedBlock, AdapterContext, SourceLocation,
    JsDiagnostic, TwDiagnostic, DiagnosticSeverity,
    SourceTextProvider, NoSourceText,
    StartupAlias, UserCallable, UserCallableKind,
};

#[cfg(test)]
mod analysis_tests;
