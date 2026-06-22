//! Project Info view provider for the Knot extension.
//!
//! Displays a clean, focused summary of the Twine project: story identity,
//! scale (passages, words, play time), writing health, and actionable issues.
//! Avoids graph-theory clutter — the Story Map handles structural visualization.

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotProfileResponse } from './types';

// ---------------------------------------------------------------------------
// Project Info webview provider
// ---------------------------------------------------------------------------

export class ProfileViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.profileView';

    private _view?: vscode.WebviewView;
    private _client: KnotLanguageClient | null = null;
    private _refreshDebounceTimer: ReturnType<typeof setTimeout> | null = null;
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

    /** Start polling until the server is ready and profile data is fetched. */
    private _startInitialPolling() {
        this._initialRetryCount = 0;
        this._pollInitial();
    }

    private async _pollInitial() {
        if (this._initialRetryCount >= ProfileViewProvider.MAX_INITIAL_RETRIES) {
            return;
        }
        this._initialRetryCount++;
        const clientReady = this._client && this._client.isRunning();
        const viewReady = !!this._view;
        if (clientReady && viewReady) {
            const gotData = await this._fetchAndPost();
            if (!gotData) {
                this._initialRetryTimer = setTimeout(() => this._pollInitial(), ProfileViewProvider.INITIAL_RETRY_MS);
            }
        } else {
            this._initialRetryTimer = setTimeout(() => this._pollInitial(), ProfileViewProvider.INITIAL_RETRY_MS);
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
            const result = await this._client.sendRequest<KnotProfileResponse>('knot/profile', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

            if (this._view) {
                this._view.webview.postMessage({
                    command: 'updateProfile',
                    data: result,
                });
            }
            return true;
        } catch (e) {
            console.error('[Knot] Failed to fetch project info:', e);
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
            }
        });

        if (this._client) {
            this._startInitialPolling();
        }

        webviewView.onDidChangeVisibility(() => {
            if (webviewView.visible) {
                this.refresh();
            }
        });
    }

    /** Schedule a debounced refresh. */
    private _scheduleRefresh(delayMs: number) {
        if (this._refreshDebounceTimer) {
            clearTimeout(this._refreshDebounceTimer);
        }
        this._refreshDebounceTimer = setTimeout(() => {
            this._refreshDebounceTimer = null;
            this.refresh();
        }, delayMs);
    }

    /** Refresh profile data from the language server (debounced). */
    public refresh() {
        if (this._refreshDebounceTimer) {
            clearTimeout(this._refreshDebounceTimer);
        }
        this._refreshDebounceTimer = setTimeout(() => {
            this._refreshDebounceTimer = null;
            this._fetchAndPost();
        }, 500);
    }

    /** Clean up pending timers. */
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

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(webview: vscode.Webview): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline' https:; img-src 'self' data:; connect-src 'self';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Project Info</title>
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

        .toolbar {
            display: flex;
            justify-content: flex-end;
            margin-bottom: 8px;
        }

        .toolbar button {
            background: var(--card);
            border: 1px solid var(--border);
            color: var(--fg);
            padding: 3px 10px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 11px;
        }

        .toolbar button:hover {
            background: var(--accent);
            color: white;
        }

        /* ── Story header ─────────────────────────────────────────── */
        .story-header {
            margin-bottom: 12px;
            padding-bottom: 10px;
            border-bottom: 1px solid var(--border);
        }

        .story-name {
            font-size: 15px;
            font-weight: 700;
            line-height: 1.3;
            margin-bottom: 4px;
        }

        .story-meta {
            display: flex;
            align-items: center;
            gap: 6px;
            flex-wrap: wrap;
        }

        .format-badge {
            background: var(--accent);
            color: white;
            padding: 2px 8px;
            border-radius: 10px;
            font-size: 10px;
            font-weight: 600;
        }

        .format-version {
            color: var(--muted);
            font-size: 10px;
        }

        .ifid {
            color: var(--muted);
            font-size: 9px;
            font-family: var(--vscode-editor-font-family, monospace);
            margin-top: 4px;
            word-break: break-all;
        }

        .no-story-data {
            color: var(--warning);
            font-size: 11px;
            margin-top: 4px;
        }

        /* ── Stat rows ────────────────────────────────────────────── */
        .section {
            margin-bottom: 12px;
        }

        .section-title {
            font-weight: 600;
            font-size: 10px;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 6px;
        }

        .stat-row {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 3px 0;
        }

        .stat-label {
            color: var(--fg);
            font-size: 11px;
        }

        .stat-value {
            font-weight: 600;
            font-size: 12px;
        }

        .stat-value.muted { color: var(--muted); font-weight: 400; }
        .stat-value.error { color: var(--error); }
        .stat-value.warning { color: var(--warning); }
        .stat-value.success { color: var(--success); }

        /* ── Big stat cards ───────────────────────────────────────── */
        .big-stats {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 6px;
            margin-bottom: 12px;
        }

        .big-stat {
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 6px;
            padding: 8px 10px;
            text-align: center;
        }

        .big-stat-value {
            font-size: 20px;
            font-weight: 700;
            line-height: 1.2;
        }

        .big-stat-label {
            font-size: 9px;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.03em;
            margin-top: 2px;
        }

        /* ── Issues list ──────────────────────────────────────────── */
        .issue-item {
            display: flex;
            align-items: center;
            gap: 6px;
            padding: 3px 0;
        }

        .issue-dot {
            width: 8px;
            height: 8px;
            border-radius: 50%;
            flex-shrink: 0;
        }

        .issue-dot.error { background: var(--error); }
        .issue-dot.warning { background: var(--warning); }
        .issue-dot.ok { background: var(--success); }

        .issue-text {
            flex: 1;
            font-size: 11px;
        }

        .issue-count {
            font-weight: 600;
            font-size: 11px;
        }

        /* ── Tag list ─────────────────────────────────────────────── */
        .tag-row {
            display: flex;
            align-items: center;
            gap: 6px;
            padding: 2px 0;
        }

        .tag-name {
            font-size: 11px;
            color: var(--fg);
            min-width: 60px;
        }

        .tag-bar-track {
            flex: 1;
            height: 10px;
            background: var(--card);
            border-radius: 3px;
            overflow: hidden;
        }

        .tag-bar-fill {
            height: 100%;
            border-radius: 3px;
            min-width: 2px;
        }

        .tag-count {
            font-size: 10px;
            color: var(--muted);
            min-width: 20px;
            text-align: right;
        }

        /* ── Empty state ──────────────────────────────────────────── */
        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 20px 0;
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Loading project info...</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateProfile') {
                if (message.error || !message.data) {
                    renderError(message.error || 'No data received from server');
                } else {
                    renderProfile(message.data);
                }
            }
        });

        function esc(str) {
            const div = document.createElement('div');
            div.textContent = String(str);
            return div.innerHTML;
        }

        function renderError(errMsg) {
            const content = document.getElementById('content');
            let html = '';
            html += '<div class="toolbar">';
            html += '<button onclick="vscode.postMessage({command: \\'refresh\\'})">Retry</button>';
            html += '</div>';
            html += '<div class="empty-state">';
            html += '<div style="color:var(--warning);margin-bottom:8px;">Unable to load project info</div>';
            html += '<div style="font-size:10px;color:var(--muted);">' + esc(errMsg) + '</div>';
            html += '</div>';
            content.innerHTML = html;
        }

        function renderProfile(d) {
            const content = document.getElementById('content');
            let html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button onclick="vscode.postMessage({command: \\'refresh\\'})" title="Refresh">&#x21BB;</button>';
            html += '</div>';

            // ── Story header ─────────────────────────────────────────
            html += '<div class="story-header">';
            if (d.story_name) {
                html += '<div class="story-name">' + esc(d.story_name) + '</div>';
            } else {
                html += '<div class="story-name" style="color:var(--muted);font-style:italic;">Untitled Story</div>';
            }
            html += '<div class="story-meta">';
            html += '<span class="format-badge">' + esc(d.format) + '</span>';
            if (d.format_version) {
                html += '<span class="format-version">v' + esc(d.format_version) + '</span>';
            }
            html += '</div>';
            if (d.ifid) {
                html += '<div class="ifid">IFID: ' + esc(d.ifid) + '</div>';
            }
            if (!d.has_story_data) {
                html += '<div class="no-story-data">No StoryData passage found</div>';
            }
            html += '</div>';

            // ── Big stats: passages, words ───────────────────────────
            html += '<div class="big-stats">';
            html += '<div class="big-stat"><div class="big-stat-value">' + d.passage_count + '</div><div class="big-stat-label">Passages</div></div>';
            html += '<div class="big-stat"><div class="big-stat-value">' + formatWords(d.total_word_count) + '</div><div class="big-stat-label">Words</div></div>';
            html += '</div>';

            // ── Scale ────────────────────────────────────────────────
            html += '<div class="section">';
            html += '<div class="section-title">Scale</div>';
            html += statRow('Documents', d.document_count);
            html += statRow('Links', d.total_links);
            html += statRow('Special passages', d.special_passage_count);
            html += statRow('Metadata passages', d.metadata_passage_count);
            html += statRow('Estimated play time', estPlayTime(d.total_word_count));
            html += '</div>';

            // ── Writing health ───────────────────────────────────────
            if (d.complexity_metrics) {
                html += '<div class="section">';
                html += '<div class="section-title">Writing</div>';
                html += statRow('Avg words / passage', Math.round(d.complexity_metrics.avg_word_count));
                html += statRow('Median words / passage', Math.round(d.complexity_metrics.median_word_count));
                html += statRow('Longest passage', d.complexity_metrics.max_word_count + ' words');
                if (d.complexity_metrics.min_word_count > 0) {
                    html += statRow('Shortest passage', d.complexity_metrics.min_word_count + ' words');
                }
                html += '</div>';
            }

            // ── Issues ───────────────────────────────────────────────
            html += '<div class="section">';
            html += '<div class="section-title">Issues</div>';
            html += issueRow('Broken links', d.broken_link_count, 'error');
            html += issueRow('Unreachable passages', d.unreachable_passage_count, 'warning');
            html += issueRow('Dead-end passages', d.dead_end_count, 'warning');
            html += issueRow('Unused variables', d.variable_issue_count, 'warning');
            html += '</div>';

            // ── Variables ────────────────────────────────────────────
            html += '<div class="section">';
            html += '<div class="section-title">Variables</div>';
            html += statRow('Tracked variables', d.variable_count);
            html += '</div>';

            // ── Tags ─────────────────────────────────────────────────
            if (d.tag_stats && d.tag_stats.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Tags</div>';
                const maxTag = Math.max(...d.tag_stats.map(t => t.passage_count), 1);
                const tagColors = ['#4fc3f7', '#81c784', '#ffb74d', '#ce93d8', '#ef9a9a', '#80cbc4', '#ffab91', '#a5d6a7'];
                for (let i = 0; i < d.tag_stats.length; i++) {
                    const t = d.tag_stats[i];
                    const pct = (t.passage_count / maxTag * 100);
                    const color = tagColors[i % tagColors.length];
                    html += '<div class="tag-row">';
                    html += '<span class="tag-name">' + esc(t.tag) + '</span>';
                    html += '<div class="tag-bar-track"><div class="tag-bar-fill" style="width:' + pct + '%;background:' + color + '"></div></div>';
                    html += '<span class="tag-count">' + t.passage_count + '</span>';
                    html += '</div>';
                }
                html += '</div>';
            }

            content.innerHTML = html;
        }

        function statRow(label, value, cls) {
            const c = cls ? ' ' + cls : '';
            return '<div class="stat-row"><span class="stat-label">' + label + '</span><span class="stat-value' + c + '">' + value + '</span></div>';
        }

        function issueRow(label, count, severity) {
            const dotClass = count > 0 ? severity : 'ok';
            const valClass = count > 0 ? severity : 'success';
            return '<div class="issue-item">' +
                '<span class="issue-dot ' + dotClass + '"></span>' +
                '<span class="issue-text">' + label + '</span>' +
                '<span class="issue-count ' + valClass + '">' + (count > 0 ? count : 'None') + '</span>' +
                '</div>';
        }

        function formatWords(n) {
            if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
            return n.toString();
        }

        function estPlayTime(words) {
            // Average reading speed: ~200 words/minute for interactive fiction
            const minutes = words / 200;
            if (minutes < 1) return '< 1 min';
            if (minutes < 60) return Math.round(minutes) + ' min';
            const hours = Math.floor(minutes / 60);
            const mins = Math.round(minutes % 60);
            return hours + 'h ' + mins + 'm';
        }
    </script>
</body>
</html>`;
    }
}
