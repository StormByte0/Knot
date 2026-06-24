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
    /// Node width from passage header metadata (Twine size convention).
    /// When present, the webview uses this instead of the default node width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_w: Option<f64>,
    /// Node height from passage header metadata (Twine size convention).
    /// When present, the webview uses this instead of the default node height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_h: Option<f64>,
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
    ///
    /// For Array-kind root variables, this is EMPTY — the element shape
    /// is stored in `element_shape` instead, using `[*]` notation.
    pub properties: Vec<KnotVariableProperty>,
    /// The structural kind of this variable: "scalar", "object", "array", or "unknown".
    /// Inferred from assignment patterns (e.g., `<<set $var to {}>>` → "object").
    pub kind: String,
    /// For Array-kind root variables: the shape of each array element.
    /// Contains a virtual KnotVariableProperty representing the element's structure
    /// with `[*]` notation. `None` for non-array root variables or arrays with
    /// unknown element shape.
    pub element_shape: Option<Box<KnotVariableProperty>>,
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
    /// For `[*]` property nodes: coverage annotation for irregular arrays.
    /// `None` for non-array properties or regular arrays (100% coverage).
    /// Format: "present_in/total" (e.g., "3/5" means property exists in 3 of 5 elements).
    /// Only set when coverage < 100%.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<String>,
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
    /// The document-absolute byte span of this usage within the source file.
    /// Enables precise highlighting and range-based navigation (e.g.,
    /// selecting the exact `key: value` token rather than just jumping
    /// to the line). `None` when span data is not available.
    pub span: Option<(u32, u32)>,
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
    /// The URI of the workspace root. Used by the client in the
    /// `knot/formatSwitchComplete` handshake to identify which workspace
    /// completed its language ID switches.
    pub workspace_uri: String,
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
    /// Optional compiler path override from the extension (VS Code setting
    /// `knot.tweegoPath`). When provided, this takes priority over the
    /// server's config and PATH lookup.
    pub compiler_path: Option<String>,
    /// Optional source directory override from the extension (VS Code setting
    /// `knot.build.sourceDir`). When provided, this takes priority over the
    /// server's config (`build.source_dir` in `.vscode/knot.json`).
    ///
    /// This is a subdirectory name relative to the workspace root (e.g. "src").
    /// When unset or empty, the workspace root is used as the source directory.
    #[serde(default)]
    pub source_dir: Option<String>,
    /// Optional output directory override from the extension (VS Code setting
    /// `knot.build.outputDir`). When provided, this takes priority over the
    /// server's config.
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Optional storyformats directory override from the extension (VS Code
    /// setting `knot.storyformats.path`). When provided, this takes priority
    /// over the server's config.
    #[serde(default)]
    pub storyformats_path: Option<String>,
    /// Path to the Knot-managed storyformats directory (in VS Code's
    /// globalStorage). The extension sets this to
    /// `<globalStorage>/storyformats/` when it has downloaded storyformats
    /// there.
    ///
    /// At build time, the server sets `TWEEGO_PATH` to this path so tweego
    /// finds the managed formats. This is searched AFTER `<cwd>/storyformats/`,
    /// so project-local format overrides take priority.
    #[serde(default)]
    pub managed_storyformats_path: Option<String>,
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
    /// Optional compiler path override from the extension.
    pub compiler_path: Option<String>,
    /// Optional source directory override (VS Code setting `knot.build.sourceDir`).
    #[serde(default)]
    pub source_dir: Option<String>,
    /// Optional output directory override (VS Code setting `knot.build.outputDir`).
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Optional storyformats directory override (VS Code setting `knot.storyformats.path`).
    #[serde(default)]
    pub storyformats_path: Option<String>,
    /// Path to the Knot-managed storyformats directory (in VS Code's
    /// globalStorage). See `KnotBuildParams.managed_storyformats_path`.
    #[serde(default)]
    pub managed_storyformats_path: Option<String>,
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
    /// Variable references (reads and writes) in this passage,
    /// resolved from the format plugin with exact line numbers.
    /// Enables the client to show where each variable is read/written
    /// and navigate to the specific source line.
    pub variable_references: Vec<KnotVariableReference>,
    /// Passage-scoped temporary variables (`_var` in SugarCube) declared
    /// in this passage, with their read/write counts and line-level
    /// references. Unlike persistent (`$`) variables, these are scoped
    /// to the passage and are therefore surfaced here rather than in
    /// the workspace-wide variable tracker.
    pub temporary_variables: Vec<KnotTemporaryVariable>,
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

/// A variable reference (read or write) within a passage.
///
/// Used by the passage diagnostics response to show which variables
/// are read/written in a passage, with exact line numbers so the
/// client can navigate to the specific source line.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotVariableReference {
    /// The dollar-prefixed variable name (e.g., "$gold", "$player.name").
    pub variable_name: String,
    /// Whether this is a write (true) or read (false).
    pub is_write: bool,
    /// The 0-based line number within the source file.
    pub line: u32,
    /// The file URI containing this reference.
    pub file_uri: String,
    /// The passage name where this reference occurs.
    pub passage_name: String,
    /// The document-absolute byte span [start, end) of this reference.
    /// Enables precise highlighting and range-based navigation.
    /// `None` when span data is not available.
    pub span_start: Option<u32>,
    pub span_end: Option<u32>,
}

