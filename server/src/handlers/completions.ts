/**
 * Knot v2 — Completion Handler
 *
 * Format-agnostic completion handler. Delegates macro and passage
 * completion through the format registry.
 *
 * Data flow:
 *   1. LSP completion request arrives
 *   2. Handler resolves active format via formatRegistry
 *   3. If format has macros capability bag, get macros from format.macros.builtins
 *   4. If format has variables capability bag, get variables from SymbolTable
 *   5. Passage completion is format-agnostic (from WorkspaceIndex)
 *   6. Build CompletionItem[] using enum-based categorization
 *   7. Return — handler never knew which format was active
 *
 * Imports:
 *   - hooks/hookTypes (MacroCategory enum for categorization)
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, MacroDef, capability bags)
 *   - core/workspaceIndex (passage data)
 *   - core/symbolTable (variable data)
 *
 * MUST NOT import from: formats/<name>/
 */

import { MacroCategory } from '../hooks/hookTypes';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, MacroDef } from '../formats/_types';
// TODO: import { WorkspaceIndex } from '../core/workspaceIndex';
// TODO: import { SymbolTable } from '../core/symbolTable';

export class CompletionHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Handle a completion request.
   * Delegates to format module for macro/variable completions.
   * Uses WorkspaceIndex for passage completions (format-agnostic).
   */
  async handleCompletion(
    uri: string,
    position: { line: number; character: number },
    triggerCharacter?: string,
  ): Promise<unknown[]> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Determine completion context (macro, passage, variable)
    // TODO: If context is macro and format has macros capability bag:
    //   const macros = format.macros?.builtins ?? [];
    //   Filter by MacroCategory if applicable
    //   Build CompletionItem[] from MacroDef[]
    // TODO: If context is passage link:
    //   Get passages from WorkspaceIndex, build CompletionItem[]
    // TODO: If context is variable and format has variables capability bag:
    //   Get variables from SymbolTable, build CompletionItem[]
    throw new Error('TODO: implement handleCompletion()');
  }

  /**
   * Resolve additional details for a completion item.
   * Delegates to format module for documentation.
   */
  async handleCompletionResolve(item: unknown): Promise<unknown> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Use format.macros to enrich completion item with docs
    throw new Error('TODO: implement handleCompletionResolve()');
  }
}
