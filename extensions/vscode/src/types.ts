//! Shared type definitions for the Knot VS Code extension.
//!
//! These interfaces match the Rust server's lsp_ext types and are used
//! across all providers and the main extension module.

// LanguageClient type for use across providers.
// The actual class is only available at runtime from the node entry point,
// so we define an interface covering the methods we use.
export interface KnotLanguageClient {
    start(): Promise<void>;
    stop(): Promise<void>;
    isRunning(): boolean;
    sendRequest<T>(method: string, params: object): Promise<T>;
    sendNotification(method: string, params: object): void;
    onNotification<P = Record<string, unknown>>(type: { method: string }, handler: (params: P) => void): void;
}

// ---------------------------------------------------------------------------
// Graph types (matches Rust-side KnotGraphResponse)
// ---------------------------------------------------------------------------

export interface KnotGraphResponse {
    nodes: KnotGraphNode[];
    edges: KnotGraphEdge[];
    game_loops: KnotGameLoop[];
    layout?: string;
}

export interface KnotGraphNode {
    id: string;
    label: string;
    file: string;
    line: number;
    tags: string[];
    out_degree: number;
    in_degree: number;
    is_special: boolean;
    is_metadata: boolean;
    is_unreachable: boolean;
    /** True if this is the story's start passage (parsed from StoryData). */
    is_start: boolean;
    /** The x-coordinate of the passage in the Twine visual editor, if available. */
    position_x?: number;
    /** The y-coordinate of the passage in the Twine visual editor, if available. */
    position_y?: number;
    /** Manual group assignment from passage header metadata. */
    group?: string;
    /** Node color from passage header metadata. */
    color?: string;
    /** Persistent variable names written in this passage. */
    var_writes: string[];
    /** Persistent variable names read in this passage. */
    var_reads: string[];
}

export interface KnotGraphEdge {
    source: string;
    target: string;
    /** The semantic type of this edge: "navigation", "upstream", "call", "include", "jump", or "broken". */
    edge_type: string;
    display_text?: string;
}

// ---------------------------------------------------------------------------
// Game loop types (matches Rust-side KnotGameLoop)
// ---------------------------------------------------------------------------

export interface KnotGameLoop {
    /** The passages that participate in this cycle. */
    members: string[];
    /** The identified loop header passage, or null if no single header could be identified. */
    header: string | null;
    /** Whether the cycle contains persistent variable writes. */
    has_mutation: boolean;
}

// ---------------------------------------------------------------------------
// Profile types (matches Rust-side KnotProfileResponse)
// ---------------------------------------------------------------------------

export interface KnotProfileResponse {
    document_count: number;
    passage_count: number;
    special_passage_count: number;
    metadata_passage_count: number;
    unreachable_passage_count: number;
    broken_link_count: number;
    game_loop_count: number;
    total_links: number;
    avg_out_degree: number;
    avg_in_degree: number;
    max_depth: number;
    dead_end_count: number;
    variable_count: number;
    variable_issue_count: number;
    /** The story/project name (from StoryTitle passage body). */
    story_name?: string;
    format: string;
    format_version?: string;
    /** The IFID (Interactive Fiction IDentifier) from StoryData. */
    ifid?: string;
    has_story_data: boolean;
    total_word_count: number;
    link_distribution: KnotLinkDistribution;
    tag_stats: KnotTagStat[];
    complexity_metrics: KnotComplexityMetrics;
    structural_balance: KnotStructuralBalance;
}

export interface KnotLinkDistribution {
    zero_links: number;
    few_links: number;
    moderate_links: number;
    many_links: number;
}

export interface KnotTagStat {
    tag: string;
    passage_count: number;
    avg_word_count: number;
    total_word_count: number;
    avg_out_links: number;
}

export interface KnotComplexityMetrics {
    avg_word_count: number;
    median_word_count: number;
    max_word_count: number;
    min_word_count: number;
    avg_out_links: number;
    out_links_stddev: number;
    complex_passage_count: number;
}

export interface KnotStructuralBalance {
    dead_end_ratio: number;
    unreachable_ratio: number;
    is_well_connected: boolean;
    connected_components: number;
    diameter: number;
    avg_clustering: number;
}

// ---------------------------------------------------------------------------
// Passage diagnostics types (matches Rust-side KnotPassageDiagnosticsResponse)
// ---------------------------------------------------------------------------

