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

// ── Custom straight edge with optional perpendicular offset ────────────────
// The `data.offsetPx` field shifts the path sideways for bidir pairs.
// The `data.edgeKind` drives color/dash.

interface StraightEdgeData {
  edgeKind: 'navigation' | 'upstream' | 'broken';
  offsetPx: number; // perpendicular offset in pixels (0 = no offset)
}

function StraightEdge({
  id,
  sourceX, sourceY,
  targetX, targetY,
  data,
  markerEnd,
}: EdgeProps) {
  const { edgeKind = 'navigation', offsetPx = 0 } = (data || {}) as unknown as StraightEdgeData;

  let sx = sourceX;
  let sy = sourceY;
  let tx = targetX;
  let ty = targetY;

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
  const startId = rawNodes.find(n => n.is_start || n.id === 'Start' || n.label === 'Start')?.id;
  const specialSet = new Set<string>(
    rawNodes
      .filter(n => (n.is_special || n.is_metadata) && n.id !== startId)
      .map(n => n.id),
  );

  // ── Detect bidirectional pairs ─────────────────────────────────────────
  // For each edge A→B, record whether B→A also exists.
  const edgeSet = new Set<string>(rawEdges.map(e => `${e.source}→${e.target}`));
  const isBidir = (src: string, tgt: string) =>
    edgeSet.has(`${src}→${tgt}`) && edgeSet.has(`${tgt}→${src}`);
  // Track which bidir pairs we've already assigned an offset to
  const bidirFirst = new Set<string>();

  // ── Build passage nodes ────────────────────────────────────────────────
  const positioned: Node[] = [];
  const unpositioned: Node[] = [];
  const childToGroup = new Map<string, string>();

  for (const [gid, members] of groupMembers) {
    for (const mid of members) childToGroup.set(mid, gid);
  }

  for (const n of rawNodes) {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    const rfNode: Node<PassageNodeData> = {
      id: n.id,
      type: 'passage',
      position: { x: n.position_x != null ? snap(n.position_x) : 0, y: n.position_y != null ? snap(n.position_y) : 0 },
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

    if (n.position_x != null && n.position_y != null) {
      positioned.push(rfNode);
    } else {
      unpositioned.push(rfNode);
    }
  }

  // ── Layout unpositioned nodes with dagre ───────────────────────────────
  if (unpositioned.length > 0) {
    const nodeIds = new Set(unpositioned.map(n => n.id));
    const subEdges: Edge[] = rawEdges
      .filter(e => nodeIds.has(e.source) && nodeIds.has(e.target))
      .map((e, i) => ({ id: `dagre-${i}`, source: e.source, target: e.target }));

    const positions = dagreLayout(unpositioned, subEdges);

    // Offset so unpositioned block doesn't overlap positioned nodes
    let offsetX = 0;
    if (positioned.length > 0) {
      const maxX = Math.max(...positioned.map(n => n.position.x)) + NODE_W + GRID_SNAP * 4;
      const minY = Math.min(...positioned.map(n => n.position.y));
      offsetX = maxX;
      for (const n of unpositioned) {
        const p = positions.get(n.id);
        if (p) n.position = { x: p.x + offsetX, y: p.y + minY };
      }
    } else {
      for (const n of unpositioned) {
        const p = positions.get(n.id);
        if (p) n.position = p;
      }
    }
  }

  // All passage nodes combined
  const allPassageNodes = [...positioned, ...unpositioned];

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
  useEffect(() => {
    if (focusRequested <= 0 || !focusPassageName) return;
    skipViewportRestoreRef.current = true;

    // Find node by id, then by label
    const nds = nodesRef.current;
    let target = getNode(focusPassageName);
    if (!target) {
      const found = nds.find(n => n.type === 'passage' && (n.data as PassageNodeData).label === focusPassageName);
      if (found) target = getNode(found.id);
    }
    if (!target) return;

    fitView({ nodes: [{ id: target.id }], padding: 0.4, duration: 0 });

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
    }, 1800);
  }, [focusRequested, focusPassageName, fitView, getNode, setNodes]);

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
    if (updates.length > 0) vscode.postMessage({ command: 'saveAllPositions', updates });
  }, [saveRequested]);

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
          variant={BackgroundVariant.Dots}
          gap={GRID_SNAP}
          size={1}
          color="rgba(255,255,255,0.07)"
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