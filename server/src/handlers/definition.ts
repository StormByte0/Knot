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
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  Location,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, BodyToken } from '../formats/_types';
import { PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { WorkspaceIndex, PassageEntry } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface DefinitionContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class DefinitionHandler {
  private ctx: DefinitionContext;

  constructor(ctx: DefinitionContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a go-to-definition request.
   * Routes to the appropriate resolver based on symbol type.
   */
  handleDefinition(params: { textDocument: { uri: string }; position: { line: number; character: number } }): Location | Location[] | null {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);

    // Passage link definition
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passage = this.ctx.workspaceIndex.getPassage(linkTarget);
      if (passage) {
        const targetDoc = this.ctx.documentStore.get(passage.uri);
        if (targetDoc) {
          return Location.create(passage.uri, {
            start: targetDoc.positionAt(passage.startOffset),
            end: targetDoc.positionAt(passage.endOffset),
          });
        }
      }
    }

    // Variable definition — find the first assignment of this variable
    const text = doc.getText();
    const varAtCursor = this.findVariableAtOffset(text, offset);
    if (varAtCursor) {
      const format = this.ctx.formatRegistry.getActiveFormat();
      const allPassages = this.ctx.workspaceIndex.getAllPassages();

      // Check special passages first (init/header) — they declare variables before usage
      const initPassage = format.specialPassages.find(sp => sp.typeId === 'init');
      const headerPassage = format.specialPassages.find(sp => sp.typeId === 'header');
      const priorityPassageNames = [
        initPassage?.name,
        headerPassage?.name,
      ].filter((n): n is string => !!n);

      // Sort passages: priority passages first, then by name
      const sortedPassages = [...allPassages].sort((a, b) => {
        const aIdx = priorityPassageNames.indexOf(a.name);
        const bIdx = priorityPassageNames.indexOf(b.name);
        if (aIdx !== -1 && bIdx !== -1) return aIdx - bIdx;
        if (aIdx !== -1) return -1;
        if (bIdx !== -1) return 1;
        return 0;
      });

      for (const p of sortedPassages) {
        // Look up the sigil definition to determine variable scope — NEVER hardcode sigil logic
        const sigilDef = format.variables?.sigils.find(s => s.sigil === varAtCursor.sigil);
        const scope = sigilDef?.kind ?? null;
        const varSet = scope === 'story' ? p.storyVars : scope === 'temp' ? p.tempVars : null;
        if (varSet && varSet.has(varAtCursor.name)) {
          const pDoc = this.ctx.documentStore.get(p.uri);
          if (pDoc) {
            // Try to find the exact assignment location within this passage
            const exactLocation = this.findVariableAssignmentLocation(pDoc, p, varAtCursor, format);
            if (exactLocation) return exactLocation;

            // Fallback: return the passage header range
            return Location.create(p.uri, {
              start: pDoc.positionAt(p.startOffset),
              end: pDoc.positionAt(p.endOffset),
            });
          }
        }
      }
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

  private findVariableAtOffset(text: string, offset: number): { sigil: string; name: string } | null {
    const format = this.ctx.formatRegistry.getActiveFormat();

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

  /**
   * Find the exact location of a variable assignment in a passage.
   * Looks for assignment macros (set, capture, etc.) that assign to this variable.
   */
  private findVariableAssignmentLocation(
    pDoc: TextDocument,
    passage: PassageEntry,
    varInfo: { sigil: string; name: string },
    format: FormatModule,
  ): Location | null {
    if (!format.variables) return null;

    const body = passage.body;
    const bodyOffset = passage.startOffset;

    // Look for assignment patterns like: <<set $var to ...>> or (set: $var to ...)
    const assignmentMacros = format.variables.assignmentMacros;
    const varPattern = new RegExp(format.variables.variablePattern.source, format.variables.variablePattern.flags);

    // Check each line for an assignment macro containing this variable
    const lines = body.split('\n');
    let currentOffset = 0;

    for (const line of lines) {
      const trimmed = line.trim();

      // Check if this line contains an assignment macro
      const hasAssignmentMacro = Array.from(assignmentMacros).some(m =>
        trimmed.includes(m) || (format.macroPattern && new RegExp(format.macroPattern.source).test(trimmed))
      );

      if (hasAssignmentMacro) {
        // Check if this line assigns to our variable
        let match: RegExpExecArray | null;
        const lineVarPattern = new RegExp(varPattern.source, varPattern.flags);
        while ((match = lineVarPattern.exec(line)) !== null) {
          const sigil = match[1];
          const name = match[2];
          if (sigil === varInfo.sigil && name === varInfo.name) {
            // Found the assignment — check if the variable is on the left side
            // (before "to" or "=" for SugarCube, before "to" for Harlowe)
            const varPos = match.index;
            const afterVar = line.substring(varPos + match[0].length).trimStart();
            const ops = format.variables.assignmentOperators;
            if (ops.some(op => afterVar.startsWith(op))) {
              // This is an assignment to our variable
              const absOffset = bodyOffset + currentOffset + varPos;
              return Location.create(passage.uri, {
                start: pDoc.positionAt(absOffset),
                end: pDoc.positionAt(absOffset + match[0].length),
              });
            }
          }
        }
      }

      currentOffset += line.length + 1; // +1 for the \n
    }

    return null;
  }
}
