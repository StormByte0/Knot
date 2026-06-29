//! Command registration for Knot.
//!
//! Registers all VS Code commands and the Tweego compiler bootstrap logic.
//! Commands are organized into three sections:
//!
//!   1. Helpers — shared guards and error formatters used by every command
//!   2. Command registration — the public `registerCommands()` entry point
//!   3. Toolchain bootstrap — tweego/story-format download helpers
//!
//! Design rules followed by every command:
//!   - Guard the client with `requireClient()` first; bail with a clear
//!     warning if the server isn't running.
//!   - Guard the workspace with `requireWorkspace()` next; bail with a
//!     clear warning if no folder is open.
//!   - Wrap LSP calls in try/catch; use `formatError()` to clean up the
//!     raw error before showing it to the user.
//!   - Never swallow errors silently — always surface a user-visible
//!     message. The previous code had several paths where a null client
//!     produced "unknown error" reports because the `?.` chain swallowed
//!     the null.

import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import * as crypto from 'crypto';
import { execSync } from 'child_process';
import {
    KnotLanguageClient,
    KnotBuildResponse,
    KnotCompilerDetectResponse,
    KnotReindexResponse,
    KnotGenerateIfidResponse,
    KnotFormatsListResponse,
    KnotFormatsRefreshResponse,
    KnotGraphResponse,
} from './types';
import { StoryMapPanelManager } from './storyMapProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';
import { VariableFlowProvider } from './variableFlowProvider';
import * as navigation from './navigation';
import {
    extractPassageName,
    getBuildRequestParams,
    getFormatsRefreshParams,
    getManagedTweegoPath,
    getManagedStoryformatsPath,
} from './utils';
import { isWatchActive } from './watchState';

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
// Shared helpers — used by every command below
// ---------------------------------------------------------------------------

/**
 * Returns the running language client or shows a warning and returns null.
 * Use this as the first line of any command that talks to the server.
 */
function requireClient(deps: CommandDeps): KnotLanguageClient | null {
    const client = deps.getClient();
    if (!client || !client.isRunning()) {
        vscode.window.showWarningMessage('Knot: Language server is not running.');
        return null;
    }
    return client;
}

/**
 * Returns the first workspace folder URI (as a string) or shows a warning
 * and returns null. Use this for any command that needs a workspace root.
 */
function requireWorkspace(): string | null {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        vscode.window.showWarningMessage('Knot: No workspace folder open.');
        return null;
    }
    return folders[0].uri.toString();
}

/**
 * Convert any thrown value into a clean single-line error string.
 *
 * LSP rejection errors come back as objects with a `.message` property;
 * `vscode.LanguageClient` sometimes wraps them further. We pull the
 * message out if we can find one, otherwise fall back to `String(e)`.
 * Trailing newlines are stripped so the message fits on one line in
 * the VS Code notification UI.
 */
