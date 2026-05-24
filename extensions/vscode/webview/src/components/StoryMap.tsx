import React, { useEffect, useRef, useCallback } from 'react';
import cytoscape from 'cytoscape';
import dagre from 'cytoscape-dagre';
import { KnotGraphResponse, KnotPositionUpdate } from '../types';
import { vscode } from '../App';

// Register the dagre layout extension
cytoscape.use(dagre);

// ── Snap-to-grid constant (Twine-inspired) ──────────────────────────────────
const GRID_SNAP = 20;

/** Snap a coordinate to the nearest grid point. */
function snapToGrid(value: number): number {
  return Math.round(value / GRID_SNAP) * GRID_SNAP;
}

// ── Standardized node dimensions ────────────────────────────────────────────
// All passage nodes use the same width/height so the graph looks clean and
// predictable. Labels that don't fit are truncated with ellipsis.
const NODE_W = 100;
const NODE_H = 36;

// ── Layout constants ──────────────────────────────────────────────────────────
/**
 * Layout follows a Twine-inspired viewport model:
 *
 *   ┌──────────────────────┬────────────────┬────────────────┐
 *   │ special_passage_group│                │                │
 *   ├──────────────────────┼────────────────┼────────────────┤
 *   │ unreachable_passages │ graph start    │ graph expands  │
 *   │  ·                   │ graph expands  │ graph expands  │
 *   └──────────────────────┴────────────────┴────────────────┘
 *
 * The Start node anchors the graph. The graph expands
 * rightward (positive X) and downward (positive Y) from that anchor,
 * giving all nodes positive coordinates — just like Twine's canvas.
 *
 * The special passages box sits top-left. Unreachable passages stack
 * vertically below the box in the left column.
 */

/** Fixed width for the special passages box (2-column layout). */
const SPECIAL_BOX_WIDTH = 260;
/** Origin of the special passages box (top-left corner). */
const BOX_ORIGIN_X = GRID_SNAP * 2;   // 40
const BOX_ORIGIN_Y = GRID_SNAP * 2;   // 40
/** Default anchor position for the start passage when it has no saved position.
 *  Placed to the right of the unreachable column, at the same vertical level
 *  as the unreachable passages area. */
const START_ANCHOR_X = 380;
const START_ANCHOR_Y = BOX_ORIGIN_Y;

// ── Zoom constraints ──────────────────────────────────────────────────────────
// The zoom level is controlled entirely by the user's scroll wheel.
// We clamp it to a sane range to prevent degenerate views.
const MIN_ZOOM = 0.15;
const MAX_ZOOM = 4.0;

// ── Special passage hierarchy for box arrangement ──────────────────────────
// Defines display order within the special passages box for visual symmetry.
// Core Twine passages come first, then format-specific ones, sorted
// alphabetically within each tier.
const SPECIAL_TIER_ORDER: Record<string, number> = {
  'StoryTitle':      0,
  'StoryData':       1,
  'StoryStylesheet': 2,
  'StoryJavaScript': 3,
  'StoryInit':       4,
  'StoryInterface':  5,
  'StoryCaption':    6,
  'StoryMenu':       7,
  'StoryAuthor':     8,
};

/** Sort key for special passages within the box. Lower tier = displayed first. */
function specialSortKey(label: string): number {
  if (label in SPECIAL_TIER_ORDER) return SPECIAL_TIER_ORDER[label];
  // Unknown special passages go last, sorted alphabetically after tier 10
  return 10;
}

// ── Twine-inspired color palette ────────────────────────────────────────────
const COLORS = {
  normal:      '#3a7ca5',
  start:       '#43a047',
  special:     '#ef6c00',
  metadata:    '#8e24aa',
  unreachable: '#4a4a4a',
  broken:      '#e53935',
  gameLoop:    '#ff7043',
  edgeNormal:  '#7a8a9e',
  edgeUpstream:'#5c6370',
  edgeCall:    '#ab47bc',
  edgeInclude: '#26a69a',
  edgeJump:    '#ffa726',
  groupBorder: '#555555',
  groupBg:     'transparent',
  selectionBox:'rgba(0,122,204,0.15)',
};

function getNodeColor(data: {
  is_metadata?: boolean;
  is_unreachable?: boolean;
  is_special?: boolean;
  is_start?: boolean;
}): string {
  if (data.is_unreachable) return COLORS.unreachable;
  if (data.is_start)       return COLORS.start;
  if (data.is_metadata)    return COLORS.metadata;
  if (data.is_special)     return COLORS.special;
  return COLORS.normal;
}

