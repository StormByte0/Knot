/**
 * Knot v2 — Semantic Tokens Handler
 *
 * Format-agnostic semantic token provider.
 * Tokenizes passage headers (Twine engine level) and
 * format-driven body tokens (macros, variables, hooks).
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *   - core/documentStore (document content)
 *   - core/workspaceIndex (passage metadata)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  SemanticTokens,
  SemanticTokensBuilder,
  SemanticTokenTypes,
} from 'vscode-languageserver/node';
import { FormatRegistry } from '../formats/formatRegistry';
import { DocumentStore } from '../core/documentStore';
import { WorkspaceIndex } from '../core/workspaceIndex';

// ─── Semantic Token Legend ──────────────────────────────────────

export const TOKEN_TYPES: string[] = [
  SemanticTokenTypes.function,   // macros
  SemanticTokenTypes.class,      // passages
  SemanticTokenTypes.variable,   // variables
  SemanticTokenTypes.operator,   // macro delimiters
  SemanticTokenTypes.string,     // strings
  SemanticTokenTypes.number,     // numbers
  SemanticTokenTypes.comment,    // comments
];

export const TOKEN_MODIFIERS: string[] = [];

// ─── Context ────────────────────────────────────────────────────

export interface SemanticTokensContext {
  formatRegistry: FormatRegistry;
  documentStore: DocumentStore;
  workspaceIndex: WorkspaceIndex;
}

// ─── Handler ────────────────────────────────────────────────────

export class SemanticTokensHandler {
  private ctx: SemanticTokensContext;

  constructor(ctx: SemanticTokensContext) {
    this.ctx = ctx;
  }

  handleSemanticTokens(params: { textDocument: { uri: string } }): SemanticTokens {
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return { data: [] };

    const builder = new SemanticTokensBuilder();
    const text = doc.getText();
    const format = this.ctx.formatRegistry.getActiveFormat();

    // Tokenize passage headers (Twine engine level — always :: headers)
    const headerRegex = /^::\s*([^\[\]\n]+)/gm;
    let match: RegExpExecArray | null;
    while ((match = headerRegex.exec(text)) !== null) {
      const start = doc.positionAt(match.index);
      // Push passage name as "class" token
      builder.push(start.line, start.character + 3, match[1].trim().length, 1, 0); // 1 = class
    }

    // Tokenize macros and variables using format-driven body lexing
    const passageHeaderRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    while ((passageMatch = passageHeaderRegex.exec(text)) !== null) {
      const bodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', bodyStart);
      const bodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;
      const bodyText = text.substring(bodyStart, bodyEnd);

      const tokens = format.lexBody(bodyText, bodyStart);
      for (const token of tokens) {
        if (token.typeId === 'macro-call') {
          // Only highlight known macros
          const isKnown = format.macros?.builtins.some(m => m.name === (token.macroName ?? '')) ?? false;
          if (isKnown) {
            const pos = doc.positionAt(token.range.start);
            builder.push(pos.line, pos.character, token.range.end - token.range.start, 0, 0); // 0 = function
          }
        } else if (token.typeId === 'macro-close') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 0, 0); // 0 = function
        } else if (token.typeId === 'variable') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 2, 0); // 2 = variable
        } else if (token.typeId === 'hook-open' || token.typeId === 'hook-close') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 3, 0); // 3 = operator
        }
      }
    }

    return builder.build();
  }
}
