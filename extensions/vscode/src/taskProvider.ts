//! Task Provider for Knot build tasks.
//!
//! Registers `knot` task type with VS Code's task system, providing:
//! - **Build Story**: Compiles the project via Tweego
//!
//! The Watch & Rebuild functionality is now handled by the status bar
//! Watch toggle (see `watchState.ts` and `statusBarItems.ts`), which is
//! simpler and doesn't require the task panel UX.

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotBuildResponse } from './types';
import { getBuildRequestParams } from './utils';

// ---------------------------------------------------------------------------
// Task Provider
// ---------------------------------------------------------------------------

/** Register a Task Provider for Knot build tasks. */
export function registerTaskProvider(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient,
): void {
    const provider: vscode.TaskProvider = {
        provideTasks(): vscode.Task[] {
            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                return [];
            }

            const result: vscode.Task[] = [];

            // Build task — equivalent to `knot.build` command
            const buildTask = new vscode.Task(
                { type: 'knot', task: 'build' },
                vscode.TaskScope.Workspace,
                'Build Story',
                'knot',
                new vscode.CustomExecution(async () => {
                    return new KnotBuildTerminal(client);
                }),
                '$(graph)'
            );
            buildTask.group = vscode.TaskGroup.Build;
            buildTask.presentationOptions = {
                reveal: vscode.TaskRevealKind.Always,
                panel: vscode.TaskPanelKind.Dedicated,
                clear: true,
            };
            result.push(buildTask);

            return result;
        },

        resolveTask(task: vscode.Task): vscode.Task {
            return task;
        },
    };

    context.subscriptions.push(
        vscode.tasks.registerTaskProvider('knot', provider)
    );
}

// ---------------------------------------------------------------------------
// Custom terminals
// ---------------------------------------------------------------------------

/** Custom terminal for the Knot build task. */
class KnotBuildTerminal implements vscode.Pseudoterminal {
    private writeEmitter = new vscode.EventEmitter<string>();
    onDidWrite: vscode.Event<string> = this.writeEmitter.event;
    private closeEmitter = new vscode.EventEmitter<number>();
    onDidClose?: vscode.Event<number> = this.closeEmitter.event;

    constructor(private client: KnotLanguageClient) {}

    async open(): Promise<void> {
        this.writeEmitter.fire('Starting Knot build...\r\n');

        if (!this.client.isRunning()) {
            this.writeEmitter.fire('Error: Knot language server is not running.\r\n');
            this.closeEmitter.fire(1);
            return;
        }

        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders || workspaceFolders.length === 0) {
            this.writeEmitter.fire('Error: No workspace folder open.\r\n');
            this.closeEmitter.fire(1);
            return;
        }

        try {
            const result = await this.client.sendRequest<KnotBuildResponse>('knot/build',
                getBuildRequestParams(workspaceFolders[0].uri.toString())
            );

            if (result.success) {
                this.writeEmitter.fire('Build succeeded!\r\n');
                if (result.output_path) {
                    this.writeEmitter.fire(`Output: ${result.output_path}\r\n`);
                }
                this.closeEmitter.fire(0);
            } else {
                this.writeEmitter.fire('Build FAILED!\r\n');
                if (result.errors) {
                    for (const err of result.errors) {
                        this.writeEmitter.fire(`  ${err}\r\n`);
                    }
                }
                this.closeEmitter.fire(1);
            }
        } catch (e) {
            this.writeEmitter.fire(`Build request failed: ${e}\r\n`);
            this.closeEmitter.fire(1);
        }
    }

    close(): void {}
}
