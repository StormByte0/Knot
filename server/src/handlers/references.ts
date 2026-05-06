/**
 * Knot v2 — References Handler
 *
 * Format-agnostic find-all-references handler.
 * Uses core ReferenceIndex for passage cross-references.
 *
 * Imports:
 *   - core/referenceIndex (reference data)
 *   - core/workspaceIndex (passage data)
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  Location,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { FormatRegistry } from '../formats/formatRegistry';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface ReferencesContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class ReferencesHandler {
  private ctx: ReferencesContext;

  constructor(ctx: ReferencesContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a find-references request.
   * Looks up all references to the symbol under the cursor.
   */
  handleReferences(params: { textDocument: { uri: string }; position: { line: number; character: number } }): Location[] {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return [];

    const offset = doc.offsetAt(params.position);
    const locations: Location[] = [];

    // 1. Check for passage link reference
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passages = this.ctx.workspaceIndex.getPassagesReferencing(linkTarget);
      for (const p of passages) {
        const pDoc = this.ctx.documentStore.get(p.uri);
        if (pDoc) {
          // Find the specific reference locations within the passage
          for (const ref of p.passageRefs) {
            if (ref.target === linkTarget) {
              locations.push(Location.create(p.uri, {
                start: pDoc.positionAt(ref.range.start),
                end: pDoc.positionAt(ref.range.end),
              }));
            }
          }
        }
      }
      return locations;
    }

    // 2. Check for variable reference
    const varAtCursor = this.findVariableAtOffset(offset, uri);
    if (varAtCursor) {
      const format = this.ctx.formatRegistry.getActiveFormat();
      const allPassages = this.ctx.workspaceIndex.getAllPassages();
      for (const p of allPassages) {
        const pDoc = this.ctx.documentStore.get(p.uri);
        if (!pDoc) continue;

        // Check if this passage has the variable
        const hasVar = (p.storyVars.has(varAtCursor.name) || p.tempVars.has(varAtCursor.name));
        if (hasVar) {
          // Find all occurrences of this variable in the passage body
          if (format.variables) {
            const pattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
            const body = p.body;
            let match: RegExpExecArray | null;
            while ((match = pattern.exec(body)) !== null) {
              const sigil = match[1];
              const name = match[2];
              if (sigil === varAtCursor.sigil && name === varAtCursor.name) {
                // The match index is relative to the body, need to add body offset
                const passageEntry = this.ctx.workspaceIndex.getPassage(p.name);
                const bodyOffset = passageEntry ? passageEntry.startOffset : 0;
                locations.push(Location.create(p.uri, {
                  start: pDoc.positionAt(bodyOffset + match.index),
                  end: pDoc.positionAt(bodyOffset + match.index + match[0].length),
                }));
              }
            }
          }
        }
      }
      return locations;
    }

    // 3. Check for macro reference
    const macroAtCursor = this.findMacroAtOffset(offset, uri);
    if (macroAtCursor) {
      const allPassages = this.ctx.workspaceIndex.getAllPassages();
      for (const p of allPassages) {
        const pDoc = this.ctx.documentStore.get(p.uri);
        if (!pDoc) continue;

        if (p.macroNames.includes(macroAtCursor)) {
          // Find all occurrences of this macro in the passage body
          const format = this.ctx.formatRegistry.getActiveFormat();
          if (format.macroPattern) {
            const pattern = new RegExp(format.macroPattern.source, format.macroPattern.flags);
            const body = p.body;
            let match: RegExpExecArray | null;
            while ((match = pattern.exec(body)) !== null) {
              if (match[1] === macroAtCursor) {
                const passageEntry = this.ctx.workspaceIndex.getPassage(p.name);
                const bodyOffset = passageEntry ? passageEntry.startOffset : 0;
                locations.push(Location.create(p.uri, {
                  start: pDoc.positionAt(bodyOffset + match.index),
                  end: pDoc.positionAt(bodyOffset + match.index + match[0].length),
                }));
              }
            }
          }
        }
      }
      return locations;
    }

    return locations;
  }

  // ─── Private Helpers ──────────────────────────────────────────

  private findLinkTargetAtOffset(offset: number, uri: string): string | null {
    const passages = this.ctx.workspaceIndex.getPassagesByUri(uri);
    for (const passage of passages) {
      for (const ref of passage.passageRefs) {
        if (offset >= ref.range.start && offset <= ref.range.end) {
          if (ref.linkKind === LinkKind.Passage || ref.kind === PassageRefKind.Macro || ref.kind === PassageRefKind.Implicit) {
            return ref.target;
          }
        }
      }
    }
    return null;
  }

  private findVariableAtOffset(offset: number, uri: string): { sigil: string; name: string } | null {
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;
    const format = this.ctx.formatRegistry.getActiveFormat();
    if (!format.variables) return null;

    const text = doc.getText();
    const headerRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    let bodyStart = 0;
    let bodyEnd = text.length;

    while ((passageMatch = headerRegex.exec(text)) !== null) {
      const nextBodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', nextBodyStart);
      const nextBodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;
      if (offset >= nextBodyStart && offset <= nextBodyEnd) {
        bodyStart = nextBodyStart;
        bodyEnd = nextBodyEnd;
        break;
      }
    }

    const bodyText = text.substring(bodyStart, bodyEnd);
    const tokens = format.lexBody(bodyText, bodyStart);
    for (const token of tokens) {
      if (token.typeId === 'variable' && token.varName) {
        if (offset >= token.range.start && offset <= token.range.end) {
          return { sigil: token.varSigil ?? '$', name: token.varName };
        }
      }
    }
    return null;
  }

  private findMacroAtOffset(offset: number, uri: string): string | null {
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;
    const format = this.ctx.formatRegistry.getActiveFormat();

    const text = doc.getText();
    const headerRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    let bodyStart = 0;
    let bodyEnd = text.length;

    while ((passageMatch = headerRegex.exec(text)) !== null) {
      const nextBodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', nextBodyStart);
      const nextBodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;
      if (offset >= nextBodyStart && offset <= nextBodyEnd) {
        bodyStart = nextBodyStart;
        bodyEnd = nextBodyEnd;
        break;
      }
    }

    const bodyText = text.substring(bodyStart, bodyEnd);
    const tokens = format.lexBody(bodyText, bodyStart);
    for (const token of tokens) {
      if ((token.typeId === 'macro-call' || token.typeId === 'macro-close') && token.macroName) {
        if (offset >= token.range.start && offset <= token.range.end) {
          return token.macroName;
        }
      }
    }
    return null;
  }
}
