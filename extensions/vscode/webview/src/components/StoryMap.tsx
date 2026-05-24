import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  ReactFlow,
  MiniMap,
  Controls,
  Background,
  useNodesState,
  useEdgesState,
  useReactFlow,
  Handle,
  Position,
  BackgroundVariant,
  ConnectionMode,
  Node,
  Edge,
  NodeChange,
  NodePositionChange,
  NodeProps,
  EdgeProps,
  getStraightPath,
  ReactFlowProvider,
  MarkerType,
  useStore,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import dagre from 'dagre';
import {
  KnotGraphResponse,
  KnotGraphNode,
  KnotGraphEdge,
  KnotPositionUpdate,
  PassageNodeData,
  GroupNodeData,
} from '../types';
import { vscode } from '../App';

// ── Constants ──────────────────────────────────────────────────────────────

const GRID_SNAP = 20;
const NODE_W = 160;
const NODE_H = 40;
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 4.0;
// Perpendicular pixel offset for bidirectional edge pairs
const BIDIR_OFFSET = 7;

// ── Twine 2 layout anchors ────────────────────────────────────────────────
// Start passage anchors here; regular graph flows right/down from it.
const START_ANCHOR_X = 420;
const START_ANCHOR_Y = 60;
// Special passages bounding box starts here (top-left corner).
const SPECIAL_BOX_X = 40;
const SPECIAL_BOX_Y = 60;
// Unreachable list starts below the special box.
const UNREACHABLE_LIST_X = 40;
const UNREACHABLE_LIST_Y_OFFSET = 60; // gap below specials

// ── Color palette ──────────────────────────────────────────────────────────

const COLORS = {
  normal:     '#2d6a9f',
  start:      '#2e7d32',
  special:    '#e65100',
  metadata:   '#6a1b9a',
  unreachable:'#424242',
  broken:     '#c62828',
  edgeNav:    '#5a6a7e',
  edgeUp:     '#3a4555',
  edgeBroken: '#c62828',
};

// ── Helpers ────────────────────────────────────────────────────────────────

function snap(v: number) { return Math.round(v / GRID_SNAP) * GRID_SNAP; }

function nodeColor(n: KnotGraphNode): string {
  if (n.color) return n.color;
  if (n.is_unreachable) return COLORS.unreachable;
  if (n.is_start || n.id === 'Start') return COLORS.start;
  if (n.is_metadata) return COLORS.metadata;
  if (n.is_special) return COLORS.special;
  return COLORS.normal;
}

function debounce<T extends (...args: any[]) => void>(fn: T, ms: number): T {
  let t: ReturnType<typeof setTimeout> | null = null;
  return ((...args: Parameters<T>) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  }) as T;
}

// ── Perpendicular offset for a straight line ───────────────────────────────
// Returns a new (x, y) shifted `dist` pixels perpendicular to the direction
// from (x1,y1) to (x2,y2).
function perpOffset(
  x1: number, y1: number, x2: number, y2: number, dist: number,
): [number, number] {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len = Math.sqrt(dx * dx + dy * dy) || 1;
  // Perpendicular unit vector: (-dy, dx) / len
  return [(-dy / len) * dist, (dx / len) * dist];
}

// ── Rectangle intersection for floating edges ──────────────────────────────
// Given a center point, a target point, and a rectangle size, returns the
// point where the center→target line intersects the rectangle boundary.
function getRectIntersection(
  center: { x: number; y: number },
  target: { x: number; y: number },
  width: number,
  height: number,
): [number, number] {
  const dx = target.x - center.x;
  const dy = target.y - center.y;
  if (dx === 0 && dy === 0) return [center.x, center.y];

  const halfW = width / 2;
  const halfH = height / 2;
  const absDx = Math.abs(dx);
  const absDy = Math.abs(dy);

  // Determine which edge the center→target line intersects
  const scale = (absDx * halfH > absDy * halfW)
    ? halfW / absDx   // left or right edge
    : halfH / absDy;  // top or bottom edge

  return [center.x + dx * scale, center.y + dy * scale];
}

// ── Custom Node: Passage ───────────────────────────────────────────────────

