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
  is_start: boolean;
  /** Declares a manual reachability link (header metadata `reachable`). */
  is_entry?: boolean;
  /** Source passage name of the manual reachability link, when declared. */
  manual_link_source?: string;
  position_x?: number;
  position_y?: number;
  group?: string;
  color?: string;
  size_w?: number;
  size_h?: number;
  var_writes: string[];
  var_reads: string[];
  /** Plain-text body preview (whitespace-collapsed, truncated). */
  excerpt?: string;
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
// Metadata editing types (inspector)
// ---------------------------------------------------------------------------

/** A metadata patch for one passage.
 *
 *  Three-state semantics: `undefined` = unchanged, `null` = remove,
 *  value = set. `tags` replaces the whole list when present. */
export interface PassageMetadataPatch {
  tags?: string[];
  group?: string | null;
  color?: string | null;
  size?: { width: number; height: number } | null;
  reachable?: string | null;
  position?: { x: number; y: number };
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
  | {
      command: 'updatePassageMetadata';
      passage_name: string;
      tags?: string[];
      group?: string | null;
      color?: string | null;
      size?: { width: number; height: number } | null;
      reachable?: string | null;
      position?: { x: number; y: number };
    }
  | {
      command: 'createPassage';
      name: string;
      tags?: string[];
      position?: { x: number; y: number };
      target_file?: string;
    }
  | { command: 'deletePassage'; passage_name: string; strip_links?: boolean }
  | { command: 'renamePassage'; name: string; new_name: string }
  | { command: 'setStartPassage'; passage_name: string }
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
  /** Declares a manual reachability link (header metadata `reachable`). */
  is_entry: boolean;
  /** Source passage name of the manual reachability link, when declared. */
  manual_link_source?: string;
  is_dead_end: boolean;
  color: string;
  metadata_color?: string;
  /** Node width from passage metadata (Twine size convention). */
  size_w?: number;
  /** Node height from passage metadata (Twine size convention). */
  size_h?: number;
  var_writes: string[];
  var_reads: string[];
  /** Plain-text body preview rendered on the card (empty when no body). */
  excerpt: string;
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