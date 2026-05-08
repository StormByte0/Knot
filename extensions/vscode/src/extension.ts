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
import * as crypto from 'crypto';
import { StoryMapProvider } from './storyMapProvider';
import { PlayModeProvider } from './playModeProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';
import { KnotLanguageClient, KnotBuildResponse, KnotCompilerDetectResponse, KnotProfileResponse, KnotGraphResponse } from './types';

// The LanguageClient class is only available at runtime from the node entry.
// We use require() to access it since the typings don't export it.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const VLCModule = require('vscode-languageclient');
const LanguageClientCtor: typeof VLCModule.LanguageClient = VLCModule.LanguageClient;

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

let client: KnotLanguageClient | null = null;
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

    const serverOptions = {
        command: serverPath,
        args: ['--stdio'],
    };

    const clientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'twee' },
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher(
                '**/*.{tw,twee}'
            ),
        },
    };

    client = new LanguageClientCtor(
        'knot',
        'Knot Language Server',
        serverOptions,
        clientOptions
    ) as unknown as KnotLanguageClient;

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
                                const profile = await client?.sendRequest<KnotProfileResponse>('knot/profile', {
                                    workspace_uri: wsFolders[0].uri.toString(),
                                });
                                if (statusBarItem && profile) {
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

    // Create the Play Mode provider — now requires context for storage URI
    playModeProvider = new PlayModeProvider(context.extensionUri, context);

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
// Tweego compiler availability
// ---------------------------------------------------------------------------

/** Check if Tweego is available; prompt to download if not. */
async function ensureTweegoAvailable(context: vscode.ExtensionContext): Promise<string | undefined> {
    // 1. Check if tweego is on PATH via the language server
    try {
        const result = await client?.sendRequest<KnotCompilerDetectResponse>('knot/compilerDetect', { workspace_uri: '' });
        if (result && result.compiler_found) {
            return result.compiler_path;
        }
    } catch { /* ignore */ }

    // 2. Check if tweego is in .knot/bin/ in the workspace
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders) {
        const localBin = vscode.Uri.joinPath(workspaceFolders[0].uri, '.knot', 'bin');
        const tweegoPath = vscode.Uri.joinPath(localBin, process.platform === 'win32' ? 'tweego.exe' : 'tweego');
        try {
            await vscode.workspace.fs.stat(tweegoPath);
            return tweegoPath.fsPath;
        } catch { /* not found */ }
    }

    // 3. Prompt user to download
    const choice = await vscode.window.showWarningMessage(
        'Tweego compiler not found. Knot needs Tweego to build and preview Twine stories.',
        'Download Tweego',
        'Set Path Manually',
        'Cancel'
    );

    if (choice === 'Download Tweego') {
        return await downloadTweego(context);
    } else if (choice === 'Set Path Manually') {
        const fileUri = await vscode.window.showOpenDialog({
            canSelectFiles: true,
            canSelectFolders: false,
            canSelectMany: false,
            title: 'Select Tweego binary',
            filters: { 'Executable': ['exe', 'sh', ''] }
        });
        if (fileUri && fileUri[0]) {
            return fileUri[0].fsPath;
        }
    }
    return undefined;
}

