//! Story Map webview provider for the Knot extension.
//!
//! This module implements a VS Code webview panel that renders an interactive
//! passage graph using Cytoscape.js. Features include:
//!
//! - Force-directed and dagre (hierarchical) layout
//! - Click-to-navigate (clicking a node opens the passage in the editor)
//! - Color-coded nodes (normal, special, metadata, unreachable, broken)
//! - Red dashed edges for broken links
//! - Search/filter passages by name or tag
//! - Real-time graph refresh when documents change
//! - Zoom-to-fit and layout switching controls

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotGraphResponse } from './types';

// ---------------------------------------------------------------------------
// Story Map webview provider
// ---------------------------------------------------------------------------

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

        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

        // Handle messages from the webview
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'openPassage': {
                    const { file, line } = message;
                    if (file) {
                        const uri = vscode.Uri.parse(file);
                        const doc = await vscode.workspace.openTextDocument(uri);
                        const editor = await vscode.window.showTextDocument(doc, {
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
                case 'switchLayout': {
                    if (this._graphData) {
                        this._graphData.layout = message.layout;
                        this._postGraphData();
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

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(webview: vscode.Webview): string {
        // Cytoscape.js scripts are loaded from the extension's media/ directory.
        // If the files are not bundled, the webview will fail to load them.
        // For development, you can download them from:
        //   https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.28.1/cytoscape.min.js
        //   https://cdnjs.cloudflare.com/ajax/libs/cytoscape-dagre/2.5.0/cytoscape-dagre.min.js
        // And place them in extensions/vscode/media/
        const cytoscapeLocal = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'cytoscape.min.js'));
        const dagreLocal = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'cytoscape-dagre.min.js'));
        const cytoscapeScript = cytoscapeLocal.toString();
        const dagreScript = dagreLocal.toString();

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline' https:; img-src 'self' data:; connect-src 'self';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Story Map</title>
    <script src="${cytoscapeScript}"></script>
    <script src="${dagreScript}"></script>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        :root {
            --bg: var(--vscode-editor-background, #1e1e1e);
            --fg: var(--vscode-editor-foreground, #d4d4d4);
            --accent: var(--vscode-focusBorder, #007acc);
            --border: var(--vscode-panel-border, #474747);
            --muted: var(--vscode-descriptionForeground, #8b8b8b);
            --card: var(--vscode-sideBar-background, #252526);
            --error: var(--vscode-errorForeground, #f14c4c);
            --warning: var(--vscode-editorWarning-foreground, #cca700);
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

        /* Toolbar */
        #toolbar {
            display: flex;
            align-items: center;
            gap: 6px;
            padding: 6px 8px;
            background: var(--card);
            border-bottom: 1px solid var(--border);
            flex-shrink: 0;
        }

        #toolbar input {
            flex: 1;
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 4px 8px;
            border-radius: 3px;
            font-size: 12px;
            outline: none;
        }

        #toolbar input:focus {
            border-color: var(--accent);
        }

        #toolbar button {
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 12px;
            white-space: nowrap;
        }

        #toolbar button:hover {
            background: var(--accent);
            color: white;
        }

        #toolbar select {
            background: var(--bg);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 6px;
            border-radius: 3px;
            font-size: 12px;
            outline: none;
        }

        /* Graph container */
        #cy {
            flex: 1;
            min-height: 0;
        }

        /* Status bar at bottom */
        #statusBar {
            display: flex;
            align-items: center;
            gap: 12px;
            padding: 4px 8px;
            background: var(--card);
            border-top: 1px solid var(--border);
            font-size: 11px;
            color: var(--muted);
            flex-shrink: 0;
        }

        #statusBar .stat {
            display: flex;
            align-items: center;
            gap: 3px;
        }

        /* Tooltip */
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
            box-shadow: 0 4px 12px rgba(0,0,0,0.4);
        }

        #tooltip .tt-name {
            font-weight: 600;
            margin-bottom: 4px;
        }

        #tooltip .tt-tag {
            display: inline-block;
            background: var(--accent);
            color: white;
            padding: 1px 5px;
            border-radius: 3px;
            font-size: 10px;
            margin-right: 3px;
            margin-top: 2px;
        }

        #tooltip .tt-meta {
            color: var(--muted);
            font-size: 11px;
            margin-top: 4px;
        }

        /* Legend */
        #legend {
            position: absolute;
            bottom: 36px;
            right: 8px;
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            padding: 8px;
            font-size: 11px;
        }

        #legend .legend-item {
            display: flex;
            align-items: center;
            gap: 6px;
            margin: 2px 0;
        }

        #legend .legend-dot {
            width: 10px;
            height: 10px;
            border-radius: 50%;
            flex-shrink: 0;
        }
    </style>
