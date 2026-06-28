//! Command registration for Knot.
//!
//! Registers all VS Code commands and the Tweego compiler bootstrap
//! logic. Commands include:
//! - Story Map, Build, Play, Play from Passage
//! - Restart Server, Re-index Workspace, Detect Compiler
//! - Configure Story Formats (browse for folder, view installed formats)
//! - Open Passage by Name
//! - Initialize Project
//! - Toggle Auto-Rebuild

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import * as crypto from 'crypto';
import { execSync } from 'child_process';
import { KnotLanguageClient, KnotBuildResponse, KnotCompilerDetectResponse, KnotReindexResponse, KnotGenerateIfidResponse, KnotFormatsListResponse, KnotFormatsRefreshResponse } from './types';
import { StoryMapPanelManager } from './storyMapProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';
import { VariableFlowProvider } from './variableFlowProvider';
import * as navigation from './navigation';
import { extractPassageName, getBuildRequestParams, getFormatsRefreshParams, getManagedTweegoPath, getManagedStoryformatsPath } from './utils';
import { isWatchActive, toggleWatch } from './watchState';

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
    context: vscode.ExtensionContext;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Register all Knot commands. */
export function registerCommands(deps: CommandDeps): void {
    const { context } = deps;

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
                // getBuildRequestParams reads knot.build.tweegoPath, knot.build.sourceDir,
                // knot.build.outputDir, and knot.build.storyformatsPath from VS Code
                // Settings. The tweegoPath from ensureTweegoAvailable() is used
                // as a fallback if the setting isn't set.
                const buildParams = getBuildRequestParams(workspaceFolders[0].uri.toString());
                if (!buildParams.compiler_path && tweegoPath) {
                    buildParams.compiler_path = tweegoPath;
                }
                const result = await client.sendRequest<KnotBuildResponse>('knot/build', buildParams);
                if (result.success) {
                    vscode.window.showInformationMessage('Knot: Build succeeded!');
                } else {
                    // Check if the failure was due to a missing story format.
                    // If so, offer a one-click download button directly in the
                    // error message — no need to open the Configure Story Formats
                    // command separately.
                    const errorText = result.errors?.join(' ') || '';
                    const looksLikeFormatError = errorText.toLowerCase().includes('story format')
                        || errorText.toLowerCase().includes('format not found');

                    if (looksLikeFormatError) {
                        // Query the server for the project's format info (from StoryData)
                        try {
                            const fmtResult = await client.sendRequest<KnotFormatsListResponse>(
                                'knot/formats/list',
                                { workspace_uri: workspaceFolders[0].uri.toString() }
                            );
                            if (fmtResult.project_format
                                && fmtResult.project_format_version
                                && fmtResult.project_format_cached === false
                                && fmtResult.project_format === 'SugarCube') {
                                // Offer one-click download
                                const fmt = fmtResult.project_format;
                                const ver = fmtResult.project_format_version;
                                const choice = await vscode.window.showErrorMessage(
                                    `Knot: Build failed — ${fmt} v${ver} is not installed. Download it now?`,
                                    'Download',
                                    'Close'
                                );
                                if (choice === 'Download') {
                                    const cacheDir = await downloadStoryFormat(context, fmt, ver);
                                    if (cacheDir) {
                                        await client.sendRequest<KnotFormatsRefreshResponse>(
                                            'knot/formats/refresh',
                                            getFormatsRefreshParams(workspaceFolders[0].uri.toString())
                                        );
                                        // Retry the build automatically
                                        const retryResult = await client.sendRequest<KnotBuildResponse>('knot/build', buildParams);
                                        if (retryResult.success) {
                                            vscode.window.showInformationMessage('Knot: Build succeeded after format download!');
                                        } else {
                                            vscode.window.showErrorMessage(
                                                `Knot: Build still failing: ${retryResult.errors?.join(', ') || 'unknown error'}`
                                            );
                                        }
                                    }
                                }
                                return;
                            }
                        } catch {
                            // Fall through to generic error
                        }
                    }

                    vscode.window.showErrorMessage(
                        `Knot: Build failed: ${result.errors?.join(', ') || 'unknown error'}`
                    );
                }
            } catch (e) {
                vscode.window.showErrorMessage(`Knot: Build request failed: ${e}`);
            }
        })
    );

    // Play Story — open the compiled HTML in the default browser.
    // If Watch is ON, just open the existing HTML (Watch keeps it fresh).
    // If Watch is OFF, build first then open.
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.play', async () => {
            const client = deps.getClient();
            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                vscode.window.showWarningMessage('Knot: No workspace folder open.');
                return;
            }

            const watchActive = isWatchActive();
            let htmlPath: string | undefined;

            if (!watchActive) {
                // Build first
                const tweegoPath = await ensureTweegoAvailable(context, client);
                if (!tweegoPath) { return; }

                const buildParams = getBuildRequestParams(workspaceFolders[0].uri.toString());
                if (!buildParams.compiler_path && tweegoPath) {
                    buildParams.compiler_path = tweegoPath;
                }
                const result = await client?.sendRequest<KnotBuildResponse>('knot/build', buildParams);
                if (!result?.success) {
                    vscode.window.showErrorMessage(
                        'Knot: Build failed — ' + (result?.errors?.join('; ') || 'unknown error'),
                    );
                    return;
                }
                htmlPath = result.output_path;
            } else {
                // Watch is ON — find the existing HTML in the output dir
                htmlPath = await findBuiltHtml(workspaceFolders[0].uri);
            }

            if (!htmlPath) {
                vscode.window.showWarningMessage(
                    'Knot: No built HTML found. ' +
                    (watchActive
                        ? 'Save a source file to trigger a build, or use Build first.'
                        : 'Build did not produce an output file.'),
                );
                return;
            }

            // Open in the system default browser
            await vscode.env.openExternal(vscode.Uri.file(htmlPath));
        })
    );

    // Play from Passage — same as Play but with --start <passage>
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.playFromPassage', async (passageName?: string) => {
            const client = deps.getClient();
            // If no passage name provided, try to detect from cursor position
            if (!passageName) {
                const editor = vscode.window.activeTextEditor;
                if (editor) {
                    const text = editor.document.getText();
                    const position = editor.selection.active;
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

            const workspaceFolders = vscode.workspace.workspaceFolders;
            if (!workspaceFolders || workspaceFolders.length === 0) {
                vscode.window.showWarningMessage('Knot: No workspace folder open.');
                return;
            }

            // playFromPassage always builds — it needs --start which changes
            // the output, so we can't just open the existing HTML.
            const tweegoPath = await ensureTweegoAvailable(context, client);
            if (!tweegoPath) { return; }

            const buildParams = getBuildRequestParams(
                workspaceFolders[0].uri.toString(),
                passageName,
            );
            if (!buildParams.compiler_path && tweegoPath) {
                buildParams.compiler_path = tweegoPath;
            }
            const result = await client?.sendRequest<KnotBuildResponse>('knot/build', buildParams);
            if (!result?.success) {
                vscode.window.showErrorMessage(
                    'Knot: Build failed — ' + (result?.errors?.join('; ') || 'unknown error'),
                );
                return;
            }

            if (result.output_path) {
                await vscode.env.openExternal(vscode.Uri.file(result.output_path));
            }
        })
    );

    // Toggle Watch (background auto-rebuild on save)
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.toggleWatch', async () => {
            const client = deps.getClient();
            const active = toggleWatch(client);
            vscode.window.showInformationMessage(
                `Knot: Watch ${active ? 'enabled' : 'disabled'}.`,
            );
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

    // Configure Story Formats — interactive UI for managing the storyformats
    // directory and the installed formats catalog. Lets the user:
    //   - See the currently resolved directory and the formats installed there
    //   - Browse for a different folder (preview before saving)
    //   - Clear the configured path (revert to auto-discovery)
    //   - Open the Settings UI at the knot.build.storyformatsPath field
    //   - Refresh the catalog after manually adding/removing format dirs
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.configureStoryFormats', async () => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                vscode.window.showWarningMessage('Knot: Language server is not running.');
                return;
            }

            const workspaceFolders = vscode.workspace.workspaceFolders;
            const workspaceUri = workspaceFolders && workspaceFolders.length > 0
                ? workspaceFolders[0].uri.toString()
                : '';

            try {
                // First, refresh the server's catalog so we have fresh data.
                await client.sendRequest<KnotFormatsRefreshResponse>('knot/formats/refresh',
                    getFormatsRefreshParams(workspaceUri)
                );

                // Then fetch the current state.
                const result = await client.sendRequest<KnotFormatsListResponse>('knot/formats/list', {
                    workspace_uri: workspaceUri,
                });

                const configuredPath = result.configured_path;
                const resolvedDir = result.resolved_dir;
                const formats = result.formats;

                // Build QuickPick items.
                const items: (vscode.QuickPickItem & { action?: string; format?: typeof formats[number] })[] = [];

                // Header: show current state.
                items.push({
                    label: 'Current State',
                    kind: vscode.QuickPickItemKind.Separator,
                });

                // Show managed storage path so users know where things live.
                const managedRoot = context.globalStorageUri.fsPath;
                items.push({
                    label: '$(folder) Managed storage:',
                    description: managedRoot,
                    detail: 'Extension-managed tweego binary + versioned storyformat cache',
                    action: 'openManagedStorage',
                });

                if (configuredPath) {
                    items.push({
                        label: `$(settings) Configured path: ${configuredPath}`,
                        description: 'From Build: Story Formats Path setting',
                        action: 'openSettings',
                    });
                } else {
                    items.push({
                        label: '$(info) No path configured — using auto-discovery',
                        description: resolvedDir
                            ? `Resolved to: ${resolvedDir}`
                            : 'No storyformats directory found',
                        action: 'openSettings',
                    });
                }

                if (formats.length > 0) {
                    items.push({
                        label: `Installed Formats (${formats.length})`,
                        kind: vscode.QuickPickItemKind.Separator,
                    });
                    for (const f of formats) {
                        items.push({
                            label: `$(package) ${f.name} v${f.version}`,
                            description: f.dir_name,
                            detail: [f.author, f.license, f.source].filter(Boolean).join(' • '),
                            format: f,
                        });
                    }
                } else if (resolvedDir) {
                    items.push({
                        label: '$(warning) No formats found in the resolved directory',
                        description: resolvedDir,
                    });
                } else {
                    items.push({
                        label: '$(warning) No storyformats directory resolved',
                        description: 'Builds will likely fail with "story format not found"',
                    });
                }

                // Actions section.
                items.push({
                    label: 'Actions',
                    kind: vscode.QuickPickItemKind.Separator,
                });

                // Dynamic download action based on what StoryData says the
                // project needs. If we know the format + version, offer a
                // one-click download — no manual version entry required.
                if (result.project_format && result.project_format_version) {
                    const fmt = result.project_format;
                    const ver = result.project_format_version;
                    if (result.project_format_cached) {
                        // Already cached — show a disabled-style info item
                        items.push({
                            label: `$(check) ${fmt} v${ver} (from StoryData) — already cached`,
                            description: 'No download needed',
                        });
                    } else if (fmt === 'SugarCube') {
                        items.push({
                            label: `$(cloud-download) Download ${fmt} v${ver}`,
                            description: 'Detected from StoryData — one click to install',
                            action: 'downloadProjectFormat',
                        });
                    } else {
                        items.push({
                            label: `$(warning) ${fmt} v${ver} needed (from StoryData) — manual install required`,
                            description: `Auto-download not available for ${fmt}. Use 'Browse for folder' instead.`,
                        });
                    }
                } else {
                    // No StoryData detected — offer manual download as fallback
                    items.push({
                        label: '$(cloud-download) Download SugarCube format...',
                        description: 'No StoryData detected — enter version manually',
                        action: 'downloadManual',
                    });
                }

                items.push({
                    label: '$(folder) Browse for story formats folder...',
                    description: 'Pick a directory containing format subdirectories',
                    action: 'browse',
                });
                items.push({
                    label: '$(refresh) Refresh catalog',
                    description: 'Re-scan the resolved directory after adding/removing formats',
                    action: 'refresh',
                });
                if (configuredPath) {
                    items.push({
                        label: '$(close) Clear configured path',
                        description: 'Revert to auto-discovery (project-local storyformats, tweego sibling)',
                        action: 'clear',
                    });
                }
                items.push({
                    label: '$(gear) Open Settings',
                    description: 'Edit Build: Story Formats Path directly in the Settings UI',
                    action: 'openSettings',
                });

                const selection = await vscode.window.showQuickPick(items, {
                    placeHolder: 'Configure story formats for Knot builds',
                    canPickMany: false,
                });

                if (!selection) {
                    return;
                }

                if (selection.action === 'openSettings') {
                    await vscode.commands.executeCommand(
                        'workbench.action.openSettings',
                        'knot.build.storyformatsPath'
                    );
                    return;
                }

                if (selection.action === 'openManagedStorage') {
                    vscode.commands.executeCommand('revealFileInOS', context.globalStorageUri);
                    return;
                }

                if (selection.action === 'browse') {
                    const folderUri = await vscode.window.showOpenDialog({
                        canSelectFiles: false,
                        canSelectFolders: true,
                        canSelectMany: false,
                        openLabel: 'Use this story formats folder',
                        title: 'Select a directory containing story format subdirectories (e.g. sugarcube-2/, harlowe-3/)',
                    });
                    if (!folderUri || folderUri.length === 0) {
                        return;
                    }
                    const selectedPath = folderUri[0].fsPath;

                    // Preview what's in that directory before saving.
                    const preview = await client.sendRequest<KnotFormatsListResponse>('knot/formats/list', {
                        workspace_uri: workspaceUri,
                        path_override: selectedPath,
                    });

                    if (preview.formats.length === 0) {
                        const proceed = await vscode.window.showWarningMessage(
                            `No format.js files found in subdirectories of:\n${selectedPath}\n\nSave this path anyway?`,
                            { modal: false },
                            'Save anyway',
                            'Cancel'
                        );
                        if (proceed !== 'Save anyway') {
                            return;
                        }
                    } else {
                        const formatList = preview.formats
                            .map(f => `  • ${f.name} v${f.version}`)
                            .join('\n');
                        const proceed = await vscode.window.showInformationMessage(
                            `Found ${preview.formats.length} format(s) in:\n${selectedPath}\n\n${formatList}\n\nSave this as the story formats path?`,
                            { modal: false },
                            'Save',
                            'Cancel'
                        );
                        if (proceed !== 'Save') {
                            return;
                        }
                    }

                    // Save to the knot.build.storyformatsPath setting.
                    const config = vscode.workspace.getConfiguration('knot');
                    await config.update('build.storyformatsPath', selectedPath, vscode.ConfigurationTarget.Global);

                    // Trigger a refresh so the server picks up the new path.
                    await client.sendRequest<KnotFormatsRefreshResponse>('knot/formats/refresh',
                        getFormatsRefreshParams(workspaceUri)
                    );

                    vscode.window.showInformationMessage(
                        `Knot: Story formats path saved. ${preview.formats.length} format(s) discovered.`
                    );
                    return;
                }

                if (selection.action === 'refresh') {
                    const refreshResult = await client.sendRequest<KnotFormatsRefreshResponse>('knot/formats/refresh',
                        getFormatsRefreshParams(workspaceUri)
                    );
                    if (refreshResult.success) {
                        vscode.window.showInformationMessage(
                            `Knot: Refreshed — ${refreshResult.format_count} format(s) from ${refreshResult.resolved_dir || '(none)'}`
                        );
                    } else {
                        vscode.window.showErrorMessage(
                            `Knot: Refresh failed — ${refreshResult.error || 'unknown error'}`
                        );
                    }
                    return;
                }

                if (selection.action === 'clear') {
                    const config = vscode.workspace.getConfiguration('knot');
                    await config.update('build.storyformatsPath', '', vscode.ConfigurationTarget.Global);
                    await client.sendRequest<KnotFormatsRefreshResponse>('knot/formats/refresh',
                        getFormatsRefreshParams(workspaceUri)
                    );
                    vscode.window.showInformationMessage(
                        'Knot: Cleared story formats path — using auto-discovery.'
                    );
                    return;
                }

                // One-click download of the format the project needs (from StoryData).
                // No manual version entry — we already know what to fetch.
                if (selection.action === 'downloadProjectFormat') {
                    const fmt = result.project_format!;
                    const ver = result.project_format_version!;
                    const cacheDir = await downloadStoryFormat(context, fmt, ver);
                    if (cacheDir) {
                        await client.sendRequest<KnotFormatsRefreshResponse>(
                            'knot/formats/refresh',
                            getFormatsRefreshParams(workspaceUri)
                        );
                    }
                    return;
                }

                // Fallback: manual download when no StoryData is available.
                if (selection.action === 'downloadManual') {
                    const formatName = await vscode.window.showQuickPick(
                        ['SugarCube', 'Harlowe', 'Chapbook', 'Snowman'],
                        { placeHolder: 'Select a story format to download' }
                    );
                    if (!formatName) { return; }

                    if (formatName !== 'SugarCube') {
                        vscode.window.showInformationMessage(
                            `Knot: Auto-download is currently only available for SugarCube. ` +
                            `For ${formatName}, please download it manually and use 'Browse for folder' to install.`
                        );
                        return;
                    }

                    const version = await vscode.window.showInputBox({
                        prompt: 'Enter the SugarCube version to download',
                        placeHolder: 'e.g. 2.37.0',
                        validateInput: (v) => {
                            if (!v.trim()) { return 'Version is required'; }
                            if (!/^\d+\.\d+\.\d+$/.test(v.trim())) {
                                return 'Version must be in the format X.Y.Z (e.g. 2.37.0)';
                            }
                            return null;
                        },
                    });
                    if (!version) { return; }

                    const cacheDir = await downloadStoryFormat(context, formatName, version.trim());
                    if (cacheDir) {
                        await client.sendRequest<KnotFormatsRefreshResponse>(
                            'knot/formats/refresh',
                            getFormatsRefreshParams(workspaceUri)
                        );
                    }
                    return;
                }
            } catch (e) {
                vscode.window.showErrorMessage(`Knot: Failed to configure story formats: ${e}`);
            }
        })
    );

    // Open Tweego Folder — opens the Knot-managed toolchain directory in
    // the OS file explorer. Lets users see (and manually update) the
    // downloaded tweego binary and storyformats without needing to dig
    // through VS Code's globalStorage path.
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openTweegoFolder', async () => {
            const managedTweego = getManagedTweegoPath();
            const managedSf = getManagedStoryformatsPath();

            if (!managedTweego && !managedSf) {
                const choice = await vscode.window.showInformationMessage(
                    'Knot: Tweego has not been downloaded yet. Download it now?',
                    'Download',
                    'Cancel'
                );
                if (choice === 'Download') {
                    await downloadTweego(context);
                }
                return;
            }

            // Open the globalStorage root (contains both tweego/ and storyformats/)
            const storageRoot = context.globalStorageUri.fsPath;
            vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(storageRoot));
        })
    );

    // Open Managed Storage Folder — opens the extension's globalStorage
    // directory in the OS file explorer. This is where the managed tweego
    // binary and versioned storyformat cache live.
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openManagedStorage', async () => {
            vscode.commands.executeCommand('revealFileInOS', context.globalStorageUri);
        })
    );

    // Open Passage by Name (used by debug view, diagnostics, etc.)
    context.subscriptions.push(
        vscode.commands.registerCommand('knot.openPassageByName', async (passageName: string, targetLine?: number, spanStart?: number, spanEnd?: number) => {
            const client = deps.getClient();
            if (!client || !client.isRunning()) {
                return;
            }
            await navigation.navigateToPassage(passageName, targetLine, spanStart, spanEnd);
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
// HTML file discovery (for Play when Watch is active)
// ---------------------------------------------------------------------------

/**
 * Find the most recently modified .html file in the build output directory.
 * Used by `knot.play` when Watch is active — the watcher keeps the HTML
 * fresh, so we just need to find and open it.
 *
 * Returns the absolute path to the HTML file, or undefined if none exists.
 */
async function findBuiltHtml(workspaceUri: vscode.Uri): Promise<string | undefined> {
    const config = vscode.workspace.getConfiguration('knot');
    const outputDirName = config.get<string>('build.outputDir', 'build') || 'build';
    const outputDirUri = vscode.Uri.joinPath(workspaceUri, outputDirName);

    try {
        const entries = await vscode.workspace.fs.readDirectory(outputDirUri);
        const htmlFiles = entries
            .filter(([name, type]) => type === vscode.FileType.File && name.endsWith('.html'))
            .map(([name]) => vscode.Uri.joinPath(outputDirUri, name));

        if (htmlFiles.length === 0) {
            return undefined;
        }

        // Get stats for all HTML files and pick the most recently modified
        const stats = await Promise.all(
            htmlFiles.map(async (uri) => {
                try {
                    const stat = await vscode.workspace.fs.stat(uri);
                    return { uri, mtime: stat.mtime };
                } catch {
                    return { uri, mtime: 0 };
                }
            }),
        );
        stats.sort((a, b) => b.mtime - a.mtime);
        return stats[0]?.uri.fsPath;
    } catch {
        // Output directory doesn't exist or isn't readable
        return undefined;
    }
}

// ---------------------------------------------------------------------------
// Tweego compiler availability
// ---------------------------------------------------------------------------

/** Check if Tweego is available; prompt to download if not.
 *
 *  Resolution order:
 *  1. VS Code setting `knot.build.tweegoPath` (persisted user preference)
 *  2. Language server detection (PATH lookup via `which`/`where`)
 *  3. Global storage path (previously downloaded by the extension)
 *  4. Prompt user: Download | Set Path Manually | Cancel
 *
 *  When a path is found via download or manual selection, it is
 *  persisted to the `knot.build.tweegoPath` setting so subsequent builds
 *  don't re-prompt.
 */
async function ensureTweegoAvailable(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient | null,
): Promise<string | undefined> {
    // 1. Check VS Code setting (power user override)
    const config = vscode.workspace.getConfiguration('knot');
    const settingPath = config.get<string>('build.tweegoPath');
    if (settingPath && settingPath.trim()) {
        try {
            fs.accessSync(settingPath, fs.constants.X_OK);
            return settingPath;
        } catch {
            // Setting exists but file is gone/invalid — fall through
        }
    }

    // 2. Check the Knot-managed binary (downloaded to globalStorage).
    // This is the preferred path — Knot owns the toolchain.
    const managedTweego = getManagedTweegoPath();
    if (managedTweego) {
        return managedTweego;
    }

    // 3. Check via the language server (PATH lookup)
    try {
        const result = await client?.sendRequest<KnotCompilerDetectResponse>('knot/compilerDetect', { workspace_uri: '' });
        if (result && result.compiler_found && result.compiler_path) {
            return result.compiler_path;
        }
    } catch { /* ignore */ }

    // 4. Prompt user: Download (managed) | Set Path Manually | Open Storage | Cancel
    const choice = await vscode.window.showWarningMessage(
        'Tweego compiler not found. Knot needs Tweego to build and preview Twine stories.',
        'Download Tweego',
        'Set Path Manually',
        'Open Storage Folder',
        'Cancel'
    );

    if (choice === 'Download Tweego') {
        const downloaded = await downloadTweego(context);
        if (downloaded) {
            // Don't persist to knot.build.tweegoPath — getBuildRequestParams()
            // checks the managed path automatically. This way, if the user
            // later sets knot.build.tweegoPath explicitly, their override works.
            return downloaded;
        }
    } else if (choice === 'Open Storage Folder') {
        // Open the managed storage folder so the user can see where tweego
        // and storyformats will be / are installed.
        vscode.commands.executeCommand('revealFileInOS', context.globalStorageUri);
        return undefined;
    } else if (choice === 'Set Path Manually') {
        const fileUri = await vscode.window.showOpenDialog({
            canSelectFiles: true,
            canSelectFolders: false,
            canSelectMany: false,
            title: 'Select Tweego binary',
            filters: process.platform === 'win32'
                ? { 'Executable': ['exe'] }
                : { 'All Files': ['*'] }
        });
        if (fileUri && fileUri[0]) {
            const selectedPath = fileUri[0].fsPath;
            // Make executable on Unix
            if (process.platform !== 'win32') {
                try {
                    fs.chmodSync(selectedPath, 0o755);
                } catch { /* ignore */ }
            }
            // Trust the user's selection — don't validate by running --version.
            // Tweego exits non-zero on --version when it can't find .storyformats,
            // which makes validation unreliable. If the path is wrong, the build
            // will fail with a clear error from the server.
            await config.update('build.tweegoPath', selectedPath, vscode.ConfigurationTarget.Global);
            vscode.window.showInformationMessage('Tweego path saved.');
            return selectedPath;
        }
    }
    return undefined;
}

/** Download Tweego from GitHub releases and extract it.
 *
 *  Cross-platform: uses Node.js built-in modules to download and extract
 *  the zip file without relying on system `unzip`.
 */
async function downloadTweego(context: vscode.ExtensionContext): Promise<string | undefined> {
    const platform = process.platform;
    // `process.arch` not currently used — kept for future per-arch URL selection.
    const _arch = process.arch;
    void _arch;

    // Determine download URL and binary name based on platform
    let downloadUrl: string;
    let binaryName: string;

    if (platform === 'win32') {
        downloadUrl = 'https://github.com/tmedwards/tweego/releases/download/v2.1.1/tweego-2.1.1-windows-x64.zip';
        binaryName = 'tweego.exe';
    } else if (platform === 'darwin') {
        // macOS: use x64 build (works on Apple Silicon via Rosetta)
        downloadUrl = 'https://github.com/tmedwards/tweego/releases/download/v2.1.1/tweego-2.1.1-macos-x64.zip';
        binaryName = 'tweego';
    } else {
        // Linux
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

            const binDir = path.join(context.globalStorageUri.fsPath, 'tweego');
            fs.mkdirSync(binDir, { recursive: true });
            const zipPath = path.join(binDir, 'tweego.zip');

            // Download using fetch (available in Node 18+ / VS Code 1.85+)
            const response = await fetch(downloadUrl);
            if (!response.ok) throw new Error(`Download failed: ${response.statusText}`);
            const buffer = Buffer.from(await response.arrayBuffer());
            fs.writeFileSync(zipPath, buffer);

            progress.report({ message: 'Extracting...' });

            // Extract using the system's built-in extraction.
            // On Windows, use PowerShell's Expand-Archive.
            // On Unix, use Python (always available on macOS, usually on Linux)
            // or fall back to `unzip` if available.
            if (platform === 'win32') {
                execSync(
                    `powershell -NoProfile -Command "Expand-Archive -Force -Path '${zipPath}' -DestinationPath '${binDir}'"`,
                    { stdio: 'pipe' }
                );
            } else {
                // Try Python first (cross-platform, no external deps needed)
                try {
                    execSync(
                        `python3 -c "import zipfile; zipfile.ZipFile('${zipPath}').extractall('${binDir}')"`,
                        { stdio: 'pipe' }
                    );
                } catch {
                    // Fall back to unzip
                    execSync(`unzip -o "${zipPath}" -d "${binDir}"`, { stdio: 'pipe' });
                }
            }

            // The zip may contain the binary at the root or in a subdirectory.
            // Find it by walking the extracted directory.
            let binaryPath = path.join(binDir, binaryName);
            if (!fs.existsSync(binaryPath)) {
                // Search subdirectories
                const findBinary = (dir: string): string | null => {
                    const entries = fs.readdirSync(dir, { withFileTypes: true });
                    for (const entry of entries) {
                        const fullPath = path.join(dir, entry.name);
                        if (entry.isDirectory()) {
                            const found = findBinary(fullPath);
                            if (found) return found;
                        } else if (entry.name === binaryName) {
                            return fullPath;
                        }
                    }
                    return null;
                };
                binaryPath = findBinary(binDir) || binaryPath;
            }

            if (!fs.existsSync(binaryPath)) {
                throw new Error(`Binary "${binaryName}" not found in downloaded archive.`);
            }

            // Make executable on Unix
            if (platform !== 'win32') {
                fs.chmodSync(binaryPath, 0o755);
            }

            // Move storyformats OUT of the tweego binary directory to a
            // separate managed location. This is critical: tweego's first
            // storyformat search path is <binary_dir>/storyformats/. If we
            // leave them there, project-local <workspace>/storyformats/
            // overrides (which tweego searches 3rd) can never take priority.
            //
            // By moving them to <globalStorage>/storyformats/, tweego's
            // binary-sibling search finds nothing, and the managed formats
            // are only found via TWEEGO_PATH (searched last). This gives
            // projects the ability to pin specific format versions by
            // shipping their own storyformats/ folder.
            const extractedSfDir = path.join(binDir, 'storyformats');
            if (fs.existsSync(extractedSfDir)) {
                const managedSfDir = path.join(context.globalStorageUri.fsPath, 'storyformats');
                fs.mkdirSync(managedSfDir, { recursive: true });

                // Move each format subdirectory (sugarcube-2, harlowe-3, etc.)
                const formatDirs = fs.readdirSync(extractedSfDir, { withFileTypes: true });
                for (const entry of formatDirs) {
                    if (entry.isDirectory()) {
                        const src = path.join(extractedSfDir, entry.name);
                        const dst = path.join(managedSfDir, entry.name);
                        // Remove existing destination if present (update case)
                        if (fs.existsSync(dst)) {
                            fs.rmSync(dst, { recursive: true, force: true });
                        }
                        fs.renameSync(src, dst);
                    }
                }
                // Remove the now-empty storyformats directory from binDir
                fs.rmSync(extractedSfDir, { recursive: true, force: true });
            }

            // Clean up zip
            fs.unlinkSync(zipPath);

            vscode.window.showInformationMessage('Tweego downloaded successfully!');
            return binaryPath;
        } catch (e) {
            vscode.window.showErrorMessage(`Failed to download Tweego: ${e}`);
            return undefined;
        }
    });
}

// ---------------------------------------------------------------------------
// Story format download — uses fetch + platform-native extraction
// ---------------------------------------------------------------------------

/** Map a format name to the tweego format ID (directory name). */
function formatNameToId(formatName: string): string | null {
    switch (formatName) {
        case 'SugarCube': return 'sugarcube-2';
        case 'Harlowe': return 'harlowe-3';
        case 'Chapbook': return 'chapbook-1';
        case 'Snowman': return 'snowman-2';
        default: return null;
    }
}

/** Construct a download URL for a story format version.
 *  Currently only SugarCube has clean per-version release URLs on GitHub.
 *
 *  SugarCube release asset naming (verified from GitHub API):
 *    sugarcube-{VERSION}-for-twine-2.1-local.zip   ← Twine 2 story-format bundle (what tweego needs)
 *    sugarcube-{VERSION}-for-twine-1.4.zip         ← Twine 1.4 format
 */
function formatDownloadUrl(formatName: string, version: string): string | null {
    if (formatName === 'SugarCube') {
        return `https://github.com/tmedwards/sugarcube-2/releases/download/v${version}/sugarcube-${version}-for-twine-2.1-local.zip`;
    }
    return null;
}

/**
 * Download a story format version into the extension-managed versioned cache.
 *
 * Downloads the format zip from the format's official release URL, extracts
 * it to `<globalStorage>/storyformats/<format-id>@<version>/<format-id>/`,
 * and makes it available for builds via TWEEGO_PATH.
 *
 * Reuses the same fetch + platform-native extraction pattern as
 * `downloadTweego()` — no npm zip dependencies needed.
 *
 * @returns The versioned cache directory path on success, undefined on failure.
 */
async function downloadStoryFormat(
    context: vscode.ExtensionContext,
    formatName: string,
    version: string,
): Promise<string | undefined> {
    const formatId = formatNameToId(formatName);
    if (!formatId) {
        vscode.window.showErrorMessage(
            `Knot: Unknown format "${formatName}". Supported: SugarCube, Harlowe, Chapbook, Snowman`
        );
        return undefined;
    }

    const url = formatDownloadUrl(formatName, version);
    if (!url) {
        vscode.window.showInformationMessage(
            `Knot: Auto-download is not available for ${formatName} v${version}. ` +
            `Please download it manually and use "Browse for folder" to install.`
        );
        return undefined;
    }

    // Versioned cache directory:
    //   <globalStorage>/storyformats/sugarcube-2@2.37.0/
    // The format itself goes inside:
    //   <globalStorage>/storyformats/sugarcube-2@2.37.0/sugarcube-2/format.js
    const versionedDir = path.join(
        context.globalStorageUri.fsPath,
        'storyformats',
        `${formatId}@${version}`,
    );
    const formatDest = path.join(versionedDir, formatId);

    // Already cached?
    if (fs.existsSync(path.join(formatDest, 'format.js'))) {
        vscode.window.showInformationMessage(
            `Knot: ${formatName} v${version} is already cached at ${versionedDir}`
        );
        return versionedDir;
    }

    return vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: `Knot: Downloading ${formatName} v${version}...`,
        cancellable: true,
    }, async (progress, _token) => {
        try {
            progress.report({ message: 'Downloading...' });

            fs.mkdirSync(versionedDir, { recursive: true });
            const zipPath = path.join(versionedDir, `${formatId}.zip`);

            // Download using fetch (Node 18+ / VS Code 1.85+)
            const response = await fetch(url);
            if (!response.ok) {
                throw new Error(`HTTP ${response.status} ${response.statusText} — check that version ${version} exists at ${url}`);
            }
            const buffer = Buffer.from(await response.arrayBuffer());
            fs.writeFileSync(zipPath, buffer);

            progress.report({ message: 'Extracting...' });

            // Extract to a temp dir first, then move the format-id subdirectory
            // into place. The SugarCube zip contains a top-level `sugarcube-2/`
            // directory — we want that to end up at `formatDest`.
            const extractDir = path.join(versionedDir, '_extract_tmp');
            fs.mkdirSync(extractDir, { recursive: true });

            const platform = process.platform;
            if (platform === 'win32') {
                execSync(
                    `powershell -NoProfile -Command "Expand-Archive -Force -Path '${zipPath}' -DestinationPath '${extractDir}'"`,
                    { stdio: 'pipe' },
                );
            } else {
                // Try Python first (cross-platform, no external deps needed)
                try {
                    execSync(
                        `python3 -c "import zipfile; zipfile.ZipFile('${zipPath}').extractall('${extractDir}')"`,
                        { stdio: 'pipe' },
                    );
                } catch {
                    // Fall back to unzip
                    execSync(`unzip -o "${zipPath}" -d "${extractDir}"`, { stdio: 'pipe' });
                }
            }

            // Find the format-id directory in the extracted contents.
            // It may be at the root (e.g. `extractDir/sugarcube-2/format.js`)
            // or nested one level deeper.
            let sourceFormatDir: string | null = null;
            if (fs.existsSync(path.join(extractDir, formatId, 'format.js'))) {
                sourceFormatDir = path.join(extractDir, formatId);
            } else {
                // Search one level deep
                const entries = fs.readdirSync(extractDir, { withFileTypes: true });
                for (const entry of entries) {
                    if (entry.isDirectory()) {
                        const candidate = path.join(extractDir, entry.name, formatId);
                        if (fs.existsSync(path.join(candidate, 'format.js'))) {
                            sourceFormatDir = candidate;
                            break;
                        }
                        // Maybe the format-id IS the top-level dir
                        if (entry.name === formatId && fs.existsSync(path.join(extractDir, entry.name, 'format.js'))) {
                            sourceFormatDir = path.join(extractDir, entry.name);
                            break;
                        }
                    }
                }
            }

            if (!sourceFormatDir) {
                // Fall back: just move the whole extract dir contents
                fs.mkdirSync(formatDest, { recursive: true });
                const entries = fs.readdirSync(extractDir, { withFileTypes: true });
                for (const entry of entries) {
                    fs.renameSync(path.join(extractDir, entry.name), path.join(formatDest, entry.name));
                }
            } else {
                // Move the format-id dir into place
                if (fs.existsSync(formatDest)) {
                    fs.rmSync(formatDest, { recursive: true, force: true });
                }
                fs.renameSync(sourceFormatDir, formatDest);
            }

            // Clean up temp + zip
            fs.rmSync(extractDir, { recursive: true, force: true });
            fs.unlinkSync(zipPath);

            // Verify
            if (!fs.existsSync(path.join(formatDest, 'format.js'))) {
                throw new Error(`format.js not found after extraction at ${formatDest}`);
            }

            vscode.window.showInformationMessage(
                `Knot: ${formatName} v${version} downloaded successfully. Cached at: ${versionedDir}`
            );
            return versionedDir;
        } catch (e) {
            vscode.window.showErrorMessage(`Knot: Failed to download ${formatName} v${version}: ${e}`);
            return undefined;
        }
    });
}
