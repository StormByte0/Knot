//! Story Map webview provider for the Knot extension.
//!
//! This module implements a VS Code webview panel that renders an interactive
//! passage graph using Cytoscape.js, inspired by the Twine 2 story editor:
//!
//! - Dot grid background (panning canvas feel)
//! - Origin at top-left (0,0), start passage near origin
//! - Position-based layout using Twee passage `<x,y>` metadata
//! - Automatic dagre layout for passages without position data
//! - Click-to-navigate (clicking a node opens the passage in the editor)
//! - Color-coded nodes (normal, special, metadata, unreachable)
//! - Red dashed edges for broken links
//! - Drag-to-reposition with position write-back (Twine-compatible `<x,y>`)
//! - Search/filter passages by name or tag
//! - Zoom-to-fit and layout switching controls

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotGraphResponse, KnotUpdatePositionsParams, KnotUpdatePositionsResponse } from './types';

// ---------------------------------------------------------------------------
// Story Map webview provider
// ---------------------------------------------------------------------------

function getNonce(): string {
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    let nonce = '';
    for (let i = 0; i < 32; i++) {
        nonce += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return nonce;
}

export class StoryMapProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.storyMap';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _graphData: KnotGraphResponse | null = null;

    constructor(private readonly _extensionUri: vscode.Uri) {}

    /** Set the language client reference so we can send LSP requests. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
        // If the view is already resolved, refresh the graph immediately
        if (this._view) {
            this.refreshGraph();
        }
    }

    /** Resolve the webview view (called when the sidebar panel is opened). */
    public resolveWebviewView(
        webviewView: vscode.WebviewView,
        _context: vscode.WebviewViewResolveContext,
        _token: vscode.CancellationToken,
    ) {
        this._view = webviewView;

        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [this._extensionUri],
        };

        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview, false);

        // Handle messages from the webview
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'openPassage': {
                    const { file, line } = message;
                    if (file) {
                        const uri = vscode.Uri.parse(file);
                        const doc = await vscode.workspace.openTextDocument(uri);
                        await vscode.window.showTextDocument(doc, {
                            preview: true,
                            selection: new vscode.Range(line, 0, line, 200),
                        });
                    }
                    break;
                }
                case 'refreshGraph': {
                    await this.refreshGraph();
                    break;
                }
                case 'openFullView': {
                    await vscode.commands.executeCommand('knot.openStoryMap');
                    break;
                }
                case 'updatePositions': {
                    const { updates } = message;
                    if (this._client && this._client.isRunning() && updates && updates.length > 0) {
                        const workspaceFolders = vscode.workspace.workspaceFolders;
                        if (workspaceFolders && workspaceFolders.length > 0) {
                            try {
                                const params: KnotUpdatePositionsParams = {
                                    workspace_uri: workspaceFolders[0].uri.toString(),
                                    updates: updates,
                                };
                                await this._client.sendRequest<KnotUpdatePositionsResponse>('knot/updatePositions', params);
                            } catch (e) {
                                console.error('[Knot] Failed to update passage positions:', e);
                            }
                        }
                    }
                    break;
                }
            }
        });

        // If client is already set, refresh immediately
        if (this._client) {
            this.refreshGraph();
        }
    }

    /** Fetch graph data from the language server and push to the webview. */
    public async refreshGraph() {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        try {
            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                return;
            }

            const result = await this._client.sendRequest<KnotGraphResponse>('knot/graph', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

            this._graphData = result;
            this._postGraphData();
        } catch (e) {
            console.error('[Knot] Failed to fetch story graph:', e);
        }
    }

    /** Post the current graph data to the webview. */
    private _postGraphData() {
        if (this._view && this._graphData) {
            this._view.webview.postMessage({
                command: 'updateGraph',
                data: this._graphData,
            });
        }
    }

    /** Generate HTML for the full-view (detached) webview panel. */
    public getFullViewHtml(webview: vscode.Webview, extensionUri: vscode.Uri): string {
        return this._getHtmlForWebview(webview, true);
    }

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(webview: vscode.Webview, isFullView: boolean): string {
        const cytoscapeLocal = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'cytoscape.min.js'));
        const dagreLocal = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'cytoscape-dagre.min.js'));
        const cytoscapeScript = cytoscapeLocal.toString();
        const dagreScript = dagreLocal.toString();
        const nonce = getNonce();

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src ${webview.cspSource} 'nonce-${nonce}'; img-src ${webview.cspSource} data:; connect-src ${webview.cspSource};">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Story Map</title>
    <script nonce="${nonce}" src="${cytoscapeScript}"></script>
    <script nonce="${nonce}" src="${dagreScript}"></script>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }

        :root {
            --bg: var(--vscode-editor-background, #1e1e1e);
            --fg: var(--vscode-editor-foreground, #d4d4d4);
            --accent: var(--vscode-focusBorder, #007acc);
            --border: var(--vscode-panel-border, #474747);
            --muted: var(--vscode-descriptionForeground, #8b8b8b);
            --card: var(--vscode-sideBar-background, #252526);
            --error: var(--vscode-errorForeground, #f14c4c);
            --grid-dot: rgba(255,255,255,0.07);
            --node-bg: #2d2d30;
            --node-border: #3e3e42;
            --node-selected: var(--accent);
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 13px;
            height: 100vh;
            display: flex;
            flex-direction: column;
            overflow: hidden;
        }

        /* ── Toolbar ─────────────────────────────────────────── */
        #toolbar {
            display: flex;
            align-items: center;
            gap: 4px;
            padding: 4px 6px;
            background: var(--card);
            border-bottom: 1px solid var(--border);
            flex-shrink: 0;
            z-index: 10;
        }

        #toolbar input {
            flex: 1;
            min-width: 60px;
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 6px;
            border-radius: 3px;
            font-size: 11px;
            outline: none;
        }
        #toolbar input:focus { border-color: var(--accent); }

        #toolbar button {
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 2px 6px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
            white-space: nowrap;
            line-height: 1.4;
        }
        #toolbar button:hover { background: var(--accent); color: #fff; }

        #toolbar select {
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 2px 4px;
            border-radius: 3px;
            font-size: 11px;
            outline: none;
        }

        /* ── Graph canvas ────────────────────────────────────── */
        #cy {
            flex: 1;
            min-height: 0;
        }

        /* ── Status bar ──────────────────────────────────────── */
        #statusBar {
            display: flex;
            align-items: center;
            gap: 10px;
            padding: 3px 8px;
            background: var(--card);
            border-top: 1px solid var(--border);
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
        }

        /* ── Tooltip ─────────────────────────────────────────── */
        #tooltip {
            position: absolute;
            display: none;
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            padding: 8px 10px;
            font-size: 12px;
            color: var(--fg);
            pointer-events: none;
            z-index: 999;
            max-width: 280px;
            box-shadow: 0 4px 12px rgba(0,0,0,0.5);
        }
        #tooltip .tt-name { font-weight: 600; margin-bottom: 3px; }
        #tooltip .tt-tag {
            display: inline-block;
            background: var(--accent);
            color: #fff;
            padding: 1px 5px;
            border-radius: 3px;
            font-size: 9px;
            margin-right: 2px;
            margin-top: 2px;
        }
        #tooltip .tt-meta { color: var(--muted); font-size: 10px; margin-top: 3px; }

        /* ── Legend ───────────────────────────────────────────── */
        #legend {
            position: absolute;
            bottom: 32px;
            right: 8px;
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            padding: 6px 8px;
            font-size: 10px;
            opacity: 0.85;
        }
        #legend:hover { opacity: 1; }
        #legend .legend-item { display: flex; align-items: center; gap: 5px; margin: 1px 0; }
        #legend .legend-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
    </style>
