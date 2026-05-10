//! Variable Flow View provider for the Knot extension.
//!
//! This module implements a VS Code sidebar webview panel that displays
//! variable flow / dataflow information for the workspace, including
//! where each variable is written and read, initialization status,
//! usage badges, and the hierarchical State.variables tree structure.
//!
//! For SugarCube, `$player.hp` maps to `State.variables.player.hp`.
//! Variables are displayed as a tree reflecting this hierarchy:
//!
//! ```text
//! $player  →  State.variables.player
//! ├── .name     →  State.variables.player.name
//! ├── .hp       →  State.variables.player.hp
//! └── .inventory → State.variables.player.inventory
//!     ├── .sword  → State.variables.player.inventory.sword
//!     └── .shield → State.variables.player.inventory.shield
//! ```

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotVariableFlowResponse, KnotVariableInfo, KnotVariableProperty, KnotVariableLocation } from './types';

// ---------------------------------------------------------------------------
// Variable Flow View webview provider
// ---------------------------------------------------------------------------

export class VariableFlowProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.variableFlowView';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _flowData: KnotVariableFlowResponse | null = null;
    private _filter: string = '';

    constructor(private readonly _extensionUri: vscode.Uri) {}

    /** Set the language client reference. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
        if (this._view) {
            this.refresh();
        }
    }

    /** Resolve the webview view. */
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

        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'refresh': {
                    await this.refresh();
                    break;
                }
                case 'filterVariable': {
                    const filter = message.filter ?? '';
                    this._filter = filter;
                    this._postFlowData();
                    break;
                }
                case 'openPassage': {
                    const { name } = message;
                    if (name) {
                        await vscode.commands.executeCommand('knot.openPassageByName', name);
                    }
                    break;
                }
            }
        });

        if (this._client) {
            this.refresh();
        }
    }

    /** Refresh variable flow data from the language server. */
    public async refresh() {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result = await this._client.sendRequest<KnotVariableFlowResponse>('knot/variableFlow', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

            this._flowData = result;
            this._postFlowData();
        } catch (e) {
            console.error('[Knot] Failed to fetch variable flow:', e);
        }
    }

    /** Post the current flow data (with filter applied) to the webview. */
    private _postFlowData() {
        if (this._view && this._flowData) {
            this._view.webview.postMessage({
                command: 'updateVariableFlow',
                data: this._flowData,
                filter: this._filter,
            });
        }
    }

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(webview: vscode.Webview): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline' https:; img-src 'self' data:; connect-src 'self';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Variable Flow</title>
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
            --warning: var(--vscode-editorWarning-foreground, #cca700);
            --success: #66bb6a;
            --prop-color: #ce9178;
            --state-path-color: #6a9955;
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 12px;
            padding: 8px;
        }

        .toolbar {
            display: flex;
            gap: 4px;
            margin-bottom: 8px;
            align-items: center;
        }

        .toolbar button {
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 10px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
            white-space: nowrap;
        }

        .toolbar button:hover {
            background: var(--accent);
            color: white;
        }

        .filter-input {
            flex: 1;
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 8px;
            border-radius: 3px;
            font-size: 11px;
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            outline: none;
        }

        .filter-input:focus {
            border-color: var(--accent);
        }

        .filter-input::placeholder {
            color: var(--muted);
        }

        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 20px 0;
        }

        .var-count {
            font-size: 10px;
            color: var(--muted);
            margin-bottom: 6px;
        }

        .var-item {
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            margin-bottom: 4px;
            overflow: hidden;
        }

        .var-header {
            display: flex;
            align-items: center;
            padding: 6px 8px;
            cursor: pointer;
            gap: 6px;
            flex-wrap: wrap;
        }

        .var-header:hover {
            background: rgba(255, 255, 255, 0.03);
        }

        .var-expand-icon {
            color: var(--muted);
            font-size: 10px;
            transition: transform 0.15s ease;
            flex-shrink: 0;
            width: 12px;
            text-align: center;
        }

        .var-item.expanded > .var-header .var-expand-icon {
            transform: rotate(90deg);
        }

        .var-name {
            font-family: monospace;
            font-size: 12px;
            font-weight: 600;
            color: var(--fg);
        }

        .var-name .dollar {
            color: var(--accent);
        }

        .state-path {
            font-family: monospace;
            font-size: 10px;
            color: var(--state-path-color);
            opacity: 0.85;
        }

        .badge {
            display: inline-block;
            padding: 1px 6px;
            border-radius: 3px;
            font-size: 9px;
            font-weight: 500;
            margin-left: 2px;
        }

        .badge-init {
            background: rgba(102, 187, 106, 0.15);
            color: var(--success);
        }

        .badge-unused {
            background: rgba(241, 76, 76, 0.15);
            color: var(--error);
        }

        .badge-temp {
            background: rgba(139, 139, 139, 0.2);
            color: var(--muted);
        }

        .badge-props {
            background: rgba(206, 145, 120, 0.15);
            color: var(--prop-color);
        }

        .var-counts {
            margin-left: auto;
            display: flex;
            gap: 8px;
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
        }

        .var-counts .count-write {
            color: var(--success);
        }

        .var-counts .count-read {
            color: var(--accent);
        }

        .var-details {
            display: none;
            padding: 4px 8px 8px 26px;
            border-top: 1px solid var(--border);
        }

        .var-item.expanded > .var-details {
            display: block;
        }

        .detail-group {
            margin-bottom: 6px;
        }

        .detail-group:last-child {
            margin-bottom: 0;
        }

        .detail-label {
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.3px;
            margin-bottom: 2px;
        }

        .passage-link {
            display: inline-block;
            color: var(--accent);
            cursor: pointer;
            font-size: 11px;
            padding: 1px 4px;
            border-radius: 2px;
        }

        .passage-link:hover {
            text-decoration: underline;
            background: rgba(0, 122, 204, 0.08);
        }

        .passage-list {
            list-style: none;
        }

        .passage-list li {
            padding: 1px 0;
        }

        .no-passages {
            font-size: 10px;
            color: var(--muted);
            font-style: italic;
        }

        /* Property tree styles */
        .prop-tree {
            margin-top: 6px;
        }

        .prop-item {
            margin-left: 12px;
            border-left: 1px solid var(--border);
            padding-left: 8px;
            margin-bottom: 2px;
        }

        .prop-header {
            display: flex;
            align-items: center;
            padding: 3px 4px;
            cursor: pointer;
            gap: 5px;
            border-radius: 2px;
        }

        .prop-header:hover {
            background: rgba(255, 255, 255, 0.03);
        }

        .prop-dot {
            color: var(--prop-color);
            font-family: monospace;
            font-weight: 600;
            font-size: 12px;
        }

        .prop-name {
            font-family: monospace;
            font-size: 11px;
            color: var(--prop-color);
            font-weight: 500;
        }

        .prop-state-path {
            font-family: monospace;
            font-size: 9px;
            color: var(--state-path-color);
            opacity: 0.7;
        }

        .prop-counts {
            margin-left: auto;
            display: flex;
            gap: 6px;
            font-size: 9px;
            color: var(--muted);
            flex-shrink: 0;
        }

        .prop-counts .count-write {
            color: var(--success);
        }

        .prop-counts .count-read {
            color: var(--accent);
        }

        .prop-expand-icon {
            color: var(--muted);
            font-size: 9px;
            transition: transform 0.15s ease;
            flex-shrink: 0;
            width: 10px;
            text-align: center;
        }

        .prop-item.expanded > .prop-header .prop-expand-icon {
            transform: rotate(90deg);
        }

        .prop-details {
            display: none;
            padding: 2px 4px 4px 20px;
        }

        .prop-item.expanded > .prop-details {
            display: block;
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Loading variable flow...</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        let currentFilter = '';

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateVariableFlow') {
                currentFilter = message.filter || '';
                renderVariableFlow(message.data, currentFilter);
            }
        });

        function esc(str) {
            const div = document.createElement('div');
            div.textContent = str;
            return div.innerHTML;
        }

        function toggleExpand(id) {
            const el = document.getElementById(id);
            if (el) {
                el.classList.toggle('expanded');
            }
        }

        function openPassage(name) {
            vscode.postMessage({ command: 'openPassage', name: name });
        }

        function onFilterChange(value) {
            vscode.postMessage({ command: 'filterVariable', filter: value });
        }

        let _propId = 0;

        function renderPropertyTree(properties, depth) {
            if (!properties || properties.length === 0) return '';
            let html = '<div class="prop-tree">';
            for (const prop of properties) {
                const propId = 'prop-' + (_propId++);
                const hasChildren = prop.properties && prop.properties.length > 0;
                const hasDetails = prop.written_in.length > 0 || prop.read_in.length > 0 || hasChildren;

                html += '<div class="prop-item' + (hasDetails ? '' : '') + '" id="' + propId + '">';

                // Property header
                html += '<div class="prop-header"' + (hasDetails ? ' onclick="toggleExpand(\\'' + propId + '\\')"' : '') + '>';

                if (hasDetails) {
                    html += '<span class="prop-expand-icon">&#x25B6;</span>';
                } else {
                    html += '<span style="width:10px;display:inline-block"></span>';
                }

                html += '<span class="prop-dot">.</span>';
                html += '<span class="prop-name">' + esc(prop.name) + '</span>';
                html += '<span class="prop-state-path">' + esc(prop.state_path) + '</span>';

                // Counts
                html += '<span class="prop-counts">';
                if (prop.written_in.length > 0) {
                    html += '<span class="count-write" title="Written">W:' + prop.written_in.length + '</span>';
                }
                if (prop.read_in.length > 0) {
                    html += '<span class="count-read" title="Read">R:' + prop.read_in.length + '</span>';
                }
                html += '</span>';

                html += '</div>';

                // Property details
                if (hasDetails) {
                    html += '<div class="prop-details">';

                    // Written in passages
                    if (prop.written_in.length > 0) {
                        html += '<div class="detail-group">';
                        html += '<div class="detail-label">Written in (' + prop.written_in.length + ')</div>';
                        html += '<ul class="passage-list">';
                        for (const loc of prop.written_in) {
                            html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\')">' + esc(loc.passage_name) + '</span></li>';
                        }
                        html += '</ul></div>';
                    }

                    // Read in passages
                    if (prop.read_in.length > 0) {
                        html += '<div class="detail-group">';
                        html += '<div class="detail-label">Read in (' + prop.read_in.length + ')</div>';
                        html += '<ul class="passage-list">';
                        for (const loc of prop.read_in) {
                            html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\')">' + esc(loc.passage_name) + '</span></li>';
                        }
                        html += '</ul></div>';
                    }

                    // Sub-properties (recursive)
                    if (hasChildren) {
                        html += renderPropertyTree(prop.properties, depth + 1);
                    }

                    html += '</div>';
                }

                html += '</div>'; // .prop-item
            }
            html += '</div>';
            return html;
        }

        function renderVariableFlow(data, filter) {
            const content = document.getElementById('content');
            let html = '';

            // Toolbar with filter and refresh
            html += '<div class="toolbar">';
            html += '<input class="filter-input" type="text" placeholder="Filter variables..." value="' + esc(filter) + '" oninput="onFilterChange(this.value)" />';
            html += '<button onclick="vscode.postMessage({command: \\'refresh\\'})">Refresh</button>';
            html += '</div>';

            let variables = data.variables || [];

            // Apply filter
            if (filter) {
                const lowerFilter = filter.toLowerCase();
                variables = variables.filter(v => {
                    if (v.name.toLowerCase().includes(lowerFilter)) return true;
                    if (v.state_path.toLowerCase().includes(lowerFilter)) return true;
                    // Also match against property names
                    function matchProps(props) {
                        for (const p of props) {
                            if (p.full_name.toLowerCase().includes(lowerFilter)) return true;
                            if (p.state_path.toLowerCase().includes(lowerFilter)) return true;
                            if (p.properties && matchProps(p.properties)) return true;
                        }
                        return false;
                    }
                    if (matchProps(v.properties || [])) return true;
                    return false;
                });
            }

            html += '<div class="var-count">' + variables.length + ' variable' + (variables.length !== 1 ? 's' : '') + (filter ? ' (filtered)' : '') + '</div>';

            if (variables.length === 0) {
                html += '<div class="empty-state">' + (filter ? 'No variables match filter' : 'No variables found') + '</div>';
                content.innerHTML = html;
                return;
            }

            for (let i = 0; i < variables.length; i++) {
                const v = variables[i];
                const varId = 'var-' + i;
                _propId = 0; // Reset for each variable

                html += '<div class="var-item" id="' + varId + '">';

                // Header row
                html += '<div class="var-header" onclick="toggleExpand(\\'' + varId + '\\')">';
                html += '<span class="var-expand-icon">&#x25B6;</span>';
                html += '<span class="var-name"><span class="dollar">$</span>' + esc(v.name.startsWith('$') ? v.name.substring(1) : v.name) + '</span>';

                // State.variables path
                html += '<span class="state-path">' + esc(v.state_path) + '</span>';

                // Badges
                if (v.initialized_at_start) {
                    html += '<span class="badge badge-init">Initialized at start</span>';
                }
                if (v.is_unused) {
                    html += '<span class="badge badge-unused">Unused</span>';
                }
                if (v.is_temporary) {
                    html += '<span class="badge badge-temp">Temporary</span>';
                }
                if (v.properties && v.properties.length > 0) {
                    html += '<span class="badge badge-props">' + v.properties.length + ' prop' + (v.properties.length !== 1 ? 's' : '') + '</span>';
                }

                // Counts
                html += '<span class="var-counts">';
                html += '<span class="count-write" title="Written in passages">W:' + v.written_in.length + '</span>';
                html += '<span class="count-read" title="Read in passages">R:' + v.read_in.length + '</span>';
                html += '</span>';

                html += '</div>';

                // Expandable details
                html += '<div class="var-details">';

                // Written in passages
                html += '<div class="detail-group">';
                html += '<div class="detail-label">Written in (' + v.written_in.length + ')</div>';
                if (v.written_in.length > 0) {
                    html += '<ul class="passage-list">';
                    for (const loc of v.written_in) {
                        html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\')">' + esc(loc.passage_name) + '</span></li>';
                    }
                    html += '</ul>';
                } else {
                    html += '<span class="no-passages">Never written</span>';
                }
                html += '</div>';

                // Read in passages
                html += '<div class="detail-group">';
                html += '<div class="detail-label">Read in (' + v.read_in.length + ')</div>';
                if (v.read_in.length > 0) {
                    html += '<ul class="passage-list">';
                    for (const loc of v.read_in) {
                        html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\')">' + esc(loc.passage_name) + '</span></li>';
                    }
                    html += '</ul>';
                } else {
                    html += '<span class="no-passages">Never read</span>';
                }
                html += '</div>';

                // Properties tree
                if (v.properties && v.properties.length > 0) {
                    html += '<div class="detail-group">';
                    html += '<div class="detail-label">Properties (State.variables tree)</div>';
                    html += renderPropertyTree(v.properties, 0);
                    html += '</div>';
                }

                html += '</div>'; // .var-details
                html += '</div>'; // .var-item
            }

            content.innerHTML = html;

            // Restore filter input focus and cursor position
            const filterInput = content.querySelector('.filter-input');
            if (filterInput && filter) {
                filterInput.focus();
                filterInput.setSelectionRange(filter.length, filter.length);
            }
        }
    </script>
</body>
</html>`;
    }
}