function formatError(e: unknown): string {
    if (e instanceof Error) {
        return e.message.trim().replace(/\s+/g, ' ');
    }
    if (typeof e === 'object' && e !== null && 'message' in e) {
        const msg = (e as { message: unknown }).message;
        if (typeof msg === 'string') {
            return msg.trim().replace(/\s+/g, ' ');
        }
    }
    return String(e).trim().replace(/\s+/g, ' ');
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Register all Knot commands. */
export function registerCommands(deps: CommandDeps): void {
    const { context } = deps;

    registerOpenStoryMap(deps);
    registerBuild(deps);
    registerPlay(deps);
    registerPlayFromPassage(deps);
    registerRestartServer(deps);
    registerReindexWorkspace(deps);
    registerDetectCompiler(deps);
    registerConfigureStoryFormats(deps);
    registerOpenManagedStorage(deps);
    registerOpenPassageByName(deps);
    registerInitProject(deps);

    // Note: knot.toggleWatch is registered in statusBarItems.ts because the
    // status bar item owns the icon-update logic that has to run alongside
    // the toggle. Registering it here would be dead code — VS Code keeps
    // only the last registration for a given command ID.

    // ── Helper registrations below this point ──────────────────────────
    // Each helper is a closure over `deps` so it can reach the client and
    // context without re-fetching them.

    function registerOpenStoryMap(deps: CommandDeps): void {
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.openStoryMap', async () => {
                if (deps.storyMapPanel) {
                    await deps.storyMapPanel.show();
                }
            }),
        );
    }

    function registerBuild(deps: CommandDeps): void {
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.build', async () => {
                const client = requireClient(deps);
                if (!client) { return; }

                const workspaceUri = requireWorkspace();
                if (!workspaceUri) { return; }

                // Check for Tweego availability — prompts the user to
                // download if no compiler is found.
                const tweegoPath = await ensureTweegoAvailable(context, client);
                if (!tweegoPath) { return; }

                const buildParams = getBuildRequestParams(workspaceUri);
                if (!buildParams.compiler_path && tweegoPath) {
                    buildParams.compiler_path = tweegoPath;
                }

                try {
                    const result = await client.sendRequest<KnotBuildResponse>(
                        'knot/build',
                        buildParams,
                    );
                    if (result.success) {
                        vscode.window.showInformationMessage('Knot: Build succeeded!');
                        return;
                    }

                    // Build failed — check if it was due to a missing story
                    // format. If so, offer a one-click download button and
                    // retry automatically. Avoids forcing the user through
                    // the Configure Story Formats UI just to download one
                    // format.
                    const errorText = result.errors?.join(' ') || '';
                    const looksLikeFormatError =
                        errorText.toLowerCase().includes('story format')
                        || errorText.toLowerCase().includes('format not found');

                    if (looksLikeFormatError) {
                        await offerFormatDownloadAndRetry(
                            client,
                            context,
                            workspaceUri,
                            buildParams,
                        );
                        return;
                    }

                    vscode.window.showErrorMessage(
                        `Knot: Build failed: ${result.errors?.join(', ') || 'unknown error'}`,
                    );
                } catch (e) {
                    vscode.window.showErrorMessage(
                        `Knot: Build request failed: ${formatError(e)}`,
                    );
                }
            }),
        );
    }

    function registerPlay(deps: CommandDeps): void {
        // Play Story — open the compiled HTML in the default browser.
        // If Watch is ON, just open the existing HTML (Watch keeps it fresh).
        // If Watch is OFF, build first then open.
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.play', async () => {
                const client = requireClient(deps);
                if (!client) { return; }

                const workspaceUriStr = requireWorkspace();
                if (!workspaceUriStr) { return; }
                const workspaceUri = vscode.Uri.parse(workspaceUriStr);

                const watchActive = isWatchActive();
                let htmlPath: string | undefined;

                if (!watchActive) {
                    // Build first
                    const tweegoPath = await ensureTweegoAvailable(context, client);
                    if (!tweegoPath) { return; }

                    const buildParams = getBuildRequestParams(workspaceUriStr);
                    if (!buildParams.compiler_path && tweegoPath) {
                        buildParams.compiler_path = tweegoPath;
                    }

                    try {
                        const result = await client.sendRequest<KnotBuildResponse>(
                            'knot/build',
                            buildParams,
                        );
                        if (!result?.success) {
                            vscode.window.showErrorMessage(
                                'Knot: Build failed — '
                                    + (result?.errors?.join('; ') || 'unknown error'),
                            );
                            return;
                        }
                        htmlPath = result.output_path;
                    } catch (e) {
                        vscode.window.showErrorMessage(
                            `Knot: Build request failed: ${formatError(e)}`,
                        );
                        return;
                    }
                } else {
                    // Watch is ON — find the existing HTML in the output dir
                    htmlPath = await findBuiltHtml(workspaceUri);
                }

                if (!htmlPath) {
                    vscode.window.showWarningMessage(
                        'Knot: No built HTML found. '
                            + (watchActive
                                ? 'Save a source file to trigger a build, or use Build first.'
                                : 'Build did not produce an output file.'),
                    );
                    return;
                }

                await vscode.env.openExternal(vscode.Uri.file(htmlPath));
            }),
        );
    }

    function registerPlayFromPassage(deps: CommandDeps): void {
        // Play from Passage — same as Play but with --start <passage>.
        // Always builds (the --start flag changes the output, so we can't
        // just open the existing HTML from Watch).
        context.subscriptions.push(
            vscode.commands.registerCommand(
                'knot.playFromPassage',
                async (passageName?: string) => {
                    const client = requireClient(deps);
                    if (!client) { return; }

                    const workspaceUri = requireWorkspace();
                    if (!workspaceUri) { return; }

                    // If no passage name provided, try to detect from cursor
                    if (!passageName) {
                        const editor = vscode.window.activeTextEditor;
                        if (editor) {
                            passageName = detectPassageAtCursor(editor);
                        }
                    }

                    if (!passageName) {
                        vscode.window.showWarningMessage(
                            'Knot: No passage found at cursor position.',
                        );
                        return;
                    }

                    const tweegoPath = await ensureTweegoAvailable(context, client);
                    if (!tweegoPath) { return; }

                    const buildParams = getBuildRequestParams(workspaceUri, passageName);
                    if (!buildParams.compiler_path && tweegoPath) {
                        buildParams.compiler_path = tweegoPath;
                    }

                    try {
                        const result = await client.sendRequest<KnotBuildResponse>(
                            'knot/build',
                            buildParams,
                        );
                        if (!result?.success) {
                            vscode.window.showErrorMessage(
                                'Knot: Build failed — '
                                    + (result?.errors?.join('; ') || 'unknown error'),
                            );
                            return;
                        }
                        if (result.output_path) {
                            await vscode.env.openExternal(vscode.Uri.file(result.output_path));
                        }
                    } catch (e) {
                        vscode.window.showErrorMessage(
                            `Knot: Build request failed: ${formatError(e)}`,
                        );
                    }
                },
            ),
        );
    }

    function registerRestartServer(deps: CommandDeps): void {
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.restartServer', async () => {
                const client = deps.getClient();
                if (!client) {
                    vscode.window.showWarningMessage('Knot: Language server is not running.');
                    return;
                }

                if (deps.statusBarItem) {
                    deps.statusBarItem.text = '$(sync~spin) Knot: Restarting...';
                    deps.statusBarItem.show();
                }

                try {
                    await client.stop();
                    // Allow in-flight requests to complete before starting a
                    // new server instance. The previous 500 ms was too short
                    // — read-lock handlers could still be blocked when the
                    // transport was torn down, causing
                    // "Cannot call write after a stream was destroyed".
                    await new Promise(resolve => setTimeout(resolve, 2000));
                    await client.start();
                    vscode.window.showInformationMessage('Knot language server restarted.');
                } catch (e) {
                    const msg = formatError(e);
                    // Cosmetic error from late LSP responses writing to a
                    // destroyed transport stream during restart. The stop
                    // completed; the new client will start fine. Suppress
                    // and retry once.
                    if (msg.includes('write after a stream was destroyed')) {
                        try {
                            await new Promise(resolve => setTimeout(resolve, 1000));
                            await client.start();
                            vscode.window.showInformationMessage(
                                'Knot language server restarted.',
                            );
                        } catch (e2) {
                            vscode.window.showErrorMessage(
                                `Failed to restart Knot server: ${formatError(e2)}`,
                            );
                        }
                    } else {
                        vscode.window.showErrorMessage(
                            `Failed to restart Knot server: ${msg}`,
                        );
                    }
                }
            }),
        );
    }

    function registerReindexWorkspace(deps: CommandDeps): void {
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.reindexWorkspace', async () => {
                const client = requireClient(deps);
                if (!client) { return; }

                const workspaceUri = requireWorkspace();
                if (!workspaceUri) { return; }

                if (deps.statusBarItem) {
                    deps.statusBarItem.text = '$(sync~spin) Knot: Re-indexing...';
                    deps.statusBarItem.show();
                }

                try {
                    const result = await client.sendRequest<KnotReindexResponse>(
                        'knot/reindexWorkspace',
                        { workspace_uri: workspaceUri },
                    );
                    if (result && !result.success) {
                        vscode.window.showWarningMessage(
                            `Knot: Re-index had issues: ${result.error || 'unknown'}`,
                        );
                    } else {
                        vscode.window.showInformationMessage(
                            `Knot: Re-indexed ${result?.files_indexed || 0} files.`,
                        );
                    }
                } catch (e) {
                    vscode.window.showErrorMessage(
                        `Failed to re-index workspace: ${formatError(e)}`,
                    );
                }
            }),
        );
    }

    function registerDetectCompiler(deps: CommandDeps): void {
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.detectCompiler', async () => {
                const client = requireClient(deps);
                if (!client) { return; }

                const workspaceUri = requireWorkspace();
                if (!workspaceUri) { return; }

                try {
                    const result = await client.sendRequest<KnotCompilerDetectResponse>(
                        'knot/compilerDetect',
                        { workspace_uri: workspaceUri },
                    );
                    if (result.compiler_found) {
                        vscode.window.showInformationMessage(
                            `Knot: Compiler found — ${result.compiler_name} `
                                + `${result.compiler_version || ''} at ${result.compiler_path}`,
                        );
                    } else {
                        vscode.window.showWarningMessage(
                            'Knot: No Twine compiler found. Install Tweego and add it to PATH, '
                                + 'or set knot.build.tweegoPath in Settings.',
                        );
                    }
                } catch (e) {
                    vscode.window.showErrorMessage(
                        `Knot: Compiler detection failed: ${formatError(e)}`,
                    );
                }
            }),
        );
    }

    function registerOpenManagedStorage(deps: CommandDeps): void {
        // Open the extension's managed storage folder in the OS file explorer.
        // This is where the managed tweego binary and versioned storyformat
        // cache live. If nothing has been downloaded yet, offer to download
        // tweego instead of opening an empty folder.
        context.subscriptions.push(
            vscode.commands.registerCommand('knot.openManagedStorage', async () => {
                const managedTweego = getManagedTweegoPath();
                const managedSf = getManagedStoryformatsPath();

                if (!managedTweego && !managedSf) {
                    const choice = await vscode.window.showInformationMessage(
                        'Knot: Nothing has been downloaded yet. Download Tweego now?',
                        'Download',
                        'Cancel',
                    );
                    if (choice === 'Download') {
                        await downloadTweego(deps.context);
                    }
                    return;
                }

                vscode.commands.executeCommand(
                    'revealFileInOS',
                    deps.context.globalStorageUri,
                );
            }),
        );
    }

    function registerOpenPassageByName(deps: CommandDeps): void {
        // Used by the Debug view, Variable Flow view, and diagnostics to
        // jump to a passage. The line/span args are optional — when
        // provided, navigation lands on the specific reference instead of
        // the passage header.
        //
        // When invoked from the Command Palette (no args), prompts the
        // user with a QuickPick of all passages in the workspace. This
        // makes the command useful as a standalone "Go to passage..."
        // action, not just an internal callback target.
        context.subscriptions.push(
            vscode.commands.registerCommand(
                'knot.openPassageByName',
                async (
                    passageName?: string,
                    targetLine?: number,
                    spanStart?: number,
                    spanEnd?: number,
                ) => {
                    const client = requireClient(deps);
                    if (!client) { return; }

                    // If called from the palette with no args, prompt.
                    if (!passageName) {
                        passageName = await promptForPassage(client);
                        if (!passageName) { return; }
                    }

                    await navigation.navigateToPassage(
                        passageName,
                        targetLine,
                        spanStart,
                        spanEnd,
                    );
                },
            ),
        );
    }

    function registerConfigureStoryFormats(deps: CommandDeps): void {
        // Interactive UI for managing the storyformats directory and the
        // installed formats catalog. Lets the user:
        //   - See the currently resolved directory and the formats installed there
        //   - Browse for a different folder (preview before saving)
        //   - Clear the configured path (revert to auto-discovery)
        //   - Open the Settings UI at the knot.build.storyformatsPath field
        //   - Refresh the catalog after manually adding/removing format dirs
        //   - One-click download of the format StoryData says the project needs
        context.subscriptions.push(
            vscode.commands.registerCommand(
                'knot.configureStoryFormats',
                async () => {
                    const client = requireClient(deps);
                    if (!client) { return; }

                    const workspaceUri = requireWorkspace();
                    if (!workspaceUri) { return; }

                    try {
                        // Refresh first so the catalog reflects any
                        // filesystem changes since the last call.
                        await client.sendRequest<KnotFormatsRefreshResponse>(
                            'knot/formats/refresh',
                            getFormatsRefreshParams(workspaceUri),
                        );

                        const result = await client.sendRequest<KnotFormatsListResponse>(
                            'knot/formats/list',
                            { workspace_uri: workspaceUri },
                        );

                        const selection = await showFormatsQuickPick(
                            deps.context,
                            client,
                            workspaceUri,
                            result,
                        );
                        if (!selection) { return; }

                        await handleFormatsSelection(
                            deps.context,
                            client,
                            workspaceUri,
                            result,
                            selection,
                        );
                    } catch (e) {
                        vscode.window.showErrorMessage(
                            `Knot: Failed to configure story formats: ${formatError(e)}`,
                        );
                    }
                },
            ),
        );
    }

    function registerInitProject(deps: CommandDeps): void {
        // Initialize Project — generates a starter Twine project skeleton
        // (src/main.tw, .vscode/knot.json, styles/story.css) with format-
        // specific content for SugarCube, Harlowe, Chapbook, or Snowman.
        const initProject = vscode.commands.registerCommand(
            'knot.initProject',
            async () => {
                const client = deps.getClient();
                const folders = vscode.workspace.workspaceFolders;
                if (!folders) {
                    vscode.window.showErrorMessage(
                        'Please open a workspace folder first.',
                    );
                    return;
                }
                const rootUri = folders[0].uri;

                // Step 1: Select story format
                const formatItems: vscode.QuickPickItem[] = [
                    {
                        label: 'SugarCube 2',
                        description: 'Most popular, full-featured format',
                        detail: 'Best for complex stories with variables, macros, and state management',
                    },
                    {
                        label: 'Harlowe 3',
                        description: 'Built-in Twine 2 format',
                        detail: 'Beginner-friendly, uses markup-based syntax',
                    },
                    {
                        label: 'Chapbook',
                        description: 'Simple, modern format',
                        detail: 'Uses markdown-style syntax with state management',
                    },
                    {
                        label: 'Snowman',
                        description: 'Developer-oriented format',
                        detail: 'Uses JavaScript and Underscore.js templating',
                    },
                ];

                const selectedFormat = await vscode.window.showQuickPick(formatItems, {
                    placeHolder: 'Select your story format',
                    title: 'Knot: Initialize Twine Project',
                });
                if (!selectedFormat) { return; }

                const formatName = selectedFormat.label.split(' ')[0];

                // Step 2: Story title
                const storyTitle = await vscode.window.showInputBox({
                    prompt: 'Enter your story title',
                    value: 'My Story',
                    title: 'Knot: Initialize Twine Project',
                });
                if (!storyTitle) { return; }

                // Step 3: Generate project files
                try {
                    await generateProjectSkeleton(deps.context, client, rootUri, formatName, storyTitle);
                } catch (e) {
                    vscode.window.showErrorMessage(
                        `Failed to initialize project: ${formatError(e)}`,
                    );
                }
            },
        );
        context.subscriptions.push(initProject);
    }
}

