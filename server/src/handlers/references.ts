/**
 * Knot v2 — References Handler
 *
 * Format-agnostic find-all-references handler.
 * Uses core ReferenceIndex for passage cross-references.
 *
 * Imports:
 *   - core/referenceIndex (reference data)
 *
 * MUST NOT import from: formats/<name>/
 */

// TODO: import { ReferenceIndex } from '../core/referenceIndex';

export class ReferencesHandler {
  /**
   * Handle a find-references request.
   * Looks up all references to the symbol under the cursor.
   */
  async handleReferences(
    uri: string,
    position: { line: number; character: number },
  ): Promise<unknown[]> {
    // TODO: Determine what's under the cursor (passage, variable, macro)
    // TODO: Look up references in ReferenceIndex (for passages)
    // TODO: Look up references in SymbolTable (for variables)
    throw new Error('TODO: implement handleReferences()');
  }
}
