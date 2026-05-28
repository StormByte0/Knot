//! Passage Diagnostics view provider for the Knot extension.
//!
//! This module implements a VS Code sidebar webview panel that shows
//! linter diagnostics for the passage under the cursor, including
//! errors, warnings, informational messages, and link status.
//!
//! The view is organized into flat, always-visible sections:
//! 1. **Passage Overview** — name, badges (special, reachable), refresh
//! 2. **Issues** — linter output sorted by severity
//! 3. **Connections** — outgoing and incoming links with status
//! 4. **Variable State** — variable dataflow info from knot/watchVariables

import * as vscode from 'vscode';
import {
    KnotLanguageClient,
    KnotPassageDiagnosticsResponse,
    KnotWatchVariablesResponse,
} from './types';

// ---------------------------------------------------------------------------
// Passage Diagnostics webview provider
// ---------------------------------------------------------------------------

export class DebugViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.debugView';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _currentPassage: string = '';

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
            }
        });
    }

    /** Update the diagnostics view for the passage under the cursor. */
    public async updateForPassage(passageName: string) {
        if (passageName === this._currentPassage) {
            return;
        }
        this._currentPassage = passageName;
        await this.refresh();
    }

    /** Refresh the diagnostics data from the language server. */
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
            // Fetch both passage diagnostics and variable watch data in parallel
            const [diagResult, watchResult] = await Promise.all([
                this._client.sendRequest<KnotPassageDiagnosticsResponse>('knot/passageDiagnostics', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                    passage_name: this._currentPassage,
                }),
                this._client.sendRequest<KnotWatchVariablesResponse>('knot/watchVariables', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                    at_passage: this._currentPassage,
                }).catch(() => null), // Non-critical: graceful fallback if not supported
            ]);

            this._postDiagnosticsData(diagResult, watchResult);
        } catch (e) {
            console.error('[Knot] Failed to fetch passage diagnostics:', e);
        }
    }

    /** Post empty state to the webview. */
    private _postEmptyState() {
        if (this._view) {
            this._view.webview.postMessage({
                command: 'updateDiagnostics',
                data: { state: 'empty' },
            });
        }
    }

    /** Post the diagnostics data (with optional variable watch data) to the webview. */
    private _postDiagnosticsData(data: KnotPassageDiagnosticsResponse, watchData: KnotWatchVariablesResponse | null) {
        if (this._view) {
            this._view.webview.postMessage({
                command: 'updateDiagnostics',
                data: {
                    state: 'loaded',
                    ...data,
                    variable_watch: watchData ? {
                        initialized_at_entry: watchData.initialized_at_entry,
                        written_in_passage: watchData.written_in_passage,
                        read_in_passage: watchData.read_in_passage,
                        potentially_uninitialized: watchData.potentially_uninitialized,
                    } : null,
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
    <title>Knot Passage Diagnostics</title>
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
            --info: var(--vscode-editorInfo-foreground, #3794ff);
            --success: #66bb6a;
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 12px;
            padding: 8px;
        }

        /* ── Empty state ─────────────────────────────────── */
        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 20px 0;
            line-height: 1.6;
        }

        /* ── Passage header ──────────────────────────────── */
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
            display: flex;
            align-items: center;
            gap: 4px;
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
        .badge-reachable { background: #66bb6a; color: #000; }

        .refresh-btn {
            background: none;
            border: 1px solid var(--border);
            color: var(--muted);
            padding: 2px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
        }

        .refresh-btn:hover {
            background: var(--accent);
            color: white;
        }

        /* ── Sections (always visible, no collapsing) ──── */
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
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .section-title .count {
            background: var(--border);
            color: var(--fg);
            padding: 0 5px;
            border-radius: 8px;
            font-size: 9px;
            font-weight: 700;
        }

        .count-error { background: rgba(241, 76, 76, 0.3); color: var(--error); }
        .count-warn { background: rgba(204, 167, 0, 0.3); color: var(--warning); }
        .count-info { background: rgba(55, 148, 255, 0.3); color: var(--info); }
        .count-ok { background: rgba(102, 187, 106, 0.3); color: var(--success); }

        /* ── Diagnostics ─────────────────────────────────── */
        .diag-list { list-style: none; }

        .diag-item {
            padding: 4px 6px;
            border-radius: 3px;
            margin-bottom: 3px;
            display: flex;
            align-items: flex-start;
            gap: 6px;
        }

        .diag-icon {
            flex-shrink: 0;
            font-size: 13px;
            line-height: 1;
            margin-top: 1px;
        }

        .diag-body { flex: 1; min-width: 0; }

        .diag-kind {
            font-weight: 600;
            font-size: 11px;
            color: var(--fg);
        }

        .diag-message {
            font-size: 11px;
            color: var(--muted);
            margin-top: 1px;
            word-break: break-word;
        }

        .diag-error { background: rgba(241, 76, 76, 0.08); }
        .diag-error .diag-icon { color: var(--error); }
        .diag-error .diag-kind { color: var(--error); }

        .diag-warning { background: rgba(204, 167, 0, 0.08); }
        .diag-warning .diag-icon { color: var(--warning); }
        .diag-warning .diag-kind { color: var(--warning); }

        .diag-info { background: rgba(55, 148, 255, 0.08); }
        .diag-info .diag-icon { color: var(--info); }
        .diag-info .diag-kind { color: var(--info); }

        .diag-hint { background: rgba(102, 187, 106, 0.08); }
        .diag-hint .diag-icon { color: var(--success); }
        .diag-hint .diag-kind { color: var(--success); }

        .diag-ok {
            text-align: center;
            color: var(--success);
            padding: 8px 0;
            font-size: 11px;
        }

        /* ── Links ──────────────────────────────────────── */
        .link-list { list-style: none; }

        .link-list li {
            padding: 2px 0;
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .link-item {
            cursor: pointer;
            color: var(--accent);
        }

        .link-item:hover { text-decoration: underline; }

        .link-broken {
            color: var(--error);
            text-decoration: line-through;
        }

        .link-arrow {
            color: var(--muted);
            font-size: 10px;
        }

        /* ── Variable State ────────────────────────────── */
        .var-list { list-style: none; }

        .var-list li {
            padding: 2px 0;
            display: flex;
            align-items: center;
            gap: 4px;
            font-size: 11px;
        }

        .var-name {
            color: var(--fg);
            font-family: var(--vscode-editor-font-family, 'Consolas', monospace);
            font-size: 11px;
        }

        .var-source {
            color: var(--muted);
            font-size: 10px;
        }

        .var-badge {
            display: inline-block;
            padding: 0 4px;
            border-radius: 2px;
            font-size: 9px;
            font-weight: 600;
            margin-left: 2px;
        }

        .var-badge-temp { background: rgba(206, 147, 216, 0.25); color: #ce93d8; }
        .var-badge-uninit { background: rgba(241, 76, 76, 0.25); color: var(--error); }

        .var-section-label {
            font-size: 10px;
            color: var(--muted);
            margin-top: 4px;
            margin-bottom: 2px;
            padding-left: 2px;
        }

        .var-ok {
            text-align: center;
            color: var(--muted);
            padding: 6px 0;
            font-size: 11px;
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Place cursor on a passage to see diagnostics</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        // Event delegation: handle clicks on [data-action] and [data-passage] elements
        document.addEventListener('click', (e) => {
            const actionEl = e.target.closest('[data-action]');
            if (actionEl) {
                const action = actionEl.dataset.action;
                if (action === 'refresh') {
                    vscode.postMessage({ command: 'refresh' });
                }
                return;
            }
            const passageEl = e.target.closest('[data-passage]');
            if (passageEl) {
                vscode.postMessage({ command: 'openPassage', name: passageEl.dataset.passage });
            }
        });

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateDiagnostics') {
                renderDiagnostics(message.data);
            }
        });

        function esc(str) {
            const div = document.createElement('div');
            div.textContent = String(str);
            return div.innerHTML;
        }

        function diagSeverity(kind) {
            const k = kind.toLowerCase();
            if (k.includes('error') || k === 'brokenlink' || k === 'duplicatepassagename' || k === 'duplicatestorydata' || k === 'missingstartpassage' || k === 'unsupportedformat') return 'error';
            if (k.includes('warning') || k === 'invalidpassagename' || k === 'missingstartlink' || k === 'uninitializedvariable') return 'warning';
            if (k.includes('info') || k === 'deadendpassage' || k === 'orphanedpassage') return 'info';
            return 'hint';
        }

        function severityIcon(sev) {
            switch (sev) {
                case 'error': return '\\u2718';
                case 'warning': return '\\u26A0';
                case 'info': return '\\u2139';
                default: return '\\u25CF';
            }
        }

        function renderDiagnostics(data) {
            const content = document.getElementById('content');

            if (data.state === 'empty') {
                content.innerHTML = '<div class="empty-state">Place cursor on a passage to see diagnostics</div>';
                return;
            }

            let html = '';

            // ── Passage overview header ─────────────────
            html += '<div class="passage-header">';
            html += '<span class="passage-name">' + esc(data.passage_name) + '</span>';
            html += '<button class="refresh-btn" data-action="refresh" title="Refresh">\\u21BB</button>';
            html += '</div>';

            // ── Badges row ──────────────────────────────
            html += '<div style="margin-bottom:8px;">';
            if (data.is_metadata) html += '<span class="badge badge-metadata">Metadata</span>';
            else if (data.is_special) html += '<span class="badge badge-special">Special</span>';
            if (data.is_reachable) html += '<span class="badge badge-reachable">Reachable</span>';
            else html += '<span class="badge badge-unreachable">Unreachable</span>';
            html += '</div>';

            // ── Issues (linter output — PRIMARY) ────────
            const diags = data.diagnostics || [];
            const errorCount = diags.filter(x => diagSeverity(x.kind) === 'error').length;
            const warnCount = diags.filter(x => diagSeverity(x.kind) === 'warning').length;
            const infoCount = diags.filter(x => diagSeverity(x.kind) === 'info').length;
            const hintCount = diags.length - errorCount - warnCount - infoCount;

            html += '<div class="section">';
            html += '<div class="section-title">Issues';
            if (errorCount > 0) html += ' <span class="count count-error">' + errorCount + '</span>';
            if (warnCount > 0) html += ' <span class="count count-warn">' + warnCount + '</span>';
            if (infoCount > 0) html += ' <span class="count count-info">' + infoCount + '</span>';
            if (hintCount > 0) html += ' <span class="count count-ok">' + hintCount + '</span>';
            html += '</div>';

            if (diags.length === 0) {
                html += '<div class="diag-ok">No issues found</div>';
            } else {
                html += '<ul class="diag-list">';
                const sorted = [...diags].sort((a, b) => {
                    const order = { error: 0, warning: 1, info: 2, hint: 3 };
                    return (order[diagSeverity(a.kind)] || 3) - (order[diagSeverity(b.kind)] || 3);
                });
                for (const diag of sorted) {
                    const sev = diagSeverity(diag.kind);
                    html += '<li class="diag-item diag-' + sev + '">';
                    html += '<span class="diag-icon">' + severityIcon(sev) + '</span>';
                    html += '<div class="diag-body">';
                    html += '<div class="diag-kind">' + esc(diag.kind) + '</div>';
                    html += '<div class="diag-message">' + esc(diag.message) + '</div>';
                    html += '</div>';
                    html += '</li>';
                }
                html += '</ul>';
            }
            html += '</div>';

            // ── Connections (outgoing + incoming links) ──
            if (data.outgoing_links.length > 0 || data.incoming_links.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Connections';
                const totalLinks = data.outgoing_links.length + data.incoming_links.length;
                html += ' <span class="count">' + totalLinks + '</span>';
                html += '</div>';

                if (data.outgoing_links.length > 0) {
                    html += '<ul class="link-list">';
                    for (const l of data.outgoing_links) {
                        const cls = l.target_exists ? 'link-item' : 'link-item link-broken';
                        const label = l.display_text ? esc(l.display_text) : esc(l.passage_name);
                        html += '<li>';
                        html += '<span class="link-arrow">\\u2192</span> ';
                        html += '<span class="' + cls + '" data-passage="' + esc(l.passage_name) + '">' + label + '</span>';
                        if (l.display_text && l.passage_name !== l.display_text) {
                            html += ' <span style="color:var(--muted);font-size:10px;">\\u2192 ' + esc(l.passage_name) + '</span>';
                        }
                        if (!l.target_exists) html += ' <span style="color:var(--error);font-size:10px;">(broken)</span>';
                        html += '</li>';
                    }
                    html += '</ul>';
                }

                if (data.incoming_links.length > 0) {
                    html += '<div style="margin-top:4px;font-size:10px;color:var(--muted);">From:</div>';
                    html += '<ul class="link-list">';
                    for (const l of data.incoming_links) {
                        html += '<li>';
                        html += '<span class="link-arrow">\\u2190</span> ';
                        html += '<span class="link-item" data-passage="' + esc(l.passage_name) + '">' + esc(l.passage_name) + '</span>';
                        html += '</li>';
                    }
                    html += '</ul>';
                }

                html += '</div>';
            }

            // ── Variable State (from knot/watchVariables) ──
            const vw = data.variable_watch;
            if (vw) {
                const initCount = vw.initialized_at_entry.length;
                const writeCount = vw.written_in_passage.length;
                const readCount = vw.read_in_passage.length;
                const uninitCount = vw.potentially_uninitialized.length;
                const totalVars = initCount + writeCount + readCount + uninitCount;

                html += '<div class="section">';
                html += '<div class="section-title">Variable State';
                html += ' <span class="count">' + totalVars + '</span>';
                html += '</div>';

                if (totalVars === 0) {
                    html += '<div class="var-ok">No variable activity in this passage</div>';
                } else {
                    // Initialized at entry
                    if (initCount > 0) {
                        html += '<div class="var-section-label">Initialized at entry:</div>';
                        html += '<ul class="var-list">';
                        for (const v of vw.initialized_at_entry) {
                            html += '<li>';
                            html += '<span class="var-name">' + esc(v.name) + '</span>';
                            if (v.is_temporary) html += '<span class="var-badge var-badge-temp">temp</span>';
                            if (v.last_written_in) html += '<span class="var-source">from ' + esc(v.last_written_in) + '</span>';
                            html += '</li>';
                        }
                        html += '</ul>';
                    }

                    // Written in passage
                    if (writeCount > 0) {
                        html += '<div class="var-section-label">Written here:</div>';
                        html += '<ul class="var-list">';
                        for (const v of vw.written_in_passage) {
                            html += '<li>';
                            html += '<span class="var-name">' + esc(v.name) + '</span>';
                            if (v.is_temporary) html += '<span class="var-badge var-badge-temp">temp</span>';
                            html += '</li>';
                        }
                        html += '</ul>';
                    }

                    // Read in passage
                    if (readCount > 0) {
                        html += '<div class="var-section-label">Read here:</div>';
                        html += '<ul class="var-list">';
                        for (const v of vw.read_in_passage) {
                            html += '<li>';
                            html += '<span class="var-name">' + esc(v.name) + '</span>';
                            if (v.is_temporary) html += '<span class="var-badge var-badge-temp">temp</span>';
                            if (v.last_written_in) html += '<span class="var-source">from ' + esc(v.last_written_in) + '</span>';
                            html += '</li>';
                        }
                        html += '</ul>';
                    }

                    // Potentially uninitialized
                    if (uninitCount > 0) {
                        html += '<div class="var-section-label">Potentially uninitialized:</div>';
                        html += '<ul class="var-list">';
                        for (const v of vw.potentially_uninitialized) {
                            html += '<li>';
                            html += '<span class="var-name">' + esc(v.name) + '</span>';
                            html += '<span class="var-badge var-badge-uninit">uninit</span>';
                            html += '</li>';
                        }
                        html += '</ul>';
                    }
                }

                html += '</div>';
            }

            content.innerHTML = html;
        }
    </script>
</body>
</html>`;
    }
}
