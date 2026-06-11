//! LSP notification handlers for Knot.
//!
//! Registers all custom notification handlers on the language client:
//! - `knot/indexProgress` — indexing progress and ready state
//! - `knot/buildOutput` — build output relay
//! - `knot/noTweeFiles` — no project detected prompt
//! - `knot/formatDetected` — language ID switching for TextMate grammars
//! - `knot/refreshSemanticTokens` — cross-file semantic token refresh

import * as vscode from 'vscode';
import {
    KnotLanguageClient,
    KnotIndexProgress,
    KnotBuildOutput,
    KnotFormatDetectedParams,
    KnotRefreshSemanticTokensParams,
    KnotProfileResponse,
} from './types';

// ---------------------------------------------------------------------------
// Detected format state (used by onDidOpenTextDocument handler)
// ---------------------------------------------------------------------------

/** The format-specific language ID detected by the server (e.g., 'twee-sugarcube').
 *  Null means the format hasn't been detected yet. */
let detectedLanguageId: string | null = null;

/** Map format names from the server to VS Code language IDs. */
const formatToLanguageId: Record<string, string> = {
    'Core': 'twee',
    'SugarCube': 'twee-sugarcube',
    'Harlowe': 'twee-harlowe',
    'Chapbook': 'twee-chapbook',
    'Snowman': 'twee-snowman',
};

// ---------------------------------------------------------------------------
// Dependencies injected from extension.ts
// ---------------------------------------------------------------------------

