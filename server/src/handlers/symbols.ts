/**
 * Knot v2 — Symbols Handler
 *
 * Format-agnostic document and workspace symbol handler.
 * Uses core WorkspaceIndex for symbol data.
 *
 * Provides:
 *   - Document symbols: passages (Class) with child variables (Variable)
 *   - Workspace symbols: passages matching a query
 *
 * Imports:
 *   - hooks/hookTypes (PassageType enum for grouping)
 *   - core/workspaceIndex (passage data)
 *   - core/documentStore (document content)
 *   - formats/formatRegistry (format for variable sigils)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  DocumentSymbol,
  SymbolKind as LspSymbolKind,
  WorkspaceSymbol,
} from 'vscode-languageserver/node';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';
import { FormatRegistry } from '../formats/formatRegistry';

export interface SymbolsContext {
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
  formatRegistry: FormatRegistry;
}

export class SymbolsHandler {
  private ctx: SymbolsContext;

  constructor(ctx: SymbolsContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a document symbols request.
   * Returns all symbols in a single document, with passages as
   * top-level symbols (Class) and variables as children (Variable).
   */
  handleDocumentSymbols(params: { textDocument: { uri: string } }): DocumentSymbol[] {
    const uri = params.textDocument.uri;
    const passages = this.ctx.workspaceIndex.getPassagesByUri(uri);
    const format = this.ctx.formatRegistry.getActiveFormat();
    const symbols: DocumentSymbol[] = [];

    for (const p of passages) {
      const doc = this.ctx.documentStore.get(p.uri);
      if (!doc) continue;

      const range = {
        start: doc.positionAt(p.startOffset),
        end: doc.positionAt(p.endOffset),
      };

      // Build child symbols for variables declared in this passage
      const children: DocumentSymbol[] = [];

      // Add story variables as children
      if (format.variables) {
        for (const varName of p.storyVars) {
          // Find the variable's location within the passage body
          const varLocation = this.findVariableInBody(doc, p.startOffset, p.body, varName, '$');
          const varRange = varLocation ?? range;
          const varSelectionRange = varLocation ?? range;
          children.push(DocumentSymbol.create(
            `$${varName}`,
            'story variable',
            LspSymbolKind.Variable,
            varRange,
            varSelectionRange,
          ));
        }

        // Add temp variables as children
        for (const varName of p.tempVars) {
          const sigil = format.variables.sigils.find(s => s.kind === 'temp')?.sigil ?? '_';
          const varLocation = this.findVariableInBody(doc, p.startOffset, p.body, varName, sigil);
          const varRange = varLocation ?? range;
          const varSelectionRange = varLocation ?? range;
          children.push(DocumentSymbol.create(
            `${sigil}${varName}`,
            'temp variable',
            LspSymbolKind.Variable,
            varRange,
            varSelectionRange,
          ));
        }
      }

      symbols.push(DocumentSymbol.create(
        p.name,
        p.type,
        LspSymbolKind.Class,
        range,
        range,
        children,
      ));
    }

    return symbols;
  }

  /**
   * Handle a workspace symbols request.
   * Returns symbols matching a query across the workspace.
   */
  handleWorkspaceSymbols(params: { query: string }): WorkspaceSymbol[] {
    const query = params.query.toLowerCase();
    const symbols: WorkspaceSymbol[] = [];

    for (const p of this.ctx.workspaceIndex.getAllPassages()) {
      if (p.name.toLowerCase().includes(query)) {
        symbols.push({
          name: p.name,
          kind: LspSymbolKind.Class,
          location: {
            uri: p.uri,
          },
          containerName: p.type,
        });
      }

      // Also search variables
      const format = this.ctx.formatRegistry.getActiveFormat();
      if (format.variables) {
        for (const varName of p.storyVars) {
          if (varName.toLowerCase().includes(query)) {
            symbols.push({
              name: `$${varName}`,
              kind: LspSymbolKind.Variable,
              location: { uri: p.uri },
              containerName: p.name,
            });
          }
        }
        for (const varName of p.tempVars) {
          if (varName.toLowerCase().includes(query)) {
            symbols.push({
              name: `_${varName}`,
              kind: LspSymbolKind.Variable,
              location: { uri: p.uri },
              containerName: p.name,
            });
          }
        }
      }
    }

    return symbols;
  }

  /**
   * Find the exact range of a variable reference in the passage body.
   * Returns null if the variable can't be located precisely.
   */
  private findVariableInBody(
    doc: import('vscode-languageserver-textdocument').TextDocument,
    bodyStartOffset: number,
    body: string,
    varName: string,
    sigil: string,
  ): { start: import('vscode-languageserver/node').Position; end: import('vscode-languageserver/node').Position } | null {
    // Search for the first occurrence of $varName or _varName in the body
    const pattern = new RegExp(`\\${sigil}${varName}\\b`);
    const match = pattern.exec(body);
    if (match) {
      const absOffset = bodyStartOffset + match.index;
      const absEnd = absOffset + match[0].length;
      return {
        start: doc.positionAt(absOffset),
        end: doc.positionAt(absEnd),
      };
    }
    return null;
  }
}
