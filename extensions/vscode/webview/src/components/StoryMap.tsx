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
  useStore,
  type InternalNode,
  type ReactFlowState,
  type OnSelectionChangeParams,
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
import ContextMenu, { ContextMenuState } from './ContextMenu';

// ── Constants ──────────────────────────────────────────────────────────────

const GRID_SNAP = 20;
// Compact single-row Twine-style chip card. The body-text preview was
// removed: a single click now navigates the code editor straight to the
// passage, so the raw text is always one click away — no need to duplicate
// it on the canvas (saves vertical real estate in the VS Code layout).
const NODE_W = 160;
const NODE_H = 36;
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 4.0;
const BIDIR_OFFSET = 7;

// ── Twine 2 layout anchors ────────────────────────────────────────────────

const START_ANCHOR_X = 420;
const START_ANCHOR_Y = 60;
const SPECIAL_BOX_X = 40;
const SPECIAL_BOX_Y = 60;
const UNREACHABLE_LIST_X = 40;
const UNREACHABLE_LIST_Y_OFFSET = 60;

// ── Color palette ──────────────────────────────────────────────────────────
//
// Modernized Twine palette: the passage families keep distinct hues so the
// map reads at a glance, while every edge kind stays legible on the dark
// canvas at low opacity.

const COLORS = {
  normal:     '#3a7ca5',   // Twine steel blue
  start:      '#2f9e6e',   // emerald
  special:    '#d97b3f',   // warm amber-orange
  metadata:   '#8e5bc0',   // violet
  unreachable:'#a87534',   // desaturated amber
  broken:     '#f87171',
  edgeNav:    '#8494ab',   // [[Target]] — player-choice navigation (slate)
  edgeJump:   '#a78bfa',   // <<goto>> — unconditional redirect (violet)
  edgeCall:   '#34d399',   // <<widget>> — pushdown call-return (mint)
  edgeInclude:'#60a5fa',   // <<include>> — inline inclusion (sky)
  edgeUp:     '#64748b',   // Special passage lifecycle chain (slate dark)
  edgeBroken: '#f87171',   // Target passage doesn't exist
  edgeManual: '#f0b429',   // `reachable` metadata — manual reachability link (gold)
};

// ── Helpers ────────────────────────────────────────────────────────────────

function snap(v: number) { return Math.round(v / GRID_SNAP) * GRID_SNAP; }

function nodeColor(n: KnotGraphNode): string {
  if (n.color) return n.color;
  if (n.is_unreachable) return COLORS.unreachable;
  if (n.is_start) return COLORS.start;
  if (n.is_metadata) return COLORS.metadata;
  if (n.is_special) return COLORS.special;
  return COLORS.normal;
}

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let t: ReturnType<typeof setTimeout> | null = null;
  return ((...args: Parameters<T>) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  }) as T;
}

// ── Perpendicular offset helper ────────────────────────────────────────────

function perpOffset(
  x1: number, y1: number, x2: number, y2: number, dist: number,
): [number, number] {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len = Math.sqrt(dx * dx + dy * dy) || 1;
  return [(-dy / len) * dist, (dx / len) * dist];
}

// ── Border intersection for floating edges ─────────────────────────────────
//
// Nodes render with rounded corners (CSS --node-r), so anchoring edges on
// the raw rectangle leaves arrow tips floating OUTSIDE the visible shape
// whenever the center-to-center ray exits near a corner. This computes the
// exit point on the actual rounded rectangle: intersect the plain rect
// first, and when the hit lands inside a corner zone, intersect the ray
// with that corner's arc circle instead (taking the far root, since the
// ray origin — the node center — is always inside).

// Must match --node-r in storymap.css.
const NODE_R = 8;

function getBorderPoint(
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

  const dxDominant = absDx * halfH > absDy * halfW;
  const scale = dxDominant ? halfW / absDx : halfH / absDy;
  const px = center.x + dx * scale;
  const py = center.y + dy * scale;

  // Corner-arc center (the rect inset by NODE_R on both axes), when the
  // plain-rect hit falls inside a corner zone.
  let ax: number | null = null;
  let ay = 0;
  if (dxDominant) {
    // Hit the left/right side: corner zone when close to top/bottom.
    if (Math.abs(py - center.y) > halfH - NODE_R) {
      ax = center.x + Math.sign(dx) * (halfW - NODE_R);
      ay = center.y + Math.sign(py - center.y) * (halfH - NODE_R);
    }
  } else {
    // Hit the top/bottom side: corner zone when close to left/right.
    if (Math.abs(px - center.x) > halfW - NODE_R) {
      ax = center.x + Math.sign(px - center.x) * (halfW - NODE_R);
      ay = center.y + Math.sign(dy) * (halfH - NODE_R);
    }
  }

  if (ax !== null) {
    // Ray/circle exit intersection: |c + t·d − a|² = r², larger root.
    const fx = ax - center.x;
    const fy = ay - center.y;
    const dd = dx * dx + dy * dy;
    const fd = fx * dx + fy * dy;
    const disc = fd * fd - dd * (fx * fx + fy * fy - NODE_R * NODE_R);
    if (disc >= 0) {
      const t = (fd + Math.sqrt(disc)) / dd;
      return [center.x + dx * t, center.y + dy * t];
    }
  }

  return [px, py];
}

// ── Custom Node: Passage ───────────────────────────────────────────────────
//
// Compact single-row Twine-style chip: the whole card carries the passage
// color (with a subtle top-light gradient), the name, and status badges.
// The body-text preview was removed — clicking a passage navigates the code
// editor to it, so the full raw text is always exactly one click away.

