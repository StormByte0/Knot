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
import { KnotLanguageClient, KnotVirtualDocResponse, KnotVirtualDocLineEntry } from './types';
import { extractPassageName } from './utils';

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

/** Custom diagnostic collection for Knot virtual doc → .tw diagnostic routing. */
const VDOC_DIAGNOSTIC_COLLECTION = 'knot-virtual-doc';

class KnotVirtualDocDiagnostics {
    private client: KnotLanguageClient;
    private diagnosticCollection: vscode.DiagnosticCollection;
    /** Map from .tw file URI to its diagnostics. */
    private twDiagnostics: Map<string, vscode.Diagnostic[]> = new Map();

    constructor(client: KnotLanguageClient) {
        this.client = client;
        this.diagnosticCollection = vscode.languages.createDiagnosticCollection(VDOC_DIAGNOSTIC_COLLECTION);
    }

    dispose(): void {
        this.diagnosticCollection.dispose();
    }

    /**
     * Called when VSCode's diagnostic collection changes.
     * We check if any diagnostics appeared on our virtual doc and
     * route them back to the .tw source files.
     *
     * ## Deduplication
     *
     * JS parser errors often cascade — a single missing comma causes
     * "expected `,`", "expected `)`", etc. on the same or nearby lines.
     * All of these map back to the same .tw source line, creating a
     * confusing list in the hover. We keep only the **first** diagnostic
     * per source line per file to show a single relevant error.
     */
    async onDiagnosticsChanged(event: vscode.DiagnosticChangeEvent): Promise<void> {
        // Check if any of the changed URIs are our virtual doc
        const vdocUris = event.uris.filter(uri => uri.scheme === VDOC_SCHEME);
        if (vdocUris.length === 0) {
            return;
        }

        // Get diagnostics from VSCode for the virtual doc
        const lineMap = cachedResponse?.line_map;
        if (!lineMap || lineMap.length === 0) {
            return;
        }

        // Clear previous .tw diagnostics
        this.twDiagnostics.clear();

        // Track which source lines already have a diagnostic (dedup key: "uri:line")
        const seenLines = new Set<string>();

        // Process each virtual doc URI
        for (const vdocUri of vdocUris) {
            const allDiags = vscode.languages.getDiagnostics(vdocUri);

            for (const diag of allDiags) {
                // Convert virtual doc line to .tw source position
                const vdocLine = diag.range.start.line;
                if (vdocLine >= lineMap.length) {
                    continue;
                }

                const mapping = lineMap[vdocLine];
                if (!mapping.passage_name || !mapping.file_uri) {
                    // Preamble line — skip
                    continue;
                }

                // Parse the file URI
                let twUri: vscode.Uri;
                try {
                    twUri = vscode.Uri.parse(mapping.file_uri);
                } catch {
                    continue;
                }

                // Find the passage in the .tw file to compute the correct position
                // The mapping gives us original_line (0-based, within passage body).
                // We need to convert this to a document-absolute line number.
                const twLine = await this.findPassageBodyLine(
                    twUri,
                    mapping.passage_name,
                    mapping.original_line,
                );

                // Dedup: only keep the first diagnostic per source line
                const dedupKey = `${twUri.toString()}:${twLine}`;
                if (seenLines.has(dedupKey)) {
                    continue;
                }
                seenLines.add(dedupKey);

                // Create the diagnostic for the .tw file
                const twRange = new vscode.Range(
                    twLine,
                    0,
                    twLine,
                    1000, // Large end char to cover the line
                );

                const twDiag = new vscode.Diagnostic(
                    twRange,
                    `[JS] ${diag.message}`,
                    diag.severity ?? vscode.DiagnosticSeverity.Warning,
                );
                twDiag.source = 'knot (virtual doc)';
                twDiag.relatedInformation = [
                    new vscode.DiagnosticRelatedInformation(
                        new vscode.Location(vdocUri, diag.range),
                        'Virtual document JS error',
                    ),
                ];

                // Add to the map
                const key = twUri.toString();
                if (!this.twDiagnostics.has(key)) {
                    this.twDiagnostics.set(key, []);
                }
                this.twDiagnostics.get(key)!.push(twDiag);
            }
        }

        // Publish the .tw diagnostics
        this.diagnosticCollection.clear();
        for (const [uriStr, diags] of this.twDiagnostics) {
            try {
                const uri = vscode.Uri.parse(uriStr);
                this.diagnosticCollection.set(uri, diags);
            } catch {
                // Skip invalid URIs
            }
        }
    }

    /**
     * Find the document-absolute line number for a passage body line.
     *
     * Given a passage name and a 0-based line within the passage body,
     * find the actual line number in the .tw file by scanning for the
     * passage header and counting down.
     *
     * Uses `extractPassageName()` from utils.ts to correctly handle
     * passage names with spaces, tags, and JSON metadata.
     * For example, `:: Forest Entrance [outdoor]` yields "Forest Entrance",
     * not just "Forest".
     */
    private async findPassageBodyLine(
        twUri: vscode.Uri,
        passageName: string,
        bodyLine: number,
    ): Promise<number> {
        try {
            const doc = await vscode.workspace.openTextDocument(twUri);
            const text = doc.getText();
            const lines = text.split('\n');

            // Find the passage header line
            for (let i = 0; i < lines.length; i++) {
                const line = lines[i];
                if (!line.trimStart().startsWith('::')) {
                    continue;
                }
                // Use the shared extractPassageName() utility which correctly
                // strips tags ([...]) and JSON metadata ({...}), preserving
                // passage names that contain spaces like "Forest Entrance".
                const extractedName = extractPassageName(line).trim();
                if (extractedName === passageName) {
                    // The header is on line i. Body starts on line i+1.
                    // bodyLine is 0-based within the body, so:
                    return i + 1 + bodyLine;
                }
            }
        } catch {
            // If we can't read the file, fall back to bodyLine as-is
        }

        return bodyLine;
    }
}
