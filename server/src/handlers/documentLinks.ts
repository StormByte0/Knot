/**
 * Knot v2 — Document Links Handler
 *
 * Format-agnostic document link handler.
 * Makes passage links clickable in the editor.
 * Uses format's FormatModule (via formatRegistry) for link syntax parsing.
 *
 * Imports:
 *   - formats/formatRegistry (format module for link parsing)
 *   - core/workspaceIndex (to validate link targets)
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  DocumentLink,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, LinkResolution } from '../formats/_types';
import { PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface DocumentLinksContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class DocumentLinksHandler {
  private ctx: DocumentLinksContext;

  constructor(ctx: DocumentLinksContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a document links request.
   * Finds all clickable links in a document.
   */
  handleDocumentLinks(params: { textDocument: { uri: string } }): DocumentLink[] {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return [];

    const links: DocumentLink[] = [];
    const passages = this.ctx.workspaceIndex.getPassagesByUri(uri);

    // Use passageRefs from the index (format-driven, single source of truth)
    for (const passage of passages) {
      for (const ref of passage.passageRefs) {
        if (ref.kind === PassageRefKind.Link && ref.linkKind === LinkKind.Passage) {
          if (this.ctx.workspaceIndex.hasPassage(ref.target)) {
            const targetPassage = this.ctx.workspaceIndex.getPassage(ref.target)!;
            const targetDoc = this.ctx.documentStore.get(targetPassage.uri);
            if (targetDoc) {
              links.push(DocumentLink.create(
                {
                  start: doc.positionAt(ref.range.start),
                  end: doc.positionAt(ref.range.end),
                },
                `${targetPassage.uri}#L1`,
              ));
            }
          }
        }
      }
    }

    return links;
  }
}