function PassageNode({ data }: NodeProps<Node<PassageNodeData>>) {
  const d = data as PassageNodeData;
  const color = d.color || COLORS.normal;

  const cls = [
    'pn',
    d.is_start      && 'pn--start',
    d.is_special    && 'pn--special',
    d.is_metadata   && 'pn--metadata',
    d.is_unreachable && 'pn--unreachable',
    d.highlighted   && 'pn--highlighted',
    d.dimmed        && 'pn--dimmed',
    d.focused       && 'pn--focused',
  ].filter(Boolean).join(' ');

  return (
    <div className={cls} style={{ '--node-color': color } as React.CSSProperties}>
      <Handle type="target" position={Position.Top}    className="pn__handle" />
      <span className="pn__label" title={d.label}>{d.label}</span>
      {d.tags?.length > 0 && (
        <span className="pn__tag-count" title={d.tags.join(', ')}>
          {d.tags.length}
        </span>
      )}
      <Handle type="source" position={Position.Bottom} className="pn__handle" />
    </div>
  );
}

// ── Custom Node: Group ─────────────────────────────────────────────────────

function GroupNode({ data }: NodeProps<Node<GroupNodeData>>) {
  return (
    <div className="gn">
      <div className="gn__label">{(data as GroupNodeData).label}</div>
    </div>
  );
}

// ── Custom straight edge with floating intersection ────────────────────────
// The `data.offsetPx` field shifts the path sideways for bidir pairs.
// The `data.edgeKind` drives color/dash.
// Instead of connecting to handle positions (Top/Bottom), this edge
// calculates where the center-to-center line intersects each node's
// rectangular boundary, so arrows appear at the border — not hidden
// behind the rounded-rectangle node.

interface StraightEdgeData {
  edgeKind: 'navigation' | 'upstream' | 'broken';
  offsetPx: number; // perpendicular offset in pixels (0 = no offset)
}

// How many pixels to pull the target endpoint back from the intersection
// so the arrow marker head sits neatly on the border instead of
// overlapping into the node.
const ARROW_PULLBACK = 5;

function StraightEdge({
  id,
  source, target,
  sourceX, sourceY,
  targetX, targetY,
  data,
  markerEnd,
}: EdgeProps) {
  const { edgeKind = 'navigation', offsetPx = 0 } = (data || {}) as unknown as StraightEdgeData;

  // ── Subscribe to source/target node positions for floating edge calc ───
  const sourceSelector = useCallback(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (s: any) => {
      const n = s.nodeLookup?.get(source);
      if (!n) return null;
      return {
        x: n.internals?.positionAbsolute?.x ?? 0,
        y: n.internals?.positionAbsolute?.y ?? 0,
        w: n.measured?.width ?? NODE_W,
        h: n.measured?.height ?? NODE_H,
      };
    },
    [source],
  );
  const targetSelector = useCallback(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (s: any) => {
      const n = s.nodeLookup?.get(target);
      if (!n) return null;
      return {
        x: n.internals?.positionAbsolute?.x ?? 0,
        y: n.internals?.positionAbsolute?.y ?? 0,
        w: n.measured?.width ?? NODE_W,
        h: n.measured?.height ?? NODE_H,
      };
    },
    [target],
  );

  const nodeEq = (a: { x: number; y: number; w: number; h: number } | null,
                  b: { x: number; y: number; w: number; h: number } | null) => {
    if (a === b) return true;
    if (!a || !b) return false;
    return a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
  };

  const srcData = useStore(sourceSelector, nodeEq);
  const tgtData = useStore(targetSelector, nodeEq);

  let sx = sourceX;
  let sy = sourceY;
  let tx = targetX;
  let ty = targetY;

  // ── Calculate border intersection points ───────────────────────────────
  if (srcData && tgtData) {
    const srcCenter = { x: srcData.x + srcData.w / 2, y: srcData.y + srcData.h / 2 };
    const tgtCenter = { x: tgtData.x + tgtData.w / 2, y: tgtData.y + tgtData.h / 2 };

    [sx, sy] = getRectIntersection(srcCenter, tgtCenter, srcData.w, srcData.h);
    [tx, ty] = getRectIntersection(tgtCenter, srcCenter, tgtData.w, tgtData.h);

    // Pull the target endpoint back so the arrow marker sits on the border
    const edgeDx = tx - sx;
    const edgeDy = ty - sy;
    const edgeLen = Math.sqrt(edgeDx * edgeDx + edgeDy * edgeDy) || 1;
    tx -= (edgeDx / edgeLen) * ARROW_PULLBACK;
    ty -= (edgeDy / edgeLen) * ARROW_PULLBACK;
  }

  // ── Apply perpendicular offset for bidirectional pairs ─────────────────
  if (offsetPx !== 0) {
    const [ox, oy] = perpOffset(sx, sy, tx, ty, offsetPx);
    sx += ox; sy += oy;
    tx += ox; ty += oy;
  }

  const [path] = getStraightPath({ sourceX: sx, sourceY: sy, targetX: tx, targetY: ty });

  let stroke = COLORS.edgeNav;
  let strokeDash = '';
  let opacity = 0.65;

  if (edgeKind === 'upstream') {
    stroke = COLORS.edgeUp;
    strokeDash = '5 3';
    opacity = 0.35;
  } else if (edgeKind === 'broken') {
    stroke = COLORS.edgeBroken;
    strokeDash = '6 3';
    opacity = 0.85;
  }

  return (
    <path
      id={id}
      d={path}
      fill="none"
      stroke={stroke}
      strokeWidth={1.5}
      strokeDasharray={strokeDash || undefined}
      opacity={opacity}
      markerEnd={markerEnd}
    />
  );
}