export interface NotificationDeps {
    statusBarItem: vscode.StatusBarItem;
    storyMapPanel: { refreshGraph(): void; focusNode(name: string): void } | null;
    variableFlowProvider: { refresh(): void } | null;
    profileViewProvider: { refresh(): void } | null;
    debugViewProvider: { updateForPassage(name: string): void } | null;
    buildOutputChannel: vscode.OutputChannel;
    /** Callback to refresh decorations for open editors. */
    refreshDecorations: () => void;
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/**
 * Register all custom LSP notification handlers on the language client.
 *
 * Must be called after `client.start()` succeeds.
 *
 * Returns a {@link vscode.Disposable} that cleans up the registered handlers.
 */
export function registerNotifications(
    client: KnotLanguageClient,
    deps: NotificationDeps,
): vscode.Disposable {
    const disposables: vscode.Disposable[] = [];
    // ── knot/indexProgress ────────────────────────────────────────────
    client.onNotification(
        { method: 'knot/indexProgress' },
        (params: KnotIndexProgress) => {
            if (deps.statusBarItem) {
                if (params.parsed_files < params.total_files) {
                    deps.statusBarItem.text = `$(sync~spin) Knot: Indexing ${params.parsed_files}/${params.total_files}`;
                } else {
                    deps.statusBarItem.text = '$(check) Knot: Ready';
                    deps.statusBarItem.command = 'knot.openStoryMap';
                    deps.statusBarItem.tooltip = 'Knot: Click to open Story Map';
                    deps.storyMapPanel?.refreshGraph();
                    deps.variableFlowProvider?.refresh();
                    deps.profileViewProvider?.refresh();

                    // Fetch profile data for status bar enrichment
                    (async () => {
                        try {
                            const wsFolders = vscode.workspace.workspaceFolders;
                            if (wsFolders && wsFolders.length > 0) {
                                const profile = await client.sendRequest<KnotProfileResponse>('knot/profile', {
                                    workspace_uri: wsFolders[0].uri.toString(),
                                });
                                if (deps.statusBarItem && profile) {
                                    const name = profile.story_name || 'Untitled';
                                    const fmt = profile.format || 'Unknown';
                                    const passages = profile.passage_count || 0;
                                    deps.statusBarItem.text = `$(graph) ${name} | ${passages} passages`;
                                    deps.statusBarItem.tooltip = `Knot IDE — ${name} | ${fmt}${profile.format_version ? ' v' + profile.format_version : ''} | ${passages} passages, ${profile.total_word_count || 0} words`;
                                }
                            }
                        } catch {
                            // If profile fetch fails, use default status
                            if (deps.statusBarItem) {
                                deps.statusBarItem.text = '$(graph) Knot';
                            }
                        }
                    })();
                }
            }
        }
    );

    // ── knot/buildOutput ──────────────────────────────────────────────
    client.onNotification(
        { method: 'knot/buildOutput' },
        (params: KnotBuildOutput) => {
            if (deps.buildOutputChannel) {
                deps.buildOutputChannel.appendLine(params.line);
                if (params.is_error) {
                    deps.buildOutputChannel.show(true);
                }
            }
        }
    );

    // ── knot/noTweeFiles ──────────────────────────────────────────────
    client.onNotification(
        { method: 'knot/noTweeFiles' },
        (_params: { workspace_uri: string }) => {
            if (deps.statusBarItem) {
                deps.statusBarItem.text = '$(plus) Knot: No project found';
                deps.statusBarItem.tooltip = 'Knot: No .tw/.twee files found. Click to initialize a project.';
                deps.statusBarItem.command = 'knot.initProject';
            }
            // Prompt the user to initialize a project
            vscode.window.showInformationMessage(
                'Knot: No Twine project files (.tw/.twee) found in this workspace. Would you like to initialize one?',
                'Initialize Project',
                'Dismiss'
            ).then(async (choice) => {
                if (choice === 'Initialize Project') {
                    await vscode.commands.executeCommand('knot.initProject');
                }
            });
        }
    );

    // ── knot/formatDetected ───────────────────────────────────────────
    client.onNotification(
        { method: 'knot/formatDetected' },
        (params: KnotFormatDetectedParams) => {
            const languageId = formatToLanguageId[params.format];
            if (!languageId) {
                console.warn(`[Knot] Unknown format: ${params.format}, keeping base 'twee' language`);
                return;
            }

            // Store the detected language ID for the onDidOpenTextDocument
            // handler, so that files opened AFTER this notification also get
            // the correct language ID automatically.
            detectedLanguageId = languageId;

            // Collect all switch promises so we can wait for them to settle
            const switchPromises: Thenable<void>[] = [];
            for (const docUri of params.document_uris) {
                try {
                    const uri = vscode.Uri.parse(docUri);
                    // Find the document in the visible editors or workspace
                    const doc = vscode.workspace.textDocuments.find(d => d.uri.toString() === uri.toString());
                    if (doc && doc.languageId !== languageId) {
                        const p = vscode.languages.setTextDocumentLanguage(doc, languageId);
                        switchPromises.push(p.then(() => {}));
                        p.then(
                            () => {
                                console.log(`[Knot] Switched ${docUri} to language: ${languageId}`);
                            },
                            (err: unknown) => {
                                console.warn(`[Knot] Failed to switch language for ${docUri}: ${err}`);
                            }
                        );
                    }
                } catch (e) {
                    console.warn(`[Knot] Error processing format notification for ${docUri}: ${e}`);
                }
            }

            // Wait for ALL switches to settle (not just resolve — allSettled
            // handles failures). Then signal completion to the server.
            Promise.allSettled(switchPromises).then(async () => {
                // Signal completion to the server — it will clear the
                // format_switch_in_progress flag and send ONE unified
                // workspace/semanticTokens/refresh
                try {
                    await client.sendRequest('knot/formatSwitchComplete', {
                        workspace_uri: params.workspace_uri,
                        switched_count: switchPromises.length,
                    });
                } catch (e) {
                    console.warn('[knot] formatSwitchComplete failed:', e);
                }
                deps.refreshDecorations();
            });
        }
    );

    // ── knot/refreshSemanticTokens ────────────────────────────────────
    client.onNotification(
        { method: 'knot/refreshSemanticTokens' },
        (params: KnotRefreshSemanticTokensParams) => {
            const reason = params.reason || 'unknown';
            console.log(`[Knot] Refreshing semantic tokens for ${params.document_uris.length} document(s) (reason: ${reason})`);

            // The server's workspace/semanticTokens/refresh already
            // triggers VS Code's built-in token refresh. We only need
            // to refresh decorations (broken link highlights, gutter
            // badges) since those are client-side overlays.
            deps.refreshDecorations();
        }
    );

    // ── Auto-switch language ID for newly opened .tw/.twee files ──────
    //
    // When a .tw/.twee file is opened after the format has been detected,
    // VS Code assigns it the default 'twee' language ID (from the file
    // extension association in package.json). This causes a visual
    // inconsistency: the file gets the 'twee' semanticTokenScopes mapping
    // instead of the format-specific one (e.g., 'twee-sugarcube'), which
    // produces different colors for the same semantic tokens.
    //
    // This handler automatically switches the language ID to the detected
    // format's language ID, ensuring consistent highlighting across all
    // open documents.
    disposables.push(
        vscode.workspace.onDidOpenTextDocument((doc) => {
            // Skip if the format hasn't been detected yet (before indexing
            // completes, or if no StoryData was found). In those cases,
            // 'twee' is the correct language ID.
            if (!detectedLanguageId) {
                return;
            }

            // Only switch .tw/.twee files. Other file types are unaffected.
            const path = doc.uri.path.toLowerCase();
            if (!path.endsWith('.tw') && !path.endsWith('.twee')) {
                return;
            }

            // Skip if the language ID is already correct (prevents
            // infinite loop — setTextDocumentLanguage fires another
            // onDidOpenTextDocument with the new language ID).
            if (doc.languageId === detectedLanguageId) {
                return;
            }

            // Skip non-file URIs (untitled, git, etc.)
            if (doc.uri.scheme !== 'file') {
                return;
            }

            vscode.languages.setTextDocumentLanguage(doc, detectedLanguageId).then(
                () => {
                    console.log(`[Knot] Auto-switched ${doc.uri} to language: ${detectedLanguageId}`);
                },
                (err: unknown) => {
                    console.warn(`[Knot] Failed to auto-switch language for ${doc.uri}: ${err}`);
                }
            );
        })
    );

    return vscode.Disposable.from(...disposables);
}