/// A passage-scoped temporary variable (`_var` in SugarCube) summary
/// for the passage diagnostics panel.
///
/// Each entry groups all reads and writes of one temporary variable
/// inside a single passage so the client can render a compact
/// infographics block (name + read/write counts + clickable line
/// refs) without having to do its own grouping.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotTemporaryVariable {
    /// The temporary variable name with sigil (e.g., "_counter").
    pub name: String,
    /// Number of write accesses inside this passage.
    pub write_count: u32,
    /// Number of read accesses inside this passage.
    pub read_count: u32,
    /// Line-level references (writes and reads, in source order) so
    /// the client can offer "go to line" navigation. Reuses the same
    /// shape as persistent-variable references for client-side parity.
    pub references: Vec<KnotVariableReference>,
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

// ---------------------------------------------------------------------------
// knot/clientReady — handshake to confirm extension is ready
// ---------------------------------------------------------------------------

/// Request: `knot/clientReady` — signal that the extension is fully initialized.
///
/// The server waits for this before starting workspace indexing. This
/// eliminates the race where the server sends `formatDetected` before
/// the extension has registered notification handlers.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyParams {}

/// Response: `knot/clientReady`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotClientReadyResponse {
    /// Whether the server acknowledged the client is ready.
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// knot/formatSwitchComplete — handshake to confirm format switch cascade is done
// ---------------------------------------------------------------------------

/// Request: `knot/formatSwitchComplete` — signal that the extension has
/// finished switching all document language IDs after a `formatDetected`
/// notification.
///
/// The server uses this to clear `format_switch_in_progress` and send ONE
/// unified `workspace/semanticTokens/refresh`, preventing the O(N²) token
/// request flood that would otherwise occur.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatSwitchCompleteParams {
    /// The URI of the workspace root.
    pub workspace_uri: String,
    /// Number of documents whose language ID was switched.
    pub switched_count: u32,
}

/// Response: `knot/formatSwitchComplete`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatSwitchCompleteResponse {
    /// Whether the server acknowledged the format switch completion.
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// knot/formats/list — list installed story formats
// ---------------------------------------------------------------------------

