//! Play Mode webview provider for the Knot extension.
//!
//! This module implements a VS Code webview panel that loads compiled
//! Twine story HTML for in-editor play testing. Features include:
//!
//! - Auto-rebuild on file save (with debounce)
//! - Navigation controls (back, forward, restart, refresh)
//! - Passage history sidebar
//! - Passage tracking via iframe message passing
//! - Build status indicator
//! - Keyboard shortcuts for navigation

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs/promises';
import { KnotLanguageClient, KnotPlayResponse } from './types';

// ---------------------------------------------------------------------------
// Play session state
// ---------------------------------------------------------------------------

/** Represents a single passage visit in the play history. */
interface PassageVisit {
    /** The passage name. */
    name: string;
    /** Timestamp of the visit. */
    timestamp: number;
}

/** Play session state, persisted across rebuilds when possible. */
interface PlaySessionState {
    /** The history of visited passages. */
    history: PassageVisit[];
    /** Current position in the history (for back/forward navigation). */
    historyIndex: number;
    /** The number of passages visited in this session. */
    totalVisits: number;
    /** Whether auto-rebuild is enabled. */
    autoRebuild: boolean;
}

// ---------------------------------------------------------------------------
// Play Mode webview provider
// ---------------------------------------------------------------------------

export class PlayModeProvider {
    private _panel: vscode.WebviewPanel | undefined;
    private _client: KnotLanguageClient | null = null;
    private _extensionUri: vscode.Uri;
    private _context: vscode.ExtensionContext;
    private _sessionState: PlaySessionState;
    private _lastBuildHtml: string | null = null;
    private _rebuildDebounceTimer: ReturnType<typeof setTimeout> | null = null;
    private _isBuilding: boolean = false;
    private _saveWatcher: vscode.Disposable | null = null;
    private _statusBarItem: vscode.StatusBarItem | null = null;
    private _startPassage: string | undefined;

    constructor(extensionUri: vscode.Uri, context: vscode.ExtensionContext) {
        this._extensionUri = extensionUri;
        this._context = context;
        this._sessionState = {
            history: [],
            historyIndex: -1,
            totalVisits: 0,
            autoRebuild: true,
        };
    }

    /** Set the language client reference. */
    public setClient(client: KnotLanguageClient | null) {
        this._client = client;
    }

    /** Open the play mode panel. */
    public async show(startPassage?: string) {
        this._startPassage = startPassage;

        if (this._panel) {
            this._panel.reveal(vscode.ViewColumn.Beside);
            // Rebuild with the new start passage if provided
            if (startPassage) {
                await this._buildAndLoad();
            }
            return;
        }

        const title = startPassage
            ? `Knot: Play from ${startPassage}`
            : 'Knot: Play Story';

        // Include the global storage URI so the webview can load the temp HTML file
        const storageUri = this._context.globalStorageUri;

        this._panel = vscode.window.createWebviewPanel(
            'knot.playMode',
            title,
            vscode.ViewColumn.Beside,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
                localResourceRoots: [this._extensionUri, storageUri],
            }
        );

        this._panel.onDidDispose(() => {
            this._panel = undefined;
            this._disposeSaveWatcher();
            this._disposeStatusBar();
        });

        this._panel.webview.onDidReceiveMessage((message) => {
            this._handleWebviewMessage(message);
        });

        // Create status bar for play mode
        this._createStatusBar();

        // Register auto-rebuild on save
        this._registerSaveWatcher();

        // Show loading state
        this._panel.webview.html = this._getLoadingHtml();

