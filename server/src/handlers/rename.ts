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
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  TextEdit,
  WorkspaceEdit,
  PrepareRenameResult,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule } from '../formats/_types';
import { PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface RenameContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class RenameHandler {
  private ctx: RenameContext;

  constructor(ctx: RenameContext) {
    this.ctx = ctx;
  }

  /**
   * Prepare a rename operation. Checks if rename is valid at the given position.
   */
  handlePrepareRename(params: { textDocument: { uri: string }; position: { line: number; character: number } }): PrepareRenameResult | null {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);

    // 1. Passage link — can rename
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      // Find the actual reference range at this offset
      const passages = this.ctx.workspaceIndex.getPassagesByUri(uri);
      for (const passage of passages) {
        for (const ref of passage.passageRefs) {
          if (offset >= ref.range.start && offset <= ref.range.end) {
            if (ref.linkKind === LinkKind.Passage || ref.kind === PassageRefKind.Macro || ref.kind === PassageRefKind.Implicit) {
              return {
                range: { start: doc.positionAt(ref.range.start), end: doc.positionAt(ref.range.end) },
                placeholder: ref.target,
              };
            }
          }
        }
      }
    }

    // 2. Variable — can rename
    const varAtCursor = this.findVariableAtOffset(offset, uri);
    if (varAtCursor) {
      return {
        range: { start: doc.positionAt(offset), end: doc.positionAt(offset + varAtCursor.name.length + 1) },
        placeholder: `${varAtCursor.sigil}${varAtCursor.name}`,
      };
    }

    return null;
  }

  /**
   * Execute a rename operation across all reference sites.
   */
  handleRename(params: { textDocument: { uri: string }; position: { line: number; character: number }; newName: string }): WorkspaceEdit | null {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);
    const newName = params.newName;

    // ── Passage rename ──────────────────────────────────────────
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      // Find all documents that reference this passage
      const changes: Record<string, TextEdit[]> = {};
      const passages = this.ctx.workspaceIndex.getPassagesReferencing(linkTarget);

      for (const p of passages) {
        const pDoc = this.ctx.documentStore.get(p.uri);
        if (!pDoc) continue;

        const edits: TextEdit[] = [];

        for (const ref of p.passageRefs) {
          if (ref.target !== linkTarget) continue;

          // For [[ ]] links, we need to replace the target within the link text
          // while preserving display text and separator syntax
          if (ref.kind === PassageRefKind.Link) {
            const linkStart = ref.range.start;
            const linkEnd = ref.range.end;
            const linkText = pDoc.getText().substring(linkStart, linkEnd);

            // Find the target within the link text and replace it
            const format = this.ctx.formatRegistry.getActiveFormat();
            const resolved = format.resolveLinkBody(linkText.slice(2, -2));
            if (resolved.target) {
              const newLinkBody = linkText.slice(2, -2).replace(this.escapeRegex(resolved.target), newName);
              const newLink = `[[${newLinkBody}]]`;
              edits.push(TextEdit.replace(
                { start: pDoc.positionAt(linkStart), end: pDoc.positionAt(linkEnd) },
                newLink,
              ));
            }
          } else {
            // For macros and implicit refs, simple target replacement
            edits.push(TextEdit.replace(
              { start: pDoc.positionAt(ref.range.start), end: pDoc.positionAt(ref.range.end) },
              newName,
            ));
          }
        }

        if (edits.length > 0) {
          changes[p.uri] = (changes[p.uri] ?? []).concat(edits);
        }
      }

      // Also rename the passage definition header itself (:: OldName → :: NewName)
      const targetPassage = this.ctx.workspaceIndex.getPassage(linkTarget);
      if (targetPassage) {
        const tDoc = this.ctx.documentStore.get(targetPassage.uri);
        if (tDoc) {
          // The passage header starts with :: OldName — find the exact name position
          const headerText = tDoc.getText().substring(targetPassage.startOffset, targetPassage.startOffset + 200);
          const nameMatch = headerText.match(/^::\s*([^\[\s]+)/);
          if (nameMatch && nameMatch[1] === linkTarget) {
            const nameStartInDoc = targetPassage.startOffset + nameMatch.index! + nameMatch[0].indexOf(nameMatch[1]);
            const nameEndInDoc = nameStartInDoc + linkTarget.length;
            const headerEdit = TextEdit.replace(
              { start: tDoc.positionAt(nameStartInDoc), end: tDoc.positionAt(nameEndInDoc) },
              newName,
            );
            if (!changes[targetPassage.uri]) {
              changes[targetPassage.uri] = [];
            }
            changes[targetPassage.uri].push(headerEdit);
          }
        }
      }

      return { changes };
    }

    // ── Variable rename ─────────────────────────────────────────
    const varAtCursor = this.findVariableAtOffset(offset, uri);
    if (varAtCursor) {
      const format = this.ctx.formatRegistry.getActiveFormat();
      if (!format.variables) return null;

      const newFullName = newName.startsWith(varAtCursor.sigil) ? newName : `${varAtCursor.sigil}${newName}`;

      const varChanges: Record<string, TextEdit[]> = {};
      const allPassages = this.ctx.workspaceIndex.getAllPassages();

      for (const p of allPassages) {
        if (!(p.storyVars.has(varAtCursor.name) || p.tempVars.has(varAtCursor.name))) continue;

        const pDoc = this.ctx.documentStore.get(p.uri);
        if (!pDoc) continue;

        const edits: TextEdit[] = [];
        const pattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);
        const body = p.body;
        let match: RegExpExecArray | null;

        while ((match = pattern.exec(body)) !== null) {
          const sigil = match[1];
          const name = match[2];
          if (sigil === varAtCursor.sigil && name === varAtCursor.name) {
            const passageEntry = this.ctx.workspaceIndex.getPassage(p.name);
            const bodyOffset = passageEntry ? passageEntry.startOffset : 0;
            edits.push(TextEdit.replace(
              { start: pDoc.positionAt(bodyOffset + match.index), end: pDoc.positionAt(bodyOffset + match.index + match[0].length) },
              newFullName,
            ));
          }
        }

        if (edits.length > 0) {
          varChanges[p.uri] = (varChanges[p.uri] ?? []).concat(edits);
        }
      }

      return { changes: varChanges };
    }

    return null;
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

  private escapeRegex(str: string): string {
    return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
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
}
