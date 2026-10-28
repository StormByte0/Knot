import React, {
  createContext,
  useCallback,
  useContext,
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
  EdgeKind,
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
// Grace (px) around a group box for drag-out membership removal.
const GROUP_ESCAPE_GRACE = 12;

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
  ghost:      '#f87171',
  edgeNav:    '#8494ab',   // [[Target]] — player-choice navigation (slate)
  edgeJump:   '#a78bfa',   // <<goto>> — unconditional redirect (violet)
  edgeInclude:'#60a5fa',   // <<include>> — inline inclusion (sky)
  edgeUp:     '#64748b',   // Special passage lifecycle chain (slate dark)
  edgeBroken: '#f87171',   // Target passage doesn't exist
  edgeManual: '#f0b429',   // `reachable` metadata — manual reachability link (gold)
};

// ── Helpers ────────────────────────────────────────────────────────────────

/** Snap to the 20px grid, or pass through (rounded to 1px) when the
 *  snap-to-grid toggle is off. */
function snapTo(v: number, enabled: boolean) {
  return enabled ? Math.round(v / GRID_SNAP) * GRID_SNAP : Math.round(v);
}

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

// ── Group actions context ──────────────────────────────────────────────────
//
// Group container nodes need to drive canvas-level operations (move-as-unit
// drags, rename commits) that live in StoryMapInner. A React context keeps
// the callbacks out of node data (functions in data break memo comparisons).

interface GroupActions {
  /** Pointer-down on a group's title bar: begin a move-as-unit drag. */
  beginGroupDrag: (groupId: string, e: React.PointerEvent) => void;
  /** Pointer-move while dragging a group: translate members + box. */
  dragGroupBy: (groupId: string, dx: number, dy: number) => void;
  /** Pointer-up: persist every member's new position (one batched write). */
  endGroupDrag: (groupId: string) => void;
  /** Commit a group rename (dblclick on the title label). */
  commitGroupRename: (groupId: string, newName: string) => void;
}

const GroupActionsContext = createContext<GroupActions | null>(null);

// ── Custom Node: Passage ───────────────────────────────────────────────────
//
// Compact single-row Twine-style chip: the whole card carries the passage
// color (with a subtle top-light gradient), the name, and status badges.
// Double-clicking the LABEL starts an in-canvas rename; clicking a GHOST
// (broken-link placeholder) opens the create-passage flow.

