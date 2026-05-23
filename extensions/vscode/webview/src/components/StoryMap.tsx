import React, { useEffect, useRef, useCallback } from 'react';
import cytoscape from 'cytoscape';
import dagre from 'cytoscape-dagre';
import { KnotGraphResponse, KnotPositionUpdate } from '../types';
import { vscode } from '../App';

// Register the dagre layout extension
cytoscape.use(dagre);

// ── Twine-inspired color palette ────────────────────────────────────────────
const COLORS = {
  normal:     '#4fc3f7',
  start:      '#66bb6a',
  special:    '#ffb74d',
  metadata:   '#ce93d8',
  unreachable:'#555555',
  broken:     '#f14c4c',
  gameLoop:   '#ff7043',
};

function getNodeColor(data: {
  is_metadata?: boolean;
  is_unreachable?: boolean;
  is_special?: boolean;
  is_start?: boolean;
}): string {
  if (data.is_metadata)    return COLORS.metadata;
  if (data.is_unreachable) return COLORS.unreachable;
  if (data.is_special)     return COLORS.special;
  if (data.is_start)       return COLORS.start;
  return COLORS.normal;
}

// ── Cytoscape stylesheet (faithfully ported from inline HTML) ───────────────
// Using `any[]` because Cytoscape's style types are extremely strict about
// property names and don't easily accommodate mappable data properties like
// `data(bgColor)`. The runtime behavior is correct regardless.
const CYTOSCAPE_STYLE: any[] = [
  {
    selector: 'node',
    style: {
      'label': 'data(label)',
      'background-color': 'data(bgColor)',
      'color': '#e0e0e0',
      'text-valign': 'center',
      'text-halign': 'center',
      'font-size': '11px',
      'text-wrap': 'ellipsis',
      'text-max-width': '90px',
      'width': 'data(w)',
      'height': 'data(h)',
      'shape': 'round-rectangle',
      'text-outline-color': '#1e1e1e',
      'text-outline-width': '2px',
      'border-width': '2px',
      'border-color': 'data(borderColor)',
      'font-family': '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
    },
  },
  {
    selector: 'edge',
    style: {
      'width': 1.5,
      'line-color': '#555',
      'target-arrow-color': '#555',
      'target-arrow-shape': 'triangle',
      'arrow-scale': 0.7,
      'curve-style': 'bezier',
      'opacity': 0.6,
    },
  },
  {
    selector: 'edge.is_broken',
    style: {
      'line-color': COLORS.broken,
      'target-arrow-color': COLORS.broken,
      'line-style': 'dashed',
      'opacity': 0.85,
    },
  },
  {
    selector: 'edge.is_upstream',
    style: {
      'line-style': 'dashed',
      'line-color': '#888',
      'target-arrow-color': '#888',
      'opacity': 0.4,
    },
  },
  {
    selector: 'edge.is_call',
    style: {
      'line-color': '#ab47bc',
      'target-arrow-color': '#ab47bc',
      'line-style': 'dotted',
    },
  },
  {
    selector: 'edge.is_include',
    style: {
      'line-color': '#26a69a',
      'target-arrow-color': '#26a69a',
      'line-style': 'dotted',
    },
  },
  {
    selector: 'edge.is_jump',
    style: {
      'line-color': '#ffa726',
      'target-arrow-color': '#ffa726',
    },
  },
  {
    selector: 'edge.has_label',
    style: {
      'label': 'data(displayText)',
      'font-size': '8px',
      'text-rotation': 'autorotate',
      'text-outline-color': '#1e1e1e',
      'text-outline-width': '1px',
      'color': '#999',
    },
  },
  {
    selector: 'node.highlighted',
    style: { 'border-color': '#fff', 'border-width': '3px', 'z-index': 999 },
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
];

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
  const gridCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  // ── Grid background renderer ──────────────────────────────────────────────
  const drawGrid = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;

    // Remove old grid canvas
    if (gridCanvasRef.current) {
      gridCanvasRef.current.remove();
    }

    const canvas = document.createElement('canvas');
    canvas.id = 'gridCanvas';
    canvas.style.position = 'absolute';
    canvas.style.top = '0';
    canvas.style.left = '0';
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.pointerEvents = 'none';
    canvas.style.zIndex = '0';
    container.insertBefore(canvas, container.firstChild);
    gridCanvasRef.current = canvas;

    const rect = container.getBoundingClientRect();
    canvas.width = rect.width;
    canvas.height = rect.height;
    const ctx = canvas.getContext('2d');

    if (ctx) {
      // Twine-style dot grid
      const spacing = 20;
      const gridDotColor = getComputedStyle(document.documentElement)
        .getPropertyValue('--grid-dot').trim() || 'rgba(255,255,255,0.07)';
      ctx.fillStyle = gridDotColor;
      for (let x = spacing; x < rect.width; x += spacing) {
        for (let y = spacing; y < rect.height; y += spacing) {
          ctx.beginPath();
          ctx.arc(x, y, 1, 0, Math.PI * 2);
          ctx.fill();
        }
      }
    }
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

    // Click → open passage
    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      const file = node.data('file');
      const line = node.data('line') || 0;
      if (file) {
        vscode.postMessage({ command: 'openPassage', file, line });
      }
    });

    // Drag end → write back position
    cy.on('dragfree', 'node', (evt) => {
      const dragged = evt.target;
      if (!dragged || !dragged.data('id')) return;
      const pos = dragged.position();
      const oldX = dragged.data('posX');
      const oldY = dragged.data('posY');
      const newX = Math.round(pos.x * 100) / 100;
      const newY = Math.round(pos.y * 100) / 100;
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

    // Tooltip: show on mouseover
    cy.on('mouseover', 'node', (evt) => {
      const d = evt.target.data();
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
        if (d.in_degree > 0)  parts.push('In: ' + d.in_degree);
        if (d.out_degree > 0) parts.push('Out: ' + d.out_degree);
        if (d.is_special)     parts.push('Special');
        if (d.is_metadata)    parts.push('Metadata');
        if (d.is_unreachable) parts.push('Unreachable');
        if (d.var_writes && d.var_writes.length > 0) parts.push('Writes: ' + d.var_writes.join(', '));
        if (d.var_reads && d.var_reads.length > 0)   parts.push('Reads: ' + d.var_reads.join(', '));
        if (d.posX != null && d.posY != null) parts.push('(' + Math.round(d.posX) + ', ' + Math.round(d.posY) + ')');
        metaEl.textContent = parts.join(' | ');
      }

      tip.style.display = 'block';
    });

    // Tooltip: hide on mouseout
    cy.on('mouseout', 'node', () => {
      const tip = document.getElementById('tooltip');
      if (tip) tip.style.display = 'none';
    });

    // Tooltip: hide on drag
    cy.on('tapdrag', () => {
      const tip = document.getElementById('tooltip');
      if (tip) tip.style.display = 'none';
    });

    // Redraw grid + notify Cytoscape on resize
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry) {
        const { width, height } = entry.contentRect;
        console.log('[StoryMap] ResizeObserver fired:', Math.round(width), 'x', Math.round(height));
      }
      drawGrid();
      // Cytoscape must be told when its container changes size
      if (cyRef.current) {
        cyRef.current.resize();
      }
    });
    ro.observe(containerRef.current);

    // Deferred resize: VS Code sidebar webviews may not have their final
    // dimensions at the time this effect runs. Scheduling a resize after
    // the browser has completed layout ensures Cytoscape sees the real
    // container size.
    requestAnimationFrame(() => {
      if (cyRef.current) {
        cyRef.current.resize();
        drawGrid();
        console.log('[StoryMap] Deferred resize applied, container:', containerRef.current?.getBoundingClientRect());
      }
    });

    return () => {
      ro.disconnect();
      cy.destroy();
      cyRef.current = null;
    };
  }, [drawGrid]);

  // ── Build the graph from server data ──────────────────────────────────────
  // NOTE: layout and onLayoutChange are intentionally NOT in the dependency
  // array to prevent an infinite re-render loop. The buildGraph callback
  // uses layoutRef.current instead of the layout prop, and calls
  // onLayoutChange only when the server-suggested layout differs from the
  // current layout (a one-way sync, not a cycle).
  const buildGraph = useCallback((data: KnotGraphResponse) => {
    const cy = cyRef.current;
    if (!cy) {
      console.warn('[StoryMap] buildGraph called but Cytoscape not initialized');
      return;
    }

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

    // Build a set of valid node IDs for edge validation
    const nodeIds = new Set<string>();

    /* ── Nodes ─────────────────────────────────────────────────────────── */
    for (const n of nodes) {
      nodeIds.add(n.id);
      const isStart = (n.id === 'Start' || n.label === 'Start');
      const color = getNodeColor({ ...n, is_start: isStart });
      const size = Math.max(50, Math.min(100, 40 + Math.max(n.out_degree || 0, n.in_degree || 0) * 6));
      const hasPos = n.position_x != null && n.position_y != null;
      const isGameLoop = gameLoopMembers.has(n.id);

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
          bgColor: n.is_unreachable ? '#2a2a2a' : '#2d2d30',
          borderColor: color,
          w: n.is_metadata ? 40 : size,
          h: n.is_metadata ? 40 : size * 0.55,
          posX: n.position_x,
          posY: n.position_y,
          var_writes: n.var_writes || [],
          var_reads: n.var_reads || [],
        },
        classes: isGameLoop ? ['game_loop'] : [],
      };

      if (hasPos) {
        el.position = { x: n.position_x!, y: n.position_y! };
        positionedIds.add(n.id);
      } else {
        unpositioned.push(n);
      }

      elements.push(el);
    }

    /* ── Edges ─────────────────────────────────────────────────────────── */
    // Track used edge IDs to prevent duplicates when multiple edges exist
    // between the same pair of nodes (e.g., a nav link AND an <<include>>)
    const usedEdgeIds = new Set<string>();
    let edgeIndex = 0;

    for (const e of edges) {
      // BUG FIX: Skip edges whose source or target node doesn't exist in
      // the graph. Broken-link edges often reference passages that don't
      // exist as nodes, and Cytoscape throws when adding an edge with a
      // non-existent endpoint.
      if (!nodeIds.has(e.source) || !nodeIds.has(e.target)) {
        console.warn('[StoryMap] Skipping edge with missing endpoint:', e.source, '->', e.target, '(' + e.edge_type + ')');
        continue;
      }

      // Build a unique edge ID. Multiple edges between the same pair are
      // possible (e.g., navigation + include), so we append the edge type
      // and a sequential index to guarantee uniqueness.
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

    // BUG FIX: Wrap cy.add() in try/catch — if any element definition is
    // invalid (e.g., an edge references a node that was somehow missed by
    // our validation), Cytoscape will throw. Previously this crashed the
    // entire React render cycle silently.
    try {
      cy.add(elements);
      console.log('[StoryMap] Added', elements.length, 'elements to Cytoscape');
    } catch (err) {
      console.error('[StoryMap] cy.add() failed:', err);
      vscode.postMessage({ command: 'log', level: 'error', message: '[StoryMap] cy.add() failed: ' + String(err) });
      return; // Don't try to layout an empty/broken graph
    }

    /* ── Layout ────────────────────────────────────────────────────────── */
    const hasAnyPositions = positionedIds.size > 0;
    const chosenLayout = data.layout || (hasAnyPositions ? 'position' : 'dagre');

    // One-way sync: only update parent if the server-suggested layout
    // differs from the current layout. This uses layoutRef to avoid
    // depending on the layout prop (which would cause a re-render loop).
    if (chosenLayout !== layoutRef.current) {
      onLayoutChange(chosenLayout);
    }

    applyLayout(chosenLayout, cy, positionedIds, unpositioned);
    drawGrid();

    // Deferred fit: ensure Cytoscape recalculates after the browser has
    // processed the DOM changes from adding elements and running layout.
    requestAnimationFrame(() => {
      if (cyRef.current && cyRef.current.nodes().length > 0) {
        cyRef.current.resize();
        cyRef.current.fit(undefined, 30);
        console.log('[StoryMap] Deferred fit applied, nodes:', cyRef.current.nodes().length);
      }
    });
  }, [onLayoutChange, drawGrid]); // intentionally excludes `layout`

  // ── Apply layout ──────────────────────────────────────────────────────────
  const applyLayout = useCallback((
    name: string,
    cy: cytoscape.Core,
    _positionedIds?: Set<string>,
    _unpositionedNodes?: any[],
  ) => {
    if (!cy || cy.nodes().length === 0) return;

    if (name === 'position') {
      // ── Saved layout: positioned nodes stay, unpositioned get auto-arranged
      cy.nodes().forEach((n: any) => {
        const px = n.data('posX');
        const py = n.data('posY');
        if (px != null && py != null) {
          n.position({ x: px, y: py });
        }
      });

      const unpos = cy.nodes().filter((n: any) => n.data('posX') == null || n.data('posY') == null);
      if (unpos.length > 0 && unpos.length < cy.nodes().length) {
        const fixed = cy.nodes().filter((n: any) => n.data('posX') != null && n.data('posY') != null);
        const bb = fixed.boundingBox();
        const offsetX = bb.x2 + 150;
        const offsetY = bb.y1;

        const subNodes = unpos;
        const subEdgeIds = new Set<string>();
        subNodes.forEach((n: any) => { subEdgeIds.add(n.id()); });

        // Build a temporary sub-cytoscape for dagre layout
        const subElements: cytoscape.ElementDefinition[] = [];
        subNodes.forEach((n: any) => {
          subElements.push({ data: { id: n.id(), label: n.data('label') } });
        });
        // Edges between unpositioned nodes
        cy.edges().forEach((e: any) => {
          if (subEdgeIds.has(e.data('source')) && subEdgeIds.has(e.data('target'))) {
            subElements.push({
              data: { id: e.id(), source: e.data('source'), target: e.data('target') },
            });
          }
        });

        if (subElements.length > 1) {
          const subCy = cytoscape({
            container: undefined, // headless
            elements: subElements,
            layout: { name: 'null' },
          });
          subCy.layout({
            name: 'dagre',
            rankDir: 'TB',
            spacingFactor: 1.0,
            nodeSep: 50,
            rankSep: 70,
            animate: false,
          } as any).run();

          subCy.nodes().forEach((sn: any) => {
            const mainNode = cy.getElementById(sn.id());
            if (mainNode.length > 0) {
              mainNode.position({
                x: sn.position().x + offsetX,
                y: sn.position().y + offsetY,
              });
            }
          });
          subCy.destroy();
        } else {
          unpos.forEach((n: any) => { n.position({ x: offsetX, y: offsetY }); });
        }
      } else if (unpos.length === cy.nodes().length) {
        // ALL nodes unpositioned — fall through to dagre
        applyLayout('dagre', cy, _positionedIds, _unpositionedNodes);
        return;
      }

      cy.fit(undefined, 30);
      return;
    }

    // ── Algorithmic layouts ──────────────────────────────────────────────────
    let opts: any;
    switch (name) {
      case 'dagre':
        opts = {
          name: 'dagre',
          rankDir: 'TB',
          spacingFactor: 1.0,
          nodeSep: 50,
          rankSep: 70,
          animate: true,
          animationDuration: 300,
        };
        break;
      case 'cose':
        opts = {
          name: 'cose',
          animate: true,
          animationDuration: 400,
          nodeRepulsion: 10000,
          idealEdgeLength: 120,
          gravity: 0.2,
        };
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

    // Don't rebuild the graph, just apply a different layout
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

    cy.nodes().addClass('dimmed');
    cy.edges().addClass('dimmed');

    const matched = cy.nodes().filter((n: any) => {
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
