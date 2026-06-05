//! Command registration for Knot.
//!
//! Registers all VS Code commands and the Tweego compiler bootstrap
//! logic. Commands include:
//! - Story Map, Build, Play, Play from Passage
//! - Restart Server, Re-index Workspace, Detect Compiler
//! - Open Passage by Name, Open Virtual Document
//! - Initialize Project
//! - Toggle Auto-Rebuild

import * as vscode from 'vscode';
import * as crypto from 'crypto';
import { KnotLanguageClient, KnotBuildResponse, KnotCompilerDetectResponse, KnotReindexResponse, KnotGenerateIfidResponse } from './types';
import { PlayModeProvider } from './playModeProvider';
import { StoryMapPanelManager } from './storyMapProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';
import { VariableFlowProvider } from './variableFlowProvider';
import * as navigation from './navigation';
import { extractPassageName } from './utils';
import { openVirtualDoc } from './virtualDocProvider';

// ---------------------------------------------------------------------------
// Dependencies injected from extension.ts
// ---------------------------------------------------------------------------

export interface CommandDeps {
    getClient: () => KnotLanguageClient | null;
    statusBarItem: vscode.StatusBarItem;
    storyMapPanel: StoryMapPanelManager | null;
    debugViewProvider: DebugViewProvider | null;
    profileViewProvider: ProfileViewProvider | null;
    variableFlowProvider: VariableFlowProvider | null;
    /** Play mode provider — may be lazily created. */
    getPlayModeProvider: () => PlayModeProvider | null;
    setPlayModeProvider: (provider: PlayModeProvider) => void;
    context: vscode.ExtensionContext;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Register all Knot commands. */
export function registerCommands(deps: CommandDeps): void {
    const { context, getPlayModeProvider, setPlayModeProvider } = deps;

    // Open Story Map — single-instance WebviewPanel.
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openStoryMap', async () => {
            if (deps.storyMapPanel) {
                await deps.storyMapPanel.show();
            }
        })
    );

    // Build Project
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.build', async () => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            // Check for Tweego availability
            const tweegoPath = await ensureTweegoAvailable(context, client);
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
            const client = deps.getClient();
            // Check for Tweego availability
            const tweegoPath = await ensureTweegoAvailable(context, client);
            if (!tweegoPath) {
                return;
            }

            let playMode = getPlayModeProvider();
            if (!playMode) {
                playMode = new PlayModeProvider(context.extensionUri, context);
                if (client) {
                    playMode.setClient(client);
                }
                setPlayModeProvider(playMode);
            }
            await playMode.show();
        })
    );

    // Play from Passage — start play from a specific passage
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.playFromPassage', async (passageName?: string) => {
            const client = deps.getClient();
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
                            currentPassage = extractPassageName(line);
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
            const tweegoPath = await ensureTweegoAvailable(context, client);
            if (!tweegoPath) {
                return;
            }

            let playMode = getPlayModeProvider();
            if (!playMode) {
                playMode = new PlayModeProvider(context.extensionUri, context);
                if (client) {
                    playMode.setClient(client);
                }
                setPlayModeProvider(playMode);
            }
            await playMode.show(passageName);
        })
    );

    // Toggle Auto-Rebuild in Play Mode
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.toggleAutoRebuild', async () => {
            const playMode = getPlayModeProvider();
            if (playMode) {
                playMode.toggleAutoRebuild();
            } else {
                vscode.window.showInformationMessage('Knot: Play mode is not active.');
            }
        })
    );

    // Restart Language Server
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.restartServer', async () => {
            const client = deps.getClient();
            if (deps.statusBarItem) {
                deps.statusBarItem.text = '$(sync~spin) Knot: Restarting...';
                deps.statusBarItem.show();
            }

            try {
                await client?.stop();
                // Allow in-flight requests to complete before starting a new
                // server instance.  The previous 500 ms was too short — if
                // did_change was still holding the write lock, read-lock
                // handlers (codeAction, documentLink, inlayHint) could be
                // blocked and would try to write their responses to a
                // transport stream that had already been destroyed.
                await new Promise(resolve => setTimeout(resolve, 2000));
                await client?.start();
                vscode.window.showInformationMessage('Knot language server restarted.');
            } catch (e) {
                // "Cannot call write after a stream was destroyed" is a
                // cosmetic error that occurs when an in-flight LSP request
                // tries to respond after the transport was torn down during
                // a restart.  Suppress it — the new client session will be
                // fully functional.
                const msg = String(e);
                if (msg.includes('write after a stream was destroyed')) {
                    // Try to start the client again — the stop completed,
                    // the error is just from a late response.
                    try {
                        await new Promise(resolve => setTimeout(resolve, 1000));
                        await client?.start();
                        vscode.window.showInformationMessage('Knot language server restarted.');
                    } catch (e2) {
                        vscode.window.showErrorMessage(`Failed to restart Knot server: ${e2}`);
                    }
                } else {
                    vscode.window.showErrorMessage(`Failed to restart Knot server: ${e}`);
                }
            }
        })
    );

    // Re-index Workspace
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.reindexWorkspace', async () => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            if (deps.statusBarItem) {
                deps.statusBarItem.text = '$(sync~spin) Knot: Re-indexing...';
                deps.statusBarItem.show();
            }

            try {
                const wsFolders = vscode.workspace.workspaceFolders;
                const result = await client.sendRequest<KnotReindexResponse>('knot/reindexWorkspace', {
                    workspace_uri: wsFolders && wsFolders.length > 0 ? wsFolders[0].uri.toString() : '',
                });
                if (result && !result.success) {
                    vscode.window.showWarningMessage(`Knot: Re-index had issues: ${result.error || 'unknown'}`);
                } else {
                    vscode.window.showInformationMessage(`Knot: Re-indexed ${result?.files_indexed || 0} files.`);
                }
            } catch (e) {
                vscode.window.showErrorMessage(`Failed to re-index workspace: ${e}`);
            }
        })
    );

    // Detect Compiler
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.detectCompiler', async () => {
            const client = deps.getClient();
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

    // Open Passage by Name (used by debug view, diagnostics, etc.)
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openPassageByName', async (passageName: string, targetLine?: number) => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                return;
            }
            await navigation.navigateToPassage(passageName, targetLine);
        })
    );

    // Open Virtual Document
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openVirtualDoc', async () => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }
            await openVirtualDoc(client);
        })
    );

    // Initialize Project
    const initProject = vscode.commands.registerCommand('knot.initProject', async () => {
        const client = deps.getClient();
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

            // Generate IFID — prefer server-side generator for consistency with
            // Workspace::generate_ifid(), fall back to local crypto if server
            // is unavailable.
            let ifid: string;
            try {
                const result = await client?.sendRequest<KnotGenerateIfidResponse>('knot/generateIfid', {
                    workspace_uri: rootUri.toString(),
                });
                ifid = result?.ifid || crypto.randomUUID().toUpperCase();
            } catch {
                ifid = crypto.randomUUID().toUpperCase();
            }

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
// Tweego compiler availability
// ---------------------------------------------------------------------------

/** Check if Tweego is available; prompt to download if not. */
async function ensureTweegoAvailable(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient | null,
): Promise<string | undefined> {
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
