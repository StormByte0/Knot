//! VS Code extension entry point for the Knot language server.
//!
//! Handles:
//! - Binary bootstrap and crash recovery
//! - Language client setup
//! - Custom command registration
//! - Status bar for indexing progress
//! - Story Map webview (sidebar panel)
//! - Play Mode (in-editor story preview)
//! - Debug View (passage inspection)
//! - Profile View (workspace statistics)
//! - Decorations API (gutter badges, faded unreachable, link highlights)
//! - Language Status API (native language status indicator)
//! - Task Provider (build/watch task integration)

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { StoryMapProvider } from './storyMapProvider';
import { PlayModeProvider } from './playModeProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

/** Map VS Code platform to the Knot server binary name. */
function getPlatformBinary(): string | null {
    const platform = process.platform;
    const arch = process.arch;

    if (platform === 'win32' && arch === 'x64') return 'knot-server.exe';
    if (platform === 'darwin' && arch === 'arm64') return 'knot-server';
    if (platform === 'darwin' && arch === 'x64') return 'knot-server';
    if (platform === 'linux' && arch === 'x64') return 'knot-server';
    if (platform === 'linux' && arch === 'arm64') return 'knot-server';

    return null;
}

/** Resolve the path to the knot-server binary. */
async function getServerPath(context: vscode.ExtensionContext): Promise<string | null> {
    // Check user override first
    const configPath = vscode.workspace.getConfiguration('knot').get<string>('server.path');
    if (configPath && configPath.trim() !== '') {
        if (fs.existsSync(configPath)) {
            return configPath;
        }
        vscode.window.showWarningMessage(
            `Knot: Configured server path does not exist: ${configPath}`
        );
    }

    // Use bundled binary
    const binaryName = getPlatformBinary();
    if (!binaryName) {
        vscode.window.showWarningMessage(
            `Knot: No native binary available for ${process.platform}-${process.arch}. ` +
            'Falling back to TextMate grammar highlighting only.'
        );
        return null;
    }

    const serverPath = path.join(context.extensionPath, 'bin', binaryName);
    if (!fs.existsSync(serverPath)) {
        vscode.window.showWarningMessage(
            `Knot: Server binary not found at ${serverPath}. ` +
            'Falling back to TextMate grammar highlighting only.'
        );
        return null;
    }

    return serverPath;
}

// ---------------------------------------------------------------------------
// Extension activation
// ---------------------------------------------------------------------------

let client: any = null;
let crashCount = 0;
const MAX_CRASH_RETRIES = 3;
let statusBarItem: vscode.StatusBarItem | null = null;
let storyMapProvider: StoryMapProvider | null = null;
let playModeProvider: PlayModeProvider | null = null;
let debugViewProvider: DebugViewProvider | null = null;
let profileViewProvider: ProfileViewProvider | null = null;
let buildOutputChannel: vscode.OutputChannel | null = null;
let languageStatusItem: vscode.LanguageStatusItem | null = null;
let passageDecorationType: vscode.TextEditorDecorationType | null = null;
let unreachableDecorationType: vscode.TextEditorDecorationType | null = null;
let linkDecorationType: vscode.TextEditorDecorationType | null = null;