// ---------------------------------------------------------------------------
// Build helpers
// ---------------------------------------------------------------------------

/**
 * If the build failed with a "story format not found" error, look up the
 * project's format+version from StoryData (via knot/formats/list), offer
 * a one-click download, and retry the build automatically.
 *
 * No-op (returns silently) if the failure wasn't a format error or if the
 * user declines the download. Falls through to a generic error message in
 * the calling command.
 */
async function offerFormatDownloadAndRetry(
    client: KnotLanguageClient,
    context: vscode.ExtensionContext,
    workspaceUri: string,
    buildParams: Record<string, unknown>,
): Promise<void> {
    try {
        const fmtResult = await client.sendRequest<KnotFormatsListResponse>(
            'knot/formats/list',
            { workspace_uri: workspaceUri },
        );

        // Only offer auto-download for SugarCube — that's the only format
        // we have a download URL for. The QuickPick in
        // `knot.configureStoryFormats` surfaces a "manual install required"
        // message for the other formats.
        if (
            !fmtResult.project_format
            || !fmtResult.project_format_version
            || fmtResult.project_format_cached === true
            || fmtResult.project_format !== 'SugarCube'
        ) {
            return;
        }

        const fmt = fmtResult.project_format;
        const ver = fmtResult.project_format_version;
        const choice = await vscode.window.showErrorMessage(
            `Knot: Build failed — ${fmt} v${ver} is not installed. Download it now?`,
            'Download',
            'Close',
        );
        if (choice !== 'Download') { return; }

        const cacheDir = await downloadStoryFormat(context, fmt, ver);
        if (!cacheDir) { return; }

        await client.sendRequest<KnotFormatsRefreshResponse>(
            'knot/formats/refresh',
            getFormatsRefreshParams(workspaceUri),
        );

        // Retry the build automatically with the same params.
        const retryResult = await client.sendRequest<KnotBuildResponse>(
            'knot/build',
            buildParams,
        );
        if (retryResult.success) {
            vscode.window.showInformationMessage(
                'Knot: Build succeeded after format download!',
            );
        } else {
            vscode.window.showErrorMessage(
                `Knot: Build still failing: ${retryResult.errors?.join(', ') || 'unknown error'}`,
            );
        }
    } catch {
        // Fall through to generic error in the caller. Don't surface a
        // nested "download also failed" message — that's noise.
    }
}