function PassageNode({ id, data }: NodeProps<Node<PassageNodeData>>) {
  const d = data as PassageNodeData;
  const color = d.color || COLORS.normal;
  const [renaming, setRenaming] = useState(false);
  const [renameDraft, setRenameDraft] = useState('');
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (renaming) {
      setRenameDraft(d.label);
      requestAnimationFrame(() => {
        renameInputRef.current?.focus();
        renameInputRef.current?.select();
      });
    }
  }, [renaming, d.label]);

  const commitRename = () => {
    const trimmed = renameDraft.trim();
    setRenaming(false);
    if (!trimmed || trimmed === d.label) return;
    vscode.postMessage({ command: 'renamePassage', name: d.label, new_name: trimmed });
  };

  const cls = [
    'pn',
    d.is_start       && 'pn--start',
    d.is_special     && 'pn--special',
    d.is_metadata    && 'pn--metadata',
    d.is_unreachable && 'pn--unreachable',
    d.is_entry       && 'pn--entry',
    d.is_dead_end    && 'pn--dead-end',
    d.is_placeholder && 'pn--ghost',
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
      {renaming ? (
        <input
          ref={renameInputRef}
          className="pn__rename-input"
          value={renameDraft}
          onChange={e => setRenameDraft(e.target.value)}
          onKeyDown={e => {
            e.stopPropagation();
            if (e.key === 'Enter') { e.preventDefault(); commitRename(); }
            if (e.key === 'Escape') { e.preventDefault(); setRenaming(false); }
          }}
          onBlur={commitRename}
          onMouseDown={e => e.stopPropagation()}
          onDoubleClick={e => e.stopPropagation()}
          spellCheck={false}
        />
      ) : (
        <span
          className="pn__label"
          title={d.is_placeholder
            ? `${d.label} — missing passage (click to create)`
            : `${d.label} (double-click to rename)`}
          onDoubleClick={e => {
            if (d.is_placeholder) return; // ghosts rename via creation
            e.stopPropagation();
            setRenaming(true);
          }}
        >
          {d.label}
        </span>
      )}
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
        {d.is_placeholder && (
          <span className="pn__badge pn__badge--ghost" title="Missing passage — click to create it">
            <svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true">
              <path d="M6 1.5 V10.5 M1.5 6 H10.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
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
//
// Real groups are interactive: dragging the TITLE BAR moves every member as
// a unit (one batched position write on release); double-clicking the label
// renames the group (a bulk metadata update on all members). Read-only
// zones (specials / unreachable) render the same box without interactions.

function GroupNode({ id, data }: NodeProps<Node<GroupNodeData>>) {
  const d = data as GroupNodeData;
  const actions = useContext(GroupActionsContext);
  const [renaming, setRenaming] = useState(false);
  const [renameDraft, setRenameDraft] = useState('');
  const renameInputRef = useRef<HTMLInputElement>(null);
  const readOnly = !!d.readOnly;

  useEffect(() => {
    if (renaming) {
      setRenameDraft(d.label);
      requestAnimationFrame(() => renameInputRef.current?.focus());
    }
  }, [renaming, d.label]);

  const commitRename = () => {
    const trimmed = renameDraft.trim();
    setRenaming(false);
    if (!trimmed || trimmed === d.label) return;
    actions?.commitGroupRename(id, trimmed);
  };

  const onPointerDown = (e: React.PointerEvent) => {
    if (readOnly || renaming) return;
    e.stopPropagation();
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    actions?.beginGroupDrag(id, e);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (readOnly || renaming) return;
    if (e.buttons === 0) return;
    actions?.dragGroupBy(id, e.movementX, e.movementY);
  };
  const onPointerUp = (e: React.PointerEvent) => {
    if (readOnly) return;
    try { (e.target as HTMLElement).releasePointerCapture(e.pointerId); } catch { /* gone */ }
    actions?.endGroupDrag(id);
  };

  return (
    <div className={`gn${readOnly ? ' gn--zone' : ' gn--interactive'}`}>
      <div
        className="gn__label"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onDoubleClick={e => {
          if (readOnly) return;
          e.stopPropagation();
          setRenaming(true);
        }}
        title={readOnly
          ? d.label
          : `${d.label} — drag title to move the group · double-click to rename · right-click for actions`}
      >
        {renaming ? (
          <input
            ref={renameInputRef}
            className="gn__rename-input"
            value={renameDraft}
            onChange={e => setRenameDraft(e.target.value)}
            onKeyDown={e => {
              e.stopPropagation();
              if (e.key === 'Enter') { e.preventDefault(); commitRename(); }
              if (e.key === 'Escape') { e.preventDefault(); setRenaming(false); }
            }}
            onBlur={commitRename}
            onMouseDown={e => e.stopPropagation()}
            onPointerDown={e => e.stopPropagation()}
            spellCheck={false}
          />
        ) : (
          d.label
        )}
        {!readOnly && <span className="gn__count">{d.memberIds.length}</span>}
      </div>
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
  edgeKind: EdgeKind;
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
    // <<goto>> — unconditional redirect, semantically distinct from a
    // player-choice link (see core EdgeType::Jump).
    stroke = COLORS.edgeJump;
    strokeDash = '8 4';
    opacity = 0.62;
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
    if (p) out.set(n.id, { x: p.x - NODE_W / 2, y: p.y - NODE_H / 2 });
  }
  return out;
}

// ── Build React Flow elements from KnotGraphResponse ──────────────────────

function buildElements(
  data: KnotGraphResponse,
  hiddenEdgeKinds: Set<EdgeKind>,
  sessionPositions?: ReadonlyMap<string, { x: number; y: number }>,
): { nodes: Node[]; edges: Edge[] } {
  const rawNodes = Array.isArray(data?.nodes) ? data.nodes : [];
  const rawEdges = Array.isArray(data?.edges) ? data.edges : [];

  const groupMembers = new Map<string, string[]>();
  for (const n of rawNodes) {
    if (n.group && !n.is_placeholder) {
      if (!groupMembers.has(n.group)) groupMembers.set(n.group, []);
      groupMembers.get(n.group)!.push(n.id);
    }
  }

  const startNode = rawNodes.find(n => n.is_start);
  const startId = startNode?.id;
  const specialSet = new Set<string>(
    rawNodes
      .filter(n => (n.is_special || n.is_metadata) && n.id !== startId && !n.is_placeholder)
      .map(n => n.id),
  );
  const unreachableSet = new Set<string>(
    rawNodes.filter(n => n.is_unreachable && !n.is_placeholder).map(n => n.id),
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
  const placeholderNodes: KnotGraphNode[] = [];

  for (const n of rawNodes) {
    if (n.is_placeholder) { placeholderNodes.push(n); continue; }
    if (n.is_start)                { /* startNode is already captured above; skip */ }
    else if (specialSet.has(n.id)) specialNodes.push(n);
    else if (unreachableSet.has(n.id)) unreachableNodes.push(n);
    else                           regularNodes.push(n);
  }

  // Stored positions only exist for REAL passages — placeholders never
  // have header metadata — so the position partition below works on the
  // real set.
  const realNodes = rawNodes.filter(n => !n.is_placeholder);

  // ── Position precedence (per node) ─────────────────────────────────
  // Stored header/StoryData metadata ("position":"x,y") wins for EACH
  // passage individually: a passage the user positioned is an anchor.
  // Only passages with NO stored position fall back to the algorithmic
  // layout. The old all-or-nothing check (`every` node had a position)
  // threw away every saved coordinate whenever a single passage lacked
  // one — dragging a node and refreshing snapped it back to the dagre
  // spot. `sessionPositions` (nodes dragged THIS session whose header
  // write may still be in flight) anchor too, so a rebuild racing the
  // position write can't yank them either.
  const positionOf = (n: KnotGraphNode): { x: number; y: number } | null => {
    if (n.position_x != null && n.position_y != null) {
      return { x: n.position_x, y: n.position_y };
    }
    return sessionPositions?.get(n.id) ?? null;
  };

  // Id → raw export node (for positionOf lookups on placed nodes).
  const rawById = new Map(rawNodes.map(n => [n.id, n] as const));

  const fixedPlacements: Array<{ n: KnotGraphNode; x: number; y: number }> = [];
  for (const n of realNodes) {
    const p = positionOf(n);
    if (p) fixedPlacements.push({ n, x: p.x, y: p.y });
  }
  const startIsFloating = !!startNode && !positionOf(startNode);

  function makePassageNode(n: KnotGraphNode, x: number, y: number): Node<PassageNodeData> {
    const groupId = childToGroup.get(n.id);
    const isDeadEnd =
      (n.out_degree || 0) === 0 &&
      !n.is_start && !n.is_special && !n.is_metadata && !n.is_unreachable &&
      !n.is_placeholder;
    return {
      id: n.id,
      type: 'passage',
      position: { x: Math.round(x), y: Math.round(y) },
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
        is_placeholder: !!n.is_placeholder,
        is_entry: !!n.is_entry && !n.is_start,
        manual_link_source: n.manual_link_source,
        is_dead_end: isDeadEnd,
        color: n.is_placeholder ? COLORS.ghost : nodeColor(n),
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

  if (fixedPlacements.length === realNodes.length && realNodes.length > 0) {
    // ── Case A: every real passage has a stored position ───────────
    for (const fp of fixedPlacements) {
      allPassageNodes.push(makePassageNode(fp.n, fp.x, fp.y));
    }
  } else if (fixedPlacements.length === 0) {
    // ── Case B: nothing positioned yet — pure algorithmic layout ───
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
            x: Math.round(p.x + START_ANCHOR_X + NODE_W + GRID_SNAP * 3),
            y: Math.round(p.y + START_ANCHOR_Y),
          };
        }
      }
      allPassageNodes.push(...dagreNodes);
    }
  } else {
    // ── Case C: mixed — some passages positioned, some not ─────────
    // Anchored passages keep their exact coordinates; the rest of the
    // story is auto-laid-out AROUND them: metadata/unreachable strays
    // park in a column below the anchored cluster, and the floating
    // flow subgraph dagre's itself into the free space to the right.
    for (const fp of fixedPlacements) {
      allPassageNodes.push(makePassageNode(fp.n, fp.x, fp.y));
    }

    // Bounding box of the anchored cluster (footprints incl. sizes).
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const fp of fixedPlacements) {
      const w = fp.n.size_w ?? NODE_W;
      const h = fp.n.size_h ?? NODE_H;
      minX = Math.min(minX, fp.x);
      minY = Math.min(minY, fp.y);
      maxX = Math.max(maxX, fp.x + w);
      maxY = Math.max(maxY, fp.y + h);
    }

    // Metadata / unreachable strays that lack positions: column below
    // the anchored cluster (same visual language as Case B's columns).
    const strays = specialNodes
      .filter(n => !positionOf(n))
      .sort((a, b) => {
        if (a.is_metadata && !b.is_metadata) return -1;
        if (!a.is_metadata && b.is_metadata) return 1;
        return a.label.localeCompare(b.label);
      });
    const strayUnreachables = unreachableNodes
      .filter(n => !positionOf(n))
      .sort((a, b) => a.label.localeCompare(b.label));
    let strayY = maxY + GRID_SNAP * 4;
    for (const n of strays) {
      allPassageNodes.push(makePassageNode(n, minX, strayY));
      strayY += NODE_H + GRID_SNAP;
    }
    for (const n of strayUnreachables) {
      allPassageNodes.push(makePassageNode(n, minX, strayY));
      strayY += NODE_H + GRID_SNAP;
    }

    // Floating flow passages (start included when it has no stored
    // position): dagre among themselves, docked right of the cluster.
    const flowNodes = regularNodes.filter(n => !positionOf(n));
    if (startIsFloating && startNode) flowNodes.unshift(startNode);
    if (flowNodes.length > 0) {
      const flowIds = new Set(flowNodes.map(n => n.id));
      const subEdges: Edge[] = rawEdges
        .filter(e => flowIds.has(e.source) && flowIds.has(e.target))
        .map((e, i) => ({ id: `dagre-mixed-${i}`, source: e.source, target: e.target }));

      const dagreNodes: Node[] = flowNodes.map(n => makePassageNode(n, 0, 0));
      const positions = dagreLayout(dagreNodes, subEdges, 'LR');

      // Normalize dagre output to a (0,0)-based bbox, then translate it
      // into the free space to the right of the anchored cluster.
      let dMinX = Infinity, dMinY = Infinity;
      for (const p of positions.values()) {
        dMinX = Math.min(dMinX, p.x);
        dMinY = Math.min(dMinY, p.y);
      }
      const baseX = maxX + NODE_W + GRID_SNAP * 3;
      const baseY = minY;
      for (const n of dagreNodes) {
        const p = positions.get(n.id);
        if (p) {
          n.position = {
            x: Math.round(p.x - dMinX + baseX),
            y: Math.round(p.y - dMinY + baseY),
          };
        }
      }
      allPassageNodes.push(...dagreNodes);
    }
  }

  // ── Ghost nodes (broken-link placeholders) ───────────────────────────
  // A ghost parks next to the first incoming edge's source when that
  // source has a position, so the broken arrow is short and readable.
  // Sources without positions (auto-layout mode) fall back to a column.
  {
    const posById = new Map<string, { x: number; y: number }>();
    for (const n of allPassageNodes) {
      posById.set(n.id, n.position);
    }
    const widthById = new Map<string, number>();
    for (const n of rawNodes) {
      if (n.size_w) widthById.set(n.id, n.size_w);
    }

    placeholderNodes.sort((a, b) => a.label.localeCompare(b.label));
    for (let i = 0; i < placeholderNodes.length; i++) {
      const ph = placeholderNodes[i];
      let x: number | null = null;
      let y: number | null = null;
      for (const e of rawEdges) {
        if (e.target !== ph.id) continue;
        const srcPos = posById.get(e.source);
        if (srcPos) {
          x = srcPos.x + (widthById.get(e.source) ?? NODE_W) + 60;
          y = srcPos.y;
          break;
        }
      }
      if (x == null || y == null) {
        // Fallback column for ghosts whose sources lack positions.
        x = UNREACHABLE_LIST_X;
        y = START_ANCHOR_Y + 120 + i * (NODE_H + GRID_SNAP);
      }
      allPassageNodes.push(makePassageNode(ph, x, y));
    }
  }

  // ── Group container nodes (interactive bounding boxes) ───────────────
  // Groups stay overlay containers (no parentId — global coordinates keep
  // the twee-header storage model intact). The box geometry recomputes
  // live while members drag (see the bounds-sync effect in StoryMapInner).
  // Real groups carry memberIds and get full interactions; the specials /
  // unreachable zones are read-only variants of the same component.
  const groupNodes: Node[] = [];
  const nodeWidth = (n: Node) => ((n.data as PassageNodeData).size_w ?? NODE_W);
  const nodeHeight = (n: Node) => ((n.data as PassageNodeData).size_h ?? NODE_H);

  for (const [gid, members] of groupMembers) {
    const children = allPassageNodes.filter(n => members.includes(n.id));
    if (children.length === 0) continue;

    const pad = 26;
    const xs = children.map(n => n.position.x);
    const ys = children.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - 16;
    const maxX = Math.max(...xs, ...children.map(n => n.position.x + nodeWidth(n))) + pad;
    const maxY = Math.max(...ys, ...children.map(n => n.position.y + nodeHeight(n))) + pad;

    groupNodes.push({
      id: gid,
      type: 'group',
      position: { x: Math.round(minX), y: Math.round(minY) },
      data: { label: gid, memberIds: [...members] } as GroupNodeData,
      style: { width: Math.round(maxX - minX), height: Math.round(maxY - minY) },
      draggable: false,
      selectable: false,
      zIndex: -1,
    });
  }

  const specialRfNodes = allPassageNodes.filter(
    n => n.type === 'passage'
      && specialSet.has(n.id)
      && !positionOf(rawById.get(n.id)!),
  );
  if (specialRfNodes.length > 0) {
    const pad = 24;
    const labelH = 20;
    const xs = specialRfNodes.map(n => n.position.x);
    const ys = specialRfNodes.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - labelH;
    const maxX = Math.max(...xs, ...specialRfNodes.map(n => n.position.x + nodeWidth(n))) + pad;
    const maxY = Math.max(...ys, ...specialRfNodes.map(n => n.position.y + nodeHeight(n))) + pad;

    groupNodes.push({
      id: '__specials__',
      type: 'group',
      position: { x: Math.round(minX), y: Math.round(minY) },
      data: { label: 'Special Passages', memberIds: [], readOnly: true } as GroupNodeData,
      style: {
        width: Math.round(maxX - minX),
        height: Math.round(maxY - minY),
        background: 'rgba(217, 123, 63, 0.045)',
        borderColor: 'rgba(217, 123, 63, 0.22)',
      },
      draggable: false,
      selectable: false,
      zIndex: -2,
    });
  }

  const unreachableRfNodes = allPassageNodes.filter(
    n => n.type === 'passage'
      && unreachableSet.has(n.id)
      && !positionOf(rawById.get(n.id)!),
  );
  if (unreachableRfNodes.length > 0) {
    const pad = 24;
    const labelH = 20;
    const xs = unreachableRfNodes.map(n => n.position.x);
    const ys = unreachableRfNodes.map(n => n.position.y);
    const minX = Math.min(...xs) - pad;
    const minY = Math.min(...ys) - pad - labelH;
    const maxX = Math.max(...xs, ...unreachableRfNodes.map(n => n.position.x + nodeWidth(n))) + pad;
    const maxY = Math.max(...ys, ...unreachableRfNodes.map(n => n.position.y + nodeHeight(n))) + pad;

    groupNodes.push({
      id: '__unreachable__',
      type: 'group',
      position: { x: Math.round(minX), y: Math.round(minY) },
      data: { label: 'Unreachable', memberIds: [], readOnly: true } as GroupNodeData,
      style: {
        width: Math.round(maxX - minX),
        height: Math.round(maxY - minY),
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

    const edgeKind: EdgeKind =
      e.edge_type === 'upstream' ? 'upstream' :
      e.edge_type === 'broken'   ? 'broken'   :
      e.edge_type === 'jump'     ? 'jump'     :
      e.edge_type === 'include'  ? 'include'  :
      e.edge_type === 'manual'   ? 'manual'   : 'navigation';

    // Edge-type visibility filter (Legend chips).
    if (hiddenEdgeKinds.has(edgeKind)) continue;

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
  const typeLabel = d.is_placeholder ? 'Missing passage (broken link)' :
    d.is_start ? 'Start passage' :
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
      <div className="node-tooltip__hint">
        {d.is_placeholder ? 'Click to create this passage' : 'Click to open in editor'}
      </div>
    </div>
  );
}

// ── Zoom widget ────────────────────────────────────────────────────────────
//
// Twine-parity zoom affordance: −/+/fit buttons, a live percentage, and a
// preset dropdown (25 / 50 / 75 / 100 / Big). Mirrors wheel zoom state.

const ZOOM_PRESETS: Array<{ label: string; value: number | 'fit' }> = [
  { label: '25%', value: 0.25 },
  { label: '50%', value: 0.5 },
  { label: '75%', value: 0.75 },
  { label: '100%', value: 1 },
  { label: 'Big', value: 2 },
  { label: 'Zoom to fit', value: 'fit' },
];

function ZoomWidget({
  zoomPct,
  onZoomIn,
  onZoomOut,
  onFit,
  onPreset,
}: {
  zoomPct: number;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFit: () => void;
  onPreset: (v: number | 'fit') => void;
}) {
  return (
    <div className="zoom-widget">
      <button className="zoom-widget__btn" title="Zoom out (-)" onClick={onZoomOut}>−</button>
      <span className="zoom-widget__pct" title="Current zoom (0 = 100%)">{zoomPct}%</span>
      <button className="zoom-widget__btn" title="Zoom in (+)" onClick={onZoomIn}>+</button>
      <button className="zoom-widget__btn zoom-widget__btn--fit" title="Zoom to fit (F / 1)" onClick={onFit}>
        ⤢
      </button>
      <select
        className="zoom-widget__select"
        title="Zoom preset"
        value=""
        onChange={e => {
          const v = e.target.value;
          if (v === '') return;
          onPreset(v === 'fit' ? 'fit' : Number(v));
          e.currentTarget.value = '';
        }}
      >
        <option value="">▾</option>
        {ZOOM_PRESETS.map(p => (
          <option key={p.label} value={String(p.value)}>{p.label}</option>
        ))}
      </select>
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
  /** Begin "new passage" at graph coordinates (prefillName for ghosts). */
  onCreatePassageAt: (x: number, y: number, prefillName?: string) => void;
  /** Set the story's start passage. */
  onSetStart: (name: string) => void;
  /** Steer the inspector: focus a field or arm the delete confirm. */
  onActionSignal: (
    field: 'rename' | 'reachable' | 'delete' | 'tags' | 'color' | 'group' | 'groupCreate',
  ) => void;
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
  /** Snap-to-grid toggle (context menu check item). */
  snapEnabled: boolean;
  onToggleSnap: () => void;
  /** Edge kinds hidden via the Legend chips. */
  hiddenEdgeKinds: Set<EdgeKind>;
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
  snapEnabled,
  onToggleSnap,
  hiddenEdgeKinds,
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
  const [zoomPct, setZoomPct] = useState(100);

  // FIX: tooltip position stored in a ref to avoid triggering re-renders on
  // every mouse-move; only the displayed tip state triggers a render.
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const tooltipPosRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const activeTooltipNodeRef = useRef<string | null>(null);

  const initialFitDoneRef = useRef(false);
  const savedViewportRef = useRef<{ x: number; y: number; zoom: number } | null>(null);

  // Positions the user has dragged THIS session (passage id → coords).
  // A drag-end fires a debounced updatePositions (150 ms) whose header
  // write then round-trips through LSP before the next graph refresh
  // carries it back. A rebuild landing inside that window used to snap
  // the node back to its pre-drag spot; buildElements treats these as
  // anchors so a dragged node stays put from the moment it is released.
  // Stored header metadata still wins (checked first in buildElements).
  const sessionPositionsRef = useRef(new Map<string, { x: number; y: number }>());
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
      .map(n => ({ id: n.id, data: n.data as PassageNodeData }))
      .filter(n => !n.data.is_placeholder);
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

  // Record released-drag coordinates immediately (not at send time) so a
  // rebuild racing the debounced write still sees them as anchors.
  const rememberSessionPositions = useCallback((updates: KnotPositionUpdate[]) => {
    for (const u of updates) {
      sessionPositionsRef.current.set(u.passage_name, { x: u.position_x, y: u.position_y });
    }
  }, []);

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

    const { nodes: newNodes, edges: newEdges } = buildElements(
      graphData,
      hiddenEdgeKinds,
      sessionPositionsRef.current,
    );

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
        .map(n => ({ id: n.id, data: n.data as PassageNodeData }))
        .filter(n => !n.data.is_placeholder);
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
  }, [graphData, hiddenEdgeKinds, setNodes, setEdges, fitView, setViewport, getViewport, onSelectionChange]);

  // ── Live group bounds ──────────────────────────────────────────────────
  // Whenever passage nodes move (drag, nudge, group move), recompute every
  // interactive group box from its members' CURRENT positions so the box
  // tracks them live — no server round-trip needed for geometry. Guarded:
  // when nothing changed the same array is returned and React bails out,
  // so this cannot loop.

  useEffect(() => {
    setNodes(nds => {
      const groups = nds.filter(n =>
        n.type === 'group' && !(n.data as GroupNodeData).readOnly,
      );
      if (groups.length === 0) return nds;

      const passageById = new Map<string, Node>();
      for (const n of nds) {
        if (n.type === 'passage') passageById.set(n.id, n);
      }

      const patches = new Map<string, { x: number; y: number; w: number; h: number }>();
      for (const g of groups) {
        const members = ((g.data as GroupNodeData).memberIds ?? [])
          .map(id => passageById.get(id))
          .filter((m): m is Node => !!m);
        if (members.length === 0) continue;

        const pad = 26;
        const w = (n: Node) => ((n.data as PassageNodeData).size_w ?? NODE_W);
        const h = (n: Node) => ((n.data as PassageNodeData).size_h ?? NODE_H);
        const xs = members.map(m => m.position.x);
        const ys = members.map(m => m.position.y);
        const minX = Math.min(...xs) - pad;
        const minY = Math.min(...ys) - pad - 16;
        const maxX = Math.max(...xs, ...members.map(m => m.position.x + w(m))) + pad;
        const maxY = Math.max(...ys, ...members.map(m => m.position.y + h(m))) + pad;

        const next = {
          x: Math.round(minX),
          y: Math.round(minY),
          w: Math.round(maxX - minX),
          h: Math.round(maxY - minY),
        };
        const cur = {
          x: g.position.x,
          y: g.position.y,
          w: (g.style?.width as number) ?? 0,
          h: (g.style?.height as number) ?? 0,
        };
        if (next.x !== cur.x || next.y !== cur.y || next.w !== cur.w || next.h !== cur.h) {
          patches.set(g.id, next);
        }
      }

      if (patches.size === 0) return nds;
      return nds.map(n => {
        const p = patches.get(n.id);
        if (!p) return n;
        return {
          ...n,
          position: { x: p.x, y: p.y },
          style: { ...n.style, width: p.w, height: p.h },
        };
      });
    });
  }, [nodes, setNodes]);

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
      .map(n => ({ node: n, d: n.data as PassageNodeData }))
      .filter(({ d }) => !d.is_placeholder)
      .map(({ node, d }) => ({
        passage_name: node.id,
        position_x: snapTo(node.position.x, snapEnabled),
        position_y: snapTo(node.position.y, snapEnabled),
        group: d.group,
        color: d.metadata_color,
      }));
    if (updates.length > 0) {
      try {
        const vp = getViewport();
        savedViewportRef.current = { x: vp.x, y: vp.y, zoom: vp.zoom };
      } catch { /* not ready */ }
      vscode.postMessage({ command: 'saveAllPositions', updates });
    }
  }, [saveRequested, getViewport, snapEnabled]);

  // ── Group actions (context for GroupNode) ─────────────────────────────
  // Move-as-unit drags: beginGroupDrag snapshots nothing (we translate
  // incrementally via movementX/Y), dragGroupBy applies the delta to the
  // group node and all members, endGroupDrag persists every member's new
  // position as ONE batched updatePositions request.

  const groupDragRef = useRef<{ groupId: string; moved: boolean } | null>(null);

  const groupActions = useMemo<GroupActions>(() => ({
    beginGroupDrag: (groupId, _e) => {
      groupDragRef.current = { groupId, moved: false };
    },
    dragGroupBy: (groupId, dx, dy) => {
      if (dx === 0 && dy === 0) return;
      const st = groupDragRef.current;
      if (!st || st.groupId !== groupId) return;
      st.moved = true;
      let zoom = 1;
      try { zoom = getViewport().zoom; } catch { /* not ready */ }
      const fdx = dx / zoom;
      const fdy = dy / zoom;
      setNodes(nds => nds.map(n => {
        if (n.id === groupId) {
          return { ...n, position: { x: n.position.x + fdx, y: n.position.y + fdy } };
        }
        if (n.type !== 'passage') return n;
        const d = n.data as PassageNodeData;
        if (d.group !== groupId) return n;
        return { ...n, position: { x: n.position.x + fdx, y: n.position.y + fdy } };
      }));
    },
    endGroupDrag: groupId => {
      const st = groupDragRef.current;
      groupDragRef.current = null;
      if (!st || st.groupId !== groupId || !st.moved) return;

      const updates: KnotPositionUpdate[] = [];
      setNodes(nds => {
        for (const n of nds) {
          if (n.type !== 'passage') continue;
          const d = n.data as PassageNodeData;
          if (d.group !== groupId) continue;
          updates.push({
            passage_name: n.id,
            position_x: snapTo(n.position.x, snapEnabled),
            position_y: snapTo(n.position.y, snapEnabled),
            group: d.group,
            color: d.metadata_color,
          });
        }
        // Snap members (and let the bounds effect resize the box).
        return nds.map(n => {
          if (n.type !== 'passage') return n;
          const d = n.data as PassageNodeData;
          if (d.group !== groupId) return n;
          return {
            ...n,
            position: {
              x: snapTo(n.position.x, snapEnabled),
              y: snapTo(n.position.y, snapEnabled),
            },
          };
        });
      });
      if (updates.length > 0) {
        rememberSessionPositions(updates);
        vscode.postMessage({ command: 'updatePositions', updates });
      }
    },
    commitGroupRename: (groupId, newName) => {
      const members = nodesRef.current
        .filter(n => n.type === 'passage' && (n.data as PassageNodeData).group === groupId)
        .map(n => n.id);
      if (members.length === 0) return;
      vscode.postMessage({
        command: 'bulkUpdateMetadata',
        passage_names: members,
        group: newName,
      });
    },
  }), [getViewport, setNodes, snapEnabled, rememberSessionPositions]);

  // ── Node drag end → snap + send position + group membership ──────────
  // React Flow emits position changes for EVERY node in a multi-selection
  // drag, so marquee-selected groups persist their positions together.
  // Drop-in / drag-out: dropping a passage inside a DIFFERENT group's box
  // assigns membership; dragging it beyond its own group's edge (12 px
  // grace) removes it — Twine 2.7 group semantics on the twee-header
  // storage model.

  const handleNodesChange = useCallback((changes: NodeChange[]) => {
    onNodesChange(changes);

    const dragEnds = changes.filter(
      (c): c is NodePositionChange =>
        c.type === 'position' && c.dragging === false && !!c.position,
    );
    if (dragEnds.length === 0) return;

    const snapped = new Map(
      dragEnds.map(c => [c.id, { x: snapTo(c.position!.x, snapEnabled), y: snapTo(c.position!.y, snapEnabled) }]),
    );
    const updates: KnotPositionUpdate[] = [];
    const membershipChanges: Array<{ id: string; group: string | null }> = [];

    setNodes(nds => {
      // Group boxes (position + size) in flow coordinates for hit-testing.
      const groupBoxes = nds
        .filter(n => n.type === 'group' && !(n.data as GroupNodeData).readOnly)
        .map(n => ({
          id: n.id,
          x: n.position.x,
          y: n.position.y,
          w: (n.style?.width as number) ?? 0,
          h: (n.style?.height as number) ?? 0,
        }));

      return nds.map(n => {
        const s = snapped.get(n.id);
        if (!s) return n;
        const d = n.data as PassageNodeData;
        if (d.is_placeholder) {
          // Ghosts move locally only (no file to write back to).
          return { ...n, position: s };
        }

        updates.push({
          passage_name: n.id,
          position_x: s.x,
          position_y: s.y,
          group: d.group,
          color: d.metadata_color,
        });

        // ── Group membership by drop position ─────────────────────────
        const w = d.size_w ?? NODE_W;
        const h = d.size_h ?? NODE_H;
        const cx = s.x + w / 2;
        const cy = s.y + h / 2;
        const inside = (b: typeof groupBoxes[number]) =>
          cx >= b.x && cx <= b.x + b.w && cy >= b.y && cy <= b.y + b.h;

        const ownBox = d.group ? groupBoxes.find(b => b.id === d.group) : undefined;
        const otherBox = groupBoxes.find(b => b.id !== d.group && inside(b));

        if (otherBox && otherBox.id !== d.group) {
          // Dropped into a different group → join it.
          membershipChanges.push({ id: n.id, group: otherBox.id });
          return { ...n, position: s, data: { ...d, group: otherBox.id } };
        }
        if (d.group && ownBox && !inside({
          ...ownBox,
          x: ownBox.x - GROUP_ESCAPE_GRACE,
          y: ownBox.y - GROUP_ESCAPE_GRACE,
          w: ownBox.w + GROUP_ESCAPE_GRACE * 2,
          h: ownBox.h + GROUP_ESCAPE_GRACE * 2,
        })) {
          // Dragged beyond its own group's edge → leave the group.
          membershipChanges.push({ id: n.id, group: null });
          return { ...n, position: s, data: { ...d, group: undefined } };
        }

        return { ...n, position: s };
      });
    });

    rememberSessionPositions(updates);
    debouncedPositionUpdate(updates);

    // Membership changes go through the single-passage metadata endpoint
    // (they're rare, one passage at a time; position rides along already).
    for (const mc of membershipChanges) {
      vscode.postMessage({
        command: 'updatePassageMetadata',
        passage_name: mc.id,
        group: mc.group,
      });
    }
  }, [onNodesChange, setNodes, debouncedPositionUpdate, rememberSessionPositions, snapEnabled]);

  // ── Instant navigation: plain click on a node → open it in the editor ──
  // The click doubles as the selection; modifier clicks (Shift/Ctrl/Cmd/
  // Alt) only build a multi-selection and never navigate. Ghost nodes have
  // no file — their click opens the create-passage flow prefilled.

  const openPassageNode = useCallback((node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.is_placeholder) return;
    if (d.file) onOpenPassage(d.file, d.line || 0, d.label);
  }, [onOpenPassage]);

  const navigateToNode = useCallback((node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.is_placeholder) return;
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
    const d = node.data as PassageNodeData;
    if (e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    primaryIdRef.current = node.id;
    if (d.is_placeholder) {
      // Broken-link ghost → create the missing passage right here.
      onCreatePassageAt(node.position.x, node.position.y, d.label);
      return;
    }
    navigateToNode(node);
  }, [navigateToNode, onCreatePassageAt]);

  const handleNodeDoubleClick = useCallback((_e: React.MouseEvent, node: Node) => {
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;
    if (d.is_placeholder) {
      onCreatePassageAt(node.position.x, node.position.y, d.label);
      return;
    }
    // Single click already navigates; keep double-click as an alias for
    // muscle memory (no double message issues — same target). The LABEL's
    // own dblclick handler (rename) stops propagation before this fires.
    openPassageNode(node);
  }, [openPassageNode, onCreatePassageAt]);

  // ── Selection → inspector (multi-aware) ─────────────────────

  const handleSelectionChange = useCallback((params: OnSelectionChangeParams) => {
    const selected = params.nodes.filter(n => {
      if (n.type !== 'passage') return false;
      return !(n.data as PassageNodeData).is_placeholder;
    });
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
      if ((n.data as PassageNodeData).is_placeholder) return n;
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
    if (el.closest('.zoom-widget')) return;
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
    if (node.type === 'group') {
      const d = node.data as GroupNodeData;
      if (d.readOnly) return;
      setCtxMenu({
        x: e.clientX,
        y: e.clientY,
        kind: 'group',
        nodeId: node.id,
        memberCount: d.memberIds.length,
        minimapOn: showMiniMap,
        legendOn: showLegend,
        snapOn: snapEnabled,
      });
      return;
    }
    if (node.type !== 'passage') return;
    const d = node.data as PassageNodeData;

    // Ghost nodes get a single-entry menu (create the passage).
    if (d.is_placeholder) {
      setCtxMenu({
        x: e.clientX,
        y: e.clientY,
        kind: 'ghost',
        nodeId: node.id,
        ghostName: d.label,
        minimapOn: showMiniMap,
        legendOn: showLegend,
        snapOn: snapEnabled,
      });
      return;
    }

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
      snapOn: snapEnabled,
    });
  }, [setNodes, onSelectionChange, showMiniMap, showLegend, snapEnabled]);

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
      snapOn: snapEnabled,
    });
  }, [screenToFlowPosition, showMiniMap, showLegend, snapEnabled]);

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
    onGroupSelected: () => onActionSignal('groupCreate'),
    onRenameGroup: (groupId: string) => {
      // Renaming via context menu = signal the modal in App (needs a name
      // prompt); the canvas inline rename is the direct path.
      onActionSignal('group');
      void groupId;
    },
    onDissolveGroup: (groupId: string) => {
      const members = nodesRef.current
        .filter(n => n.type === 'passage' && (n.data as PassageNodeData).group === groupId)
        .map(n => n.id);
      if (members.length === 0) return;
      vscode.postMessage({
        command: 'bulkUpdateMetadata',
        passage_names: members,
        group: null,
      });
    },
    onDelete: (_nodeId: string) => onActionSignal('delete'),
    onNewPassageHere: (x: number, y: number) => onCreatePassageAt(x, y),
    onNewPassageAtViewCenter,
    onCreateGhost: (nodeId: string) => {
      const node = nodesRef.current.find(n => n.id === nodeId);
      if (!node) return;
      onCreatePassageAt(node.position.x, node.position.y, (node.data as PassageNodeData).label);
    },
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
    onToggleSnap: onToggleSnap,
  }), [openPassageNode, onActionSignal, onSetStart, onCreatePassageAt, onNewPassageAtViewCenter, fitView, zoomIn, zoomOut, selectAllPassages, clearSelection, onFindPassages, onRequestSaveLayout, onRequestRefresh, onToggleMiniMap, onToggleLegend, onToggleSnap]);

  // ── Zoom helpers (widget + keyboard) ──────────────────────────────────

  /** Zoom to an absolute level, keeping the viewport CENTER pinned
   *  (Twine's preset dropdown behavior). */
  const zoomTo = useCallback((level: number) => {
    let vp;
    try { vp = getViewport(); } catch { return; }
    const el = document.querySelector('.storymap-inner');
    const rect = el?.getBoundingClientRect();
    const cx = rect ? rect.left + rect.width / 2 : window.innerWidth / 2;
    const cy = rect ? rect.top + rect.height / 2 : window.innerHeight / 2;
    const scale = level / vp.zoom;
    setViewport({
      zoom: level,
      x: cx - (cx - vp.x) * scale,
      y: cy - (cy - vp.y) * scale,
    }, { duration: 200 });
  }, [getViewport, setViewport]);

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
      // Keyboard zoom: +/- step, 0 → 100%, 1 → fit (Twine muscle memory).
      if (!mod && !e.altKey && (e.key === '+' || e.key === '=')) {
        e.preventDefault();
        zoomIn({ duration: 200 });
        return;
      }
      if (!mod && !e.altKey && (e.key === '-' || e.key === '_')) {
        e.preventDefault();
        zoomOut({ duration: 200 });
        return;
      }
      if (!mod && !e.altKey && e.key === '0') {
        e.preventDefault();
        zoomTo(1);
        return;
      }
      if (!mod && !e.altKey && e.key === '1') {
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

      // Arrow-key nudge: 1 grid cell (20 px, or 1 px when snap is off),
      // Shift = 10×. Persists through the normal position-update path.
      if (!e.altKey && ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
        e.preventDefault();
        const step = (snapEnabled ? GRID_SNAP : 1) * (e.shiftKey ? 10 : 1);
        const dx = e.key === 'ArrowLeft' ? -step : e.key === 'ArrowRight' ? step : 0;
        const dy = e.key === 'ArrowUp' ? -step : e.key === 'ArrowDown' ? step : 0;

        const updates: KnotPositionUpdate[] = [];
        setNodes(nds => nds.map(n => {
          if (n.type !== 'passage' || !selectedIdsRef.current.has(n.id)) return n;
          const d = n.data as PassageNodeData;
          if (d.is_placeholder) return n;
          const nx = n.position.x + dx;
          const ny = n.position.y + dy;
          updates.push({
            passage_name: n.id,
            position_x: nx,
            position_y: ny,
            group: d.group,
            color: d.metadata_color,
          });
          return { ...n, position: { x: nx, y: ny } };
        }));
        rememberSessionPositions(updates);
        debouncedPositionUpdate(updates);
        return;
      }

      if (e.key === 'Delete' || e.key === 'Backspace') {
        const node = nodesRef.current.find(n => n.id === selectedId);
        if (node && (node.data as PassageNodeData).is_placeholder) return;
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
      } else if (!mod && !e.altKey && e.key.toLowerCase() === 'g') {
        // Group the current selection (asks for a name via the app shell).
        if (selectedIdsRef.current.size >= 2) {
          e.preventDefault();
          onActionSignal('groupCreate');
        }
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onActionSignal, navigateToNode, selectAllPassages, clearSelection, fitView, goBack, goForward, zoomIn, zoomOut, zoomTo, setNodes, debouncedPositionUpdate, rememberSessionPositions, snapEnabled]);

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
      // Live zoom readout for the widget (guard: only re-render on 1%
      // steps — the nodes array doesn't change so node memoization makes
      // this cheap).
      const pct = Math.round(vp.zoom * 100);
      setZoomPct(prev => (prev === pct ? prev : pct));
    } catch { /* not ready */ }
  }, [getViewport, debouncedViewportUpdate, onViewportKnown]);

  // ── MiniMap color ──────────────────────────────────────────────────────

  const miniMapColor = useCallback((n: Node) => {
    if (n.type !== 'passage') return 'rgba(148, 163, 184, 0.18)';
    const d = n.data as PassageNodeData;
    if (d.is_placeholder) return 'rgba(248, 113, 113, 0.35)';
    return d.color || COLORS.normal;
  }, []);

  return (
    <GroupActionsContext.Provider value={groupActions}>
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
          //   space+left-drag    → pan (explicit; React Flow default made
          //                        loud here so the affordance is testable)
          //   wheel              → zoom
          //   right-click        → context menu (the "action panel")
          //   back/forward       → navigation history (wrapper handlers)
          panOnDrag={[1]}
          panActivationKeyCode="Space"
          selectionOnDrag
          selectionKeyCode={null}
          multiSelectionKeyCode={['Shift', 'Control', 'Meta']}
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
        <ZoomWidget
          zoomPct={zoomPct}
          onZoomIn={() => zoomIn({ duration: 200 })}
          onZoomOut={() => zoomOut({ duration: 200 })}
          onFit={() => fitView({ padding: 0.12, duration: 280 })}
          onPreset={v => (v === 'fit' ? fitView({ padding: 0.12, duration: 280 }) : zoomTo(v))}
        />
        <NodeTooltip tip={tooltip} />
        {ctxMenu && (
          <ContextMenu menu={ctxMenu} actions={ctxActions} onClose={closeCtxMenu} />
        )}
      </div>
    </GroupActionsContext.Provider>
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
  /** Snap-to-grid toggle (context menu check item). */
  snapEnabled: boolean;
  onToggleSnap: () => void;
  /** Edge kinds hidden via the Legend chips. */
  hiddenEdgeKinds: Set<EdgeKind>;
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