// ── Node / edge type maps ──────────────────────────────────────────────────

const nodeTypes = { passage: PassageNode, group: GroupNode };
const edgeTypes = { straight: StraightEdge };

// ── Dagre layout ───────────────────────────────────────────────────────────
// Returns a position map for all nodes passed in.

function dagreLayout(
  nodes: Node[],
  edges: Edge[],
  dir: 'TB' | 'LR' = 'TB',
): Map<string, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph({ multigraph: true });
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: dir,
    nodesep: 60,
    ranksep: 80,
    marginx: 60,
    marginy: 60,
    acyclicer: 'greedy',
    ranker: 'network-simplex',
  });

  for (const n of nodes) {
    g.setNode(n.id, { width: NODE_W + 20, height: NODE_H + 16 });
  }
  for (const e of edges) {
    // multigraph requires a name for each edge
    g.setEdge(e.source, e.target, {}, e.id);
  }

  dagre.layout(g);

  const out = new Map<string, { x: number; y: number }>();
  for (const n of nodes) {
    const p = g.node(n.id);
    if (p) out.set(n.id, { x: snap(p.x - NODE_W / 2), y: snap(p.y - NODE_H / 2) });
  }
  return out;
}

// ── Build React Flow elements from KnotGraphResponse ──────────────────────
//
// Twine 2 inspired layout:
//   ┌──────────────────┐   ┌─────────┐
//   │ Special Passages  │   │  Start  │──→ regular reachable flow ──→
//   │ (bounding box)    │   └─────────┘         (dagre, right/down)
//   └──────────────────┘       │
//        │                     ↓
//   ┌──────────────────┐   more reachable passages
//   │ Unreachable List │        ↓
//   │ (stacked)        │       ...
//   └──────────────────┘
//
// When all nodes have positions (user has manually positioned them),
// those positions are used as-is. The auto-layout only applies to
// nodes without saved positions.

