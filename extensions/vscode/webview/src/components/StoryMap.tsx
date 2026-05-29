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
  type InternalNode,
  type ReactFlowState,
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
const BIDIR_OFFSET = 7;

// ── Twine 2 layout anchors ────────────────────────────────────────────────

const START_ANCHOR_X = 420;
const START_ANCHOR_Y = 60;
const SPECIAL_BOX_X = 40;
const SPECIAL_BOX_Y = 60;
const UNREACHABLE_LIST_X = 40;
const UNREACHABLE_LIST_Y_OFFSET = 60;

// ── Color palette ──────────────────────────────────────────────────────────

const COLORS = {
  normal:     '#2d6a9f',
  start:      '#2e7d32',
  special:    '#e65100',
  metadata:   '#6a1b9a',
  unreachable:'#bf6900',
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

// ── Rectangle intersection for floating edges ──────────────────────────────

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

  const scale = (absDx * halfH > absDy * halfW)
    ? halfW / absDx
    : halfH / absDy;

  return [center.x + dx * scale, center.y + dy * scale];
}

// ── Custom Node: Passage ───────────────────────────────────────────────────

function PassageNode({ data }: NodeProps<Node<PassageNodeData>>) {
  const d = data as PassageNodeData;
  const color = d.color || COLORS.normal;

  const cls = [
    'pn',
    d.is_start       && 'pn--start',
    d.is_special     && 'pn--special',
    d.is_metadata    && 'pn--metadata',
    d.is_unreachable && 'pn--unreachable',
    d.highlighted    && 'pn--highlighted',
    d.dimmed         && 'pn--dimmed',
    d.focused        && 'pn--focused',
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
  edgeKind: 'navigation' | 'upstream' | 'broken';
  offsetPx: number;
}

const ARROW_PULLBACK = 5;

function StraightEdge({
  id,
  source, target,
  sourceX, sourceY,
  targetX, targetY,
  data,
  markerEnd,
}: EdgeProps) {
  const { edgeKind = 'navigation', offsetPx = 0 } = (data ?? {}) as unknown as StraightEdgeData;

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

  let sx = sourceX;
  let sy = sourceY;
  let tx = targetX;
  let ty = targetY;

  if (srcData && tgtData) {
    const srcCenter = { x: srcData.x + srcData.w / 2, y: srcData.y + srcData.h / 2 };
    const tgtCenter = { x: tgtData.x + tgtData.w / 2, y: tgtData.y + tgtData.h / 2 };

    [sx, sy] = getRectIntersection(srcCenter, tgtCenter, srcData.w, srcData.h);
    [tx, ty] = getRectIntersection(tgtCenter, srcCenter, tgtData.w, tgtData.h);

    const edgeDx = tx - sx;
    const edgeDy = ty - sy;
    const edgeLen = Math.sqrt(edgeDx * edgeDx + edgeDy * edgeDy) || 1;
    tx -= (edgeDx / edgeLen) * ARROW_PULLBACK;
    ty -= (edgeDy / edgeLen) * ARROW_PULLBACK;
  }

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

  const edgeSet = new Set<string>(rawEdges.map(e => `${e.source}→${e.target}`));
  const isBidir = (src: string, tgt: string) =>
    edgeSet.has(`${src}→${tgt}`) && edgeSet.has(`${tgt}→${src}`);
  const bidirFirst = new Set<string>();

  const childToGroup = new Map<string, string>();
  for (const [gid, members] of groupMembers) {
    for (const mid of members) childToGroup.set(mid, gid);
  }

  const startNodes: KnotGraphNode[] = [];
  const specialNodes: KnotGraphNode[] = [];
  const unreachableNodes: KnotGraphNode[] = [];
  const regularNodes: KnotGraphNode[] = [];

  for (const n of rawNodes) {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    if (isStart)                   startNodes.push(n);
    else if (specialSet.has(n.id)) specialNodes.push(n);
    else if (unreachableSet.has(n.id)) unreachableNodes.push(n);
    else                           regularNodes.push(n);
  }

  const allHavePositions = rawNodes.every(
    n => n.position_x != null && n.position_y != null,
  );

  function makePassageNode(n: KnotGraphNode, x: number, y: number): Node<PassageNodeData> {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    const groupId = childToGroup.get(n.id);

    return {
      id: n.id,
      type: 'passage',
      // FIX: when a node has a parentId, React Flow treats its position as
      // relative to the parent. We do NOT set parentId here to keep all
      // passage coordinates in the global canvas frame, which is also what
      // the Rust server stores and what saveAllPositions writes back.
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
        group: groupId,
        dimmed: false,
        highlighted: false,
        focused: false,
      },
      // NOTE: parentId intentionally omitted — see comment above.
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

    const pad = 32;
    const xs = children.map(n => n.position.x);
    const ys = children.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - 18;
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
        background: 'rgba(191, 105, 0, 0.04)',
        borderColor: 'rgba(191, 105, 0, 0.25)',
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
      e.edge_type === 'broken'   ? 'broken'   : 'navigation';

    const markerColor =
      edgeKind === 'broken'   ? COLORS.edgeBroken :
      edgeKind === 'upstream' ? COLORS.edgeUp     : COLORS.edgeNav;

    rfEdges.push({
      id: eid,
      source: e.source,
      target: e.target,
      type: 'straight',
      data: { edgeKind, offsetPx } as unknown as Record<string, unknown>,
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
  restoreViewport: { x: number; y: number; zoom: number; ts: number } | null;
}

function StoryMapInner({
  graphData,
  searchQuery,
  fitRequested,
  saveRequested,
  focusRequested,
  focusPassageName,
  restoreViewport,
}: StoryMapInnerProps) {
  const { fitView, setViewport, getViewport, getNode } = useReactFlow();

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
  }, [graphData, setNodes, setEdges, fitView, setViewport, getViewport]);

  // ── Search filter ──────────────────────────────────────────────────────

  useEffect(() => {
    const q = searchQuery.toLowerCase().trim();

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

  // ── Restore persisted viewport (sent by extension on panel show) ───────
  // FIX: storyMapProvider.ts sends restoreViewport after a 200ms delay when
  // the panel is first created. The old code had no handler for this message
  // so the persisted viewport was always ignored.

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

  // ── Focus a passage ────────────────────────────────────────────────────
  // FIX: use setViewport directly instead of fitView with locked min/maxZoom,
  // which broke when current zoom > 0.5. We pan to the node center while
  // preserving the user's zoom level (or nudging it up to MIN_READABLE_ZOOM).

  const MIN_READABLE_ZOOM = 0.5;

  useEffect(() => {
    if (focusRequested <= 0 || !focusPassageName) return;

    const nds = nodesRef.current;
    let target = getNode(focusPassageName);
    if (!target) {
      const found = nds.find(
        n => n.type === 'passage' && (n.data as PassageNodeData).label === focusPassageName,
      );
      if (found) target = getNode(found.id);
    }
    if (!target) return;

    // Mark that we're navigating so the next graph refresh doesn't fight us
    skipNextViewportRestoreRef.current = true;

    // Pan to node, preserve zoom (or bump up to readable minimum)
    const currentZoom = getViewport().zoom;
    const zoom = Math.max(currentZoom, MIN_READABLE_ZOOM);

    // Node center in canvas coords
    const cx = target.position.x + NODE_W / 2;
    const cy = target.position.y + NODE_H / 2;

    // Convert to viewport: vp.x = viewportCenterX - cx * zoom
    // We don't have viewport dimensions here so use fitView with tight padding,
    // but clamp zoom manually via the min/maxZoom trick only when needed.
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

    // Flash highlight the focused node
    setNodes(nds => nds.map(n => {
      if (n.type !== 'passage') return n;
      const d = n.data as PassageNodeData;
      return { ...n, data: { ...d, focused: n.id === target!.id } };
    }));

    const timer = setTimeout(() => {
      setNodes(nds => nds.map(n => {
        if (n.type !== 'passage') return n;
        const d = n.data as PassageNodeData;
        return { ...n, data: { ...d, focused: false } };
      }));
      // Now safe to restore on next refresh
      skipNextViewportRestoreRef.current = false;
    }, 1200);

    return () => clearTimeout(timer);
  }, [focusRequested, focusPassageName, fitView, getViewport, getNode, setNodes]);

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

  // ── Node click → open passage ──────────────────────────────────────────

  const handleNodeClick = useCallback((_e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.file) vscode.postMessage({ command: 'openPassage', file: d.file, line: d.line || 0, passageName: d.label });
  }, []);

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
  /** Viewport state to restore, forwarded from the extension's restoreViewport message. */
  restoreViewport: { x: number; y: number; zoom: number; ts: number } | null;
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