export async function activate(context: vscode.ExtensionContext) {
    const useRustServer = vscode.workspace
        .getConfiguration('knot')
        .get<boolean>('experimental.rustServer', true);

    if (!useRustServer) {
        vscode.window.showInformationMessage(
            'Knot: Rust language server is disabled. Using TextMate grammar only.'
        );
        return;
    }

    const serverPath = await getServerPath(context);
    if (!serverPath) {
        return;
    }

    // Create status bar item for indexing progress
    statusBarItem = vscode.window.createStatusBarItem(
        vscode.StatusBarAlignment.Left,
        50
    );
    statusBarItem.text = '$(sync~spin) Knot: Starting...';
    statusBarItem.show();
    context.subscriptions.push(statusBarItem);

    const serverOptions: any = {
        command: serverPath,
        args: ['--stdio'],
    };

    const clientOptions: any = {
        documentSelector: [
            { scheme: 'file', language: 'twee' },
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher(
                '**/*.{tw,twee}'
            ),
        },
    };

    // We need to import LanguageClient dynamically since the types
    // are from vscode-languageclient
    const { LanguageClient } = await import('vscode-languageclient');

    client = new LanguageClient(
        'knot',
        'Knot Language Server',
        serverOptions,
        clientOptions
    );

    // Register custom notification handler for knot/indexProgress
    client.onNotification(
        { method: 'knot/indexProgress' },
        (params: { total_files: number; parsed_files: number }) => {
            if (statusBarItem) {
                if (params.parsed_files < params.total_files) {
                    statusBarItem.text = `$(sync~spin) Knot: Indexing ${params.parsed_files}/${params.total_files}`;
                } else {
                    statusBarItem.text = '$(check) Knot: Ready';
                    statusBarItem.command = 'knot.openStoryMap';
                    statusBarItem.tooltip = 'Knot: Click to open Story Map';
                    // Fetch profile data for status bar enrichment
                    (async () => {
                        try {
                            const wsFolders = vscode.workspace.workspaceFolders;
                            if (wsFolders && wsFolders.length > 0) {
                                const profile: any = await client.sendRequest('knot/profile', {
                                    workspace_uri: wsFolders[0].uri.toString(),
                                });
                                if (statusBarItem) {
                                    const fmt = profile.format || 'Unknown';
                                    const passages = profile.passage_count || 0;
                                    statusBarItem.text = `$(graph) Knot: ${fmt} | ${passages} passages`;
                                    statusBarItem.tooltip = `Knot IDE — ${fmt}${profile.format_version ? ' v' + profile.format_version : ''} | ${passages} passages, ${profile.total_word_count || 0} words`;
                                }
                            }
                        } catch {
                            // If profile fetch fails, use default status
                            if (statusBarItem) {
                                statusBarItem.text = '$(graph) Knot';
                            }
                        }
                    })();
                }
            }
        }
    );

    // Register custom notification handler for knot/buildOutput
    client.onNotification(
        { method: 'knot/buildOutput' },
        (params: { line: string; is_error: boolean }) => {
            if (buildOutputChannel) {
                buildOutputChannel.appendLine(params.line);
                if (params.is_error) {
                    buildOutputChannel.show(true);
                }
            }
        }
    );

    // Register the Story Map webview provider (sidebar panel)
    storyMapProvider = new StoryMapProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            StoryMapProvider.viewType,
            storyMapProvider,
        )
    );

    // Register the Debug View webview provider (sidebar panel)
    debugViewProvider = new DebugViewProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            DebugViewProvider.viewType,
            debugViewProvider,
        )
    );

    // Register the Profile View webview provider (sidebar panel)
    profileViewProvider = new ProfileViewProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            ProfileViewProvider.viewType,
            profileViewProvider,
        )
    );

    // Create the Play Mode provider
    playModeProvider = new PlayModeProvider(context.extensionUri);

    // Create the build output channel
    buildOutputChannel = vscode.window.createOutputChannel('Knot Build');
    context.subscriptions.push(buildOutputChannel);

    try {
        await client.start();
        crashCount = 0;

        // Wire up the story map to the language client
        if (storyMapProvider) {
            storyMapProvider.setClient(client);
        }
        if (debugViewProvider) {
            debugViewProvider.setClient(client);
        }
        if (profileViewProvider) {
            profileViewProvider.setClient(client);
        }
        if (playModeProvider) {
            playModeProvider.setClient(client);
        }
    } catch (e) {
        handleServerFailure(e, context, serverPath);
    }

    // Register Language Status API
    registerLanguageStatus(context);

    // Register Decorations API
    registerDecorations(context);

    // Register Task Provider
    registerTaskProvider(context);

    // Register commands
    registerCommands(context);

    // Auto-refresh the Story Map when Twee files change
    const watcher = vscode.workspace.createFileSystemWatcher('**/*.{tw,twee}');
    watcher.onDidChange(() => refreshStoryMap());
    watcher.onDidCreate(() => refreshStoryMap());
    watcher.onDidDelete(() => refreshStoryMap());
    context.subscriptions.push(watcher);

    // Also refresh on active editor change (for live updates)
    vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (editor && editor.document.languageId === 'twee') {
            refreshStoryMap();
            // Update debug view with passage under cursor
            updateDebugViewForEditor(editor);
        }
    });

    // Update debug view on cursor position change
    vscode.window.onDidChangeTextEditorSelection((event) => {
        if (event.textEditor.document.languageId === 'twee') {
            updateDebugViewForEditor(event.textEditor);
        }
    });
}

