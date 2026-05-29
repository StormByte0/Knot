//! Centralized navigation coordinator for the Knot extension.
//!
//! All passage navigation — from StoryMap node clicks, Passage Diagnostics
//! links, Variable Tracking passage links, and Play Mode — flows through
//! `navigateToPassage()`.  This guarantees:
//!
//! 1. **Consistent cross-view synchronization** — the StoryMap focuses the
//!    node, the DebugView updates for the passage, and the editor scrolls
//!    to the correct line, all from a single code path.
//!
//! 2. **Smart ViewColumn placement** — when the StoryMap is open in a split,
//!    passage files open in the non-graph column rather than creating extra
//!    splits.  Previously only the StoryMap click handler had this logic;
//!    now every navigation path benefits.
//!
//! 3. **Uniform line number support** — every caller can optionally specify
//!    a target line within the passage.

import * as vscode from 'vscode';

// ---------------------------------------------------------------------------
// Re-exports: shared state set during activation
// ---------------------------------------------------------------------------

/** References to the active panel managers — set once during activation. */
let _storyMapPanel: import('./storyMapProvider').StoryMapPanelManager | null = null;
let _debugViewProvider: import('./debugViewProvider').DebugViewProvider | null = null;

/** Set the StoryMap panel manager reference. Called once during activation. */
export function setStoryMapPanel(panel: import('./storyMapProvider').StoryMapPanelManager | null): void {
    _storyMapPanel = panel;
}

/** Set the DebugView provider reference. Called once during activation. */
export function setDebugViewProvider(provider: import('./debugViewProvider').DebugViewProvider | null): void {
    _debugViewProvider = provider;
}

// ---------------------------------------------------------------------------
// ViewColumn logic
// ---------------------------------------------------------------------------

/**
 * Find the best ViewColumn for opening a passage.
 *
 * Logic:
 * - If the graph panel has no viewColumn (sidebar or detached window),
 *   open in the default active editor (no split).
 * - If the graph is in a tab in the same window, find an existing
 *   non-graph column to reuse.
 * - If no non-graph editors exist and the graph is in column 2+,
 *   open in column 1 (reusing the empty slot rather than creating a
 *   third column via ViewColumn.Beside).
 * - If the graph is in column 1 and no other editors exist, create a
 *   column beside it (ViewColumn.Beside) — this is the expected
 *   layout: editor left, graph right.
 *
 * This prevents creating a new split for every passage click.
 */
export function findTargetViewColumn(graphColumn: vscode.ViewColumn | undefined): vscode.ViewColumn | undefined {
    // Sidebar or detached window → use default active editor
    if (!graphColumn) {
        return undefined;
    }

    // Check if there are text editors in columns other than the graph's
    const nonGraphEditors = vscode.window.visibleTextEditors.filter(
        e => e.viewColumn !== undefined && e.viewColumn !== graphColumn
    );

    if (nonGraphEditors.length > 0) {
        // Reuse the first available non-graph column
        return nonGraphEditors[0].viewColumn;
    }

    // No other editors exist. If the graph is in column 2+, put the
    // passage in column 1 (the graph already claimed column 2, so
    // column 1 is empty and available). This avoids creating a third
    // column via ViewColumn.Beside which would waste screen space.
    if (graphColumn > vscode.ViewColumn.One) {
        return vscode.ViewColumn.One;
    }

    // Graph is in column 1 — create a column beside it so the passage
    // opens in column 2. This is the standard layout: editor right of
    // graph, or graph left of editor.
    return vscode.ViewColumn.Beside;
}

// ---------------------------------------------------------------------------
// ViewColumn guard — prevent VSCode from opening files in the StoryMap's column
// ---------------------------------------------------------------------------

/**
 * Register an `onDidChangeActiveTextEditor` listener that redirects any
 * text editor that lands in the StoryMap's column to a different column.
 *
 * When the user drags or clicks the background of the StoryMap webview,
 * VSCode makes it the "active editor." Subsequent file-explorer clicks
 * then open files in the StoryMap's column, hiding it behind a new tab.
 *
 * This guard detects that situation and re-opens the file in a
 * non-graph column, then re-reveals the StoryMap so it stays visible.
 */
export function registerViewColumnGuard(context: vscode.Disposable[]): void {
    context.push(
        vscode.window.onDidChangeActiveTextEditor(async (editor) => {
            if (!editor || !_storyMapPanel) { return; }

            const graphColumn = _storyMapPanel.viewColumn;
            if (graphColumn === undefined) { return; }

            // Only act if the new text editor landed in the StoryMap's column
            if (editor.viewColumn !== graphColumn) { return; }

            // Find a better column for this file
            const targetColumn = findTargetViewColumn(graphColumn);
            if (!targetColumn || targetColumn === graphColumn) { return; }

            // Re-show the document in the correct column
            try {
                await vscode.window.showTextDocument(editor.document, {
                    preview: true,
                    viewColumn: targetColumn,
                    selection: editor.selection,
                });

                // Re-reveal the StoryMap so it stays visible
                _storyMapPanel.reveal();
            } catch {
                // If the editor was already disposed, just ignore
            }
        })
    );
}