function buildElements(data: KnotGraphResponse): { nodes: Node[]; edges: Edge[] } {
  const rawNodes = Array.isArray(data?.nodes) ? data.nodes : [];
  const rawEdges = Array.isArray(data?.edges) ? data.edges : [];

  // ── Groups ────────────────────────────────────────────────────────────
  const groupMembers = new Map<string, string[]>();
  for (const n of rawNodes) {
    if (n.group) {
      if (!groupMembers.has(n.group)) groupMembers.set(n.group, []);
      groupMembers.get(n.group)!.push(n.id);
    }
  }

  // ── Identify start & special sets ─────────────────────────────────────
  const startNode = rawNodes.find(n => n.is_start || n.id === 'Start' || n.label === 'Start');
  const startId = startNode?.id;
  const specialSet = new Set<string>(
    rawNodes
      .filter(n => (n.is_special || n.is_metadata) && n.id !== startId)
      .map(n => n.id),
  );
  const unreachableSet = new Set<string>(
    rawNodes.filter(n => n.is_unreachable).map(n => n.id),
  );

  // ── Detect bidirectional pairs ─────────────────────────────────────────
  const edgeSet = new Set<string>(rawEdges.map(e => `${e.source}→${e.target}`));
  const isBidir = (src: string, tgt: string) =>
    edgeSet.has(`${src}→${tgt}`) && edgeSet.has(`${tgt}→${src}`);
  const bidirFirst = new Set<string>();

  // ── Classify nodes into categories ────────────────────────────────────
  const childToGroup = new Map<string, string>();
  for (const [gid, members] of groupMembers) {
    for (const mid of members) childToGroup.set(mid, gid);
  }

  // Separate nodes into: start, special, unreachable, regular
  const startNodes: KnotGraphNode[] = [];
  const specialNodes: KnotGraphNode[] = [];
  const unreachableNodes: KnotGraphNode[] = [];
  const regularNodes: KnotGraphNode[] = [];

  for (const n of rawNodes) {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    if (isStart) {
      startNodes.push(n);
    } else if (specialSet.has(n.id)) {
      specialNodes.push(n);
    } else if (unreachableSet.has(n.id)) {
      unreachableNodes.push(n);
    } else {
      regularNodes.push(n);
    }
  }

  // ── Check if we need auto-layout ──────────────────────────────────────
  // If ANY node lacks a saved position, we apply the full Twine 2 auto-
  // layout. Otherwise, we respect the user's manually-saved positions.
  const allHavePositions = rawNodes.every(
    n => n.position_x != null && n.position_y != null,
  );

  // ── Helper: create a React Flow passage node ──────────────────────────
  function makePassageNode(n: KnotGraphNode, x: number, y: number): Node<PassageNodeData> {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    return {
      id: n.id,
      type: 'passage',
      position: { x: snap(x), y: snap(y) },
      data: {
        label: n.label,
        file: n.file,
        line: n.line,
        tags: n.tags || [],
        out_degree: n.out_degree || 0,
        in_degree: n.in_degree || 0,
        is_special: !!n.is_special,
        is_metadata: !!n.is_metadata,
        is_unreachable: !!n.is_unreachable,
        is_start: isStart,
        color: nodeColor(n),
        metadata_color: n.color,
        var_writes: n.var_writes || [],
        var_reads: n.var_reads || [],
        group: n.group,
        dimmed: false,
        highlighted: false,
        focused: false,
      },
      parentId: childToGroup.get(n.id),
    };
  }

  // ── Build passage nodes ───────────────────────────────────────────────
  const allPassageNodes: Node[] = [];

  if (allHavePositions) {
    // User has saved positions — use them directly
    for (const n of rawNodes) {
      const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
      allPassageNodes.push(makePassageNode(n, n.position_x!, n.position_y!));
    }
  } else {
    // ── Twine 2 auto-layout ─────────────────────────────────────────────

    // 1. Start passage: anchor at fixed position
    if (startNode) {
      allPassageNodes.push(makePassageNode(startNode, START_ANCHOR_X, START_ANCHOR_Y));
    }

    // 2. Special passages: arrange in a column inside a bounding box
    //    Sort: metadata (StoryData, StoryTitle) first, then other specials
    specialNodes.sort((a, b) => {
      if (a.is_metadata && !b.is_metadata) return -1;
      if (!a.is_metadata && b.is_metadata) return 1;
      return a.label.localeCompare(b.label);
    });
    for (let i = 0; i < specialNodes.length; i++) {
      allPassageNodes.push(makePassageNode(
        specialNodes[i],
        SPECIAL_BOX_X,
        SPECIAL_BOX_Y + i * (NODE_H + GRID_SNAP),
      ));
    }

    // 3. Unreachable passages: stacked vertically below the specials
    unreachableNodes.sort((a, b) => a.label.localeCompare(b.label));
    const unreachableStartY = specialNodes.length > 0
      ? SPECIAL_BOX_Y + specialNodes.length * (NODE_H + GRID_SNAP) + UNREACHABLE_LIST_Y_OFFSET
      : SPECIAL_BOX_Y;
    for (let i = 0; i < unreachableNodes.length; i++) {
      allPassageNodes.push(makePassageNode(
        unreachableNodes[i],
        UNREACHABLE_LIST_X,
        unreachableStartY + i * (NODE_H + GRID_SNAP),
      ));
    }

    // 4. Regular reachable passages: layout with dagre, flowing right and
    //    down from the start passage.
    if (regularNodes.length > 0) {
      const regularIds = new Set(regularNodes.map(n => n.id));
      const subEdges: Edge[] = rawEdges
        .filter(e => regularIds.has(e.source) && regularIds.has(e.target))
        .map((e, i) => ({ id: `dagre-${i}`, source: e.source, target: e.target }));

      // Also include edges from start → regular to anchor the flow
      if (startId) {
        for (const e of rawEdges) {
          if (e.source === startId && regularIds.has(e.target)) {
            subEdges.push({ id: `dagre-start-${e.target}`, source: startId, target: e.target });
          }
        }
      }

      // Create temporary dagre nodes for regular passages
      const dagreNodes: Node[] = regularNodes.map(n => makePassageNode(n, 0, 0));
      const positions = dagreLayout(dagreNodes, subEdges, 'LR'); // Left-to-Right flow

      // Apply dagre positions, offset so they flow right/down from start
      for (const n of dagreNodes) {
        const p = positions.get(n.id);
        if (p) {
          n.position = {
            x: snap(p.x + START_ANCHOR_X + NODE_W + GRID_SNAP * 3),
            y: snap(p.y + START_ANCHOR_Y),
          };
        }
      }
      allPassageNodes.push(...dagreNodes);
    }
  }

  // ── Build group container nodes ────────────────────────────────────────
  const groupNodes: Node[] = [];
  for (const [gid, members] of groupMembers) {
    const children = allPassageNodes.filter(n => members.includes(n.id));
    if (children.length === 0) continue;

    const pad = 32;
    const xs = children.map(n => n.position.x);
    const ys = children.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - 18; // room for label
    const maxX = Math.max(...xs) + NODE_W + pad;
    const maxY = Math.max(...ys) + NODE_H + pad;

    groupNodes.push({
      id: gid,
      type: 'group',
      position: { x: snap(minX), y: snap(minY) },
      data: { label: gid } as GroupNodeData,
      style: { width: snap(maxX - minX), height: snap(maxY - minY) },
      draggable: false,
      selectable: false,
      zIndex: -1,
    });
  }

  // Add special passage bounding box as a group node (Twine 2 style)
  const specialRfNodes = allPassageNodes.filter(
    n => n.type === 'passage' && specialSet.has(n.id),
  );
  if (specialRfNodes.length > 0 && !allHavePositions) {
    const pad = 28;
    const labelH = 22;
    const xs = specialRfNodes.map(n => n.position.x);
    const ys = specialRfNodes.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - labelH;
    const maxX = Math.max(...xs) + NODE_W + pad;
    const maxY = Math.max(...ys) + NODE_H + pad;

    groupNodes.push({
      id: '__specials__',
      type: 'group',
      position: { x: snap(minX), y: snap(minY) },
      data: { label: 'Special Passages' } as GroupNodeData,
      style: {
        width: snap(maxX - minX),
        height: snap(maxY - minY),
        background: 'rgba(230, 81, 0, 0.04)',
        borderColor: 'rgba(230, 81, 0, 0.25)',
      },
      draggable: false,
      selectable: false,
      zIndex: -2,
    });
  }

  // Add unreachable passage list bounding box
  const unreachableRfNodes = allPassageNodes.filter(
    n => n.type === 'passage' && unreachableSet.has(n.id),
  );
  if (unreachableRfNodes.length > 0 && !allHavePositions) {
    const pad = 28;
    const labelH = 22;
    const xs = unreachableRfNodes.map(n => n.position.x);
    const ys = unreachableRfNodes.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - labelH;
    const maxX = Math.max(...xs) + NODE_W + pad;
    const maxY = Math.max(...ys) + NODE_H + pad;

    groupNodes.push({
      id: '__unreachable__',
      type: 'group',
      position: { x: snap(minX), y: snap(minY) },
      data: { label: 'Unreachable' } as GroupNodeData,
      style: {
        width: snap(maxX - minX),
        height: snap(maxY - minY),
        background: 'rgba(66, 66, 66, 0.04)',
        borderColor: 'rgba(66, 66, 66, 0.25)',
      },
      draggable: false,
      selectable: false,
      zIndex: -2,
    });
  }

  const rfNodes: Node[] = [...groupNodes, ...allPassageNodes];

  // ── Build edges ────────────────────────────────────────────────────────
  const nodeIdSet = new Set(rawNodes.map(n => n.id));
  const rfEdges: Edge[] = [];
  const usedIds = new Set<string>();
  let idx = 0;

  for (const e of rawEdges) {
    if (!nodeIdSet.has(e.source) || !nodeIdSet.has(e.target)) continue;

    // Suppress noisy special↔special and special→start edges
    if (specialSet.has(e.source) && specialSet.has(e.target)) continue;
    if (specialSet.has(e.source) && e.target === startId) continue;

    let eid = `${e.source}→${e.target}`;
    if (usedIds.has(eid)) eid = `${eid}_${idx}`;
    usedIds.add(eid);
    idx++;

    const bidir = isBidir(e.source, e.target);
    let offsetPx = 0;
    if (bidir) {
      const pairKey = [e.source, e.target].sort().join('↔');
      if (!bidirFirst.has(pairKey)) {
        bidirFirst.add(pairKey);
        offsetPx = BIDIR_OFFSET;
      } else {
        offsetPx = -BIDIR_OFFSET;
      }
    }

    const edgeKind =
      e.edge_type === 'upstream' ? 'upstream' :
      e.edge_type === 'broken'   ? 'broken'   : 'navigation';

    const markerColor =
      edgeKind === 'broken'   ? COLORS.edgeBroken :
      edgeKind === 'upstream' ? COLORS.edgeUp     : COLORS.edgeNav;

    rfEdges.push({
      id: eid,
      source: e.source,
      target: e.target,
      type: 'straight',
      data: { edgeKind, offsetPx } as any,
      markerEnd: {
        type: MarkerType.Arrow,
        color: markerColor,
        width: 14,
        height: 14,
      },
      style: { opacity: edgeKind === 'upstream' ? 0.35 : edgeKind === 'broken' ? 0.85 : 0.65 },
    });
  }

  return { nodes: rfNodes, edges: rfEdges };
}