/** Refresh the Story Map webview if it's visible. */
function refreshStoryMap() {
    if (storyMapProvider) {
        storyMapProvider.refreshGraph();
    }
}

// ---------------------------------------------------------------------------
// Command registration
// ---------------------------------------------------------------------------

function registerCommands(context: vscode.ExtensionContext) {
    // Open Story Map (focuses the sidebar panel)
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openStoryMap', async () => {
            // Focus the Story Map view in the sidebar
            await vscode.commands.executeCommand('knot.storyMap.focus');
            // Trigger a graph refresh
            refreshStoryMap();
        })
    );

    // Build Project
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.build', async () => {
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                vscode.window.showWarningMessage('Knot: No workspace folder open.');
                return;
            }

            try {
                const result = await client.sendRequest('knot/build', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                });
                if (result.success) {
                    vscode.window.showInformationMessage('Knot: Build succeeded!');
                } else {
                    vscode.window.showErrorMessage(
                        `Knot: Build failed: ${result.errors?.join(', ') || 'unknown error'}`
                    );
                }
            } catch (e) {
                vscode.window.showErrorMessage(`Knot: Build request failed: ${e}`);
            }
        })
    );

    // Play Story
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.play', async () => {
            if (!playModeProvider) {
                playModeProvider = new PlayModeProvider(context.extensionUri);
                if (client) {
                    playModeProvider.setClient(client);
                }
            }
            await playModeProvider.show();
        })
    );

    // Play from Passage — start play from a specific passage
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.playFromPassage', async (passageName?: string) => {
            // If no passage name provided, try to detect from cursor position
            if (!passageName) {
                const editor = vscode.window.activeTextEditor;
                if (editor) {
                    const text = editor.document.getText();
                    const position = editor.selection.active;
                    // Find passage header at or before cursor
                    const lines = text.split('\n');
                    let currentPassage: string | undefined;
                    for (let i = 0; i <= position.line; i++) {
                        const line = lines[i];
                        if (line.startsWith('::')) {
                            // Parse passage name (remove tags if present)
                            let name = line.substring(2).trim();
                            const bracketIdx = name.indexOf('[');
                            if (bracketIdx > 0) {
                                name = name.substring(0, bracketIdx).trim();
                            }
                            currentPassage = name;
                        }
                    }
                    passageName = currentPassage;
                }
            }

            if (!passageName) {
                vscode.window.showWarningMessage('Knot: No passage found at cursor position.');
                return;
            }

            if (!playModeProvider) {
                playModeProvider = new PlayModeProvider(context.extensionUri);
                if (client) {
                    playModeProvider.setClient(client);
                }
            }
            await playModeProvider.show(passageName);
        })
    );

    // Toggle Auto-Rebuild in Play Mode
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.toggleAutoRebuild', async () => {
            if (playModeProvider) {
                playModeProvider.toggleAutoRebuild();
            } else {
                vscode.window.showInformationMessage('Knot: Play mode is not active.');
            }
        })
    );

    // Restart Language Server
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.restartServer', async () => {
            if (client) {
                try {
                    await client.stop();
                } catch {
                    // Ignore errors during stop
                }
                client = null;
            }

            // Re-activate
            if (statusBarItem) {
                statusBarItem.text = '$(sync~spin) Knot: Restarting...';
                statusBarItem.show();
            }

            vscode.commands.executeCommand('workbench.action.reloadWindow');
        })
    );

    // Re-index Workspace
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.reindexWorkspace', async () => {
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            if (statusBarItem) {
                statusBarItem.text = '$(sync~spin) Knot: Re-indexing...';
                statusBarItem.show();
            }

            // Restart the server to force a full re-index
            try {
                await client.stop();
            } catch {
                // Ignore
            }
            client = null;
            vscode.commands.executeCommand('workbench.action.reloadWindow');
        })
    );

    // Detect Compiler
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.detectCompiler', async () => {
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                vscode.window.showWarningMessage('Knot: No workspace folder open.');
                return;
            }

            try {
                const result = await client.sendRequest('knot/compilerDetect', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                });
                if (result.compiler_found) {
                    vscode.window.showInformationMessage(
                        `Knot: Compiler found — ${result.compiler_name} ${result.compiler_version || ''} at ${result.compiler_path}`
                    );
                } else {
                    vscode.window.showWarningMessage(
                        'Knot: No Twine compiler found. Install Tweego and add it to PATH, or set compiler_path in .vscode/knot.json'
                    );
                }
            } catch (e) {
                vscode.window.showErrorMessage(`Knot: Compiler detection failed: ${e}`);
            }
        })
    );

    // Open Passage by Name (used by debug view)
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openPassageByName', async (passageName: string) => {
            if (!client || !client.isRunning()) {
                return;
            }
            // Search for the passage across all open documents
            for (const doc of vscode.workspace.textDocuments) {
                if (doc.languageId !== 'twee') { continue; }
                const text = doc.getText();
                for (let i = 0; i < text.lineCount; i++) {
                    const line = text.lineAt(i).text;
                    if (line.startsWith('::')) {
                        const name = line.substring(2).trim().split('[')[0].trim();
                        if (name === passageName) {
                            const editor = await vscode.window.showTextDocument(doc, {
                                preview: true,
                                selection: new vscode.Range(i, 0, i, line.length),
                            });
                            return;
                        }
                    }
                }
            }
            vscode.window.showWarningMessage(`Knot: Passage '${passageName}' not found in open documents.`);
        })
    );
}

