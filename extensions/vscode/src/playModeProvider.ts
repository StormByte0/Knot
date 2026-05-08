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
import * as fs from 'fs';
import * as path from 'path';

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
    private _client: any;
    private _extensionUri: vscode.Uri;
    private _sessionState: PlaySessionState;
    private _lastBuildHtml: string | null = null;
    private _rebuildDebounceTimer: ReturnType<typeof setTimeout> | null = null;
    private _isBuilding: boolean = false;
    private _saveWatcher: vscode.Disposable | null = null;
    private _statusBarItem: vscode.StatusBarItem | null = null;
    private _startPassage: string | undefined;

    constructor(extensionUri: vscode.Uri) {
        this._extensionUri = extensionUri;
        this._sessionState = {
            history: [],
            historyIndex: -1,
            totalVisits: 0,
            autoRebuild: true,
        };
    }

    /** Set the language client reference. */
    public setClient(client: any) {
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

        this._panel = vscode.window.createWebviewPanel(
            'knot.playMode',
            title,
            vscode.ViewColumn.Beside,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
                localResourceRoots: [],
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
                // Reload the iframe by re-rendering
                if (this._lastBuildHtml && this._panel) {
                    this._panel.webview.html = this._getStoryHtml(this._lastBuildHtml);
                }
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
            const result = await this._client.sendRequest('knot/debug', {
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
            const requestParams: any = {
                workspace_uri: workspaceFolders[0].uri.toString(),
            };
            if (this._startPassage) {
                requestParams.start_passage = this._startPassage;
            }

            const result = await this._client.sendRequest('knot/play', requestParams);

            if (result.html_path) {
                const htmlContent = fs.readFileSync(result.html_path, 'utf-8');
                this._lastBuildHtml = htmlContent;

                if (this._panel) {
                    this._panel.webview.html = this._getStoryHtml(htmlContent);
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
        <button class="retry-btn" onclick="vscode.postMessage({command:'restart'})">Retry Build</button>
    </div>
</html>
<script>
    const vscode = acquireVsCodeApi();
</script>
</html>`;
    }

    /** Generate the HTML that embeds the story with navigation and history. */
    private _getStoryHtml(storyHtml: string): string {
        // Embed the story HTML using srcdoc on an iframe.
        // We also inject a script into the iframe that tracks passage navigation
        // by monitoring the hash/URL changes and DOM mutations that Twine formats use.
        const escaped = storyHtml
            .replace(/&/g, '&amp;')
            .replace(/"/g, '&quot;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;');

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Knot: Play Story</title>
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
        }

        body {
            background: var(--bg);
            color: var(--fg);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            display: flex;
            flex-direction: column;
            height: 100vh;
            overflow: hidden;
        }

        /* Navigation toolbar */
        .toolbar {
            display: flex;
            align-items: center;
            gap: 4px;
            padding: 4px 8px;
            background: var(--card);
            border-bottom: 1px solid var(--border);
            flex-shrink: 0;
            z-index: 10;
        }

        .toolbar button {
            background: transparent;
            border: 1px solid transparent;
            color: var(--fg);
            padding: 4px 8px;
            border-radius: 3px;
            cursor: pointer;
            font-size: 12px;
            display: flex;
            align-items: center;
            gap: 4px;
        }

        .toolbar button:hover:not(:disabled) {
            background: var(--accent);
            color: white;
        }

        .toolbar button:disabled {
            opacity: 0.4;
            cursor: default;
        }

        .toolbar .separator {
            width: 1px;
            height: 20px;
            background: var(--border);
            margin: 0 4px;
        }

        .toolbar .passage-indicator {
            font-size: 11px;
            color: var(--muted);
            margin-left: auto;
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
            max-width: 300px;
        }

        .toolbar .build-indicator {
            font-size: 11px;
            padding: 2px 8px;
            border-radius: 8px;
            margin-left: 4px;
        }

        .build-indicator.building {
            color: var(--accent);
            background: rgba(0, 122, 204, 0.15);
        }

        .build-indicator.success {
            color: #66bb6a;
            background: rgba(102, 187, 106, 0.15);
        }

        .build-indicator.error {
            color: var(--error);
            background: rgba(241, 76, 76, 0.15);
        }

        /* Main content area */
        .main {
            display: flex;
            flex: 1;
            overflow: hidden;
        }

        /* Story iframe */
        .story-frame {
            flex: 1;
            border: none;
            background: #1e1e1e;
        }

        /* History sidebar */
        .history-panel {
            width: 200px;
            background: var(--card);
            border-left: 1px solid var(--border);
            display: flex;
            flex-direction: column;
            flex-shrink: 0;
            overflow: hidden;
        }

        .history-header {
            padding: 8px 10px;
            font-size: 11px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            border-bottom: 1px solid var(--border);
            display: flex;
            justify-content: space-between;
            align-items: center;
        }

        .history-count {
            font-weight: 400;
            font-size: 10px;
        }

        .history-list {
            flex: 1;
            overflow-y: auto;
            padding: 4px 0;
        }

        .history-item {
            padding: 4px 10px;
            font-size: 11px;
            cursor: pointer;
            display: flex;
            align-items: center;
            gap: 6px;
            color: var(--fg);
            border-left: 2px solid transparent;
        }

        .history-item:hover {
            background: rgba(255,255,255,0.05);
        }

        .history-item.current {
            background: rgba(0, 122, 204, 0.1);
            border-left-color: var(--accent);
        }

        .history-item .index {
            color: var(--muted);
            font-size: 10px;
            min-width: 16px;
        }

        .history-item .name {
            flex: 1;
            overflow: hidden;
            text-overflow: ellipsis;
            white-space: nowrap;
        }

        .history-item .time {
            color: var(--muted);
            font-size: 9px;
        }

        /* Keyboard shortcuts hint */
        .shortcuts {
            padding: 6px 10px;
            border-top: 1px solid var(--border);
            font-size: 9px;
            color: var(--muted);
            line-height: 1.5;
        }

        /* Debug section */
        .debug-section {
            border-top: 1px solid var(--border);
        }

        .debug-header {
            padding: 6px 10px;
            font-size: 10px;
            font-weight: 600;
            color: var(--muted);
            text-transform: uppercase;
            letter-spacing: 0.5px;
            cursor: pointer;
            user-select: none;
        }

        .debug-header:hover {
            color: var(--fg);
        }

        .debug-content {
            padding: 4px 10px 8px;
            max-height: 200px;
            overflow-y: auto;
        }

        .debug-empty {
            color: var(--muted);
            font-size: 10px;
            font-style: italic;
        }

        .debug-passage-name {
            font-weight: 600;
            font-size: 11px;
            margin-bottom: 4px;
        }

        .debug-row {
            display: flex;
            justify-content: space-between;
            font-size: 10px;
            padding: 1px 0;
        }

        .debug-row .label { color: var(--muted); }
        .debug-row .value { font-weight: 500; }
        .debug-row .value.warn { color: var(--warning, #cca700); }
        .debug-row .value.error { color: var(--error, #f14c4c); }
        .debug-row .value.ok { color: #66bb6a; }

        .debug-links {
            margin-top: 4px;
        }

        .debug-link-item {
            font-size: 10px;
            padding: 1px 0 1px 8px;
            border-left: 2px solid var(--accent, #007acc);
            margin-bottom: 1px;
            cursor: pointer;
        }

        .debug-link-item:hover {
            background: rgba(0, 122, 204, 0.1);
        }

        .debug-link-item.broken {
            border-left-color: var(--error, #f14c4c);
            color: var(--error, #f14c4c);
        }

        .debug-vars {
            margin-top: 4px;
        }

        .debug-var-item {
            font-size: 10px;
            padding: 1px 0;
        }

        .debug-var-item .var-name { font-family: monospace; }
        .debug-var-item .var-kind {
            font-size: 9px;
            color: var(--muted);
            margin-left: 4px;
        }

        .shortcuts kbd {
            background: var(--bg);
            border: 1px solid var(--border);
            border-radius: 2px;
            padding: 0 3px;
            font-family: monospace;
            font-size: 9px;
        }
    </style>
</head>
<body>
    <!-- Navigation Toolbar -->
    <div class="toolbar">
        <button id="btn-restart" title="Restart story (Alt+R)">&#x21BB; Restart</button>
        <div class="separator"></div>
        <button id="btn-back" title="Go back (Alt+Left)" disabled>&#x2190; Back</button>
        <button id="btn-forward" title="Go forward (Alt+Right)" disabled>Forward &#x2192;</button>
        <div class="separator"></div>
        <button id="btn-rebuild" title="Rebuild story (Alt+B)">&#x21BB; Rebuild</button>
        <button id="btn-autorebuild" title="Toggle auto-rebuild on save">&#x1F504; Auto</button>
        <div class="separator"></div>
        <button id="btn-goto" title="Open current passage in editor (Alt+G)">&#x1F4DD; Edit</button>
        <span class="passage-indicator" id="passage-indicator">Starting...</span>
        <span class="build-indicator success" id="build-indicator">Ready</span>
    </div>

    <!-- Main area -->
    <div class="main">
        <!-- Story iframe -->
        <iframe id="story-frame" class="story-frame" srcdoc="${escaped}" sandbox="allow-scripts allow-same-origin"></iframe>

        <!-- History sidebar -->
        <div class="history-panel">
            <div class="history-header">
                Passage History
                <span class="history-count" id="history-count">0</span>
            </div>
            <div class="history-list" id="history-list"></div>
            <!-- Debug info section -->
            <div class="debug-section">
                <div class="debug-header" id="debug-toggle">
                    Debug Info &#x25B6;
                </div>
                <div class="debug-content" id="debug-content" style="display:none;">
                    <div class="debug-empty">Visit a passage to see debug info</div>
                </div>
            </div>
            <div class="shortcuts">
                <kbd>Alt</kbd>+<kbd>R</kbd> Restart &nbsp;
                <kbd>Alt</kbd>+<kbd>B</kbd> Rebuild<br>
                <kbd>Alt</kbd>+<kbd>&#x2190;</kbd> Back &nbsp;
                <kbd>Alt</kbd>+<kbd>&#x2192;</kbd> Forward<br>
                <kbd>Alt</kbd>+<kbd>G</kbd> Edit passage
            </div>
        </div>
    </div>

    <script>
        const vscode = acquireVsCodeApi();

        // Session state
        let sessionHistory = [];
        let sessionHistoryIndex = -1;
        let sessionAutoRebuild = true;

        // DOM elements
        const btnRestart = document.getElementById('btn-restart');
        const btnBack = document.getElementById('btn-back');
        const btnForward = document.getElementById('btn-forward');
        const btnRebuild = document.getElementById('btn-rebuild');
        const btnAutoRebuild = document.getElementById('btn-autorebuild');
        const btnGoto = document.getElementById('btn-goto');
        const passageIndicator = document.getElementById('passage-indicator');
        const buildIndicator = document.getElementById('build-indicator');
        const historyList = document.getElementById('history-list');
        const historyCount = document.getElementById('history-count');
        const storyFrame = document.getElementById('story-frame');

        // Button handlers
        btnRestart.addEventListener('click', () => {
            vscode.postMessage({ command: 'restart' });
        });

        btnBack.addEventListener('click', () => {
            vscode.postMessage({ command: 'goBack' });
        });

        btnForward.addEventListener('click', () => {
            vscode.postMessage({ command: 'goForward' });
        });

        btnRebuild.addEventListener('click', () => {
            vscode.postMessage({ command: 'restart' });
        });

        btnAutoRebuild.addEventListener('click', () => {
            vscode.postMessage({ command: 'toggleAutoRebuild' });
        });

        btnGoto.addEventListener('click', () => {
            if (sessionHistory.length > 0 && sessionHistoryIndex >= 0) {
                vscode.postMessage({ command: 'openPassage', name: sessionHistory[sessionHistoryIndex].name });
            }
        });

        // Keyboard shortcuts
        document.addEventListener('keydown', (e) => {
            if (e.altKey) {
                switch (e.key) {
                    case 'r':
                        e.preventDefault();
                        vscode.postMessage({ command: 'restart' });
                        break;
                    case 'b':
                        e.preventDefault();
                        vscode.postMessage({ command: 'restart' });
                        break;
                    case 'ArrowLeft':
                        e.preventDefault();
                        vscode.postMessage({ command: 'goBack' });
                        break;
                    case 'ArrowRight':
                        e.preventDefault();
                        vscode.postMessage({ command: 'goForward' });
                        break;
                    case 'g':
                        e.preventDefault();
                        if (sessionHistory.length > 0 && sessionHistoryIndex >= 0) {
                            vscode.postMessage({ command: 'openPassage', name: sessionHistory[sessionHistoryIndex].name });
                        }
                        break;
                }
            }
        });

        // Listen for messages from the extension
        window.addEventListener('message', (event) => {
            const message = event.data;
            switch (message.command) {
                case 'updateSession':
                    sessionHistory = message.data.history;
                    sessionHistoryIndex = message.data.historyIndex;
                    sessionAutoRebuild = message.data.autoRebuild;
                    updateUI();
                    break;
                case 'buildStatus':
                    updateBuildStatus(message.status);
                    break;
                case 'debugInfo':
                    updateDebugPanel(message.data, message.passage_name);
                    break;
            }
        });

        // Update UI based on session state
        function updateUI() {
            // Navigation buttons
            btnBack.disabled = sessionHistoryIndex <= 0;
            btnForward.disabled = sessionHistoryIndex >= sessionHistory.length - 1;

            // Passage indicator
            if (sessionHistory.length > 0 && sessionHistoryIndex >= 0) {
                passageIndicator.textContent = sessionHistory[sessionHistoryIndex].name;
            } else {
                passageIndicator.textContent = 'Starting...';
            }

            // Auto-rebuild button
            btnAutoRebuild.textContent = sessionAutoRebuild ? '\\u{1F504} Auto: ON' : '\\u{1F504} Auto: OFF';
            btnAutoRebuild.style.opacity = sessionAutoRebuild ? '1' : '0.5';

            // History list
            historyCount.textContent = String(sessionHistory.length);
            let html = '';
            for (let i = 0; i < sessionHistory.length; i++) {
                const item = sessionHistory[i];
                const isCurrent = i === sessionHistoryIndex;
                const cls = isCurrent ? 'history-item current' : 'history-item';
                const time = new Date(item.timestamp);
                const timeStr = time.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
                html += '<div class="' + cls + '" data-index="' + i + '">';
                html += '<span class="index">' + (i + 1) + '</span>';
                html += '<span class="name" title="' + esc(item.name) + '">' + esc(item.name) + '</span>';
                html += '<span class="time">' + timeStr + '</span>';
                html += '</div>';
            }
            historyList.innerHTML = html;

            // Scroll to current item
            const currentItem = historyList.querySelector('.current');
            if (currentItem) {
                currentItem.scrollIntoView({ block: 'nearest' });
            }

            // Click handlers for history items
            historyList.querySelectorAll('.history-item').forEach((el) => {
                el.addEventListener('click', () => {
                    const idx = parseInt(el.getAttribute('data-index'), 10);
                    if (idx >= 0 && idx < sessionHistory.length) {
                        // Navigate to this history position
                        // We can't actually navigate the Twine story to a specific
                        // passage from here, but we can update the indicator
                        sessionHistoryIndex = idx;
                        updateUI();
                    }
                });
            });
        }

        // Update build status indicator
        function updateBuildStatus(status) {
            switch (status) {
                case 'building':
                    buildIndicator.className = 'build-indicator building';
                    buildIndicator.textContent = 'Building...';
                    break;
                case 'success':
                    buildIndicator.className = 'build-indicator success';
                    buildIndicator.textContent = 'Ready';
                    break;
                case 'error':
                    buildIndicator.className = 'build-indicator error';
                    buildIndicator.textContent = 'Build Error';
                    break;
            }
        }

        // Update debug panel with passage debug info
        let debugVisible = false;
        const debugToggle = document.getElementById('debug-toggle');
        const debugContent = document.getElementById('debug-content');

        if (debugToggle) {
            debugToggle.addEventListener('click', () => {
                debugVisible = !debugVisible;
                debugContent.style.display = debugVisible ? 'block' : 'none';
                debugToggle.innerHTML = debugVisible ? 'Debug Info &#x25BC;' : 'Debug Info &#x25B6;';
            });
        }

        function updateDebugPanel(data, passageName) {
            if (!debugContent) return;

            // Auto-expand debug panel when info arrives
            if (!debugVisible) {
                debugVisible = true;
                debugContent.style.display = 'block';
                if (debugToggle) debugToggle.innerHTML = 'Debug Info &#x25BC;';
            }

            let html = '<div class="debug-passage-name">' + esc(passageName) + '</div>';

            // Reachability
            if (data.is_reachable !== undefined) {
                const reachClass = data.is_reachable ? 'ok' : 'warn';
                html += '<div class="debug-row"><span class="label">Reachable</span><span class="value ' + reachClass + '">' + (data.is_reachable ? 'Yes' : 'No') + '</span></div>';
            }

            // Links
            if (data.links && data.links.length > 0) {
                html += '<div class="debug-links">';
                html += '<div class="debug-row"><span class="label">Outgoing links</span><span class="value">' + data.links.length + '</span></div>';
                for (const link of data.links) {
                    const broken = link.is_broken ? ' broken' : '';
                    html += '<div class="debug-link-item' + broken + '" onclick="vscode.postMessage({command:\\'openPassage\\',name:\\'' + esc(link.target || link.name || '') + '\\'})">' + esc(link.target || link.name || link.display_text || 'unknown') + (link.display_text ? ' (' + esc(link.display_text) + ')' : '') + (link.is_broken ? ' &#x26A0; broken' : '') + '</div>';
                }
                html += '</div>';
            }

            // Variables
            if (data.variables && data.variables.length > 0) {
                html += '<div class="debug-vars">';
                html += '<div class="debug-row"><span class="label">Variables</span><span class="value">' + data.variables.length + '</span></div>';
                for (const v of data.variables) {
                    html += '<div class="debug-var-item"><span class="var-name">' + esc(v.name) + '</span><span class="var-kind">' + (v.kind || 'unknown') + '</span></div>';
                }
                html += '</div>';
            }

            // Loops
            if (data.in_loops && data.in_loops.length > 0) {
                html += '<div class="debug-row"><span class="label">In loops</span><span class="value warn">' + data.in_loops.length + '</span></div>';
            }

            // Dataflow
            if (data.initialized_at_entry) {
                const initVars = data.initialized_at_entry;
                if (initVars.length > 0) {
                    html += '<div class="debug-row"><span class="label">Initialized at entry</span><span class="value">' + initVars.length + '</span></div>';
                }
            }

            debugContent.innerHTML = html;
        }

        // Escape HTML
        function esc(str) {
            const div = document.createElement('div');
            div.textContent = str;
            return div.innerHTML;
        }

        // Passage tracking: Monitor the iframe for passage navigation events.
        // Different Twine formats use different mechanisms for passage transitions.
        // We try to detect them by:
        // 1. Watching for hash changes (some formats use #passage-name)
        // 2. Observing DOM mutations (Harlowe, SugarCube add passage elements)
        // 3. Listening for custom events
        storyFrame.addEventListener('load', () => {
            try {
                const iframeDoc = storyFrame.contentDocument;
                if (!iframeDoc) { return; }

                // Try to detect passage name from the DOM
                function detectCurrentPassage() {
                    // SugarCube: #passage-start or .passage element with data-name
                    const scPassage = iframeDoc.querySelector('.passage[data-name]');
                    if (scPassage) {
                        return scPassage.getAttribute('data-name');
                    }

                    // Harlowe: tw-passage element
                    const harlowePassage = iframeDoc.querySelector('tw-passage');
                    if (harlowePassage) {
                        return harlowePassage.getAttribute('name');
                    }

                    // Chapbook: section element with data-passage
                    const chapbookPassage = iframeDoc.querySelector('section[data-passage]');
                    if (chapbookPassage) {
                        return chapbookPassage.getAttribute('data-passage');
                    }

                    // Snowman: #passage container
                    const snowmanContent = iframeDoc.querySelector('#passage');
                    if (snowmanContent) {
                        // Snowman doesn't expose passage name directly, try hash
                        const hash = iframeDoc.location?.hash;
                        if (hash && hash.length > 1) {
                            return decodeURIComponent(hash.substring(1));
                        }
                    }

                    // Fallback: hash-based detection
                    const hash = iframeDoc.location?.hash;
                    if (hash && hash.length > 1) {
                        return decodeURIComponent(hash.substring(1));
                    }

                    return null;
                }

                // Observe DOM mutations for passage transitions
                const observer = new MutationObserver(() => {
                    const passageName = detectCurrentPassage();
                    if (passageName) {
                        vscode.postMessage({ command: 'passageVisited', name: passageName });
                    }
                });

                // Observe the iframe body for changes
                if (iframeDoc.body) {
                    observer.observe(iframeDoc.body, {
                        childList: true,
                        subtree: true,
                    });
                }

                // Also watch for hash changes
                try {
                    storyFrame.contentWindow.addEventListener('hashchange', () => {
                        const passageName = detectCurrentPassage();
                        if (passageName) {
                            vscode.postMessage({ command: 'passageVisited', name: passageName });
                        }
                    });
                } catch (e) {
                    // Cross-origin restrictions may prevent this
                }

                // Detect the initial passage
                const initialPassage = detectCurrentPassage();
                if (initialPassage) {
                    vscode.postMessage({ command: 'passageVisited', name: initialPassage });
                }
            } catch (e) {
                // Cross-origin restrictions may prevent iframe access
                // This is expected in some configurations
            }
        });
    </script>
</body>
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
