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

import * as vscode from 'vscode';
import {
    KnotLanguageClient,
    KnotPassageDiagnosticsResponse,
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
                case 'openPassageAtLine': {
                    // Used by the Temporary Variables section to jump to a
                    // specific read/write line within the passage.
                    const { name, line, spanStart, spanEnd } = message;
                    if (name && typeof line === 'number') {
                        await vscode.commands.executeCommand('knot.openPassageByName', name, line, spanStart, spanEnd);
                    }
                    break;
                }
                case 'refresh': {
                    await this.refresh();
                    break;
                }
            }
        });

        // Re-fetch data when the view becomes visible again (e.g. after
        // the sidebar was collapsed and re-expanded). Without this, the
        // webview shows stale or empty content because the DOM was reset.
        //
        // If no passage is currently selected, try to auto-detect one from
        // the active editor. This prevents the "empty state" from showing
        // when the user opens the sidebar — the view should always show
        // SOMETHING useful (either the passage under the cursor, or the
        // Start passage as a fallback).
        webviewView.onDidChangeVisibility(() => {
            if (webviewView.visible) {
                if (!this._currentPassage) {
                    this._autoDetectPassage();
                }
                this.refresh();
            }
        });

        // Also try to auto-detect on initial resolve — the view might
        // become visible before any onDidChangeActiveTextEditor fires.
        if (!this._currentPassage) {
            this._autoDetectPassage();
        }
    }

    /**
     * Try to detect the current passage from the active text editor.
     * If no twee editor is active, fall back to the "Start" passage
     * (the most useful default — it's where the story begins).
     *
     * This is called when the view becomes visible and no passage is
     * currently selected, preventing the empty state from showing
     * when there's a project open.
     */
    private _autoDetectPassage(): void {
        const editor = vscode.window.activeTextEditor;
        if (editor) {
            const text = editor.document.getText();
            const position = editor.selection.active;
            const lines = text.split('\n');
            let currentPassage: string | undefined;
            for (let i = 0; i <= position.line; i++) {
                const line = lines[i];
                if (line.startsWith('::')) {
                    // Extract passage name from the header line
                    let name = line.replace(/^::\s*/, '');
                    // Strip JSON metadata blocks {...}
                    const braceStart = name.indexOf('{');
                    if (braceStart >= 0) {
                        name = name.substring(0, braceStart);
                    }
                    // Strip tag blocks [...]
                    const bracketStart = name.indexOf('[');
                    if (bracketStart >= 0) {
                        name = name.substring(0, bracketStart);
                    }
                    currentPassage = name.trim();
                }
            }
            if (currentPassage) {
                this._currentPassage = currentPassage;
                return;
            }
        }

        // Fall back to "Start" — it's the most useful default since
        // it's where the story begins and is always present in a
        // well-formed Twine project.
        this._currentPassage = 'Start';
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
            const result = await this._client.sendRequest<KnotPassageDiagnosticsResponse>('knot/passageDiagnostics', {
                workspace_uri: workspaceFolders[0].uri.toString(),
                passage_name: this._currentPassage,
            });

            this._postDiagnosticsData(result);
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

    /** Post the diagnostics data to the webview. */
    private _postDiagnosticsData(data: KnotPassageDiagnosticsResponse) {
        if (this._view) {
            this._view.webview.postMessage({
                command: 'updateDiagnostics',
                data: { state: 'loaded', ...data },
            });
        }
    }

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(_webview: vscode.Webview): string {
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
        .badge-unreachable { background: #e65100; color: #fff; }
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

        /* ── Temporary variables (passage-scoped _var infographics) ── */
        .temp-list { list-style: none; }

        .temp-item {
            padding: 4px 6px;
            border-radius: 3px;
            margin-bottom: 3px;
            background: rgba(55, 148, 255, 0.05);
            border-left: 2px solid var(--info);
        }

        .temp-head {
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 6px;
        }

        .temp-name {
            font-family: var(--vscode-editor-font-family, 'Consolas', 'Courier New', monospace);
            font-size: 11px;
            color: var(--fg);
            font-weight: 600;
        }

        .temp-counts {
            display: flex;
            gap: 3px;
            flex-shrink: 0;
        }

        .temp-count {
            display: inline-flex;
            align-items: center;
            gap: 2px;
            padding: 0 5px;
            border-radius: 8px;
            font-size: 9px;
            font-weight: 700;
            line-height: 14px;
        }

        .temp-count-w { background: rgba(204, 167, 0, 0.25); color: var(--warning); }
        .temp-count-r { background: rgba(55, 148, 255, 0.25); color: var(--info); }

        /* Mini bar showing write vs read proportion */
        .temp-bar {
            display: flex;
            height: 3px;
            border-radius: 2px;
            overflow: hidden;
            margin-top: 4px;
            background: var(--border);
        }

        .temp-bar-w { background: var(--warning); }
        .temp-bar-r { background: var(--info); }

        .temp-refs {
            list-style: none;
            margin-top: 3px;
            padding-left: 8px;
        }

        .temp-refs li {
            font-size: 10px;
            color: var(--muted);
            display: flex;
            align-items: center;
            gap: 4px;
            padding: 1px 0;
        }

        .temp-ref-link {
            color: var(--accent);
            cursor: pointer;
            font-family: var(--vscode-editor-font-family, 'Consolas', 'Courier New', monospace);
        }

        .temp-ref-link:hover { text-decoration: underline; }

        .temp-ref-kind {
            font-size: 9px;
            padding: 0 4px;
            border-radius: 3px;
            font-weight: 600;
        }

        .temp-ref-kind-w { background: rgba(204, 167, 0, 0.2); color: var(--warning); }
        .temp-ref-kind-r { background: rgba(55, 148, 255, 0.2); color: var(--info); }

        .temp-empty {
            font-size: 11px;
            color: var(--muted);
            font-style: italic;
            padding: 4px 0;
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Place cursor on a passage to see diagnostics</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

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
            if (k.includes('warning') || k === 'invalidpassagename' || k === 'missingstartlink' || k === 'uninitializedvariable' || k === 'unreachablepassage') return 'warning';
            if (k.includes('info') || k === 'deadendpassage') return 'info';
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

        function openPassageCmd(name) {
            return "vscode.postMessage({command: 'openPassage', name: '" + esc(name).replace(/'/g, "\\'") + "'})";
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
            html += '<button class="refresh-btn" onclick="vscode.postMessage({command: \\'refresh\\'})" title="Refresh">\\u21BB</button>';
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
                        html += '<span class="' + cls + '" onclick="' + openPassageCmd(l.passage_name) + '">' + label + '</span>';
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
                        html += '<span class="link-item" onclick="' + openPassageCmd(l.passage_name) + '">' + esc(l.passage_name) + '</span>';
                        html += '</li>';
                    }
                    html += '</ul>';
                }

                html += '</div>';
            }

            // ── Temporary variables (passage-scoped _var infographics) ──
            html += renderTempVariables(data);

            content.innerHTML = html;
        }

        function renderTempVariables(data) {
            const temps = data.temporary_variables || [];
            if (temps.length === 0) {
                // Hide the section entirely when there are no temps — this
                // also covers formats without passage-scoped temp vars
                // (Harlowe, Snowman, Chapbook) which return an empty list.
                return '';
            }

            let html = '<div class="section">';
            html += '<div class="section-title">Temporary Variables';
            html += ' <span class="count">' + temps.length + '</span>';
            html += '</div>';

            html += '<ul class="temp-list">';
            for (const t of temps) {
                const total = t.write_count + t.read_count;
                const wPct = total > 0 ? (t.write_count / total) * 100 : 0;
                const rPct = total > 0 ? (t.read_count / total) * 100 : 0;

                html += '<li class="temp-item">';
                html += '<div class="temp-head">';
                html += '<span class="temp-name">' + esc(t.name) + '</span>';
                html += '<span class="temp-counts">';
                html += '<span class="temp-count temp-count-w" title="writes">W ' + t.write_count + '</span>';
                html += '<span class="temp-count temp-count-r" title="reads">R ' + t.read_count + '</span>';
                html += '</span>';
                html += '</div>';

                // Mini proportion bar (writes vs reads). Hidden when no
                // accesses at all (defensive — shouldn't happen since we
                // only emit a summary when refs is non-empty).
                if (total > 0) {
                    html += '<div class="temp-bar">';
                    html += '<div class="temp-bar-w" style="width:' + wPct + '%;"></div>';
                    html += '<div class="temp-bar-r" style="width:' + rPct + '%;"></div>';
                    html += '</div>';
                }

                // Line-level refs (clickable). Show up to 8 to keep the
                // panel compact; the rest are still accessible by scrolling
                // the passage once the user clicks any ref.
                const refs = t.references || [];
                if (refs.length > 0) {
                    html += '<ul class="temp-refs">';
                    const shown = refs.slice(0, 8);
                    for (const r of shown) {
                        const lineLabel = 'L' + (r.line + 1); // 0-based → 1-based
                        const kindCls = r.is_write ? 'temp-ref-kind-w' : 'temp-ref-kind-r';
                        const kindLabel = r.is_write ? 'W' : 'R';
                        const fullName = r.variable_name || t.name;
                        // Passages, line numbers, and span data travel via
                        // the message handler. The span enables precise
                        // range-based selection in the editor.
                        var spanArg = '';
                        if (typeof r.span_start === 'number' && typeof r.span_end === 'number') {
                            spanArg = ', spanStart: ' + r.span_start + ', spanEnd: ' + r.span_end;
                        }
                        const onClick = "vscode.postMessage({command: 'openPassageAtLine', name: '" +
                            esc(r.passage_name || data.passage_name).replace(/'/g, "\\'") +
                            "', line: " + r.line + spanArg + "})";
                        html += '<li>';
                        html += '<span class="temp-ref-kind ' + kindCls + '">' + kindLabel + '</span>';
                        html += '<span class="temp-ref-link" onclick="' + onClick + '" title="Open ' + esc(fullName) + ' at line ' + (r.line + 1) + '">';
                        html += esc(lineLabel) + ' ' + esc(fullName);
                        html += '</span>';
                        html += '</li>';
                    }
                    if (refs.length > shown.length) {
                        html += '<li style="color:var(--muted);font-size:9px;font-style:italic;">+' + (refs.length - shown.length) + ' more…</li>';
                    }
                    html += '</ul>';
                }

                html += '</li>';
            }
            html += '</ul>';
            html += '</div>';

            return html;
        }
    </script>
</body>
</html>`;
    }
}