// ---------------------------------------------------------------------------
// Cursor / passage detection
// ---------------------------------------------------------------------------

/**
 * Check whether a file exists at the given URI.
 */
async function fileExists(uri: vscode.Uri): Promise<boolean> {
    try {
        await vscode.workspace.fs.stat(uri);
        return true;
    } catch {
        return false;
    }
}

/**
 * Scan the workspace for a `:: StoryData` passage — the universal marker
 * of an existing Twine project. Returns the URI of the first file that
 * contains one, or undefined if none is found.
 *
 * Used by `initProject` to guard against initializing a project that
 * already exists. Limited to scanning the first 50 .tw/.twee files and
 * the first 200 lines of each (StoryData is conventionally near the top
 * of the file) — keeps the check fast even for large workspaces.
 */
async function findExistingStoryData(_rootUri: vscode.Uri): Promise<vscode.Uri | undefined> {
    const files = await vscode.workspace.findFiles('**/*.{tw,twee}', '**/node_modules/**', 50);
    for (const fileUri of files) {
        try {
            const doc = await vscode.workspace.openTextDocument(fileUri);
            const maxLines = Math.min(doc.lineCount, 200);
            for (let i = 0; i < maxLines; i++) {
                const line = doc.lineAt(i).text;
                if (line.startsWith('::')) {
                    // Extract the passage name from the header — strip
                    // tags [...] and metadata {...} the same way the
                    // Rust-side extract_passage_name does.
                    let name = line.replace(/^::\s*/, '');
                    const braceStart = name.indexOf('{');
                    if (braceStart >= 0) { name = name.substring(0, braceStart); }
                    const bracketStart = name.indexOf('[');
                    if (bracketStart >= 0) { name = name.substring(0, bracketStart); }
                    name = name.trim().toLowerCase();
                    if (name === 'storydata') {
                        return fileUri;
                    }
                }
            }
        } catch {
            // Skip files that can't be opened (binary, permission, etc.)
        }
    }
    return undefined;
}

/**
 * Find the name of the passage containing the cursor in the active editor.
 * Returns undefined if no `::` header is found above the cursor line.
 */
function detectPassageAtCursor(editor: vscode.TextEditor): string | undefined {
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
    return currentPassage;
}

/**
 * Show a QuickPick of all passages in the workspace, sorted with special
 * passages (Start, StoryTitle, StoryData) and the start passage at the top.
 *
 * Used by `knot.openPassageByName` when invoked from the Command Palette
 * with no arguments. Fetches the passage list from the server's graph
 * endpoint — no client-side file scanning needed.
 *
 * @returns The selected passage name, or undefined if the user dismissed
 *          the picker or the server returned no passages.
 */
async function promptForPassage(client: KnotLanguageClient): Promise<string | undefined> {
    const workspaceUri = requireWorkspace();
    if (!workspaceUri) { return undefined; }

    let graph: KnotGraphResponse;
    try {
        graph = await client.sendRequest<KnotGraphResponse>('knot/graph', {
            workspace_uri: workspaceUri,
        });
    } catch (e) {
        vscode.window.showErrorMessage(
            `Knot: Failed to load passage list: ${formatError(e)}`,
        );
        return undefined;
    }

    if (!graph.nodes || graph.nodes.length === 0) {
        vscode.window.showInformationMessage(
            'Knot: No passages found in this workspace.',
        );
        return undefined;
    }

    // Build QuickPick items. Special passages and the start passage get
    // pinned to the top with icons so they're easy to find.
    const items: (vscode.QuickPickItem & { passageName: string })[] = graph.nodes
        .map((n) => {
            const badges: string[] = [];
            if (n.is_start) { badges.push('$(play)'); }
            if (n.is_special) { badges.push('$(star)'); }
            if (n.is_metadata) { badges.push('$(info)'); }
            if (n.is_unreachable) { badges.push('$(warning)'); }
            return {
                label: `${badges.join(' ')}${n.label}`,
                description: n.tags.length > 0 ? `[${n.tags.join(', ')}]` : '',
                detail: `${n.file.split('/').pop() || n.file}:${n.line + 1}`,
                passageName: n.id,
            };
        })
        .sort((a, b) => {
            // Sort: start passage first, then special passages, then alphabetical
            // The $(play) badge marks the start passage
            const aStart = a.label.startsWith('$(play)');
            const bStart = b.label.startsWith('$(play)');
            if (aStart && !bStart) { return -1; }
            if (!aStart && bStart) { return 1; }
            return a.label.localeCompare(b.label);
        });

    const selection = await vscode.window.showQuickPick(items, {
        placeHolder: 'Select a passage to open',
        matchOnDescription: true,
        matchOnDetail: true,
    });

    return selection?.passageName;
}

// ---------------------------------------------------------------------------
// HTML file discovery (for Play when Watch is active)
// ---------------------------------------------------------------------------

/**
 * Find the most recently modified .html file in the build output directory.
 * Used by `knot.play` when Watch is active — the watcher keeps the HTML
 * fresh, so we just need to find and open it.
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

        if (htmlFiles.length === 0) { return undefined; }

        // Pick the most recently modified HTML file. If the user has
        // multiple HTML files in build/ (e.g. they copied one in), we
        // assume the freshest one is the story Knot just built.
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
// Configure Story Formats — QuickPick UI
// ---------------------------------------------------------------------------

interface FormatsQuickPickItem extends vscode.QuickPickItem {
    action?: string;
    format?: KnotFormatsListResponse['formats'][number];
}

/**
 * Build and show the Configure Story Formats QuickPick. Returns the
 * selected item, or undefined if the user dismissed the picker.
 */
