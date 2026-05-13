//! Variable Flow View provider for the Knot extension.
//!
//! This module implements a VS Code sidebar webview panel that displays
//! variable flow / dataflow information for the workspace using a
//! **drill-down navigation** pattern instead of a crammed tree view.
//!
//! For SugarCube, `$player.hp` maps to `State.variables.player.hp`.
//! Variables are navigated via drill-down levels:
//!
//! **Level 0 — Variable List**: Shows all top-level state variables
//! (`$gold`, `$player`, `$hp`, etc.) as clickable rows.
//!
//! **Level 1 — Variable Detail**: Shows breadcrumb + variable details +
//! properties as clickable items that drill deeper.
//!
//! **Level 2+ — Property Detail**: Shows breadcrumb + property details +
//! sub-properties as clickable items.
//!
//! Navigation is managed entirely in the webview; the extension host
//! only sends data and handles `openPassage` requests.

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotVariableFlowResponse } from './types';

// ---------------------------------------------------------------------------
// Variable Flow View webview provider (drill-down design)
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
                    const { name, line } = message;
                    if (name) {
                        await vscode.commands.executeCommand('knot.openPassageByName', name, line ?? 0);
                    }
                    break;
                }
                // drillDown, drillUp, drillTo are handled entirely
                // in the webview JavaScript — no round-trip needed.
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

    /** Generate the HTML for the webview with drill-down navigation. */
    private _getHtmlForWebview(_webview: vscode.Webview): string {
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
            --hover: var(--vscode-list-hoverBackground, rgba(255,255,255,0.04));
            --error: var(--vscode-errorForeground, #f14c4c);
            --success: #66bb6a;
            --prop-color: #ce9178;
            --state-path-color: #6a9955;
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 12px;
            padding: 0;
        }

        /* ── Toolbar ──────────────────────────────────────────────── */

        .toolbar {
            display: flex;
            gap: 4px;
            padding: 6px 8px;
            align-items: center;
            border-bottom: 1px solid var(--border);
            position: sticky;
            top: 0;
            background: var(--bg);
            z-index: 10;
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
            flex-shrink: 0;
        }

        .toolbar button:hover {
            background: var(--accent);
            color: white;
        }

        .toolbar button:disabled {
            opacity: 0.4;
            cursor: default;
        }

        .toolbar button:disabled:hover {
            background: var(--card);
            color: var(--fg);
        }

        .filter-input {
            flex: 1;
            min-width: 0;
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

        /* ── Breadcrumb bar ───────────────────────────────────────── */

        .breadcrumb-bar {
            display: flex;
            align-items: center;
            padding: 5px 10px;
            background: var(--card);
            border-bottom: 1px solid var(--border);
            gap: 2px;
            flex-wrap: wrap;
            min-height: 26px;
        }

        .breadcrumb-bar.hidden {
            display: none;
        }

        .crumb {
            font-size: 11px;
            color: var(--accent);
            cursor: pointer;
            padding: 1px 4px;
            border-radius: 2px;
            white-space: nowrap;
        }

        .crumb:hover {
            background: var(--hover);
            text-decoration: underline;
        }

        .crumb.current {
            color: var(--fg);
            cursor: default;
            font-weight: 600;
        }

        .crumb.current:hover {
            background: transparent;
            text-decoration: none;
        }

        .crumb-sep {
            color: var(--muted);
            font-size: 10px;
            user-select: none;
            margin: 0 1px;
        }

        /* ── Content area ─────────────────────────────────────────── */

        .content-area {
            padding: 4px 0;
        }

        .section-label {
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.4px;
            padding: 6px 12px 3px;
        }

        .item-count {
            font-size: 10px;
            color: var(--muted);
            padding: 2px 12px 4px;
        }

        /* ── Clickable row (variable or property) ─────────────────── */

        .drill-row {
            display: flex;
            align-items: center;
            padding: 5px 12px;
            gap: 8px;
            cursor: pointer;
            border-bottom: 1px solid transparent;
            transition: background 0.1s ease;
        }

        .drill-row:hover {
            background: var(--hover);
        }

        .drill-row:active {
            background: rgba(255,255,255,0.07);
        }

        .drill-row .row-chevron {
            color: var(--muted);
            font-size: 9px;
            flex-shrink: 0;
            width: 10px;
            text-align: center;
        }

        .drill-row .row-name {
            font-family: monospace;
            font-size: 12px;
            font-weight: 600;
            color: var(--fg);
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
            min-width: 0;
        }

        .drill-row .row-name .dollar {
            color: var(--accent);
        }

        .drill-row .row-name .dot-prefix {
            color: var(--prop-color);
        }

        /* ── Badges ───────────────────────────────────────────────── */

        .badge {
            display: inline-block;
            padding: 1px 6px;
            border-radius: 3px;
            font-size: 9px;
            font-weight: 500;
            flex-shrink: 0;
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

        /* ── Counts (right-aligned in rows) ───────────────────────── */

        .row-counts {
            margin-left: auto;
            display: flex;
            gap: 8px;
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
        }

        .row-counts .cw {
            color: var(--success);
        }

        .row-counts .cr {
            color: var(--accent);
        }

        /* ── Detail view (variable detail / property detail) ──────── */

        .detail-header {
            padding: 8px 12px 4px;
            font-family: monospace;
            font-size: 14px;
            font-weight: 700;
            word-break: break-all;
        }

        .detail-header .dollar {
            color: var(--accent);
        }

        .detail-header .dot-prefix {
            color: var(--prop-color);
        }

        .detail-state-path {
            padding: 0 12px 6px;
            font-family: monospace;
            font-size: 10px;
            color: var(--state-path-color);
            opacity: 0.85;
            word-break: break-all;
        }

        .detail-badges {
            display: flex;
            gap: 4px;
            padding: 0 12px 6px;
            flex-wrap: wrap;
        }

        .detail-section {
            padding: 4px 12px;
        }

        .detail-section + .detail-section {
            border-top: 1px solid var(--border);
            margin-top: 2px;
            padding-top: 6px;
        }

        .detail-section-title {
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.3px;
            margin-bottom: 4px;
        }

        .passage-link {
            display: inline-block;
            color: var(--accent);
            cursor: pointer;
            font-size: 11px;
            padding: 2px 4px;
            border-radius: 2px;
        }

        .passage-link:hover {
            text-decoration: underline;
            background: rgba(0, 122, 204, 0.08);
        }

        .passage-link .line-num {
            color: var(--muted);
            font-size: 10px;
            margin-left: 2px;
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

        /* ── Properties section within a detail view ──────────────── */

        .props-section {
            padding: 4px 0;
        }

        .props-section .section-label {
            padding: 6px 12px 3px;
        }

        /* ── Empty state ──────────────────────────────────────────── */

        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 24px 12px;
        }

        .empty-state .empty-icon {
            font-size: 24px;
            margin-bottom: 6px;
            opacity: 0.5;
        }
    </style>
</head>
<body>
    <div id="root">
        <div class="empty-state">Loading variable flow&hellip;</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        // ── State ────────────────────────────────────────────────────
        let flowData = null;      // KnotVariableFlowResponse
        let currentFilter = '';
        /** Navigation stack. Each entry = { type, key, data }.
         *  type: 'root' | 'variable' | 'property'
         *  key:  unique identifier (variable name or property full_name)
         *  data: the KnotVariableInfo or KnotVariableProperty object
         */
        let navStack = [];        // previous levels
        let currentLevel = null;  // { type, key, data } — what we're viewing now

        // ── Helpers ──────────────────────────────────────────────────

        function esc(str) {
            const d = document.createElement('div');
            d.textContent = str;
            return d.innerHTML;
        }

        function openPassage(name, line) {
            vscode.postMessage({ command: 'openPassage', name: name, line: line ?? 0 });
        }

        function onFilterChange(value) {
            vscode.postMessage({ command: 'filterVariable', filter: value });
        }

        /** Find a top-level variable by name from flowData. */
        function findVariable(name) {
            if (!flowData) return null;
            return (flowData.variables || []).find(v => v.name === name) || null;
        }

        /** Find a property inside a variable or property by full_name (recursive). */
        function findProperty(container, fullName) {
            if (!container || !container.properties) return null;
            for (const p of container.properties) {
                if (p.full_name === fullName) return p;
                const found = findProperty(p, fullName);
                if (found) return found;
            }
            return null;
        }

        /** Check if a variable or property has any match in its subtree. */
        function matchesFilter(item, lowerFilter) {
            if (!lowerFilter) return true;
            // Check own names
            if ((item.name || '').toLowerCase().includes(lowerFilter)) return true;
            if ((item.full_name || '').toLowerCase().includes(lowerFilter)) return true;
            if ((item.state_path || '').toLowerCase().includes(lowerFilter)) return true;
            // Check children recursively
            if (item.properties) {
                for (const p of item.properties) {
                    if (matchesFilter(p, lowerFilter)) return true;
                }
            }
            return false;
        }

        /** Build the breadcrumb segments from navStack + currentLevel. */
        function getBreadcrumbSegments() {
            const segs = [];
            segs.push({ label: 'State.variables', level: -1 }); // root
            for (let i = 0; i < navStack.length; i++) {
                const entry = navStack[i];
                segs.push({ label: entry.type === 'variable' ? entry.key : '.' + entry.data.name, level: i });
            }
            if (currentLevel) {
                segs.push({ label: currentLevel.type === 'variable' ? currentLevel.key : '.' + currentLevel.data.name, level: navStack.length, current: true });
            }
            return segs;
        }

        // ── Navigation ───────────────────────────────────────────────

        /** Drill down into a variable (from root list). */
        function drillDownToVariable(varName) {
            const v = findVariable(varName);
            if (!v) return;
            // Push current root as "root" level
            navStack.push({ type: 'root', key: null, data: null });
            currentLevel = { type: 'variable', key: varName, data: v };
            render();
        }

        /** Drill down into a property from the current level. */
        function drillDownToProperty(fullName) {
            if (!currentLevel) return;
            const parent = currentLevel.data;
            const prop = findProperty(parent, fullName);
            if (!prop) return;
            navStack.push(currentLevel);
            currentLevel = { type: 'property', key: fullName, data: prop };
            render();
        }

        /** Go up one level. */
        function drillUp() {
            if (navStack.length === 0) return;
            const prev = navStack.pop();
            if (prev.type === 'root') {
                currentLevel = null;
            } else {
                currentLevel = prev;
            }
            render();
        }

        /** Jump to a specific breadcrumb level. levelIdx = -1 means root. */
        function drillTo(levelIdx) {
            if (levelIdx === -1) {
                navStack = [];
                currentLevel = null;
                render();
                return;
            }
            // levelIdx corresponds to the index in navStack
            if (levelIdx < 0 || levelIdx >= navStack.length) return;
            const target = navStack[levelIdx];
            navStack = navStack.slice(0, levelIdx);
            if (target.type === 'root') {
                currentLevel = null;
            } else {
                currentLevel = target;
            }
            render();
        }

        // ── Rendering ────────────────────────────────────────────────

        function render() {
            if (!currentLevel) {
                renderRootList();
            } else if (currentLevel.type === 'variable') {
                renderVariableDetail(currentLevel.data);
            } else if (currentLevel.type === 'property') {
                renderPropertyDetail(currentLevel.data);
            }
        }

        /** Level 0: Variable list. */
        function renderRootList() {
            const root = document.getElementById('root');
            let html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button disabled title="Back">&#x2190;</button>';
            html += '<input class="filter-input" type="text" placeholder="Filter variables..." value="' + esc(currentFilter) + '" oninput="onFilterChange(this.value)" />';
            html += '<button onclick="vscode.postMessage({command:\\'refresh\\'})" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            // Breadcrumb (hidden at root)
            html += '<div class="breadcrumb-bar hidden"></div>';

            // Content
            html += '<div class="content-area">';

            let variables = flowData ? (flowData.variables || []) : [];

            // Apply filter
            if (currentFilter) {
                const lf = currentFilter.toLowerCase();
                variables = variables.filter(v => matchesFilter(v, lf));
            }

            html += '<div class="item-count">' + variables.length + ' variable' + (variables.length !== 1 ? 's' : '') + (currentFilter ? ' (filtered)' : '') + '</div>';

            if (variables.length === 0) {
                html += '<div class="empty-state"><div class="empty-icon">&#x1F50D;</div>' + (currentFilter ? 'No variables match filter' : 'No variables found') + '</div>';
            } else {
                for (const v of variables) {
                    const displayName = v.name.startsWith('$') ? v.name.substring(1) : v.name;
                    html += '<div class="drill-row" onclick="drillDownToVariable(\\'' + esc(v.name).replace(/'/g, "\\'") + '\\')">';
                    html += '<span class="row-chevron">&#x25B6;</span>';
                    html += '<span class="row-name"><span class="dollar">$</span>' + esc(displayName) + '</span>';

                    // Badges (compact — only show most important ones)
                    if (v.initialized_at_start) {
                        html += '<span class="badge badge-init">init</span>';
                    }
                    if (v.is_unused) {
                        html += '<span class="badge badge-unused">unused</span>';
                    }
                    if (v.is_temporary) {
                        html += '<span class="badge badge-temp">temp</span>';
                    }
                    if (v.properties && v.properties.length > 0) {
                        html += '<span class="badge badge-props">' + v.properties.length + ' prop' + (v.properties.length !== 1 ? 's' : '') + '</span>';
                    }

                    // Counts
                    html += '<span class="row-counts">';
                    html += '<span class="cw" title="Written">W:' + v.written_in.length + '</span>';
                    html += '<span class="cr" title="Read">R:' + v.read_in.length + '</span>';
                    html += '</span>';

                    html += '</div>';
                }
            }

            html += '</div>'; // .content-area
            root.innerHTML = html;
            restoreFilterFocus();
        }

        /** Level 1: Variable detail. */
        function renderVariableDetail(v) {
            const root = document.getElementById('root');
            let html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button onclick="drillUp()" title="Back">&#x2190;</button>';
            html += '<input class="filter-input" type="text" placeholder="Filter..." value="' + esc(currentFilter) + '" oninput="onFilterChange(this.value)" />';
            html += '<button onclick="vscode.postMessage({command:\\'refresh\\'})" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            // Breadcrumb
            html += renderBreadcrumb();

            // Content
            html += '<div class="content-area">';

            const displayName = v.name.startsWith('$') ? v.name.substring(1) : v.name;

            // Header
            html += '<div class="detail-header"><span class="dollar">$</span>' + esc(displayName) + '</div>';
            html += '<div class="detail-state-path">' + esc(v.state_path) + '</div>';

            // Badges
            html += '<div class="detail-badges">';
            if (v.initialized_at_start) {
                html += '<span class="badge badge-init">Initialized at start</span>';
            }
            if (v.is_unused) {
                html += '<span class="badge badge-unused">Unused</span>';
            }
            if (v.is_temporary) {
                html += '<span class="badge badge-temp">Temporary</span>';
            }
            html += '</div>';

            // Written in
            html += '<div class="detail-section">';
            html += '<div class="detail-section-title">Written in (' + v.written_in.length + ')</div>';
            if (v.written_in.length > 0) {
                html += '<ul class="passage-list">';
                for (const loc of v.written_in) {
                    html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\', ' + loc.line + ')">' + esc(loc.passage_name) + (loc.line > 0 ? '<span class="line-num">:' + loc.line + '</span>' : '') + '</span></li>';
                }
                html += '</ul>';
            } else {
                html += '<span class="no-passages">Never written</span>';
            }
            html += '</div>';

            // Read in
            html += '<div class="detail-section">';
            html += '<div class="detail-section-title">Read in (' + v.read_in.length + ')</div>';
            if (v.read_in.length > 0) {
                html += '<ul class="passage-list">';
                for (const loc of v.read_in) {
                    html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\', ' + loc.line + ')">' + esc(loc.passage_name) + (loc.line > 0 ? '<span class="line-num">:' + loc.line + '</span>' : '') + '</span></li>';
                }
                html += '</ul>';
            } else {
                html += '<span class="no-passages">Never read</span>';
            }
            html += '</div>';

            // Properties
            if (v.properties && v.properties.length > 0) {
                html += renderPropertyList(v.properties);
            }

            html += '</div>'; // .content-area
            root.innerHTML = html;
            restoreFilterFocus();
        }

        /** Level 2+: Property detail. */
        function renderPropertyDetail(p) {
            const root = document.getElementById('root');
            let html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button onclick="drillUp()" title="Back">&#x2190;</button>';
            html += '<input class="filter-input" type="text" placeholder="Filter..." value="' + esc(currentFilter) + '" oninput="onFilterChange(this.value)" />';
            html += '<button onclick="vscode.postMessage({command:\\'refresh\\'})" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            // Breadcrumb
            html += renderBreadcrumb();

            // Content
            html += '<div class="content-area">';

            // Header — show full_name with dot prefix styled
            const nameParts = p.full_name.split('.');
            const lastPart = nameParts[nameParts.length - 1];
            html += '<div class="detail-header"><span class="dot-prefix">.</span>' + esc(lastPart) + '</div>';
            html += '<div class="detail-state-path">' + esc(p.state_path) + '</div>';

            // Written in
            html += '<div class="detail-section">';
            html += '<div class="detail-section-title">Written in (' + p.written_in.length + ')</div>';
            if (p.written_in.length > 0) {
                html += '<ul class="passage-list">';
                for (const loc of p.written_in) {
                    html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\', ' + loc.line + ')">' + esc(loc.passage_name) + (loc.line > 0 ? '<span class="line-num">:' + loc.line + '</span>' : '') + '</span></li>';
                }
                html += '</ul>';
            } else {
                html += '<span class="no-passages">Never written</span>';
            }
            html += '</div>';

            // Read in
            html += '<div class="detail-section">';
            html += '<div class="detail-section-title">Read in (' + p.read_in.length + ')</div>';
            if (p.read_in.length > 0) {
                html += '<ul class="passage-list">';
                for (const loc of p.read_in) {
                    html += '<li><span class="passage-link" onclick="openPassage(\\'' + esc(loc.passage_name).replace(/'/g, "\\'") + '\\', ' + loc.line + ')">' + esc(loc.passage_name) + (loc.line > 0 ? '<span class="line-num">:' + loc.line + '</span>' : '') + '</span></li>';
                }
                html += '</ul>';
            } else {
                html += '<span class="no-passages">Never read</span>';
            }
            html += '</div>';

            // Sub-properties
            if (p.properties && p.properties.length > 0) {
                html += renderPropertyList(p.properties);
            }

            html += '</div>'; // .content-area
            root.innerHTML = html;
            restoreFilterFocus();
        }

        /** Render a list of property rows (clickable to drill down). */
        function renderPropertyList(properties) {
            let html = '<div class="props-section">';

            // Apply filter
            let filtered = properties;
            if (currentFilter) {
                const lf = currentFilter.toLowerCase();
                filtered = properties.filter(p => matchesFilter(p, lf));
            }

            html += '<div class="section-label">Properties (' + filtered.length + ')</div>';

            for (const p of filtered) {
                const propDisplayName = '.' + p.name;
                html += '<div class="drill-row" onclick="drillDownToProperty(\\'' + esc(p.full_name).replace(/'/g, "\\'") + '\\')">';
                html += '<span class="row-chevron">&#x25B6;</span>';
                html += '<span class="row-name"><span class="dot-prefix">.</span>' + esc(p.name) + '</span>';

                // Show sub-property count badge if any
                if (p.properties && p.properties.length > 0) {
                    html += '<span class="badge badge-props">' + p.properties.length + ' sub</span>';
                }

                // Counts
                html += '<span class="row-counts">';
                html += '<span class="cw" title="Written">W:' + p.written_in.length + '</span>';
                html += '<span class="cr" title="Read">R:' + p.read_in.length + '</span>';
                html += '</span>';

                html += '</div>';
            }

            html += '</div>';
            return html;
        }

        /** Render the breadcrumb bar. */
        function renderBreadcrumb() {
            const segs = getBreadcrumbSegments();
            if (segs.length <= 1) {
                return '<div class="breadcrumb-bar hidden"></div>';
            }

            let html = '<div class="breadcrumb-bar">';
            for (let i = 0; i < segs.length; i++) {
                const s = segs[i];
                if (i > 0) {
                    html += '<span class="crumb-sep">&#x276F;</span>';
                }
                if (s.current) {
                    html += '<span class="crumb current">' + esc(s.label) + '</span>';
                } else {
                    html += '<span class="crumb" onclick="drillTo(' + s.level + ')">' + esc(s.label) + '</span>';
                }
            }
            html += '</div>';
            return html;
        }

        /** Restore filter input focus after render. */
        function restoreFilterFocus() {
            const inp = document.querySelector('.filter-input');
            if (inp && currentFilter) {
                inp.focus();
                inp.setSelectionRange(currentFilter.length, currentFilter.length);
            }
        }

        // ── Message handling ─────────────────────────────────────────

        /** When new data arrives from the extension host, refresh the
         *  navigation state references so drilled-down views point to the
         *  latest data objects. */
        function refreshNavState() {
            if (!flowData) {
                navStack = [];
                currentLevel = null;
                return;
            }
            // Rebuild navStack references
            const newStack = [];
            for (const entry of navStack) {
                if (entry.type === 'root') {
                    newStack.push({ type: 'root', key: null, data: null });
                } else if (entry.type === 'variable') {
                    const v = findVariable(entry.key);
                    if (v) {
                        newStack.push({ type: 'variable', key: entry.key, data: v });
                    }
                } else if (entry.type === 'property') {
                    // Need to find the property — it belongs to the parent in the previous stack entry
                    const parentEntry = newStack.length > 0 ? newStack[newStack.length - 1] : null;
                    const parent = parentEntry ? parentEntry.data : null;
                    if (parent) {
                        const p = findProperty(parent, entry.key);
                        if (p) {
                            newStack.push({ type: 'property', key: entry.key, data: p });
                        }
                    }
                }
            }
            navStack = newStack;

            // Rebuild currentLevel
            if (currentLevel) {
                if (currentLevel.type === 'variable') {
                    const v = findVariable(currentLevel.key);
                    currentLevel = v ? { type: 'variable', key: currentLevel.key, data: v } : null;
                } else if (currentLevel.type === 'property') {
                    const parentEntry = navStack.length > 0 ? navStack[navStack.length - 1] : null;
                    const parent = parentEntry ? parentEntry.data : null;
                    if (parent) {
                        const p = findProperty(parent, currentLevel.key);
                        currentLevel = p ? { type: 'property', key: currentLevel.key, data: p } : null;
                    } else {
                        currentLevel = null;
                    }
                }
            }

            // If currentLevel became null and stack has entries, pop back
            if (!currentLevel && navStack.length > 0) {
                const prev = navStack.pop();
                if (prev.type === 'root') {
                    currentLevel = null;
                } else {
                    currentLevel = prev;
                }
            }
        }

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateVariableFlow') {
                flowData = message.data;
                currentFilter = message.filter || '';
                refreshNavState();
                render();
            }
        });
    </script>
</body>
</html>`;
    }
}