// ── Cytoscape stylesheet ────────────────────────────────────────────────────
const CYTOSCAPE_STYLE: any[] = [
  {
    selector: 'node',
    style: {
      'label': 'data(label)',
      'background-color': 'data(bgColor)',
      'color': '#ffffff',
      'text-valign': 'center',
      'text-halign': 'center',
      'font-size': '11px',
      'text-wrap': 'ellipsis',
      'text-max-width': `${NODE_W - 10}px`,
      'width': NODE_W,
      'height': NODE_H,
      'shape': 'round-rectangle',
      'text-outline-color': 'transparent',
      'text-outline-width': '0px',
      'border-width': '2px',
      'border-color': 'data(borderColor)',
      'font-family': '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
      'font-weight': 'normal',
    },
  },
  {
    selector: 'edge',
    style: {
      'width': 1.5,
      'line-color': COLORS.edgeNormal,
      'target-arrow-color': COLORS.edgeNormal,
      'target-arrow-shape': 'triangle',
      'arrow-scale': 0.7,
      'curve-style': 'bezier',
      'opacity': 0.7,
    },
  },
  {
    selector: 'edge.is_broken',
    style: {
      'line-color': COLORS.broken,
      'target-arrow-color': COLORS.broken,
      'line-style': 'dashed',
      'opacity': 0.9,
    },
  },
  {
    selector: 'edge.is_upstream',
    style: {
      'line-style': 'dashed',
      'line-color': COLORS.edgeUpstream,
      'target-arrow-color': COLORS.edgeUpstream,
      'opacity': 0.4,
    },
  },
  {
    selector: 'edge.is_call',
    style: {
      'line-color': COLORS.edgeCall,
      'target-arrow-color': COLORS.edgeCall,
      'line-style': 'dotted',
    },
  },
  {
    selector: 'edge.is_include',
    style: {
      'line-color': COLORS.edgeInclude,
      'target-arrow-color': COLORS.edgeInclude,
      'line-style': 'dotted',
    },
  },
  {
    selector: 'edge.is_jump',
    style: {
      'line-color': COLORS.edgeJump,
      'target-arrow-color': COLORS.edgeJump,
    },
  },
  {
    selector: 'edge.has_label',
    style: {
      'label': 'data(displayText)',
      'font-size': '8px',
      'text-rotation': 'autorotate',
      'text-outline-color': '#1e1e2e',
      'text-outline-width': '2px',
      'color': '#aab0bc',
    },
  },
  {
    selector: 'node.highlighted',
    style: { 'border-color': '#ffffff', 'border-width': '3px', 'z-index': 999 },
  },
  {
    selector: 'node.dimmed',
    style: { 'opacity': 0.2 },
  },
  {
    selector: 'edge.dimmed',
    style: { 'opacity': 0.08 },
  },
  {
    selector: 'node.game_loop',
    style: {
      'border-color': COLORS.gameLoop,
      'border-width': '3px',
      'border-style': 'double',
    },
  },
  {
    selector: 'node.is_start',
    style: {
      'font-weight': 'bold',
      'border-width': '3px',
      'border-color': '#ffffff',
    },
  },
  // ── Compound group boxes ─────────────────────────────────────────────────
  // Border-only style — no solid fill. The box is just a labelled border
  // that visually groups the special passages without obscuring the canvas.
  {
    selector: '.group-box',
    style: {
      'background-color': COLORS.groupBg,
      'background-opacity': 0,
      'border-width': 1,
      'border-style': 'solid',
      'border-color': COLORS.groupBorder,
      'border-opacity': 0.5,
      'label': 'data(label)',
      'text-valign': 'top',
      'text-halign': 'left',
      'font-size': '9px',
      'color': '#888888',
      'text-margin-x': 4,
      'text-margin-y': 2,
      'padding-left': 8,
      'padding-right': 8,
      'padding-top': 16,
      'padding-bottom': 6,
      'shape': 'round-rectangle',
      'compound-sizing-wrt-labels': 'include',
      'z-index': 0,
      'z-compound-depth': 'bottom',
    },
  },
  // ── Focused node (highlighted via focusNode command) ────────────────────
  {
    selector: 'node.focused',
    style: {
      'border-color': '#007acc',
      'border-width': '3px',
      'z-index': 998,
    },
  },
];

// ── Compound group IDs ──────────────────────────────────────────────────────
const GROUP_SPECIAL = '__group_special';
// Unreachable passages get NO compound group box — they are positioned
// to the left side of the main graph in a vertical stack.

interface StoryMapProps {
  graphData: KnotGraphResponse | null;
  layout: string;
  searchQuery: string;
  fitRequested: number;
  saveRequested: number;
  focusRequested: number;
  focusPassageName: string;
  onLayoutChange: (layout: string) => void;
}

