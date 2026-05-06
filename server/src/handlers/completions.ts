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
 *   - hooks/hookTypes (MacroKind enum for categorization)
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, MacroDef, capability bags)
 *   - core/workspaceIndex (passage data)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  CompletionItem,
  CompletionItemKind,
  InsertTextFormat,
  MarkupKind,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule } from '../formats/_types';
import { MacroKind, PassageRefKind, LinkKind } from '../hooks/hookTypes';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface CompletionContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class CompletionHandler {
  private ctx: CompletionContext;

  constructor(ctx: CompletionContext) {
    this.ctx = ctx;
  }

  /**
   * Handle a completion request.
   * Delegates to format module for macro/variable completions.
   * Uses WorkspaceIndex for passage completions (format-agnostic).
   */
  handleCompletion(params: { textDocument: { uri: string }; position: { line: number; character: number } }): CompletionItem[] {
    const items: CompletionItem[] = [];
    const uri = params.textDocument.uri;
    const doc = this.ctx.documentStore.get(uri);
    if (!doc) return items;

    const position = params.position;
    const offset = doc.offsetAt(position);
    const text = doc.getText();
    const line = text.substring(text.lastIndexOf('\n', offset - 1) + 1, offset);

    const format = this.ctx.formatRegistry.getActiveFormat();

    // Macro completion: triggered by format-specific macro trigger chars
    if (format.macros !== undefined) {
      const macroPrefix = format.macroDelimiters.open;
      if (macroPrefix && line.endsWith(macroPrefix)) {
        const macroSuffix = format.macroDelimiters.close;
        const closePrefix = format.macroDelimiters.closeTagPrefix ?? '';
        const closeSuffix = format.macroDelimiters.close;
        for (const macro of format.macros.builtins) {
          // Use MacroKind.Changer to determine if the macro needs a close tag/hook
          const isContainer = macro.kind === MacroKind.Changer;
          let insertText: string;
          if (isContainer && closePrefix) {
            // Close-tag style: name $1>><</name>> or format equivalent
            insertText = `${macro.name} \$1${macroSuffix}${closePrefix}${macro.name}${closeSuffix}`;
          } else {
            insertText = `${macro.name} \$0${macroSuffix}`;
          }
          items.push({
            label: macro.name,
            kind: CompletionItemKind.Function,
            detail: macro.category,
            documentation: macro.description,
            insertText,
            insertTextFormat: InsertTextFormat.Snippet,
            sortText: `0${macro.name}`,
            data: { type: 'macro', name: macro.name, formatId: format.formatId },
          });
        }
        return items;
      }
    }

    // Passage completion: triggered by [[ — always available (core provides this)
    if (line.endsWith('[[')) {
      for (const name of this.ctx.workspaceIndex.getAllPassageNames()) {
        items.push({
          label: name,
          kind: CompletionItemKind.Class,
          detail: 'Passage',
          insertText: name + ']]',
          sortText: `1${name}`,
          data: { type: 'passage', name },
        });
      }
      return items;
    }

    // Variable completion: triggered by format-specific variable trigger chars
    if (format.variables !== undefined) {
      const varTriggerChars = format.variables.triggerChars;
      const lastChar = line.length > 0 ? line[line.length - 1] : '';
      if (varTriggerChars.includes(lastChar)) {
        const allPassages = this.ctx.workspaceIndex.getAllPassages();
        const vars = new Set<string>();
        // Look up the sigil definition to determine scope
        const sigilDef = format.variables.sigils.find(s => s.sigil === lastChar);
        const scope = sigilDef?.kind ?? null;
        for (const p of allPassages) {
          if (scope === 'story') {
            p.storyVars.forEach(v => vars.add(v));
          } else if (scope === 'temp') {
            p.tempVars.forEach(v => vars.add(v));
          } else {
            // Unknown scope — show all variables
            p.storyVars.forEach(v => vars.add(v));
            p.tempVars.forEach(v => vars.add(v));
          }
        }
        for (const v of vars) {
          items.push({
            label: v,
            kind: CompletionItemKind.Variable,
            insertText: v,
            sortText: `2${v}`,
            data: { type: 'variable', name: v, sigil: lastChar, formatId: format.formatId },
          });
        }
        return items;
      }
    }

    return items;
  }

  /**
   * Resolve additional details for a completion item.
   * Delegates to format module for documentation.
   */
  handleCompletionResolve(item: CompletionItem): CompletionItem {
    const data = item.data as { type?: string; name?: string; sigil?: string; formatId?: string } | undefined;
    if (!data?.type || !data?.name) return item;

    const format = this.ctx.formatRegistry.getActiveFormat();

    if (data.type === 'macro' && format.macros) {
      const macro = format.macros.builtins.find(m => m.name === data.name);
      if (macro) {
        const prefix = format.macroDelimiters.open;
        const suffix = format.macroDelimiters.close;
        item.detail = macro.signatures
          .map(s => `${prefix}${macro.name} ${s.args.map(a => a.name).join(' ')}${suffix}`)
          .join(' | ');
        item.documentation = {
          kind: MarkupKind.Markdown,
          value: `**${prefix}${macro.name}${suffix}** — ${macro.category}\n\n${macro.description}${macro.deprecated ? `\n\n⚠ **Deprecated**: ${macro.deprecationMessage ?? ''}` : ''}`,
        };
      }
    } else if (data.type === 'passage') {
      const passage = this.ctx.workspaceIndex.getPassage(data.name);
      if (passage) {
        item.detail = `${passage.type}${passage.tags.length > 0 ? ` [${passage.tags.join(', ')}]` : ''}`;
        // Show first 3 lines of passage body as preview
        const preview = passage.body.split('\n').slice(0, 3).join('\n').trim();
        if (preview) {
          item.documentation = {
            kind: MarkupKind.PlainText,
            value: preview,
          };
        }
      }
    } else if (data.type === 'variable' && format.variables) {
      const sigilDef = format.variables.sigils.find(s => s.sigil === data.sigil);
      if (sigilDef) {
        item.detail = `${sigilDef.kind} variable`;
        item.documentation = sigilDef.description;
      }
    }

    return item;
  }
}
