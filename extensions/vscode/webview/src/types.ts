//! Shared type definitions for the Knot Story Map webview.
//!
//! These interfaces mirror the extension-side types from types.ts.
//! They represent the data contracts between the VS Code extension
//! and the React webview.

// ---------------------------------------------------------------------------
// Graph types (matches extension-side KnotGraphResponse)
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
  /** True if this is the story's start passage (from StoryData).
   *  Older servers may not send this; client falls back to name heuristic. */
  is_start?: boolean;
  position_x?: number;
  position_y?: number;
  /** Group name — used to render bounding boxes around related passages. */
  group?: string;
  /** Custom color override from passage metadata. */
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
// Game loop types (matches extension-side KnotGameLoop)
// ---------------------------------------------------------------------------

export interface KnotGameLoop {
  members: string[];
  header: string | null;
  has_mutation: boolean;
}

// ---------------------------------------------------------------------------
// Position update types (matches extension-side KnotPositionUpdate)
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

/** Messages sent FROM the webview TO the extension. */
export type WebviewOutboundMessage =
  | { command: 'openPassage'; file: string; line: number }
  | { command: 'refreshGraph' }
  | { command: 'updatePositions'; updates: KnotPositionUpdate[] }
  | { command: 'saveAllPositions'; updates: KnotPositionUpdate[] }
  | { command: 'updateViewport'; x: number; y: number; zoom: number }
  | { command: 'log'; level: 'error' | 'warn' | 'info'; message: string };

/** Messages sent FROM the extension TO the webview. */
export type WebviewInboundMessage =
  | { command: 'updateGraph'; data: KnotGraphResponse }
  | { command: 'focusNode'; passageName: string };

// ---------------------------------------------------------------------------
// VS Code API type
// ---------------------------------------------------------------------------

/** The VS Code webview API, acquired via acquireVsCodeApi().
 *  The actual ambient declaration is in global.d.ts. */
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
  /** Computed rendering color (for display only, NEVER written back to metadata). */
  color: string;
  /** Color from passage metadata (may be undefined). Only THIS is written back. */
  metadata_color?: string;
  var_writes: string[];
  var_reads: string[];
  group?: string;
  dimmed: boolean;
  highlighted: boolean;
  [key: string]: unknown;
}

export interface GroupNodeData {
  label: string;
  color?: string;
  [key: string]: unknown;
}
