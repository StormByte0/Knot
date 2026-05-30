//! Custom LSP extension types for Knot-specific requests.
//!
//! These types define the request/response pairs for custom `knot/*` LSP
//! methods that the VS Code extension calls to interact with the Story Map
//! webview, trigger builds, launch preview play, and query variable flow.

use lsp_types::notification::Notification;
use lsp_types::request::Request;
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
    /// Detected game loops (strongly connected components).
    /// Cycles with state mutation — the client uses these for loop
    /// visualization (cycle highlighting, loop header indicators).
    pub game_loops: Vec<KnotGameLoop>,
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
    /// Whether this is the story's start passage (parsed from StoryData).
    pub is_start: bool,
    /// The x-coordinate of the passage in the Twine visual editor, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_x: Option<f64>,
    /// The y-coordinate of the passage in the Twine visual editor, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_y: Option<f64>,
    /// Persistent variable names written in this passage.
    #[serde(default)]
    pub var_writes: Vec<String>,
    /// Persistent variable names read in this passage.
    #[serde(default)]
    pub var_reads: Vec<String>,
    /// Manual group assignment from passage header metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Node color from passage header metadata (hex or named).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A directed edge (link) between two passages.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGraphEdge {
    /// Source passage name.
    pub source: String,
    /// Target passage name.
    pub target: String,
    /// The semantic type of this edge: "navigation", "upstream", "call",
    /// "include", "jump", or "broken".
    pub edge_type: String,
    /// The display text of the link (e.g., "Go to forest" in [[Go to forest->Forest]]).
    pub display_text: Option<String>,
}

/// A detected game loop (strongly connected component with mutation).
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGameLoop {
    /// The passages that participate in this cycle.
    pub members: Vec<String>,
    /// The identified loop header passage, or `None` if no single header
    /// could be identified.
    pub header: Option<String>,
    /// Whether the cycle contains persistent variable writes.
    pub has_mutation: bool,
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
///
/// In SugarCube, `$var` maps to `State.variables.var`. Dot-notation references
/// like `$player.name` map to `State.variables.player.name`. This struct
/// represents the base variable and its known properties as a tree, reflecting
/// the hierarchical structure of `State.variables`.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableInfo {
    /// The dollar-prefixed variable name (e.g., "$gold", "$player").
    pub name: String,
    /// The full State.variables path (e.g., "State.variables.gold",
    /// "State.variables.player").
    pub state_path: String,
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
    /// Known dot-notation properties of this variable.
    /// For `$player`, this would contain entries for `name`, `hp`, etc.
    /// Each property may itself have sub-properties (e.g., `$player.inventory.sword`
    /// means `inventory` has a sub-property `sword`).
    pub properties: Vec<KnotVariableProperty>,
    /// The structural kind of this variable: "scalar", "object", "array", or "unknown".
    /// Inferred from assignment patterns (e.g., `<<set $var to {}>>` → "object").
    pub kind: String,
}

/// A known property of a state variable, reflecting the tree structure
/// of `State.variables`.
///
/// For example, if `$player.name` and `$player.hp` are used, the `$player`
/// variable will have two properties: `name` and `hp`. Each property tracks
/// where it is read and written independently of the parent variable.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableProperty {
    /// The property name without the parent path (e.g., "name", "hp").
    pub name: String,
    /// The full dollar-prefixed path (e.g., "$player.name", "$player.hp").
    pub full_name: String,
    /// The full State.variables path (e.g., "State.variables.player.name").
    pub state_path: String,
    /// Passages where this property is written.
    pub written_in: Vec<KnotVariableLocation>,
    /// Passages where this property is read.
    pub read_in: Vec<KnotVariableLocation>,
    /// Sub-properties (e.g., for `$player.inventory.sword`, the `inventory`
    /// property would have `sword` as a sub-property).
    pub properties: Vec<KnotVariableProperty>,
    /// The structural kind of this property: "scalar", "object", "array", or "unknown".
    /// Inferred from assignment patterns.
    pub kind: String,
    /// For array-kind properties: the shape of each array element.
    /// `None` means the element shape is unknown (scalar or mixed).
    /// Contains a virtual KnotVariableProperty representing the element's structure.
    pub element_shape: Option<Box<KnotVariableProperty>>,
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
    /// The 0-based line number within the file where this usage occurs.
    /// Enables "goto" navigation to a specific line within a passage,
    /// not just the passage header. Defaults to 0 when not yet computed.
    pub line: u32,
}