// ── Tooltip ────────────────────────────────────────────────────────────────

interface TooltipState {
  x: number;
  y: number;
  data: PassageNodeData;
}

function NodeTooltip({ tip }: { tip: TooltipState | null }) {
  if (!tip) return null;
  const d = tip.data;
  return (
    <div
      className="node-tooltip"
      style={{ left: tip.x + 14, top: tip.y - 8 }}
    >
      <div className="node-tooltip__name">{d.label}</div>
      {d.tags?.length > 0 && (
        <div className="node-tooltip__row">
          <span className="node-tooltip__key">tags</span>
          <span className="node-tooltip__val">{d.tags.join(', ')}</span>
        </div>
      )}
      <div className="node-tooltip__row">
        <span className="node-tooltip__key">in / out</span>
        <span className="node-tooltip__val">{d.in_degree} / {d.out_degree}</span>
      </div>
      {d.var_writes?.length > 0 && (
        <div className="node-tooltip__row">
          <span className="node-tooltip__key">writes</span>
          <span className="node-tooltip__val">{d.var_writes.slice(0, 5).join(', ')}{d.var_writes.length > 5 ? '…' : ''}</span>
        </div>
      )}
      {d.var_reads?.length > 0 && (
        <div className="node-tooltip__row">
          <span className="node-tooltip__key">reads</span>
          <span className="node-tooltip__val">{d.var_reads.slice(0, 5).join(', ')}{d.var_reads.length > 5 ? '…' : ''}</span>
        </div>
      )}
      {d.file && (
        <div className="node-tooltip__file">{d.file.split('/').pop()}</div>
      )}
    </div>
  );
}