</head>
<body>
    <div id="toolbar">
        <input type="text" id="searchInput" placeholder="Filter passages..." />
        <select id="layoutSelect" title="Layout">
            <option value="position">Saved</option>
            <option value="dagre">Flow</option>
            <option value="cose">Force</option>
        </select>
        <button id="fitBtn" title="Zoom to fit">Fit</button>
        <button id="refreshBtn" title="Refresh">&#x21BB;</button>
    </div>

    <div id="cy"></div>

    <div id="tooltip">
        <div class="tt-name"></div>
        <div class="tt-tags"></div>
        <div class="tt-meta"></div>
    </div>

    <div id="legend">
        <div class="legend-item"><span class="legend-dot" style="background:#4fc3f7"></span> Passage</div>
        <div class="legend-item"><span class="legend-dot" style="background:#66bb6a"></span> Start</div>
        <div class="legend-item"><span class="legend-dot" style="background:#ffb74d"></span> Special</div>
        <div class="legend-item"><span class="legend-dot" style="background:#ce93d8"></span> Metadata</div>
        <div class="legend-item"><span class="legend-dot" style="background:#555"></span> Unreachable</div>
        <div class="legend-item"><span class="legend-dot" style="background:transparent; border:2px dashed #f14c4c"></span> Broken link</div>
    </div>

    <div id="statusBar">
        <span id="statNodes">0 passages</span>
        <span id="statEdges">0 links</span>
        <span id="statBroken">0 broken</span>
    </div>

    <script nonce="${nonce}">
        const vscode = acquireVsCodeApi();
        let cy = null;
        let currentData = null;

        /* ── Twine-inspired color palette ──────────────────── */
        const COLORS = {
            normal:    '#4fc3f7',
            start:     '#66bb6a',
            special:   '#ffb74d',
            metadata:  '#ce93d8',
            unreachable:'#555555',
            broken:    '#f14c4c',
        };

        function getNodeColor(data) {
            if (data.is_metadata)   return COLORS.metadata;
            if (data.is_unreachable) return COLORS.unreachable;
            if (data.is_special)    return COLORS.special;
            if (data.is_start)      return COLORS.start;
            return COLORS.normal;
        }

        /* ── Grid background renderer ──────────────────────── */
        function drawGrid() {
            const container = document.getElementById('cy');
            if (!container) return;
            // Remove old grid canvas if present
            const old = document.getElementById('gridCanvas');
            if (old) old.remove();

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

            const rect = container.getBoundingClientRect();
            canvas.width = rect.width;
            canvas.height = rect.height;
            const ctx = canvas.getContext('2d');

            // Twine-style dot grid
            const spacing = 20;
            ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--grid-dot').trim() || 'rgba(255,255,255,0.07)';
            for (let x = spacing; x < rect.width; x += spacing) {
                for (let y = spacing; y < rect.height; y += spacing) {
                    ctx.beginPath();
                    ctx.arc(x, y, 1, 0, Math.PI * 2);
                    ctx.fill();
                }
            }
        }

        /* ── Initialize Cytoscape ──────────────────────────── */
        function initCytoscape() {
            cy = cytoscape({
                container: document.getElementById('cy'),
                style: [
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
                        }
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
                        }
                    },
                    {
                        selector: 'edge.is_broken',
                        style: {
                            'line-color': COLORS.broken,
                            'target-arrow-color': COLORS.broken,
                            'line-style': 'dashed',
                            'opacity': 0.85,
                        }
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
                        }
                    },
                    {
                        selector: 'node.highlighted',
                        style: { 'border-color': '#fff', 'border-width': '3px', 'z-index': 999 }
                    },
                    {
                        selector: 'node.dimmed',
                        style: { 'opacity': 0.2 }
                    },
                    {
                        selector: 'edge.dimmed',
                        style: { 'opacity': 0.08 }
                    },
                ],
                // Start with no layout — buildGraph will apply one
                layout: { name: 'null' },
            });

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
                    vscode.postMessage({
                        command: 'updatePositions',
                        updates: [{ passage_name: dragged.data('id'), position_x: newX, position_y: newY }],
                    });
                }
            });

            // Tooltip
            cy.on('mouseover', 'node', (evt) => {
                const d = evt.target.data();
                const tip = document.getElementById('tooltip');
                tip.querySelector('.tt-name').textContent = d.label;
                const tagsDiv = tip.querySelector('.tt-tags');
                tagsDiv.innerHTML = '';
                (d.tags || []).forEach(t => {
                    const s = document.createElement('span');
                    s.className = 'tt-tag'; s.textContent = t;
                    tagsDiv.appendChild(s);
                });
                const parts = [];
                if (d.in_degree > 0)  parts.push('In: ' + d.in_degree);
                if (d.out_degree > 0) parts.push('Out: ' + d.out_degree);
                if (d.is_special)    parts.push('Special');
                if (d.is_metadata)   parts.push('Metadata');
                if (d.is_unreachable) parts.push('Unreachable');
                if (d.posX != null && d.posY != null) parts.push('(' + Math.round(d.posX) + ', ' + Math.round(d.posY) + ')');
                tip.querySelector('.tt-meta').textContent = parts.join(' | ');
                tip.style.display = 'block';
            });
            cy.on('mouseout', 'node', () => {
                document.getElementById('tooltip').style.display = 'none';
            });
            cy.on('tapdrag', () => {
                document.getElementById('tooltip').style.display = 'none';
            });

            // Redraw grid on resize
            const ro = new ResizeObserver(() => drawGrid());
            ro.observe(document.getElementById('cy'));
        }

        /* ── Build the graph from server data ──────────────── */
        function buildGraph(data) {
            if (!cy) return;
            const nodes = Array.isArray(data?.nodes) ? data.nodes : [];
            const edges = Array.isArray(data?.edges) ? data.edges : [];
            currentData = { ...data, nodes, edges };

            cy.elements().remove();

            const elements = [];
            const positionedIds = new Set();
            const unpositioned = [];

            // Determine start passage name
            let startName = 'Start';
            // The start passage is the one with in_degree 0 and is not special/metadata
            // or just the one named "Start" — we mark it for the green color

            /* ── Nodes ────────────────────────────────────── */
            for (const n of nodes) {
                const isStart = (n.id === 'Start' || n.label === 'Start');
                const color = getNodeColor({ ...n, is_start: isStart });
                const size = Math.max(50, Math.min(100, 40 + Math.max(n.out_degree || 0, n.in_degree || 0) * 6));
                const hasPos = n.position_x != null && n.position_y != null;

                const el = {
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
                    }
                };

                if (hasPos) {
                    // Use saved position — Twine-compatible coordinates
                    el.position = { x: n.position_x, y: n.position_y };
                    positionedIds.add(n.id);
                } else {
                    unpositioned.push(n);
                }

                elements.push(el);
            }

            /* ── Edges ────────────────────────────────────── */
            for (const e of edges) {
                const el = {
                    data: {
                        id: e.source + '->' + e.target,
                        source: e.source,
                        target: e.target,
                        displayText: e.display_text || null,
                    },
                    classes: [
                        e.is_broken ? 'is_broken' : '',
                        e.display_text ? 'has_label' : '',
                    ].filter(Boolean).join(' '),
                };
                elements.push(el);
            }

            cy.add(elements);

            /* ── Layout ───────────────────────────────────── */
            // Strategy (Twine-like):
            //   1. If ANY nodes have saved positions, use "Saved" layout
            //      (positioned nodes at their coords, unpositioned get dagre'd)
            //   2. If NO nodes have positions, use dagre for everything
            const hasAnyPositions = positionedIds.size > 0;
            const layoutSelect = document.getElementById('layoutSelect');
            const chosenLayout = currentData.layout || (hasAnyPositions ? 'position' : 'dagre');

            // Disable "Saved" option when no positions exist
            const savedOpt = layoutSelect.querySelector('option[value="position"]');
            if (savedOpt) savedOpt.disabled = !hasAnyPositions;

            if (chosenLayout !== layoutSelect.value) layoutSelect.value = chosenLayout;
            applyLayout(chosenLayout);

            /* ── Stats ────────────────────────────────────── */
            const brokenCount = edges.filter(e => e.is_broken).length;
            document.getElementById('statNodes').textContent = nodes.length + ' passages';
            document.getElementById('statEdges').textContent = edges.length + ' links';
            document.getElementById('statBroken').textContent = brokenCount + ' broken';

            drawGrid();
        }

        /* ── Apply layout ──────────────────────────────────── */
        function applyLayout(name) {
            if (!cy || cy.nodes().length === 0) return;

            if (name === 'position') {
                // ── Saved layout: positioned nodes stay, unpositioned get auto-arranged
                // First place all positioned nodes at their saved coordinates
                cy.nodes().forEach(n => {
                    const px = n.data('posX');
                    const py = n.data('posY');
                    if (px != null && py != null) {
                        n.position({ x: px, y: py });
                    }
                });

                // For unpositioned nodes, run a sub-graph dagre layout
                // that respects existing positioned nodes as fixed anchors
                const unpos = cy.nodes().filter(n => n.data('posX') == null || n.data('posY') == null);
                if (unpos.length > 0 && unpos.length < cy.nodes().length) {
                    // Place unpositioned nodes relative to their connected positioned neighbors.
                    // We use a BFS from positioned nodes through edges to compute offset positions.
                    const fixed = cy.nodes().filter(n => n.data('posX') != null && n.data('posY') != null);
                    const bb = fixed.boundingBox();
                    const offsetX = bb.x2 + 150;
                    const offsetY = bb.y1;

                    // Run dagre only on unpositioned nodes + their internal edges
                    const subNodes = unpos;
                    const subEdgeIds = new Set();
                    subNodes.forEach(n => subEdgeIds.add(n.id()));

                    // Build a temporary sub-cytoscape for dagre layout
                    const subElements = [];
                    subNodes.forEach(n => {
                        subElements.push({ data: { id: n.id(), label: n.data('label') } });
                    });
                    // Edges between unpositioned nodes
                    cy.edges().forEach(e => {
                        if (subEdgeIds.has(e.data('source')) && subEdgeIds.has(e.data('target'))) {
                            subElements.push({ data: { id: e.id(), source: e.data('source'), target: e.data('target') } });
                        }
                    });

                    if (subElements.length > 1) {
                        const subCy = cytoscape({
                            container: undefined,  // headless
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
                        }).run();

                        // Map sub-graph positions back, offset by the bounding box
                        subCy.nodes().forEach(sn => {
                            const mainNode = cy.getElementById(sn.id());
                            if (mainNode.length > 0) {
                                mainNode.position({
                                    x: sn.position().x + offsetX,
                                    y: sn.position().y + offsetY,
                                });
                            }
                        });
                    } else {
                        // Just one unpositioned node — place it
                        unpos.forEach(n => n.position({ x: offsetX, y: offsetY }));
                    }
                } else if (unpos.length === cy.nodes().length) {
                    // ALL nodes unpositioned — fall through to dagre
                    applyLayout('dagre');
                    return;
                }

                cy.fit(undefined, 30);
                return;
            }

            // ── Algorithmic layouts ──────────────────────────
            let opts;
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
        }

        /* ── Filter / search ───────────────────────────────── */
        function filterGraph(query) {
            if (!cy || !currentData) return;
            const q = query.toLowerCase().trim();
            if (q === '') {
                cy.elements().removeClass('dimmed highlighted');
                return;
            }
            cy.nodes().addClass('dimmed');
            cy.edges().addClass('dimmed');
            const matched = cy.nodes().filter(n => {
                const label = (n.data('label') || '').toLowerCase();
                const tags = n.data('tags') || [];
                return label.includes(q) || tags.some(t => t.toLowerCase().includes(q));
            });
            matched.removeClass('dimmed').addClass('highlighted');
            matched.neighborhood('edge').removeClass('dimmed');
            matched.neighborhood('node').removeClass('dimmed');
        }

        /* ── Boot ──────────────────────────────────────────── */
        initCytoscape();

        document.getElementById('searchInput').addEventListener('input', e => filterGraph(e.target.value));
        document.getElementById('layoutSelect').addEventListener('change', e => {
            if (currentData) currentData.layout = e.target.value;
            applyLayout(e.target.value);
        });
        document.getElementById('fitBtn').addEventListener('click', () => { if (cy) cy.fit(undefined, 30); });
        document.getElementById('refreshBtn').addEventListener('click', () => {
            vscode.postMessage({ command: 'refreshGraph' });
        });

        window.addEventListener('message', event => {
            const msg = event.data;
            if (msg.command === 'updateGraph') buildGraph(msg.data);
        });
    </script>
</body>
</html>`;
    }
}