// ---------------------------------------------------------------------------
// knot/noTweeFiles — notification sent when no .tw/.twee files are found
// ---------------------------------------------------------------------------

/// Notification: `knot/noTweeFiles` — no Twee source files found in workspace.
///
/// Sent by the server when the initial workspace scan finds zero `.tw`/`.twee`
/// files. The client can use this to automatically suggest project
/// initialization (skeleton generation) via the `knot.initProject` command.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotNoTweeFiles {
    /// The URI of the workspace root.
    pub workspace_uri: String,
}

/// The LSP notification type for `knot/noTweeFiles`.
pub struct KnotNoTweeFilesNotification;

impl Notification for KnotNoTweeFilesNotification {
    type Params = KnotNoTweeFiles;
    const METHOD: &'static str = "knot/noTweeFiles";
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
// knot/formatDetected — notification sent when story format is detected
// ---------------------------------------------------------------------------

/// Notification: `knot/formatDetected` — story format detected.
///
/// Sent by the server when the story format is first detected or changes.
/// The client should use this to switch document language IDs via
/// `vscode.languages.setTextDocumentLanguage()`, which activates the
/// correct TextMate grammar for the detected format (e.g., SugarCube,
/// Harlowe, Chapbook, Snowman).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatDetectedParams {
    /// The detected story format name (e.g., "SugarCube", "Harlowe", "Chapbook", "Snowman").
    pub format: String,
    /// URIs of all twee documents in the workspace that should be updated.
    pub document_uris: Vec<String>,
}

/// The LSP notification type for `knot/formatDetected`.
pub struct FormatDetectedNotification;

impl Notification for FormatDetectedNotification {
    type Params = FormatDetectedParams;
    const METHOD: &'static str = "knot/formatDetected";
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
// knot/passageDiagnostics — get diagnostic information about a passage
// ---------------------------------------------------------------------------

/// Request: `knot/passageDiagnostics` — get diagnostic information about a passage.
///
/// Returns linter issues (errors, warnings, info, hints), link connections,
/// and passage metadata (special, reachable). Variable data is available
/// separately via `knot/watchVariables` and `knot/variableFlow`.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPassageDiagnosticsParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// The passage name to inspect.
    pub passage_name: String,
}

/// Response: `knot/passageDiagnostics`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPassageDiagnosticsResponse {
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
    /// Outgoing links from this passage.
    pub outgoing_links: Vec<KnotPassageLink>,
    /// Incoming links to this passage.
    pub incoming_links: Vec<KnotPassageLink>,
    /// Diagnostic messages associated with this passage.
    pub diagnostics: Vec<KnotPassageDiagnostic>,
}

/// Link info for passage diagnostics response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPassageLink {
    /// Target/source passage name.
    pub passage_name: String,
    /// Display text of the link.
    pub display_text: Option<String>,
    /// Whether the link target exists.
    pub target_exists: bool,
}

/// Diagnostic info for passage diagnostics response.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPassageDiagnostic {
    /// The diagnostic kind.
    pub kind: String,
    /// The diagnostic message.
    pub message: String,
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
    /// Number of game loops detected.
    pub game_loop_count: u32,
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
    /// The story/project name (from StoryTitle passage body).
    pub story_name: Option<String>,
    /// Per-format information.
    pub format: String,
    /// Format version.
    pub format_version: Option<String>,
    /// The IFID (Interactive Fiction IDentifier) from StoryData.
    pub ifid: Option<String>,
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
    /// Ratio of unreachable passages (0 incoming links or no path from Start) to total passages.
    pub unreachable_ratio: f64,
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
// knot/generateIfid — generate a new IFID (Interactive Fiction IDentifier)
// ---------------------------------------------------------------------------

