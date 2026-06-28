//! VS Code extension entry point for the Knot language server.
//!
//! This is the orchestration layer — it owns shared state (language client,
//! panel providers) and delegates to focused sub-modules:
//!
//! - `binaryResolution` — Server binary discovery and platform mapping
//! - `notifications`     — Custom LSP notification handlers
//! - `commands`          — VS Code command registration + Tweego bootstrap
//! - `languageStatus`    — Native Language Status API indicator
//! - `decorations`       — Editor decorations (gutter badges, fades, links)
//! - `taskProvider`      — Build/watch task integration
//! - `crashRecovery`     — Automatic restart and failure handling

import * as vscode from 'vscode';
import * as path from 'path';
import * as vscodeLanguageClient from 'vscode-languageclient';
import { StoryMapPanelManager } from './storyMapProvider';
import { DebugViewProvider } from './debugViewProvider';
import { ProfileViewProvider } from './profileViewProvider';
import { VariableFlowProvider } from './variableFlowProvider';
import * as navigation from './navigation';
import { isTweeLanguage, extractPassageName, setGlobalStoragePath, getResolvedTweegoPath } from './utils';
import { KnotLanguageClient } from './types';
import { getServerPath } from './binaryResolution';
import { registerNotifications, NotificationDeps } from './notifications';
import { registerCommands, CommandDeps } from './commands';
import { registerLanguageStatus } from './languageStatus';
import { registerDecorations, refreshDecorationsForOpenEditors } from './decorations';
import { registerTaskProvider } from './taskProvider';
import { handleServerFailure, resetCrashCount } from './crashRecovery';
import { createStatusBarItems } from './statusBarItems';

// `LanguageClient` is exported at runtime from the package's node entry
// (lib/node/main.js), but the `typings` field in its package.json points
// to lib/common/api.d.ts, which doesn't re-export it. We use a typed
// accessor on the namespace import so both TypeScript and the runtime
// are satisfied without falling back to `require()`.
type LanguageClientCtor = new (
    id: string,
    name: string,
    serverOptions: object,
    clientOptions: object,
) => unknown;
const LanguageClient = (
    vscodeLanguageClient as unknown as {
        LanguageClient: LanguageClientCtor;
    }
).LanguageClient;

// ---------------------------------------------------------------------------
// Module-level state (owned by this file, passed to sub-modules)
// ---------------------------------------------------------------------------

let client: KnotLanguageClient | null = null;
let statusBarItem: vscode.StatusBarItem | null = null;
let storyMapPanel: StoryMapPanelManager | null = null;
let debugViewProvider: DebugViewProvider | null = null;
let profileViewProvider: ProfileViewProvider | null = null;
let variableFlowProvider: VariableFlowProvider | null = null;
let buildOutputChannel: vscode.OutputChannel | null = null;
let profileRefreshDebounceTimer: ReturnType<typeof setTimeout> | null = null;

// ---------------------------------------------------------------------------
// Extension activation
// ---------------------------------------------------------------------------

