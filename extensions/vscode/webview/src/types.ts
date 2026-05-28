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
  | { command: 'openPassage'; file: string; line: number; passageName: string }
  | { command: 'refreshGraph' }
  | { command: 'updatePositions'; updates: KnotPositionUpdate[] }
  | { command: 'saveAllPositions'; updates: KnotPositionUpdate[] }
  | { command: 'updateViewport'; x: number; y: number; zoom: number }
  | { command: 'log'; level: 'error' | 'warn' | 'info'; message: string };

/** Messages sent FROM the extension TO the webview. */
export type WebviewInboundMessage =
  | { command: 'updateGraph'; data: KnotGraphResponse }
  | { command: 'focusNode'; passageName: string }
  // FIX: restoreViewport was sent by storyMapProvider.ts but never handled
  | { command: 'restoreViewport'; x: number; y: number; zoom: number };

// ---------------------------------------------------------------------------
// VS Code API type
// ---------------------------------------------------------------------------

export type VsCodeApi = ReturnType<typeof acquireVsCodeApi>;

// ---------------------------------------------------------------------------
// React Flow node data types
// ---------------------------------------------------------------------------

/** Node data for a passage node in the Story Map graph.
 *
 *  Satisfies `Record<string, unknown>` so it can be used as React Flow's
 *  `Node<T>` generic parameter (which requires `T extends Record<string, unknown>`).
 *  Unlike the previous `[key: string]: unknown` index signature, this explicit
 *  satisfaction preserves full type-checking — misspelled or missing properties
 *  will be caught at compile time rather than silently accepted.
 */
export interface PassageNodeData extends Record<string, unknown> {
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
  /** Computed rendering color (display only — never written back to metadata). */
  color: string;
  /** Color from passage metadata (may be undefined). Only this is written back. */
  metadata_color?: string;
  var_writes: string[];
  var_reads: string[];
  group?: string;
  dimmed: boolean;
  highlighted: boolean;
  focused: boolean;
}

/** Node data for a group container in the Story Map graph. */
export interface GroupNodeData extends Record<string, unknown> {
  label: string;
  color?: string;
}