        // Build and load the story
        await this._buildAndLoad();
    }

    /** Rebuild and reload the story. */
    public async refresh() {
        if (this._panel) {
            this._notifyBuildStatus('building');
            await this._buildAndLoad();
        }
    }

    /** Toggle auto-rebuild on save. */
    public toggleAutoRebuild() {
        this._sessionState.autoRebuild = !this._sessionState.autoRebuild;
        this._postSessionState();
        if (this._statusBarItem) {
            this._updateStatusBar();
        }
        vscode.window.showInformationMessage(
            `Knot: Auto-rebuild on save ${this._sessionState.autoRebuild ? 'enabled' : 'disabled'}`
        );
    }

    // -----------------------------------------------------------------------
    // Save watcher for auto-rebuild
    // -----------------------------------------------------------------------

    /** Register a save watcher that triggers rebuild on Twee file saves. */
    private _registerSaveWatcher() {
        this._disposeSaveWatcher();
        this._saveWatcher = vscode.workspace.onDidSaveTextDocument((doc) => {
            if (!this._sessionState.autoRebuild || !this._panel) {
                return;
            }
            const ext = path.extname(doc.fileName).toLowerCase();
            if (ext === '.tw' || ext === '.twee') {
                this._debouncedRebuild();
            }
        });
    }

    /** Dispose the save watcher. */
    private _disposeSaveWatcher() {
        if (this._saveWatcher) {
            this._saveWatcher.dispose();
            this._saveWatcher = null;
        }
    }

    /** Debounced rebuild — waits 500ms after the last save before rebuilding. */
    private _debouncedRebuild() {
        if (this._rebuildDebounceTimer) {
            clearTimeout(this._rebuildDebounceTimer);
        }
        this._rebuildDebounceTimer = setTimeout(async () => {
            this._rebuildDebounceTimer = null;
            await this.refresh();
        }, 500);
    }

    // -----------------------------------------------------------------------
    // Status bar
    // -----------------------------------------------------------------------

    /** Create a status bar item for play mode. */
    private _createStatusBar() {
        this._disposeStatusBar();
        this._statusBarItem = vscode.window.createStatusBarItem(
            vscode.StatusBarAlignment.Right,
            100
        );
        this._updateStatusBar();
        this._statusBarItem.show();
    }

    /** Update the status bar text. */
    private _updateStatusBar() {
        if (!this._statusBarItem) { return; }
        const autoIcon = this._sessionState.autoRebuild ? '$(sync)' : '$(sync-ignored)';
        const visits = this._sessionState.totalVisits;
        this._statusBarItem.text = `${autoIcon} Knot Play | ${visits} passage visits`;
        this._statusBarItem.tooltip = `Auto-rebuild: ${this._sessionState.autoRebuild ? 'ON' : 'OFF'} | Click to toggle`;
        this._statusBarItem.command = 'knot.toggleAutoRebuild';
    }

    /** Dispose the status bar item. */
    private _disposeStatusBar() {
        if (this._statusBarItem) {
            this._statusBarItem.dispose();
            this._statusBarItem = null;
        }
    }

    // -----------------------------------------------------------------------
    // Webview message handling
    // -----------------------------------------------------------------------

    /** Handle messages from the webview. */
    private _handleWebviewMessage(message: { command: string; [key: string]: any }) {
        switch (message.command) {
            case 'passageVisited': {
                const passageName = message.name as string;
                if (passageName) {
                    this._recordPassageVisit(passageName);
                    // Fetch debug info for the visited passage
                    this._fetchDebugInfo(passageName);
                }
                break;
            }
            case 'restart': {
                this._sessionState.history = [];
                this._sessionState.historyIndex = -1;
                this._sessionState.totalVisits = 0;
                this._postSessionState();
                this._updateStatusBar();
                // Rebuild from scratch (don't replay cached HTML — it may be
                // stale or from a failed build)
                this._buildAndLoad();
                break;
            }
            case 'rebuild': {
                // Triggered by the error page "Retry Build" button.
                // Performs a fresh build (not a session restart).
                this._buildAndLoad();
                break;
            }
            case 'goBack': {
                if (this._sessionState.historyIndex > 0) {
                    this._sessionState.historyIndex--;
                    this._postSessionState();
                }
                break;
            }
            case 'goForward': {
                if (this._sessionState.historyIndex < this._sessionState.history.length - 1) {
                    this._sessionState.historyIndex++;
                    this._postSessionState();
                }
                break;
            }
            case 'openPassage': {
                const name = message.name as string;
                if (name) {
                    vscode.commands.executeCommand('knot.openPassageByName', name);
                }
                break;
            }
            case 'toggleAutoRebuild': {
                this.toggleAutoRebuild();
                break;
            }
        }
    }

    /** Record a passage visit in the session history. */
    private _recordPassageVisit(passageName: string) {
        // Truncate forward history if we visited from a non-latest position
        if (this._sessionState.historyIndex < this._sessionState.history.length - 1) {
            this._sessionState.history = this._sessionState.history.slice(
                0,
                this._sessionState.historyIndex + 1
            );
        }

        this._sessionState.history.push({
            name: passageName,
            timestamp: Date.now(),
        });
        this._sessionState.historyIndex = this._sessionState.history.length - 1;
        this._sessionState.totalVisits++;

        this._postSessionState();
        this._updateStatusBar();
    }

    /** Post the current session state to the webview. */
    private _postSessionState() {
        if (this._panel) {
            this._panel.webview.postMessage({
                command: 'updateSession',
                data: {
                    history: this._sessionState.history,
                    historyIndex: this._sessionState.historyIndex,
                    totalVisits: this._sessionState.totalVisits,
                    autoRebuild: this._sessionState.autoRebuild,
                    canGoBack: this._sessionState.historyIndex > 0,
                    canGoForward: this._sessionState.historyIndex < this._sessionState.history.length - 1,
                },
            });
        }
    }

    /** Notify the webview about the build status. */
    private _notifyBuildStatus(status: 'building' | 'success' | 'error') {
        if (this._panel) {
            this._panel.webview.postMessage({
                command: 'buildStatus',
                status,
            });
        }
    }

    /** Fetch debug info for a passage and send it to the webview. */
    private async _fetchDebugInfo(passageName: string) {
        if (!this._client || !this._client.isRunning()) {
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            return;
        }

        try {
            const result = await this._client.sendRequest('knot/passageDiagnostics', {
                passage_name: passageName,
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

            if (this._panel) {
                this._panel.webview.postMessage({
                    command: 'debugInfo',
                    data: result,
                    passage_name: passageName,
                });
            }
        } catch (e) {
            // Silently ignore debug fetch errors during play
        }
    }

    // -----------------------------------------------------------------------
    // Build & load
    // -----------------------------------------------------------------------

    /** Build the project and load the result into the webview. */
    private async _buildAndLoad() {
        if (this._isBuilding) { return; }
        this._isBuilding = true;

        if (!this._client || !this._client.isRunning()) {
            this._showError('Knot: Language server is not running.');
            this._isBuilding = false;
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            this._showError('Knot: No workspace folder open.');
            this._isBuilding = false;
            return;
        }

        this._notifyBuildStatus('building');

        try {
            const requestParams: Record<string, string> = {
                workspace_uri: workspaceFolders[0].uri.toString(),
            };
            if (this._startPassage) {
                requestParams.start_passage = this._startPassage;
            }

            const result: KnotPlayResponse = await this._client.sendRequest('knot/play', requestParams);

            if (result.html_path) {
                const htmlContent: string = await fs.readFile(result.html_path, 'utf-8');
                this._lastBuildHtml = htmlContent;

                if (this._panel) {
                    await this._loadStoryHtml(htmlContent);
                }
                this._notifyBuildStatus('success');
            } else {
                this._showError(`Build failed: ${result.error || 'unknown error'}`);
                this._notifyBuildStatus('error');
            }
        } catch (e) {
            this._showError(`Play request failed: ${e}`);
            this._notifyBuildStatus('error');
        } finally {
            this._isBuilding = false;
        }
    }

    /** Write the story HTML to a temp file and load it via file:// URI in the iframe. */
    private async _loadStoryHtml(storyHtml: string) {
        if (!this._panel) { return; }

        // Write the compiled HTML to a temp file in the extension's global storage
        const storageUri = this._context.globalStorageUri;
        await vscode.workspace.fs.createDirectory(storageUri);
        const htmlFile = vscode.Uri.joinPath(storageUri, 'play-preview.html');
        await vscode.workspace.fs.writeFile(htmlFile, Buffer.from(storyHtml, 'utf-8'));

        // Convert the file URI to a webview-compatible URI and send it to the webview
        const webViewUri = this._panel.webview.asWebviewUri(htmlFile);
        this._panel.webview.postMessage({ command: 'loadStory', url: webViewUri.toString() });
    }

    /** Show an error message in the webview. */
    private _showError(message: string) {
        if (this._panel) {
            this._panel.webview.html = this._getErrorHtml(message);
        }
    }

    // -----------------------------------------------------------------------
    // HTML generation
    // -----------------------------------------------------------------------

    /** Generate the HTML for loading state. */
    private _getLoadingHtml(): string {
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
    <title>Knot: Play Story</title>
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
        }
        .loading {
            text-align: center;
        }
        .spinner {
            width: 40px;
            height: 40px;
            border: 3px solid rgba(255,255,255,0.1);
            border-top-color: var(--vscode-focusBorder, #007acc);
            border-radius: 50%;
            animation: spin 1s linear infinite;
            margin: 0 auto 16px;
        }
        @keyframes spin {
            to { transform: rotate(360deg); }
        }
    </style>
</head>
<body>
    <div class="loading">
        <div class="spinner"></div>
        <div>Building story...</div>
    </div>
</body>
</html>`;
    }

    /** Generate the HTML for error state. */
    private _getErrorHtml(message: string): string {
        const escapedMsg = message
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;');

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
    <title>Knot: Play Story</title>
    <style>
        body {
            background: var(--vscode-editor-background, #1e1e1e);
            color: var(--vscode-errorForeground, #f14c4c);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            display: flex;
            align-items: center;
            justify-content: center;
            height: 100vh;
            margin: 0;
        }
        .error {
            text-align: center;
            max-width: 500px;
            padding: 20px;
        }
        .error-icon {
            font-size: 48px;
            margin-bottom: 16px;
        }
        .error-message {
            font-family: monospace;
            white-space: pre-wrap;
            background: rgba(255,0,0,0.1);
            padding: 12px;
            border-radius: 4px;
            margin-top: 12px;
        }
        .retry-btn {
            margin-top: 16px;
            padding: 8px 20px;
            background: var(--vscode-focusBorder, #007acc);
            color: white;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            font-size: 13px;
        }
        .retry-btn:hover {
            opacity: 0.9;
        }
    </style>
</head>
<body>
    <div class="error">
        <div class="error-icon">&#x26A0;</div>
        <div>Build Failed</div>
        <div class="error-message">${escapedMsg}</div>
        <button class="retry-btn" data-action="rebuild">Retry Build</button>
    </div>
</html>
<script>
    const vscode = acquireVsCodeApi();
    document.addEventListener('click', (e) => {
        const el = e.target.closest('[data-action]');
        if (el) {
            const action = el.dataset.action;
            if (action === 'restart') { vscode.postMessage({ command: 'restart' }); }
            else if (action === 'rebuild') { vscode.postMessage({ command: 'rebuild' }); }
            else if (action === 'openPassage') { vscode.postMessage({ command: 'openPassage', name: el.dataset.passage }); }
        }
    });
</script>
</html>`;
    }

    /** Dispose the play mode panel. */
    public dispose() {
        this._disposeSaveWatcher();
        this._disposeStatusBar();
        if (this._rebuildDebounceTimer) {
            clearTimeout(this._rebuildDebounceTimer);
            this._rebuildDebounceTimer = null;
        }
        if (this._panel) {
            this._panel.dispose();
            this._panel = undefined;
        }
    }
}