// ---------------------------------------------------------------------------
// Language Status API
// ---------------------------------------------------------------------------

/** Register the native Language Status indicator for Twee files. */
function registerLanguageStatus(context: vscode.ExtensionContext) {
    languageStatusItem = vscode.languages.createLanguageStatusItem('knot.status', { language: 'twee' });
    languageStatusItem.name = 'Knot IDE';
    languageStatusItem.text = '$(sync~spin) Knot';
    languageStatusItem.detail = 'Starting...';
    languageStatusItem.severity = vscode.LanguageStatusSeverity.Information;
    languageStatusItem.command = {
        title: 'Open Story Map',
        command: 'knot.openStoryMap',
    };
    context.subscriptions.push(languageStatusItem);

    // Update language status when indexing progress arrives
    const origOnNotification = client.onNotification;
    // The client is already set up with knot/indexProgress handler above,
    // so we update language status from there
    // We also set up a periodic refresh for profile data
    const statusRefreshInterval = setInterval(async () => {
        if (!client || !client.isRunning() || !languageStatusItem) { return; }
        try {
            const wsFolders = vscode.workspace.workspaceFolders;
            if (wsFolders && wsFolders.length > 0) {
                const profile: any = await client.sendRequest('knot/profile', {
                    workspace_uri: wsFolders[0].uri.toString(),
                });
                const fmt = profile.format || 'Unknown';
                const passages = profile.passage_count || 0;
                const brokenLinks = profile.broken_link_count || 0;
                const unreachable = profile.unreachable_passage_count || 0;

                languageStatusItem.text = `$(graph) ${fmt}`;
                languageStatusItem.detail = `${passages} passages · ${brokenLinks} broken · ${unreachable} unreachable`;

                if (brokenLinks > 0) {
                    languageStatusItem.severity = vscode.LanguageStatusSeverity.Warning;
                } else {
                    languageStatusItem.severity = vscode.LanguageStatusSeverity.Information;
                }
            }
        } catch {
            // Silently ignore
        }
    }, 30000); // Refresh every 30 seconds

    context.subscriptions.push({ dispose: () => clearInterval(statusRefreshInterval) });
}

// ---------------------------------------------------------------------------
// Decorations API
// ---------------------------------------------------------------------------

