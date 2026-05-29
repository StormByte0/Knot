//! Story Map webview panel provider for the Knot extension.
//!
//! This module implements the Story Map as a **WebviewPanel only** (no sidebar
//! WebviewView). Per the v3 design spec:
//!
//! - The Story Map is a visualization and navigation tool, not an editor.
//! - One panel instance at a time (single-instance guarantee).
//! - Can be docked anywhere, popped to a separate window.
//! - Editor ↔ Graph coupling: cursor movement focuses nodes; clicking nodes
//!   opens passages.
//! - Viewport persistence via workspace state.
//!
//! The graph UI is built as a React + Vite application using React Flow,
//! located in `extensions/vscode/webview/`. The Vite build produces two files
//! in `extensions/vscode/media/storymap/`:
//!
//!   - `storymap.js`  — the bundled React application
//!   - `storymap.css` — the bundled styles
//!
//! This provider loads those built assets and injects them into a minimal
//! HTML shell that the VS Code webview API can render.

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { KnotLanguageClient, KnotGraphResponse, KnotUpdatePositionsParams, KnotUpdatePositionsResponse } from './types';
import { navigateToPassage, findTargetViewColumn } from './navigation';

// ---------------------------------------------------------------------------
// Story Map output channel — shared across all instances
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
// Nonce helper
// ---------------------------------------------------------------------------

