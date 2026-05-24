import React, { useCallback, useEffect, useMemo, useRef } from 'react';
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
  BaseEdge,
  EdgeProps,
  getSmoothStepPath,
  ReactFlowProvider,
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
const NODE_WIDTH = 150;
const NODE_HEIGHT = 36;
const MIN_ZOOM = 0.15;
const MAX_ZOOM = 4.0;

// ── Twine-inspired color palette ───────────────────────────────────────────

const COLORS = {
  normal: '#3a7ca5',
  start: '#43a047',
  special: '#ef6c00',
  metadata: '#8e24aa',
  unreachable: '#4a4a4a',
  broken: '#e53935',
  edgeNormal: '#7a8a9e',
  edgeUpstream: '#5c6370',
};

// ── Helpers ────────────────────────────────────────────────────────────────

function snapToGrid(value: number): number {
  return Math.round(value / GRID_SNAP) * GRID_SNAP;
}

function getNodeColor(node: KnotGraphNode): string {
  if (node.color) return node.color;
  if (node.is_unreachable) return COLORS.unreachable;
  if (node.is_start || node.id === 'Start') return COLORS.start;
  if (node.is_metadata) return COLORS.metadata;
  if (node.is_special) return COLORS.special;
  return COLORS.normal;
}

// ── Debounce utility ───────────────────────────────────────────────────────

function debounce<T extends (...args: any[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const debounced = (...args: Parameters<T>) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
  return debounced as T;
}

// ── Custom Node: Passage ───────────────────────────────────────────────────

function PassageNodeComponent({ data }: NodeProps<Node<PassageNodeData>>) {
  const d = data as PassageNodeData;
  const color = d.color || COLORS.normal;

  const nodeClassNames = [
    'passage-node',
    d.is_start && 'passage-node--start',
    d.is_special && 'passage-node--special',
    d.is_metadata && 'passage-node--metadata',
    d.is_unreachable && 'passage-node--unreachable',
    d.highlighted && 'passage-node--highlighted',
    d.dimmed && 'passage-node--dimmed',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div
      className={nodeClassNames}
      style={{
        backgroundColor: color,
        borderColor: d.is_unreachable ? '#3a3a3a' : color,
      }}
    >
      <Handle
        type="target"
        position={Position.Top}
        style={{ background: 'transparent', border: 'none', width: 8, height: 8 }}
      />
      <span className="passage-node__label" title={d.label}>
        {d.label}
      </span>
      <Handle
        type="source"
        position={Position.Bottom}
        style={{ background: 'transparent', border: 'none', width: 8, height: 8 }}
      />
    </div>
  );
}

// ── Custom Node: Group ─────────────────────────────────────────────────────

function GroupNodeComponent({ data }: NodeProps<Node<GroupNodeData>>) {
  const d = data as GroupNodeData;

  return (
    <div className="group-node">
      <div className="group-node__label">{d.label}</div>
    </div>
  );
}

// ── Custom Edges ───────────────────────────────────────────────────────────

function NavigationEdge(props: EdgeProps) {
  const [edgePath] = getSmoothStepPath({
    sourceX: props.sourceX,
    sourceY: props.sourceY,
    sourcePosition: props.sourcePosition,
    targetX: props.targetX,
    targetY: props.targetY,
    targetPosition: props.targetPosition,
    borderRadius: 8,
  });

  return (
    <BaseEdge
      id={props.id}
      path={edgePath}
      style={{
        stroke: COLORS.edgeNormal,
        strokeWidth: 1.5,
        opacity: 0.7,
      }}
      markerEnd="url(#arrowhead)"
    />
  );
}

function UpstreamEdge(props: EdgeProps) {
  const [edgePath] = getSmoothStepPath({
    sourceX: props.sourceX,
    sourceY: props.sourceY,
    sourcePosition: props.sourcePosition,
    targetX: props.targetX,
    targetY: props.targetY,
    targetPosition: props.targetPosition,
    borderRadius: 8,
  });

  return (
    <BaseEdge
      id={props.id}
      path={edgePath}
      style={{
        stroke: COLORS.edgeUpstream,
        strokeWidth: 1.5,
        strokeDasharray: '5 3',
        opacity: 0.4,
      }}
      markerEnd="url(#arrowhead-upstream)"
    />
  );
}

function BrokenEdge(props: EdgeProps) {
  const [edgePath] = getSmoothStepPath({
    sourceX: props.sourceX,
    sourceY: props.sourceY,
    sourcePosition: props.sourcePosition,
    targetX: props.targetX,
    targetY: props.targetY,
    targetPosition: props.targetPosition,
    borderRadius: 8,
  });

  return (
    <BaseEdge
      id={props.id}
      path={edgePath}
      style={{
        stroke: COLORS.broken,
        strokeWidth: 1.5,
        strokeDasharray: '6 3',
        opacity: 0.9,
      }}
      markerEnd="url(#arrowhead-broken)"
    />
  );
}

// ── Node type map ──────────────────────────────────────────────────────────

const nodeTypes = {
  passage: PassageNodeComponent,
  group: GroupNodeComponent,
};

const edgeTypes = {
  navigation: NavigationEdge,
  upstream: UpstreamEdge,
  broken: BrokenEdge,
};

// ── Dagre layout (fallback for unpositioned nodes) ─────────────────────────

function runDagreLayout(
  nodes: Node[],
  edges: Edge[],
): Map<string, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'TB', nodesep: 50, ranksep: 70, marginx: 40, marginy: 40 });

  for (const node of nodes) {
    const w = node.type === 'group' ? 300 : NODE_WIDTH;
    const h = node.type === 'group' ? 200 : NODE_HEIGHT;
    g.setNode(node.id, { width: w, height: h });
  }

  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  const positions = new Map<string, { x: number; y: number }>();
  for (const node of nodes) {
    const pos = g.node(node.id);
    if (pos) {
      positions.set(node.id, { x: snapToGrid(pos.x), y: snapToGrid(pos.y) });
    }
  }

  return positions;
}

