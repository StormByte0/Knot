//! Task Provider for Knot build and watch tasks.
//!
//! Registers `knot` task type with VS Code's task system, providing:
//! - **Build Story**: Compiles the project via Tweego
//! - **Watch & Rebuild**: Auto-rebuilds on file saves

import * as vscode from 'vscode';
import * as path from 'path';
import { KnotLanguageClient, KnotBuildResponse } from './types';
import { getBuildRequestParams } from './utils';

// ---------------------------------------------------------------------------
// Task Provider
// ---------------------------------------------------------------------------

/** Register a Task Provider for Knot build and watch tasks. */
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

            // Watch task — auto-rebuild on file changes
            const watchTask = new vscode.Task(
                { type: 'knot', task: 'watch' },
                vscode.TaskScope.Workspace,
                'Watch & Rebuild',
                'knot',
                new vscode.CustomExecution(async () => {
                    return new KnotWatchTerminal(client);
                }),
                '$(eye)'
            );
            watchTask.group = vscode.TaskGroup.Build;
            watchTask.isBackground = true;
            watchTask.presentationOptions = {
                reveal: vscode.TaskRevealKind.Silent,
                panel: vscode.TaskPanelKind.Dedicated,
                clear: false,
            };
            result.push(watchTask);

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

/** Custom terminal for the Knot watch task. */
class KnotWatchTerminal implements vscode.Pseudoterminal {
    private writeEmitter = new vscode.EventEmitter<string>();
    onDidWrite: vscode.Event<string> = this.writeEmitter.event;
    private closeEmitter = new vscode.EventEmitter<number>();
    onDidClose?: vscode.Event<number> = this.closeEmitter.event;
    private watcher: vscode.Disposable | null = null;

    constructor(private client: KnotLanguageClient) {}

    async open(): Promise<void> {
        this.writeEmitter.fire('Knot watch mode started. Saving a .tw/.twee file will trigger a rebuild.\r\n');
        this.writeEmitter.fire('Press Ctrl+C to stop.\r\n\r\n');

        this.watcher = vscode.workspace.onDidSaveTextDocument(async (doc) => {
            const ext = path.extname(doc.fileName).toLowerCase();
            if (ext !== '.tw' && ext !== '.twee') { return; }

            this.writeEmitter.fire(`[${new Date().toLocaleTimeString()}] File saved: ${path.basename(doc.fileName)} — rebuilding...\r\n`);

            if (!this.client.isRunning()) {
                this.writeEmitter.fire('  Error: Language server not running\r\n');
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) { return; }

            try {
                const result = await this.client.sendRequest<KnotBuildResponse>('knot/build',
                    getBuildRequestParams(workspaceFolders[0].uri.toString())
                );
                if (result.success) {
                    this.writeEmitter.fire('  Build succeeded.\r\n');
                } else {
                    this.writeEmitter.fire('  Build FAILED.\r\n');
                }
            } catch (e) {
                this.writeEmitter.fire(`  Build failed: ${e}\r\n`);
            }
        });
    }

    close(): void {
        if (this.watcher) {
            this.watcher.dispose();
            this.watcher = null;
        }
    }
}
