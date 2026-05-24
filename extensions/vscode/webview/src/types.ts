//! Shared type definitions for the Knot Story Map webview.

// ---------------------------------------------------------------------------
// Graph types
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
  is_start?: boolean;
  position_x?: number;
  position_y?: number;
  group?: string;
  color?: string;
  var_writes: string[];
  var_reads: string[];
  block?: string;
}

export interface KnotGraphEdge {
  source: string;
  target: string;
  edge_type: string;
  display_text?: string;
}

// ---------------------------------------------------------------------------
// Game loop types
// ---------------------------------------------------------------------------

export interface KnotGameLoop {
  members: string[];
  header: string | null;
  has_mutation: boolean;
}

// ---------------------------------------------------------------------------
// Position update types
// ---------------------------------------------------------------------------

export interface KnotPositionUpdate {
  passage_name: string;
  position_x: number;
  position_y: number;
  group?: string;
  color?: string;
}

// ---------------------------------------------------------------------------
// VS Code webview message types
// ---------------------------------------------------------------------------

export type WebviewOutboundMessage =
  | { command: 'openPassage'; file: string; line: number }
  | { command: 'refreshGraph' }
  | { command: 'updatePositions'; updates: KnotPositionUpdate[] }
  | { command: 'saveAllPositions'; updates: KnotPositionUpdate[] }
  | { command: 'updateViewport'; x: number; y: number; zoom: number }
  | { command: 'log'; level: 'error' | 'warn' | 'info'; message: string };

export type WebviewInboundMessage =
  | { command: 'updateGraph'; data: KnotGraphResponse }
  | { command: 'focusNode'; passageName: string };

// ---------------------------------------------------------------------------
// VS Code API
// ---------------------------------------------------------------------------

export type VsCodeApi = ReturnType<typeof acquireVsCodeApi>;

// ---------------------------------------------------------------------------
// React Flow node data types
// ---------------------------------------------------------------------------

export interface PassageNodeData {
  label: string;
  file: string;
  line: number;
  tags: string[];
  out_degree: number;
  in_degree: number;
  is_special: boolean;
  is_metadata: boolean;
  is_unreachable: boolean;
  is_start: boolean;
  /** Computed rendering color. Never written back to metadata. */
  color: string;
  /** Color from passage metadata only. This is what gets written back. */
  metadata_color?: string;
  var_writes: string[];
  var_reads: string[];
  group?: string;
  dimmed: boolean;
  highlighted: boolean;
  /** Temporarily true after a focusNode command. */
  focused: boolean;
  [key: string]: unknown;
}

export interface GroupNodeData {
  label: string;
  color?: string;
  [key: string]: unknown;
}