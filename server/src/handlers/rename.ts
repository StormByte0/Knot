/**
 * Knot v2 — Rename Handler
 *
 * Format-agnostic rename handler.
 * Uses core data structures to find all rename sites.
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, capability bags)
 *   - core/workspaceIndex (passage name lookup)
 *   - core/referenceIndex (cross-references)
 *   - core/symbolTable (variable references)
 *
 * MUST NOT import from: formats/<name>/
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule } from '../formats/_types';
// TODO: import { WorkspaceIndex } from '../core/workspaceIndex';
// TODO: import { ReferenceIndex } from '../core/referenceIndex';
// TODO: import { SymbolTable } from '../core/symbolTable';

export class RenameHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Prepare a rename operation. Checks if rename is valid at the given position.
   */
  async handlePrepareRename(
    uri: string,
    position: { line: number; character: number },
  ): Promise<unknown> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Check if symbol is renameable
    // TODO: Check if format supports rename via capability bags
    //   (e.g. format.variables for variable rename, format.macros for macro rename)
    // TODO: Return current name and range
    throw new Error('TODO: implement handlePrepareRename()');
  }

  /**
   * Execute a rename operation across all reference sites.
   */
  async handleRename(
    uri: string,
    position: { line: number; character: number },
    newName: string,
  ): Promise<unknown> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Find all references to the symbol
    // TODO: Build WorkspaceEdit with all rename locations
    throw new Error('TODO: implement handleRename()');
  }
}
