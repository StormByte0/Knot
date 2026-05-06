/**
 * Knot v2 — Document Store
 *
 * Manages the lifecycle of open documents in the workspace.
 * Format-agnostic — stores raw document content and metadata.
 *
 * Promises:
 *   - Track open documents
 *   - Provide document content on demand
 *   - Document change notifications
 *
 * Imports:
 *   - (none from formats/)
 *
 * MUST NOT import from: formats/
 */

import { TextDocument } from 'vscode-languageserver-textdocument';

export class DocumentStore {
  private documents: Map<string, TextDocument> = new Map();

  /**
   * Add or update a document in the store.
   */
  set(uri: string, document: TextDocument): void {
    this.documents.set(uri, document);
  }

  /**
   * Get a document by URI.
   */
  get(uri: string): TextDocument | undefined {
    return this.documents.get(uri);
  }

  /**
   * Remove a document from the store.
   */
  delete(uri: string): boolean {
    return this.documents.delete(uri);
  }

  /**
   * Check if a document is tracked.
   */
  has(uri: string): boolean {
    return this.documents.has(uri);
  }

  /**
   * Get all tracked document URIs.
   */
  getUris(): string[] {
    return Array.from(this.documents.keys());
  }

  /**
   * Get the number of tracked documents.
   */
  get size(): number {
    return this.documents.size;
  }
}
