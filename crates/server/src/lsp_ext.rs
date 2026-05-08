//! Custom LSP extension types for Knot-specific requests.
//!
//! These types define the request/response pairs for custom `knot/*` LSP
//! methods that the VS Code extension calls to interact with the Story Map
//! webview, trigger builds, launch preview play, and query variable flow.

use lsp_types::notification::Notification;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// knot/graph — export the passage graph for visualization
// ---------------------------------------------------------------------------

/// Request: `knot/graph` — export the passage graph for the Story Map webview.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGraphParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
}

/// Response: `knot/graph`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGraphResponse {
    /// Passage nodes in the graph.
    pub nodes: Vec<KnotGraphNode>,
    /// Edges (links) between passages.
    pub edges: Vec<KnotGraphEdge>,
    /// Optional layout hint for the webview renderer.
    pub layout: Option<String>,
}

/// A single passage node in the exported graph.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGraphNode {
    /// Unique identifier (passage name).
    pub id: String,
    /// Display label.
    pub label: String,
    /// The file URI containing this passage.
    pub file: String,
    /// The line number (0-based) where the passage header starts.
    pub line: u32,
    /// Tags assigned to this passage.
    pub tags: Vec<String>,
    /// Number of outgoing links from this passage.
    pub out_degree: u32,
    /// Number of incoming links to this passage.
    pub in_degree: u32,
    /// Whether this is a format-specific special passage.
    pub is_special: bool,
    /// Whether this is a metadata passage (StoryData / StoryTitle).
    pub is_metadata: bool,
    /// Whether this passage is unreachable from the start passage.
    pub is_unreachable: bool,
}

/// A directed edge (link) between two passages.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGraphEdge {
    /// Source passage name.
    pub source: String,
    /// Target passage name.
    pub target: String,
    /// Whether the target passage does not exist (broken link).
    pub is_broken: bool,
}

// ---------------------------------------------------------------------------
// knot/variableFlow — export variable dataflow information
// ---------------------------------------------------------------------------

/// Request: `knot/variableFlow` — query variable usage across the workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableFlowParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// Optional: filter to a specific variable name (e.g., "$gold").
    /// If omitted, returns data for all variables.
    pub variable_name: Option<String>,
}

/// Response: `knot/variableFlow`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableFlowResponse {
    /// Variable usage information across the workspace.
    pub variables: Vec<KnotVariableInfo>,
}

/// Information about a single variable's usage across passages.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableInfo {
    /// The variable name (e.g., "$gold").
    pub name: String,
    /// Whether this variable is temporary (per-passage only).
    pub is_temporary: bool,
    /// Passages where this variable is written.
    pub written_in: Vec<KnotVariableLocation>,
    /// Passages where this variable is read.
    pub read_in: Vec<KnotVariableLocation>,
    /// Whether this variable is definitely initialized from the start
    /// (e.g., via StoryInit).
    pub initialized_at_start: bool,
    /// Whether this variable is never read (unused write).
    pub is_unused: bool,
}

/// Location where a variable is used within a passage.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableLocation {
    /// The passage name.
    pub passage_name: String,
    /// The file URI containing this usage.
    pub file_uri: String,
    /// Whether this is a write or read.
    pub is_write: bool,
}

// ---------------------------------------------------------------------------
// knot/indexProgress — notification sent during workspace indexing
// ---------------------------------------------------------------------------

/// Notification: `knot/indexProgress`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotIndexProgress {
    /// Total number of files to index.
    pub total_files: u32,
    /// Number of files parsed so far.
    pub parsed_files: u32,
}

/// The LSP notification type for `knot/indexProgress`.
pub struct KnotIndexProgressNotification;

impl Notification for KnotIndexProgressNotification {
    type Params = KnotIndexProgress;
    const METHOD: &'static str = "knot/indexProgress";
}

// ---------------------------------------------------------------------------
// knot/build — trigger compilation
// ---------------------------------------------------------------------------

/// Request: `knot/build` — trigger a full project build.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBuildParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// Optional passage name to use as start for compilation.
    pub start_passage: Option<String>,
}