export interface KnotPassageDiagnosticsResponse {
    passage_name: string;
    file_uri: string;
    is_reachable: boolean;
    is_special: boolean;
    is_metadata: boolean;
    outgoing_links: KnotPassageLink[];
    incoming_links: KnotPassageLink[];
    diagnostics: KnotPassageDiagnostic[];
    /** Variable references (reads and writes) in this passage,
     *  resolved from passage analysis with exact line numbers. */
    variable_references: KnotVariableReference[];
    /** Passage-scoped temporary variables (`_var` in SugarCube) declared
     *  in this passage. Empty for formats without passage-scoped temps. */
    temporary_variables: KnotTemporaryVariable[];
}

export interface KnotPassageLink {
    passage_name: string;
    display_text?: string;
    target_exists: boolean;
}

export interface KnotPassageDiagnostic {
    kind: string;
    message: string;
}

/** A variable reference (read or write) within a passage. */
export interface KnotVariableReference {
    /** The variable name (e.g., "$gold", "$player.name"). */
    variable_name: string;
    /** Whether this is a write (true) or read (false). */
    is_write: boolean;
    /** The 0-based line number within the source file. */
    line: number;
    /** The file URI containing this reference. */
    file_uri: string;
    /** The passage name where this reference occurs. */
    passage_name: string;
    /** The document-absolute byte offset of the start of this reference.
     *  `null` when span data is not available. */
    span_start: number | null;
    /** The document-absolute byte offset of the end of this reference.
     *  `null` when span data is not available. */
    span_end: number | null;
}

/** A passage-scoped temporary variable summary (`_var` in SugarCube). */
export interface KnotTemporaryVariable {
    /** The temporary variable name with sigil (e.g., "_counter"). */
    name: string;
    /** Number of write accesses inside this passage. */
    write_count: number;
    /** Number of read accesses inside this passage. */
    read_count: number;
    /** Line-level references (writes and reads, in source order). */
    references: KnotVariableReference[];
}

// ---------------------------------------------------------------------------
// Variable flow types (matches Rust-side KnotVariableFlowResponse)
// ---------------------------------------------------------------------------

export interface KnotVariableFlowParams {
    workspace_uri: string;
    variable_name?: string;
}

export interface KnotVariableFlowResponse {
    variables: KnotVariableInfo[];
}

export interface KnotVariableInfo {
    name: string;
    state_path: string;
    is_temporary: boolean;
    written_in: KnotVariableLocation[];
    read_in: KnotVariableLocation[];
    initialized_at_start: boolean;
    is_unused: boolean;
    /** The structural kind: "scalar", "object", "array", or "unknown". */
    kind: string;
    properties: KnotVariableProperty[];
    /** For Array-kind root variables: the shape of each array element.
     *  Contains a virtual [*] node whose children describe element properties. */
    element_shape?: KnotVariableProperty;
}

export interface KnotVariableProperty {
    name: string;
    full_name: string;
    state_path: string;
    written_in: KnotVariableLocation[];
    read_in: KnotVariableLocation[];
    properties: KnotVariableProperty[];
    /** The structural kind: "scalar", "object", "array", or "unknown". */
    kind: string;
    /** For array-kind properties: the shape of each array element. */
    element_shape?: KnotVariableProperty;
    /** Coverage annotation for irregular arrays (e.g., "3/5"). */
    coverage?: string;
}

export interface KnotVariableLocation {
    passage_name: string;
    file_uri: string;
    is_write: boolean;
    /** The 0-based line number within the file where this usage occurs.
     *  Enables "goto" navigation to a specific line within a passage.
     *  Defaults to 0 when not yet computed. */
    line: number;
    /** The document-absolute byte span [start, end) of this usage.
     *  Enables precise highlighting and range-based navigation.
     *  `null` when span data is not available. */
    span: [number, number] | null;
}

// ---------------------------------------------------------------------------
// Build types (matches Rust-side KnotBuildResponse)
// ---------------------------------------------------------------------------

export interface KnotBuildResponse {
    success: boolean;
    output_path?: string;
    errors: string[];
}

// ---------------------------------------------------------------------------
// Play types (matches Rust-side KnotPlayResponse)
// ---------------------------------------------------------------------------

export interface KnotPlayResponse {
    html_path?: string;
    error?: string;
}

// ---------------------------------------------------------------------------
// Compiler detection types (matches Rust-side KnotCompilerDetectResponse)
// ---------------------------------------------------------------------------

export interface KnotCompilerDetectResponse {
    compiler_found: boolean;
    compiler_name?: string;
    compiler_version?: string;
    compiler_path?: string;
}

// ---------------------------------------------------------------------------
// Story formats catalog types (matches Rust-side KnotFormatsListResponse)
// ---------------------------------------------------------------------------