// ---------------------------------------------------------------------------
// Central navigation function
// ---------------------------------------------------------------------------

/**
 * Navigate to a passage by name, synchronizing all views.
 *
 * This is the **single entry point** for all passage navigation in the
 * extension. It:
 *
 * 1. Focuses the StoryMap graph node (if the panel is open).
 * 2. Updates the Passage Diagnostics sidebar for the passage.
 * 3. Opens the source file at the correct location with smart
 *    ViewColumn placement.
 *
 * @param passageName  The passage to navigate to.
 * @param targetLine   Optional line number within the passage to select.
 *                      If omitted, the passage header line is selected.
 * @returns `true` if the passage was found and opened; `false` otherwise.
 */
export async function navigateToPassage(passageName: string, targetLine?: number): Promise<boolean> {
    // ── 1. Cross-view synchronization ────────────────────────────

    // Focus the StoryMap node
    if (_storyMapPanel) {
        _storyMapPanel.focusNode(passageName);
    }

    // Update Passage Diagnostics for the clicked passage
    if (_debugViewProvider) {
        _debugViewProvider.updateForPassage(passageName);
    }

    // ── 2. Determine smart ViewColumn ────────────────────────────

    const graphColumn = _storyMapPanel?.viewColumn;
    const viewColumn = findTargetViewColumn(graphColumn);

    // ── 3. Open the document ─────────────────────────────────────

    // First, search open documents
    for (const doc of vscode.workspace.textDocuments) {
        if (!isTweeLanguage(doc.languageId)) { continue; }
        for (let i = 0; i < doc.lineCount; i++) {
            const line = doc.lineAt(i).text;
            if (line.startsWith('::')) {
                const name = extractPassageName(line);
                if (name === passageName) {
                    const selectionLine = (targetLine !== undefined && targetLine > 0)
                        ? targetLine
                        : i;
                    await vscode.window.showTextDocument(doc, {
                        preview: true,
                        viewColumn,
                        selection: new vscode.Range(selectionLine, 0, selectionLine, doc.lineAt(selectionLine).text.length),
                    });
                    return true;
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
                    const name = extractPassageName(line);
                    if (name === passageName) {
                        const selectionLine = (targetLine !== undefined && targetLine > 0)
                            ? targetLine
                            : i;
                        await vscode.window.showTextDocument(doc, {
                            preview: true,
                            viewColumn,
                            selection: new vscode.Range(selectionLine, 0, selectionLine, doc.lineAt(selectionLine).text.length),
                        });
                        return true;
                    }
                }
            }
        } catch {
            // Skip files that can't be opened
        }
    }

    vscode.window.showWarningMessage(`Knot: Passage '${passageName}' not found in workspace.`);
    return false;
}

// ---------------------------------------------------------------------------
// Shared utilities (duplicated from extension.ts to avoid circular imports)
// ---------------------------------------------------------------------------

const TWEE_LANGUAGE_IDS = ['twee', 'twee-sugarcube', 'twee-harlowe', 'twee-chapbook', 'twee-snowman'];

function isTweeLanguage(languageId: string): boolean {
    return TWEE_LANGUAGE_IDS.includes(languageId);
}

/**
 * Extract the passage name from a `::` header line.
 *
 * Strips `::` prefix, `[tag]` blocks, and `{JSON}` metadata blocks,
 * matching the Rust-side `extract_passage_name()`.
 */
function extractPassageName(headerLine: string): string {
    let name = headerLine.replace(/^::\s*/, '');
    name = stripJsonBlock(name);
    name = stripTagBlock(name);
    return name.trim();
}

function stripJsonBlock(s: string): string {
    const start = s.indexOf('{');
    if (start < 0) { return s; }
    let depth = 0;
    for (let i = start; i < s.length; i++) {
        if (s[i] === '{') { depth++; }
        else if (s[i] === '}') {
            depth--;
            if (depth === 0) {
                const candidate = s.substring(start, i + 1);
                try {
                    JSON.parse(candidate);
                    return s.substring(0, start) + s.substring(i + 1);
                } catch {
                    return s;
                }
            }
        }
    }
    return s;
}

function stripTagBlock(s: string): string {
    const start = s.indexOf('[');
    if (start < 0) { return s; }
    const end = s.indexOf(']', start);
    if (end < 0) { return s; }
    return s.substring(0, start) + s.substring(end + 1);
}
