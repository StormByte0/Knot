/**
 * Knot v2 — Code Action Handler
 *
 * Format-agnostic code action handler.
 * Provides quick fixes based on diagnostics and format capabilities.
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  CodeAction,
  CodeActionKind,
  TextEdit,
} from 'vscode-languageserver/node';

import { FormatRegistry } from '../formats/formatRegistry';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';

export interface CodeActionContext {
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
}

export class CodeActionHandler {
  private ctx: CodeActionContext;

  constructor(ctx: CodeActionContext) {
    this.ctx = ctx;
  }

  handleCodeAction(params: {
    textDocument: { uri: string };
    range: { start: { line: number; character: number }; end: { line: number; character: number } };
    context: { diagnostics: { message: string; range: { start: { line: number; character: number }; end: { line: number; character: number } }; code?: string | number }[] };
  }): CodeAction[] {
    const actions: CodeAction[] = [];
    const uri = params.textDocument.uri;
    const format = this.ctx.formatRegistry.getActiveFormat();

    for (const diag of params.context.diagnostics) {
      // Quick fix: create unknown passage
      if (diag.message.startsWith('Unknown passage:')) {
        const passageName = diag.message.match(/"([^"]+)"/)?.[1];
        if (passageName) {
          actions.push(CodeAction.create(
            `Create passage "${passageName}"`,
            {
              changes: {
                [uri]: [TextEdit.insert(
                  { line: 0, character: 0 },
                  `:: ${passageName}\n\n`,
                )],
              },
            },
            CodeActionKind.QuickFix,
          ));
        }
      }

      // Quick fix: deprecated macro → replace with alternative
      if (diag.message.startsWith('Deprecated macro:')) {
        // Try to extract the suggested replacement from the deprecation message
        // Pattern: "Use <<name>> instead of <<old>>" or "Use <<name>> instead"
        const replacementMatch = diag.message.match(/Use\s+<<(\w+)>>/i);
        const macroNameMatch = diag.message.match(/Deprecated macro:\s*"(\w+)"/);
        if (replacementMatch && macroNameMatch) {
          const oldName = macroNameMatch[1];
          const newName = replacementMatch[1];
          actions.push(CodeAction.create(
            `Replace <<${oldName}>> with <<${newName}>>`,
            {
              changes: {
                [uri]: [TextEdit.replace(
                  diag.range,
                  `<<${newName}>>`,
                )],
              },
            },
            CodeActionKind.QuickFix,
          ));
        }
      }

      // Quick fix: unknown macro → create widget passage (SugarCube)
      if (diag.message.startsWith('Unknown macro:') && format.customMacros) {
        const macroName = diag.message.match(/"(\w+)"/)?.[1];
        if (macroName) {
          for (const defMacro of format.customMacros.definitionMacros) {
            // e.g. 'widget' for SugarCube
            actions.push(CodeAction.create(
              `Create ${defMacro} "${macroName}"`,
              {
                changes: {
                  [uri]: [TextEdit.insert(
                    { line: 0, character: 0 },
                    `:: ${defMacro}-${macroName} [${defMacro}]\n<<${defMacro} "${macroName}">>\n$0\n<</${defMacro}>>\n\n`,
                  )],
                },
              },
              CodeActionKind.QuickFix,
            ));
          }
        }
      }

      // Quick fix: duplicate passage → rename (refactor action)
      if (diag.message.startsWith('Duplicate passage name:')) {
        const passageName = diag.message.match(/"([^"]+)"/)?.[1];
        if (passageName) {
          actions.push(CodeAction.create(
            `Rename duplicate passage "${passageName}"`,
            // This is a refactor — the actual rename is handled by the rename handler
            // We just surface the action to trigger it
            CodeActionKind.Refactor,
          ));
        }
      }
    }

    return actions;
  }
}
