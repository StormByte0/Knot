//! Virtual document content provider for Knot.
//!
//! Registers a `knot-vdoc` URI scheme that serves the assembled virtual
//! document (translated JavaScript) from the LSP server. VSCode's native
//! JS/TS validation runs on the virtual doc content, and diagnostics are
//! routed back to .tw source positions using the line map.
//!
//! ## URI Scheme
//!
//! - `knot-vdoc://workspace/virtual-doc.js` → virtual doc for the entire workspace
//!
//! ## Auto-refresh
//!
//! The virtual doc auto-refreshes when .tw files change (debounced by 500ms).
//! This ensures the translated JS stays in sync with the source files,
//! especially for custom macro definitions that need to be registered before
//! invocations in other passages can be translated as function calls.
//!
//! ## Diagnostic Routing
//!
//! When VSCode reports JS diagnostics on the virtual doc, the
//! `KnotVirtualDocDiagnostics` listener converts them to diagnostics on the
//! original .tw files using the line map:
//!
//! 1. Virtual doc line number → `KnotVirtualDocLineEntry` (passage_name, file_uri, original_line)
//! 2. passage_name + original_line → position in the .tw file
//! 3. Publish as `vscode.Diagnostic` on the .tw file

import * as vscode from 'vscode';
import { KnotLanguageClient, KnotVirtualDocResponse, KnotJsDiagnosticsParams, KnotJsDiagnostic, KnotJsDiagnosticsResponse } from './types';

// ---------------------------------------------------------------------------
// Virtual Document Content Provider
// ---------------------------------------------------------------------------

/** The URI scheme for virtual documents. */
export const VDOC_SCHEME = 'knot-vdoc';

/** The virtual doc URI — single workspace-wide virtual doc. */
const VDOC_URI = vscode.Uri.parse(`${VDOC_SCHEME}://workspace/virtual-doc.js`);

/** Cached virtual doc data — updated on every refresh. */
let cachedResponse: KnotVirtualDocResponse | null = null;

/** The provider instance — stored so extension.ts can trigger refreshes. */
let providerInstance: KnotVirtualDocProvider | null = null;

/** Debounce timer for auto-refresh. */
let refreshDebounceTimer: ReturnType<typeof setTimeout> | null = null;

/** Reference to the silently-opened virtual doc, kept alive to prevent GC
 *  and keep the document registered with VS Code's language services. */
let vdocDocument: vscode.TextDocument | null = null;

/**
 * Register the virtual document content provider and diagnostic routing.
 *
 * Call this once during extension activation.
 */
export function registerVirtualDocProvider(
    context: vscode.ExtensionContext,
    client: KnotLanguageClient,
): void {
    // Register the content provider for the knot-vdoc:// scheme
    const provider = new KnotVirtualDocProvider(client);
    providerInstance = provider;
    context.subscriptions.push(
        vscode.workspace.registerTextDocumentContentProvider(VDOC_SCHEME, provider),
    );

    // Register the diagnostic routing: JS errors on virtual doc → .tw diagnostics
    const diagnostics = new KnotVirtualDocDiagnostics(client);
    // Push BOTH the subscription AND the diagnostics object itself so that
    // the diagnostic collection is properly disposed on deactivation.
    context.subscriptions.push(
        vscode.languages.onDidChangeDiagnostics((e) => diagnostics.onDiagnosticsChanged(e)),
    );
    context.subscriptions.push(diagnostics);

    // NOTE: The 'knot.openVirtualDoc' command is registered in extension.ts
    // registerCommands(). Do NOT register it here — VS Code throws on
    // duplicate command registration.
}

/**
 * Open the virtual document in an editor tab.
 */
export async function openVirtualDoc(client: KnotLanguageClient): Promise<void> {
    const wsFolders = vscode.workspace.workspaceFolders;
    if (!wsFolders || wsFolders.length === 0) {
        vscode.window.showWarningMessage('Knot: No workspace folder open.');
        return;
    }

    // Refresh the cached data
    await refreshVirtualDoc(client);

    // Create a virtual doc URI and open it
    const doc = await vscode.workspace.openTextDocument(VDOC_URI);
    await vscode.window.showTextDocument(doc, { preview: true, viewColumn: vscode.ViewColumn.Beside });
}

/**
 * Refresh the cached virtual doc data from the server.
 *
 * After refreshing, fires the `onDidChange` event so that any open
 * virtual doc tabs reload their content from the provider.
 */
