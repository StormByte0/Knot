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
     *  resolved from the virtual document with exact line numbers. */
    variable_references: KnotVariableReference[];
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

/** Notification: knot/refreshVirtualDoc */
export interface KnotRefreshVirtualDocParams {
    /** Optional reason for the refresh (for logging/debugging). */
    reason?: string;
}

// ---------------------------------------------------------------------------
// Virtual doc types (matches Rust-side KnotVirtualDocResponse)
// ---------------------------------------------------------------------------

/** Response: knot/virtualDoc */
export interface KnotVirtualDocResponse {
    /** The assembled JavaScript content of the virtual document. */
    content: string;
    /** Per-line mapping from virtual doc lines to source positions. */
    line_map: KnotVirtualDocLineEntry[];
    /** Names of all passages included in the virtual doc. */
    passage_names: string[];
}

/** A single entry in the virtual document's line map. */
export interface KnotVirtualDocLineEntry {
    /** The passage name this line belongs to. Empty for preamble lines. */
    passage_name: string;
    /** The file URI where this passage lives. Empty for preamble lines. */
    file_uri: string;
    /** The 0-based line number within the original passage body. */
    original_line: number;
}

// ---------------------------------------------------------------------------
// JS Diagnostics relay types (matches Rust-side KnotJsDiagnosticsParams)
// ---------------------------------------------------------------------------

/** Request: knot/jsDiagnostics — relay JS diagnostics from client to server. */
export interface KnotJsDiagnosticsParams {
    /** The URI of the virtual doc. */
    uri: string;
    /** JS diagnostics from VSCode's built-in JS service. */
    diagnostics: KnotJsDiagnostic[];
}

/** A single JS diagnostic from VSCode's built-in JS service. */
export interface KnotJsDiagnostic {
    /** 0-based line number where the diagnostic starts. */
    start_line: number;
    /** 0-based character offset on the start line. */
    start_character: number;
    /** 0-based line number where the diagnostic ends. */
    end_line: number;
    /** 0-based character offset on the end line. */
    end_character: number;
    /** The diagnostic message. */
    message: string;
    /** Severity: 1=Error, 2=Warning, 3=Information, 4=Hint. */
    severity: number;
    /** The diagnostic code, if any. */
    code?: string;
}

/** Response: knot/jsDiagnostics */
export interface KnotJsDiagnosticsResponse {
    /** Number of diagnostics successfully processed. */
    processed: number;
}
