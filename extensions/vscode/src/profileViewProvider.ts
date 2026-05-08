//! Profile View provider for the Knot extension.
//!
//! This module implements a VS Code sidebar webview panel that displays
//! workspace profiling statistics including passage counts, link density,
//! variable metrics, and graph analysis results.

import * as vscode from 'vscode';

// ---------------------------------------------------------------------------
// Profile data types (matches Rust-side KnotProfileResponse)
// ---------------------------------------------------------------------------

interface KnotLinkDistribution {
    zero_links: number;
    few_links: number;
    moderate_links: number;
    many_links: number;
}

interface KnotTagStat {
    tag: string;
    passage_count: number;
    avg_word_count: number;
    total_word_count: number;
    avg_out_links: number;
}

interface KnotComplexityMetrics {
    avg_word_count: number;
    median_word_count: number;
    max_word_count: number;
    min_word_count: number;
    avg_out_links: number;
    out_links_stddev: number;
    complex_passage_count: number;
}

interface KnotStructuralBalance {
    dead_end_ratio: number;
    orphaned_ratio: number;
    is_well_connected: boolean;
    connected_components: number;
    diameter: number;
    avg_clustering: number;
}

interface KnotProfileResponse {
    document_count: number;
    passage_count: number;
    special_passage_count: number;
    metadata_passage_count: number;
    unreachable_passage_count: number;
    broken_link_count: number;
    infinite_loop_count: number;
    total_links: number;
    avg_out_degree: number;
    avg_in_degree: number;
    max_depth: number;
    dead_end_count: number;
    variable_count: number;
    variable_issue_count: number;
    format: string;
    format_version: string | null;
    has_story_data: boolean;
    total_word_count: number;
    link_distribution: KnotLinkDistribution;
    tag_stats: KnotTagStat[];
    complexity_metrics: KnotComplexityMetrics;
    structural_balance: KnotStructuralBalance;
}

// ---------------------------------------------------------------------------
// Profile View webview provider
// ---------------------------------------------------------------------------

export class ProfileViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.profileView';

    private _view?: vscode.WebviewView;
    private _client: any;

    constructor(private readonly _extensionUri: vscode.Uri) {}

    /** Set the language client reference. */
    public setClient(client: any) {
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
            }
        });

        if (this._client) {
            this.refresh();
        }
    }

    /** Refresh profile data from the language server. */
    public async refresh() {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result: KnotProfileResponse = await this._client.sendRequest('knot/profile', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

            if (this._view) {
                this._view.webview.postMessage({
                    command: 'updateProfile',
                    data: result,
                });
            }
        } catch (e) {
            console.error('[Knot] Failed to fetch profile:', e);
        }
    }

    /** Generate the HTML for the webview. */
    private _getHtmlForWebview(webview: vscode.Webview): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Profile</title>
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

        .section {
            margin-bottom: 10px;
        }

        .section-title {
            font-weight: 600;
            font-size: 11px;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            margin-bottom: 6px;
            padding-bottom: 3px;
            border-bottom: 1px solid var(--border);
        }

        .stat-grid {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 4px;
        }

        .stat-card {
            background: var(--card);
            border: 1px solid var(--border);
            border-radius: 4px;
            padding: 6px 8px;
        }

        .stat-value {
            font-size: 18px;
            font-weight: 700;
            line-height: 1.2;
        }

        .stat-label {
            font-size: 10px;
            color: var(--muted);
        }

        .stat-card.error .stat-value { color: var(--error); }
        .stat-card.warning .stat-value { color: var(--warning); }
        .stat-card.success .stat-value { color: var(--success); }

        .format-badge {
            display: inline-block;
            background: var(--accent);
            color: white;
            padding: 3px 10px;
            border-radius: 12px;
            font-size: 12px;
            font-weight: 500;
        }

        .format-version {
            color: var(--muted);
            font-size: 11px;
            margin-left: 6px;
        }

        .bar-chart {
            display: flex;
            flex-direction: column;
            gap: 4px;
        }

        .bar-row {
            display: flex;
            align-items: center;
            gap: 6px;
        }

        .bar-label {
            width: 70px;
            font-size: 10px;
            color: var(--muted);
            text-align: right;
        }

        .bar-track {
            flex: 1;
            height: 12px;
            background: var(--card);
            border-radius: 3px;
            overflow: hidden;
        }

        .bar-fill {
            height: 100%;
            border-radius: 3px;
            transition: width 0.3s ease;
        }

        .bar-count {
            font-size: 10px;
            color: var(--muted);
            min-width: 24px;
        }

        .issue-row {
            display: flex;
            justify-content: space-between;
            padding: 2px 0;
        }

        .issue-label { color: var(--fg); }
        .issue-count { font-weight: 600; }
        .issue-count.error { color: var(--error); }
        .issue-count.warning { color: var(--warning); }

        .empty-state {
            text-align: center;
            color: var(--muted);
            padding: 20px 0;
        }
    </style>