function PassageNode({ data }: NodeProps<Node<PassageNodeData>>) {
  const d = data as PassageNodeData;
  const color = d.color || COLORS.normal;

  const cls = [
    'pn',
    d.is_start       && 'pn--start',
    d.is_special     && 'pn--special',
    d.is_metadata    && 'pn--metadata',
    d.is_unreachable && 'pn--unreachable',
    d.is_entry       && 'pn--entry',
    d.is_dead_end    && 'pn--dead-end',
    d.highlighted    && 'pn--highlighted',
    d.dimmed         && 'pn--dimmed',
    d.focused        && 'pn--focused',
  ].filter(Boolean).join(' ');

  // Apply custom size from passage metadata (Twine size convention).
  // Fall back to the CSS defaults (--node-w / --node-h) when not set.
  const style: React.CSSProperties = { '--node-color': color } as React.CSSProperties;
  if (d.size_w != null) {
    style.width = `${d.size_w}px`;
    style.minWidth = `${d.size_w}px`;
  }
  if (d.size_h != null) {
    style.height = `${d.size_h}px`;
    style.minHeight = `${d.size_h}px`;
  }

  return (
    <div className={cls} style={style}>
      <Handle type="target" position={Position.Top} className="pn__handle" />
      <span className="pn__label" title={d.label}>{d.label}</span>
      <span className="pn__badges">
        {d.is_start && (
          <span className="pn__badge pn__badge--start" title="Start passage">
            <svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true">
              <path d="M1.5 1 L8.5 5 L1.5 9 Z" fill="currentColor" />
            </svg>
          </span>
        )}
        {d.is_entry && (
          <span
            className="pn__badge pn__badge--entry"
            title={`Manual link from "${d.manual_link_source ?? '?'}" (reachable metadata)`}
          >
            <svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true">
              <path
                d="M4.5 7.5 L7.5 4.5 M3.5 5.5 L2.2 6.8 a1.7 1.7 0 0 0 2.4 2.4 L5.8 8 M6.2 4 L7.4 2.8 a1.7 1.7 0 0 1 2.4 2.4 L8.5 6.5"
                fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round"
              />
            </svg>
          </span>
        )}
        {d.is_dead_end && (
          <span className="pn__badge pn__badge--dead-end" title="Dead end — no outgoing links">
            <svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true">
              <path
                d="M6 1.5 L11 10 L1 10 Z"
                fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round"
              />
              <path d="M6 4.6 L6 7.2 M6 8.6 L6 8.7" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
            </svg>
          </span>
        )}
        {d.tags?.length > 0 && (
          <span className="pn__badge pn__badge--tags" title={d.tags.join(', ')}>
            {d.tags.length}
          </span>
        )}
      </span>
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

// ── React Flow store selector types ────────────────────────────────────────

/** Shape extracted from a React Flow InternalNode for edge geometry. */
interface NodeGeometry {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Build a store selector that reads a node's absolute position + measured size. */
function makeNodeGeometrySelector(
  nodeId: string,
  store: ReactFlowState,
): NodeGeometry | null {
  const n = store.nodeLookup?.get(nodeId) as InternalNode | undefined;
  if (!n) return null;
  return {
    x: n.internals?.positionAbsolute?.x ?? 0,
    y: n.internals?.positionAbsolute?.y ?? 0,
    w: n.measured?.width ?? NODE_W,
    h: n.measured?.height ?? NODE_H,
  };
}

// ── Custom straight edge with floating intersection ────────────────────────

interface StraightEdgeData {
  edgeKind: 'navigation' | 'upstream' | 'broken' | 'jump' | 'call' | 'include' | 'manual';
  offsetPx: number;
  displayText?: string;
}

// Open-arrowhead geometry. The TIP sits exactly on the target node's
// border; the main line stops at the arrow BASE so the two strokes never
// stack opacity. Both grow slightly on hover.
const ARROW_LEN = 7;
const ARROW_HALF = 3.75;
const ARROW_LEN_HOVER = 9;
const ARROW_HALF_HOVER = 4.75;

function StraightEdge({
  id,
  source, target,
  sourceX, sourceY,
  targetX, targetY,
  data,
  style: edgeStyle,
}: EdgeProps) {
  const { edgeKind = 'navigation', offsetPx = 0, displayText } = (data ?? {}) as unknown as StraightEdgeData;
  const [hovered, setHovered] = useState(false);

  const sourceSelector = useCallback(
    (s: ReactFlowState) => makeNodeGeometrySelector(source, s),
    [source],
  );
  const targetSelector = useCallback(
    (s: ReactFlowState) => makeNodeGeometrySelector(target, s),
    [target],
  );

  const nodeEq = (
    a: NodeGeometry | null,
    b: NodeGeometry | null,
  ) => {
    if (a === b) return true;
    if (!a || !b) return false;
    return a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
  };

  const srcData = useStore(sourceSelector, nodeEq);
  const tgtData = useStore(targetSelector, nodeEq);

  const strokeWidth = hovered ? 2.25 : 1.4;
  const arrowLen = hovered ? ARROW_LEN_HOVER : ARROW_LEN;
  const arrowHalf = hovered ? ARROW_HALF_HOVER : ARROW_HALF;

  // ── Geometry ──────────────────────────────────────────────────────────
  // tip    — arrowhead tip; always anchored ON the target node's border.
  // tipDir — unit vector the arrowhead points along (travel direction).
  // path   — the visible main line (stops at the arrowhead base).
  // start  — where the visible line begins (used for the label midpoint).
  let tip = { x: targetX, y: targetY };
  let tipDir = { x: 1, y: 0 };
  let start = { x: sourceX, y: sourceY };
  let path: string;

  if (srcData && tgtData && source === target) {
    // ── Self-loop: bulge out from the node's right side and curve back ──
    // (React Flow straight edges degenerate to a zero-length path for
    // source === target, which rendered as nothing at all.)
    const right = srcData.x + srcData.w;
    const cY = srcData.y + srcData.h / 2;
    const yA = cY - Math.min(10, srcData.h * 0.3);
    const yB = cY + Math.min(10, srcData.h * 0.3);
    const bulge = 26 + Math.min(24, srcData.w * 0.2);
    tip = { x: right, y: yB };
    const tLen = Math.hypot(bulge, 18) || 1;
    tipDir = { x: -bulge / tLen, y: -18 / tLen };
    start = { x: right, y: yA };
    // End the curve at the arrowhead base, pulled back along its tangent.
    const ex = right - tipDir.x * arrowLen;
    const ey = yB - tipDir.y * arrowLen;
    path = `M ${right} ${yA} C ${right + bulge} ${yA - 18}, ${right + bulge} ${yB + 18}, ${ex} ${ey}`;
  } else if (srcData && tgtData) {
    // ── Floating straight edge anchored on both node borders ───────────
    let sc = { x: srcData.x + srcData.w / 2, y: srcData.y + srcData.h / 2 };
    let tc = { x: tgtData.x + tgtData.w / 2, y: tgtData.y + tgtData.h / 2 };

    // Bidirectional pairs: translate the whole center line perpendicular
    // instead of shifting the endpoints. The shifted line still crosses
    // both node rectangles, so both anchors stay exactly on the border —
    // the old endpoint-shift made arrows float off the rounded corners.
    if (offsetPx !== 0) {
      const [ox, oy] = perpOffset(sc.x, sc.y, tc.x, tc.y, offsetPx);
      sc = { x: sc.x + ox, y: sc.y + oy };
      tc = { x: tc.x + ox, y: tc.y + oy };
    }

    const [bx, by] = getBorderPoint(sc, tc, srcData.w, srcData.h);
    const [tx, ty] = getBorderPoint(tc, sc, tgtData.w, tgtData.h);
    start = { x: bx, y: by };
    tip = { x: tx, y: ty };
    const dx = tx - bx;
    const dy = ty - by;
    const len = Math.hypot(dx, dy) || 1;
    tipDir = { x: dx / len, y: dy / len };

    // Main line runs border → arrowhead base. For very short edges the
    // base gap would look broken, so run the line all the way to the tip.
    const lineLen = len > arrowLen * 2 ? len - arrowLen : len;
    const ex = bx + tipDir.x * lineLen;
    const ey = by + tipDir.y * lineLen;
    [path] = getStraightPath({ sourceX: bx, sourceY: by, targetX: ex, targetY: ey });
  } else {
    // Fallback before node measurement: handle-derived coordinates.
    const dx = targetX - sourceX;
    const dy = targetY - sourceY;
    const len = Math.hypot(dx, dy) || 1;
    tipDir = { x: dx / len, y: dy / len };
    const lineLen = len > arrowLen * 2 ? len - arrowLen : len;
    const ex = sourceX + tipDir.x * lineLen;
    const ey = sourceY + tipDir.y * lineLen;
    [path] = getStraightPath({ sourceX, sourceY, targetX: ex, targetY: ey });
  }

  // ── Stroke styling ────────────────────────────────────────────────────
  let stroke = COLORS.edgeNav;
  let strokeDash = '';
  let opacity = 0.55;

  if (edgeKind === 'upstream') {
    stroke = COLORS.edgeUp;
    strokeDash = '5 3';
    opacity = 0.32;
  } else if (edgeKind === 'broken') {
    stroke = COLORS.edgeBroken;
    strokeDash = '6 3';
    opacity = 0.8;
  } else if (edgeKind === 'jump') {
    stroke = COLORS.edgeJump;
    strokeDash = '8 4';
    opacity = 0.62;
  } else if (edgeKind === 'call') {
    stroke = COLORS.edgeCall;
    strokeDash = '3 2';
    opacity = 0.55;
  } else if (edgeKind === 'include') {
    stroke = COLORS.edgeInclude;
    strokeDash = '4 2 1 2';
    opacity = 0.55;
  } else if (edgeKind === 'manual') {
    // Manual `reachable` metadata link: dotted golden line — visually
    // distinct from every link the parser extracted from body text,
    // because these are author *declarations*, not story markup.
    stroke = COLORS.edgeManual;
    strokeDash = '1.5 3.5';
    opacity = 0.85;
  }

  // Search/tag-filter dimming: the filter writes `style.opacity` onto the
  // edge object, but React Flow only auto-applies `style` for its built-in
  // edge components — a custom edge must apply it itself (this dimming
  // previously had no effect at all).
  const styleOpacity = (edgeStyle as React.CSSProperties | undefined)?.opacity;
  const dimmed = typeof styleOpacity === 'number';

  // Boost opacity on hover (not on dimmed/disconnected edges).
  if (hovered && !dimmed) opacity = Math.min(opacity + 0.25, 1.0);
  if (dimmed) opacity = Math.min(opacity, styleOpacity as number);

  // ── Arrowhead: open V, tip exactly on the node border ─────────────────
  const baseX = tip.x - tipDir.x * arrowLen;
  const baseY = tip.y - tipDir.y * arrowLen;
  const nX = -tipDir.y * arrowHalf;
  const nY = tipDir.x * arrowHalf;
  const arrowPath = `M ${baseX + nX} ${baseY + nY} L ${tip.x} ${tip.y} L ${baseX - nX} ${baseY - nY}`;

  // Label midpoint (self-loops label near the loop's apex).
  const midX = source === target && srcData
    ? srcData.x + srcData.w + (26 + Math.min(24, srcData.w * 0.2)) * 0.75
    : (start.x + tip.x) / 2;
  const midY = source === target && srcData
    ? srcData.y + srcData.h / 2
    : (start.y + tip.y) / 2;

  return (
    <>
      <path
        id={id}
        d={path}
        fill="none"
        stroke={stroke}
        strokeWidth={strokeWidth}
        strokeDasharray={strokeDash || undefined}
        strokeLinecap={edgeKind === 'manual' ? 'round' : undefined}
        opacity={opacity}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{ cursor: 'pointer', pointerEvents: 'stroke' }}
      />
      {/* Arrowhead — custom-drawn so the tip lands exactly on the node
          border (React Flow markers anchor the tip at the path end and
          scale with strokeWidth, which left a floating gap). */}
      <path
        d={arrowPath}
        fill="none"
        stroke={stroke}
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        opacity={opacity}
        style={{ pointerEvents: 'none' }}
      />
      {/* Invisible wider hit area for easier hovering */}
      <path
        d={path}
        fill="none"
        stroke="transparent"
        strokeWidth={12}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{ pointerEvents: 'stroke' }}
      />
      {/* Label on hover */}
      {hovered && (displayText || edgeKind === 'manual') && (
        <text
          x={midX}
          y={midY - 6}
          textAnchor="middle"
          className="edge-label"
          style={{
            fill: edgeKind === 'manual' ? COLORS.edgeManual : 'var(--fg, #c8ccd4)',
            fontSize: '10px',
            pointerEvents: 'none',
            userSelect: 'none',
          }}
        >
          {displayText
            ? (displayText.length > 30 ? displayText.slice(0, 28) + '…' : displayText)
            : 'manual link'}
        </text>
      )}
    </>
  );
}

// ── Node / edge type maps ──────────────────────────────────────────────────

const nodeTypes = { passage: PassageNode, group: GroupNode };
const edgeTypes = { straight: StraightEdge };

// ── Dagre layout ───────────────────────────────────────────────────────────

function dagreLayout(
  nodes: Node[],
  edges: Edge[],
  dir: 'TB' | 'LR' = 'TB',
): Map<string, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph({ multigraph: true });
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: dir,
    nodesep: 34,
    ranksep: 48,
    marginx: 50,
    marginy: 50,
    acyclicer: 'greedy',
    ranker: 'network-simplex',
  });

  for (const n of nodes) {
    g.setNode(n.id, { width: NODE_W + 20, height: NODE_H + 14 });
  }
  for (const e of edges) {
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

function buildElements(data: KnotGraphResponse): { nodes: Node[]; edges: Edge[] } {
  const rawNodes = Array.isArray(data?.nodes) ? data.nodes : [];
  const rawEdges = Array.isArray(data?.edges) ? data.edges : [];

  const groupMembers = new Map<string, string[]>();
  for (const n of rawNodes) {
    if (n.group) {
      if (!groupMembers.has(n.group)) groupMembers.set(n.group, []);
      groupMembers.get(n.group)!.push(n.id);
    }
  }

  const startNode = rawNodes.find(n => n.is_start);
  const startId = startNode?.id;
  const specialSet = new Set<string>(
    rawNodes
      .filter(n => (n.is_special || n.is_metadata) && n.id !== startId)
      .map(n => n.id),
  );
  const unreachableSet = new Set<string>(
    rawNodes.filter(n => n.is_unreachable).map(n => n.id),
  );

  const edgeSet = new Set<string>(rawEdges.map(e => `${e.source}→${e.target}`));
  const isBidir = (src: string, tgt: string) =>
    edgeSet.has(`${src}→${tgt}`) && edgeSet.has(`${tgt}→${src}`);
  const bidirFirst = new Set<string>();

  const childToGroup = new Map<string, string>();
  for (const [gid, members] of groupMembers) {
    for (const mid of members) childToGroup.set(mid, gid);
  }

  const specialNodes: KnotGraphNode[] = [];
  const unreachableNodes: KnotGraphNode[] = [];
  const regularNodes: KnotGraphNode[] = [];

  for (const n of rawNodes) {
    if (n.is_start)                { /* startNode is already captured above; skip */ }
    else if (specialSet.has(n.id)) specialNodes.push(n);
    else if (unreachableSet.has(n.id)) unreachableNodes.push(n);
    else                           regularNodes.push(n);
  }

  const allHavePositions = rawNodes.every(
    n => n.position_x != null && n.position_y != null,
  );

  function makePassageNode(n: KnotGraphNode, x: number, y: number): Node<PassageNodeData> {
    const groupId = childToGroup.get(n.id);
    const isDeadEnd =
      (n.out_degree || 0) === 0 &&
      !n.is_start && !n.is_special && !n.is_metadata && !n.is_unreachable;
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
        is_start: !!n.is_start,
        is_entry: !!n.is_entry && !n.is_start,
        manual_link_source: n.manual_link_source,
        is_dead_end: isDeadEnd,
        color: nodeColor(n),
        metadata_color: n.color,
        size_w: n.size_w,
        size_h: n.size_h,
        var_writes: n.var_writes || [],
        var_reads: n.var_reads || [],
        excerpt: n.excerpt || '',
        group: groupId,
        dimmed: false,
        highlighted: false,
        focused: false,
      },
    };
  }

  const allPassageNodes: Node[] = [];

  if (allHavePositions) {
    for (const n of rawNodes) {
      allPassageNodes.push(makePassageNode(n, n.position_x!, n.position_y!));
    }
  } else {
    if (startNode) {
      allPassageNodes.push(makePassageNode(startNode, START_ANCHOR_X, START_ANCHOR_Y));
    }

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

    if (regularNodes.length > 0) {
      const regularIds = new Set(regularNodes.map(n => n.id));
      const subEdges: Edge[] = rawEdges
        .filter(e => regularIds.has(e.source) && regularIds.has(e.target))
        .map((e, i) => ({ id: `dagre-${i}`, source: e.source, target: e.target }));

      if (startId) {
        for (const e of rawEdges) {
          if (e.source === startId && regularIds.has(e.target)) {
            subEdges.push({ id: `dagre-start-${e.target}`, source: startId, target: e.target });
          }
        }
      }

      const dagreNodes: Node[] = regularNodes.map(n => makePassageNode(n, 0, 0));
      const positions = dagreLayout(dagreNodes, subEdges, 'LR');

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

  // ── Group container nodes (visual bounding boxes only) ─────────────────
  // Groups use zIndex: -1 and are not interactive. Passage nodes are NOT
  // assigned parentId, so their coordinates stay in the global frame.
  const groupNodes: Node[] = [];

  for (const [gid, members] of groupMembers) {
    const children = allPassageNodes.filter(n => members.includes(n.id));
    if (children.length === 0) continue;

    const pad = 26;
    const xs = children.map(n => n.position.x);
    const ys = children.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - 16;
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

  const specialRfNodes = allPassageNodes.filter(
    n => n.type === 'passage' && specialSet.has(n.id),
  );
  if (specialRfNodes.length > 0 && !allHavePositions) {
    const pad = 24;
    const labelH = 20;
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
        background: 'rgba(217, 123, 63, 0.045)',
        borderColor: 'rgba(217, 123, 63, 0.22)',
      },
      draggable: false,
      selectable: false,
      zIndex: -2,
    });
  }

  const unreachableRfNodes = allPassageNodes.filter(
    n => n.type === 'passage' && unreachableSet.has(n.id),
  );
  if (unreachableRfNodes.length > 0 && !allHavePositions) {
    const pad = 24;
    const labelH = 20;
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
        background: 'rgba(168, 117, 52, 0.045)',
        borderColor: 'rgba(168, 117, 52, 0.22)',
      },
      draggable: false,
      selectable: false,
      zIndex: -2,
    });
  }

  const rfNodes: Node[] = [...groupNodes, ...allPassageNodes];

  // ── Edges ──────────────────────────────────────────────────────────────

  const nodeIdSet = new Set(rawNodes.map(n => n.id));
  const rfEdges: Edge[] = [];
  const usedIds = new Set<string>();
  let idx = 0;

  for (const e of rawEdges) {
    if (!nodeIdSet.has(e.source) || !nodeIdSet.has(e.target)) continue;
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
      e.edge_type === 'broken'   ? 'broken'   :
      e.edge_type === 'jump'     ? 'jump'     :
      e.edge_type === 'call'     ? 'call'     :
      e.edge_type === 'include'  ? 'include'  :
      e.edge_type === 'manual'   ? 'manual'   : 'navigation';

    // Include edges represent content flowing FROM the included passage INTO
    // the including passage. In Twine's convention, the arrow points backward
    // (from the included passage to the includer) to show "content is pulled
    // in here." We achieve this by swapping source/target so the arrow
    // naturally points from the included passage back to the includer.
    const isInclude = edgeKind === 'include';
    const edgeSource = isInclude ? e.target : e.source;
    const edgeTarget = isInclude ? e.source : e.target;

    // NOTE: no `markerEnd` here — the custom StraightEdge draws its own
    // arrowhead so the tip lands exactly on the target node's border
    // (React Flow markers anchor the tip at the path end and scale with
    // strokeWidth). `style` is likewise left empty: per-kind opacity lives
    // in the edge component, and the search filter uses `style.opacity`
    // for dimming.
    rfEdges.push({
      id: eid,
      source: edgeSource,
      target: edgeTarget,
      type: 'straight',
      data: { edgeKind, offsetPx, displayText: e.display_text } as unknown as Record<string, unknown>,
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

  // Build a simple type label for the tooltip
  const typeLabel = d.is_start ? 'Start passage' :
    d.is_metadata ? 'Metadata' :
    d.is_special ? 'Special' :
    d.is_entry ? 'Manual entry point' :
    d.is_unreachable ? 'Unreachable' :
    d.is_dead_end ? 'Dead end' :
    'Passage';

  // One-line excerpt (the card itself no longer shows a body preview —
  // clicking navigates the code editor to the full text).
  const excerpt = (d.excerpt || '').trim();

  return (
    <div
      className="node-tooltip"
      style={{ left: tip.x + 14, top: tip.y - 8 }}
    >
      <div className="node-tooltip__name">{d.label}</div>
      <div className="node-tooltip__type">{typeLabel}</div>
      {excerpt && (
        <div className="node-tooltip__excerpt">
          {excerpt.length > 110 ? excerpt.slice(0, 108) + '…' : excerpt}
        </div>
      )}
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
          <span className="node-tooltip__val">
            {d.var_writes.slice(0, 5).join(', ')}{d.var_writes.length > 5 ? '…' : ''}
          </span>
        </div>
      )}
      {d.var_reads?.length > 0 && (
        <div className="node-tooltip__row">
          <span className="node-tooltip__key">reads</span>
          <span className="node-tooltip__val">
            {d.var_reads.slice(0, 5).join(', ')}{d.var_reads.length > 5 ? '…' : ''}
          </span>
        </div>
      )}
      {d.file && (
        <div className="node-tooltip__file">{d.file.split('/').pop()}</div>
      )}
      <div className="node-tooltip__hint">Click to open in editor</div>
    </div>
  );
}