/// Response: `knot/build`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBuildResponse {
    /// Whether the build succeeded.
    pub success: bool,
    /// Path to the compiled output HTML file.
    pub output_path: Option<String>,
    /// Build errors (if any).
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// knot/play — get compiled HTML for preview
// ---------------------------------------------------------------------------

/// Request: `knot/play` — return compiled HTML for the preview pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPlayParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// Optional passage name to start play from (instead of the default start).
    pub start_passage: Option<String>,
}

/// Response: `knot/play`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPlayResponse {
    /// Path to the compiled HTML file for preview.
    pub html_path: Option<String>,
    /// Error message if compilation failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// knot/buildOutput — streamed build output notification
// ---------------------------------------------------------------------------

/// Notification: `knot/buildOutput` — streamed build output line.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBuildOutput {
    /// The line of compiler output.
    pub line: String,
    /// Whether this line is an error.
    pub is_error: bool,
}

pub struct KnotBuildOutputNotification;

impl Notification for KnotBuildOutputNotification {
    type Params = KnotBuildOutput;
    const METHOD: &'static str = "knot/buildOutput";
}

// ---------------------------------------------------------------------------
// knot/debug — get debug information about a passage
// ---------------------------------------------------------------------------

/// Request: `knot/debug` — get debug information about a passage.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotDebugParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// The passage name to debug.
    pub passage_name: String,
}

/// Response: `knot/debug`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotDebugResponse {
    /// The passage name.
    pub passage_name: String,
    /// The file URI containing this passage.
    pub file_uri: String,
    /// Whether this passage is reachable from start.
    pub is_reachable: bool,
    /// Whether this passage is special.
    pub is_special: bool,
    /// Whether this passage is a metadata passage.
    pub is_metadata: bool,
    /// Variables written in this passage.
    pub variables_written: Vec<KnotDebugVariable>,
    /// Variables read in this passage.
    pub variables_read: Vec<KnotDebugVariable>,
    /// Variables that are definitely initialized at this passage's entry.
    pub initialized_at_entry: Vec<String>,
    /// Outgoing links from this passage.
    pub outgoing_links: Vec<KnotDebugLink>,
    /// Incoming links to this passage.
    pub incoming_links: Vec<KnotDebugLink>,
    /// Passages that can reach this one (predecessors in the graph).
    pub predecessors: Vec<String>,
    /// Passages reachable from this one (successors in the graph).
    pub successors: Vec<String>,
    /// Whether this passage is part of an infinite loop.
    pub in_infinite_loop: bool,
    /// Diagnostic messages associated with this passage.
    pub diagnostics: Vec<KnotDebugDiagnostic>,
}

/// Variable info for debug response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotDebugVariable {
    /// Variable name.
    pub name: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
}

/// Link info for debug response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotDebugLink {
    /// Target/source passage name.
    pub passage_name: String,
    /// Display text of the link.
    pub display_text: Option<String>,
    /// Whether the link target exists.
    pub target_exists: bool,
}

/// Diagnostic info for debug response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotDebugDiagnostic {
    /// The diagnostic kind.
    pub kind: String,
    /// The diagnostic message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// knot/trace — simulate execution starting from a passage
// ---------------------------------------------------------------------------

/// Request: `knot/trace` — simulate execution starting from a passage.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotTraceParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// The passage name to start tracing from.
    pub start_passage: String,
    /// Maximum depth to trace (prevents infinite traces).
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
}

fn default_max_depth() -> u32 {
    50
}

/// Response: `knot/trace`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotTraceResponse {
    /// The execution trace steps.
    pub steps: Vec<KnotTraceStep>,
    /// Whether the trace was truncated due to max_depth.
    pub truncated: bool,
}

/// A single step in the execution trace.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotTraceStep {
    /// The passage name at this step.
    pub passage_name: String,
    /// The depth in the trace (0 = start passage).
    pub depth: u32,
    /// Variables written at this step.
    pub variables_written: Vec<String>,
    /// Links available at this step (choices the player can make).
    pub available_links: Vec<String>,
    /// Whether this step represents a loop back to a previously visited passage.
    pub is_loop: bool,
}

// ---------------------------------------------------------------------------
// knot/profile — get workspace profiling statistics
// ---------------------------------------------------------------------------

