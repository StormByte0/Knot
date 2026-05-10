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
}

export interface KnotGraphEdge {
    source: string;
    target: string;
    is_broken: boolean;
    display_text?: string;
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
    infinite_loop_count: number;
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
    in_infinite_loop: boolean;
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