// ── Inner StoryMap ─────────────────────────────────────────────────────────

/** Actions the graph surface can request from the app shell. */
interface StoryMapActions {
  /** Selection changed (empty array = nothing selected). Multi-select aware. */
  onSelectionChange: (nodes: { id: string; data: PassageNodeData }[]) => void;
  /** Open a passage in the editor (instant navigation). */
  onOpenPassage: (file: string, line: number, passageName: string) => void;
  /** Begin "new passage" at graph coordinates. */
  onCreatePassageAt: (x: number, y: number) => void;
  /** Set the story's start passage. */
  onSetStart: (name: string) => void;
  /** Steer the inspector: focus a field or arm the delete confirm. */
  onActionSignal: (field: 'rename' | 'reachable' | 'delete' | 'tags' | 'color' | 'group') => void;
  /** Refresh the graph from the server. */
  onRequestRefresh: () => void;
  /** Persist the whole layout to the workspace. */
  onRequestSaveLayout: () => void;
  /** Viewport tracking (stored in a ref by the shell — no re-render). */
  onViewportKnown: (vp: { x: number; y: number; zoom: number }) => void;
}

interface StoryMapInnerProps extends StoryMapActions {
  graphData: KnotGraphResponse | null;
  searchQuery: string;
  selectedTags: Set<string>;
  fitRequested: number;
  saveRequested: number;
  focusRequested: number;
  focusPassageName: string;
  /** Whether an App-driven focus should be recorded in the nav history. */
  focusPushHistory: boolean;
  /** Timestamp signal: the shell asks the canvas to drop its selection. */
  clearSelectionRequested: number;
  restoreViewport: { x: number; y: number; zoom: number; ts: number } | null;
  /** View toggles (owned by the shell, flipped from the context menu). */
  showMiniMap: boolean;
  showLegend: boolean;
  onToggleMiniMap: () => void;
  onToggleLegend: () => void;
  /** Open the find palette (context menu entry). */
  onFindPassages: () => void;
  /** New passage at the current view center (context menu entry). */
  onNewPassageAtViewCenter: () => void;
}

