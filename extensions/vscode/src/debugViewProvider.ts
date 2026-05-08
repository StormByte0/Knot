//! Debug View provider for the Knot extension.
//!
//! This module implements a VS Code sidebar webview panel that shows
//! debug information about the passage under the cursor, including
//! variable state, link connections, reachability, diagnostics,
//! step-over capability, and variable watch.

import * as vscode from 'vscode';
import {
    KnotLanguageClient,
    KnotDebugResponse,
    KnotTraceStep,
    KnotTraceResponse,
    KnotStepChoice,
    KnotStepOverResponse,
    KnotWatchVariable,
    KnotWatchVariablesResponse,
    KnotBreakpointInfo,
} from './types';

// ---------------------------------------------------------------------------
// Debug View webview provider
// ---------------------------------------------------------------------------

export class DebugViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.debugView';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _currentPassage: string = '';
    private _debugData: KnotDebugResponse | null = null;
    private _traceData: KnotTraceResponse | null = null;
    private _watchData: KnotWatchVariablesResponse | null = null;
    private _stepData: KnotStepOverResponse | null = null;
    private _breakpoints: string[] = [];

    constructor(private readonly _extensionUri: vscode.Uri) {}

    /** Set the language client reference. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
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

        // Handle messages from the webview
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'openPassage': {
                    const { name } = message;
                    if (name) {
                        await vscode.commands.executeCommand('knot.openPassageByName', name);
                    }
                    break;
                }
                case 'refresh': {
                    await this.refresh();
                    break;
                }
                case 'trace': {
                    await this._runTrace();
                    break;
                }
                case 'stepOver': {
                    await this._runStepOver();
                    break;
                }
                case 'toggleBreakpoint': {
                    const { name } = message;
                    if (name) {
                        await this._toggleBreakpoint(name);
                    }
                    break;
                }
                case 'watchVariables': {
                    await this._fetchWatchVariables();
                    break;
                }
            }
        });
    }

    /** Update the debug view for the passage under the cursor. */
    public async updateForPassage(passageName: string) {
        if (passageName === this._currentPassage) {
            return;
        }
        this._currentPassage = passageName;
        this._traceData = null;
        this._stepData = null;
        this._watchData = null;
        await this.refresh();
    }

    /** Refresh the debug data from the language server. */
    public async refresh() {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        if (!this._currentPassage) {
            this._postEmptyState();
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result = await this._client.sendRequest<KnotDebugResponse>('knot/debug', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                passage_name: this._currentPassage,
            });

            this._debugData = result;
            this._postDebugData();
        } catch (e) {
            console.error('[Knot] Failed to fetch debug info:', e);
        }
    }

    /** Run a trace from the current passage. */
    private async _runTrace() {
        if (!this._client || !this._client.isRunning() || !this._currentPassage) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result: KnotTraceResponse = await this._client.sendRequest('knot/trace', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                start_passage: this._currentPassage,
                max_depth: 30,
            });

            this._traceData = result;
            this._postDebugData();
        } catch (e) {
            console.error('[Knot] Failed to run trace:', e);
        }
    }

    /** Run a step-over from the current passage. */
    private async _runStepOver() {
        if (!this._client || !this._client.isRunning() || !this._currentPassage) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result: KnotStepOverResponse = await this._client.sendRequest('knot/stepOver', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                from_passage: this._currentPassage,
            });

            this._stepData = result;
            this._postDebugData();
        } catch (e) {
            console.error('[Knot] Failed to step over:', e);
        }
    }

    /** Fetch variable watch data for the current passage. */
    private async _fetchWatchVariables() {
        if (!this._client || !this._client.isRunning() || !this._currentPassage) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result: KnotWatchVariablesResponse = await this._client.sendRequest('knot/watchVariables', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                at_passage: this._currentPassage,
            });

            this._watchData = result;
            this._postDebugData();
        } catch (e) {
            console.error('[Knot] Failed to fetch watch variables:', e);
        }
    }

    /** Toggle a breakpoint on the current passage. */
    private async _toggleBreakpoint(passageName: string) {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        const idx = this._breakpoints.indexOf(passageName);
        if (idx >= 0) {
            this._breakpoints.splice(idx, 1);
        } else {
            this._breakpoints.push(passageName);
        }

        try {
            await this._client.sendRequest('knot/breakpoints', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                set_breakpoints: this._breakpoints,
            });
        } catch (e) {
            console.error('[Knot] Failed to set breakpoints:', e);
        }

        this._postDebugData();
    }

    /** Post empty state to the webview. */
    private _postEmptyState() {
        if (this._view) {
            this._view.webview.postMessage({
                command: 'updateDebug',
                data: { state: 'empty' },
            });
        }
    }

    /** Post the current debug data to the webview. */
    private _postDebugData() {
        if (this._view && this._debugData) {
            this._view.webview.postMessage({
                command: 'updateDebug',
                data: {
                    state: 'loaded',
                    debug: this._debugData,
                    trace: this._traceData,
                    step: this._stepData,
                    watch: this._watchData,
                    breakpoints: this._breakpoints,
                },
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
    <title>Knot Debug</title>
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
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 12px;
            padding: 8px;
        }

        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 20px 0;
        }

        .passage-header {
            display: flex;
            align-items: center;
            justify-content: space-between;
            margin-bottom: 8px;
            padding-bottom: 6px;
            border-bottom: 1px solid var(--border);
        }

        .passage-name {
            font-weight: 600;
            font-size: 14px;
        }

        .badge {
            display: inline-block;
            padding: 1px 6px;
            border-radius: 3px;
            font-size: 10px;
            font-weight: 500;
            margin-left: 4px;
        }

        .badge-special { background: #ffb74d; color: #000; }
        .badge-metadata { background: #ce93d8; color: #000; }
        .badge-unreachable { background: #666; color: #fff; }
        .badge-loop { background: #f14c4c; color: #fff; }
        .badge-reachable { background: #66bb6a; color: #000; }
        .badge-breakpoint { background: #e53935; color: #fff; }

        .toolbar {
            display: flex;
            gap: 4px;
            margin-bottom: 8px;
            flex-wrap: wrap;
        }

        .toolbar button {
            flex: 1;
            min-width: 55px;
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 4px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
        }

        .toolbar button:hover {
            background: var(--accent);
            color: white;
        }

        .section {
            margin-bottom: 10px;
        }

        .section-title {
            font-weight: 600;
            font-size: 11px;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 4px;
        }

        .var-list, .link-list, .diag-list, .trace-list, .step-list, .watch-list {
            list-style: none;
        }

        .var-list li, .link-list li, .diag-list li {
            padding: 2px 0;
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .var-name {
            font-family: monospace;
            font-size: 11px;
        }

        .var-icon { opacity: 0.7; }

        .link-item {
            cursor: pointer;
            color: var(--accent);
        }

        .link-item:hover {
            text-decoration: underline;
        }

        .link-broken {
            color: var(--error);
            text-decoration: line-through;
        }

        .diag-item {
            padding: 4px 6px;
            border-radius: 3px;
            margin-bottom: 2px;
        }

        .diag-error { background: rgba(241, 76, 76, 0.1); }
        .diag-warning { background: rgba(204, 167, 0, 0.1); }
        .diag-info { background: rgba(0, 122, 204, 0.1); }

        .trace-step {
            padding: 3px 6px;
            margin-bottom: 2px;
            border-left: 2px solid var(--accent);
            background: var(--card);
            border-radius: 0 3px 3px 0;
        }

        .trace-loop {
            border-left-color: var(--error);
        }

        .trace-depth {
            color: var(--muted);
            font-size: 10px;
        }

        .tag-list {
            display: flex;
            flex-wrap: wrap;
            gap: 3px;
        }

        .init-var {
            display: inline-block;
            background: rgba(102, 187, 106, 0.15);
            color: var(--success);
            padding: 1px 5px;
            border-radius: 3px;
            font-family: monospace;
            font-size: 10px;
            margin: 1px;
        }

        .uninit-var {
            display: inline-block;
            background: rgba(241, 76, 76, 0.15);
            color: var(--error);
            padding: 1px 5px;
            border-radius: 3px;
            font-family: monospace;
            font-size: 10px;
            margin: 1px;
        }

        .step-choice {
            padding: 3px 6px;
            margin-bottom: 2px;
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 3px;
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .step-choice .arrow {
            color: var(--accent);
        }

        .step-choice.broken {
            border-color: var(--error);
        }

        .watch-section {
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            padding: 6px;
            margin-top: 4px;
        }

        .watch-group-title {
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            margin-bottom: 3px;
            margin-top: 6px;
        }

        .watch-group-title:first-child {
            margin-top: 0;
        }

        .bp-toggle {
            cursor: pointer;
            font-size: 12px;
            padding: 2px 4px;
            border-radius: 2px;
        }

        .bp-toggle:hover {
            background: rgba(255,255,255,0.1);
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Place cursor on a passage to see debug info</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateDebug') {
                renderDebug(message.data);
            }
        });

        function esc(str) {
            const div = document.createElement('div');
            div.textContent = str;
            return div.innerHTML;
        }

        function renderDebug(data) {
            const content = document.getElementById('content');

            if (data.state === 'empty') {
                content.innerHTML = '<div class="empty-state">Place cursor on a passage to see debug info</div>';
                return;
            }

            const d = data.debug;
            const t = data.trace;
            const s = data.step;
            const w = data.watch;
            const bp = data.breakpoints || [];

            let html = '';

            // Header with breakpoint toggle
            html += '<div class="passage-header">';
            html += '<span class="passage-name">';
            const isBp = bp.includes(d.passage_name);
            html += '<span class="bp-toggle" onclick="vscode.postMessage({command: \\'toggleBreakpoint\\', name: \\'' + esc(d.passage_name).replace(/'/g, "\\'") + '\\'})" title="Toggle breakpoint">' + (isBp ? '\\u{1F534}' : '\\u26AA') + '</span> ';
            html += esc(d.passage_name) + '</span>';
            html += '<span>';
            if (d.is_metadata) html += '<span class="badge badge-metadata">Metadata</span>';
            else if (d.is_special) html += '<span class="badge badge-special">Special</span>';
            if (isBp) html += '<span class="badge badge-breakpoint">Breakpoint</span>';
            if (d.is_reachable) html += '<span class="badge badge-reachable">Reachable</span>';
            else html += '<span class="badge badge-unreachable">Unreachable</span>';
            if (d.in_infinite_loop) html += '<span class="badge badge-loop">Loop</span>';
            html += '</span></div>';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button onclick="vscode.postMessage({command: \\'refresh\\'})">Refresh</button>';
            html += '<button onclick="vscode.postMessage({command: \\'trace\\'})">Trace</button>';
            html += '<button onclick="vscode.postMessage({command: \\'stepOver\\'})">Step</button>';
            html += '<button onclick="vscode.postMessage({command: \\'watchVariables\\'})">Watch</button>';
            html += '</div>';

            // Variables written
            if (d.variables_written.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Variables Written</div>';
                html += '<ul class="var-list">';
                for (const v of d.variables_written) {
                    html += '<li><span class="var-icon">&#x270D;</span> <span class="var-name">' + esc(v.name) + '</span>';
                    if (v.is_temporary) html += ' <span class="badge" style="background:#666;color:#fff;font-size:9px;">temp</span>';
                    html += '</li>';
                }
                html += '</ul></div>';
            }

            // Variables read
            if (d.variables_read.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Variables Read</div>';
                html += '<ul class="var-list">';
                for (const v of d.variables_read) {
                    html += '<li><span class="var-icon">&#x1F441;</span> <span class="var-name">' + esc(v.name) + '</span>';
                    if (v.is_temporary) html += ' <span class="badge" style="background:#666;color:#fff;font-size:9px;">temp</span>';
                    html += '</li>';
                }
                html += '</ul></div>';
            }

            // Initialized at entry
            if (d.initialized_at_entry.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Initialized at Entry</div>';
                html += '<div class="tag-list">';
                for (const v of d.initialized_at_entry) {
                    html += '<span class="init-var">' + esc(v) + '</span>';
                }
                html += '</div></div>';
            }

            // Step-over results
            if (s && s.choices.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Step Over: Choices from ' + esc(s.from_passage) + '</div>';
                html += '<ul class="step-list">';
                for (const choice of s.choices) {
                    const cls = choice.target_exists ? 'step-choice' : 'step-choice broken';
                    html += '<li class="' + cls + '">';
                    html += '<span class="arrow">&#x2192;</span> ';
                    html += '<span class="link-item" onclick="vscode.postMessage({command: \\'openPassage\\', name: \\'' + esc(choice.passage_name).replace(/'/g, "\\'") + '\\'})">' + esc(choice.passage_name) + '</span>';
                    if (choice.display_text) html += ' <span style="color:var(--muted)">(' + esc(choice.display_text) + ')</span>';
                    if (!choice.target_exists) html += ' <span style="color:var(--error)">(broken)</span>';
                    html += '</li>';
                }
                html += '</ul>';
                if (s.variables_written.length > 0) {
                    html += '<div style="color:var(--success);font-size:10px;margin-top:4px;">Writes: ' + s.variables_written.map(esc).join(', ') + '</div>';
                }
                if (s.variables_read.length > 0) {
                    html += '<div style="color:var(--muted);font-size:10px;">Reads: ' + s.variables_read.map(esc).join(', ') + '</div>';
                }
                html += '</div>';
            }

            // Outgoing links
            if (d.outgoing_links.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Outgoing Links (' + d.outgoing_links.length + ')</div>';
                html += '<ul class="link-list">';
                for (const l of d.outgoing_links) {
                    const cls = l.target_exists ? 'link-item' : 'link-item link-broken';
                    const label = l.display_text ? esc(l.display_text) + ' \\u2192 ' + esc(l.passage_name) : esc(l.passage_name);
                    html += '<li><span class="' + cls + '" onclick="vscode.postMessage({command: \\'openPassage\\', name: \\'' + esc(l.passage_name).replace(/'/g, "\\'") + '\\'})">' + label + '</span>';
                    if (!l.target_exists) html += ' (broken)';
                    html += '</li>';
                }
                html += '</ul></div>';
            }

            // Incoming links
            if (d.incoming_links.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Incoming Links (' + d.incoming_links.length + ')</div>';
                html += '<ul class="link-list">';
                for (const l of d.incoming_links) {
                    html += '<li><span class="link-item" onclick="vscode.postMessage({command: \\'openPassage\\', name: \\'' + esc(l.passage_name).replace(/'/g, "\\'") + '\\'})">' + esc(l.passage_name) + '</span></li>';
                }
                html += '</ul></div>';
            }

            // Variable Watch panel
            if (w) {
                html += '<div class="section">';
                html += '<div class="section-title">Variable Watch</div>';
                html += '<div class="watch-section">';

                if (w.initialized_at_entry.length > 0) {
                    html += '<div class="watch-group-title">Initialized at Entry</div>';
                    html += '<div class="tag-list">';
                    for (const v of w.initialized_at_entry) {
                        html += '<span class="init-var">' + esc(v.name) + '</span>';
                    }
                    html += '</div>';
                }

                if (w.written_in_passage.length > 0) {
                    html += '<div class="watch-group-title">Written in Passage</div>';
                    html += '<div class="tag-list">';
                    for (const v of w.written_in_passage) {
                        html += '<span class="init-var">' + esc(v.name) + (v.last_written_in ? ' (here)' : '') + '</span>';
                    }
                    html += '</div>';
                }

                if (w.read_in_passage.length > 0) {
                    html += '<div class="watch-group-title">Read in Passage</div>';
                    html += '<div class="tag-list">';
                    for (const v of w.read_in_passage) {
                        html += '<span style="display:inline-block;background:rgba(0,122,204,0.15);color:var(--accent);padding:1px 5px;border-radius:3px;font-family:monospace;font-size:10px;margin:1px;">' + esc(v.name) + '</span>';
                    }
                    html += '</div>';
                }

                if (w.potentially_uninitialized.length > 0) {
                    html += '<div class="watch-group-title">Potentially Uninitialized</div>';
                    html += '<div class="tag-list">';
                    for (const v of w.potentially_uninitialized) {
                        html += '<span class="uninit-var">' + esc(v.name) + '</span>';
                    }
                    html += '</div>';
                }

                html += '</div></div>';
            }

            // Diagnostics
            if (d.diagnostics.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Diagnostics (' + d.diagnostics.length + ')</div>';
                html += '<ul class="diag-list">';
                for (const diag of d.diagnostics) {
                    const cls = diag.kind.includes('Error') || diag.kind === 'BrokenLink' || diag.kind === 'DuplicateStoryData' ? 'diag-error' :
                                diag.kind.includes('Warning') || diag.kind === 'InfiniteLoop' || diag.kind === 'InvalidPassageName' ? 'diag-warning' : 'diag-info';
                    html += '<li class="diag-item ' + cls + '"><strong>' + esc(diag.kind) + '</strong>: ' + esc(diag.message) + '</li>';
                }
                html += '</ul></div>';
            }

            // Trace results
            if (t && t.steps.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Execution Trace' + (t.truncated ? ' (truncated)' : '') + '</div>';
                html += '<ul class="trace-list">';
                for (const step of t.steps) {
                    const cls = step.is_loop ? 'trace-step trace-loop' : 'trace-step';
                    html += '<li class="' + cls + '">';
                    html += '<span class="trace-depth">d' + step.depth + '</span> ';
                    html += '<span class="link-item" onclick="vscode.postMessage({command: \\'openPassage\\', name: \\'' + esc(step.passage_name).replace(/'/g, "\\'") + '\\'})">' + esc(step.passage_name) + '</span>';
                    if (step.is_loop) html += ' (loop)';
                    if (step.variables_written.length > 0) html += ' <span style="color:var(--success)">writes: ' + step.variables_written.map(esc).join(', ') + '</span>';
                    html += '</li>';
                }
                html += '</ul></div>';
            }

            content.innerHTML = html;
        }
    </script>
</body>
</html>`;
    }
}