/// Request: `knot/formats/list` — return the catalog of installed story
/// formats discovered by the server.
///
/// The server resolves the storyformats directory (see
/// `helpers::compiler::resolve_storyformats_dir`), scans it for
/// subdirectories containing `format.js`, parses each, and returns the
/// resulting list. The catalog is cached on `ServerStateInner` and only
/// re-scanned when the user changes `knot.storyformats.path` or invokes
/// `knot/formats/refresh`.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatsListParams {
    /// The URI of the workspace root. Used to resolve project-local
    /// `.storyformats/` directories. May be empty to use the server's
    /// current workspace root.
    #[serde(default)]
    pub workspace_uri: String,
    /// Optional override for the storyformats directory. When set, the
    /// server scans this directory instead of the configured path. Used
    /// by the `Knot: Configure Story Formats` command's "browse for
    /// folder" flow to preview what's in a directory before saving it.
    #[serde(default)]
    pub path_override: Option<String>,
}

/// Response: `knot/formats/list`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatsListResponse {
    /// The storyformats directory the server scanned. Empty if no
    /// directory could be resolved.
    pub resolved_dir: Option<String>,
    /// The list of installed formats found in that directory. Empty if
    /// `resolved_dir` is None or contains no `format.js` files.
    pub formats: Vec<KnotFormatEntry>,
    /// The configured storyformats path (from `knot.storyformats.path`
    /// setting or `.vscode/knot.json`). Empty when unset.
    pub configured_path: Option<String>,
    /// The format name detected from the project's StoryData passage
    /// (e.g. "SugarCube"). `None` if no StoryData was found or the format
    /// field is missing. Used by the extension to offer one-click download
    /// of the exact format version the project needs.
    #[serde(default)]
    pub project_format: Option<String>,
    /// The format version detected from the project's StoryData passage
    /// (e.g. "2.37.0"). `None` if no StoryData or no format-version field.
    #[serde(default)]
    pub project_format_version: Option<String>,
    /// Whether the project's needed format is already available in the
    /// managed cache. `None` if we can't determine (no StoryData or no
    /// global storage path). `Some(true)` means the build will find it;
    /// `Some(false)` means the user should download it.
    #[serde(default)]
    pub project_format_cached: Option<bool>,
}

/// A single installed story format entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatEntry {
    /// Format name (e.g. "SugarCube", "Harlowe").
    pub name: String,
    /// Format version (e.g. "2.37.0").
    pub version: String,
    /// Short description.
    #[serde(default)]
    pub description: String,
    /// Author name(s).
    #[serde(default)]
    pub author: String,
    /// License identifier (e.g. "BSD-3-Clause").
    #[serde(default)]
    pub license: String,
    /// Source code URL.
    #[serde(default)]
    pub source: String,
    /// Homepage URL.
    #[serde(default)]
    pub url: String,
    /// Absolute path to the format directory (contains format.js, etc.).
    pub dir: String,
    /// Name of the format directory (e.g. "sugarcube-2").
    pub dir_name: String,
}

// ---------------------------------------------------------------------------
// knot/formats/refresh — re-scan the storyformats directory
// ---------------------------------------------------------------------------

/// Request: `knot/formats/refresh` — force the server to re-scan the
/// storyformats directory and refresh the in-memory catalog.
///
/// Called by the extension when:
/// - The user changes `knot.storyformats.path` in Settings.
/// - The user adds/removes a format directory on disk.
/// - The `Knot: Configure Story Formats` command runs after a path change.
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatsRefreshParams {
    /// The URI of the workspace root.
    #[serde(default)]
    pub workspace_uri: String,
    /// Optional storyformats directory override from the extension (VS Code
    /// setting `knot.storyformats.path`). When provided, this takes priority
    /// over the server's config (`.vscode/knot.json`).
    #[serde(default)]
    pub storyformats_path: Option<String>,
}

/// Response: `knot/formats/refresh`
#[derive(Debug, Serialize, Deserialize)]
pub struct KnotFormatsRefreshResponse {
    /// Whether the refresh succeeded.
    pub success: bool,
    /// The storyformats directory that was scanned. Empty if resolution failed.
    pub resolved_dir: Option<String>,
    /// Number of formats discovered.
    pub format_count: usize,
    /// Error message if the refresh failed.
    #[serde(default)]
    pub error: Option<String>,
}
