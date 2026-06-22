//! Variable Tracking View provider for the Knot extension.
//!
//! Three-zone sidebar webview panel:
//!
//! **Zone 1 — Navigation**: Breadcrumb path, home/back buttons, summary.
//! **Zone 2 — Passage List**: All passages referencing the selected variable,
//!   sorted by BFS reachability from StoryInit.
//! **Zone 3 — Detail**: Individual references for the selected passage.
//!
//! Format-agnostic: no `$` prefix, no `State.variables`, no format-specific
//! syntax. Variable names are extracted identifiers only.

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotVariableFlowParams, KnotVariableFlowResponse, KnotVariableInfo, KnotVariableProperty, KnotVariableLocation } from './types';

// ---------------------------------------------------------------------------
// Server → Webview data transformation
// ---------------------------------------------------------------------------

/**
 * Transform the server's flat variable response into the nested format
 * the webview expects for its three-zone rendering.
 *
 * Server provides: KnotVariableInfo with flat written_in/read_in arrays.
 * Webview expects: nested passages array, computed flags, ref counts, etc.
 */
function transformVariableFlow(data: KnotVariableFlowResponse): Record<string, unknown> {
    const variables = (data.variables || []).map(transformVariable);
    return { variables };
}

function transformVariable(v: KnotVariableInfo): Record<string, unknown> {
    // Merge written_in and read_in into a passages map
    const passageMap = new Map<string, {
        passage_name: string;
        writes: KnotVariableLocation[];
        reads: KnotVariableLocation[];
    }>();

    for (const loc of v.written_in || []) {
        let entry = passageMap.get(loc.passage_name);
        if (!entry) {
            entry = { passage_name: loc.passage_name, writes: [], reads: [] };
            passageMap.set(loc.passage_name, entry);
        }
        entry.writes.push(loc);
    }
    for (const loc of v.read_in || []) {
        let entry = passageMap.get(loc.passage_name);
        if (!entry) {
            entry = { passage_name: loc.passage_name, writes: [], reads: [] };
            passageMap.set(loc.passage_name, entry);
        }
        entry.reads.push(loc);
    }

    // Build passages array with references
    const passages = Array.from(passageMap.values()).map(p => {
        const references = [
            ...p.writes.map(w => ({
                is_write: true,
                line: w.line,
                span: w.span,
                is_struct_def: false,
                is_reassign: false,
                type_conflict: false,
            })),
            ...p.reads.map(r => ({
                is_write: false,
                line: r.line,
                span: r.span,
                is_struct_def: false,
                is_reassign: false,
                type_conflict: false,
            })),
        ];
        return {
            passage_name: p.passage_name,
            reachable: true,
            total_refs: references.length,
            in_loop: false,
            references,
        };
    });

    // Compute flags
    const flags: Array<{ flag_type: string; message: string }> = [];
    if (v.is_unused) {
        flags.push({ flag_type: 'unused', message: 'This variable is written but never read' });
    }
    if (!v.initialized_at_start && (v.written_in || []).length > 0) {
        flags.push({ flag_type: 'single-use', message: 'Variable may not be initialized before first use' });
    }

    // Count total refs
    const totalWrites = (v.written_in || []).length;
    const totalReads = (v.read_in || []).length;

    // For Array-kind root variables, the children come from element_shape.[*]
    // For Object/Unknown root variables, children come from properties directly
    let children: Record<string, unknown>[];
    let elementShape: Record<string, unknown> | null = null;

    if (v.kind === 'array' && v.element_shape) {
        // Array root variable: element_shape contains the [*] virtual node
        elementShape = transformProperty(v.element_shape);
        children = (v.element_shape.properties || []).map(transformProperty);
    } else {
        children = (v.properties || []).map(transformProperty);
    }

    return {
        name: v.name.startsWith('$') ? v.name.slice(1) : v.name,
        full_name: v.name,
        state_path: v.state_path,
        is_temporary: v.is_temporary,
        passage_count: passages.length,
        ref_count: totalWrites + totalReads,
        initialized_at_start: v.initialized_at_start,
        is_unused: v.is_unused,
        flags,
        passages,
        children,
        elementShape,
        kind: v.kind || 'unknown',
    };
}

function transformProperty(p: KnotVariableProperty): Record<string, unknown> {
    const passageMap = new Map<string, {
        passage_name: string;
        writes: KnotVariableLocation[];
        reads: KnotVariableLocation[];
    }>();

    for (const loc of p.written_in || []) {
        let entry = passageMap.get(loc.passage_name);
        if (!entry) {
            entry = { passage_name: loc.passage_name, writes: [], reads: [] };
            passageMap.set(loc.passage_name, entry);
        }
        entry.writes.push(loc);
    }
    for (const loc of p.read_in || []) {
        let entry = passageMap.get(loc.passage_name);
        if (!entry) {
            entry = { passage_name: loc.passage_name, writes: [], reads: [] };
            passageMap.set(loc.passage_name, entry);
        }
        entry.reads.push(loc);
    }

    // Build passages array — same structure as transformVariable so that
    // the three-zone drill-down view works for property nodes too.
    // Without this, drilling into a child property shows "No references found"
    // even though ref_count is non-zero.
    const passages = Array.from(passageMap.values()).map(p => {
        const references = [
            ...p.writes.map(w => ({
                is_write: true,
                line: w.line,
                span: w.span,
                is_struct_def: false,
                is_reassign: false,
                type_conflict: false,
            })),
            ...p.reads.map(r => ({
                is_write: false,
                line: r.line,
                span: r.span,
                is_struct_def: false,
                is_reassign: false,
                type_conflict: false,
            })),
        ];
        return {
            passage_name: p.passage_name,
            reachable: true,
            total_refs: references.length,
            in_loop: false,
            references,
        };
    });

    const totalWrites = (p.written_in || []).length;
    const totalReads = (p.read_in || []).length;

    // For Array-kind properties, children come from element_shape.[*]
    // (same pattern as Array root variables in transformVariable).
    // For Object/Unknown properties, children come from properties directly.
    let children: Record<string, unknown>[];
    let elementShape: Record<string, unknown> | null = null;

    if (p.kind === 'array' && p.element_shape) {
        elementShape = transformProperty(p.element_shape);
        children = (p.element_shape.properties || []).map(transformProperty);
    } else {
        children = (p.properties || []).map(transformProperty);
        elementShape = p.element_shape ? transformProperty(p.element_shape) : null;
    }

    return {
        name: p.name,
        full_name: p.full_name,
        state_path: p.state_path,
        ref_count: totalWrites + totalReads,
        passage_count: Array.from(passageMap.keys()).length,
        passages,
        children,
        elementShape,
        coverage: p.coverage || null,
        kind: p.kind || 'unknown',
    };
}