async function showFormatsQuickPick(
    context: vscode.ExtensionContext,
    _client: KnotLanguageClient,
    _workspaceUri: string,
    result: KnotFormatsListResponse,
): Promise<FormatsQuickPickItem | undefined> {
    const items: FormatsQuickPickItem[] = [];

    // ── Current State section ──────────────────────────────────────────
    items.push({
        label: 'Current State',
        kind: vscode.QuickPickItemKind.Separator,
    });

    const managedRoot = context.globalStorageUri.fsPath;
    items.push({
        label: '$(folder) Managed storage:',
        description: managedRoot,
        detail: 'Extension-managed tweego binary + versioned storyformat cache',
        action: 'openManagedStorage',
    });

    const configuredPath = result.configured_path;
    if (configuredPath) {
        items.push({
            label: `$(settings) Configured path: ${configuredPath}`,
            description: 'From Build: Story Formats Path setting',
            action: 'openSettings',
        });
    } else {
        items.push({
            label: '$(info) No path configured — using auto-discovery',
            description: result.resolved_dir
                ? `Resolved to: ${result.resolved_dir}`
                : 'No storyformats directory found',
            action: 'openSettings',
        });
    }

    if (result.formats.length > 0) {
        items.push({
            label: `Installed Formats (${result.formats.length})`,
            kind: vscode.QuickPickItemKind.Separator,
        });
        for (const f of result.formats) {
            items.push({
                label: `$(package) ${f.name} v${f.version}`,
                description: f.dir_name,
                detail: [f.author, f.license, f.source].filter(Boolean).join(' • '),
                format: f,
            });
        }
    } else if (result.resolved_dir) {
        items.push({
            label: '$(warning) No formats found in the resolved directory',
            description: result.resolved_dir,
        });
    } else {
        items.push({
            label: '$(warning) No storyformats directory resolved',
            description: 'Builds will likely fail with "story format not found"',
        });
    }

    // ── Actions section ────────────────────────────────────────────────
    items.push({
        label: 'Actions',
        kind: vscode.QuickPickItemKind.Separator,
    });

    // Dynamic download action based on what StoryData says the project needs.
    if (result.project_format && result.project_format_version) {
        const fmt = result.project_format;
        const ver = result.project_format_version;
        if (result.project_format_cached) {
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
            description: 'Revert to auto-discovery',
            action: 'clear',
        });
    }
    items.push({
        label: '$(gear) Open Settings',
        description: 'Edit Build: Story Formats Path directly in the Settings UI',
        action: 'openSettings',
    });

    return vscode.window.showQuickPick(items, {
        placeHolder: 'Configure story formats for Knot builds',
        canPickMany: false,
    });
}

/**
 * Handle the user's selection from the Configure Story Formats QuickPick.
 * Each action is implemented inline; the `result` param is the original
 * formats/list response (needed for downloadProjectFormat).
 */
async function handleFormatsSelection(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient,
    workspaceUri: string,
    result: KnotFormatsListResponse,
    selection: FormatsQuickPickItem,
): Promise<void> {
    const action = selection.action;
    if (!action) { return; }

    if (action === 'openSettings') {
        await vscode.commands.executeCommand(
            'workbench.action.openSettings',
            'knot.build.storyformatsPath',
        );
        return;
    }

    if (action === 'openManagedStorage') {
        vscode.commands.executeCommand('revealFileInOS', context.globalStorageUri);
        return;
    }

    if (action === 'browse') {
        await browseAndSaveStoryFormatsPath(client, workspaceUri);
        return;
    }

    if (action === 'refresh') {
        try {
            const refreshResult = await client.sendRequest<KnotFormatsRefreshResponse>(
                'knot/formats/refresh',
                getFormatsRefreshParams(workspaceUri),
            );
            if (refreshResult.success) {
                vscode.window.showInformationMessage(
                    `Knot: Refreshed — ${refreshResult.format_count} format(s) from `
                        + `${refreshResult.resolved_dir || '(none)'}`,
                );
            } else {
                vscode.window.showErrorMessage(
                    `Knot: Refresh failed — ${refreshResult.error || 'unknown error'}`,
                );
            }
        } catch (e) {
            vscode.window.showErrorMessage(
                `Knot: Refresh failed: ${formatError(e)}`,
            );
        }
        return;
    }

    if (action === 'clear') {
        const config = vscode.workspace.getConfiguration('knot');
        await config.update(
            'build.storyformatsPath',
            '',
            vscode.ConfigurationTarget.Global,
        );
        await client.sendRequest<KnotFormatsRefreshResponse>(
            'knot/formats/refresh',
            getFormatsRefreshParams(workspaceUri),
        );
        vscode.window.showInformationMessage(
            'Knot: Cleared story formats path — using auto-discovery.',
        );
        return;
    }

    if (action === 'downloadProjectFormat') {
        const fmt = result.project_format!;
        const ver = result.project_format_version!;
        const cacheDir = await downloadStoryFormat(context, fmt, ver);
        if (cacheDir) {
            await client.sendRequest<KnotFormatsRefreshResponse>(
                'knot/formats/refresh',
                getFormatsRefreshParams(workspaceUri),
            );
        }
        return;
    }

    if (action === 'downloadManual') {
        const formatName = await vscode.window.showQuickPick(
            ['SugarCube', 'Harlowe', 'Chapbook', 'Snowman'],
            { placeHolder: 'Select a story format to download' },
        );
        if (!formatName) { return; }

        if (formatName !== 'SugarCube') {
            vscode.window.showInformationMessage(
                `Knot: Auto-download is currently only available for SugarCube. `
                    + `For ${formatName}, please download it manually and use 'Browse for folder' to install.`,
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
                getFormatsRefreshParams(workspaceUri),
            );
        }
        return;
    }
}

/**
 * Show a folder picker, preview what's in the chosen directory, and if the
 * user confirms, save the path to `knot.build.storyformatsPath` and
 * trigger a server-side refresh.
 */
async function browseAndSaveStoryFormatsPath(
    client: KnotLanguageClient,
    workspaceUri: string,
): Promise<void> {
    const folderUri = await vscode.window.showOpenDialog({
        canSelectFiles: false,
        canSelectFolders: true,
        canSelectMany: false,
        openLabel: 'Use this story formats folder',
        title: 'Select a directory containing story format subdirectories (e.g. sugarcube-2/, harlowe-3/)',
    });
    if (!folderUri || folderUri.length === 0) { return; }
    const selectedPath = folderUri[0].fsPath;

    // Preview what's in that directory before saving.
    let preview: KnotFormatsListResponse;
    try {
        preview = await client.sendRequest<KnotFormatsListResponse>('knot/formats/list', {
            workspace_uri: workspaceUri,
            path_override: selectedPath,
        });
    } catch (e) {
        vscode.window.showErrorMessage(
            `Knot: Failed to preview folder: ${formatError(e)}`,
        );
        return;
    }

    if (preview.formats.length === 0) {
        const proceed = await vscode.window.showWarningMessage(
            `No format.js files found in subdirectories of:\n${selectedPath}\n\nSave this path anyway?`,
            { modal: false },
            'Save anyway',
            'Cancel',
        );
        if (proceed !== 'Save anyway') { return; }
    } else {
        const formatList = preview.formats
            .map(f => `  • ${f.name} v${f.version}`)
            .join('\n');
        const proceed = await vscode.window.showInformationMessage(
            `Found ${preview.formats.length} format(s) in:\n${selectedPath}\n\n${formatList}\n\nSave this as the story formats path?`,
            { modal: false },
            'Save',
            'Cancel',
        );
        if (proceed !== 'Save') { return; }
    }

    const config = vscode.workspace.getConfiguration('knot');
    await config.update(
        'build.storyformatsPath',
        selectedPath,
        vscode.ConfigurationTarget.Global,
    );

    try {
        await client.sendRequest<KnotFormatsRefreshResponse>(
            'knot/formats/refresh',
            getFormatsRefreshParams(workspaceUri),
        );
        vscode.window.showInformationMessage(
            `Knot: Story formats path saved. ${preview.formats.length} format(s) discovered.`,
        );
    } catch (e) {
        vscode.window.showErrorMessage(
            `Knot: Path saved, but server refresh failed: ${formatError(e)}`,
        );
    }
}

