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
import {
    KnotLanguageClient,
    KnotGraphResponse,
    KnotUpdatePositionsResponse,
    KnotUpdatePassageMetadataParams,
    KnotUpdatePassageMetadataResponse,
    KnotBulkUpdateMetadataParams,
    KnotBulkUpdateMetadataResponse,
    KnotCreatePassageParams,
    KnotCreatePassageResponse,
    KnotDeletePassageParams,
    KnotDeletePassageResponse,
    KnotRenamePassageParams,
    KnotRenamePassageResponse,
    KnotSetStartPassageParams,
    KnotSetStartPassageResponse,
} from './types';
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

    /** Get the panel's view column, if open. */
    public get viewColumn(): vscode.ViewColumn | undefined {
        return this._panel?.viewColumn;
    }

    /** Reveal the panel without changing focus (used by ViewColumn guard). */
    public reveal(): void {
        if (this._panel) {
            this._panel.reveal(this._panel.viewColumn, true); // preserveFocus = true
        }
    }

    /** Open or reveal the Story Map panel. Single-instance: opening a new
     *  one closes the old one. The panel stays in its assigned view column
     *  and does not create new tabs or splits on subsequent opens. */
    public async show() {
        // If a panel already exists, reveal it in its CURRENT column
        // (not ViewColumn.Beside, which would move it to a new split).
        if (this._panel) {
            this._panel.reveal(this._panel.viewColumn ?? vscode.ViewColumn.Beside, true);
            return;
        }

        // First creation: open beside the active editor.
        // retainContextWhenHidden keeps the webview alive when the tab
        // is not visible, so the graph state persists across tab switches.
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

            // Game-loop (SCC) visualization is stashed pending algorithm
            // hardening: the flow-analysis results aren't yet solid enough
            // to confidently show loops. The server still computes the
            // data (dormant, tested); we strip it from the payload unless
            // the user opts back in via knot.storyMap.gameLoops.
            const gameLoopsEnabled = vscode.workspace
                .getConfiguration('knot.storyMap')
                .get<boolean>('gameLoops', false);
            this._graphData = gameLoopsEnabled
                ? result
                : { ...result, game_loops: [] };

            this._postGraphData();
            this._pushWorkspaceFiles();
        } catch (e) {
            console.error('[Knot] Failed to fetch story graph:', e);
            getStoryMapChannel().appendLine(`❌ [${new Date().toLocaleTimeString()}] Failed to fetch story graph: ${String(e)}`);
        }
    }

    /** Compute the game-source file list (the files Tweego compiles:
     *  .twee/.tw sources, .js scripts, .css styles) and push it to the
     *  webview for the file navigator and the create-passage file picker. */
    private async _pushWorkspaceFiles() {
        if (!this._panel) {
            return;
        }
        try {
            const exclude = '**/{node_modules,.git,.vscode,dist,build,out}/**';
            const [twee, js, css] = await Promise.all([
                vscode.workspace.findFiles('**/*.{twee,tw}', exclude),
                vscode.workspace.findFiles('**/*.js', exclude),
                vscode.workspace.findFiles('**/*.css', exclude),
            ]);
            const toRel = (u: vscode.Uri) => {
                const ws = vscode.workspace.getWorkspaceFolder(u);
                return ws ? vscode.workspace.asRelativePath(u) : u.fsPath;
            };
            const passageCountByFile = new Map<string, number>();
            for (const n of this._graphData?.nodes ?? []) {
                if (!n.is_placeholder && n.file) {
                    passageCountByFile.set(n.file, (passageCountByFile.get(n.file) ?? 0) + 1);
                }
            }
            const fmt = (uris: vscode.Uri[], kind: string) => uris.map(u => ({
                uri: u.toString(),
                name: toRel(u),
                kind,
                passageCount: passageCountByFile.get(u.toString()) ?? 0,
            })).sort((a, b) => a.name.localeCompare(b.name));
            this._panel.webview.postMessage({
                command: 'updateWorkspaceFiles',
                tweeFiles: fmt(twee, 'twee'),
                jsFiles: fmt(js, 'js'),
                cssFiles: fmt(css, 'css'),
                sourceDir: this._sourceDirSegment(),
            });
        } catch (e) {
            console.error('[Knot] Failed to list workspace files:', e);
        }
    }

    /** The `knot.build.sourceDir` setting, normalized to a clean
     *  workspace-relative segment ('' when unset / root). New game
     *  source files (create-passage "New file…") are created inside it
     *  so Tweego picks them up, instead of littering the root. */
    private _sourceDirSegment(): string {
        const raw = vscode.workspace.getConfiguration('knot').get<string>('build.sourceDir') || '';
        return raw.trim().replace(/\\/g, '/').replace(/^\/+/, '').replace(/\/+$/, '');
    }

    /** Refresh the graph after a workspace edit round-trips through the
     *  server. The client applies the edit, `did_change` re-syncs the
     *  server's document model, and the delayed refresh then pulls the
     *  updated graph. The delay keeps the refresh behind the re-sync. */
    private _refreshAfterEdit(delayMs = 400) {
        setTimeout(() => {
            this.refreshGraph();
        }, delayMs);
    }

    /** Resolve the workspace root URI for knot/* requests. */
    private _workspaceUri(): string | undefined {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return undefined;
        }
        return workspaceFolders[0].uri.toString();
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
                case 'updatePassageMetadata': {
                    // GUI inspector edit (Tier A): tags/color/group/size/
                    // manual reachable link/position on one passage.
                    const { passage_name, tags, group, color, size, reachable, position } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    // Build the request with the three-state field semantics:
                    // undefined = absent (unchanged), null = remove, value = set.
                    // JSON.stringify drops undefined properties; null survives.
                    const params: KnotUpdatePassageMetadataParams = {
                        workspace_uri: workspaceUri,
                        passage_name,
                    };
                    if (tags !== undefined) params.tags = tags;
                    if (group !== undefined) params.group = group;
                    if (color !== undefined) params.color = color;
                    if (size !== undefined) params.size = size;
                    if (reachable !== undefined) params.reachable = reachable;
                    if (position !== undefined) params.position = position;
                    try {
                        const result = await this._client.sendRequest<KnotUpdatePassageMetadataResponse>(
                            'knot/updatePassageMetadata', params,
                        );
                        if (!result.success) {
                            vscode.window.showErrorMessage(`Knot: ${result.error ?? 'Failed to update passage metadata'}`);
                        } else {
                            this._refreshAfterEdit();
                        }
                    } catch (e) {
                        console.error('[Knot] Failed to update passage metadata:', e);
                        vscode.window.showErrorMessage('Knot: Failed to update passage metadata.');
                    }
                    break;
                }
                case 'openFile': {
                    // File navigator click — open the game source file in
                    // the editor (same column logic as passage navigation).
                    const { file } = message;
                    if (!file) break;
                    try {
                        const uri = vscode.Uri.parse(file);
                        const doc = await vscode.workspace.openTextDocument(uri);
                        const targetColumn = findTargetViewColumn(panel.viewColumn);
                        await vscode.window.showTextDocument(doc, {
                            preview: true,
                            viewColumn: targetColumn,
                        });
                    } catch (e) {
                        console.error('[Knot] Failed to open file from navigator:', e);
                        vscode.window.showErrorMessage(`Knot: Failed to open ${file}`);
                    }
                    break;
                }
                case 'createPassage': {
                    // GUI passage creation (Tier B): new passage from the
                    // graph (context menu / double-click). The webview's
                    // file picker sends either `target_file` (an existing
                    // .twee URI) or `new_file` (a workspace-relative path
                    // for a file that should be created with the passage).
                    const { name, tags, position, target_file, new_file } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    const params: KnotCreatePassageParams = {
                        workspace_uri: workspaceUri,
                        name,
                    };
                    if (tags !== undefined) params.tags = tags;
                    if (position !== undefined) params.position = position;
                    if (target_file !== undefined) params.target_file = target_file;
                    if (new_file !== undefined && new_file !== null && `${new_file}`.trim() !== '') {
                        // Resolve the relative path against the SOURCE
                        // directory (`knot.build.sourceDir`, falling back
                        // to the workspace root) — new game files must
                        // land where Tweego actually compiles from, not
                        // in the project root. The server re-validates
                        // (extension + root confinement) before creating.
                        const rel = `${new_file}`.trim().replace(/\\/g, '/');
                        if (rel.includes('..') || !/\.(twee|tw)$/i.test(rel)) {
                            const sourceDir = this._sourceDirSegment();
                            vscode.window.showErrorMessage(
                                `Knot: Invalid new file name '${rel}' — must be a .twee/.tw path inside ${sourceDir ? `the source directory (${sourceDir})` : 'the workspace'}`);
                            break;
                        }
                        const root = vscode.workspace.getWorkspaceFolder(vscode.Uri.parse(workspaceUri));
                        if (!root) break;
                        const sourceDir = this._sourceDirSegment();
                        const baseUri = sourceDir
                            ? vscode.Uri.joinPath(root.uri, ...sourceDir.split('/'))
                            : root.uri;
                        const newUri = vscode.Uri.joinPath(baseUri, ...rel.split('/'));
                        params.target_file = newUri.toString();
                        params.create_file = true;
                    }
                    try {
                        const result = await this._client.sendRequest<KnotCreatePassageResponse>(
                            'knot/createPassage', params,
                        );
                        if (!result.success) {
                            vscode.window.showErrorMessage(`Knot: ${result.error ?? 'Failed to create passage'}`);
                        } else {
                            vscode.window.setStatusBarMessage(`Knot: Created passage '${name}'`, 3000);
                            this._refreshAfterEdit();
                        }
                    } catch (e) {
                        console.error('[Knot] Failed to create passage:', e);
                        vscode.window.showErrorMessage('Knot: Failed to create passage.');
                    }
                    break;
                }
                case 'bulkUpdateMetadata': {
                    // Atomic multi-passage metadata edit: group create /
                    // rename / dissolve and multi-select tag/color/group/
                    // size edits — ONE request, ONE refresh.
                    const { passage_names, tags, group, color, size } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    if (!Array.isArray(passage_names) || passage_names.length === 0) break;
                    const params: KnotBulkUpdateMetadataParams = {
                        workspace_uri: workspaceUri,
                        passage_names,
                    };
                    if (tags !== undefined) params.tags = tags;
                    if (group !== undefined) params.group = group;
                    if (color !== undefined) params.color = color;
                    if (size !== undefined) params.size = size;
                    try {
                        const result = await this._client.sendRequest<KnotBulkUpdateMetadataResponse>(
                            'knot/bulkUpdateMetadata', params,
                        );
                        if (!result.success) {
                            const detail = result.errors?.length
                                ? `: ${result.errors.slice(0, 3).join('; ')}`
                                : '';
                            vscode.window.showWarningMessage(
                                `Knot: Bulk edit updated ${result.updated_count}/${passage_names.length} passages${detail}`);
                        } else {
                            vscode.window.setStatusBarMessage(
                                `Knot: Updated ${result.updated_count} passages`, 3000);
                        }
                        this._refreshAfterEdit();
                    } catch (e) {
                        console.error('[Knot] Failed to bulk update metadata:', e);
                        vscode.window.showErrorMessage('Knot: Failed to bulk update passage metadata.');
                    }
                    break;
                }
                case 'deletePassage': {
                    // GUI passage deletion (Tier B): remove the passage
                    // block, optionally unwrinking links to it.
                    const { passage_name, strip_links } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    const params: KnotDeletePassageParams = {
                        workspace_uri: workspaceUri,
                        passage_name,
                        strip_links: !!strip_links,
                    };
                    try {
                        const result = await this._client.sendRequest<KnotDeletePassageResponse>(
                            'knot/deletePassage', params,
                        );
                        if (!result.success) {
                            vscode.window.showErrorMessage(`Knot: ${result.error ?? 'Failed to delete passage'}`);
                        } else {
                            const stripped = result.links_stripped > 0
                                ? ` (${result.links_stripped} link${result.links_stripped === 1 ? '' : 's'} removed)`
                                : '';
                            vscode.window.setStatusBarMessage(
                                `Knot: Deleted passage '${passage_name}'${stripped}`, 4000);
                            this._refreshAfterEdit();
                        }
                    } catch (e) {
                        console.error('[Knot] Failed to delete passage:', e);
                        vscode.window.showErrorMessage('Knot: Failed to delete passage.');
                    }
                    break;
                }
                case 'renamePassage': {
                    // GUI passage rename (Tier B): header + all links +
                    // manual reachable declarations, cross-file.
                    const { name, new_name } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    const params: KnotRenamePassageParams = {
                        workspace_uri: workspaceUri,
                        name,
                        new_name,
                    };
                    try {
                        const result = await this._client.sendRequest<KnotRenamePassageResponse>(
                            'knot/renamePassage', params,
                        );
                        if (!result.success) {
                            vscode.window.showErrorMessage(`Knot: ${result.error ?? 'Failed to rename passage'}`);
                        } else {
                            vscode.window.setStatusBarMessage(
                                `Knot: Renamed '${name}' → '${new_name}' (${result.files_changed} file${result.files_changed === 1 ? '' : 's'})`,
                                4000);
                            this._refreshAfterEdit();
                        }
                    } catch (e) {
                        console.error('[Knot] Failed to rename passage:', e);
                        vscode.window.showErrorMessage('Knot: Failed to rename passage.');
                    }
                    break;
                }
                case 'setStartPassage': {
                    // GUI story setting (Tier C): rewrite StoryData's start.
                    const { passage_name } = message;
                    const workspaceUri = this._workspaceUri();
                    if (!this._client || !this._client.isRunning() || !workspaceUri) {
                        break;
                    }
                    const params: KnotSetStartPassageParams = {
                        workspace_uri: workspaceUri,
                        passage_name,
                    };
                    try {
                        const result = await this._client.sendRequest<KnotSetStartPassageResponse>(
                            'knot/setStartPassage', params,
                        );
                        if (!result.success) {
                            vscode.window.showErrorMessage(`Knot: ${result.error ?? 'Failed to set start passage'}`);
                        } else {
                            vscode.window.setStatusBarMessage(
                                `Knot: Start passage set to '${passage_name}'`, 3000);
                            this._refreshAfterEdit();
                        }
                    } catch (e) {
                        console.error('[Knot] Failed to set start passage:', e);
                        vscode.window.showErrorMessage('Knot: Failed to set start passage.');
                    }
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
    private _getFallbackHtml(webview: vscode.Webview, _nonce: string): string {
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
