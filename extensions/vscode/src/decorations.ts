//! Editor decorations for Twee files.
//!
//! Provides three decoration types:
//! - **Gutter badge**: Colored circle on passage headers
//! - **Unreachable fade**: Dimmed text for unreachable passages
//! - **Broken link underline**: Wavy red underline on broken links
//!
//! Also handles debounced refresh on document changes and
//! cross-file semantic token invalidation.

import * as vscode from 'vscode';
import { KnotLanguageClient } from './types';
import { isTweeLanguage, extractPassageName } from './utils';

// ---------------------------------------------------------------------------
// Decoration types (owned by this module)
// ---------------------------------------------------------------------------

let passageDecorationType: vscode.TextEditorDecorationType | null = null;
let unreachableDecorationType: vscode.TextEditorDecorationType | null = null;
let linkDecorationType: vscode.TextEditorDecorationType | null = null;
let decorationDebounceTimer: ReturnType<typeof setTimeout> | null = null;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Register editor decorations for Twee files. */
export function registerDecorations(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient,
): void {
    // Gutter badge for passage headers — small colored circle
    passageDecorationType = vscode.window.createTextEditorDecorationType({
        gutterIconPath: context.asAbsolutePath('media/passage-icon.svg'),
        gutterIconSize: 'auto',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
        overviewRulerColor: 'rgba(79, 195, 247, 0.5)', // Light blue
    });
    context.subscriptions.push(passageDecorationType);

    // Faded text for unreachable passages — amber tint to signal warning
    unreachableDecorationType = vscode.window.createTextEditorDecorationType({
        opacity: '0.5',
        overviewRulerLane: vscode.OverviewRulerLane.Left,
        overviewRulerColor: 'rgba(230, 81, 0, 0.5)', // Amber/warning
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
        if (editor && isTweeLanguage(editor.document.languageId)) {
            updateDecorations(editor, client);
        }
    }, null, context.subscriptions);

    vscode.workspace.onDidChangeTextDocument((event) => {
        const editor = vscode.window.activeTextEditor;
        if (editor && editor.document === event.document) {
            // Debounce decoration updates to avoid fetching the full graph
            // on every keystroke. The graph request is expensive for large
            // workspaces, so we wait 300ms after the last edit before refreshing.
            if (decorationDebounceTimer) {
                clearTimeout(decorationDebounceTimer);
            }
            decorationDebounceTimer = setTimeout(() => {
                decorationDebounceTimer = null;
                updateDecorations(editor, client);
            }, 300);
        }

        // Note: virtual doc refresh is now handled by the server-push
        // knot/refreshVirtualDoc notification. The server sends this
        // after parse() completes following did_change, so we no longer
        // need client-side debounced refresh here.
    }, null, context.subscriptions);

    // Initial update
    if (vscode.window.activeTextEditor && isTweeLanguage(vscode.window.activeTextEditor.document.languageId)) {
        updateDecorations(vscode.window.activeTextEditor, client);
    }
}

/** Refresh decorations for all currently visible twee editors. */
export function refreshDecorationsForOpenEditors(client: KnotLanguageClient): void {
    for (const editor of vscode.window.visibleTextEditors) {
        if (isTweeLanguage(editor.document.languageId)) {
            updateDecorations(editor, client);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

/** Update decorations for the given editor based on workspace analysis. */
async function updateDecorations(editor: vscode.TextEditor, client: KnotLanguageClient): Promise<void> {
    if (!client.isRunning()) { return; }
    if (!isTweeLanguage(editor.document.languageId)) { return; }

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
                        const name = extractPassageName(line);
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