// ---------------------------------------------------------------------------
// Project initialization
// ---------------------------------------------------------------------------

/**
 * Generate a starter Twine project skeleton in the workspace root:
 *   - src/main.tw      — StoryTitle, StoryData, Start + sample passages
 *   - .vscode/knot.json — project config with diagnostics defaults
 *   - styles/story.css — empty stylesheet for custom styles
 *
 * Then open src/main.tw in the editor.
 *
 * Safeguards: before writing, scans the workspace for an existing
 * `:: StoryData` passage — the true marker of an existing Twine project.
 * If found, the user is prompted to confirm before creating a new
 * StoryData (which would conflict with the existing one). Also checks
 * whether `src/main.tw` already exists (the file we're about to write).
 */
async function generateProjectSkeleton(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient | null,
    rootUri: vscode.Uri,
    formatName: string,
    storyTitle: string,
): Promise<void> {
    // ── Safeguard: check for existing Twine project ──────────────────
    //
    // The presence of a `:: StoryData` passage anywhere in the workspace
    // means this is already a Twine project. Initializing would create a
    // second StoryData, causing conflicts (duplicate passage names, IFID
    // mismatches, etc.). We also check if src/main.tw already exists
    // since that's the specific file we're about to overwrite.
    const mainFile = vscode.Uri.joinPath(rootUri, 'src', 'main.tw');
    const existingStoryDataFile = await findExistingStoryData(rootUri);

    if (existingStoryDataFile || await fileExists(mainFile)) {
        const conflicts: string[] = [];
        if (existingStoryDataFile) {
            const relPath = vscode.workspace.asRelativePath(existingStoryDataFile);
            conflicts.push(`StoryData passage in ${relPath}`);
        }
        try {
            await vscode.workspace.fs.stat(mainFile);
            conflicts.push('src/main.tw');
        } catch {
            // Doesn't exist
        }

        const proceed = await vscode.window.showWarningMessage(
            `Knot: This workspace already contains: ${conflicts.join(', ')}. `
                + 'Initializing will create a new StoryData passage and overwrite src/main.tw. Continue?',
            { modal: true },
            'Overwrite',
            'Cancel',
        );
        if (proceed !== 'Overwrite') {
            vscode.window.showInformationMessage('Knot: Project initialization cancelled.');
            return;
        }
    }

    // Create directory structure
    const srcDir = vscode.Uri.joinPath(rootUri, 'src');
    const assetsDir = vscode.Uri.joinPath(rootUri, 'assets');
    const stylesDir = vscode.Uri.joinPath(rootUri, 'styles');
    await vscode.workspace.fs.createDirectory(srcDir);
    await vscode.workspace.fs.createDirectory(assetsDir);
    await vscode.workspace.fs.createDirectory(stylesDir);

    // Generate IFID — prefer server-side generator for consistency with
    // Workspace::generate_ifid(), fall back to local crypto if server
    // is unavailable. The init command runs before the server may be
    // fully started, so the fallback is important.
    let ifid: string;
    try {
        const result = await client?.sendRequest<KnotGenerateIfidResponse>(
            'knot/generateIfid',
            { workspace_uri: rootUri.toString() },
        );
        ifid = result?.ifid || crypto.randomUUID().toUpperCase();
    } catch {
        ifid = crypto.randomUUID().toUpperCase();
    }

    // Generate main.tw content
    const mainContent = buildMainTwContent(formatName, storyTitle, ifid);

    // Write main.tw (mainFile was already declared in the safeguard check above)
    await vscode.workspace.fs.writeFile(mainFile, new TextEncoder().encode(mainContent));

    // Generate .vscode/knot.json config
    //
    // Set source_dir to "src" and output_dir to "build" so tweego scans
    // only src/ for .twee files and writes to build/. This prevents the
    // common "Output file cannot be an input source" error that occurs
    // when the workspace root is the source AND contains the build/ dir.
    // The recommended layout is: workspace contains src/ + build/, with
    // src/ holding all .twee/.js/.css files.
    const knotConfig = {
        format: formatName,
        build: {
            source_dir: 'src',
            output_dir: 'build',
        },
        compiler: { path: '', args: [] },
        diagnostics: {
            'broken-link': 'warning',
            'unreachable-passage': 'hint',
            'uninitialized-variable': 'warning',
            'unused-variable': 'hint',
        },
    };
    const vscodeDir = vscode.Uri.joinPath(rootUri, '.vscode');
    await vscode.workspace.fs.createDirectory(vscodeDir);
    const knotConfigFile = vscode.Uri.joinPath(vscodeDir, 'knot.json');
    await vscode.workspace.fs.writeFile(
        knotConfigFile,
        new TextEncoder().encode(JSON.stringify(knotConfig, null, 2)),
    );

    // Generate styles/story.css
    const cssFile = vscode.Uri.joinPath(stylesDir, 'story.css');
    await vscode.workspace.fs.writeFile(
        cssFile,
        new TextEncoder().encode('/* Custom story styles */\n'),
    );

    vscode.window.showInformationMessage(
        `Knot: Initialized ${formatName} project "${storyTitle}" successfully!`,
    );

    // Open the main file
    const doc = await vscode.workspace.openTextDocument(mainFile);
    await vscode.window.showTextDocument(doc);

    // Touch context for type-only usage — context is needed for future
    // per-workspace activation hooks, but not currently used in this fn.
    void context;
}

/**
 * Build the content of src/main.tw for a new project. The Start passage
 * contains format-specific sample content so the user can immediately
 * run `knot.play` and see something working.
 */