</head>
<body>
    <div id="content">
        <div class="empty-state">Loading workspace profile...</div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        window.addEventListener('message', (event) => {
            const message = event.data;
            if (message.command === 'updateProfile') {
                renderProfile(message.data);
            }
        });

        function esc(str) {
            const div = document.createElement('div');
            div.textContent = str;
            return div.innerHTML;
        }

        function renderProfile(d) {
            const content = document.getElementById('content');
            let html = '';

            // Toolbar
            html += '<div class="toolbar">';
            html += '<button onclick="vscode.postMessage({command: \'refresh\'})">Refresh</button>';
            html += '</div>';

            // Format info
            html += '<div class="section">';
            html += '<div class="section-title">Format</div>';
            html += '<span class="format-badge">' + esc(d.format) + '</span>';
            if (d.format_version) html += '<span class="format-version">v' + esc(d.format_version) + '</span>';
            if (!d.has_story_data) html += ' <span style="color:var(--warning);font-size:11px;">(No StoryData)</span>';
            html += '</div>';

            // Overview stats
            html += '<div class="section">';
            html += '<div class="section-title">Overview</div>';
            html += '<div class="stat-grid">';
            html += makeCard(d.passage_count, 'Passages');
            html += makeCard(d.document_count, 'Documents');
            html += makeCard(d.total_links, 'Links');
            html += makeCard(d.total_word_count, 'Words');
            html += makeCard(d.special_passage_count, 'Special');
            html += makeCard(d.metadata_passage_count, 'Metadata');
            html += '</div></div>';

            // Graph health
            html += '<div class="section">';
            html += '<div class="section-title">Graph Health</div>';
            html += '<div class="stat-grid">';
            html += makeCard(d.unreachable_passage_count, 'Unreachable', d.unreachable_passage_count > 0 ? 'warning' : 'success');
            html += makeCard(d.broken_link_count, 'Broken Links', d.broken_link_count > 0 ? 'error' : 'success');
            html += makeCard(d.infinite_loop_count, 'Loops', d.infinite_loop_count > 0 ? 'warning' : 'success');
            html += makeCard(d.dead_end_count, 'Dead Ends', d.dead_end_count > 0 ? 'warning' : 'success');
            html += '</div>';

            // Graph metrics
            html += '<div class="issue-row"><span class="issue-label">Avg outgoing links</span><span class="issue-count">' + d.avg_out_degree.toFixed(1) + '</span></div>';
            html += '<div class="issue-row"><span class="issue-label">Avg incoming links</span><span class="issue-count">' + d.avg_in_degree.toFixed(1) + '</span></div>';
            html += '<div class="issue-row"><span class="issue-label">Max depth from start</span><span class="issue-count">' + d.max_depth + '</span></div>';
            html += '</div>';

            // Variable stats
            html += '<div class="section">';
            html += '<div class="section-title">Variables</div>';
            html += '<div class="stat-grid">';
            html += makeCard(d.variable_count, 'Variables');
            html += makeCard(d.variable_issue_count, 'Issues', d.variable_issue_count > 0 ? 'warning' : 'success');
            html += '</div></div>';

            // Link distribution
            html += '<div class="section">';
            html += '<div class="section-title">Link Distribution</div>';
            html += '<div class="bar-chart">';
            const maxBar = Math.max(d.link_distribution.zero_links, d.link_distribution.few_links, d.link_distribution.moderate_links, d.link_distribution.many_links, 1);
            html += makeBar('0 links', d.link_distribution.zero_links, maxBar, '#666');
            html += makeBar('1-2 links', d.link_distribution.few_links, maxBar, '#4fc3f7');
            html += makeBar('3-5 links', d.link_distribution.moderate_links, maxBar, '#ffb74d');
            html += makeBar('6+ links', d.link_distribution.many_links, maxBar, '#ce93d8');
            html += '</div></div>';

            // Complexity metrics
            if (d.complexity_metrics) {
                html += '<div class="section">';
                html += '<div class="section-title">Passage Complexity</div>';
                html += '<div class="issue-row"><span class="issue-label">Avg words/passage</span><span class="issue-count">' + d.complexity_metrics.avg_word_count.toFixed(0) + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Median words/passage</span><span class="issue-count">' + d.complexity_metrics.median_word_count.toFixed(0) + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Max words (single passage)</span><span class="issue-count">' + d.complexity_metrics.max_word_count + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Min words (non-empty)</span><span class="issue-count">' + d.complexity_metrics.min_word_count + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Avg outgoing links</span><span class="issue-count">' + d.complexity_metrics.avg_out_links.toFixed(1) + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Link count std dev</span><span class="issue-count">' + d.complexity_metrics.out_links_stddev.toFixed(2) + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Complex passages (6+ links)</span><span class="issue-count ' + (d.complexity_metrics.complex_passage_count > 0 ? 'warning' : '') + '">' + d.complexity_metrics.complex_passage_count + '</span></div>';
                html += '</div>';
            }

            // Structural balance
            if (d.structural_balance) {
                html += '<div class="section">';
                html += '<div class="section-title">Structural Balance</div>';
                const connColor = d.structural_balance.is_well_connected ? 'success' : 'warning';
                html += '<div class="stat-grid">';
                html += makeCard((d.structural_balance.dead_end_ratio * 100).toFixed(0) + '%', 'Dead-end ratio', d.structural_balance.dead_end_ratio > 0.3 ? 'warning' : 'success');
                html += makeCard((d.structural_balance.orphaned_ratio * 100).toFixed(0) + '%', 'Orphan ratio', d.structural_balance.orphaned_ratio > 0.3 ? 'warning' : 'success');
                html += makeCard(d.structural_balance.connected_components, 'Components', d.structural_balance.connected_components > 1 ? 'warning' : connColor);
                html += makeCard(d.structural_balance.diameter, 'Diameter');
                html += '</div>';
                html += '<div class="issue-row"><span class="issue-label">Avg clustering coeff</span><span class="issue-count">' + d.structural_balance.avg_clustering.toFixed(3) + '</span></div>';
                html += '<div class="issue-row"><span class="issue-label">Well connected</span><span class="issue-count ' + connColor + '">' + (d.structural_balance.is_well_connected ? 'Yes' : 'No') + '</span></div>';
                html += '</div>';
            }

            // Tag statistics
            if (d.tag_stats && d.tag_stats.length > 0) {
                html += '<div class="section">';
                html += '<div class="section-title">Tags</div>';
                html += '<div class="bar-chart">';
                const maxTagCount = Math.max(...d.tag_stats.map(t => t.passage_count), 1);
                const tagColors = ['#4fc3f7', '#81c784', '#ffb74d', '#ce93d8', '#ef9a9a', '#80cbc4', '#ffab91', '#a5d6a7'];
                for (let i = 0; i < d.tag_stats.length; i++) {
                    const t = d.tag_stats[i];
                    const color = tagColors[i % tagColors.length];
                    html += makeBar(t.tag, t.passage_count, maxTagCount, color);
                    html += '<div style="margin-left:76px;font-size:9px;color:var(--muted);margin-top:-2px;">' + t.avg_word_count.toFixed(0) + ' avg words, ' + t.avg_out_links.toFixed(1) + ' avg links</div>';
                }
                html += '</div></div>';
            }

            content.innerHTML = html;
        }

        function makeCard(value, label, colorClass) {
            const cls = colorClass ? 'stat-card ' + colorClass : 'stat-card';
            return '<div class="' + cls + '"><div class="stat-value">' + value + '</div><div class="stat-label">' + label + '</div></div>';
        }

        function makeBar(label, count, max, color) {
            const pct = max > 0 ? (count / max * 100) : 0;
            return '<div class="bar-row">' +
                '<span class="bar-label">' + label + '</span>' +
                '<div class="bar-track"><div class="bar-fill" style="width:' + pct + '%;background:' + color + '"></div></div>' +
                '<span class="bar-count">' + count + '</span></div>';
        }
    </script>
</body>
</html>`;
    }
}
