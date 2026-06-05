//! Language Status API integration for Knot.
//!
//! Registers the native VS Code Language Status indicator that shows
//! the detected story format, passage count, and issue summary in
//! the editor's status area.

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotProfileResponse } from './types';

/**
 * Register the native Language Status indicator for Twee files.
 *
 * Returns the created `LanguageStatusItem` so that `extension.ts`
 * can store it and update it from notification handlers.
 */
export function registerLanguageStatus(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient,
): vscode.LanguageStatusItem {
    const statusItem = vscode.languages.createLanguageStatusItem('knot.status', { language: 'twee' });
    statusItem.name = 'Knot IDE';
    statusItem.text = '$(sync~spin) Knot';
    statusItem.detail = 'Starting...';
    statusItem.severity = vscode.LanguageStatusSeverity.Information;
    statusItem.command = {
        title: 'Open Story Map',
        command: 'knot.openStoryMap',
    };
    context.subscriptions.push(statusItem);

    // Periodic refresh for profile data
    const statusRefreshInterval = setInterval(async () => {
        if (!client.isRunning()) { return; }
        try {
            const wsFolders = vscode.workspace.workspaceFolders;
            if (wsFolders && wsFolders.length > 0) {
                const profile = await client.sendRequest<KnotProfileResponse>('knot/profile', {
                    workspace_uri: wsFolders[0].uri.toString(),
                });
                const fmt = profile.format || 'Unknown';
                const passages = profile.passage_count || 0;
                const brokenLinks = profile.broken_link_count || 0;
                const unreachable = profile.unreachable_passage_count || 0;

                statusItem.text = `$(graph) ${fmt}`;
                statusItem.detail = `${passages} passages · ${brokenLinks} broken · ${unreachable} unreachable`;

                if (brokenLinks > 0) {
                    statusItem.severity = vscode.LanguageStatusSeverity.Warning;
                } else {
                    statusItem.severity = vscode.LanguageStatusSeverity.Information;
                }
            }
        } catch {
            // Silently ignore
        }
    }, 30000); // Refresh every 30 seconds

    context.subscriptions.push({ dispose: () => clearInterval(statusRefreshInterval) });

    return statusItem;
}
