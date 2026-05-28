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
    /** True if this is the story's start passage (parsed from StoryData).
     *  Not yet populated by the server — client falls back to name heuristic. */
    is_start?: boolean;
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
    /** Block assignment placeholder for future block detection.
     *
     *  TODO: Implement logical block grouping. The block field is intended
     *  to simplify the graph by creating virtual logical blocks — contiguous
     *  passages that form a coherent unit in the story's control flow (e.g.,
     *  a branching dialogue tree, a mini-game sequence, a conditional section).
     *  When implemented, each block will group related nodes so that the graph
     *  can be collapsed/expanded at the block level, and variable flow tracking
     *  can scope analysis to a block's boundary. This will revolutionize the
     *  current tracking system by enabling block-scoped variable flow analysis
     *  instead of passage-scoped only.
     */
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
    orphaned_ratio: number;
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

// ---------------------------------------------------------------------------
// Variable watch types (matches Rust-side KnotWatchVariablesParams/Response)
// ---------------------------------------------------------------------------

/** Request params for knot/watchVariables. */
export interface KnotWatchVariablesParams {
    workspace_uri: string;
    at_passage: string;
    /** Optional: filter to specific variable names. */
    filter?: string[];
}

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
// Variable flow types (matches Rust-side KnotVariableFlowResponse)
// ---------------------------------------------------------------------------

export interface KnotVariableFlowParams {
    workspace_uri: string;
}

export interface KnotVariableFlowResponse {
    variables: KnotVariableInfo[];
}

export interface KnotVariableInfo {
    /** Variable name without format-specific prefix (e.g., "player", "gold"). */
    name: string;
    /** Full dot-notation path (e.g., "player", "player.hp"). */
    full_name: string;
    /** Whether this variable is temporary (per-passage only). */
    is_temporary: boolean;
    /** Total references including children (bubbled up). */
    ref_count: number;
    /** Number of distinct passages referencing this variable (including children). */
    passage_count: number;
    /** Whether this variable has child properties. */
    has_children: boolean;
    /** The type from StoryInit definition, if known. */
    struct_type?: string;
    /** Flags for this variable (unused, write-only, single-use). */
    flags: VariableFlag[];
    /** Child properties (recursive). */
    children: KnotVariableInfo[];
    /** References grouped by passage, in reachability order. */
    passages: KnotVariablePassage[];
}

export interface KnotVariablePassage {
    /** The passage name. */
    passage_name: string;
    /** BFS depth from StoryInit (0 = StoryInit itself). */
    depth: number;
    /** Whether this passage is reachable from StoryInit. */
    reachable: boolean;
    /** Whether this passage is part of a story graph loop. */
    in_loop: boolean;
    /** Total refs in this passage (including children for parent variables). */
    total_refs: number;
    /** Individual references in this passage. */
    references: KnotVariableLocation[];
}

export interface KnotVariableLocation {
    /** Whether this is a write or read. */
    is_write: boolean;
    /** The 0-based line number within the file. */
    line: number;
    /** The file URI containing this usage. */
    file_uri: string;
    /** Whether this is the initial structure definition (StoryInit). */
    is_struct_def: boolean;
    /** Whether this reassigns the whole variable (overwrites all children). */
    is_reassign: boolean;
    /** Whether this conflicts with the StoryInit type definition. */
    type_conflict: boolean;
}

export interface VariableFlag {
    /** The flag type: "unused", "write-only", or "single-use". */
    flag_type: string;
    /** A human-readable tip for the user. */
    message: string;
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
}

/** Notification: knot/refreshSemanticTokens */
export interface KnotRefreshSemanticTokensParams {
    /** URIs of documents whose semantic tokens need to be refreshed. */
    document_uris: string[];
    /** Optional reason for the refresh (for logging/debugging). */
    reason?: string;
}