export async function refreshVirtualDoc(client: KnotLanguageClient): Promise<KnotVirtualDocResponse | null> {
    const wsFolders = vscode.workspace.workspaceFolders;
    if (!wsFolders || wsFolders.length === 0) {
        return null;
    }

    try {
        const response = await client.sendRequest<KnotVirtualDocResponse>('knot/virtualDoc', {
            workspace_uri: wsFolders[0].uri.toString(),
        });
        cachedResponse = response;

        // Fire the onDidChange event so VSCode re-queries
        // provideTextDocumentContent() for the virtual doc tab.
        // This is the key mechanism for updating the virtual doc display.
        if (providerInstance) {
            providerInstance.notifyContentChanged();
        }

        return response;
    } catch (error) {
        console.error('Knot: Failed to fetch virtual doc:', error);
        return null;
    }
}

/**
 * Debounced refresh — retained as a utility but no longer used for
 * automatic refresh. The server now pushes `knot/refreshVirtualDoc`
 * notifications, which the client handles directly via `refreshVirtualDoc()`.
 *
 * This function is still exported for any edge cases where the client
 * needs to manually trigger a debounced refresh (e.g., after reindex).
 */
export function debouncedRefreshVirtualDoc(client: KnotLanguageClient): void {
    if (refreshDebounceTimer) {
        clearTimeout(refreshDebounceTimer);
    }
    refreshDebounceTimer = setTimeout(async () => {
        refreshDebounceTimer = null;
        await refreshVirtualDoc(client);
    }, 500);
}

/**
 * Open the virtual document silently — no editor tab, no visible UI.
 *
 * Calls `openTextDocument()` without `showTextDocument()`, which loads the
 * document into VS Code's document registry and fires `onDidOpenTextDocument`.
 * This registers the document with VS Code's built-in JS/TS language service,
 * which validates it and publishes diagnostics. Our `KnotVirtualDocDiagnostics`
 * listener catches those diagnostics and relays them to the server.
 *
 * The module-level `vdocDocument` reference prevents VS Code from garbage-
 * collecting the document. If the document is already open (visible tab or
 * in-memory), this just refreshes the cache.
 *
 * ## Fallback
 *
 * If VS Code's JS/TS extension does not validate documents that aren't in a
 * visible editor (this varies by VS Code version), call
 * `openVirtualDocTab()` instead — it opens a background tab with
 * `preserveFocus: true`.
 */
export async function openVirtualDocSilently(
    client: KnotLanguageClient,
): Promise<void> {
    // If the document is already open (tab or in-memory), just refresh.
    if (vdocDocument) {
        await refreshVirtualDoc(client);
        return;
    }
    // If a tab is already visible, also just refresh.
    if (isVirtualDocTabOpen()) {
        await refreshVirtualDoc(client);
        return;
    }

    const wsFolders = vscode.workspace.workspaceFolders;
    if (!wsFolders || wsFolders.length === 0) {
        return;
    }

    try {
        await refreshVirtualDoc(client);
        // Open in memory — no tab, no visible UI. The document is registered
        // with VS Code's language services via onDidOpenTextDocument.
        vdocDocument = await vscode.workspace.openTextDocument(VDOC_URI);
    } catch (error) {
        console.error('Knot: Failed to silently open virtual doc:', error);
    }
}

/**
 * Open the virtual document in a visible background editor tab.
 *
 * This is the fallback if `openVirtualDocSilently()` doesn't trigger JS
 * validation (some VS Code versions only validate visible editors). The
 * tab opens with `preserveFocus: true` so the user's active editor isn't
 * disrupted.
 */
export async function openVirtualDocTab(
    client: KnotLanguageClient,
): Promise<void> {
    if (isVirtualDocTabOpen()) {
        await refreshVirtualDoc(client);
        return;
    }

    const wsFolders = vscode.workspace.workspaceFolders;
    if (!wsFolders || wsFolders.length === 0) {
        return;
    }

    try {
        await refreshVirtualDoc(client);
        const doc = await vscode.workspace.openTextDocument(VDOC_URI);
        await vscode.window.showTextDocument(doc, {
            preview: true,
            viewColumn: vscode.ViewColumn.Beside,
            preserveFocus: true,
        });
        vdocDocument = doc;
    } catch (error) {
        console.error('Knot: Failed to open virtual doc tab:', error);
    }
}

/**
 * Check whether the virtual doc is open — either as a visible editor tab
 * or as an in-memory document (silently opened).
 */
export function isVirtualDocOpen(): boolean {
    // Check for a visible tab
    if (vscode.window.visibleTextEditors.some(
        e => e.document.uri.scheme === VDOC_SCHEME
    )) {
        return true;
    }
    // Check for in-memory document (silently opened)
    return vdocDocument !== null;
}

/**
 * Check whether the virtual doc tab is currently visible in any editor.
 */
export function isVirtualDocTabOpen(): boolean {
    return vscode.window.visibleTextEditors.some(
        e => e.document.uri.scheme === VDOC_SCHEME
    );
}

/**
 * Get the cached virtual doc response.
 */
export function getCachedVirtualDoc(): KnotVirtualDocResponse | null {
    return cachedResponse;
}