/** Register editor decorations for Twee files. */
function registerDecorations(context: vscode.ExtensionContext) {
    // Gutter badge for passage headers — small colored circle
    passageDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: context.asAbsolutePath('media/passage-icon.svg'),
        gutterIconSize: 'auto',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
        overviewRulerColor: 'rgba(79, 195, 247, 0.5)', // Light blue
    });
    context.subscriptions.push(passageDecorationType);

    // Faded text for unreachable passages
    unreachableDecorationType = vscode.window.createTextEditorDecorationType({
        opacity: '0.4',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
        overviewRulerColor: 'rgba(102, 102, 102, 0.4)', // Gray
    });
    context.subscriptions.push(unreachableDecorationType);

    // Underline for broken links
    linkDecorationType = vscode.window.createTextEditorDecorationType({
        textDecoration: 'underline wavy rgba(241, 76, 76, 0.6)', // Red wavy
        overviewRulerLane: vscode.OverviewRulerLane.Right,
        overviewRulerColor: 'rgba(241, 76, 76, 0.6)',
    });
    context.subscriptions.push(linkDecorationType);

    // Update decorations on active editor change and document changes
    vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (editor && editor.document.languageId === 'twee') {
            updateDecorations(editor);
        }
    }, null, context.subscriptions);

    vscode.workspace.onDidChangeTextDocument((event) => {
        const editor = vscode.window.activeTextEditor;
        if (editor && editor.document === event.document) {
            updateDecorations(editor);
        }
    }, null, context.subscriptions);

    // Initial update
    if (vscode.window.activeTextEditor && vscode.window.activeTextEditor.document.languageId === 'twee') {
        updateDecorations(vscode.window.activeTextEditor);
    }
}

/** Update decorations for the given editor based on workspace analysis. */
async function updateDecorations(editor: vscode.TextEditor) {
    if (!client || !client.isRunning()) { return; }
    if (editor.document.languageId !== 'twee') { return; }

    const text = editor.document.getText();
    const lines = text.split('\n');

    // Collect passage header ranges and link ranges
    const passageHeaders: vscode.Range[] = [];
    const unreachableRanges: vscode.Range[] = [];
    const brokenLinkRanges: vscode.Range[] = [];

    // Find all passage headers
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        if (line.startsWith('::')) {
            passageHeaders.push(new vscode.Range(i, 0, i, line.length));
        }
    }

    // Find broken links (links with red squiggles from diagnostics)
    const diagnostics = vscode.languages.getDiagnostics(editor.document.uri);
    for (const diag of diagnostics) {
        if (diag.message.includes('Broken link') || diag.message.includes('broken link')) {
            brokenLinkRanges.push(diag.range);
        }
    }

    try {
        const wsFolders = vscode.workspace.workspaceFolders;
        if (wsFolders && wsFolders.length > 0) {
            // Fetch graph data to find unreachable passages
            const graph: any = await client.sendRequest('knot/graph', {
                workspace_uri: wsFolders[0].uri.toString(),
            });

            // Find unreachable passages in this document
            if (graph && graph.nodes) {
                const unreachableNames = new Set<string>();
                for (const node of graph.nodes) {
                    if (node.is_unreachable) {
                        unreachableNames.add(node.label);
                    }
                }

                // Find ranges of unreachable passages in this document
                for (let i = 0; i < lines.length; i++) {
                    const line = lines[i];
                    if (line.startsWith('::')) {
                        let name = line.substring(2).trim();
                        const bracketIdx = name.indexOf('[');
                        if (bracketIdx > 0) {
                            name = name.substring(0, bracketIdx).trim();
                        }
                        if (unreachableNames.has(name)) {
                            // Find end of this passage (next :: or end of file)
                            let endLine = i + 1;
                            while (endLine < lines.length && !lines[endLine].startsWith('::')) {
                                endLine++;
                            }
                            unreachableRanges.push(new vscode.Range(
                                i, 0, endLine - 1, lines[endLine - 1]?.length || 0
                            ));
                        }
                    }
                }
            }
        }
    } catch {
        // Silently ignore — decorations will be empty
    }

    // Apply decorations
    if (passageDecorationType) {
        editor.setDecorations(passageDecorationType, passageHeaders);
    }
    if (unreachableDecorationType) {
        editor.setDecorations(unreachableDecorationType, unreachableRanges);
    }
    if (linkDecorationType) {
        editor.setDecorations(linkDecorationType, brokenLinkRanges);
    }
}

