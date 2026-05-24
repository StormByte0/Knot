//! Sidebar launch card for the Story Map.
//!
//! This implements a WebviewView that shows an "Open Story Map" card at the
//! top of the Knot sidebar. Per the v3 design spec, this is the primary
//! discoverable access point for the graph view, modeled after VS Code's
//! "Open Folder" empty-state pattern.
//!
//! The launch card sits above the Passage Diagnostics, Variable Tracking,
//! and Workspace Profile tabs in the sidebar.

import * as vscode from 'vscode';

export class StoryMapLaunchProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'knot.storyMapLaunch';

    private _view?: vscode.WebviewView;

    constructor(private readonly _extensionUri: vscode.Uri) {}

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
                case 'openStoryMap': {
                    await vscode.commands.executeCommand('knot.openStoryMap');
                    break;
                }
            }
        });
    }

    private _getHtmlForWebview(webview: vscode.Webview): string {
        const nonce = getNonce();

        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src ${webview.cspSource} 'nonce-${nonce}';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Story Map Launch</title>
    <style>
        :root {
            --launch-card-bg: var(--vscode-button-secondaryBackground, #3a3d41);
            --launch-card-hover: var(--vscode-button-secondaryHoverBackground, #45494e);
            --launch-card-fg: var(--vscode-button-secondaryForeground, #ffffff);
            --launch-card-radius: 6px;
        }
        body {
            margin: 0;
            padding: 12px;
            background: var(--vscode-editor-background, #1e1e1e);
            color: var(--vscode-editor-foreground, #d4d4d4);
            font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif);
            font-size: 13px;
        }
        .launch-card {
            background: var(--launch-card-bg);
            color: var(--launch-card-fg);
            border-radius: var(--launch-card-radius);
            padding: 12px 16px;
            cursor: pointer;
            display: flex;
            align-items: center;
            gap: 10px;
            transition: background 0.15s ease;
            user-select: none;
        }
        .launch-card:hover {
            background: var(--launch-card-hover);
        }
        .launch-card:active {
            opacity: 0.9;
        }
        .launch-icon {
            width: 20px;
            height: 20px;
            flex-shrink: 0;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .launch-icon svg {
            width: 18px;
            height: 18px;
            fill: currentColor;
        }
        .launch-text {
            display: flex;
            flex-direction: column;
        }
        .launch-title {
            font-weight: 600;
            font-size: 13px;
            line-height: 1.3;
        }
        .launch-subtitle {
            font-size: 11px;
            opacity: 0.8;
            line-height: 1.3;
            margin-top: 2px;
        }
    </style>
</head>
<body>
    <div class="launch-card" id="launchBtn">
        <span class="launch-icon">
            <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                <path d="M1 3a2 2 0 012-2h10a2 2 0 012 2v10a2 2 0 01-2 2H3a2 2 0 01-2-2V3zm2-1a1 1 0 00-1 1v10a1 1 0 001 1h10a1 1 0 001-1V3a1 1 0 00-1-1H3zm1 2h8v1H4V4zm0 3h8v1H4V7zm0 3h5v1H4v-1z"/>
            </svg>
        </span>
        <span class="launch-text">
            <span class="launch-title">Open Story Map</span>
            <span class="launch-subtitle">Visualize and navigate story structure</span>
        </span>
    </div>
    <script nonce="${nonce}">
        const vscode = acquireVsCodeApi();
        document.getElementById('launchBtn').addEventListener('click', () => {
            vscode.postMessage({ command: 'openStoryMap' });
        });
    </script>
</body>
</html>`;
    }
}

function getNonce(): string {
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    let nonce = '';
    for (let i = 0; i < 32; i++) {
        nonce += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return nonce;
}