// ---------------------------------------------------------------------------
// TextDocumentContentProvider implementation
// ---------------------------------------------------------------------------

class KnotVirtualDocProvider implements vscode.TextDocumentContentProvider {
    private _onDidChange = new vscode.EventEmitter<vscode.Uri>();
    private client: KnotLanguageClient;

    constructor(client: KnotLanguageClient) {
        this.client = client;
    }

    get onDidChange(): vscode.Event<vscode.Uri> {
        return this._onDidChange.event;
    }

    async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
        // Use the cached content if available. The cache is kept fresh by:
        //   1. Initial project load (indexProgress completion)
        //   2. File watcher changes (debounced 500ms)
        //   3. Text document changes (debounced 500ms)
        //   4. Explicit refresh via openVirtualDoc()
        // Avoiding a server round-trip here prevents UI lag when VSCode
        // re-queries the virtual doc content (e.g., on tab focus).
        if (cachedResponse) {
            return cachedResponse.content;
        }

        // No cache yet — fetch from server (first load scenario)
        const response = await refreshVirtualDoc(this.client);
        if (response) {
            return response.content;
        }

        return '// No virtual document available\n';
    }

    /**
     * Signal that the virtual doc content has changed.
     * This causes VSCode to re-query provideTextDocumentContent().
     */
    notifyContentChanged(): void {
        this._onDidChange.fire(VDOC_URI);
    }
}

// ---------------------------------------------------------------------------
// Diagnostic Routing: JS errors on virtual doc → .tw diagnostics
// ---------------------------------------------------------------------------

class KnotVirtualDocDiagnostics {
    private client: KnotLanguageClient;
    /** Debounce timer for batching diagnostics before relaying. */
    private debounceTimer: ReturnType<typeof setTimeout> | null = null;
    /** Pending diagnostics to relay. */
    private pendingDiagnostics: KnotJsDiagnostic[] = [];

    constructor(client: KnotLanguageClient) {
        this.client = client;
    }

    dispose(): void {
        if (this.debounceTimer) {
            clearTimeout(this.debounceTimer);
            this.debounceTimer = null;
        }
    }

    /**
     * Called when VSCode's diagnostic collection changes.
     * We check if any diagnostics appeared on our virtual doc and
     * relay them to the server for processing via the two-stage
     * diagnostic relay pipeline.
     *
     * ## Debouncing
     *
     * JS diagnostics can arrive in bursts (e.g., after a full virtual
     * doc refresh). We collect diagnostics for 300ms after the first
     * diagnostic appears, then send a single batch to the server.
     */
    async onDiagnosticsChanged(event: vscode.DiagnosticChangeEvent): Promise<void> {
        // Check if any of the changed URIs are our virtual doc
        const vdocUris = event.uris.filter(uri => uri.scheme === VDOC_SCHEME);
        if (vdocUris.length === 0) {
            return;
        }

        // Collect raw JS diagnostics from VSCode
        for (const vdocUri of vdocUris) {
            const allDiags = vscode.languages.getDiagnostics(vdocUri);

            for (const diag of allDiags) {
                this.pendingDiagnostics.push({
                    start_line: diag.range.start.line,
                    start_character: diag.range.start.character,
                    end_line: diag.range.end.line,
                    end_character: diag.range.end.character,
                    message: diag.message,
                    severity: this.convertSeverity(diag.severity),
                    code: diag.code?.toString(),
                });
            }
        }

        // Debounce: wait 300ms before sending the batch
        if (this.debounceTimer) {
            clearTimeout(this.debounceTimer);
        }

        this.debounceTimer = setTimeout(() => {
            this.debounceTimer = null;
            this.flushDiagnostics();
        }, 300);
    }

    /**
     * Send the collected diagnostics to the server.
     */
    private flushDiagnostics(): void {
        if (this.pendingDiagnostics.length === 0) {
            return;
        }

        const diagnostics = this.pendingDiagnostics.splice(0);
        const params: KnotJsDiagnosticsParams = {
            uri: VDOC_URI.toString(),
            diagnostics,
        };

        try {
            this.client.sendRequest<KnotJsDiagnosticsResponse>('knot/jsDiagnostics', params);
        } catch (error) {
            console.error('Knot: Failed to relay JS diagnostics:', error);
        }
    }

    /**
     * Convert VSCode DiagnosticSeverity to the numeric wire format.
     */
    private convertSeverity(severity: vscode.DiagnosticSeverity): number {
        switch (severity) {
            case vscode.DiagnosticSeverity.Error: return 1;
            case vscode.DiagnosticSeverity.Warning: return 2;
            case vscode.DiagnosticSeverity.Information: return 3;
            case vscode.DiagnosticSeverity.Hint: return 4;
            default: return 2; // Default to Warning
        }
    }
}
