/**
 * Knot v2 — Document Links Handler
 *
 * Format-agnostic document link handler.
 * Makes passage links clickable in the editor.
 * Uses format's FormatModule (via formatRegistry) for link syntax parsing.
 *
 * Imports:
 *   - formats/formatRegistry (format module for link parsing)
 *   - formats/_types (FormatModule, LinkResolution)
 *   - core/workspaceIndex (to validate link targets)
 *
 * MUST NOT import from: formats/<name>/
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, LinkResolution } from '../formats/_types';
// TODO: import { WorkspaceIndex } from '../core/workspaceIndex';

export class DocumentLinksHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Handle a document links request.
   * Finds all clickable links in a document.
   */
  async handleDocumentLinks(uri: string): Promise<unknown[]> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Parse document for link syntax using format.resolveLinkBody()
    // TODO: Validate targets against WorkspaceIndex
    // TODO: Build DocumentLink[]
    throw new Error('TODO: implement handleDocumentLinks()');
  }
}
