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
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  Hover,
  MarkupKind,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule, BodyToken } from '../formats/_types';
import { PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface HoverContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class HoverHandler {
  private ctx: HoverContext;

  constructor(ctx: HoverContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a hover request.
   * Routes to the appropriate documentation provider.
   */
  handleHover(params: { textDocument: { uri: string }; position: { line: number; character: number } }): Hover | null {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return null;

    const position = params.position;
    const offset = doc.offsetAt(position);
    const text = doc.getText();
    const format = this.ctx.formatRegistry.getActiveFormat();

    // Check if cursor is on a macro
    const macroMatch = this.findMacroAtOffset(text, offset, format);
    if (macroMatch && format.macros !== undefined) {
      const macro = format.macros.builtins.find(m => m.name === macroMatch.name);
      if (macro) {
        const prefix = format.macroDelimiters.open;
        const suffix = format.macroDelimiters.close;
        const sigText = macro.signatures
          .map(s => `${prefix}${macro.name} ${s.args.map(a => a.name).join(' ')}${suffix}`)
          .join('\n\n');
        return {
          contents: {
            kind: MarkupKind.Markdown,
            value: `**${prefix}${macro.name}${suffix}** — ${macro.category}\n\n${macro.description}\n\n\`\`\`\n${sigText}\n\`\`\``,
          },
        };
      }
    }

    // Check if cursor is on a variable
    const varAtCursor = this.findVariableAtOffset(offset, uri);
    if (varAtCursor) {
      const activeFormat = this.ctx.formatRegistry.getActiveFormat();
      const sigilDef = activeFormat.variables?.sigils.find(s => s.sigil === varAtCursor.sigil);
      const scope = sigilDef?.kind ?? 'unknown';

      // Find all passages that declare/assign this variable
      const declaringPassages: string[] = [];
      const allPassages = this.ctx.workspaceIndex.getAllPassages();
      for (const p of allPassages) {
        if (p.storyVars.has(varAtCursor.name) || p.tempVars.has(varAtCursor.name)) {
          declaringPassages.push(p.name);
        }
      }

      return {
        contents: {
          kind: MarkupKind.Markdown,
          value: `**${varAtCursor.sigil}${varAtCursor.name}** — ${scope} variable\n\n${sigilDef?.description ?? ''}${declaringPassages.length > 0 ? `\n\nDeclared in: ${declaringPassages.slice(0, 5).join(', ')}${declaringPassages.length > 5 ? ', ...' : ''}` : ''}`,
        },
      };
    }

    // Check if cursor is on a passage link
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passage = this.ctx.workspaceIndex.getPassage(linkTarget);
      if (passage) {
        return {
          contents: {
            kind: MarkupKind.Markdown,
            value: `**${passage.name}** — ${passage.type}\n\nTags: ${passage.tags.length > 0 ? passage.tags.join(', ') : 'none'}`,
          },
        };
      }
    }

    return null;
  }

  // ─── Private Helpers ──────────────────────────────────────────

  private findMacroAtOffset(text: string, offset: number, format: FormatModule): { name: string } | null {
    // Use format-driven lexing to find macro at offset
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
          return { name: token.macroName };
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
}