// ── Build graph data into React Flow nodes and edges ───────────────────────

function buildGraphElements(
  data: KnotGraphResponse,
): { nodes: Node[]; edges: Edge[] } {
  const rawNodes = Array.isArray(data?.nodes) ? data.nodes : [];
  const rawEdges = Array.isArray(data?.edges) ? data.edges : [];

  // Collect groups
  const groupMap = new Map<string, { label: string; members: string[] }>();
  for (const n of rawNodes) {
    if (n.group) {
      if (!groupMap.has(n.group)) {
        groupMap.set(n.group, { label: n.group, members: [] });
      }
      groupMap.get(n.group)!.members.push(n.id);
    }
  }

  // Only use EXPLICIT groups from passage metadata. No auto-grouping.
  // Unreachable passages should NOT be lumped into a "Special Passages"
  // group — that was confusing and hid real structural issues.

  // Identify start ID for edge suppression
  const startId = rawNodes.find(
    (n) => n.is_start || n.id === 'Start' || n.label === 'Start',
  )?.id;

  // Build set of special passage IDs for edge suppression
  const specialSet = new Set<string>();
  for (const n of rawNodes) {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    if ((n.is_special || n.is_metadata) && !isStart) {
      specialSet.add(n.id);
    }
  }

  // ── Create group nodes ──────────────────────────────────────────────
  const rfNodes: Node[] = [];
  const groupChildToParent = new Map<string, string>();

  for (const [groupId, group] of groupMap) {
    for (const memberId of group.members) {
      groupChildToParent.set(memberId, groupId);
    }

    rfNodes.push({
      id: groupId,
      type: 'group',
      position: { x: 0, y: 0 },
      data: { label: group.label } as GroupNodeData,
      style: {
        width: 300,
        height: 200,
      },
      draggable: false,
      selectable: false,
    });
  }

  // ── Create passage nodes ────────────────────────────────────────────
  const positionedNodes: Node[] = [];
  const unpositionedNodes: Node[] = [];

  for (const n of rawNodes) {
    const isStart = n.is_start || n.id === 'Start' || n.label === 'Start';
    const color = getNodeColor(n);
    const parentId = groupChildToParent.get(n.id);

    let posX = n.position_x != null ? snapToGrid(n.position_x) : null;
    let posY = n.position_y != null ? snapToGrid(n.position_y) : null;

    const rfNode: Node<PassageNodeData> = {
      id: n.id,
      type: 'passage',
      position: { x: posX ?? 0, y: posY ?? 0 },
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
        color,
        metadata_color: n.color,  // only the server-provided metadata color
        var_writes: n.var_writes || [],
        var_reads: n.var_reads || [],
        group: n.group,
        dimmed: false,
        highlighted: false,
      },
      parentId,
    };

    if (posX != null && posY != null) {
      positionedNodes.push(rfNode);
    } else {
      unpositionedNodes.push(rfNode);
    }

    rfNodes.push(rfNode);
  }

  // ── Layout unpositioned nodes with dagre ────────────────────────────
  if (unpositionedNodes.length > 0) {
    // Build edges for the unpositioned subgraph
    const nodeIds = new Set(unpositionedNodes.map((n) => n.id));
    const subEdges: Edge[] = rawEdges
      .filter((e) => nodeIds.has(e.source) && nodeIds.has(e.target))
      .map((e) => ({
        id: `${e.source}->${e.target}`,
        source: e.source,
        target: e.target,
      }));

    const dagrePositions = runDagreLayout(unpositionedNodes, subEdges);

    // If there are also positioned nodes, offset the dagre layout so it
    // doesn't overlap with the positioned nodes
    if (positionedNodes.length > 0) {
      let maxX = -Infinity;
      let minY = Infinity;
      for (const pn of positionedNodes) {
        if (pn.position.x > maxX) maxX = pn.position.x;
        if (pn.position.y < minY) minY = pn.position.y;
      }
      const offsetX = maxX + NODE_WIDTH + GRID_SNAP * 3;
      const offsetY = minY;

      for (const un of unpositionedNodes) {
        const pos = dagrePositions.get(un.id);
        if (pos) {
          un.position = {
            x: pos.x + offsetX,
            y: pos.y + offsetY,
          };
        }
      }
    } else {
      // All nodes are unpositioned — just use dagre positions directly
      for (const un of unpositionedNodes) {
        const pos = dagrePositions.get(un.id);
        if (pos) {
          un.position = { x: pos.x, y: pos.y };
        }
      }
    }
  }

  // ── Position group nodes to encompass their children ────────────────
  for (const [groupId, group] of groupMap) {
    const childNodes = rfNodes.filter(
      (n) => groupChildToParent.get(n.id) === groupId,
    );
    if (childNodes.length === 0) continue;

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;

    for (const cn of childNodes) {
      minX = Math.min(minX, cn.position.x - NODE_WIDTH / 2);
      minY = Math.min(minY, cn.position.y - NODE_HEIGHT / 2);
      maxX = Math.max(maxX, cn.position.x + NODE_WIDTH / 2);
      maxY = Math.max(maxY, cn.position.y + NODE_HEIGHT / 2);
    }

    const padding = 30;
    const groupNode = rfNodes.find((n) => n.id === groupId);
    if (groupNode) {
      groupNode.position = {
        x: snapToGrid(minX - padding),
        y: snapToGrid(minY - padding - 20), // extra top padding for label
      };
      groupNode.style = {
        width: snapToGrid(maxX - minX + padding * 2),
        height: snapToGrid(maxY - minY + padding * 2 + 20),
      };
    }

    // Offset child positions relative to group
    if (groupNode) {
      const gx = groupNode.position.x;
      const gy = groupNode.position.y;
      for (const cn of childNodes) {
        // Children with parentId are positioned absolutely in React Flow
        // unless the group is a subflow. We use absolute positioning.
      }
    }
  }

  // ── Create edges ────────────────────────────────────────────────────
  const nodeIds = new Set(rawNodes.map((n) => n.id));
  const rfEdges: Edge[] = [];
  const usedEdgeIds = new Set<string>();
  let edgeIndex = 0;

  for (const e of rawEdges) {
    // Skip edges referencing missing nodes
    if (!nodeIds.has(e.source) || !nodeIds.has(e.target)) {
      continue;
    }

    // Edge suppression:
    // 1. No edges between special passages within the same group
    // 2. No edges from special group members to Start
    if (specialSet.has(e.source) && specialSet.has(e.target)) {
      continue;
    }
    if (specialSet.has(e.source) && e.target === startId) {
      continue;
    }

    let edgeId = `${e.source}->${e.target}[${e.edge_type || 'nav'}]`;
    if (usedEdgeIds.has(edgeId)) {
      edgeId = `${e.source}->${e.target}[${e.edge_type || 'nav'}_${edgeIndex}]`;
    }
    usedEdgeIds.add(edgeId);
    edgeIndex++;

    // Determine edge type for rendering
    let edgeType = 'navigation';
    if (e.edge_type === 'upstream') {
      edgeType = 'upstream';
    } else if (e.edge_type === 'broken') {
      edgeType = 'broken';
    }

    rfEdges.push({
      id: edgeId,
      source: e.source,
      target: e.target,
      type: edgeType,
      data: {
        displayText: e.display_text || null,
      },
    });
  }

  return { nodes: rfNodes, edges: rfEdges };
}

