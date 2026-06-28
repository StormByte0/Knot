//! Shared state for the Watch toggle.
//!
//! The Watch status bar item toggles this state; the Play command reads it
//! to decide whether to build before opening the browser. Kept as a simple
//! module-level singleton rather than a full provider because the state is
//! trivial (one boolean) and only accessed from two places.

/** Whether the background save watcher is currently active. */
let _watchActive = false;

/** Save watcher disposable — set when Watch is toggled on. */
let _watcherDisposable: vscode.Disposable | null = null;

/** Build output channel — set during activation so the watcher can log. */
let _buildOutputChannel: vscode.OutputChannel | null = null;

import * as vscode from 'vscode';
import * as path from 'path';
import { getBuildRequestParams } from './utils';
import { KnotLanguageClient, KnotBuildResponse } from './types';

/** Set the build output channel (called once during activation). */
export function setBuildOutputChannel(channel: vscode.OutputChannel): void {
    _buildOutputChannel = channel;
}

/** Returns true if the background save watcher is active. */
export function isWatchActive(): boolean {
    return _watchActive;
}

/**
 * Start the background save watcher. Idempotent — does nothing if already
 * active. Watches .tw/.twee/.js/.css files and triggers a build on save.
 */
export function startWatch(client: KnotLanguageClient | null): void {
    if (_watchActive) { return; }
    _watchActive = true;

    _watcherDisposable = vscode.workspace.onDidSaveTextDocument(async (doc) => {
        const ext = path.extname(doc.fileName).toLowerCase();
        // Watch all source file types that tweego bundles
        if (!['.tw', '.twee', '.js', '.css', '.html', '.htm'].includes(ext)) {
            return;
        }

        if (!client || !client.isRunning()) {
            _buildOutputChannel?.appendLine('[watch] Language server not running — skipping rebuild');
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) { return; }

        const timestamp = new Date().toLocaleTimeString();
        _buildOutputChannel?.appendLine(`[watch ${timestamp}] File saved: ${path.basename(doc.fileName)} — rebuilding...`);

        try {
            const result = await client.sendRequest<KnotBuildResponse>(
                'knot/build',
                getBuildRequestParams(workspaceFolders[0].uri.toString()),
            );
            if (result.success) {
                _buildOutputChannel?.appendLine(`[watch ${timestamp}] Build succeeded.`);
            } else {
                _buildOutputChannel?.appendLine(`[watch ${timestamp}] Build FAILED:`);
                for (const err of result.errors) {
                    _buildOutputChannel?.appendLine(`  ${err}`);
                }
                _buildOutputChannel?.show(true);
            }
        } catch (e) {
            _buildOutputChannel?.appendLine(`[watch ${timestamp}] Build request failed: ${e}`);
            _buildOutputChannel?.show(true);
        }
    });
}

/**
 * Stop the background save watcher. Idempotent — does nothing if already
 * inactive.
 */
export function stopWatch(): void {
    if (!_watchActive) { return; }
    _watchActive = false;
    if (_watcherDisposable) {
        _watcherDisposable.dispose();
        _watcherDisposable = null;
    }
    _buildOutputChannel?.appendLine('[watch] Stopped.');
}

/** Toggle the watch state. Returns the new state. */
export function toggleWatch(client: KnotLanguageClient | null): boolean {
    if (_watchActive) {
        stopWatch();
    } else {
        startWatch(client);
    }
    return _watchActive;
}
