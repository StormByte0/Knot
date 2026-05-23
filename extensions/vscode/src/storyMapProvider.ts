//! Story Map webview provider for the Knot extension.
//!
//! This module implements a VS Code webview panel that renders an interactive
//! passage graph using Cytoscape.js, inspired by the Twine 2 story editor.
//!
//! The graph UI is built as a React + Vite application located in
//! `extensions/vscode/webview/`. The Vite build produces two files in
//! `extensions/vscode/media/storymap/`:
//!
//!   - `storymap.js`  — the bundled React application
//!   - `storymap.css` — the bundled styles
//!
//! This provider loads those built assets and injects them into a minimal
//! HTML shell that the VS Code webview API can render.
//!
//! Features (all preserved from the original inline implementation):
//!
//! - Dot grid background (panning canvas feel)
//! - Origin at top-left (0,0), start passage near origin
//! - Position-based layout using Twee passage `{"position":"x,y"}` metadata
//! - Automatic dagre layout for passages without position data
//! - Click-to-navigate (clicking a node opens the passage in the editor)
//! - Color-coded nodes (normal, special, metadata, unreachable)
//! - Red dashed edges for broken links
//! - Drag-to-reposition with position write-back (Twine-compatible `{"position":"x,y"}`)
//! - Search/filter passages by name or tag
//! - Zoom-to-fit and layout switching controls
//! - Game loop visualization

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { KnotLanguageClient, KnotGraphResponse, KnotUpdatePositionsParams, KnotUpdatePositionsResponse } from './types';

// ---------------------------------------------------------------------------
// Story Map output channel — shared across all StoryMapProvider instances
// ---------------------------------------------------------------------------

let _storyMapChannel: vscode.OutputChannel | null = null;

/** Get or create the dedicated output channel for Story Map log messages. */
function getStoryMapChannel(): vscode.OutputChannel {
    if (!_storyMapChannel) {
        _storyMapChannel = vscode.window.createOutputChannel('Knot Story Map');
    }
    return _storyMapChannel;
}

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

        const channel = getStoryMapChannel();

        // Handle messages from the webview
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'openPassage': {
                    const { file, line } = message;
                    if (file) {
                        const uri = vscode.Uri.parse(file);
                        const doc = await vscode.workspace.openTextDocument(uri);
                        // Sidebar: open in the active editor — no split created.
                        // Omitting viewColumn uses VS Code's default (active editor group).
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
                                const result = await this._client.sendRequest<KnotUpdatePositionsResponse>('knot/updatePositions', params);
                                // After the server applies the workspace edit, VS Code sends
                                // textDocument/didChange which re-parses and re-analyzes. However,
                                // the did_change handler runs asynchronously and diagnostics may
                                // not be published immediately. Force a diagnostic refresh by
                                // requesting a fresh graph — this ensures the Story Map and
                                // diagnostics are in sync after position updates.
                                if (result.success) {
                                    // Small delay to allow did_change to process first
                                    setTimeout(() => {
                                        vscode.commands.executeCommand('editor.action.semanticTokens.refresh');
                                    }, 100);
                                }
                            } catch (e) {
                                console.error('[Knot] Failed to update passage positions:', e);
                            }
                        }
                    }
                    break;
                }
                case 'log': {
                    // Webview → Extension logging bridge
                    const { level, message: msg } = message;
                    const prefix = level === 'error' ? '❌' : level === 'warn' ? '⚠️' : 'ℹ️';
                    channel.appendLine(`${prefix} [${new Date().toLocaleTimeString()}] ${msg}`);
                    // Also log to the extension host console for debugging
                    if (level === 'error') {
                        console.error('[Knot StoryMap]', msg);
                    } else if (level === 'warn') {
                        console.warn('[Knot StoryMap]', msg);
                    } else {
                        console.log('[Knot StoryMap]', msg);
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
            getStoryMapChannel().appendLine(`❌ [${new Date().toLocaleTimeString()}] Failed to fetch story graph: ${String(e)}`);
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

    /** Generate the HTML for the webview.
     *
     *  This loads the pre-built React application from `media/storymap/`.
     *  If the build artifacts don't exist yet (e.g., during development
     *  before running `npm run build:webview`), a fallback page is shown
     *  with instructions.
     */
    private _getHtmlForWebview(webview: vscode.Webview, isFullView: boolean): string {
        const nonce = getNonce();

        // Resolve URIs for the built React app assets
        const scriptUri = webview.asWebviewUri(
            vscode.Uri.joinPath(this._extensionUri, 'media', 'storymap', 'storymap.js')
        );
        const styleUri = webview.asWebviewUri(
            vscode.Uri.joinPath(this._extensionUri, 'media', 'storymap', 'storymap.css')
        );

        // Check if the build artifacts exist — if not, show a helpful fallback
        const storymapJsPath = path.join(this._extensionUri.fsPath, 'media', 'storymap', 'storymap.js');
        if (!fs.existsSync(storymapJsPath)) {
            return this._getFallbackHtml(webview, nonce);
        }

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src ${webview.cspSource} 'nonce-${nonce}'; img-src ${webview.cspSource} data:; connect-src ${webview.cspSource};">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Story Map</title>
    <link rel="stylesheet" href="${styleUri}">
</head>
<body>
    <div id="root"></div>
    <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
    }

    /** Fallback HTML shown when the React webview hasn't been built yet. */
    private _getFallbackHtml(webview: vscode.Webview, nonce: string): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knot Story Map</title>
    <style>
        body {
            background: var(--vscode-editor-background, #1e1e1e);
            color: var(--vscode-editor-foreground, #d4d4d4);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            display: flex;
            align-items: center;
            justify-content: center;
            height: 100vh;
            margin: 0;
            padding: 20px;
            text-align: center;
        }
        .fallback-box {
            max-width: 400px;
        }
        .fallback-box h2 {
            color: var(--vscode-editor-foreground, #d4d4d4);
            margin-bottom: 12px;
        }
        .fallback-box p {
            color: var(--vscode-descriptionForeground, #8b8b8b);
            font-size: 13px;
            line-height: 1.6;
        }
        .fallback-box code {
            background: var(--vscode-textCodeBlock-background, #2d2d30);
            padding: 2px 6px;
            border-radius: 3px;
            font-size: 12px;
        }
    </style>
</head>
<body>
    <div class="fallback-box">
        <h2>Story Map — Not Built</h2>
        <p>
            The Story Map webview has not been built yet. Run the following
            command from the <code>extensions/vscode</code> directory:
        </p>
        <p style="margin-top: 12px;">
            <code>npm run build:webview</code>
        </p>
        <p style="margin-top: 12px;">
            Or use the project build script from the repository root:
        </p>
        <p style="margin-top: 12px;">
            <code>./scripts/build.sh</code>
        </p>
    </div>
</body>
</html>`;
    }
}
