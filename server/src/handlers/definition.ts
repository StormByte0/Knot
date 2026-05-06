/**
 * Knot v2 — Definition Handler
 *
 * Format-agnostic go-to-definition handler.
 * Delegates macro definition resolution through the format registry.
 * Passage and variable definitions are resolved from core data structures.
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, capability bags)
 *   - core/workspaceIndex (passage location)
 *   - core/symbolTable (variable/macro location)
 *
 * MUST NOT import from: formats/<name>/
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule } from '../formats/_types';
// TODO: import { WorkspaceIndex } from '../core/workspaceIndex';
// TODO: import { SymbolTable } from '../core/symbolTable';

export class DefinitionHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Handle a go-to-definition request.
   * Routes to the appropriate resolver based on symbol type.
   */
  async handleDefinition(
    uri: string,
    position: { line: number; character: number },
  ): Promise<unknown> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Determine what's under the cursor (passage link, macro, variable)
    // TODO: If passage link → resolve from WorkspaceIndex
    // TODO: If macro and format has macros capability bag:
    //   const macro = format.macros?.builtins.find(m => m.name === name);
    // TODO: If variable → resolve from SymbolTable
    throw new Error('TODO: implement handleDefinition()');
  }
}
