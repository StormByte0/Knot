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
    sendRequest<T>(method: string, params: any): Promise<T>;
    onNotification(type: { method: string }, handler: (params: any) => void): void;
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
    /** True if this is the story's start passage (parsed from StoryData).
     *  Not yet populated by the server — client falls back to name heuristic. */
    is_start?: boolean;
    /** The x-coordinate of the passage in the Twine visual editor, if available. */
    position_x?: number;
    /** The y-coordinate of the passage in the Twine visual editor, if available. */
    position_y?: number;
    /** Persistent variable names written in this passage. */
    var_writes: string[];
    /** Persistent variable names read in this passage. */
    var_reads: string[];
    /** Block assignment placeholder for future block detection. */
    block?: string;
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
    format: string;
    format_version?: string;
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
    orphaned_ratio: number;
    is_well_connected: boolean;
    connected_components: number;
    diameter: number;
    avg_clustering: number;
}

// ---------------------------------------------------------------------------
// Debug types (matches Rust-side KnotDebugResponse)
// ---------------------------------------------------------------------------

export interface KnotDebugResponse {
    passage_name: string;
    file_uri: string;
    is_reachable: boolean;
    is_special: boolean;
    is_metadata: boolean;
    variables_written: KnotDebugVariable[];
    variables_read: KnotDebugVariable[];
    initialized_at_entry: string[];
    outgoing_links: KnotDebugLink[];
    incoming_links: KnotDebugLink[];
    predecessors: string[];
    successors: string[];
    game_loops: KnotGameLoop[];
    diagnostics: KnotDebugDiagnostic[];
}

export interface KnotDebugVariable {
    name: string;
    is_temporary: boolean;
}

export interface KnotDebugLink {
    passage_name: string;
    display_text?: string;
    target_exists: boolean;
}

export interface KnotDebugDiagnostic {
    kind: string;
    message: string;
}

// ---------------------------------------------------------------------------
// Trace types (matches Rust-side KnotTraceResponse)
// ---------------------------------------------------------------------------

export interface KnotTraceStep {
    passage_name: string;
    depth: number;
    variables_written: string[];
    available_links: string[];
    is_loop: boolean;
}

export interface KnotTraceResponse {
    steps: KnotTraceStep[];
    truncated: boolean;
}

// ---------------------------------------------------------------------------
// Step-over types (matches Rust-side KnotStepOverResponse)
// ---------------------------------------------------------------------------

export interface KnotStepChoice {
    passage_name: string;
    display_text: string | null;
    target_exists: boolean;
}

export interface KnotStepOverResponse {
    from_passage: string;
    choices: KnotStepChoice[];
    variables_written: string[];
    variables_read: string[];
}

// ---------------------------------------------------------------------------
// Variable watch types (matches Rust-side KnotWatchVariablesResponse)
// ---------------------------------------------------------------------------

export interface KnotWatchVariable {
    name: string;
    is_temporary: boolean;
    file_uri: string;
    last_written_in: string | null;
}

export interface KnotWatchVariablesResponse {
    at_passage: string;
    initialized_at_entry: KnotWatchVariable[];
    written_in_passage: KnotWatchVariable[];
    read_in_passage: KnotWatchVariable[];
    potentially_uninitialized: KnotWatchVariable[];
}

// ---------------------------------------------------------------------------
// Breakpoint types (matches Rust-side KnotBreakpointsResponse)
// ---------------------------------------------------------------------------

export interface KnotBreakpointInfo {
    passage_name: string;
    passage_exists: boolean;
    file_uri: string | null;
    incoming_links: number;
    outgoing_links: number;
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
    properties: KnotVariableProperty[];
}

export interface KnotVariableProperty {
    name: string;
    full_name: string;
    state_path: string;
    written_in: KnotVariableLocation[];
    read_in: KnotVariableLocation[];
    properties: KnotVariableProperty[];
}

export interface KnotVariableLocation {
    passage_name: string;
    file_uri: string;
    is_write: boolean;
    /** The 0-based line number within the file where this usage occurs.
     *  Enables "goto" navigation to a specific line within a passage.
     *  Defaults to 0 when not yet computed. */
    line: number;
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
}

/** Notification: knot/refreshSemanticTokens */
export interface KnotRefreshSemanticTokensParams {
    /** URIs of documents whose semantic tokens need to be refreshed. */
    document_uris: string[];
    /** Optional reason for the refresh (for logging/debugging). */
    reason?: string;
}
