/**
 * Knot v2 — Custom LSP Protocol
 *
 * Defines custom request/notification types for Knot-specific
 * communication between client and server. These extend the
 * standard LSP protocol with Knot-specific features.
 *
 * Client → Server requests:
 *   - knot/selectFormat     → Change active story format
 *   - knot/refreshDocuments → Force full re-index
 *
 * Server → Client notifications:
 *   - knot/formatChanged    → Active format was changed
 *   - knot/indexingComplete → Workspace indexing finished
 *   - knot/buildResult      → Build completed
 *
 * Server → Client requests:
 *   - knot/listPassages     → Get all passage names
 *   - knot/listFormats      → Get available format IDs
 */

import { NotificationType, RequestType } from 'vscode-languageserver';

// ─── Client → Server ────────────────────────────────────────────

export namespace SelectFormatRequest {
  export const type = new RequestType<{ formatId: string }, { success: boolean; formatName: string }, void>('knot/selectFormat');
}

export namespace RefreshDocumentsRequest {
  export const type = new RequestType<void, void, void>('knot/refreshDocuments');
}

// ─── Server → Client Notifications ──────────────────────────────

export namespace FormatChangedNotification {
  export const type = new NotificationType<{ formatId: string; formatName: string }>('knot/formatChanged');
}

export namespace IndexingCompleteNotification {
  export const type = new NotificationType<{ passageCount: number; durationMs: number }>('knot/indexingComplete');
}

export namespace BuildResultNotification {
  export const type = new NotificationType<{ success: boolean; outputPath?: string; errors?: string[] }>('knot/buildResult');
}

// ─── Server → Client Requests ───────────────────────────────────

export namespace ListPassagesRequest {
  export const type = new RequestType<void, string[], void>('knot/listPassages');
}

export namespace ListFormatsRequest {
  export const type = new RequestType<void, Array<{ id: string; name: string; version: string }>, void>('knot/listFormats');
}