/// Request: `knot/profile` — get workspace profiling statistics.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotProfileParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
}

/// Response: `knot/profile`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotProfileResponse {
    /// Total number of documents.
    pub document_count: u32,
    /// Total number of passages.
    pub passage_count: u32,
    /// Number of special passages.
    pub special_passage_count: u32,
    /// Number of metadata passages.
    pub metadata_passage_count: u32,
    /// Number of unreachable passages.
    pub unreachable_passage_count: u32,
    /// Number of broken links.
    pub broken_link_count: u32,
    /// Number of infinite loops detected.
    pub infinite_loop_count: u32,
    /// Total number of links (edges).
    pub total_links: u32,
    /// Average outgoing links per passage.
    pub avg_out_degree: f64,
    /// Average incoming links per passage.
    pub avg_in_degree: f64,
    /// Maximum depth from start passage (longest path).
    pub max_depth: u32,
    /// Number of dead-end passages (no outgoing links and not special/metadata).
    pub dead_end_count: u32,
    /// Number of unique variables across the workspace.
    pub variable_count: u32,
    /// Number of variables with potential issues (uninitialized, unused, redundant).
    pub variable_issue_count: u32,
    /// Per-format information.
    pub format: String,
    /// Format version.
    pub format_version: Option<String>,
    /// Whether the workspace has StoryData.
    pub has_story_data: bool,
    /// Total word count across all passages (approximate).
    pub total_word_count: u32,
    /// Distribution of passages by number of outgoing links.
    pub link_distribution: KnotLinkDistribution,
    /// Per-tag statistics: tag name → count and average word count.
    pub tag_stats: Vec<KnotTagStat>,
    /// Passage complexity metrics.
    pub complexity_metrics: KnotComplexityMetrics,
    /// Structural balance analysis.
    pub structural_balance: KnotStructuralBalance,
}

/// Distribution of passages by link count ranges.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotLinkDistribution {
    /// Passages with 0 outgoing links.
    pub zero_links: u32,
    /// Passages with 1-2 outgoing links.
    pub few_links: u32,
    /// Passages with 3-5 outgoing links.
    pub moderate_links: u32,
    /// Passages with 6+ outgoing links.
    pub many_links: u32,
}

/// Per-tag statistics.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotTagStat {
    /// The tag name.
    pub tag: String,
    /// Number of passages with this tag.
    pub passage_count: u32,
    /// Average word count of passages with this tag.
    pub avg_word_count: f64,
    /// Total word count of passages with this tag.
    pub total_word_count: u32,
    /// Average number of outgoing links in passages with this tag.
    pub avg_out_links: f64,
}

/// Passage complexity metrics.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotComplexityMetrics {
    /// Average word count per passage.
    pub avg_word_count: f64,
    /// Median word count per passage.
    pub median_word_count: f64,
    /// Maximum word count in a single passage.
    pub max_word_count: u32,
    /// Minimum word count in a non-empty passage.
    pub min_word_count: u32,
    /// Average number of outgoing links per passage.
    pub avg_out_links: f64,
    /// Standard deviation of outgoing links.
    pub out_links_stddev: f64,
    /// Number of passages exceeding complexity threshold (6+ links).
    pub complex_passage_count: u32,
}

/// Structural balance analysis.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotStructuralBalance {
    /// Ratio of dead-end passages to total passages.
    pub dead_end_ratio: f64,
    /// Ratio of orphaned passages (1 incoming link) to total passages.
    pub orphaned_ratio: f64,
    /// Whether the graph is well-connected (no isolated components).
    pub is_well_connected: bool,
    /// Number of connected components.
    pub connected_components: u32,
    /// Graph diameter (longest shortest path).
    pub diameter: u32,
    /// Average clustering coefficient.
    pub avg_clustering: f64,
}

// ---------------------------------------------------------------------------
// knot/compilerDetect — detect whether a compiler is available
// ---------------------------------------------------------------------------

/// Request: `knot/compilerDetect` — detect whether a compiler is available.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotCompilerDetectParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
}