export default function StoryMap({
  graphData,
  layout,
  searchQuery,
  fitRequested,
  saveRequested,
  focusRequested,
  focusPassageName,
  onLayoutChange,
}: StoryMapProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<cytoscape.Core | null>(null);
  const currentDataRef = useRef<KnotGraphResponse | null>(null);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  // Track whether the initial fit has been applied so we don't
  // re-fit on subsequent graph updates (which would fight user zoom).
  const initialFitDoneRef = useRef(false);

  // ── Pan-aware CSS grid background ─────────────────────────────────────────
  // Updates the container's CSS background-position and background-size
  // so the dot grid pans and zooms with the Cytoscape viewport.
  const updateGridBackground = useCallback(() => {
    const container = containerRef.current;
    const cy = cyRef.current;
    if (!container || !cy) return;

    const pan = cy.pan();
    const zoom = cy.zoom();
    const gridSize = GRID_SNAP * zoom;

    container.style.backgroundSize = `${gridSize}px ${gridSize}px`;
    container.style.backgroundPosition = `${pan.x % gridSize}px ${pan.y % gridSize}px`;
  }, []);

  // ── Focus on a node by passage name ─────────────────────────────────────
  // Pans and zooms the viewport to center on the specified passage node.
  // Used when navigating to a passage from diagnostics or other views.
  // Preserves the user's current zoom level — only pans the viewport.
  const focusOnNode = useCallback((passageName: string) => {
    const cy = cyRef.current;
    if (!cy) return;

    // Try to find the node by ID first, then by label
    let node = cy.getElementById(passageName);
    if (node.length === 0) {
      node = cy.nodes().filter((n: any) => n.data('label') === passageName && !n.data('isGroup'));
    }
    if (node.length === 0) return;

    const target = node[0];

    // Remove previous focus highlights
    cy.nodes().removeClass('focused');
    target.addClass('focused');

    // Pan to center the node in the viewport at the current zoom level.
    // We do NOT change zoom — the user controls zoom via scroll.
    cy.animate({
      pan: {
        x: (cy.width() / 2) - target.position().x * cy.zoom(),
        y: (cy.height() / 2) - target.position().y * cy.zoom(),
      },
    }, {
      duration: 300,
      easing: 'ease-in-out-cubic',
    });

    // Remove focus highlight after a delay
    setTimeout(() => {
      target.removeClass('focused');
    }, 2000);
  }, []);

  // ── Initialize Cytoscape ──────────────────────────────────────────────────
  useEffect(() => {
    if (!containerRef.current) {
      vscode.postMessage({ command: 'log', level: 'warn', message: '[StoryMap] containerRef is null at init time' });
      return;
    }

    console.log('[StoryMap] Initializing Cytoscape, container:', containerRef.current.getBoundingClientRect());

    const cy = cytoscape({
      container: containerRef.current,
      style: CYTOSCAPE_STYLE,
      layout: { name: 'null' },
      // ── Box selection: click-drag on empty canvas draws a selection
      // rectangle. All nodes inside are selected. Dragging any selected
      // node moves ALL selected nodes together (Cytoscape default).
      boxSelectionEnabled: true,
      // Auto-dismiss selection when clicking on empty canvas
      autounselectify: false,
      // Allow selecting nodes by clicking
      selectionType: 'single',
      // ── Zoom constraints: the zoom level is entirely user-controlled
      // via scroll wheel. We just clamp it to a sane range.
      minZoom: MIN_ZOOM,
      maxZoom: MAX_ZOOM,
    });

    cyRef.current = cy;
    console.log('[StoryMap] Cytoscape instance created successfully');

    // Click → open passage (ignore compound group parents)
    // Ctrl/Cmd+click toggles selection without opening
    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      if (node.data('isGroup')) return; // skip group parent nodes
      // If the node is part of a multi-selection, don't open — just select
      if (cy.nodes(':selected').length > 1 && node.selected()) return;
      const file = node.data('file');
      const line = node.data('line') || 0;
      if (file) {
        vscode.postMessage({ command: 'openPassage', file, line });
      }
    });

    // Drag end → snap to grid and write back positions
    // Handles BOTH single-node drag and multi-node drag (when multiple
    // nodes are selected, Cytoscape moves them all together).
    cy.on('dragfree', 'node', (evt) => {
      const dragged = evt.target;
      if (!dragged || !dragged.data('id') || dragged.data('isGroup')) return;

      // Collect all selected non-group nodes for batch position update
      const selectedNodes = cy.nodes(':selected').filter((n: any) => !n.data('isGroup'));
      const nodesToSnap = selectedNodes.length > 1 ? selectedNodes : cy.collection([dragged]);

      const updates: KnotPositionUpdate[] = [];

      nodesToSnap.forEach((n: any) => {
        const pos = n.position();
        const oldX = n.data('posX');
        const oldY = n.data('posY');
        const newX = snapToGrid(pos.x);
        const newY = snapToGrid(pos.y);

        // Animate to snapped position for visual feedback
        n.animate({
          position: { x: newX, y: newY },
        }, {
          duration: 80,
          easing: 'ease-out',
          complete: () => {
            n.data('posX', newX);
            n.data('posY', newY);
          },
        });

        if (oldX == null || oldY == null ||
            Math.abs(newX - oldX) > 0.5 || Math.abs(newY - oldY) > 0.5) {
          updates.push({
            passage_name: n.data('id'),
            position_x: newX,
            position_y: newY,
          });
        }
      });

      if (updates.length > 0) {
        vscode.postMessage({ command: 'updatePositions', updates });
      }
    });

    // Tooltip: show on mouseover (group parents excluded)
    cy.on('mouseover', 'node', (evt) => {
      const d = evt.target.data();
      if (d.isGroup) return;
      const tip = document.getElementById('tooltip');
      if (!tip) return;
      const nameEl = tip.querySelector('.tt-name');
      const tagsEl = tip.querySelector('.tt-tags');
      const metaEl = tip.querySelector('.tt-meta');
      if (nameEl) nameEl.textContent = d.label;

      if (tagsEl) {
        tagsEl.innerHTML = '';
        (d.tags || []).forEach((t: string) => {
          const s = document.createElement('span');
          s.className = 'tt-tag';
          s.textContent = t;
          tagsEl.appendChild(s);
        });
      }

      if (metaEl) {
        const parts: string[] = [];
        if (d.is_start)       parts.push('Start');
        if (d.is_special)     parts.push('Special');
        if (d.is_metadata)    parts.push('Metadata');
        if (d.is_unreachable) parts.push('Unreachable');
        if (d.in_degree > 0)  parts.push('In: ' + d.in_degree);
        if (d.out_degree > 0) parts.push('Out: ' + d.out_degree);
        if (d.posX != null && d.posY != null) parts.push('(' + Math.round(d.posX) + ', ' + Math.round(d.posY) + ')');
        metaEl.textContent = parts.join(' | ');
      }

      tip.style.display = 'block';
    });

    cy.on('mouseout', 'node', () => {
      const tip = document.getElementById('tooltip');
      if (tip) tip.style.display = 'none';
    });

    cy.on('tapdrag', () => {
      const tip = document.getElementById('tooltip');
      if (tip) tip.style.display = 'none';
    });

    // Pan/zoom → update CSS grid background to stay in sync
    cy.on('viewport', updateGridBackground);

    // Resize → update Cytoscape + grid
    const ro = new ResizeObserver(() => {
      if (cyRef.current) {
        cyRef.current.resize();
      }
      updateGridBackground();
    });
    ro.observe(containerRef.current);

    // Deferred resize
    requestAnimationFrame(() => {
      if (cyRef.current) {
        cyRef.current.resize();
        updateGridBackground();
      }
    });

    return () => {
      ro.disconnect();
      cy.destroy();
      cyRef.current = null;
    };
  }, [updateGridBackground]);

  // ── Build the graph from server data ──────────────────────────────────────
  const buildGraph = useCallback((data: KnotGraphResponse) => {
    const cy = cyRef.current;
    if (!cy) return;

    const nodes = Array.isArray(data?.nodes) ? data.nodes : [];
    const edges = Array.isArray(data?.edges) ? data.edges : [];
    currentDataRef.current = { ...data, nodes, edges };

    console.log('[StoryMap] buildGraph: nodes=', nodes.length, 'edges=', edges.length);

    // Save the current viewport state before rebuilding so we can
    // restore it after (prevents the "glitching zoom" effect).
    const prevZoom = cy.zoom();
    const prevPan = cy.pan();

    cy.elements().remove();

    const elements: cytoscape.ElementDefinition[] = [];
    const positionedIds = new Set<string>();
    const unpositioned: typeof nodes = [];

    // Determine game loop members
    const gameLoopMembers = new Set<string>();
    (data.game_loops || []).forEach((loop) => {
      loop.members.forEach((m) => gameLoopMembers.add(m));
    });

    // ── Classify nodes into groups ──────────────────────────────────────
    const specialChildren: string[] = [];   // all special/metadata passages (one box)
    const unreachableIds: string[] = [];     // unreachable — no box, positioned to left side

    const nodeIds = new Set<string>();

    for (const n of nodes) {
      nodeIds.add(n.id);

      const isStart = n.is_start || (n.id === 'Start' || n.label === 'Start');

      // All special passages (is_special || is_metadata) go into the single
      // "Special Passages" box, EXCEPT the start passage which sits outside
      // so its many outgoing edges don't clutter the box.
      if ((n.is_special || n.is_metadata) && !isStart) {
        specialChildren.push(n.id);
      } else if (n.is_unreachable) {
        unreachableIds.push(n.id);
      }

      const color = getNodeColor({ ...n, is_start: isStart });
      const isGameLoop = gameLoopMembers.has(n.id);

      // Determine compound parent — start passage never gets a parent.
      // Unreachable passages also get NO parent (no box for them).
      let parent: string | undefined;
      if (!isStart && specialChildren.includes(n.id)) {
        parent = GROUP_SPECIAL;
      }

      const nodeClasses: string[] = [];
      if (isGameLoop) nodeClasses.push('game_loop');
      if (isStart) nodeClasses.push('is_start');

      // Anchor the start passage at a default location if it has no saved
      // position. This gives the dagre layout a stable root to build around,
      // making visual navigation predictable from the very first run.
      let effectivePosX = n.position_x != null ? snapToGrid(n.position_x) : null;
      let effectivePosY = n.position_y != null ? snapToGrid(n.position_y) : null;
      if (isStart && effectivePosX == null && effectivePosY == null) {
        effectivePosX = snapToGrid(START_ANCHOR_X);
        effectivePosY = snapToGrid(START_ANCHOR_Y);
      }

      const el: cytoscape.NodeDefinition = {
        data: {
          id: n.id,
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
          isGroup: false,
          bgColor: color,
          borderColor: n.is_unreachable ? '#3a3a3a' : color,
          // All nodes get standardized dimensions
          w: NODE_W,
          h: NODE_H,
          posX: effectivePosX,
          posY: effectivePosY,
          var_writes: n.var_writes || [],
          var_reads: n.var_reads || [],
          parent,
        },
        classes: nodeClasses,
      };

      const effectiveHasPos = effectivePosX != null && effectivePosY != null;
      if (effectiveHasPos) {
        el.position = { x: effectivePosX!, y: effectivePosY! };
        positionedIds.add(n.id);
      } else {
        unpositioned.push(n);
      }

      elements.push(el);
    }

    // ── Add single compound group parent node ──────────────────────────
    if (specialChildren.length > 0) {
      elements.push({
        data: { id: GROUP_SPECIAL, label: 'Special Passages', isGroup: true },
        classes: ['group-box'],
        position: { x: 0, y: 0 }, // will be repositioned
      });
    }
    // No group box for unreachable passages — they are arranged to the side.

    // ── Edges ───────────────────────────────────────────────────────────
    // Build a set of special box member IDs for edge suppression.
    const specialSet = new Set(specialChildren);
    // Find the start passage ID (same logic as the node loop above).
    const startId = nodes.find(n => n.is_start || n.id === 'Start' || n.label === 'Start')?.id;

    const usedEdgeIds = new Set<string>();
    let edgeIndex = 0;

    for (const e of edges) {
      // Skip edges referencing compound group parents
      if (!nodeIds.has(e.source) || !nodeIds.has(e.target)) {
        console.warn('[StoryMap] Skipping edge with missing endpoint:', e.source, '->', e.target, '(' + e.edge_type + ')');
        continue;
      }

      // ── Edge suppression ─────────────────────────────────────────────
      // 1. No edges between passages inside the special box. Special
      //    passages are all loaded/serviced at startup; internal edges
      //    are noise.
      // 2. No edges from special box members to the start passage. The
      //    upstream lifecycle chain is implicit and clutters the view.
      // The ONLY edges drawn to/from special box members are references
      // FROM outside the box INTO a special passage (e.g., a user passage
      // that [[-> StoryInterface]]).
      if (specialSet.has(e.source) && specialSet.has(e.target)) {
        continue; // both in box → skip
      }
      if (specialSet.has(e.source) && e.target === startId) {
        continue; // box → Start → skip
      }

      let edgeId = `${e.source}->${e.target}[${e.edge_type || 'nav'}]`;
      if (usedEdgeIds.has(edgeId)) {
        edgeId = `${e.source}->${e.target}[${e.edge_type || 'nav'}_${edgeIndex}]`;
      }
      usedEdgeIds.add(edgeId);
      edgeIndex++;

      const el: cytoscape.EdgeDefinition = {
        data: {
          id: edgeId,
          source: e.source,
          target: e.target,
          displayText: e.display_text || null,
        },
        classes: [
          e.edge_type === 'broken' ? 'is_broken' : '',
          e.edge_type === 'upstream' ? 'is_upstream' : '',
          e.edge_type === 'call' ? 'is_call' : '',
          e.edge_type === 'include' ? 'is_include' : '',
          e.edge_type === 'jump' ? 'is_jump' : '',
          e.display_text ? 'has_label' : '',
        ].filter(Boolean) as string[],
      };
      elements.push(el);
    }

    try {
      cy.add(elements);
      console.log('[StoryMap] Added', elements.length, 'elements to Cytoscape');
    } catch (err) {
      console.error('[StoryMap] cy.add() failed:', err);
      vscode.postMessage({ command: 'log', level: 'error', message: '[StoryMap] cy.add() failed: ' + String(err) });
      return;
    }

    // ── Layout ──────────────────────────────────────────────────────────
    const hasAnyPositions = positionedIds.size > 0;
    const chosenLayout = data.layout || (hasAnyPositions ? 'position' : 'dagre');

    if (chosenLayout !== layoutRef.current) {
      onLayoutChange(chosenLayout);
    }

    // Callback to run after layout completes (handles both synchronous
    // position layout and animated dagre/cose layouts).
    const afterLayout = () => {
      // ── Reposition compound groups & anchors ─────────────────────────
      repositionGroups(cy, specialChildren, unreachableIds);

      updateGridBackground();

      // Only fit on the very first build. Subsequent graph updates
      // (e.g., from file saves triggering refreshGraph) restore the
      // previous viewport to avoid the "glitching zoom" effect.
      if (!initialFitDoneRef.current) {
        initialFitDoneRef.current = true;
        requestAnimationFrame(() => {
          if (cyRef.current && cyRef.current.nodes().length > 0) {
            cyRef.current.resize();
            cyRef.current.fit(undefined, 30);
          }
        });
      } else {
        // Restore the viewport state from before the rebuild
        requestAnimationFrame(() => {
          if (cyRef.current) {
            cyRef.current.resize();
            cyRef.current.viewport({ zoom: prevZoom, pan: prevPan });
            updateGridBackground();
          }
        });
      }
    };

    applyLayout(chosenLayout, cy, positionedIds, unpositioned, afterLayout);
  }, [onLayoutChange, updateGridBackground]);

  // ── Reposition compound groups after layout ───────────────────────────────
  // Layout follows the Twine-inspired viewport model:
  //
  //   ┌──────────────────────┬────────────────┬────────────────┐
  //   │ special_passage_group│                │                │
  //   ├──────────────────────┼────────────────┼────────────────┤
  //   │ unreachable_passages │ graph start    │ graph expands  │
  //   │  ·                   │ graph expands  │ graph expands  │
  //   └──────────────────────┴────────────────┴────────────────┘
  //
  // The special passages box is a border-only compound node at the top-left.
  // Unreachable passages stack vertically below it on the left side.
  // The Start node anchors the graph to the right of the unreachable column;
  // from there the graph expands rightward (positive X) and downward
  // (positive Y).
  const repositionGroups = useCallback((
    cy: cytoscape.Core,
    specialIds: string[],
    unreachableIds: string[],
  ) => {
    // ── Position Special Passages group at top-left ─────────────────────
    // Fixed-width 2-column layout with tiered sort for visual symmetry.
    // Children are sorted by importance tier (core Twine first, then
    // format-specific), then alphabetically within each tier.
    const colWidth = snapToGrid(SPECIAL_BOX_WIDTH / 2);
    const colGap = GRID_SNAP;
    const rowGap = GRID_SNAP;

    let groupBottomY = BOX_ORIGIN_Y; // tracks the bottom of the group

    if (specialIds.length > 0) {
      const children = specialIds.map(id => cy.getElementById(id)).filter(n => n.length > 0);

      // Sort by tier (importance) then alphabetically for symmetric layout
      children.sort((a, b) => {
        const tierA = specialSortKey(a.data('label') || '');
        const tierB = specialSortKey(b.data('label') || '');
        if (tierA !== tierB) return tierA - tierB;
        return (a.data('label') || '').localeCompare(b.data('label') || '');
      });

      // Lay out children in a 2-column grid within the fixed-width box
      let col = 0;
      let rowY = BOX_ORIGIN_Y;
      let rowMaxH = 0;

      for (const n of children) {
        const h = NODE_H; // standardized height

        // Calculate center position for this cell
        const cellCenterX = BOX_ORIGIN_X + col * (colWidth + colGap) + colWidth / 2;
        const cellCenterY = rowY + h / 2;

        n.position({ x: snapToGrid(cellCenterX), y: snapToGrid(cellCenterY) });
        n.data('posX', snapToGrid(cellCenterX));
        n.data('posY', snapToGrid(cellCenterY));

        rowMaxH = Math.max(rowMaxH, h);

        // Alternate columns: 0 → 1 → next row
        col++;
        if (col >= 2) {
          col = 0;
          rowY += rowMaxH + rowGap;
          rowMaxH = 0;
        }
      }

      // Track the bottom of the group for placing unreachable passages below
      const groupEl = cy.getElementById(GROUP_SPECIAL);
      if (groupEl.length > 0) {
        const bb = groupEl.boundingBox();
        groupBottomY = bb.y2 + GRID_SNAP * 2;
      } else {
        groupBottomY = rowY + rowMaxH + GRID_SNAP * 3;
      }
    } else {
      groupBottomY = BOX_ORIGIN_Y + GRID_SNAP * 2;
    }

    // ── Position Unreachable passages vertically on the LEFT side ───────
    // Unreachable passages are NOT in a compound group — they stack
    // vertically (single column) below the special passages box on the
    // left side of the canvas. This keeps them visible but out of the way
    // of the main graph flow which expands right and down.
    if (unreachableIds.length > 0) {
      const children = unreachableIds.map(id => cy.getElementById(id)).filter(n => n.length > 0);

      // Vertical stack starting below the special box
      let offsetY = groupBottomY;
      const unreachableX = BOX_ORIGIN_X + NODE_W / 2; // same left margin as the special box

      for (const n of children) {
        const h = NODE_H; // standardized height
        n.position({ x: snapToGrid(unreachableX), y: snapToGrid(offsetY + h / 2) });
        n.data('posX', snapToGrid(unreachableX));
        n.data('posY', snapToGrid(offsetY + h / 2));
        offsetY += h + GRID_SNAP;
      }
    }

    // ── Position Start passage to the right of the unreachable column ──
    // The start passage is the graph anchor. It sits to the right of the
    // left column (special box + unreachable) at the top of the canvas,
    // so the graph flows right and down from it — just like Twine.
    const startNode = cy.nodes().filter((n: any) => n.data('is_start') && !n.data('isGroup'));
    if (startNode.length > 0) {
      const sn = startNode[0];
      const existingPosX = sn.data('posX');
      const existingPosY = sn.data('posY');
      // Only reposition if the start passage has no position set at all
      // (i.e., this is a fresh layout with no saved positions)
      if (existingPosX == null || existingPosY == null) {
        sn.position({ x: snapToGrid(START_ANCHOR_X), y: snapToGrid(START_ANCHOR_Y + NODE_H / 2) });
        sn.data('posX', snapToGrid(START_ANCHOR_X));
        sn.data('posY', snapToGrid(START_ANCHOR_Y + NODE_H / 2));
      }
    }
  }, []);

  // ── Apply layout ──────────────────────────────────────────────────────────
  // The `onComplete` callback is invoked after the layout finishes — for
  // synchronous layouts it fires immediately, for animated layouts it fires
  // on the Cytoscape `layoutstop` event. This ensures `repositionGroups`
  // (passed as onComplete from buildGraph) never gets overridden by a
  // running layout animation.
  const applyLayout = useCallback((
    name: string,
    cy: cytoscape.Core,
    _positionedIds?: Set<string>,
    _unpositionedNodes?: any[],
    onComplete?: () => void,
  ) => {
    if (!cy || cy.nodes().length === 0) {
      onComplete?.();
      return;
    }

    if (name === 'position') {
      cy.nodes().forEach((n: any) => {
        if (n.data('isGroup')) return;
        const px = n.data('posX');
        const py = n.data('posY');
        if (px != null && py != null) {
          n.position({ x: px, y: py });
        }
      });

      const unpos = cy.nodes().filter((n: any) => !n.data('isGroup') && (n.data('posX') == null || n.data('posY') == null));
      if (unpos.length > 0 && unpos.length < cy.nodes().filter((n: any) => !n.data('isGroup')).length) {
        const fixed = cy.nodes().filter((n: any) => !n.data('isGroup') && n.data('posX') != null && n.data('posY') != null);
        const bb = fixed.boundingBox();
        const offsetX = bb.x2 + 150;
        const offsetY = bb.y1;

        const subElements: cytoscape.ElementDefinition[] = [];
        unpos.forEach((n: any) => {
          subElements.push({ data: { id: n.id(), label: n.data('label') } });
        });
        cy.edges().forEach((e: any) => {
          const src = e.data('source');
          const tgt = e.data('target');
          if (unpos.some((n: any) => n.id() === src) && unpos.some((n: any) => n.id() === tgt)) {
            subElements.push({ data: { id: e.id(), source: src, target: tgt } });
          }
        });

        if (subElements.length > 1) {
          const subCy = cytoscape({ container: undefined, elements: subElements, layout: { name: 'null' } });
          subCy.layout({ name: 'dagre', rankDir: 'TB', spacingFactor: 1.0, nodeSep: 50, rankSep: 70, animate: false } as any).run();
          subCy.nodes().forEach((sn: any) => {
            const mainNode = cy.getElementById(sn.id());
            if (mainNode.length > 0) {
              mainNode.position({ x: sn.position().x + offsetX, y: sn.position().y + offsetY });
            }
          });
          subCy.destroy();
        } else {
          unpos.forEach((n: any) => { n.position({ x: offsetX, y: offsetY }); });
        }
      } else if (unpos.length === cy.nodes().filter((n: any) => !n.data('isGroup')).length) {
        applyLayout('dagre', cy, _positionedIds, _unpositionedNodes, onComplete);
        return;
      }

      onComplete?.();
      return;
    }

    let opts: any;
    switch (name) {
      case 'dagre':
        opts = { name: 'dagre', rankDir: 'TB', spacingFactor: 1.0, nodeSep: 50, rankSep: 70, animate: true, animationDuration: 300 };
        break;
      case 'cose':
        opts = { name: 'cose', animate: true, animationDuration: 400, nodeRepulsion: 10000, idealEdgeLength: 120, gravity: 0.2 };
        break;
      default:
        opts = { name: 'dagre', spacingFactor: 1.0, animate: true };
    }

    const layoutInst = cy.layout(opts);
    // Fire onComplete when the animated layout finishes, so that
    // repositionGroups doesn't get overridden by the animation.
    if (onComplete) {
      layoutInst.one('layoutstop', onComplete);
    }
    layoutInst.run();
  }, []);

  // ── Update graph when data changes ────────────────────────────────────────
  useEffect(() => {
    if (graphData) {
      buildGraph(graphData);
    }
  }, [graphData, buildGraph]);

  // ── Apply layout when it changes ──────────────────────────────────────────
  useEffect(() => {
    const cy = cyRef.current;
    if (!cy || cy.nodes().length === 0 || !currentDataRef.current) return;
    applyLayout(layout, cy, new Set(), []);
  }, [layout, applyLayout]);

  // ── Search / filter ───────────────────────────────────────────────────────
  useEffect(() => {
    const cy = cyRef.current;
    if (!cy || !currentDataRef.current) return;

    const q = searchQuery.toLowerCase().trim();
    if (q === '') {
      cy.elements().removeClass('dimmed highlighted');
      return;
    }

    // Dim everything, then highlight matches (skip group parents)
    cy.nodes().addClass('dimmed');
    cy.edges().addClass('dimmed');

    const matched = cy.nodes().filter((n: any) => {
      if (n.data('isGroup')) return false;
      const label = (n.data('label') || '').toLowerCase();
      const tags: string[] = n.data('tags') || [];
      return label.includes(q) || tags.some(t => t.toLowerCase().includes(q));
    });

    matched.removeClass('dimmed').addClass('highlighted');
    matched.neighborhood('edge').removeClass('dimmed');
    matched.neighborhood('node').removeClass('dimmed');
  }, [searchQuery]);

  // ── Fit to view (manual only — triggered by toolbar button) ───────────────
  useEffect(() => {
    const cy = cyRef.current;
    if (cy && fitRequested > 0) {
      cy.fit(undefined, 30);
    }
  }, [fitRequested]);

  // ── Focus on a passage node (triggered by extension focusNode message) ────
  useEffect(() => {
    if (focusRequested > 0 && focusPassageName) {
      focusOnNode(focusPassageName);
    }
  }, [focusRequested, focusPassageName, focusOnNode]);

  // ── Save all positions to workspace ─────────────────────────────────────
  // When the user clicks "Save", collect the current position of every
  // non-group node from the Cytoscape instance and send them to the
  // extension. The extension forwards to the LSP server which writes
  // {"position":"x,y"} metadata into each passage header.
  useEffect(() => {
    if (saveRequested <= 0) return;
    const cy = cyRef.current;
    if (!cy) return;

    const updates: KnotPositionUpdate[] = [];
    cy.nodes().forEach((n: any) => {
      if (n.data('isGroup')) return;
      const id = n.data('id');
      const pos = n.position();
      if (id && pos) {
        updates.push({
          passage_name: id,
          position_x: snapToGrid(pos.x),
          position_y: snapToGrid(pos.y),
        });
      }
    });

    if (updates.length > 0) {
      vscode.postMessage({ command: 'saveAllPositions', updates });
    }
  }, [saveRequested]);

  return (
    <div
      ref={containerRef}
      id="cy"
      style={{
        position: 'relative',
        flex: '1 1 0%',
        minHeight: 0,
        width: '100%',
        overflow: 'hidden',
      }}
    />
  );
}
