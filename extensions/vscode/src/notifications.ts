//! LSP notification handlers for Knot.
//!
//! Registers all custom notification handlers on the language client:
//! - `knot/indexProgress` — indexing progress and ready state
//! - `knot/buildOutput` — build output relay
//! - `knot/noTweeFiles` — no project detected prompt
//! - `knot/formatDetected` — language ID switching for TextMate grammars
//! - `knot/refreshSemanticTokens` — cross-file semantic token refresh
//! - `knot/refreshVirtualDoc` — virtual document content refresh

import * as vscode from 'vscode';
import {
    KnotLanguageClient,
    KnotIndexProgress,
    KnotBuildOutput,
    KnotFormatDetectedParams,
    KnotRefreshSemanticTokensParams,
    KnotRefreshVirtualDocParams,
    KnotProfileResponse,
} from './types';
import { refreshVirtualDoc, openVirtualDocTab, isVirtualDocTabOpen, getCachedVirtualDoc } from './virtualDocProvider';

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
 */
export function registerNotifications(
    client: KnotLanguageClient,
    deps: NotificationDeps,
): void {
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

                    // Open the virtual doc in a background tab so that
                    // VSCode's JS language service validates it and diagnostics
                    // flow through the relay pipeline. preserveFocus keeps the
                    // user's .tw editor active.
                    openVirtualDocTab(client);

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
            // Map format names from the server to VS Code language IDs.
            // "Core" means no format was detected — keep the base 'twee'
            // language ID so the default TextMate grammar is used.
            const formatToLanguageId: Record<string, string> = {
                'Core': 'twee',
                'SugarCube': 'twee-sugarcube',
                'Harlowe': 'twee-harlowe',
                'Chapbook': 'twee-chapbook',
                'Snowman': 'twee-snowman',
            };

            const languageId = formatToLanguageId[params.format];
            if (!languageId) {
                console.warn(`[Knot] Unknown format: ${params.format}, keeping base 'twee' language`);
                return;
            }

            for (const docUri of params.document_uris) {
                try {
                    const uri = vscode.Uri.parse(docUri);
                    // Find the document in the visible editors or workspace
                    const doc = vscode.workspace.textDocuments.find(d => d.uri.toString() === uri.toString());
                    if (doc && doc.languageId !== languageId) {
                        vscode.languages.setTextDocumentLanguage(doc, languageId).then(
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

            // After switching language IDs, trigger a semantic token refresh
            // so that already-open editors get proper highlighting based on
            // the newly detected format. Without this, editors that were
            // already open before format detection complete may show stale
            // or empty tokens.
            vscode.commands.executeCommand('editor.action.semanticTokens.refresh');
            deps.refreshDecorations();
        }
    );

    // ── knot/refreshSemanticTokens ────────────────────────────────────
    client.onNotification(
        { method: 'knot/refreshSemanticTokens' },
        (params: KnotRefreshSemanticTokensParams) => {
            const reason = params.reason || 'unknown';
            console.log(`[Knot] Refreshing semantic tokens for ${params.document_uris.length} document(s) (reason: ${reason})`);

            // Trigger VS Code's built-in semantic token refresh for all
            // visible editors. VS Code doesn't provide a per-document
            // semantic token refresh API, so we refresh all visible
            // editors. This is the same mechanism that
            // `workspace/semanticTokens/refresh` uses under the hood.
            vscode.commands.executeCommand('editor.action.semanticTokens.refresh');

            // Also refresh decorations for the affected documents, since
            // broken link highlights and gutter badges may have changed.
            deps.refreshDecorations();
        }
    );

    // ── knot/refreshVirtualDoc ────────────────────────────────────────
    client.onNotification(
        { method: 'knot/refreshVirtualDoc' },
        (params: KnotRefreshVirtualDocParams) => {
            const reason = params.reason || 'unknown';
            console.log(`[Knot] Virtual doc refresh requested (reason: ${reason})`);
            // Refresh cached content and update any open tab / in-memory doc
            refreshVirtualDoc(client).then(() => {
                // If the virtual doc tab was closed, re-open it in
                // background to keep JS validation active.
                if (!isVirtualDocTabOpen() && getCachedVirtualDoc()?.content?.length) {
                    openVirtualDocTab(client);
                }
            });
        }
    );
}