function buildMainTwContent(formatName: string, storyTitle: string, ifid: string): string {
    const formatVersion =
        formatName === 'SugarCube' ? '2.36.1'
        : formatName === 'Harlowe' ? '3.3.0'
        : formatName === 'Chapbook' ? '1.2.1'
        : '1.4.0'; // Snowman

    let content = '';
    content += `:: StoryTitle\n${storyTitle}\n\n`;
    content += `:: StoryData\n`;
    content += JSON.stringify(
        {
            ifid,
            format: formatName,
            'format-version': formatVersion,
            start: 'Start',
            zoom: 1,
        },
        null,
        2,
    );
    content += '\n\n';

    content += `:: Start\n`;
    switch (formatName) {
        case 'SugarCube':
            content += `Welcome to ${storyTitle}.\n\n`;
            content += `<<set $playerName to "">>\n`;
            content += `<<set $score to 0>>\n\n`;
            content += `[[Enter the story->First Passage]]\n\n`;
            content += `:: First Passage\n`;
            content += `You find yourself at the beginning of your adventure.\n\n`;
            content += `<<if $score eq 0>>You have no points yet.<<else>>You have $score points.<</if>>\n\n`;
            content += `<<set $score to $score + 1>>\n\n`;
            content += `[[Continue->Second Passage]]\n\n`;
            content += `:: Second Passage\n`;
            content += `The story continues from here.\n\n`;
            content += `[[Go back->Start]]\n`;
            break;
        case 'Harlowe':
            content += `Welcome to ${storyTitle}.\n\n`;
            content += `(set: $playerName to "")\n`;
            content += `(set: $score to 0)\n\n`;
            content += `[[Enter the story->First Passage]]\n\n`;
            content += `:: First Passage\n`;
            content += `You find yourself at the beginning of your adventure.\n\n`;
            content += `(if: $score is 0)[You have no points yet.](else:)[You have $score points.]\n\n`;
            content += `(set: $score to it + 1)\n\n`;
            content += `[[Continue->Second Passage]]\n\n`;
            content += `:: Second Passage\n`;
            content += `The story continues from here.\n\n`;
            content += `[[Go back->Start]]\n`;
            break;
        case 'Chapbook':
            content += `Welcome to ${storyTitle}.\n\n`;
            content += `[javascript]\nstate.score = 0;\n[/javascript]\n\n`;
            content += `[[First Passage]]\n\n`;
            content += `:: First Passage\n`;
            content += `You find yourself at the beginning of your adventure.\n\n`;
            content += `Your score is {state.score}.\n\n`;
            content += `[javascript]\nstate.score = state.score + 1;\n[/javascript]\n\n`;
            content += `[[Second Passage]]\n\n`;
            content += `:: Second Passage\n`;
            content += `The story continues from here.\n\n`;
            content += `[[Start]]\n`;
            break;
        case 'Snowman':
            content += `Welcome to ${storyTitle}.\n\n`;
            content += `<% s.score = 0; %>\n\n`;
            content += `[[First Passage]]\n\n`;
            content += `:: First Passage\n`;
            content += `You find yourself at the beginning of your adventure.\n\n`;
            content += `<p>Your score is <%= s.score %>.</p>\n\n`;
            content += `<% s.score += 1; %>\n\n`;
            content += `[[Second Passage]]\n\n`;
            content += `:: Second Passage\n`;
            content += `The story continues from here.\n\n`;
            content += `[[Start]]\n`;
            break;
    }
    return content;
}

// ---------------------------------------------------------------------------
// Tweego compiler availability
// ---------------------------------------------------------------------------

/**
 * Check if Tweego is available; prompt to download if not.
 *
 * Resolution order:
 *   1. VS Code setting `knot.build.tweegoPath` (persisted user preference)
 *   2. Knot-managed binary in globalStorage (downloaded by the extension)
 *   3. Language server detection (PATH lookup via `which`/`where`)
 *   4. Prompt user: Download | Set Path Manually | Open Storage | Cancel
 *
 * When a path is found via download or manual selection, the managed
 * download is NOT persisted to `knot.build.tweegoPath` — that setting is
 * the user's override. The managed path is checked automatically by
 * `getBuildRequestParams()` so subsequent builds don't re-prompt.
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
        const result = await client?.sendRequest<KnotCompilerDetectResponse>(
            'knot/compilerDetect',
            { workspace_uri: '' },
        );
        if (result && result.compiler_found && result.compiler_path) {
            return result.compiler_path;
        }
    } catch {
        // Server may not be fully ready yet — fall through to prompt
    }

    // 4. Prompt user: Download (managed) | Set Path Manually | Open Storage | Cancel
    const choice = await vscode.window.showWarningMessage(
        'Tweego compiler not found. Knot needs Tweego to build and preview Twine stories.',
        'Download Tweego',
        'Set Path Manually',
        'Open Storage Folder',
        'Cancel',
    );

    if (choice === 'Download Tweego') {
        return await downloadTweego(context);
    }
    if (choice === 'Open Storage Folder') {
        vscode.commands.executeCommand('revealFileInOS', context.globalStorageUri);
        return undefined;
    }
    if (choice === 'Set Path Manually') {
        const fileUri = await vscode.window.showOpenDialog({
            canSelectFiles: true,
            canSelectFolders: false,
            canSelectMany: false,
            title: 'Select Tweego binary',
            filters: process.platform === 'win32'
                ? { 'Executable': ['exe'] }
                : { 'All Files': ['*'] },
        });
        if (fileUri && fileUri[0]) {
            const selectedPath = fileUri[0].fsPath;
            // Make executable on Unix
            if (process.platform !== 'win32') {
                try {
                    fs.chmodSync(selectedPath, 0o755);
                } catch {
                    // Ignore — the build will fail with a clear error if
                    // the binary isn't executable.
                }
            }
            // Trust the user's selection — don't validate by running --version.
            // Tweego exits non-zero on --version when it can't find .storyformats,
            // which makes validation unreliable.
            await config.update(
                'build.tweegoPath',
                selectedPath,
                vscode.ConfigurationTarget.Global,
            );
            vscode.window.showInformationMessage('Tweego path saved.');
            return selectedPath;
        }
    }
    return undefined;
}

// ---------------------------------------------------------------------------
// Tweego download
// ---------------------------------------------------------------------------

/**
 * Download Tweego from GitHub releases and extract it into
 * `<globalStorage>/tweego/`. Cross-platform: uses Node's built-in fetch
 * for download and platform-native extraction (PowerShell on Windows,
 * Python or `unzip` on Unix).
 *
 * After extraction, storyformats that ship inside the tweego zip are
 * moved OUT of the binary directory to `<globalStorage>/storyformats/`.
 * This is critical: tweego's first storyformat search path is
 * `<binary_dir>/storyformats/`. If we leave them there, project-local
 * `<workspace>/storyformats/` overrides can never take priority.
 *
 * @returns Absolute path to the extracted tweego binary, or undefined on failure.
 */