/// Response: `knot/compilerDetect`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotCompilerDetectResponse {
    /// Whether a compiler was found.
    pub compiler_found: bool,
    /// The compiler name (e.g., "tweego").
    pub compiler_name: Option<String>,
    /// The compiler version string.
    pub compiler_version: Option<String>,
    /// The path to the compiler binary.
    pub compiler_path: Option<String>,
}

// ---------------------------------------------------------------------------
// knot/breakpoints — manage debug breakpoints on passages
// ---------------------------------------------------------------------------

/// Request: `knot/breakpoints` — set or list debug breakpoints.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBreakpointsParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// If provided, set the breakpoint list to these passage names.
    /// If omitted, return the current breakpoint list without modifying it.
    pub set_breakpoints: Option<Vec<String>>,
    /// If true, clear all breakpoints.
    pub clear_all: Option<bool>,
}

/// Response: `knot/breakpoints`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBreakpointsResponse {
    /// Current list of breakpoint passage names.
    pub breakpoints: Vec<KnotBreakpointInfo>,
}

/// Information about a single breakpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotBreakpointInfo {
    /// The passage name where the breakpoint is set.
    pub passage_name: String,
    /// Whether the passage exists in the workspace.
    pub passage_exists: bool,
    /// The file URI of the passage (if it exists).
    pub file_uri: Option<String>,
    /// Number of incoming links to this passage.
    pub incoming_links: u32,
    /// Number of outgoing links from this passage.
    pub outgoing_links: u32,
}

// ---------------------------------------------------------------------------
// knot/stepOver — simulate a single step from a passage (next passage choices)
// ---------------------------------------------------------------------------

/// Request: `knot/stepOver` — get the next choices from a passage.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotStepOverParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// The passage name to step from.
    pub from_passage: String,
}

/// Response: `knot/stepOver`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotStepOverResponse {
    /// The passage we stepped from.
    pub from_passage: String,
    /// Available choices (outgoing links) from this passage.
    pub choices: Vec<KnotStepChoice>,
    /// Variables written in this passage.
    pub variables_written: Vec<String>,
    /// Variables read in this passage.
    pub variables_read: Vec<String>,
}

/// A single choice (outgoing link) in a step-over.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotStepChoice {
    /// The target passage name.
    pub passage_name: String,
    /// Display text of the link (if any).
    pub display_text: Option<String>,
    /// Whether the target passage exists.
    pub target_exists: bool,
}

// ---------------------------------------------------------------------------
// knot/watchVariables — watch specific variables across passages
// ---------------------------------------------------------------------------

/// Request: `knot/watchVariables` — get variable state at a specific passage.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotWatchVariablesParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// The passage name to inspect variable state at.
    pub at_passage: String,
    /// Optional: filter to specific variable names.
    pub filter: Option<Vec<String>>,
}

/// Response: `knot/watchVariables`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotWatchVariablesResponse {
    /// The passage name.
    pub at_passage: String,
    /// Variables definitely initialized at this passage's entry.
    pub initialized_at_entry: Vec<KnotWatchVariable>,
    /// Variables written in this passage.
    pub written_in_passage: Vec<KnotWatchVariable>,
    /// Variables read in this passage.
    pub read_in_passage: Vec<KnotWatchVariable>,
    /// Variables that may be uninitialized when reaching this passage.
    pub potentially_uninitialized: Vec<KnotWatchVariable>,
}

/// Variable watch info.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotWatchVariable {
    /// Variable name.
    pub name: String,
    /// Whether this is a temporary variable.
    pub is_temporary: bool,
    /// The file URI where this variable operation occurs.
    pub file_uri: String,
    /// The passage name where this variable was last written (if traceable).
    pub last_written_in: Option<String>,
}

// ---------------------------------------------------------------------------
// knot/reindexWorkspace — trigger full workspace re-index
// ---------------------------------------------------------------------------

/// Request: `knot/reindexWorkspace` — re-index all workspace files.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotReindexParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
}

/// Response: `knot/reindexWorkspace`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotReindexResponse {
    /// Whether re-indexing succeeded.
    pub success: bool,
    /// Number of files indexed.
    pub files_indexed: u32,
    /// Error message if re-indexing failed.
    pub error: Option<String>,
}