</head>
<body>
    <div id="toolbar">
        <input type="text" id="searchInput" placeholder="Search passages..." />
        <select id="layoutSelect" title="Layout algorithm">
            <option value="dagre">Dagre</option>
            <option value="breadthfirst">Tree</option>
            <option value="cose">Force</option>
            <option value="circle">Circle</option>
        </select>
        <button id="fitBtn" title="Zoom to fit">&#x26F6; Fit</button>
        <button id="refreshBtn" title="Refresh graph">&#x21BB;</button>
    </div>

    <div id="cy"></div>

    <div id="tooltip">
        <div class="tt-name"></div>
        <div class="tt-tags"></div>
        <div class="tt-meta"></div>
    </div>

    <div id="legend">
        <div class="legend-item"><span class="legend-dot" style="background:#4fc3f7"></span> Passage</div>
        <div class="legend-item"><span class="legend-dot" style="background:#ffb74d"></span> Special</div>
        <div class="legend-item"><span class="legend-dot" style="background:#ce93d8"></span> Metadata</div>
        <div class="legend-item"><span class="legend-dot" style="background:#666"></span> Unreachable</div>
        <div class="legend-item"><span class="legend-dot" style="background:transparent; border:2px dashed #f14c4c"></span> Broken link</div>
    </div>

    <div id="statusBar">
        <span class="stat" id="statNodes">Nodes: 0</span>
        <span class="stat" id="statEdges">Edges: 0</span>
        <span class="stat" id="statBroken">Broken: 0</span>
        <span class="stat" id="statUnreachable">Unreachable: 0</span>
    </div>

    <script>
        const vscode = acquireVsCodeApi();
        let cy = null;
        let currentData = null;

        // Color palette for different node types
        const COLORS = {
            normal: '#4fc3f7',       // Light blue
            special: '#ffb74d',      // Orange
            metadata: '#ce93d8',     // Purple
            unreachable: '#666666',  // Gray
            broken: '#f14c4c',       // Red
            start: '#66bb6a',        // Green
        };

        function getNodeColor(node) {
            if (node.is_metadata) return COLORS.metadata;
            if (node.is_unreachable) return COLORS.unreachable;
            if (node.is_special) return COLORS.special;
            // Check if it's the start passage
            if (node.id === 'Start' || node.label === 'Start') return COLORS.start;
            return COLORS.normal;
        }

        function initCytoscape() {
            cy = cytoscape({
                container: document.getElementById('cy'),
                style: [
                    {
                        selector: 'node',
                        style: {
                            'label': 'data(label)',
                            'background-color': 'data(color)',
                            'color': '#d4d4d4',
                            'text-valign': 'center',
                            'text-halign': 'center',
                            'font-size': '10px',
                            'text-wrap': 'ellipsis',
                            'text-max-width': '80px',
                            'width': 'data(width)',
                            'height': 'data(height)',
                            'shape': 'round-rectangle',
                            'text-outline-color': '#1e1e1e',
                            'text-outline-width': '2px',
                            'border-width': '2px',
                            'border-color': 'data(borderColor)',
                            'font-family': '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
                        }
                    },
                    {
                        selector: 'node:active',
                        style: {
                            'overlay-opacity': 0.1,
                        }
                    },
                    {
                        selector: 'node[[is_metadata = true]]',
                        style: {
                            'shape': 'diamond',
                            'width': 30,
                            'height': 30,
                            'font-size': '8px',
                        }
                    },
                    {
                        selector: 'edge',
                        style: {
                            'width': 1.5,
                            'line-color': '#555',
                            'target-arrow-color': '#555',
                            'target-arrow-shape': 'triangle',
                            'arrow-scale': 0.8,
                            'curve-style': 'bezier',
                            'opacity': 0.7,
                        }
                    },
                    {
                        selector: 'edge[[is_broken = true]]',
                        style: {
                            'line-color': COLORS.broken,
                            'target-arrow-color': COLORS.broken,
                            'line-style': 'dashed',
                            'opacity': 0.9,
                        }
                    },
                    {
                        selector: 'node.highlighted',
                        style: {
                            'border-color': '#ffffff',
                            'border-width': '3px',
                            'z-index': 999,
                        }
                    },
                    {
                        selector: 'node.dimmed',
                        style: {
                            'opacity': 0.25,
                        }
                    },
                    {
                        selector: 'edge.dimmed',
                        style: {
                            'opacity': 0.1,
                        }
                    },
                ],
                layout: { name: 'dagre', spacingFactor: 1.2 },
            });

            // Click handler — navigate to passage
            cy.on('tap', 'node', (evt) => {
                const node = evt.target;
                const file = node.data('file');
                const line = node.data('line') || 0;
                if (file) {
                    vscode.postMessage({
                        command: 'openPassage',
                        file: file,
                        line: line,
                    });
                }
            });

            // Hover handler — show tooltip
            cy.on('mouseover', 'node', (evt) => {
                const node = evt.target;
                const data = node.data();
                const tooltip = document.getElementById('tooltip');
                tooltip.querySelector('.tt-name').textContent = data.label;
                const tagsDiv = tooltip.querySelector('.tt-tags');
                tagsDiv.innerHTML = '';
                if (data.tags && data.tags.length > 0) {
                    data.tags.forEach(t => {
                        const span = document.createElement('span');
                        span.className = 'tt-tag';
                        span.textContent = t;
                        tagsDiv.appendChild(span);
                    });
                }
                const meta = tooltip.querySelector('.tt-meta');
                const parts = [];
                if (data.in_degree > 0) parts.push('In: ' + data.in_degree);
                if (data.out_degree > 0) parts.push('Out: ' + data.out_degree);
                if (data.is_special) parts.push('Special');
                if (data.is_metadata) parts.push('Metadata');
                if (data.is_unreachable) parts.push('Unreachable');
                meta.textContent = parts.join(' | ');

                tooltip.style.display = 'block';
            });

            cy.on('mouseout', 'node', () => {
                document.getElementById('tooltip').style.display = 'none';
            });

            cy.on('tapdrag', () => {
                document.getElementById('tooltip').style.display = 'none';
            });
        }

        function buildGraph(data) {
            if (!cy) return;
            currentData = data;

            cy.elements().remove();

            const elements = [];

            // Add nodes
            for (const node of data.nodes) {
                const color = getNodeColor(node);
                const size = Math.max(40, Math.min(80, 30 + Math.max(node.out_degree, node.in_degree) * 5));
                elements.push({
                    data: {
                        id: node.id,
                        label: node.label,
                        file: node.file,
                        line: node.line,
                        tags: node.tags,
                        out_degree: node.out_degree,
                        in_degree: node.in_degree,
                        is_special: node.is_special,
                        is_metadata: node.is_metadata,
                        is_unreachable: node.is_unreachable,
                        color: color,
                        borderColor: node.is_unreachable ? '#444' : color,
                        width: node.is_metadata ? 30 : size,
                        height: node.is_metadata ? 30 : size * 0.6,
                    }
                });
            }

            // Add edges
            for (const edge of data.edges) {
                elements.push({
                    data: {
                        id: edge.source + '->' + edge.target,
                        source: edge.source,
                        target: edge.target,
                        is_broken: edge.is_broken,
                    }
                });
            }

            cy.add(elements);

            // Apply layout
            applyLayout(data.layout || 'dagre');

            // Update stats
            const brokenCount = data.edges.filter(e => e.is_broken).length;
            const unreachableCount = data.nodes.filter(n => n.is_unreachable).length;
            document.getElementById('statNodes').textContent = 'Nodes: ' + data.nodes.length;
            document.getElementById('statEdges').textContent = 'Edges: ' + data.edges.length;
            document.getElementById('statBroken').textContent = 'Broken: ' + brokenCount;
            document.getElementById('statUnreachable').textContent = 'Unreachable: ' + unreachableCount;
        }

        function applyLayout(layoutName) {
            if (!cy) return;

            let layoutOpts;
            switch (layoutName) {
                case 'dagre':
                    layoutOpts = {
                        name: 'dagre',
                        rankDir: 'TB',
                        spacingFactor: 1.2,
                        nodeSep: 40,
                        rankSep: 60,
                        animate: true,
                        animationDuration: 300,
                    };
                    break;
                case 'breadthfirst':
                    layoutOpts = {
                        name: 'breadthfirst',
                        directed: true,
                        spacingFactor: 1.5,
                        animate: true,
                        animationDuration: 300,
                    };
                    break;
                case 'cose':
                    layoutOpts = {
                        name: 'cose',
                        animate: true,
                        animationDuration: 500,
                        nodeRepulsion: 8000,
                        idealEdgeLength: 100,
                        gravity: 0.3,
                    };
                    break;
                case 'circle':
                    layoutOpts = {
                        name: 'circle',
                        animate: true,
                        animationDuration: 300,
                    };
                    break;
                default:
                    layoutOpts = { name: 'dagre', spacingFactor: 1.2, animate: true };
            }

            cy.layout(layoutOpts).run();
        }

        function filterGraph(query) {
            if (!cy || !currentData) return;
            const q = query.toLowerCase().trim();

            if (q === '') {
                cy.elements().removeClass('dimmed');
                cy.elements().removeClass('highlighted');
                return;
            }

            cy.nodes().addClass('dimmed');
            cy.edges().addClass('dimmed');

            const matched = cy.nodes().filter((node) => {
                const label = (node.data('label') || '').toLowerCase();
                const tags = (node.data('tags') || []);
                return label.includes(q) || tags.some(t => t.toLowerCase().includes(q));
            });

            matched.removeClass('dimmed');
            matched.addClass('highlighted');
            matched.neighborhood('edge').removeClass('dimmed');
            matched.neighborhood('node').removeClass('dimmed');
        }

        // Initialize
        initCytoscape();

        // Toolbar event handlers
        document.getElementById('searchInput').addEventListener('input', (e) => {
            filterGraph(e.target.value);
        });

        document.getElementById('layoutSelect').addEventListener('change', (e) => {
            const layout = e.target.value;
            if (currentData) {
                currentData.layout = layout;
            }
            applyLayout(layout);
        });

        document.getElementById('fitBtn').addEventListener('click', () => {
            if (cy) {
                cy.fit(undefined, 20);
            }
        });

        document.getElementById('refreshBtn').addEventListener('click', () => {
            vscode.postMessage({ command: 'refreshGraph' });
        });

        // Listen for messages from the extension
        window.addEventListener('message', (event) => {
            const message = event.data;
            switch (message.command) {
                case 'updateGraph':
                    buildGraph(message.data);
                    break;
            }
        });
    </script>
</body>
</html>`;
    }
}