async function downloadTweego(context: vscode.ExtensionContext): Promise<string | undefined> {
    const platform = process.platform;

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

    return vscode.window.withProgress(
        {
            location: vscode.ProgressLocation.Notification,
            title: 'Downloading Tweego...',
            cancellable: true,
        },
        async (progress, _token) => {
            try {
                progress.report({ message: 'Downloading...' });

                const binDir = path.join(context.globalStorageUri.fsPath, 'tweego');
                fs.mkdirSync(binDir, { recursive: true });
                const zipPath = path.join(binDir, 'tweego.zip');

                const response = await fetch(downloadUrl);
                if (!response.ok) {
                    throw new Error(`Download failed: ${response.statusText}`);
                }
                const buffer = Buffer.from(await response.arrayBuffer());
                fs.writeFileSync(zipPath, buffer);

                progress.report({ message: 'Extracting...' });

                extractZip(zipPath, binDir, platform);

                // The zip may contain the binary at the root or in a
                // subdirectory. Find it by walking the extracted tree.
                let binaryPath = path.join(binDir, binaryName);
                if (!fs.existsSync(binaryPath)) {
                    binaryPath = findFileRecursive(binDir, binaryName) || binaryPath;
                }

                if (!fs.existsSync(binaryPath)) {
                    throw new Error(`Binary "${binaryName}" not found in downloaded archive.`);
                }

                // Make executable on Unix
                if (platform !== 'win32') {
                    fs.chmodSync(binaryPath, 0o755);
                }

                // Move storyformats OUT of the tweego binary directory to
                // a separate managed location. See function docstring for
                // the rationale.
                const extractedSfDir = path.join(binDir, 'storyformats');
                if (fs.existsSync(extractedSfDir)) {
                    const managedSfDir = path.join(context.globalStorageUri.fsPath, 'storyformats');
                    fs.mkdirSync(managedSfDir, { recursive: true });

                    for (const entry of fs.readdirSync(extractedSfDir, { withFileTypes: true })) {
                        if (entry.isDirectory()) {
                            const src = path.join(extractedSfDir, entry.name);
                            const dst = path.join(managedSfDir, entry.name);
                            if (fs.existsSync(dst)) {
                                fs.rmSync(dst, { recursive: true, force: true });
                            }
                            fs.renameSync(src, dst);
                        }
                    }
                    fs.rmSync(extractedSfDir, { recursive: true, force: true });
                }

                // Clean up zip
                fs.unlinkSync(zipPath);

                vscode.window.showInformationMessage('Tweego downloaded successfully!');
                return binaryPath;
            } catch (e) {
                vscode.window.showErrorMessage(`Failed to download Tweego: ${formatError(e)}`);
                return undefined;
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Story format download
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

/**
 * Construct a download URL for a story format version.
 * Currently only SugarCube has clean per-version release URLs on GitHub.
 *
 * SugarCube release asset naming (verified from GitHub API):
 *   sugarcube-{VERSION}-for-twine-2.1-local.zip   ← Twine 2 story-format bundle (what tweego needs)
 *   sugarcube-{VERSION}-for-twine-1.4.zip         ← Twine 1.4 format
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
 * Layout after success:
 *   `<globalStorage>/storyformats/<format-id>@<version>/<format-id>/format.js`
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
            `Knot: Unknown format "${formatName}". Supported: SugarCube, Harlowe, Chapbook, Snowman`,
        );
        return undefined;
    }

    const url = formatDownloadUrl(formatName, version);
    if (!url) {
        vscode.window.showInformationMessage(
            `Knot: Auto-download is not available for ${formatName} v${version}. `
                + `Please download it manually and use "Browse for folder" to install.`,
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
            `Knot: ${formatName} v${version} is already cached at ${versionedDir}`,
        );
        return versionedDir;
    }

    return vscode.window.withProgress(
        {
            location: vscode.ProgressLocation.Notification,
            title: `Knot: Downloading ${formatName} v${version}...`,
            cancellable: true,
        },
        async (progress, _token) => {
            try {
                progress.report({ message: 'Downloading...' });

                fs.mkdirSync(versionedDir, { recursive: true });
                const zipPath = path.join(versionedDir, `${formatId}.zip`);

                const response = await fetch(url);
                if (!response.ok) {
                    throw new Error(
                        `HTTP ${response.status} ${response.statusText} — check that version ${version} exists at ${url}`,
                    );
                }
                const buffer = Buffer.from(await response.arrayBuffer());
                fs.writeFileSync(zipPath, buffer);

                progress.report({ message: 'Extracting...' });

                // Extract to a temp dir first, then move the format-id
                // subdirectory into place. The SugarCube zip contains a
                // top-level `sugarcube-2/` directory — we want that to
                // end up at `formatDest`.
                const extractDir = path.join(versionedDir, '_extract_tmp');
                fs.mkdirSync(extractDir, { recursive: true });

                extractZip(zipPath, extractDir, process.platform);

                // Find the format-id directory in the extracted contents.
                let sourceFormatDir: string | null = null;
                if (fs.existsSync(path.join(extractDir, formatId, 'format.js'))) {
                    sourceFormatDir = path.join(extractDir, formatId);
                } else {
                    // Search one level deep
                    for (const entry of fs.readdirSync(extractDir, { withFileTypes: true })) {
                        if (entry.isDirectory()) {
                            const candidate = path.join(extractDir, entry.name, formatId);
                            if (fs.existsSync(path.join(candidate, 'format.js'))) {
                                sourceFormatDir = candidate;
                                break;
                            }
                            // Maybe the format-id IS the top-level dir
                            if (
                                entry.name === formatId
                                && fs.existsSync(path.join(extractDir, entry.name, 'format.js'))
                            ) {
                                sourceFormatDir = path.join(extractDir, entry.name);
                                break;
                            }
                        }
                    }
                }

                if (!sourceFormatDir) {
                    // Fall back: just move the whole extract dir contents
                    fs.mkdirSync(formatDest, { recursive: true });
                    for (const entry of fs.readdirSync(extractDir, { withFileTypes: true })) {
                        fs.renameSync(
                            path.join(extractDir, entry.name),
                            path.join(formatDest, entry.name),
                        );
                    }
                } else {
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
                    `Knot: ${formatName} v${version} downloaded successfully. Cached at: ${versionedDir}`,
                );
                return versionedDir;
            } catch (e) {
                vscode.window.showErrorMessage(
                    `Knot: Failed to download ${formatName} v${version}: ${formatError(e)}`,
                );
                return undefined;
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Zip extraction utilities
// ---------------------------------------------------------------------------

/**
 * Extract a zip file using platform-native tooling.
 *
 * - Windows: PowerShell's `Expand-Archive`
 * - Unix: Python 3 first (cross-platform, no external deps), then `unzip`
 *   as a fallback.
 *
 * Throws on failure. The caller is responsible for cleanup on error.
 */
function extractZip(zipPath: string, destDir: string, platform: NodeJS.Platform): void {
    if (platform === 'win32') {
        execSync(
            `powershell -NoProfile -Command "Expand-Archive -Force -Path '${zipPath}' -DestinationPath '${destDir}'"`,
            { stdio: 'pipe' },
        );
        return;
    }
    // Unix: try Python first (cross-platform, no external deps needed)
    try {
        execSync(
            `python3 -c "import zipfile; zipfile.ZipFile('${zipPath}').extractall('${destDir}')"`,
            { stdio: 'pipe' },
        );
    } catch {
        // Fall back to unzip
        execSync(`unzip -o "${zipPath}" -d "${destDir}"`, { stdio: 'pipe' });
    }
}

/**
 * Recursively search a directory for a file by name. Returns the absolute
 * path of the first match, or null if not found.
 */
function findFileRecursive(dir: string, targetName: string): string | null {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            const found = findFileRecursive(fullPath, targetName);
            if (found) { return found; }
        } else if (entry.name === targetName) {
            return fullPath;
        }
    }
    return null;
}
