//! Crash recovery for the Knot language server.
//!
//! Handles automatic restart attempts and user-facing prompts when
//! the server process crashes during operation.

import * as vscode from 'vscode';
import { KnotLanguageClient } from './types';

/** Maximum number of automatic restart attempts before giving up. */
export const MAX_CRASH_RETRIES = 3;

/** Mutable crash counter — shared across this module and extension.ts. */
let crashCount = 0;

/** Get the current crash count (for external checks). */
export function getCrashCount(): number {
    return crashCount;
}

/** Reset the crash counter (e.g., after a successful start). */
export function resetCrashCount(): void {
    crashCount = 0;
}

export interface CrashRecoveryDeps {
    client: KnotLanguageClient;
    statusBarItem: vscode.StatusBarItem;
}

/**
 * Handle a language server crash with automatic restart and user prompts.
 *
 * - On the first few crashes, attempt an automatic restart after a 2s delay.
 * - After `MAX_CRASH_RETRIES`, prompt the user to restart or disable the server.
 */
export function handleServerFailure(
    error: unknown,
    context: vscode.ExtensionContext,
    serverPath: string,
    deps: CrashRecoveryDeps,
): void {
    crashCount++;
    const errorMsg = error instanceof Error ? error.message : String(error);
    tracing('Server crashed: ' + errorMsg);

    if (crashCount >= MAX_CRASH_RETRIES) {
        vscode.window.showErrorMessage(
            `Knot: Language server has crashed ${crashCount} times. ` +
            'Advanced analysis is disabled. Click to restart.',
            'Restart',
            'Disable'
        ).then(choice => {
            if (choice === 'Restart') {
                crashCount = 0;
                vscode.commands.executeCommand('knot.restartServer');
            } else if (choice === 'Disable') {
                vscode.workspace
                    .getConfiguration('knot')
                    .update('experimental.rustServer', false, vscode.ConfigurationTarget.Global);
            }
        });

        deps.statusBarItem.text = '$(error) Knot: Server crashed';
        deps.statusBarItem.show();
    } else {
        vscode.window.showWarningMessage(
            `Knot: Language server crashed (attempt ${crashCount}/${MAX_CRASH_RETRIES}). Restarting...`
        );

        // Attempt automatic restart
        setTimeout(async () => {
            try {
                await deps.client.start();
                crashCount = 0;
            } catch {
                handleServerFailure('Restart failed', context, serverPath, deps);
            }
        }, 2000);
    }
}

function tracing(message: string) {
    const config = vscode.workspace.getConfiguration('knot').get<string>('trace.server', 'off');
    if (config !== 'off') {
        console.log(`[Knot] ${message}`);
    }
}
