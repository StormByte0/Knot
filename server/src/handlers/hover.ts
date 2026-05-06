/**
 * Knot v2 — Hover Handler
 *
 * Format-agnostic hover handler.
 * Delegates macro hover documentation through the format registry.
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, MacroDef, capability bags)
 *   - core/workspaceIndex (passage metadata)
 *
 * MUST NOT import from: formats/<name>/
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, MacroDef } from '../formats/_types';
// TODO: import { WorkspaceIndex } from '../core/workspaceIndex';

export class HoverHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Handle a hover request.
   * Routes to the appropriate documentation provider.
   */
  async handleHover(
    uri: string,
    position: { line: number; character: number },
  ): Promise<unknown> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Determine what's under the cursor
    // TODO: If macro and format has macros capability bag:
    //   const macro = format.macros?.builtins.find(m => m.name === name);
    //   Build Hover from MacroDef.description + signatures
    // TODO: If passage:
    //   Build Hover from passage metadata
    throw new Error('TODO: implement handleHover()');
  }
}