/** A single installed story format entry, parsed from a format.js file. */
export interface KnotFormatEntry {
    /** Format name (e.g. "SugarCube", "Harlowe"). */
    name: string;
    /** Format version (e.g. "2.37.0"). */
    version: string;
    /** Short description. */
    description?: string;
    /** Author name(s). */
    author?: string;
    /** License identifier (e.g. "BSD-3-Clause"). */
    license?: string;
    /** Source code URL. */
    source?: string;
    /** Homepage URL. */
    url?: string;
    /** Absolute path to the format directory (contains format.js, etc.). */
    dir: string;
    /** Name of the format directory (e.g. "sugarcube-2"). */
    dir_name: string;
}

export interface KnotFormatsListParams {
    workspace_uri?: string;
    /** Optional override for the storyformats directory. Used by the
     *  "Browse for folder..." flow to preview what's in a directory
     *  before saving it. */
    path_override?: string;
}

export interface KnotFormatsListResponse {
    /** The storyformats directory the server scanned. Null if no
     *  directory could be resolved. */
    resolved_dir: string | null;
    /** The list of installed formats found. */
    formats: KnotFormatEntry[];
    /** The configured storyformats path (from knot.build.storyformatsPath
     *  setting or .vscode/knot.json). Null when unset. */
    configured_path: string | null;
    /** The format name detected from the project's StoryData passage
     *  (e.g. "SugarCube"). Null if no StoryData was found. */
    project_format?: string | null;
    /** The format version detected from the project's StoryData passage
     *  (e.g. "2.37.0"). Null if no StoryData or no format-version field. */
    project_format_version?: string | null;
    /** Whether the project's needed format is already in the managed cache.
     *  Null if we can't determine (no StoryData or no global storage path). */
    project_format_cached?: boolean | null;
}

export interface KnotFormatsRefreshParams {
    workspace_uri?: string;
    /** Optional storyformats directory override from the VS Code
     *  `knot.build.storyformatsPath` setting. When provided, takes priority
     *  over `.vscode/knot.json`. */
    storyformats_path?: string;
}

export interface KnotFormatsRefreshResponse {
    success: boolean;
    resolved_dir: string | null;
    format_count: number;
    error?: string;
}

// ---------------------------------------------------------------------------
// Reindex types (matches Rust-side KnotReindexResponse)
// ---------------------------------------------------------------------------

export interface KnotReindexResponse {
    success: boolean;
    files_indexed: number;
    error?: string;
}

// ---------------------------------------------------------------------------
// IFID generation types (matches Rust-side KnotGenerateIfidResponse)
// ---------------------------------------------------------------------------

export interface KnotGenerateIfidResponse {
    ifid: string;
}

// ---------------------------------------------------------------------------
// Update positions types (matches Rust-side KnotUpdatePositionsResponse)
// ---------------------------------------------------------------------------

export interface KnotPositionUpdate {
    passage_name: string;
    position_x: number;
    position_y: number;
    /** Optional group assignment to write back to passage metadata. */
    group?: string;
    /** Optional color to write back to passage metadata. */
    color?: string;
}

export interface KnotUpdatePositionsParams {
    workspace_uri: string;
    updates: KnotPositionUpdate[];
}

export interface KnotUpdatePositionsResponse {
    success: boolean;
    updated_count: number;
    errors: string[];
}

// ---------------------------------------------------------------------------
// Notification types
// ---------------------------------------------------------------------------

/** Notification: knot/indexProgress */
export interface KnotIndexProgress {
    total_files: number;
    parsed_files: number;
}

/** Notification: knot/buildOutput */
export interface KnotBuildOutput {
    line: string;
    is_error: boolean;
}

/** Notification: knot/formatDetected */
export interface KnotFormatDetectedParams {
    format: string;
    document_uris: string[];
    /** The URI of the workspace root. Used by the formatSwitchComplete handshake. */
    workspace_uri: string;
}

/** Notification: knot/refreshSemanticTokens */
export interface KnotRefreshSemanticTokensParams {
    /** URIs of documents whose semantic tokens need to be refreshed. */
    document_uris: string[];
    /** Optional reason for the refresh (for logging/debugging). */
    reason?: string;
}

// ---------------------------------------------------------------------------
// Handshake types (matches Rust-side lsp_ext types)
// ---------------------------------------------------------------------------

/** Request: knot/clientReady */
export type KnotClientReadyParams = Record<string, never>;

/** Response: knot/clientReady */
export interface KnotClientReadyResponse {
    acknowledged: boolean;
}

/** Request: knot/formatSwitchComplete */
export interface KnotFormatSwitchCompleteParams {
    workspace_uri: string;
    switched_count: number;
}

/** Response: knot/formatSwitchComplete */
export interface KnotFormatSwitchCompleteResponse {
    acknowledged: boolean;
}