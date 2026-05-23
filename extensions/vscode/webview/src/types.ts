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
  position_x?: number;
  position_y?: number;
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
}

// ---------------------------------------------------------------------------
// VS Code webview message types
// ---------------------------------------------------------------------------

/** Messages sent FROM the webview TO the extension. */
export type WebviewOutboundMessage =
  | { command: 'openPassage'; file: string; line: number }
  | { command: 'refreshGraph' }
  | { command: 'openFullView' }
  | { command: 'updatePositions'; updates: KnotPositionUpdate[] }
  | { command: 'log'; level: 'error' | 'warn' | 'info'; message: string };

/** Messages sent FROM the extension TO the webview. */
export type WebviewInboundMessage =
  | { command: 'updateGraph'; data: KnotGraphResponse };

// ---------------------------------------------------------------------------
// VS Code API type
// ---------------------------------------------------------------------------

/** The VS Code webview API, acquired via acquireVsCodeApi().
 *  The actual ambient declaration is in global.d.ts. */
export type VsCodeApi = ReturnType<typeof acquireVsCodeApi>;
