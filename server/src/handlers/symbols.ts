/**
 * Knot v2 — Symbols Handler
 *
 * Format-agnostic document and workspace symbol handler.
 * Uses core SymbolTable for symbol data.
 *
 * Imports:
 *   - hooks/hookTypes (MacroCategory, PassageType enums for grouping)
 *   - core/symbolTable (symbol data)
 *
 * MUST NOT import from: formats/<name>/
 */

import { MacroCategory, PassageType } from '../hooks/hookTypes';
// TODO: import { SymbolTable, SymbolKind } from '../core/symbolTable';

export class SymbolsHandler {
  /**
   * Handle a document symbols request.
   * Returns all symbols in a single document.
   */
  async handleDocumentSymbols(uri: string): Promise<unknown[]> {
    // TODO: Get all symbols for the document from SymbolTable
    // TODO: Build DocumentSymbol[] with hierarchical structure
    // TODO: Use MacroCategory/PassageType for symbol kind mapping
    throw new Error('TODO: implement handleDocumentSymbols()');
  }

  /**
   * Handle a workspace symbols request.
   * Returns symbols matching a query across the workspace.
   */
  async handleWorkspaceSymbols(query: string): Promise<unknown[]> {
    // TODO: Search SymbolTable for symbols matching query
    // TODO: Build SymbolInformation[]
    throw new Error('TODO: implement handleWorkspaceSymbols()');
  }
}