export async function activate(context: vscode.ExtensionContext) {
    // The Rust language server is always used — there is no legacy
    // TextMate-only fallback. The old `knot.experimental.rustServer`
    // setting has been removed; the server is the server.

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

    // Set the global storage path early — this is used by utils.ts to locate
    // the managed tweego binary and storyformats cache. Without this call,
    // getManagedTweegoPath() always returns undefined, causing the extension
    // to re-prompt for tweego download even after it's already been downloaded.
    setGlobalStoragePath(context.globalStorageUri.fsPath);

    // Populate the read-only managed path settings so users can see where
    // the extension stores things, visible in the Settings UI (cog icon).
    // These are updated on every activation to stay current.
    const storageRoot = context.globalStorageUri.fsPath;
    const config = vscode.workspace.getConfiguration('knot');
    const tweegoBinaryName = process.platform === 'win32' ? 'tweego.exe' : 'tweego';
    await config.update('managed.storagePath', storageRoot, vscode.ConfigurationTarget.Global);
    await config.update('managed.tweegoPath', path.join(storageRoot, 'tweego', tweegoBinaryName), vscode.ConfigurationTarget.Global);
    await config.update('managed.storyformatsPath', path.join(storageRoot, 'storyformats'), vscode.ConfigurationTarget.Global);

    // ── Settings migration: tweegoPath / storyformats.path → build.* ──
    // The pre-v2 settings `knot.tweegoPath` and `knot.storyformats.path`
    // were renamed to `knot.build.tweegoPath` and `knot.build.storyformatsPath`
    // for namespace consistency. Copy any lingering values forward, then
    // clear the old keys so they disappear from the Settings UI. Idempotent
    // — safe to run on every activation.
    const legacyTweego = config.get<string>('tweegoPath', '');
    const legacySfPath = config.get<string>('storyformats.path', '');
    const newTweego = config.get<string>('build.tweegoPath', '');
    const newSfPath = config.get<string>('build.storyformatsPath', '');
    if (legacyTweego && !newTweego) {
        await config.update('build.tweegoPath', legacyTweego, vscode.ConfigurationTarget.Global);
    }
    if (legacySfPath && !newSfPath) {
        await config.update('build.storyformatsPath', legacySfPath, vscode.ConfigurationTarget.Global);
    }
    // Always clear the legacy keys — `undefined` removes them from settings.json.
    await config.update('tweegoPath', undefined, vscode.ConfigurationTarget.Global);
    await config.update('storyformats.path', undefined, vscode.ConfigurationTarget.Global);

    // Compute and publish the resolved Tweego path (read-only Status & Paths).
    // Re-computed on every config change so it stays in sync with the user's
    // `build.tweegoPath` setting and the managed download state.
    const updateResolvedTweegoPath = () => {
        const resolved = getResolvedTweegoPath();
        // Only write at Global scope so it shows in the Settings UI regardless
        // of workspace. Ignore the "no change" case silently.
        vscode.workspace.getConfiguration('knot').update(
            'resolved.tweegoPath', resolved, vscode.ConfigurationTarget.Global,
        );
    };
    updateResolvedTweegoPath();
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration(e => {
            if (e.affectsConfiguration('knot.build.tweegoPath')) {
                updateResolvedTweegoPath();
            }
        }),
    );

    // ── Warn about deprecated project-local storyformats/ ──────────────
    // The workspace should be purely game files now — story formats live
    // in the extension-managed folder. If a `<workspace>/storyformats/`
    // folder exists, warn the user once per session that it's no longer
    // used and offer to open the managed folder instead.
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (workspaceFolders && workspaceFolders.length > 0) {
        const localStoryformats = vscode.Uri.joinPath(
            workspaceFolders[0].uri, 'storyformats'
        );
        vscode.workspace.fs.stat(localStoryformats).then((stat) => {
            if (stat.type === vscode.FileType.Directory) {
                vscode.window.showWarningMessage(
                    'Knot: A "storyformats" folder was found in this workspace. ' +
                    'Project-local story formats are no longer supported — the workspace should contain only game files. ' +
                    'Story formats now live in the extension-managed folder. ' +
                    'Use "Knot: Open Story Formats Folder" to manage them.',
                    'Open Managed Folder',
                    'Dismiss'
                ).then((choice) => {
                    if (choice === 'Open Managed Folder') {
                        vscode.commands.executeCommand('knot.openTweegoFolder');
                    }
                });
            }
        }, () => {
            // No storyformats/ folder — expected, nothing to do.
        });
    }

    // Create the build output channel early — shared between the status bar
    // (for the watch toggle's logging) and the build notification handler.
    buildOutputChannel = vscode.window.createOutputChannel('Knot Build');
    context.subscriptions.push(buildOutputChannel);

    // Create the permanent left-side status bar items (Story Map, Build, Watch, Play, Settings)
    // These appear after indexing completes; during indexing, the statusBarItem
    // above shows progress instead.
    createStatusBarItems(
        context,
        buildOutputChannel,
        () => client,
    );

    // ── Language client setup ──────────────────────────────────────────

    const serverOptions = {
        command: serverPath,
        args: ['--stdio'],
    };

    const clientOptions = {
        documentSelector: [
            { scheme: 'file', language: 'twee' },
            { scheme: 'file', language: 'twee-sugarcube' },
            { scheme: 'file', language: 'twee-harlowe' },
            { scheme: 'file', language: 'twee-chapbook' },
            { scheme: 'file', language: 'twee-snowman' },
        ],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher(
                '**/*.{tw,twee}'
            ),
        },
        initializationOptions: {
            // Pass the extension's global storage path so the server can
            // locate the extension-managed toolchain (tweego binary + versioned
            // storyformat cache). This is the root of the "never worry about
            // storyformats" architecture: the extension downloads tweego and
            // formats into globalStorage, and the server uses them at build time.
            globalStoragePath: context.globalStorageUri.fsPath,

            // Pass indexing settings from VS Code configuration. These are
            // merged with patterns from .vscode/knot.json on the server side.
            indexingExclude: vscode.workspace
                .getConfiguration('knot.indexing')
                .get<string[]>('exclude', []),
            indexingMaxFiles: vscode.workspace
                .getConfiguration('knot.indexing')
                .get<number>('maxFiles', 1000),
        },
    };

    client = new LanguageClient(
        'knot',
        'Knot Language Server',
        serverOptions,
        clientOptions
    ) as unknown as KnotLanguageClient;

    // ── Panel providers ────────────────────────────────────────────────

    storyMapPanel = new StoryMapPanelManager(context.extensionUri, context);
    context.subscriptions.push(storyMapPanel);

    debugViewProvider = new DebugViewProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            DebugViewProvider.viewType,
            debugViewProvider,
        )
    );

    profileViewProvider = new ProfileViewProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            ProfileViewProvider.viewType,
            profileViewProvider,
        )
    );

    variableFlowProvider = new VariableFlowProvider(context.extensionUri);
    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(
            VariableFlowProvider.viewType,
            variableFlowProvider,
        )
    );
    context.subscriptions.push({ dispose: () => variableFlowProvider?.dispose() });

    // ── Start client & register notifications ──────────────────────────

    try {
        await client.start();
        resetCrashCount();

        // Wire up the story map to the language client
        if (storyMapPanel) {
            storyMapPanel.setClient(client);
        }

        // Wire up the centralized navigation module with view references
        navigation.setStoryMapPanel(storyMapPanel);
        navigation.setDebugViewProvider(debugViewProvider);

        // Guard against files opening in the StoryMap's column
        navigation.registerViewColumnGuard(context.subscriptions);

        if (debugViewProvider) {
            debugViewProvider.setClient(client);
        }
        if (profileViewProvider) {
            profileViewProvider.setClient(client);
        }
        if (variableFlowProvider) {
            variableFlowProvider.setClient(client);
        }

        // Register custom LSP notification handlers
        const notifDeps: NotificationDeps = {
            statusBarItem: statusBarItem!,
            storyMapPanel,
            variableFlowProvider,
            profileViewProvider,
            debugViewProvider,
            buildOutputChannel: buildOutputChannel!,
            refreshDecorations: () => refreshDecorationsForOpenEditors(client!),
        };
        const notifDisposable = registerNotifications(client, notifDeps);
        context.subscriptions.push(notifDisposable);

        // Signal to the server that all notification handlers are registered.
        // The server waits for this before starting indexing, ensuring that
        // formatDetected and indexProgress notifications won't be dropped.
        try {
            const response = await client.sendRequest('knot/clientReady', {});
            console.log('[knot] clientReady acknowledged:', response);
        } catch (e) {
            console.warn('[knot] clientReady failed (server may be older version):', e);
        }
    } catch (e) {
        handleServerFailure(e, context, serverPath, {
            client: client!,
            statusBarItem: statusBarItem!,
        });
    }

    // ── Register sub-modules ───────────────────────────────────────────

    // Language Status API
    const languageStatusItem = registerLanguageStatus(context, client!);
    context.subscriptions.push(languageStatusItem);

    // Decorations API
    registerDecorations(context, client!);

    // Task Provider
    registerTaskProvider(context, client!);

    // Commands
    const cmdDeps: CommandDeps = {
        getClient: () => client,
        statusBarItem: statusBarItem!,
        storyMapPanel,
        debugViewProvider,
        profileViewProvider,
        variableFlowProvider,
        context,
    };
    registerCommands(cmdDeps);

    // ── File watchers & editor events ──────────────────────────────────

    // Auto-refresh the Story Map when Twee files change
    const watcher = vscode.workspace.createFileSystemWatcher('**/*.{tw,twee}');
    function debouncedProfileRefresh() {
        if (profileRefreshDebounceTimer) {
            clearTimeout(profileRefreshDebounceTimer);
        }
        profileRefreshDebounceTimer = setTimeout(() => {
            profileRefreshDebounceTimer = null;
            profileViewProvider?.refresh();
        }, 500);
    }
    function onTwFileChange() {
        refreshStoryMap();
        variableFlowProvider?.refresh();
        debouncedProfileRefresh();
    }
    watcher.onDidChange(onTwFileChange);
    watcher.onDidCreate(onTwFileChange);
    watcher.onDidDelete(onTwFileChange);
    context.subscriptions.push(watcher);

    // Refresh on active editor change (for live updates)
    vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (editor && isTweeLanguage(editor.document.languageId)) {
            refreshStoryMap();
            updateDebugViewForEditor(editor);
            if (variableFlowProvider) {
                variableFlowProvider.refresh();
            }
            debouncedProfileRefresh();
        }
    });

    // Update debug view on cursor position change
    vscode.window.onDidChangeTextEditorSelection((event) => {
        if (isTweeLanguage(event.textEditor.document.languageId)) {
            updateDebugViewForEditor(event.textEditor);
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Refresh the active Story Map webview panel. */
async function refreshStoryMap() {
    if (storyMapPanel) {
        storyMapPanel.refreshGraph();
    }
}

/** Update the debug view with the passage under the cursor. */
function updateDebugViewForEditor(editor: vscode.TextEditor) {
    if (!debugViewProvider) { return; }

    const document = editor.document;
    if (!isTweeLanguage(document.languageId)) { return; }

    const position = editor.selection.active;
    const text = document.getText();
    const lines = text.split('\n');

    // Walk backwards from the cursor to find the passage header
    let passageName: string | null = null;
    for (let i = position.line; i >= 0; i--) {
        const line = lines[i];
        if (line.startsWith('::')) {
            passageName = extractPassageName(line);
            break;
        }
    }

    if (passageName) {
        debugViewProvider.updateForPassage(passageName);
        // Sync StoryMap focus when cursor moves to a different passage
        if (storyMapPanel) {
            storyMapPanel.focusNode(passageName);
        }
    }
}

// ---------------------------------------------------------------------------
// Deactivation
// ---------------------------------------------------------------------------

export async function deactivate() {
    if (client) {
        await client.stop();
        client = null;
    }
    if (statusBarItem) {
        statusBarItem.dispose();
        statusBarItem = null;
    }
}