function StoryMapInner({
  graphData,
  searchQuery,
  selectedTags,
  fitRequested,
  saveRequested,
  focusRequested,
  focusPassageName,
  focusPushHistory,
  clearSelectionRequested,
  restoreViewport,
  showMiniMap,
  showLegend,
  onToggleMiniMap,
  onToggleLegend,
  onFindPassages,
  onNewPassageAtViewCenter,
  onSelectionChange,
  onOpenPassage,
  onCreatePassageAt,
  onSetStart,
  onActionSignal,
  onRequestRefresh,
  onRequestSaveLayout,
  onViewportKnown,
}: StoryMapInnerProps) {
  const { fitView, setViewport, getViewport, getNode, screenToFlowPosition, zoomIn, zoomOut } = useReactFlow();

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

  // FIX: tooltip position stored in a ref to avoid triggering re-renders on
  // every mouse-move; only the displayed tip state triggers a render.
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const tooltipPosRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const activeTooltipNodeRef = useRef<string | null>(null);

  const initialFitDoneRef = useRef(false);
  const savedViewportRef = useRef<{ x: number; y: number; zoom: number } | null>(null);
  // When true, the next graph rebuild should NOT restore the saved viewport
  // (e.g. after a focusNode navigation — we've already set the viewport there).
  const skipNextViewportRestoreRef = useRef(false);
  const nodesRef = useRef<Node[]>([]);
  useEffect(() => { nodesRef.current = nodes; }, [nodes]);

  // ── Selection bookkeeping (multi-select aware) ───────────────────────
  // selectedIdsRef mirrors React Flow's selection so keyboard handlers and
  // graph rebuilds can read it without re-binding. primaryIdRef is the
  // "focused" member of the selection (last plain-clicked) — Enter/F2 act
  // on it.
  const selectedIdsRef = useRef<Set<string>>(new Set());
  const primaryIdRef = useRef<string | null>(null);

  /** Report a selection (built from a set of ids) to the app shell. */
  const reportFromIds = useCallback((ids: Set<string>) => {
    const sel = nodesRef.current
      .filter(n => ids.has(n.id) && n.type === 'passage')
      .map(n => ({ id: n.id, data: n.data as PassageNodeData }));
    onSelectionChange(sel);
  }, [onSelectionChange]);

  // ── Navigation history (mouse back / forward buttons) ────────────────
  // Every plain click on a passage (and palette navigations) pushes onto
  // the history; mouse buttons 3/4 (back/forward) and Alt+←/→ walk it.
  const navHistoryRef = useRef<{ names: string[]; idx: number }>({ names: [], idx: -1 });

  const pushHistory = useCallback((name: string) => {
    const h = navHistoryRef.current;
    if (h.names[h.idx] === name) return;
    h.names = h.names.slice(0, h.idx + 1);
    h.names.push(name);
    h.idx = h.names.length - 1;
    if (h.names.length > 120) {
      h.names.shift();
      h.idx--;
    }
  }, []);

  // ── Focus target: pan + select + flash a passage ─────────────────────
  // Driven either by App props (editor cursor / find palette) or internally
  // (history back/forward).
  const [focus, setFocus] = useState<{ name: string; ts: number; push: boolean } | null>(null);

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

    // Snapshot current viewport before rebuilding so we can restore it
    try {
      const vp = getViewport();
      savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
    } catch { /* not ready yet */ }

    const { nodes: newNodes, edges: newEdges } = buildElements(graphData);

    // Preserve the whole selection across rebuilds (metadata edits and
    // other refreshes shouldn't slam the inspector shut). Deselected are
    // only the passages that no longer exist.
    const keptIds = selectedIdsRef.current;
    if (keptIds.size > 0) {
      const survivors = new Set<string>();
      newNodes.forEach((n, i) => {
        if (keptIds.has(n.id)) {
          survivors.add(n.id);
          newNodes[i] = { ...n, selected: true } as Node;
        }
      });
      selectedIdsRef.current = survivors;
      if (primaryIdRef.current && !survivors.has(primaryIdRef.current)) {
        primaryIdRef.current = survivors.size > 0 ? [...survivors][0] : null;
      }
      // Re-report so the inspector shows fresh data after the rebuild.
      const sel = newNodes
        .filter(n => survivors.has(n.id) && n.type === 'passage')
        .map(n => ({ id: n.id, data: n.data as PassageNodeData }));
      onSelectionChange(sel);
    }

    setNodes(newNodes);
    setEdges(newEdges);

    if (!initialFitDoneRef.current && newNodes.length > 0) {
      // First load — fit all nodes into view
      initialFitDoneRef.current = true;
      requestAnimationFrame(() => {
        fitView({ padding: 0.12, duration: 350 });
      });
    } else if (skipNextViewportRestoreRef.current) {
      // A focusNode navigation just happened — don't fight it
      skipNextViewportRestoreRef.current = false;
    } else if (savedViewportRef.current) {
      // Normal refresh — restore the user's current viewport
      const vp = savedViewportRef.current;
      requestAnimationFrame(() => {
        setViewport(vp, { duration: 0 });
      });
    }
  }, [graphData, setNodes, setEdges, fitView, setViewport, getViewport, onSelectionChange]);

  // ── Search + tag filter ─────────────────────────────────────────────

  useEffect(() => {
    const q = searchQuery.toLowerCase().trim();
    const hasTags = selectedTags.size > 0;
    const hasFilter = q !== '' || hasTags;

    const matchIds = new Set<string>();
    if (hasFilter) {
      for (const n of nodesRef.current) {
        if (n.type !== 'passage') continue;
        const d = n.data as PassageNodeData;

        // Search query: match label or tags
        const matchesSearch = q === '' ||
          (d.label || '').toLowerCase().includes(q) ||
          (d.tags || []).some(t => t.toLowerCase().includes(q));

        // Tag filter: a node matches when it carries ANY selected tag
        // (OR logic — each added tag widens the match). Use .every()
        // instead if AND semantics are ever wanted.
        const matchesTags = !hasTags ||
          (d.tags || []).some(t => selectedTags.has(t));

        if (matchesSearch && matchesTags) {
          matchIds.add(n.id);
        }
      }
    }

    setNodes(nds => nds.map(n => {
      if (n.type !== 'passage') return n;
      const d = n.data as PassageNodeData;
      if (!hasFilter) return { ...n, data: { ...d, dimmed: false, highlighted: false } };
      const matches = matchIds.has(n.id);
      return { ...n, data: { ...d, dimmed: !matches, highlighted: matches } };
    }));

    setEdges(eds => eds.map(e => {
      if (!hasFilter) return { ...e, style: undefined };
      const connected = matchIds.has(e.source) || matchIds.has(e.target);
      return { ...e, style: { ...e.style, opacity: connected ? undefined : 0.06 } };
    }));
  }, [searchQuery, selectedTags, setNodes, setEdges]);

  // ── Fit view ───────────────────────────────────────────────────────────

  useEffect(() => {
    if (fitRequested > 0) fitView({ padding: 0.12, duration: 280 });
  }, [fitRequested, fitView]);

  // ── Restore persisted viewport (sent by extension on panel show) ───────

  useEffect(() => {
    if (!restoreViewport) return;
    // Only restore if we haven't yet done the initial fitView (i.e. no graph
    // data arrived yet and the panel was just re-shown to an existing session).
    // If the graph is already loaded, the graph useEffect will restore it.
    if (!initialFitDoneRef.current) {
      setViewport(
        { x: restoreViewport.x, y: restoreViewport.y, zoom: restoreViewport.zoom },
        { duration: 0 },
      );
    }
  }, [restoreViewport, setViewport]);

  // ── Shell-driven selection clear (inspector ✕ button) ────────────────

  useEffect(() => {
    if (clearSelectionRequested > 0) clearSelection();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clearSelectionRequested]);

  // ── App-driven focus (editor cursor / find palette) → internal state ───

  useEffect(() => {
    if (focusRequested > 0 && focusPassageName) {
      setFocus({ name: focusPassageName, ts: focusRequested, push: focusPushHistory });
    }
  }, [focusRequested, focusPassageName, focusPushHistory]);

  // ── Focus a passage: pan, select, flash ────────────────────────────────
  // FIX: use setViewport directly instead of fitView with locked min/maxZoom,
  // which broke when current zoom > 0.5. We pan to the node center while
  // preserving the user's zoom level (or nudging it up to MIN_READABLE_ZOOM).

  const MIN_READABLE_ZOOM = 0.5;

  useEffect(() => {
    if (!focus) return;

    const nds = nodesRef.current;
    let target = getNode(focus.name);
    if (!target) {
      const found = nds.find(
        n => n.type === 'passage' && (n.data as PassageNodeData).label === focus.name,
      );
      if (found) target = getNode(found.id);
    }
    if (!target) return;

    // Mark that we're navigating so the next graph refresh doesn't fight us
    skipNextViewportRestoreRef.current = true;

    // Pan to node, preserve zoom (or bump up to readable minimum)
    const currentZoom = getViewport().zoom;
    const zoom = Math.max(currentZoom, MIN_READABLE_ZOOM);

    if (Math.abs(currentZoom - zoom) < 0.01) {
      // Zoom didn't change — use fitView with locked zoom for a pure pan
      fitView({
        nodes: [{ id: target.id }],
        padding: 0.4,
        duration: 300,
        minZoom: zoom,
        maxZoom: zoom,
      });
    } else {
      // Zoom needs to change — let fitView pick a sensible level
      fitView({
        nodes: [{ id: target.id }],
        padding: 0.35,
        duration: 300,
        minZoom: MIN_READABLE_ZOOM,
        maxZoom: MAX_ZOOM,
      });
    }

    // Make the focused node the sole selection (inspector follows).
    setNodes(cur => cur.map(n => {
      if (n.type !== 'passage') return n;
      const d = n.data as PassageNodeData;
      const hit = n.id === target!.id;
      return { ...n, selected: hit, data: { ...d, focused: hit } };
    }));
    selectedIdsRef.current = new Set([target.id]);
    primaryIdRef.current = target.id;
    onSelectionChange([{ id: target.id, data: target.data as PassageNodeData }]);

    if (focus.push) pushHistory(focus.name);

    const timer = setTimeout(() => {
      setNodes(cur => cur.map(n => {
        if (n.type !== 'passage') return n;
        const d = n.data as PassageNodeData;
        return { ...n, data: { ...d, focused: false } };
      }));
      // Now safe to restore on next refresh
      skipNextViewportRestoreRef.current = false;
    }, 1200);

    return () => clearTimeout(timer);
  }, [focus, fitView, getViewport, getNode, setNodes, onSelectionChange, pushHistory]);

  // ── Save all positions ─────────────────────────────────────────────────

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
      try {
        const vp = getViewport();
        savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
      } catch { /* not ready */ }
      vscode.postMessage({ command: 'saveAllPositions', updates });
    }
  }, [saveRequested, getViewport]);

  // ── Node drag end → snap + send position ──────────────────────────────
  // React Flow emits position changes for EVERY node in a multi-selection
  // drag, so marquee-selected groups persist their positions together.

  const handleNodesChange = useCallback((changes: NodeChange[]) => {
    onNodesChange(changes);

    const dragEnds = changes.filter(
      (c): c is NodePositionChange =>
        c.type === 'position' && c.dragging === false && !!c.position,
    );
    if (dragEnds.length === 0) return;

    const snapped = new Map(
      dragEnds.map(c => [c.id, { x: snap(c.position!.x), y: snap(c.position!.y) }]),
    );
    const updates: KnotPositionUpdate[] = [];

    setNodes(nds => nds.map(n => {
      const s = snapped.get(n.id);
      if (!s) return n;
      const d = n.data as PassageNodeData;
      updates.push({
        passage_name: n.id,
        position_x: s.x,
        position_y: s.y,
        group: d.group,
        color: d.metadata_color,
      });
      return { ...n, position: s };
    }));

    debouncedPositionUpdate(updates);
  }, [onNodesChange, setNodes, debouncedPositionUpdate]);

  // ── Instant navigation: plain click on a node → open it in the editor ──
  // The click doubles as the selection; modifier clicks (Shift/Ctrl/Cmd/
  // Alt) only build a multi-selection and never navigate.

  const openPassageNode = useCallback((node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.file) onOpenPassage(d.file, d.line || 0, d.label);
  }, [onOpenPassage]);

  const navigateToNode = useCallback((node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.file) onOpenPassage(d.file, d.line || 0, d.label);
    pushHistory(d.label);
  }, [onOpenPassage, pushHistory]);

  // Suppress the click that follows a node drag (mouseup after a real move
  // still synthesizes a click event — don't let it slam the editor around).
  const suppressClickRef = useRef(false);
  const dragStartPosRef = useRef<{ x: number; y: number } | null>(null);

  const handleNodeDragStart = useCallback((e: MouseEvent | TouchEvent) => {
    const ce = e as MouseEvent;
    dragStartPosRef.current = { x: ce.clientX, y: ce.clientY };
  }, []);

  const handleNodeDragStop = useCallback((e: MouseEvent | TouchEvent) => {
    const s = dragStartPosRef.current;
    dragStartPosRef.current = null;
    const ce = e as MouseEvent;
    if (s && Math.hypot(ce.clientX - s.x, ce.clientY - s.y) > 4) {
      suppressClickRef.current = true;
      window.setTimeout(() => { suppressClickRef.current = false; }, 250);
    }
  }, []);

  const handleNodeClick = useCallback((e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    if (suppressClickRef.current) return;
    if (e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    primaryIdRef.current = node.id;
    navigateToNode(node);
  }, [navigateToNode]);

  const handleNodeDoubleClick = useCallback((_e: React.MouseEvent, node: Node) => {
    // Single click already navigates; keep double-click as an alias for
    // muscle memory (no double message issues — same target).
    openPassageNode(node);
  }, [openPassageNode]);

  // ── Selection → inspector (multi-aware) ─────────────────────────────

  const handleSelectionChange = useCallback((params: OnSelectionChangeParams) => {
    const selected = params.nodes.filter(n => n.type === 'passage');
    const ids = new Set(selected.map(n => n.id));
    selectedIdsRef.current = ids;
    if (!primaryIdRef.current || !ids.has(primaryIdRef.current)) {
      primaryIdRef.current = selected.length > 0 ? selected[selected.length - 1].id : null;
    }
    onSelectionChange(
      selected.map(n => ({ id: n.id, data: n.data as PassageNodeData })),
    );
  }, [onSelectionChange]);

  // ── Selection helpers (context menu / keyboard) ──────────────────────

  const selectAllPassages = useCallback(() => {
    const ids = new Set<string>();
    setNodes(nds => nds.map(n => {
      if (n.type !== 'passage') return n;
      ids.add(n.id);
      return { ...n, selected: true };
    }));
    selectedIdsRef.current = ids;
    if (!primaryIdRef.current || !ids.has(primaryIdRef.current)) {
      primaryIdRef.current = ids.size > 0 ? [...ids][0] : null;
    }
    reportFromIds(ids);
  }, [setNodes, reportFromIds]);

  const clearSelection = useCallback(() => {
    if (selectedIdsRef.current.size === 0) return;
    setNodes(nds => nds.map(n => (n.selected ? { ...n, selected: false } : n)));
    selectedIdsRef.current = new Set();
    primaryIdRef.current = null;
    onSelectionChange([]);
  }, [setNodes, onSelectionChange]);

  // ── History navigation (mouse back/forward + Alt+←/→) ───────────────

  const historyNavigate = useCallback((name: string) => {
    const node = nodesRef.current.find(
      n => n.id === name || (n.type === 'passage' && (n.data as PassageNodeData).label === name),
    );
    if (!node) return;
    const d = node.data as PassageNodeData;
    if (d.file) onOpenPassage(d.file, d.line || 0, d.label);
    setFocus({ name, ts: Date.now(), push: false });
  }, [onOpenPassage]);

  const goBack = useCallback(() => {
    const h = navHistoryRef.current;
    if (h.idx > 0) {
      h.idx--;
      historyNavigate(h.names[h.idx]);
    }
  }, [historyNavigate]);

  const goForward = useCallback(() => {
    const h = navHistoryRef.current;
    if (h.idx < h.names.length - 1) {
      h.idx++;
      historyNavigate(h.names[h.idx]);
    }
  }, [historyNavigate]);

  // Mouse button 3 = back, 4 = forward. Also swallow the default so the
  // webview host doesn't walk its own history.
  const handleCanvasMouseUp = useCallback((e: React.MouseEvent) => {
    if (e.button === 3) {
      e.preventDefault();
      e.stopPropagation();
      goBack();
    } else if (e.button === 4) {
      e.preventDefault();
      e.stopPropagation();
      goForward();
    }
  }, [goBack, goForward]);

  // Middle-button autoscroll guard (Windows browsers scroll the page on
  // middle-mousedown when the event isn't cancelled).
  const handleCanvasMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button === 1) e.preventDefault();
  }, []);

  // Twine-native: double-click empty canvas → create a passage there.
  // (React Flow v12 has no onPaneDoubleClick prop, so we discriminate on the
  // native event target: only bare-pane double-clicks count — not nodes,
  // not edges, not the minimap.)
  const handleCanvasDoubleClick = useCallback((e: React.MouseEvent) => {
    const el = e.target as HTMLElement;
    if (el.closest('.react-flow__node')) return;
    if (el.closest('.react-flow__edges')) return;
    if (el.closest('.react-flow__minimap')) return;
    if (el.closest('.react-flow__panel')) return;
    if (!el.closest('.react-flow')) return;
    const pos = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    onCreatePassageAt(pos.x - NODE_W / 2, pos.y - NODE_H / 2);
  }, [screenToFlowPosition, onCreatePassageAt]);

  // ── Context menu ─────────────────────────────────────────────────────

  const [ctxMenu, setCtxMenu] = useState<ContextMenuState | null>(null);

  const closeCtxMenu = useCallback(() => setCtxMenu(null), []);

  // Close on any click / Escape (registered once, capture-phase so it runs
  // before menu-item handling has a chance to swallow events). Escape also
  // stops propagation so the canvas keyboard layer doesn't deselect at the
  // same time — one press, one action.
  useEffect(() => {
    if (!ctxMenu) return;
    const onMouseDown = () => closeCtxMenu();
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        closeCtxMenu();
      }
    };
    document.addEventListener('mousedown', onMouseDown);
    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('mousedown', onMouseDown);
      document.removeEventListener('keydown', onKeyDown, true);
    };
  }, [ctxMenu, closeCtxMenu]);

  const handleNodeContextMenu = useCallback((e: React.MouseEvent, node: Node) => {
    e.preventDefault();
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;

    // Right-clicking an unselected node makes it the sole selection (the
    // standard OS pattern) so menu actions and the inspector agree.
    if (!selectedIdsRef.current.has(node.id)) {
      setNodes(nds => nds.map(n => ({ ...n, selected: n.id === node.id })));
      selectedIdsRef.current = new Set([node.id]);
      primaryIdRef.current = node.id;
      onSelectionChange([{ id: node.id, data: d }]);
    }

    setCtxMenu({
      x: e.clientX,
      y: e.clientY,
      kind: 'node',
      nodeId: node.id,
      isStart: d.is_start,
      selectionCount: selectedIdsRef.current.size,
      minimapOn: showMiniMap,
      legendOn: showLegend,
    });
  }, [setNodes, onSelectionChange, showMiniMap, showLegend]);

  const handlePaneContextMenu = useCallback((e: React.MouseEvent | MouseEvent) => {
    e.preventDefault();
    const pos = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    setCtxMenu({
      x: e.clientX,
      y: e.clientY,
      kind: 'pane',
      graphX: pos.x,
      graphY: pos.y,
      selectionCount: selectedIdsRef.current.size,
      minimapOn: showMiniMap,
      legendOn: showLegend,
    });
  }, [screenToFlowPosition, showMiniMap, showLegend]);

  const ctxActions = useMemo(() => ({
    onOpen: (nodeId: string) => {
      const node = nodesRef.current.find(n => n.id === nodeId);
      if (node) openPassageNode(node);
    },
    onRename: (_nodeId: string) => onActionSignal('rename'),
    onSetStart: (nodeId: string) => {
      const node = nodesRef.current.find(n => n.id === nodeId);
      if (node) {
        const d = node.data as PassageNodeData;
        onSetStart(d.label);
      }
    },
    onManualLink: (_nodeId: string) => onActionSignal('reachable'),
    onEditTags: () => onActionSignal('tags'),
    onEditColor: () => onActionSignal('color'),
    onEditGroup: () => onActionSignal('group'),
    onDelete: (_nodeId: string) => onActionSignal('delete'),
    onNewPassageHere: (x: number, y: number) => onCreatePassageAt(x, y),
    onNewPassageAtViewCenter,
    onFitView: () => fitView({ padding: 0.12, duration: 280 }),
    onZoomIn: () => zoomIn({ duration: 200 }),
    onZoomOut: () => zoomOut({ duration: 200 }),
    onSelectAll: selectAllPassages,
    onClearSelection: clearSelection,
    onFind: onFindPassages,
    onSaveLayout: onRequestSaveLayout,
    onRefresh: onRequestRefresh,
    onToggleMiniMap: onToggleMiniMap,
    onToggleLegend: onToggleLegend,
  }), [openPassageNode, onActionSignal, onSetStart, onCreatePassageAt, onNewPassageAtViewCenter, fitView, zoomIn, zoomOut, selectAllPassages, clearSelection, onFindPassages, onRequestSaveLayout, onRequestRefresh, onToggleMiniMap, onToggleLegend]);

  // ── Keyboard shortcuts ───────────────────────────────────────────────

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Never intercept typing in inputs (inspector, modal, palette).
      const target = e.target as HTMLElement | null;
      if (target && (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.tagName === 'SELECT' ||
        target.isContentEditable
      )) {
        return;
      }

      const mod = e.ctrlKey || e.metaKey;

      // Canvas-level actions work without a selection.
      if (mod && e.key.toLowerCase() === 'a') {
        e.preventDefault();
        selectAllPassages();
        return;
      }
      if (e.key === 'Escape') {
        clearSelection();
        return;
      }
      if (!mod && !e.altKey && (e.key === 'f' || e.key === 'F')) {
        e.preventDefault();
        fitView({ padding: 0.12, duration: 280 });
        return;
      }
      if (e.altKey && e.key === 'ArrowLeft') {
        e.preventDefault();
        goBack();
        return;
      }
      if (e.altKey && e.key === 'ArrowRight') {
        e.preventDefault();
        goForward();
        return;
      }

      // Selection-level actions.
      const selectedId = primaryIdRef.current;
      if (!selectedId) return;

      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        onActionSignal('delete');
      } else if (e.key === 'Enter') {
        e.preventDefault();
        const node = nodesRef.current.find(n => n.id === selectedId);
        if (node) navigateToNode(node);
      } else if (e.key === 'F2') {
        if (selectedIdsRef.current.size === 1) {
          e.preventDefault();
          onActionSignal('rename');
        }
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onActionSignal, navigateToNode, selectAllPassages, clearSelection, fitView, goBack, goForward]);

  // ── Tooltip (ref-based to avoid re-render on every mousemove) ──────────

  const handleNodeMouseEnter = useCallback((_e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    activeTooltipNodeRef.current = node.id;
    setTooltip({
      x: tooltipPosRef.current.x,
      y: tooltipPosRef.current.y,
      data: node.data as PassageNodeData,
    });
  }, []);

  const handleNodeMouseLeave = useCallback(() => {
    activeTooltipNodeRef.current = null;
    setTooltip(null);
  }, []);

  // FIX: only update tooltip state (causing re-render) when a node tooltip is
  // actually active; otherwise just update the position ref silently.
  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    tooltipPosRef.current = { x: e.clientX, y: e.clientY };
    if (activeTooltipNodeRef.current !== null) {
      setTooltip(t => t ? { ...t, x: e.clientX, y: e.clientY } : null);
    }
  }, []);

  // ── Viewport change ────────────────────────────────────────────────────

  const handleViewportChange = useCallback(() => {
    try {
      const vp = getViewport();
      debouncedViewportUpdate(vp.x, vp.y, vp.zoom);
      // Report the live viewport to the shell (it stores it in a ref —
      // no re-render) so "new passage" from the context menu can anchor
      // at the current view center.
      onViewportKnown(vp);
    } catch { /* not ready */ }
  }, [getViewport, debouncedViewportUpdate, onViewportKnown]);

  // ── MiniMap color ──────────────────────────────────────────────────────

  const miniMapColor = useCallback((n: Node) => {
    if (n.type !== 'passage') return 'rgba(148, 163, 184, 0.18)';
    return (n.data as PassageNodeData).color || COLORS.normal;
  }, []);

  return (
    <div
      className="storymap-inner"
      onMouseMove={handleMouseMove}
      onMouseUp={handleCanvasMouseUp}
      onMouseDown={handleCanvasMouseDown}
      onDoubleClick={handleCanvasDoubleClick}
    >
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={handleNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeClick={handleNodeClick}
        onNodeDoubleClick={handleNodeDoubleClick}
        onNodeDragStart={handleNodeDragStart}
        onNodeDragStop={handleNodeDragStop}
        onNodeContextMenu={handleNodeContextMenu}
        onPaneContextMenu={handlePaneContextMenu}
        onSelectionChange={handleSelectionChange}
        onNodeMouseEnter={handleNodeMouseEnter}
        onNodeMouseLeave={handleNodeMouseLeave}
        onViewportChange={handleViewportChange}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        connectionMode={ConnectionMode.Loose}
        minZoom={MIN_ZOOM}
        maxZoom={MAX_ZOOM}
        fitView={false}
        // Mouse map:
        //   left-drag on pane  → marquee (multi-)selection
        //   left-drag on node  → move node(s)
        //   middle-drag        → pan
        //   wheel              → zoom
        //   right-click        → context menu (the "action panel")
        //   back/forward       → navigation history (wrapper handlers)
        panOnDrag={[1]}
        selectionOnDrag
        selectionKeyCode={null}
        multiSelectionKeyCode="Shift"
        zoomOnDoubleClick={false}
        nodesDraggable
        nodesConnectable={false}
        elementsSelectable
        // Delete/Backspace are handled by our keyboard layer (which arms
        // the inspector's confirm flow) — disable React Flow's built-in
        // node deletion so the two never fight.
        deleteKeyCode={null}
        proOptions={{ hideAttribution: true }}
        defaultEdgeOptions={{ type: 'straight' }}
      >
        {showMiniMap && (
          <MiniMap
            nodeColor={miniMapColor}
            nodeStrokeColor="rgba(0, 0, 0, 0.45)"
            nodeBorderRadius={3}
            maskColor="rgba(10, 11, 16, 0.62)"
            pannable
            zoomable
          />
        )}
        <Background
          variant={BackgroundVariant.Dots}
          gap={GRID_SNAP}
          size={1.5}
          color="rgba(148, 163, 184, 0.10)"
        />
      </ReactFlow>
      <NodeTooltip tip={tooltip} />
      {ctxMenu && (
        <ContextMenu menu={ctxMenu} actions={ctxActions} onClose={closeCtxMenu} />
      )}
    </div>
  );
}

// ── Outer component (provides ReactFlowProvider) ───────────────────────────

interface StoryMapProps extends StoryMapActions {
  graphData: KnotGraphResponse | null;
  searchQuery: string;
  selectedTags: Set<string>;
  fitRequested: number;
  saveRequested: number;
  focusRequested: number;
  focusPassageName: string;
  focusPushHistory: boolean;
  /** Timestamp signal: the shell asks the canvas to drop its selection. */
  clearSelectionRequested: number;
  /** Viewport state to restore, forwarded from the extension's restoreViewport message. */
  restoreViewport: { x: number; y: number; zoom: number; ts: number } | null;
  showMiniMap: boolean;
  showLegend: boolean;
  onToggleMiniMap: () => void;
  onToggleLegend: () => void;
  onFindPassages: () => void;
  onNewPassageAtViewCenter: () => void;
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