/// Request: `knot/generateIfid` — generate a new IFID.
///
/// IFIDs are UUIDs in uppercase, following the Twine/Twee specification.
/// This endpoint is accessible at workspace init time so that clients can
/// generate IFIDs for new project skeletons without depending on a local
/// crypto library.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGenerateIfidParams {
    /// The URI of the workspace root (for validation, not used in generation).
    pub workspace_uri: String,
}

/// Response: `knot/generateIfid`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotGenerateIfidResponse {
    /// The generated IFID (uppercase UUID).
    pub ifid: String,
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

// ---------------------------------------------------------------------------
// knot/updatePositions — update passage position metadata in source files
// ---------------------------------------------------------------------------

/// Request: `knot/updatePositions` — update the position metadata for passages
/// that were moved in the Story Map graph view.
///
/// The server applies WorkspaceEdit operations to update the `{"position":"x,y"}`
/// JSON metadata in the passage headers. This preserves compatibility with Twine
/// and other Twee editors — no custom metadata format is introduced.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotUpdatePositionsParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// Position updates: passage name → (new_x, new_y).
    pub updates: Vec<KnotPositionUpdate>,
}

/// A single passage position update.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotPositionUpdate {
    /// The passage name.
    pub passage_name: String,
    /// New x coordinate.
    pub position_x: f64,
    /// New y coordinate.
    pub position_y: f64,
    /// Optional group assignment to write back to passage metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Optional color to write back to passage metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Response: `knot/updatePositions`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotUpdatePositionsResponse {
    /// Whether all updates were applied successfully.
    pub success: bool,
    /// Number of passages updated.
    pub updated_count: u32,
    /// Errors for passages that couldn't be updated.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// knot/refreshSemanticTokens — request client to refresh semantic tokens
// ---------------------------------------------------------------------------

/// Notification: `knot/refreshSemanticTokens` — request the client to
/// re-request semantic tokens for the specified documents.
///
/// Sent by the server when a change in one document affects the semantic
/// highlighting of other documents (e.g., broken link status changes,
/// format detection updates, passage name resolution changes).
///
/// The client should call `textDocument/semanticTokens/full` for each
/// specified URI to get updated tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnotRefreshSemanticTokensParams {
    /// URIs of documents whose semantic tokens need to be refreshed.
    pub document_uris: Vec<String>,
    /// Optional reason for the refresh (for logging/debugging).
    pub reason: Option<String>,
}

/// The LSP notification type for `knot/refreshSemanticTokens`.
pub struct KnotRefreshSemanticTokensNotification;

impl Notification for KnotRefreshSemanticTokensNotification {
    type Params = KnotRefreshSemanticTokensParams;
    const METHOD: &'static str = "knot/refreshSemanticTokens";
}

// ---------------------------------------------------------------------------
// workspace/semanticTokens/refresh — standard LSP request (missing from
// lsp-types 0.94, defined here for tower-lsp send_request)
// ---------------------------------------------------------------------------

/// Server-to-client request: `workspace/semanticTokens/refresh`.
///
/// Defined in LSP 3.16+. Asks the client to refresh semantic tokens for all
/// visible documents. The client responds by re-issuing
/// `textDocument/semanticTokens/full` for every open editor.
///
/// This type is defined here because `lsp_types 0.94` does not include it.
/// Once the crate is upgraded to a version that provides it, this definition
/// can be removed.
pub struct WorkspaceSemanticTokensRefreshRequest;

impl Request for WorkspaceSemanticTokensRefreshRequest {
    type Params = ();
    type Result = ();
    const METHOD: &'static str = "workspace/semanticTokens/refresh";
}