/** Download Tweego from GitHub releases. */
async function downloadTweego(context: vscode.ExtensionContext): Promise<string | undefined> {
    const platform = process.platform;

    // Determine download URL based on platform
    let downloadUrl: string;
    let binaryName: string;

    if (platform === 'win32') {
        downloadUrl = 'https://github.com/tmedwards/tweego/releases/download/v2.1.1/tweego-2.1.1-windows-x64.zip';
        binaryName = 'tweego.exe';
    } else if (platform === 'darwin') {
        downloadUrl = 'https://github.com/tmedwards/tweego/releases/download/v2.1.1/tweego-2.1.1-macos-x64.zip';
        binaryName = 'tweego';
    } else {
        downloadUrl = 'https://github.com/tmedwards/tweego/releases/download/v2.1.1/tweego-2.1.1-linux-x64.zip';
        binaryName = 'tweego';
    }

    return vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: 'Downloading Tweego...',
        cancellable: true
    }, async (progress, _token) => {
        try {
            progress.report({ message: 'Downloading...' });

            const binDir = vscode.Uri.joinPath(context.globalStorageUri, 'tweego');
            await vscode.workspace.fs.createDirectory(binDir);
            const zipPath = vscode.Uri.joinPath(binDir, 'tweego.zip');

            // Download
            const response = await fetch(downloadUrl);
            if (!response.ok) throw new Error(`Download failed: ${response.statusText}`);
            const buffer = Buffer.from(await response.arrayBuffer());
            await vscode.workspace.fs.writeFile(zipPath, new Uint8Array(buffer));

            // Extract (use system unzip)
            const { execSync } = require('child_process');
            execSync(`unzip -o "${zipPath.fsPath}" -d "${binDir.fsPath}"`, { stdio: 'pipe' });

            // Find the binary
            const binaryPath = vscode.Uri.joinPath(binDir, binaryName);

            // Make executable on Unix
            if (platform !== 'win32') {
                execSync(`chmod +x "${binaryPath.fsPath}"`, { stdio: 'pipe' });
            }

            // Clean up zip
            await vscode.workspace.fs.delete(zipPath);

            vscode.window.showInformationMessage('Tweego downloaded successfully!');
            return binaryPath.fsPath;
        } catch (e) {
            vscode.window.showErrorMessage(`Failed to download Tweego: ${e}`);
            return undefined;
        }
    });
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

            // Check for Tweego availability
            const tweegoPath = await ensureTweegoAvailable(context);
            if (!tweegoPath) {
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                vscode.window.showWarningMessage('Knot: No workspace folder open.');
                return;
            }

            try {
                const result = await client.sendRequest<KnotBuildResponse>('knot/build', {
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
            // Check for Tweego availability
            const tweegoPath = await ensureTweegoAvailable(context);
            if (!tweegoPath) {
                return;
            }

            if (!playModeProvider) {
                playModeProvider = new PlayModeProvider(context.extensionUri, context);
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

            // Check for Tweego availability
            const tweegoPath = await ensureTweegoAvailable(context);
            if (!tweegoPath) {
                return;
            }

            if (!playModeProvider) {
                playModeProvider = new PlayModeProvider(context.extensionUri, context);
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
            if (statusBarItem) {
                statusBarItem.text = '$(sync~spin) Knot: Restarting...';
                statusBarItem.show();
            }

            try {
                await client?.stop();
                // Small delay to let the server fully shut down
                await new Promise(resolve => setTimeout(resolve, 500));
                await client?.start();
                vscode.window.showInformationMessage('Knot language server restarted.');
            } catch (e) {
                vscode.window.showErrorMessage(`Failed to restart Knot server: ${e}`);
            }
        })
    );

    // Re-index Workspace
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.reindexWorkspace', async () => {
            if (statusBarItem) {
                statusBarItem.text = '$(sync~spin) Knot: Re-indexing...';
                statusBarItem.show();
            }

            try {
                await client?.stop();
                await new Promise(resolve => setTimeout(resolve, 500));
                await client?.start();
                // Request re-indexing
                await client?.sendRequest('knot/reindexWorkspace', {});
                vscode.window.showInformationMessage('Knot workspace re-indexed.');
            } catch (e) {
                vscode.window.showErrorMessage(`Failed to re-index workspace: ${e}`);
            }
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
                const result = await client.sendRequest<KnotCompilerDetectResponse>('knot/compilerDetect', {
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

            // First, search open documents
            for (const doc of vscode.workspace.textDocuments) {
                if (doc.languageId !== 'twee') { continue; }
                for (let i = 0; i < doc.lineCount; i++) {
                    const line = doc.lineAt(i).text;
                    if (line.startsWith('::')) {
                        const name = line.substring(2).trim().split('[')[0].trim();
                        if (name === passageName) {
                            await vscode.window.showTextDocument(doc, {
                                preview: true,
                                selection: new vscode.Range(i, 0, i, line.length),
                            });
                            return;
                        }
                    }
                }
            }

            // If not found in open documents, search all workspace files
            const files = await vscode.workspace.findFiles('**/*.{tw,twee}');
            for (const fileUri of files) {
                try {
                    const doc = await vscode.workspace.openTextDocument(fileUri);
                    for (let i = 0; i < doc.lineCount; i++) {
                        const line = doc.lineAt(i).text;
                        if (line.startsWith('::')) {
                            const name = line.substring(2).trim().split('[')[0].trim();
                            if (name === passageName) {
                                await vscode.window.showTextDocument(doc, {
                                    preview: true,
                                    selection: new vscode.Range(i, 0, i, line.length),
                                });
                                return;
                            }
                        }
                    }
                } catch {
                    // Skip files that can't be opened
                }
            }

            vscode.window.showWarningMessage(`Knot: Passage '${passageName}' not found in workspace.`);
        })
    );

    // Initialize Project
    const initProject = vscode.commands.registerCommand('knot.initProject', async () => {
        const workspaceFolders = vscode.workspace.workspaceFolders;
        if (!workspaceFolders) {
            vscode.window.showErrorMessage('Please open a workspace folder first.');
            return;
        }

        const rootUri = workspaceFolders[0].uri;

        // Step 1: Select story format
        const formatItems: vscode.QuickPickItem[] = [
            { label: 'SugarCube 2', description: 'Most popular, full-featured format', detail: 'Best for complex stories with variables, macros, and state management' },
            { label: 'Harlowe 3', description: 'Built-in Twine 2 format', detail: 'Beginner-friendly, uses markup-based syntax' },
            { label: 'Chapbook', description: 'Simple, modern format', detail: 'Uses markdown-style syntax with state management' },
            { label: 'Snowman', description: 'Developer-oriented format', detail: 'Uses JavaScript and Underscore.js templating' },
        ];

        const selectedFormat = await vscode.window.showQuickPick(formatItems, {
            placeHolder: 'Select your story format',
            title: 'Knot: Initialize Twine Project'
        });
        if (!selectedFormat) return;

        const formatName = selectedFormat.label.split(' ')[0]; // SugarCube, Harlowe, Chapbook, Snowman

        // Step 2: Story title
        const storyTitle = await vscode.window.showInputBox({
            prompt: 'Enter your story title',
            value: 'My Story',
            title: 'Knot: Initialize Twine Project'
        });
        if (!storyTitle) return;

        // Step 3: Generate project files
        try {
            // Create directory structure
            const srcDir = vscode.Uri.joinPath(rootUri, 'src');
            const assetsDir = vscode.Uri.joinPath(rootUri, 'assets');
            const stylesDir = vscode.Uri.joinPath(rootUri, 'styles');
            await vscode.workspace.fs.createDirectory(srcDir);
            await vscode.workspace.fs.createDirectory(assetsDir);
            await vscode.workspace.fs.createDirectory(stylesDir);

            // Generate IFID
            const ifid = crypto.randomUUID().toUpperCase();

            // Generate main.tw with StoryData and Start passage
            let mainContent = '';

            // StoryTitle passage
            mainContent += `:: StoryTitle\n${storyTitle}\n\n`;

            // StoryData passage
            mainContent += `:: StoryData\n`;
            mainContent += JSON.stringify({
                ifid,
                format: formatName,
                "format-version": formatName === 'SugarCube' ? '2.36.1' : formatName === 'Harlowe' ? '3.3.0' : formatName === 'Chapbook' ? '1.2.1' : '1.4.0',
                start: 'Start',
                zoom: 1
            }, null, 2);
            mainContent += '\n\n';

            // Start passage with format-specific content
            mainContent += `:: Start\n`;
            switch (formatName) {
                case 'SugarCube':
                    mainContent += `Welcome to ${storyTitle}.\n\n`;
                    mainContent += `<<set $playerName to "">>\n`;
                    mainContent += `<<set $score to 0>>\n\n`;
                    mainContent += `[[Enter the story->First Passage]]\n\n`;
                    mainContent += `:: First Passage\n`;
                    mainContent += `You find yourself at the beginning of your adventure.\n\n`;
                    mainContent += `<<if $score eq 0>>You have no points yet.<<else>>You have $score points.<</if>>\n\n`;
                    mainContent += `<<set $score to $score + 1>>\n\n`;
                    mainContent += `[[Continue->Second Passage]]\n\n`;
                    mainContent += `:: Second Passage\n`;
                    mainContent += `The story continues from here.\n\n`;
                    mainContent += `[[Go back->Start]]\n`;
                    break;
                case 'Harlowe':
                    mainContent += `Welcome to ${storyTitle}.\n\n`;
                    mainContent += `(set: $playerName to "")\n`;
                    mainContent += `(set: $score to 0)\n\n`;
                    mainContent += `[[Enter the story->First Passage]]\n\n`;
                    mainContent += `:: First Passage\n`;
                    mainContent += `You find yourself at the beginning of your adventure.\n\n`;
                    mainContent += `(if: $score is 0)[You have no points yet.](else:)[You have $score points.]\n\n`;
                    mainContent += `(set: $score to it + 1)\n\n`;
                    mainContent += `[[Continue->Second Passage]]\n\n`;
                    mainContent += `:: Second Passage\n`;
                    mainContent += `The story continues from here.\n\n`;
                    mainContent += `[[Go back->Start]]\n`;
                    break;
                case 'Chapbook':
                    mainContent += `Welcome to ${storyTitle}.\n\n`;
                    mainContent += `[javascript]\nstate.score = 0;\n[/javascript]\n\n`;
                    mainContent += `[[First Passage]]\n\n`;
                    mainContent += `:: First Passage\n`;
                    mainContent += `You find yourself at the beginning of your adventure.\n\n`;
                    mainContent += `Your score is {state.score}.\n\n`;
                    mainContent += `[javascript]\nstate.score = state.score + 1;\n[/javascript]\n\n`;
                    mainContent += `[[Second Passage]]\n\n`;
                    mainContent += `:: Second Passage\n`;
                    mainContent += `The story continues from here.\n\n`;
                    mainContent += `[[Start]]\n`;
                    break;
                case 'Snowman':
                    mainContent += `Welcome to ${storyTitle}.\n\n`;
                    mainContent += `<% s.score = 0; %>\n\n`;
                    mainContent += `[[First Passage]]\n\n`;
                    mainContent += `:: First Passage\n`;
                    mainContent += `You find yourself at the beginning of your adventure.\n\n`;
                    mainContent += `<p>Your score is <%= s.score %>.</p>\n\n`;
                    mainContent += `<% s.score += 1; %>\n\n`;
                    mainContent += `[[Second Passage]]\n\n`;
                    mainContent += `:: Second Passage\n`;
                    mainContent += `The story continues from here.\n\n`;
                    mainContent += `[[Start]]\n`;
                    break;
            }

            // Write main.tw
            const mainFile = vscode.Uri.joinPath(srcDir, 'main.tw');
            await vscode.workspace.fs.writeFile(mainFile, new TextEncoder().encode(mainContent));

            // Generate .vscode/knot.json config
            const knotConfig = {
                format: formatName,
                compiler: {
                    path: '',
                    args: []
                },
                diagnostics: {
                    'broken-link': 'warning',
                    'unreachable-passage': 'hint',
                    'infinite-loop': 'warning',
                    'uninitialized-variable': 'warning',
                    'unused-variable': 'hint'
                }
            };
            const vscodeDir = vscode.Uri.joinPath(rootUri, '.vscode');
            await vscode.workspace.fs.createDirectory(vscodeDir);
            const knotConfigFile = vscode.Uri.joinPath(vscodeDir, 'knot.json');
            await vscode.workspace.fs.writeFile(knotConfigFile, new TextEncoder().encode(JSON.stringify(knotConfig, null, 2)));

            // Generate styles/story.css
            const cssFile = vscode.Uri.joinPath(stylesDir, 'story.css');
            await vscode.workspace.fs.writeFile(cssFile, new TextEncoder().encode('/* Custom story styles */\n'));

            vscode.window.showInformationMessage(`Knot: Initialized ${formatName} project "${storyTitle}" successfully!`);

            // Open the main file
            const doc = await vscode.workspace.openTextDocument(mainFile);
            await vscode.window.showTextDocument(doc);

        } catch (e) {
            vscode.window.showErrorMessage(`Failed to initialize project: ${e}`);
        }
    });
    context.subscriptions.push(initProject);
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
    // We also set up a periodic refresh for profile data
    const statusRefreshInterval = setInterval(async () => {
        if (!client || !client.isRunning() || !languageStatusItem) { return; }
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
            const graph = await client.sendRequest<import('./types').KnotGraphResponse>('knot/graph', {
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
            const result = await client.sendRequest<KnotBuildResponse>('knot/build', {
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
                const result = await client.sendRequest<KnotBuildResponse>('knot/build', {
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