// ---------------------------------------------------------------------------
// Task Provider
// ---------------------------------------------------------------------------

/** Register a Task Provider for Knot build and watch tasks. */
function registerTaskProvider(context: vscode.ExtensionContext) {
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
                    return new KnotBuildTerminal();
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
                    return new KnotWatchTerminal();
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

/** Custom terminal for the Knot build task. */
class KnotBuildTerminal implements vscode.Pseudoterminal {
    private writeEmitter = new vscode.EventEmitter<string>();
    onDidWrite: vscode.Event<string> = this.writeEmitter.event;
    private closeEmitter = new vscode.EventEmitter<number>();
    onDidClose?: vscode.Event<number> = this.closeEmitter.event;

    async open(): Promise<void> {
        this.writeEmitter.fire('Starting Knot build...\r\n');

        if (!client || !client.isRunning()) {
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
            const result: any = await client.sendRequest('knot/build', {
                workspace_uri: workspaceFolders[0].uri.toString(),
            });

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

    async open(): Promise<void> {
        this.writeEmitter.fire('Knot watch mode started. Saving a .tw/.twee file will trigger a rebuild.\r\n');
        this.writeEmitter.fire('Press Ctrl+C to stop.\r\n\r\n');

        this.watcher = vscode.workspace.onDidSaveTextDocument(async (doc) => {
            const ext = path.extname(doc.fileName).toLowerCase();
            if (ext !== '.tw' && ext !== '.twee') { return; }

            this.writeEmitter.fire(`[${new Date().toLocaleTimeString()}] File saved: ${path.basename(doc.fileName)} — rebuilding...\r\n`);

            if (!client || !client.isRunning()) {
                this.writeEmitter.fire('  Error: Language server not running\r\n');
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) { return; }

            try {
                const result: any = await client.sendRequest('knot/build', {
                    workspace_uri: workspaceFolders[0].uri.toString(),
                });
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

// ---------------------------------------------------------------------------
// Crash recovery
// ---------------------------------------------------------------------------

function handleServerFailure(
    error: unknown,
    context: vscode.ExtensionContext,
    serverPath: string
) {
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

        if (statusBarItem) {
            statusBarItem.text = '$(error) Knot: Server crashed';
            statusBarItem.show();
        }
    } else {
        vscode.window.showWarningMessage(
            `Knot: Language server crashed (attempt ${crashCount}/${MAX_CRASH_RETRIES}). Restarting...`
        );

        // Attempt automatic restart
        setTimeout(async () => {
            try {
                if (client) {
                    await client.start();
                    crashCount = 0;
                }
            } catch {
                handleServerFailure('Restart failed', context, serverPath);
            }
        }, 2000);
    }
}

// ---------------------------------------------------------------------------
// Deactivation
// ---------------------------------------------------------------------------

/** Refresh the Play Mode webview if it's active. */
function refreshPlayMode() {
    if (playModeProvider) {
        playModeProvider.refresh();
    }
}

export async function deactivate() {
    if (playModeProvider) {
        playModeProvider.dispose();
        playModeProvider = null;
    }
    if (client) {
        await client.stop();
        client = null;
    }
    if (statusBarItem) {
        statusBarItem.dispose();
        statusBarItem = null;
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

function tracing(message: string) {
    const config = vscode.workspace.getConfiguration('knot').get<string>('trace.server', 'off');
    if (config !== 'off') {
        console.log(`[Knot] ${message}`);
    }
}

/** Update the debug view with the passage under the cursor. */
function updateDebugViewForEditor(editor: vscode.TextEditor) {
    if (!debugViewProvider) { return; }

    const document = editor.document;
    if (document.languageId !== 'twee') { return; }

    const position = editor.selection.active;
    const text = document.getText();
    const lines = text.split('\n');

    // Walk backwards from the cursor to find the passage header
    let passageName: string | null = null;
    for (let i = position.line; i >= 0; i--) {
        const line = lines[i];
        if (line.startsWith('::')) {
            passageName = line.substring(2).trim().split('[')[0].trim();
            break;
        }
    }

    if (passageName) {
        debugViewProvider.updateForPassage(passageName);
    }
}