// ── Inner StoryMap ─────────────────────────────────────────────────────────

interface StoryMapInnerProps {
  graphData: KnotGraphResponse | null;
  searchQuery: string;
  fitRequested: number;
  saveRequested: number;
  focusRequested: number;
  focusPassageName: string;
}

function StoryMapInner({
  graphData,
  searchQuery,
  fitRequested,
  saveRequested,
  focusRequested,
  focusPassageName,
}: StoryMapInnerProps) {
  const { fitView, setViewport, getViewport, getNode } = useReactFlow();

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);

  const initialFitDoneRef = useRef(false);
  const savedViewportRef = useRef<{ x: number; y: number; zoom: number } | null>(null);
  const skipViewportRestoreRef = useRef(false);
  // Keep a stable ref to nodes for use inside callbacks without stale closure
  const nodesRef = useRef<Node[]>([]);
  useEffect(() => { nodesRef.current = nodes; }, [nodes]);

  // ── Debounced message senders ──────────────────────────────────────────
  const debouncedPositionUpdate = useMemo(
    () => debounce((updates: KnotPositionUpdate[]) => {
      if (updates.length > 0) vscode.postMessage({ command: 'updatePositions', updates });
    }, 150),
    [],
  );

  const debouncedViewportUpdate = useMemo(
    () => debounce((x: number, y: number, zoom: number) => {
      vscode.postMessage({ command: 'updateViewport', x, y, zoom });
    }, 500),
    [],
  );

  // ── Build graph on data change ─────────────────────────────────────────
  useEffect(() => {
    if (!graphData) return;

    try {
      const vp = getViewport();
      savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
    } catch { /* not ready yet */ }

    const { nodes: newNodes, edges: newEdges } = buildElements(graphData);
    setNodes(newNodes);
    setEdges(newEdges);

    if (!initialFitDoneRef.current && newNodes.length > 0) {
      initialFitDoneRef.current = true;
      requestAnimationFrame(() => {
        fitView({ padding: 0.12, duration: 350 });
      });
    } else if (skipViewportRestoreRef.current) {
      skipViewportRestoreRef.current = false;
    } else if (savedViewportRef.current) {
      requestAnimationFrame(() => {
        setViewport(savedViewportRef.current!, { duration: 0 });
      });
    }
  }, [graphData, setNodes, setEdges, fitView, setViewport, getViewport]);

  // ── Search filter ──────────────────────────────────────────────────────
  useEffect(() => {
    const q = searchQuery.toLowerCase().trim();

    // Compute matching IDs first (using ref — no stale closure)
    const matchIds = new Set<string>();
    if (q !== '') {
      for (const n of nodesRef.current) {
        if (n.type !== 'passage') continue;
        const d = n.data as PassageNodeData;
        if (
          (d.label || '').toLowerCase().includes(q) ||
          (d.tags || []).some(t => t.toLowerCase().includes(q))
        ) {
          matchIds.add(n.id);
        }
      }
    }

    setNodes(nds => nds.map(n => {
      if (n.type !== 'passage') return n;
      const d = n.data as PassageNodeData;
      if (q === '') return { ...n, data: { ...d, dimmed: false, highlighted: false } };
      const matches = matchIds.has(n.id);
      return { ...n, data: { ...d, dimmed: !matches, highlighted: matches } };
    }));

    setEdges(eds => eds.map(e => {
      if (q === '') return { ...e, style: undefined };
      const connected = matchIds.has(e.source) || matchIds.has(e.target);
      return { ...e, style: { ...e.style, opacity: connected ? undefined : 0.06 } };
    }));
  }, [searchQuery, setNodes, setEdges]);

  // ── Fit view ───────────────────────────────────────────────────────────
  useEffect(() => {
    if (fitRequested > 0) fitView({ padding: 0.12, duration: 280 });
  }, [fitRequested, fitView]);

  // ── Focus a passage ────────────────────────────────────────────────────
  // Pan-only focus: preserve the user's current zoom level so navigating
  // to a passage feels like jumping across the map, not zooming in/out.
  // Only bump the zoom up if it's so low the node would be invisible.
  useEffect(() => {
    if (focusRequested <= 0 || !focusPassageName) return;
    // Don't let the viewport-restore effect fight our navigation
    skipViewportRestoreRef.current = true;

    // Find node by id, then by label
    const nds = nodesRef.current;
    let target = getNode(focusPassageName);
    if (!target) {
      const found = nds.find(n => n.type === 'passage' && (n.data as PassageNodeData).label === focusPassageName);
      if (found) target = getNode(found.id);
    }
    if (!target) return;

    // Pan-only: lock zoom to the current level so fitView only pans.
    // If the user is zoomed out very far, nudge up to a readable level.
    const currentZoom = getViewport().zoom;
    const focusZoom = Math.max(currentZoom, 0.5);
    fitView({
      nodes: [{ id: target.id }],
      padding: 0.4,
      duration: 0,
      minZoom: focusZoom,
      maxZoom: focusZoom,
    });

    // Brief highlight to show which node was focused (fades after 1s)
    setNodes(nds => nds.map(n => {
      if (n.type !== 'passage') return n;
      const d = n.data as PassageNodeData;
      return { ...n, data: { ...d, focused: n.id === target!.id } };
    }));

    setTimeout(() => {
      setNodes(nds => nds.map(n => {
        if (n.type !== 'passage') return n;
        const d = n.data as PassageNodeData;
        return { ...n, data: { ...d, focused: false } };
      }));
      // After focus highlight fades, stop skipping viewport restore
      // so subsequent graph refreshes preserve the user's viewport
      skipViewportRestoreRef.current = false;
    }, 1000);
  }, [focusRequested, focusPassageName, fitView, getViewport, getNode, setNodes]);

  // ── Save all positions ─────────────────────────────────────────────────
  // After saving, the file watcher will trigger a refreshGraph, which
  // sends new data. Since all nodes now have positions in the source,
  // the rebuild will use the saved positions. We mark the viewport to
  // be preserved so the user doesn't see a jarring reset.
  useEffect(() => {
    if (saveRequested <= 0) return;
    const updates: KnotPositionUpdate[] = nodesRef.current
      .filter(n => n.type === 'passage')
      .map(n => {
        const d = n.data as PassageNodeData;
        return {
          passage_name: n.id,
          position_x: snap(n.position.x),
          position_y: snap(n.position.y),
          group: d.group,
          color: d.metadata_color,
        };
      });
    if (updates.length > 0) {
      // Save current viewport so the next graph rebuild preserves it
      try {
        const vp = getViewport();
        savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
      } catch { /* not ready */ }
      vscode.postMessage({ command: 'saveAllPositions', updates });
    }
  }, [saveRequested, getViewport]);

  // ── Node drag end → snap + send position ──────────────────────────────
  const handleNodesChange = useCallback((changes: NodeChange[]) => {
    onNodesChange(changes);

    const dragEnds = changes.filter(
      (c): c is NodePositionChange => c.type === 'position' && c.dragging === false && !!c.position,
    );
    if (dragEnds.length === 0) return;

    const snapped = new Map(dragEnds.map(c => [c.id, { x: snap(c.position!.x), y: snap(c.position!.y) }]));
    const updates: KnotPositionUpdate[] = [];

    setNodes(nds => nds.map(n => {
      const s = snapped.get(n.id);
      if (!s) return n;
      const d = n.data as PassageNodeData;
      updates.push({ passage_name: n.id, position_x: s.x, position_y: s.y, group: d.group, color: d.metadata_color });
      return { ...n, position: s };
    }));

    debouncedPositionUpdate(updates);
  }, [onNodesChange, setNodes, debouncedPositionUpdate]);

  // ── Node click → open passage ──────────────────────────────────────────
  const handleNodeClick = useCallback((_e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.file) vscode.postMessage({ command: 'openPassage', file: d.file, line: d.line || 0 });
  }, []);

  // ── Tooltip ────────────────────────────────────────────────────────────
  const handleNodeMouseEnter = useCallback((e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    setTooltip({ x: e.clientX, y: e.clientY, data: node.data as PassageNodeData });
  }, []);

  const handleNodeMouseLeave = useCallback(() => {
    setTooltip(null);
  }, []);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (tooltip) setTooltip(t => t ? { ...t, x: e.clientX, y: e.clientY } : null);
  }, [tooltip]);

  // ── Viewport change ────────────────────────────────────────────────────
  const handleViewportChange = useCallback(() => {
    try {
      const vp = getViewport();
      debouncedViewportUpdate(vp.x, vp.y, vp.zoom);
    } catch { /* not ready */ }
  }, [getViewport, debouncedViewportUpdate]);

  // ── MiniMap color ──────────────────────────────────────────────────────
  const miniMapColor = useCallback((n: Node) => {
    if (n.type !== 'passage') return '#2a2a3a';
    return (n.data as PassageNodeData).color || COLORS.normal;
  }, []);

  return (
    <div className="storymap-inner" onMouseMove={handleMouseMove}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={handleNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeClick={handleNodeClick}
        onNodeMouseEnter={handleNodeMouseEnter}
        onNodeMouseLeave={handleNodeMouseLeave}
        onViewportChange={handleViewportChange}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        connectionMode={ConnectionMode.Loose}
        minZoom={MIN_ZOOM}
        maxZoom={MAX_ZOOM}
        fitView={false}
        panOnDrag
        selectionOnDrag={false}
        nodesDraggable
        nodesConnectable={false}
        elementsSelectable
        proOptions={{ hideAttribution: true }}
        defaultEdgeOptions={{ type: 'straight' }}
      >
        <MiniMap
          nodeColor={miniMapColor}
          maskColor="rgba(0,0,0,0.55)"
          style={{
            background: 'var(--vscode-sideBar-background, #1e1e2e)',
            border: '1px solid #3a3a4a',
            borderRadius: 4,
          }}
        />
        <Controls showInteractive={false} />
        <Background
          variant={BackgroundVariant.Lines}
          gap={GRID_SNAP}
          size={1}
          color="rgba(255,255,255,0.06)"
        />
      </ReactFlow>
      <NodeTooltip tip={tooltip} />
    </div>
  );
}

// ── Outer component (provides ReactFlowProvider) ───────────────────────────

interface StoryMapProps {
  graphData: KnotGraphResponse | null;
  searchQuery: string;
  fitRequested: number;
  saveRequested: number;
  focusRequested: number;
  focusPassageName: string;
}

export default function StoryMap(props: StoryMapProps) {
  return (
    <div className="storymap-container">
      <ReactFlowProvider>
        <StoryMapInner {...props} />
      </ReactFlowProvider>
    </div>
  );
}