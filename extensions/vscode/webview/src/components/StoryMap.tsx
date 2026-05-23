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

// ── Special passage classification ──────────────────────────────────────────
// Twine-core special passages defined by the Twee specification.
// Everything else that's is_special but NOT in this list is format-specific
// (e.g., SugarCube's StoryInit, StoryInterface, etc.).
const TWINE_CORE_SPECIALS = new Set([
  'StoryTitle', 'StoryData', 'StoryStylesheet', 'StoryJavaScript',
  'StorySettings', 'StoryIncludes',
]);

type SpecialGroup = 'twine_core' | 'format_special' | null;

function classifySpecial(label: string, isMetadata: boolean, isStart: boolean): SpecialGroup {
  // The start passage is a special case — it should NOT be placed in any
  // group box. According to Twine's engine design, special passages don't
  // typically have edges to the start passage. Keeping start outside the
  // bundles keeps the graph clean since start has many outgoing edges to
  // user-defined passages.
  if (isStart) return null;
  // Metadata passages (tagged [stylesheet], [script], etc.) are Twine-standard
  if (isMetadata) return 'twine_core';
  if (TWINE_CORE_SPECIALS.has(label)) return 'twine_core';
  return 'format_special';
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
  groupBorder: '#666666',
  groupBg:     'rgba(255,255,255,0.03)',
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
      'text-max-width': '90px',
      'width': 'data(w)',
      'height': 'data(h)',
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
  {
    selector: '.group-box',
    style: {
      'background-color': COLORS.groupBg,
      'background-opacity': 1,
      'border-width': 1,
      'border-style': 'dashed',
      'border-color': COLORS.groupBorder,
      'border-opacity': 0.6,
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
];

// ── Compound group IDs ──────────────────────────────────────────────────────
const GROUP_TWINE_CORE = '__group_twine_core';
const GROUP_FORMAT_SPECIAL = '__group_format_special';
const GROUP_UNREACHABLE = '__group_unreachable';

interface StoryMapProps {
  graphData: KnotGraphResponse | null;
  layout: string;
  searchQuery: string;
  fitRequested: number;
  onLayoutChange: (layout: string) => void;
}

export default function StoryMap({
  graphData,
  layout,
  searchQuery,
  fitRequested,
  onLayoutChange,
}: StoryMapProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<cytoscape.Core | null>(null);
  const currentDataRef = useRef<KnotGraphResponse | null>(null);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

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
    });

    cyRef.current = cy;
    console.log('[StoryMap] Cytoscape instance created successfully');

    // Click → open passage (ignore compound group parents)
    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      if (node.data('isGroup')) return; // skip group parent nodes
      const file = node.data('file');
      const line = node.data('line') || 0;
      if (file) {
        vscode.postMessage({ command: 'openPassage', file, line });
      }
    });

    // Drag end → write back position (snapped to grid)
    cy.on('dragfree', 'node', (evt) => {
      const dragged = evt.target;
      if (!dragged || !dragged.data('id') || dragged.data('isGroup')) return;
      const pos = dragged.position();
      const oldX = dragged.data('posX');
      const oldY = dragged.data('posY');
      const newX = snapToGrid(pos.x);
      const newY = snapToGrid(pos.y);
      dragged.position({ x: newX, y: newY });
      if (oldX == null || oldY == null ||
          Math.abs(newX - oldX) > 0.5 || Math.abs(newY - oldY) > 0.5) {
        dragged.data('posX', newX);
        dragged.data('posY', newY);
        const updates: KnotPositionUpdate[] = [
          { passage_name: dragged.data('id'), position_x: newX, position_y: newY },
        ];
        vscode.postMessage({ command: 'updatePositions', updates });
      }
    });

    // Snap node to grid in real-time while dragging (Twine-style)
    cy.on('drag', 'node', (evt) => {
      const node = evt.target;
      if (node.data('isGroup')) return; // don't snap group parents
      const pos = node.position();
      node.position({ x: snapToGrid(pos.x), y: snapToGrid(pos.y) });
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
    const twineCoreChildren: string[] = [];
    const formatSpecialChildren: string[] = [];
    const unreachableChildren: string[] = [];

    const nodeIds = new Set<string>();

    for (const n of nodes) {
      nodeIds.add(n.id);

      const isStart = n.is_start || (n.id === 'Start' || n.label === 'Start');

      // Classify special/unreachable into groups.
      // The start passage is NEVER placed in a group box — it sits outside
      // so its many outgoing edges to user passages don't clutter the bundles.
      // Priority: special > unreachable (special passages stay in their
      // category group even if unreachable; only non-special unreachables
      // go to the unreachable group)
      if ((n.is_special || n.is_metadata) && !isStart) {
        const group = classifySpecial(n.label, !!n.is_metadata, isStart);
        if (group === 'twine_core') {
          twineCoreChildren.push(n.id);
        } else if (group === 'format_special') {
          formatSpecialChildren.push(n.id);
        }
      } else if (n.is_unreachable) {
        unreachableChildren.push(n.id);
      }

      const color = getNodeColor({ ...n, is_start: isStart });
      const size = 60;
      const hasPos = n.position_x != null && n.position_y != null;
      const isGameLoop = gameLoopMembers.has(n.id);

      // Determine compound parent — start passage never gets a parent
      let parent: string | undefined;
      if (!isStart) {
        if (twineCoreChildren.includes(n.id)) {
          parent = GROUP_TWINE_CORE;
        } else if (formatSpecialChildren.includes(n.id)) {
          parent = GROUP_FORMAT_SPECIAL;
        } else if (unreachableChildren.includes(n.id)) {
          parent = GROUP_UNREACHABLE;
        }
      }

      const nodeClasses: string[] = [];
      if (isGameLoop) nodeClasses.push('game_loop');
      if (isStart) nodeClasses.push('is_start');

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
          w: n.is_metadata ? 40 : size,
          h: n.is_metadata ? 40 : size * 0.55,
          posX: n.position_x != null ? snapToGrid(n.position_x) : null,
          posY: n.position_y != null ? snapToGrid(n.position_y) : null,
          var_writes: n.var_writes || [],
          var_reads: n.var_reads || [],
          parent,
        },
        classes: nodeClasses,
      };

      if (hasPos) {
        el.position = { x: snapToGrid(n.position_x!), y: snapToGrid(n.position_y!) };
        positionedIds.add(n.id);
      } else {
        unpositioned.push(n);
      }

      elements.push(el);
    }

    // ── Add compound group parent nodes ─────────────────────────────────
    if (twineCoreChildren.length > 0) {
      elements.push({
        data: { id: GROUP_TWINE_CORE, label: 'Twine Core', isGroup: true },
        classes: ['group-box'],
        position: { x: 0, y: 0 }, // will be repositioned
      });
    }
    if (formatSpecialChildren.length > 0) {
      elements.push({
        data: { id: GROUP_FORMAT_SPECIAL, label: 'Format Specials', isGroup: true },
        classes: ['group-box'],
        position: { x: 0, y: 0 },
      });
    }
    if (unreachableChildren.length > 0) {
      elements.push({
        data: { id: GROUP_UNREACHABLE, label: 'Unreachable', isGroup: true },
        classes: ['group-box'],
        position: { x: 0, y: 0 },
      });
    }

    // ── Edges ───────────────────────────────────────────────────────────
    const usedEdgeIds = new Set<string>();
    let edgeIndex = 0;

    for (const e of edges) {
      // Skip edges referencing compound group parents
      if (!nodeIds.has(e.source) || !nodeIds.has(e.target)) {
        console.warn('[StoryMap] Skipping edge with missing endpoint:', e.source, '->', e.target, '(' + e.edge_type + ')');
        continue;
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

    applyLayout(chosenLayout, cy, positionedIds, unpositioned);

    // ── Reposition compound groups ──────────────────────────────────────
    repositionGroups(cy, twineCoreChildren, formatSpecialChildren, unreachableChildren);

    updateGridBackground();

    // Pan to origin like Twine, then fit
    requestAnimationFrame(() => {
      if (cyRef.current && cyRef.current.nodes().length > 0) {
        cyRef.current.resize();
        cyRef.current.fit(undefined, 30);
        console.log('[StoryMap] Viewport fit applied');
      }
    });
  }, [onLayoutChange, updateGridBackground]);

  // ── Reposition compound groups after layout ───────────────────────────────
  // Special passages go to the top-left in labeled boxes.
  // Start passage goes below the special groups but outside any box.
  // Unreachable passages go to the bottom-right in a labeled box.
  const repositionGroups = useCallback((
    cy: cytoscape.Core,
    twineCoreIds: string[],
    formatSpecialIds: string[],
    unreachableIds: string[],
  ) => {
    // ── Position Twine Core group at top-left ───────────────────────────
    let groupY = GRID_SNAP * 2; // start at y=40

    if (twineCoreIds.length > 0) {
      const children = twineCoreIds.map(id => cy.getElementById(id)).filter(n => n.length > 0);
      // Sort alphabetically for consistent layout
      children.sort((a, b) => (a.data('label') || '').localeCompare(b.data('label') || ''));

      let offsetX = GRID_SNAP * 2;
      let rowMaxH = 0;
      for (const n of children) {
        const w = n.width();
        const h = n.height();
        if (offsetX + w > 500 && offsetX > GRID_SNAP * 2) {
          offsetX = GRID_SNAP * 2;
          groupY += rowMaxH + GRID_SNAP;
          rowMaxH = 0;
        }
        n.position({ x: snapToGrid(offsetX + w / 2), y: snapToGrid(groupY + h / 2) });
        n.data('posX', snapToGrid(offsetX + w / 2));
        n.data('posY', snapToGrid(groupY + h / 2));
        offsetX += w + GRID_SNAP;
        rowMaxH = Math.max(rowMaxH, h);
      }
      // Advance Y past the Twine Core group (compound padding + row height)
      const twineCoreBB = cy.getElementById(GROUP_TWINE_CORE).boundingBox();
      groupY = twineCoreBB.y2 + GRID_SNAP;
    }

    // ── Position Format Specials group below Twine Core ─────────────────
    if (formatSpecialIds.length > 0) {
      const children = formatSpecialIds.map(id => cy.getElementById(id)).filter(n => n.length > 0);
      children.sort((a, b) => (a.data('label') || '').localeCompare(b.data('label') || ''));

      let offsetX = GRID_SNAP * 2;
      let rowMaxH = 0;
      for (const n of children) {
        const w = n.width();
        const h = n.height();
        if (offsetX + w > 500 && offsetX > GRID_SNAP * 2) {
          offsetX = GRID_SNAP * 2;
          groupY += rowMaxH + GRID_SNAP;
          rowMaxH = 0;
        }
        n.position({ x: snapToGrid(offsetX + w / 2), y: snapToGrid(groupY + h / 2) });
        n.data('posX', snapToGrid(offsetX + w / 2));
        n.data('posY', snapToGrid(groupY + h / 2));
        offsetX += w + GRID_SNAP;
        rowMaxH = Math.max(rowMaxH, h);
      }
      // Advance Y past the Format Specials group
      const formatGroupEl = cy.getElementById(GROUP_FORMAT_SPECIAL);
      if (formatGroupEl.length > 0) {
        const formatBB = formatGroupEl.boundingBox();
        groupY = formatBB.y2 + GRID_SNAP;
      }
    }

    // ── Position Start passage below the special groups ─────────────────
    // The start passage is placed outside any group box, right below the
    // special passage area, so its outgoing edges don't clutter the boxes.
    const startNode = cy.nodes().filter((n: any) => n.data('is_start') && !n.data('isGroup'));
    if (startNode.length > 0) {
      const sn = startNode[0];
      const w = sn.width();
      const h = sn.height();
      sn.position({ x: snapToGrid(GRID_SNAP * 2 + w / 2), y: snapToGrid(groupY + h / 2) });
      sn.data('posX', snapToGrid(GRID_SNAP * 2 + w / 2));
      sn.data('posY', snapToGrid(groupY + h / 2));
      // Advance Y past the start node
      groupY += h + GRID_SNAP;
    }

    // ── Position Unreachable group to the right of main graph ───────────
    if (unreachableIds.length > 0) {
      // Find the bounding box of all non-unreachable, non-special nodes
      const reachableNodes = cy.nodes().filter((n: any) =>
        !n.data('isGroup') && !n.data('is_unreachable') && !n.data('is_special') && !n.data('is_metadata')
      );
      let baseX = 400;
      let baseY = GRID_SNAP * 2;

      if (reachableNodes.length > 0) {
        const bb = reachableNodes.boundingBox();
        baseX = bb.x2 + GRID_SNAP * 4;
        baseY = bb.y1;
      }

      const children = unreachableIds.map(id => cy.getElementById(id)).filter(n => n.length > 0);

      let offsetX = baseX;
      let offsetY = baseY;
      let rowMaxH = 0;
      for (const n of children) {
        const w = n.width();
        const h = n.height();
        if (offsetX + w > baseX + 400 && offsetX > baseX) {
          offsetX = baseX;
          offsetY += rowMaxH + GRID_SNAP;
          rowMaxH = 0;
        }
        n.position({ x: snapToGrid(offsetX + w / 2), y: snapToGrid(offsetY + h / 2) });
        n.data('posX', snapToGrid(offsetX + w / 2));
        n.data('posY', snapToGrid(offsetY + h / 2));
        offsetX += w + GRID_SNAP;
        rowMaxH = Math.max(rowMaxH, h);
      }
    }
  }, []);

  // ── Apply layout ──────────────────────────────────────────────────────────
  const applyLayout = useCallback((
    name: string,
    cy: cytoscape.Core,
    _positionedIds?: Set<string>,
    _unpositionedNodes?: any[],
  ) => {
    if (!cy || cy.nodes().length === 0) return;

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
        applyLayout('dagre', cy, _positionedIds, _unpositionedNodes);
        return;
      }

      cy.fit(undefined, 30);
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
    cy.layout(opts).run();
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

  // ── Fit to view ───────────────────────────────────────────────────────────
  useEffect(() => {
    const cy = cyRef.current;
    if (cy && fitRequested > 0) {
      cy.fit(undefined, 30);
    }
  }, [fitRequested]);

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
