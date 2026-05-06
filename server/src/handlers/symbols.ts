/**
 * Knot v2 — Symbols Handler
 *
 * Format-agnostic document and workspace symbol handler.
 * Uses core WorkspaceIndex for symbol data.
 *
 * Imports:
 *   - hooks/hookTypes (PassageType enum for grouping)
 *   - core/workspaceIndex (passage data)
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  DocumentSymbol,
  SymbolKind as LspSymbolKind,
  WorkspaceSymbol,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface SymbolsContext {
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class SymbolsHandler {
  private ctx: SymbolsContext;

  constructor(ctx: SymbolsContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a document symbols request.
   * Returns all symbols in a single document.
   */
  handleDocumentSymbols(params: { textDocument: { uri: string } }): DocumentSymbol[] {
    const uri = params.textDocument.uri;
    const passages = this.ctx.workspaceIndex.getPassagesByUri(uri);
    const symbols: DocumentSymbol[] = [];

    for (const p of passages) {
      const doc = this.ctx.documentStore.get(p.uri);
      if (!doc) continue;

      const range = {
        start: doc.positionAt(p.startOffset),
        end: doc.positionAt(p.endOffset),
      };

      symbols.push(DocumentSymbol.create(
        p.name,
        p.type,
        LspSymbolKind.Class,
        range,
        range,
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
    }

    return symbols;
  }
}