// ---------------------------------------------------------------------------
// Variable Tracking View webview provider (three-zone design)
// ---------------------------------------------------------------------------

export class VariableFlowProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.variableFlowView';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _flowData: KnotVariableFlowResponse | null = null;
    private _filter: string = '';
    private _refreshDebounceTimer: ReturnType<typeof setTimeout> | null = null;
    private static readonly DEBOUNCE_MS = 500;
    private static readonly MAX_INITIAL_RETRIES = 15;
    private static readonly INITIAL_RETRY_MS = 2000;
    private _initialRetryCount = 0;
    private _initialRetryTimer: ReturnType<typeof setTimeout> | null = null;

    constructor(private readonly _extensionUri: vscode.Uri) {}

    /** Set the language client reference. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
        if (this._view) {
            this._startInitialPolling();
        }
    }

    /** Start polling until the server is ready and data is fetched. */
    private _startInitialPolling() {
        this._initialRetryCount = 0;
        this._pollInitial();
    }

    private async _pollInitial() {
        if (this._initialRetryCount >= VariableFlowProvider.MAX_INITIAL_RETRIES) {
            return;
        }
        this._initialRetryCount++;
        const clientReady = this._client && this._client.isRunning();
        const viewReady = !!this._view;
        if (clientReady && viewReady) {
            const gotData = await this._fetchAndPost();
            if (!gotData) {
                this._initialRetryTimer = setTimeout(() => this._pollInitial(), VariableFlowProvider.INITIAL_RETRY_MS);
            }
        } else {
            this._initialRetryTimer = setTimeout(() => this._pollInitial(), VariableFlowProvider.INITIAL_RETRY_MS);
        }
    }

    /** Clean up pending timers when the provider is disposed. */
    public dispose() {
        if (this._initialRetryTimer) {
            clearTimeout(this._initialRetryTimer);
            this._initialRetryTimer = null;
        }
        if (this._refreshDebounceTimer) {
            clearTimeout(this._refreshDebounceTimer);
            this._refreshDebounceTimer = null;
        }
    }

    /** Core fetch — returns true if data was obtained. */
    private async _fetchAndPost(): Promise<boolean> {
        if (!this._client || !this._client.isRunning()) {
            return false;
        }
        if (!this._view) {
            return false;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return false;
        }

        try {
            const result = await this._client.sendRequest<KnotVariableFlowResponse>('knot/variableFlow', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            } as KnotVariableFlowParams);

            // Transform the server's flat response into the nested format
            // the webview expects for its three-zone rendering
            this._flowData = transformVariableFlow(result) as unknown as KnotVariableFlowResponse;
            this._postFlowData();
            return true;
        } catch (e) {
            console.error('[Knot] Failed to fetch variable flow:', e);
            return false;
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
                    const { name, line, spanStart, spanEnd } = message;
                    if (name) {
                        await vscode.commands.executeCommand('knot.openPassageByName', name, line ?? 0, spanStart, spanEnd);
                    }
                    break;
                }
            }
        });

        if (this._client) {
            this._startInitialPolling();
        }

        // Re-fetch data when the view becomes visible again (e.g. after
        // the sidebar was collapsed and re-expanded).
        webviewView.onDidChangeVisibility(() => {
            if (webviewView.visible) {
                this.refresh();
            }
        });
    }

    /** Refresh variable flow data from the language server (debounced). */
    public refresh() {
        if (this._refreshDebounceTimer) {
            clearTimeout(this._refreshDebounceTimer);
        }
        this._refreshDebounceTimer = setTimeout(() => {
            this._refreshDebounceTimer = null;
            this._fetchAndPost();
        }, VariableFlowProvider.DEBOUNCE_MS);
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

    /** Generate the HTML for the webview with three-zone layout. */
    private _getHtmlForWebview(_webview: vscode.Webview): string {
        return /*html*/`<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline' https:; img-src 'self' data:; connect-src 'self';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Variable Tracking</title>
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
            --warning: #e0a526;
            --prop-color: #ce9178;
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 12px;
            padding: 0;
        }

        /* -- Toolbar -- */

        .toolbar {
            display: flex;
            gap: 4px;
            padding: 6px 8px;
            align-items: center;
            border-bottom: 1px solid var(--border);
            background: var(--bg);
            z-index: 10;
        }

        .toolbar button {
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 12px;
            flex-shrink: 0;
            line-height: 1;
        }

        .toolbar button:hover { background: var(--accent); color: white; }
        .toolbar button:disabled { opacity: 0.4; cursor: default; }
        .toolbar button:disabled:hover { background: var(--card); color: var(--fg); }

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
        .filter-input:focus { border-color: var(--accent); }
        .filter-input::placeholder { color: var(--muted); }

        /* -- Zone 1: Navigation -- */

        .zone-nav {
            padding: 6px 10px;
            background: var(--card);
            border-bottom: 1px solid var(--border);
        }

        .breadcrumb {
            display: flex;
            align-items: center;
            gap: 2px;
            flex-wrap: wrap;
            min-height: 20px;
            margin-bottom: 4px;
        }

        .crumb {
            font-size: 11px;
            color: var(--accent);
            cursor: pointer;
            padding: 1px 4px;
            border-radius: 2px;
            white-space: nowrap;
        }
        .crumb:hover { background: var(--hover); text-decoration: underline; }
        .crumb.current { color: var(--fg); cursor: default; font-weight: 600; }
        .crumb.current:hover { background: transparent; text-decoration: none; }

        .crumb-sep {
            color: var(--muted);
            font-size: 10px;
            user-select: none;
            margin: 0 1px;
        }

        .nav-buttons {
            display: flex;
            gap: 4px;
            margin-bottom: 4px;
        }

        .nav-buttons button {
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 2px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
        }
        .nav-buttons button:hover { background: var(--accent); color: white; }
        .nav-buttons button:disabled { opacity: 0.4; cursor: default; }
        .nav-buttons button:disabled:hover { background: var(--card); color: var(--fg); }

        .summary-line {
            font-size: 10px;
            color: var(--muted);
        }

        /* -- Zone 2: Passage List -- */

        .zone-passages {
            border-bottom: 1px solid var(--border);
            max-height: 40vh;
            overflow-y: auto;
        }

        .passage-row {
            display: flex;
            align-items: center;
            padding: 4px 12px;
            gap: 6px;
            cursor: pointer;
            transition: background 0.1s ease;
        }
        .passage-row:hover { background: var(--hover); }
        .passage-row.selected { background: var(--hover); border-left: 2px solid var(--accent); padding-left: 10px; }

        .passage-chevron {
            color: var(--muted);
            font-size: 9px;
            width: 10px;
            text-align: center;
            flex-shrink: 0;
        }

        .passage-name {
            font-size: 11px;
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
            min-width: 0;
        }

        .passage-loop {
            color: var(--warning);
            font-size: 11px;
            flex-shrink: 0;
        }

        .passage-refs {
            margin-left: auto;
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
        }

        .passage-flag {
            font-size: 9px;
            color: var(--warning);
            flex-shrink: 0;
        }

        .unreachable-sep {
            padding: 4px 12px;
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.3px;
            border-top: 1px solid var(--border);
        }

        /* -- Zone 3: Detail -- */

        .zone-detail {
            padding: 6px 0;
        }

        .detail-passage-header {
            padding: 4px 12px;
            font-size: 11px;
            font-weight: 600;
            color: var(--fg);
            border-bottom: 1px solid var(--border);
            margin-bottom: 2px;
        }

        .ref-row {
            display: flex;
            align-items: center;
            padding: 3px 12px 3px 20px;
            gap: 6px;
        }

        .ref-type {
            font-size: 10px;
            font-weight: 600;
            width: 18px;
            text-align: center;
            flex-shrink: 0;
        }
        .ref-type.write { color: var(--success); }
        .ref-type.read { color: var(--accent); }

        .ref-line {
            font-size: 11px;
            color: var(--accent);
            cursor: pointer;
            font-family: monospace;
        }
        .ref-line:hover { text-decoration: underline; }

        .ref-flag {
            font-size: 9px;
            padding: 0 4px;
            border-radius: 2px;
        }
        .ref-flag.struct-def { background: rgba(102,187,106,0.15); color: var(--success); }
        .ref-flag.reassign { background: rgba(224,165,38,0.15); color: var(--warning); }
        .ref-flag.type-conflict { background: rgba(241,76,76,0.15); color: var(--error); }

        /* -- Root variable list -- */

        .var-row {
            display: flex;
            align-items: center;
            padding: 5px 12px;
            gap: 4px;
            transition: background 0.1s ease;
        }
        .var-row:hover { background: var(--hover); }

        .var-chevron {
            color: var(--muted);
            font-size: 9px;
            width: 14px;
            height: 14px;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
            cursor: pointer;
            border-radius: 2px;
            transition: background 0.1s ease, transform 0.15s ease;
            user-select: none;
        }
        .var-chevron:hover { background: rgba(255,255,255,0.08); }
        .var-chevron.expanded { transform: rotate(90deg); }
        .var-chevron.leaf { cursor: default; opacity: 0.4; }
        .var-chevron.leaf:hover { background: transparent; }

        .var-name-wrap {
            flex: 1;
            min-width: 0;
            cursor: pointer;
            padding: 1px 4px;
            border-radius: 2px;
        }
        .var-name-wrap:hover { background: rgba(255,255,255,0.04); }

        .var-name {
            font-family: monospace;
            font-size: 12px;
            font-weight: 600;
            color: var(--fg);
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
            min-width: 0;
        }

        .var-kind-badge {
            font-family: monospace;
            font-size: 9px;
            font-weight: 400;
            color: var(--muted);
            margin-left: 3px;
            opacity: 0.7;
        }

        .coverage-badge {
            font-family: monospace;
            font-size: 9px;
            color: #e8a838;
            margin-left: 2px;
        }

        .var-meta {
            margin-left: auto;
            display: flex;
            gap: 6px;
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
            align-items: center;
        }

        .badge {
            display: inline-block;
            padding: 1px 5px;
            border-radius: 3px;
            font-size: 9px;
            font-weight: 500;
            flex-shrink: 0;
        }
        .badge-warning { background: rgba(224,165,38,0.15); color: var(--warning); }
        .badge-error { background: rgba(241,76,76,0.15); color: var(--error); }
        .badge-info { background: rgba(139,139,139,0.2); color: var(--muted); }

        /* -- Children sublist in root -- */

        .var-children {
            overflow: hidden;
        }

        .child-row {
            display: flex;
            align-items: center;
            padding: 3px 12px 3px 28px;
            gap: 6px;
            cursor: pointer;
            transition: background 0.1s ease;
        }
        .child-row:hover { background: var(--hover); }

        .child-chevron {
            color: var(--muted);
            font-size: 8px;
            width: 12px;
            height: 12px;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
            cursor: pointer;
            border-radius: 2px;
            transition: background 0.1s ease, transform 0.15s ease;
            user-select: none;
        }
        .child-chevron:hover { background: rgba(255,255,255,0.08); }
        .child-chevron.expanded { transform: rotate(90deg); }
        .child-chevron.leaf { cursor: default; opacity: 0.4; }
        .child-chevron.leaf:hover { background: transparent; }

        .child-name-wrap {
            flex: 1;
            min-width: 0;
            cursor: pointer;
            padding: 1px 2px;
            border-radius: 2px;
        }
        .child-name-wrap:hover { background: rgba(255,255,255,0.04); }

        .child-prefix {
            font-family: monospace;
            font-size: 11px;
            color: var(--prop-color);
        }

        .child-name {
            font-family: monospace;
            font-size: 11px;
            color: var(--fg);
        }

        .child-meta {
            margin-left: auto;
            font-size: 10px;
            color: var(--muted);
            flex-shrink: 0;
        }

        .grandchild-row {
            display: flex;
            align-items: center;
            padding: 2px 12px 2px 44px;
            gap: 5px;
            cursor: pointer;
            transition: background 0.1s ease;
        }
        .grandchild-row:hover { background: var(--hover); }

        /* -- Empty / loading states -- */

        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 24px 12px;
            font-size: 11px;
        }

        .section-label {
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.4px;
            padding: 6px 12px 3px;
        }
    </style>
</head>
<body>
    <div id="root">
        <div class="empty-state">Loading variables...</div>
    </div>

    <script>
        var vscode = acquireVsCodeApi();

        // -- Event delegation --
        document.addEventListener('click', function(e) {
            // Check for chevron clicks first (before the broader data-action handler)
            var chevronEl = e.target.closest('.var-chevron[data-fullname]');
            if (chevronEl && !chevronEl.classList.contains('leaf')) {
                toggleExpand(chevronEl.dataset.fullname);
                return;
            }
            var childChevronEl = e.target.closest('.child-chevron[data-fullname]');
            if (childChevronEl && !childChevronEl.classList.contains('leaf')) {
                toggleExpand(childChevronEl.dataset.fullname);
                return;
            }
            var actionEl = e.target.closest('[data-action]');
            if (actionEl) {
                var action = actionEl.dataset.action;
                if (action === 'refresh') { vscode.postMessage({ command: 'refresh' }); }
                else if (action === 'goHome') { goHome(); }
                else if (action === 'goBack') { goBack(); }
                else if (action === 'drillTo') { drillTo(parseInt(actionEl.dataset.level || '0', 10)); }
                else if (action === 'drillVariable') { drillVariable(actionEl.dataset.name); }
                else if (action === 'drillChild') { drillChild(actionEl.dataset.fullname); }
                else if (action === 'selectPassage') { selectPassage(actionEl.dataset.passage); }
                return;
            }
            var lineEl = e.target.closest('[data-passage][data-line]');
            if (lineEl) {
                var msg = { command: 'openPassage', name: lineEl.dataset.passage, line: parseInt(lineEl.dataset.line || '0', 10) };
                // Include span data for precise range-based navigation.
                if (lineEl.dataset.spanStart !== undefined && lineEl.dataset.spanEnd !== undefined) {
                    msg.spanStart = parseInt(lineEl.dataset.spanStart, 10);
                    msg.spanEnd = parseInt(lineEl.dataset.spanEnd, 10);
                }
                vscode.postMessage(msg);
            }
        });

        // -- Filter input delegation --
        document.addEventListener('input', function(e) {
            if (e.target.classList && e.target.classList.contains('filter-input')) {
                vscode.postMessage({ command: 'filterVariable', filter: e.target.value });
            }
        });

        // -- State --
        var flowData = null;
        var currentFilter = '';

        // Navigation stack: array of { variableFullName, selectedPassage, scrollOffset }
        var navStack = [];
        // Current view: { variable: KnotVariableInfo, selectedPassage: string|null } or null (root list)
        var currentView = null;

        // Expanded nodes tracking: Set of full_name values that are expanded
        var expandedNodes = new Set();

        // -- Helpers --

        function esc(str) {
            var d = document.createElement('div');
            d.textContent = str;
            return d.innerHTML;
        }

        function findVariableByFullName(fullName) {
            if (!flowData) return null;
            return findInList(flowData.variables || [], fullName);
        }

        function findInList(list, fullName) {
            for (var i = 0; i < list.length; i++) {
                if (list[i].full_name === fullName) return list[i];
                if (list[i].children && list[i].children.length > 0) {
                    var found = findInList(list[i].children, fullName);
                    if (found) return found;
                }
                // Also search in elementShape children (for array root variables)
                if (list[i].elementShape && list[i].elementShape.children && list[i].elementShape.children.length > 0) {
                    var found = findInList(list[i].elementShape.children, fullName);
                    if (found) return found;
                }
            }
            return null;
        }

        function matchesFilter(item, lf) {
            if (!lf) return true;
            if ((item.name || '').toLowerCase().indexOf(lf) >= 0) return true;
            if ((item.full_name || '').toLowerCase().indexOf(lf) >= 0) return true;
            if (item.children) {
                for (var i = 0; i < item.children.length; i++) {
                    if (matchesFilter(item.children[i], lf)) return true;
                }
            }
            if (item.elementShape && item.elementShape.children) {
                for (var i = 0; i < item.elementShape.children.length; i++) {
                    if (matchesFilter(item.elementShape.children[i], lf)) return true;
                }
            }
            return false;
        }

        // -- Navigation --

        function drillVariable(varName) {
            var v = findVariableByFullName(varName);
            if (!v) return;
            // Push current state
            if (currentView) {
                navStack.push({
                    variableFullName: currentView.variable.full_name,
                    selectedPassage: currentView.selectedPassage,
                    scrollOffset: 0
                });
            }
            currentView = {
                variable: v,
                selectedPassage: v.passages && v.passages.length > 0 ? v.passages[0].passage_name : null
            };
            render();
        }

        function drillChild(fullName) {
            var v = findVariableByFullName(fullName);
            if (!v) return;
            if (currentView) {
                navStack.push({
                    variableFullName: currentView.variable.full_name,
                    selectedPassage: currentView.selectedPassage,
                    scrollOffset: 0
                });
            }
            currentView = {
                variable: v,
                selectedPassage: v.passages && v.passages.length > 0 ? v.passages[0].passage_name : null
            };
            render();
        }

        function goBack() {
            if (navStack.length > 0) {
                var prev = navStack.pop();
                var v = findVariableByFullName(prev.variableFullName);
                if (v) {
                    currentView = { variable: v, selectedPassage: prev.selectedPassage };
                } else {
                    currentView = null;
                }
            } else {
                // At root→variable level, navStack is empty but we still need
                // to go back to the root variable list.
                currentView = null;
            }
            render();
        }

        function goHome() {
            navStack = [];
            currentView = null;
            render();
        }

        function drillTo(levelIdx) {
            if (levelIdx < 0 || levelIdx >= navStack.length) {
                goHome();
                return;
            }
            var target = navStack[levelIdx];
            navStack = navStack.slice(0, levelIdx);
            var v = findVariableByFullName(target.variableFullName);
            if (v) {
                currentView = { variable: v, selectedPassage: target.selectedPassage };
            } else {
                currentView = null;
            }
            render();
        }

        function selectPassage(passageName) {
            if (currentView) {
                currentView.selectedPassage = passageName;
                render();
            }
        }

        function toggleExpand(fullName) {
            if (expandedNodes.has(fullName)) {
                expandedNodes.delete(fullName);
            } else {
                expandedNodes.add(fullName);
            }
            render();
        }

        // -- Breadcrumb --

        function buildBreadcrumb() {
            var segs = [];
            segs.push({ label: 'Variables', level: -1, current: !currentView });
            for (var i = 0; i < navStack.length; i++) {
                var entry = navStack[i];
                var name = entry.variableFullName;
                var parts = name.split('.');
                segs.push({ label: parts[parts.length - 1], level: i, current: false });
            }
            if (currentView) {
                var cn = currentView.variable.full_name.split('.');
                segs.push({ label: cn[cn.length - 1], level: navStack.length, current: true });
            }

            // Truncation: if more than 4 segments, collapse middle
            if (segs.length > 4) {
                var first = segs[0];
                var last = segs[segs.length - 1];
                var mid = segs[segs.length - 2];
                segs = [first, { label: '...', level: -2, current: false }, mid, last];
            }

            return segs;
        }

        // -- Flag rendering --

        function renderFlags(flags) {
            if (!flags || flags.length === 0) return '';
            var html = '';
            for (var i = 0; i < flags.length; i++) {
                var f = flags[i];
                var cls = 'badge-info';
                if (f.flag_type === 'unused' || f.flag_type === 'write-only') cls = 'badge-error';
                else if (f.flag_type === 'single-use') cls = 'badge-warning';
                html += '<span class="badge ' + cls + '" title="' + esc(f.message) + '">' + esc(f.flag_type) + '</span>';
            }
            return html;
        }

        // -- Rendering --

        function render() {
            if (!currentView) {
                renderRootList();
            } else {
                renderVariableView();
            }
        }

        function renderRootList() {
            var root = document.getElementById('root');
            var html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button data-action="goHome" title="Home" disabled>&#x2302;</button>';
            html += '<button data-action="goBack" title="Back" disabled>&#x2190;</button>';
            html += '<input class="filter-input" type="text" placeholder="Filter variables..." value="' + esc(currentFilter) + '" />';
            html += '<button data-action="refresh" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            var variables = flowData ? (flowData.variables || []) : [];
            if (currentFilter) {
                var lf = currentFilter.toLowerCase();
                variables = variables.filter(function(v) { return matchesFilter(v, lf); });
            }

            html += '<div class="section-label">' + variables.length + ' variable' + (variables.length !== 1 ? 's' : '') + '</div>';

            if (variables.length === 0) {
                html += '<div class="empty-state">' + (currentFilter ? 'No variables match filter' : 'No variables found') + '</div>';
            } else {
                for (var i = 0; i < variables.length; i++) {
                    var v = variables[i];
                    var isArrRoot = v.kind === 'array' && v.elementShape;
                    var hasChildren = (v.children && v.children.length > 0) || isArrRoot;
                    var isExpanded = expandedNodes.has(v.full_name);

                    // Variable row with separate chevron and name click targets
                    html += '<div class="var-row">';
                    html += '<span class="var-chevron' + (isExpanded ? ' expanded' : '') + (!hasChildren ? ' leaf' : '') + '" data-fullname="' + esc(v.full_name) + '" title="' + (hasChildren ? (isExpanded ? 'Collapse' : 'Expand') : 'No children') + '">&#x25B6;</span>';
                    html += '<span class="var-name-wrap" data-action="drillVariable" data-name="' + esc(v.full_name) + '" title="View references for ' + esc(v.name) + '">';
                    html += '<span class="var-name">' + esc(v.name) + '</span>';
                    if (v.kind === 'array') {
                        html += '<span class="var-kind-badge">[ ]</span>';
                    } else if (v.kind === 'object') {
                        html += '<span class="var-kind-badge">{ }</span>';
                    }
                    html += '</span>';
                    html += '<span class="var-meta">';
                    html += renderFlags(v.flags);
                    html += '<span>' + v.ref_count + ' ref' + (v.ref_count !== 1 ? 's' : '') + ' &middot; ' + v.passage_count + ' passage' + (v.passage_count !== 1 ? 's' : '') + '</span>';
                    html += '</span>';
                    html += '</div>';

                    // Children — only rendered when expanded
                    if (hasChildren && isExpanded) {
                        html += '<div class="var-children">';
                        // For array root vars, children come from elementShape with [*] prefix
                        if (isArrRoot && v.elementShape && v.elementShape.children) {
                            for (var j = 0; j < v.elementShape.children.length; j++) {
                                html += renderArrayElementChild(v.elementShape.children[j], v.full_name);
                            }
                        } else {
                            for (var j = 0; j < (v.children || []).length; j++) {
                                html += renderChildRow(v.children[j]);
                            }
                        }
                        html += '</div>';
                    }
                }
            }

            root.innerHTML = html;
            restoreFilterFocus();
        }

        /** Render a child row for array element properties (with [*] prefix). */
        function renderArrayElementChild(c, parentFullName) {
            var html = '';
            var hasGrandchildren = c.children && c.children.length > 0;
            // For arrays, element_shape children are also grandchildren
            if (!hasGrandchildren && c.elementShape && c.elementShape.children && c.elementShape.children.length > 0) {
                hasGrandchildren = true;
            }
            // Use the full_name from the property node for expansion tracking
            var childFullName = c.full_name || (parentFullName + '[*].' + c.name);
            var isExpanded = expandedNodes.has(childFullName);

            html += '<div class="child-row">';
            html += '<span class="child-chevron' + (isExpanded ? ' expanded' : '') + (!hasGrandchildren ? ' leaf' : '') + '" data-fullname="' + esc(childFullName) + '" title="' + (hasGrandchildren ? (isExpanded ? 'Collapse' : 'Expand') : 'No children') + '">&#x25B6;</span>';
            html += '<span class="child-prefix">[*].</span>';
            html += '<span class="child-name-wrap" data-action="drillChild" data-fullname="' + esc(childFullName) + '" title="View references for ' + esc(childFullName) + '">';
            html += '<span class="child-name">' + esc(c.name) + '</span>';
            if (c.kind === 'array') {
                html += '<span class="var-kind-badge">[ ]</span>';
            }
            if (c.coverage) {
                html += '<span class="coverage-badge">' + esc(c.coverage) + '</span>';
            }
            html += '</span>';
            html += '<span class="child-meta">' + c.ref_count + ' ref' + (c.ref_count !== 1 ? 's' : '') + '</span>';
            html += '</div>';

            // Grandchildren — only rendered when expanded
            if (hasGrandchildren && isExpanded) {
                html += '<div class="var-children">';
                // If this child has its own element_shape (nested array), render those
                if (c.elementShape && c.elementShape.children) {
                    for (var k = 0; k < c.elementShape.children.length; k++) {
                        var gc = c.elementShape.children[k];
                        html += renderArrayElementChild(gc, childFullName);
                    }
                }
                // Regular grandchildren
                for (var k = 0; k < (c.children || []).length; k++) {
                    var gc = c.children[k];
                    html += '<div class="grandchild-row" data-action="drillChild" data-fullname="' + esc(gc.full_name) + '" title="View references for ' + esc(gc.full_name) + '">';
                    html += '<span class="child-prefix">.</span>';
                    html += '<span class="child-name">' + esc(gc.name) + '</span>';
                    if (gc.kind === 'array') {
                        html += '<span class="var-kind-badge">[ ]</span>';
                    }
                    if (gc.coverage) {
                        html += '<span class="coverage-badge">' + esc(gc.coverage) + '</span>';
                    }
                    html += '<span class="child-meta">' + gc.ref_count + ' ref' + (gc.ref_count !== 1 ? 's' : '') + '</span>';
                    html += '</div>';
                }
                html += '</div>';
            }

            return html;
        }

        function renderChildRow(c) {
            var html = '';
            var hasGrandchildren = c.children && c.children.length > 0;
            // For arrays, element_shape children are also grandchildren
            if (!hasGrandchildren && c.elementShape && c.elementShape.children && c.elementShape.children.length > 0) {
                hasGrandchildren = true;
            }
            var isExpanded = expandedNodes.has(c.full_name);

            html += '<div class="child-row">';
            html += '<span class="child-chevron' + (isExpanded ? ' expanded' : '') + (!hasGrandchildren ? ' leaf' : '') + '" data-fullname="' + esc(c.full_name) + '" title="' + (hasGrandchildren ? (isExpanded ? 'Collapse' : 'Expand') : 'No children') + '">&#x25B6;</span>';
            html += '<span class="child-prefix">.</span>';
            html += '<span class="child-name-wrap" data-action="drillChild" data-fullname="' + esc(c.full_name) + '" title="View references for ' + esc(c.full_name) + '">';
            html += '<span class="child-name">' + esc(c.name) + '</span>';
            if (c.kind === 'array') {
                html += '<span class="var-kind-badge">[ ]</span>';
            }
            if (c.coverage) {
                html += '<span class="coverage-badge">' + esc(c.coverage) + '</span>';
            }
            html += '</span>';
            html += '<span class="child-meta">' + c.ref_count + ' ref' + (c.ref_count !== 1 ? 's' : '') + '</span>';
            html += '</div>';

            // Grandchildren — only rendered when expanded
            if (hasGrandchildren && isExpanded) {
                html += '<div class="var-children">';
                for (var k = 0; k < c.children.length; k++) {
                    var gc = c.children[k];
                    html += '<div class="grandchild-row" data-action="drillChild" data-fullname="' + esc(gc.full_name) + '" title="View references for ' + esc(gc.full_name) + '">';
                    html += '<span class="child-prefix">.</span>';
                    html += '<span class="child-name">' + esc(gc.name) + '</span>';
                    html += '<span class="child-meta">' + gc.ref_count + ' ref' + (gc.ref_count !== 1 ? 's' : '') + '</span>';
                    html += '</div>';
                }
                html += '</div>';
            }

            return html;
        }

        function renderVariableView() {
            var root = document.getElementById('root');
            var v = currentView.variable;
            var html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button data-action="goHome" title="Home">&#x2302;</button>';
            html += '<button data-action="goBack" title="Back"' + (!currentView ? ' disabled' : '') + '>&#x2190;</button>';
            html += '<input class="filter-input" type="text" placeholder="Filter..." value="' + esc(currentFilter) + '" />';
            html += '<button data-action="refresh" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            // Zone 1: Navigation
            html += '<div class="zone-nav">';

            // Breadcrumb
            var segs = buildBreadcrumb();
            html += '<div class="breadcrumb">';
            for (var i = 0; i < segs.length; i++) {
                if (i > 0) html += '<span class="crumb-sep">&#x276F;</span>';
                var s = segs[i];
                if (s.current) {
                    html += '<span class="crumb current">' + esc(s.label) + '</span>';
                } else if (s.level === -1) {
                    html += '<span class="crumb" data-action="goHome">' + esc(s.label) + '</span>';
                } else {
                    html += '<span class="crumb" data-action="drillTo" data-level="' + s.level + '">' + esc(s.label) + '</span>';
                }
            }
            html += '</div>';

            // Summary
            html += '<div class="summary-line">' + v.passage_count + ' passage' + (v.passage_count !== 1 ? 's' : '') + ' &middot; ' + v.ref_count + ' ref' + (v.ref_count !== 1 ? 's' : '') + '</div>';

            // Flags
            if (v.flags && v.flags.length > 0) {
                html += '<div style="margin-top:2px">' + renderFlags(v.flags) + '</div>';
            }

            // Children list (for drill-down)
            var isArrDrill = v.kind === 'array' && v.elementShape;
            var drillChildren = isArrDrill ? (v.elementShape.children || []) : (v.children || []);
            if (drillChildren.length > 0) {
                html += '<div style="margin-top:4px;display:flex;flex-wrap:wrap;gap:4px">';
                for (var ci = 0; ci < drillChildren.length; ci++) {
                    var ch = drillChildren[ci];
                    var prefix = isArrDrill ? '[*].' : '.';
                    html += '<span class="crumb" data-action="drillChild" data-fullname="' + esc(ch.full_name) + '" title="' + esc(ch.full_name) + ': ' + ch.ref_count + ' refs">' + prefix + esc(ch.name) + '</span>';
                }
                html += '</div>';
            }

            html += '</div>'; // .zone-nav

            // Zone 2: Passage List
            html += '<div class="zone-passages">';

            var passages = v.passages || [];
            var reachablePassages = [];
            var unreachablePassages = [];
            for (var pi = 0; pi < passages.length; pi++) {
                if (passages[pi].reachable) {
                    reachablePassages.push(passages[pi]);
                } else {
                    unreachablePassages.push(passages[pi]);
                }
            }

            // Render reachable passages
            for (var ri = 0; ri < reachablePassages.length; ri++) {
                html += renderPassageRow(reachablePassages[ri], currentView.selectedPassage);
            }

            // Render unreachable separator + passages
            if (unreachablePassages.length > 0) {
                html += '<div class="unreachable-sep">Unreachable</div>';
                for (var ui = 0; ui < unreachablePassages.length; ui++) {
                    html += renderPassageRow(unreachablePassages[ui], currentView.selectedPassage);
                }
            }

            if (passages.length === 0) {
                html += '<div class="empty-state">No references found</div>';
            }

            html += '</div>'; // .zone-passages

            // Zone 3: Detail
            html += '<div class="zone-detail">';
            var selectedPassage = null;
            if (currentView.selectedPassage) {
                for (var si = 0; si < passages.length; si++) {
                    if (passages[si].passage_name === currentView.selectedPassage) {
                        selectedPassage = passages[si];
                        break;
                    }
                }
            }

            if (selectedPassage && selectedPassage.references && selectedPassage.references.length > 0) {
                html += '<div class="detail-passage-header">' + esc(selectedPassage.passage_name) + '</div>';
                for (var li = 0; li < selectedPassage.references.length; li++) {
                    var ref = selectedPassage.references[li];
                    html += '<div class="ref-row">';
                    html += '<span class="ref-type ' + (ref.is_write ? 'write' : 'read') + '">' + (ref.is_write ? 'W' : 'R') + '</span>';
                    // Include span data for precise range-based navigation.
                    // The span is a [start, end] byte offset pair (document-absolute).
                    var spanAttrs = '';
                    if (ref.span) {
                        spanAttrs = ' data-span-start="' + ref.span[0] + '" data-span-end="' + ref.span[1] + '"';
                    }
                    html += '<span class="ref-line" data-passage="' + esc(selectedPassage.passage_name) + '" data-line="' + ref.line + '"' + spanAttrs + '>line ' + ref.line + '</span>';
                    if (ref.is_struct_def) html += '<span class="ref-flag struct-def">struct def</span>';
                    if (ref.is_reassign) html += '<span class="ref-flag reassign">reassign</span>';
                    if (ref.type_conflict) html += '<span class="ref-flag type-conflict">type conflict</span>';
                    html += '</div>';
                }
            } else if (currentView.selectedPassage) {
                html += '<div class="detail-passage-header">' + esc(currentView.selectedPassage) + '</div>';
                html += '<div class="empty-state">No direct references in this passage</div>';
            } else {
                html += '<div class="empty-state">Select a passage above</div>';
            }

            html += '</div>'; // .zone-detail

            root.innerHTML = html;
            restoreFilterFocus();
        }

        function renderPassageRow(p, selectedPassage) {
            var isSelected = (p.passage_name === selectedPassage);
            var html = '<div class="passage-row' + (isSelected ? ' selected' : '') + '" data-action="selectPassage" data-passage="' + esc(p.passage_name) + '">';
            html += '<span class="passage-chevron">' + (isSelected ? '&#x25BC;' : '&#x25B6;') + '</span>';
            html += '<span class="passage-name">' + esc(p.passage_name) + '</span>';
            if (p.in_loop) html += '<span class="passage-loop" title="This passage is part of a loop">&#x1F501;</span>';
            // Flags for struct def / reassign in references
            var hasStructDef = false;
            var hasReassign = false;
            var hasConflict = false;
            if (p.references) {
                for (var i = 0; i < p.references.length; i++) {
                    if (p.references[i].is_struct_def) hasStructDef = true;
                    if (p.references[i].is_reassign) hasReassign = true;
                    if (p.references[i].type_conflict) hasConflict = true;
                }
            }
            if (hasStructDef) html += '<span class="passage-flag">&#x26A0; struct</span>';
            if (hasReassign) html += '<span class="passage-flag">&#x26A0; reassign</span>';
            if (hasConflict) html += '<span class="passage-flag">&#x26A0; conflict</span>';
            html += '<span class="passage-refs">' + p.total_refs + ' ref' + (p.total_refs !== 1 ? 's' : '') + '</span>';
            html += '</div>';
            return html;
        }

        function restoreFilterFocus() {
            var inp = document.querySelector('.filter-input');
            if (inp && currentFilter) {
                inp.focus();
                inp.setSelectionRange(currentFilter.length, currentFilter.length);
            }
        }

        // -- Data refresh --

        function refreshNavState() {
            if (!flowData) {
                navStack = [];
                currentView = null;
                return;
            }

            // Rebuild navStack
            var newStack = [];
            for (var i = 0; i < navStack.length; i++) {
                var entry = navStack[i];
                var v = findVariableByFullName(entry.variableFullName);
                if (v) {
                    newStack.push({
                        variableFullName: entry.variableFullName,
                        selectedPassage: entry.selectedPassage,
                        scrollOffset: 0
                    });
                }
            }
            navStack = newStack;

            // Rebuild currentView
            if (currentView) {
                var cv = findVariableByFullName(currentView.variable.full_name);
                if (cv) {
                    currentView.variable = cv;
                    // Keep selectedPassage if it still exists
                    if (currentView.selectedPassage) {
                        var found = false;
                        for (var i = 0; i < (cv.passages || []).length; i++) {
                            if (cv.passages[i].passage_name === currentView.selectedPassage) {
                                found = true;
                                break;
                            }
                        }
                        if (!found && cv.passages && cv.passages.length > 0) {
                            currentView.selectedPassage = cv.passages[0].passage_name;
                        }
                    }
                } else {
                    currentView = null;
                }
            }

            // If currentView became null and stack has entries, pop back
            if (!currentView && navStack.length > 0) {
                var prev = navStack.pop();
                var pv = findVariableByFullName(prev.variableFullName);
                if (pv) {
                    currentView = { variable: pv, selectedPassage: prev.selectedPassage };
                }
            }
        }

        window.addEventListener('message', function(event) {
            var message = event.data;
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