function getNonce(): string {
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    let nonce = '';
    for (let i = 0; i < 32; i++) {
        nonce += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return nonce;
}

// ---------------------------------------------------------------------------
// Story Map Panel Manager (WebviewPanel only, no WebviewView)
// ---------------------------------------------------------------------------

/**
 * Manages the single-instance Story Map WebviewPanel.
 *
 * Design: There is only ONE graph view at a time. Opening a new one closes
 * the old one. This eliminates redundant position writes and ensures focusNode
 * always reaches the active view.
 */
export class StoryMapPanelManager {
    private _panel: vscode.WebviewPanel | null = null;
    private _client: KnotLanguageClient | null = null;
    private _extensionUri: vscode.Uri;
    private _context: vscode.ExtensionContext;
    private _graphData: KnotGraphResponse | null = null;

    constructor(extensionUri: vscode.Uri, context: vscode.ExtensionContext) {
        this._extensionUri = extensionUri;
        this._context = context;
    }

    /** Dispose the panel (implements Disposable for context.subscriptions). */
    public dispose() {
        if (this._panel) {
            this._panel.dispose();
            this._panel = null;
        }
        this._graphData = null;
    }

    /** Set the language client reference so we can send LSP requests. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
        // If the panel is already open, refresh immediately
        if (this._panel) {
            this.refreshGraph();
        }
    }

    // setDebugViewProvider removed — cross-view sync is now handled by
    // the centralized navigation module (navigation.ts).

    /** Check if the panel is currently visible. */
    public isVisible(): boolean {
        return this._panel !== null && this._panel.visible;
    }

    /** Get the panel's view column, if open. */
    public get viewColumn(): vscode.ViewColumn | undefined {
        return this._panel?.viewColumn;
    }

    /** Open or reveal the Story Map panel. Single-instance: opening a new
     *  one closes the old one. */
    public async show() {
        // If a panel already exists, just reveal it
        if (this._panel) {
            this._panel.reveal(vscode.ViewColumn.Beside);
            return;
        }

        const panel = vscode.window.createWebviewPanel(
            'knot.storyMapPanel',
            'Knot Story Map',
            vscode.ViewColumn.Beside,
            {
                enableScripts: true,
                localResourceRoots: [this._extensionUri],
                retainContextWhenHidden: true,
            }
        );

        this._panel = panel;
        this._setupPanel(panel);
    }

    /** Focus on a specific passage node in the graph. */
    public focusNode(passageName: string) {
        if (this._panel) {
            this._panel.webview.postMessage({
                command: 'focusNode',
                passageName,
            });
        }
    }

    /** Fetch graph data from the language server and push to the webview. */
    public async refreshGraph() {
        if (!this._client || !this._client.isRunning() || !this._panel) {
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
        if (this._panel && this._graphData) {
            this._panel.webview.postMessage({
                command: 'updateGraph',
                data: this._graphData,
            });
        }
    }

    /** Set up a newly created panel with HTML, message handlers, and lifecycle. */
    private _setupPanel(panel: vscode.WebviewPanel) {
        // Generate the HTML for the webview
        panel.webview.html = this._getHtmlForWebview(panel.webview);

        // Restore viewport from workspace state
        const savedViewport = this._context.workspaceState.get<{ x: number; y: number; zoom: number }>('knot.storyMapViewport');
        if (savedViewport) {
            // Send viewport after a short delay to let the webview initialize
            setTimeout(() => {
                panel.webview.postMessage({
                    command: 'restoreViewport',
                    x: savedViewport.x,
                    y: savedViewport.y,
                    zoom: savedViewport.zoom,
                });
            }, 200);
        }

        const channel = getStoryMapChannel();

        // Handle messages from the webview
        panel.webview.onDidReceiveMessage(async (message) => {
            switch (message.command) {
                case 'openPassage': {
                    const { file, line, passageName } = message;
                    if (passageName) {
                        // Use centralized navigation — handles ViewColumn
                        // placement, StoryMap focus, and DebugView sync.
                        // The StoryMap already knows which passage was clicked
                        // (it IS the source), so focusNode is a no-op inside
                        // navigateToPassage when the panel is already focused.
                        await navigateToPassage(passageName, line ?? undefined);
                    } else if (file) {
                        // Fallback: file-only navigation (no passage name)
                        const uri = vscode.Uri.parse(file);
                        const doc = await vscode.workspace.openTextDocument(uri);
                        const targetColumn = findTargetViewColumn(panel.viewColumn);
                        await vscode.window.showTextDocument(doc, {
                            preview: true,
                            viewColumn: targetColumn,
                            selection: new vscode.Range(line || 0, 0, line || 0, 200),
                        });
                    }
                    break;
                }
                case 'refreshGraph': {
                    await this.refreshGraph();
                    break;
                }
                case 'updatePositions': {
                    const { updates } = message;
                    if (this._client && this._client.isRunning() && updates && updates.length > 0) {
                        const workspaceFolders = vscode.workspace.workspaceFolders;
                        if (workspaceFolders && workspaceFolders.length > 0) {
                            try {
                                await this._client.sendRequest<KnotUpdatePositionsResponse>('knot/updatePositions', {
                                    workspace_uri: workspaceFolders[0].uri.toString(),
                                    updates: updates,
                                });
                            } catch (e) {
                                console.error('[Knot] Failed to update passage positions:', e);
                            }
                        }
                    }
                    break;
                }
                case 'saveAllPositions': {
                    const { updates } = message;
                    if (this._client && this._client.isRunning() && updates && updates.length > 0) {
                        const workspaceFolders = vscode.workspace.workspaceFolders;
                        if (workspaceFolders && workspaceFolders.length > 0) {
                            try {
                                const result = await this._client.sendRequest<KnotUpdatePositionsResponse>('knot/updatePositions', {
                                    workspace_uri: workspaceFolders[0].uri.toString(),
                                    updates: updates,
                                });
                                if (result.success) {
                                    vscode.window.setStatusBarMessage(`Knot: Saved ${result.updated_count} passage positions`, 3000);
                                } else if (result.errors && result.errors.length > 0) {
                                    vscode.window.showWarningMessage(`Knot: Some positions failed to save: ${result.errors.join(', ')}`);
                                }
                            } catch (e) {
                                console.error('[Knot] Failed to save all passage positions:', e);
                                vscode.window.showErrorMessage('Knot: Failed to save passage positions.');
                            }
                        }
                    }
                    break;
                }
                case 'updateViewport': {
                    // Persist viewport state to workspace state
                    const { x, y, zoom } = message;
                    this._context.workspaceState.update('knot.storyMapViewport', { x, y, zoom });
                    break;
                }
                case 'log': {
                    const { level, message: msg } = message;
                    const prefix = level === 'error' ? '❌' : level === 'warn' ? '⚠️' : 'ℹ️';
                    channel.appendLine(`${prefix} [${new Date().toLocaleTimeString()}] ${msg}`);
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

        // When the panel is closed, clean up
        panel.onDidDispose(() => {
            this._panel = null;
            this._graphData = null;
        });

        // When the panel becomes visible again, refresh
        panel.onDidChangeViewState((e) => {
            if (e.webviewPanel.visible) {
                this.refreshGraph();
            }
        });

        // Initial graph load
        if (this._client && this._client.isRunning()) {
            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (workspaceFolders && workspaceFolders.length > 0) {
                this._client.sendRequest<KnotGraphResponse>('knot/graph', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                }).then((result) => {
                    this._graphData = result;
                    this._postGraphData();
                }).catch((e) => {
                    console.error('[Knot] Failed to fetch initial graph data:', e);
                });
            }
        }
    }

    /**
     * Find the best ViewColumn for opening a passage from the Story Map.
     *
     * Logic:
     * - If the graph panel has no viewColumn (detached window), open in the
     *   default active editor (no split).
     * - If the graph is in a tab in the same window, find an existing
     *   non-graph column to reuse.
     * - If no non-graph editors exist and the graph is in column 2+,
     *   open in column 1.
     * - If the graph is in column 1 and no other editors exist, create a
     *   column beside it (ViewColumn.Beside).
     */
    // _findTargetViewColumn removed — use the shared findTargetViewColumn()
    // from navigation.ts instead. Kept as a thin wrapper for the fallback
    // file-only navigation path above.


    /** Generate HTML for the webview.
     *
     *  This loads the pre-built React application from `media/storymap/`.
     *  If the build artifacts don't exist yet, a fallback page is shown.
     */
    private _getHtmlForWebview(webview: vscode.Webview): string {
        const nonce = getNonce();

        // Resolve URIs for the built React app assets
        const scriptUri = webview.asWebviewUri(
            vscode.Uri.joinPath(this._extensionUri, 'media', 'storymap', 'storymap.js')
        );
        const styleUri = webview.asWebviewUri(
            vscode.Uri.joinPath(this._extensionUri, 'media', 'storymap', 'storymap.css')
        );

        // Check if the build artifacts exist
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