// ── SVG marker definitions for arrowheads ──────────────────────────────────

function ArrowMarkers() {
  return (
    <defs>
      <marker
        id="arrowhead"
        markerWidth="10"
        markerHeight="7"
        refX="9"
        refY="3.5"
        orient="auto"
        markerUnits="strokeWidth"
      >
        <polygon
          points="0 0, 10 3.5, 0 7"
          fill={COLORS.edgeNormal}
          opacity="0.7"
        />
      </marker>
      <marker
        id="arrowhead-upstream"
        markerWidth="10"
        markerHeight="7"
        refX="9"
        refY="3.5"
        orient="auto"
        markerUnits="strokeWidth"
      >
        <polygon
          points="0 0, 10 3.5, 0 7"
          fill={COLORS.edgeUpstream}
          opacity="0.4"
        />
      </marker>
      <marker
        id="arrowhead-broken"
        markerWidth="10"
        markerHeight="7"
        refX="9"
        refY="3.5"
        orient="auto"
        markerUnits="strokeWidth"
      >
        <polygon points="0 0, 10 3.5, 0 7" fill={COLORS.broken} opacity="0.9" />
      </marker>
    </defs>
  );
}

// ── Inner StoryMap component (needs ReactFlow context) ─────────────────────

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

  const initialFitDoneRef = useRef(false);
  const savedViewportRef = useRef<{ x: number; y: number; zoom: number } | null>(null);
  const graphDataRef = useRef<KnotGraphResponse | null>(null);
  // When focusNode is active, skip viewport restoration on the next graph
  // rebuild so the user's manual panning isn't overridden by the saved
  // viewport from before the focus action.
  const skipViewportRestoreRef = useRef(false);

  // ── Debounced position update ───────────────────────────────────────
  const debouncedPositionUpdate = useMemo(
    () =>
      debounce((updates: KnotPositionUpdate[]) => {
        if (updates.length > 0) {
          vscode.postMessage({ command: 'updatePositions', updates });
        }
      }, 150),
    [],
  );

  // ── Debounced viewport update ───────────────────────────────────────
  const debouncedViewportUpdate = useMemo(
    () =>
      debounce((x: number, y: number, zoom: number) => {
        vscode.postMessage({ command: 'updateViewport', x, y, zoom });
      }, 500),
    [],
  );

  // ── Build graph when data changes ───────────────────────────────────
  useEffect(() => {
    if (!graphData) return;

    // Save current viewport before rebuild
    try {
      const vp = getViewport();
      savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
    } catch {
      // React Flow may not be ready yet
    }

    graphDataRef.current = graphData;
    const { nodes: newNodes, edges: newEdges } = buildGraphElements(graphData);

    setNodes(newNodes);
    setEdges(newEdges);

    // Fit view on first load
    if (!initialFitDoneRef.current && newNodes.length > 0) {
      initialFitDoneRef.current = true;
      // Defer fitView so React Flow can measure the nodes first
      requestAnimationFrame(() => {
        fitView({ padding: 0.15, duration: 400 });
      });
    } else if (skipViewportRestoreRef.current) {
      // Don't restore viewport — a focusNode action set this flag.
      // The user should be free to pan/zoom after the focus completes.
      skipViewportRestoreRef.current = false;
    } else if (savedViewportRef.current) {
      // Restore previous viewport on subsequent updates
      requestAnimationFrame(() => {
        setViewport(savedViewportRef.current!, { duration: 0 });
      });
    }
  }, [graphData, setNodes, setEdges, fitView, setViewport, getViewport]);

  // ── Search / filter ─────────────────────────────────────────────────
  useEffect(() => {
    const q = searchQuery.toLowerCase().trim();

    setNodes((nds: Node[]) =>
      nds.map((n: Node) => {
        // Skip group nodes
        if (n.type === 'group') return n;

        const d = n.data as PassageNodeData;

        if (q === '') {
          return {
            ...n,
            data: { ...d, dimmed: false, highlighted: false },
          };
        }

        const label = (d.label || '').toLowerCase();
        const tags: string[] = d.tags || [];
        const matches =
          label.includes(q) || tags.some((t) => t.toLowerCase().includes(q));

        return {
          ...n,
          data: {
            ...d,
            dimmed: !matches,
            highlighted: matches,
          },
        };
      }),
    );

    // Dim non-matching edges
    setEdges((eds: Edge[]) => {
      if (q === '') {
        return eds.map((e: Edge) => ({ ...e, style: undefined }));
      }

      // Find matching node IDs
      const matchIds = new Set<string>();
      const nds = nodes; // read current nodes
      for (const n of nds) {
        if (n.type === 'group') continue;
        const d = n.data as PassageNodeData;
        const label = (d.label || '').toLowerCase();
        const tags: string[] = d.tags || [];
        if (label.includes(q) || tags.some((t) => t.toLowerCase().includes(q))) {
          matchIds.add(n.id);
        }
      }

      return eds.map((e: Edge) => {
        const connected = matchIds.has(e.source) || matchIds.has(e.target);
        return {
          ...e,
          style: connected ? undefined : { opacity: 0.08 },
        };
      });
    });
  }, [searchQuery, setNodes, setEdges, nodes]);

  // ── Fit to view ─────────────────────────────────────────────────────
  useEffect(() => {
    if (fitRequested > 0) {
      fitView({ padding: 0.15, duration: 300 });
    }
  }, [fitRequested, fitView]);

  // ── Focus on a passage node ─────────────────────────────────────────
  useEffect(() => {
    if (focusRequested <= 0 || !focusPassageName) return;

    // Prevent viewport restoration from overriding the focus pan
    skipViewportRestoreRef.current = true;

    // Try to find node by ID, then by label
    let targetNode = getNode(focusPassageName);
    if (!targetNode) {
      const nds = nodes;
      const found = nds.find((n: Node) => {
        if (n.type === 'group') return false;
        const d = n.data as PassageNodeData;
        return d.label === focusPassageName;
      });
      if (found) {
        targetNode = getNode(found.id);
      }
    }

    if (targetNode) {
      // Instant pan to the focused node — no animation duration.
      // Animated pans feel sluggish and prevent the user from scrolling
      // freely after navigation (the animation fights with manual input).
      fitView({ nodes: [{ id: targetNode.id }], padding: 0.5, duration: 0 });

      // Temporarily highlight the focused node
      setNodes((nds: Node[]) =>
        nds.map((n: Node) => {
          if (n.id === targetNode!.id) {
            const d = n.data as PassageNodeData;
            return {
              ...n,
              className: 'passage-node--focused',
              data: { ...d, highlighted: true },
            };
          }
          return n;
        }),
      );

      // Remove highlight after 1.5s (shorter to feel responsive)
      setTimeout(() => {
        setNodes((nds: Node[]) =>
          nds.map((n: Node) => {
            if (n.id === targetNode!.id) {
              const d = n.data as PassageNodeData;
              return {
                ...n,
                className: undefined,
                data: { ...d, highlighted: searchQuery ? true : false },
              };
            }
            return n;
          }),
        );
      }, 1500);
    }
  }, [focusRequested, focusPassageName, fitView, getNode, nodes, setNodes, searchQuery]);

  // ── Save all positions ──────────────────────────────────────────────
  useEffect(() => {
    if (saveRequested <= 0) return;

    const updates: KnotPositionUpdate[] = [];
    const currentNodes = nodes;

    for (const n of currentNodes) {
      if (n.type === 'group') continue;
      const d = n.data as PassageNodeData;
      updates.push({
        passage_name: n.id,
        position_x: snapToGrid(n.position.x),
        position_y: snapToGrid(n.position.y),
        group: d.group,
        color: d.metadata_color,  // only write back metadata color, never rendering fallback
      });
    }

    if (updates.length > 0) {
      vscode.postMessage({ command: 'saveAllPositions', updates });
    }
  }, [saveRequested, nodes]);

  // ── Handle node changes (drag end → snap to grid + update positions) ─
  const handleNodesChange = useCallback(
    (changes: NodeChange[]) => {
      // Apply all changes first
      onNodesChange(changes);

      // Process position changes (drag end)
      const positionChanges = changes.filter(
        (c): c is NodePositionChange =>
          c.type === 'position' && c.dragging === false,
      );

      if (positionChanges.length === 0) return;

      // Snap to grid and collect updates
      const updates: KnotPositionUpdate[] = [];
      const snappedPositions = new Map<string, { x: number; y: number }>();

      for (const change of positionChanges) {
        if (change.position) {
          const snappedX = snapToGrid(change.position.x);
          const snappedY = snapToGrid(change.position.y);
          snappedPositions.set(change.id, { x: snappedX, y: snappedY });
        }
      }

      if (snappedPositions.size === 0) return;

      // Apply snapped positions
      setNodes((nds: Node[]) =>
        nds.map((n: Node) => {
          const snapped = snappedPositions.get(n.id);
          if (snapped) {
            const d = n.data as PassageNodeData;
            updates.push({
              passage_name: n.id,
              position_x: snapped.x,
              position_y: snapped.y,
              group: d.group,
              color: d.metadata_color,  // only write back metadata color, never rendering fallback
            });
            return { ...n, position: { x: snapped.x, y: snapped.y } };
          }
          return n;
        }),
      );

      debouncedPositionUpdate(updates);
    },
    [onNodesChange, setNodes, debouncedPositionUpdate],
  );

  // ── Node click → open passage ───────────────────────────────────────
  const handleNodeClick = useCallback(
    (_event: React.MouseEvent, node: Node) => {
      if (node.type === 'group') return;
      const d = node.data as PassageNodeData;
      if (d.file) {
        vscode.postMessage({
          command: 'openPassage',
          file: d.file,
          line: d.line || 0,
        });
      }
    },
    [],
  );

  // ── Viewport change → save debounced ────────────────────────────────
  const handleViewportChange = useCallback(() => {
    try {
      const vp = getViewport();
      debouncedViewportUpdate(vp.x, vp.y, vp.zoom);
    } catch {
      // React Flow may not be ready
    }
  }, [getViewport, debouncedViewportUpdate]);

  // ── MiniMap node color ──────────────────────────────────────────────
  const miniMapNodeColor = useCallback((node: Node) => {
    if (node.type === 'group') return '#333344';
    const d = node.data as PassageNodeData;
    return d.color || COLORS.normal;
  }, []);

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      onNodesChange={handleNodesChange}
      onEdgesChange={onEdgesChange}
      onNodeClick={handleNodeClick}
      onViewportChange={handleViewportChange}
      nodeTypes={nodeTypes}
      edgeTypes={edgeTypes}
      connectionMode={ConnectionMode.Loose}
      minZoom={MIN_ZOOM}
      maxZoom={MAX_ZOOM}
      fitView={false}
      panOnDrag={true}
      selectionOnDrag={false}
      nodesDraggable={true}
      nodesConnectable={false}
      elementsSelectable={true}
      proOptions={{ hideAttribution: true }}
    >
      <ArrowMarkers />
      <MiniMap
        nodeColor={miniMapNodeColor}
        maskColor="rgba(0, 0, 0, 0.6)"
        style={{
          backgroundColor: 'var(--vscode-sideBar-background, #252536)',
          border: '1px solid var(--vscode-panel-border, #3a3a4a)',
          borderRadius: '4px',
        }}
      />
      <Controls
        showInteractive={false}
      />
      <Background variant={BackgroundVariant.Dots} gap={GRID_SNAP} size={1} color="rgba(255,255,255,0.09)" />
    </ReactFlow>
  );
}

// ── Outer StoryMap component (provides ReactFlowProvider) ──────────────────

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
