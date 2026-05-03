import * as path from 'node:path';
import {
  type Connection,
  DocumentSymbol,
  WorkspaceSymbol,
  Range,
  SymbolKind as LspSymbolKind,
  type DocumentSymbolParams,
  type WorkspaceSymbolParams,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { TextDocuments } from 'vscode-languageserver/node';
import { WorkspaceIndex } from '../workspaceIndex';
import { SymbolKind } from '../symbols';
import { uriToPath, offsetToPosition, normalizeUri, getFileText } from '../serverUtils';
import type { ExpressionNode, StoryVarNode, MarkupNode } from '../ast';
import { walkMarkup } from '../visitors';

export function registerSymbolHandlers(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
  getWorkspaceFolderPath: () => string | undefined,
): void {
  // ── Document symbols (Outline panel) ───────────────────────────────────────
  connection.onDocumentSymbol((params: DocumentSymbolParams): DocumentSymbol[] => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];
    // Use cached AST from workspace index
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast || !ast.passages.length) return [];

    const adapter = workspace.getActiveAdapter();
    const varAssignmentMacros = adapter.getVariableAssignmentMacros();
    const macroDefMacros = adapter.getMacroDefinitionMacros();

    const results: DocumentSymbol[] = [];
    for (const passage of ast.passages) {
      // Guard: passage name must be non-empty for VS Code to accept the symbol
      if (!passage.name) continue;

      const children: DocumentSymbol[] = [];
      if (Array.isArray(passage.body)) {
        collectDocSymbols(passage.body, doc, children, varAssignmentMacros, macroDefMacros, adapter);
      }

      results.push({
        name: passage.name,
        kind: LspSymbolKind.Module,
        range: Range.create(doc.positionAt(passage.range.start), doc.positionAt(passage.range.end)),
        selectionRange: Range.create(doc.positionAt(passage.nameRange.start), doc.positionAt(passage.nameRange.end)),
        children,
      });
    }
    return results;
  });

  // ── Workspace symbols (Ctrl+T) ─────────────────────────────────────────────
  connection.onWorkspaceSymbol((params: WorkspaceSymbolParams): WorkspaceSymbol[] => {
    const query = params.query.toLowerCase();
    const seen  = new Set<string>();
    const results: WorkspaceSymbol[] = [];
    const workspaceFolderPath = getWorkspaceFolderPath();

    for (const uri of workspace.getCachedUris()) {
      if (workspaceFolderPath) {
        const filePath = uriToPath(uri);
        if (!filePath.startsWith(workspaceFolderPath)) continue;
        if (filePath.includes(path.sep + 'test' + path.sep + 'fixtures')) continue;
        if (filePath.includes('/test/fixtures')) continue;
      }

      const analysis = workspace.getAnalysis(uri);
      if (!analysis) continue;

      for (const sym of analysis.symbols.getUserSymbols()) {
        if (
          sym.kind !== SymbolKind.Passage &&
          sym.kind !== SymbolKind.Widget  &&
          sym.kind !== SymbolKind.Macro   &&
          sym.kind !== SymbolKind.StoryVar
        ) continue;

        const displayName = sym.kind === SymbolKind.StoryVar ? `$${sym.name}` : sym.name;

        // Guard: name must be non-empty
        if (!displayName) continue;

        if (query && !displayName.toLowerCase().includes(query)) continue;

        const key = `${sym.kind}:${sym.name}`;
        if (seen.has(key)) continue;
        seen.add(key);

        const lspKind =
          sym.kind === SymbolKind.Passage  ? LspSymbolKind.Module
          : sym.kind === SymbolKind.Widget ? LspSymbolKind.Function
          : sym.kind === SymbolKind.Macro  ? LspSymbolKind.Function
          : LspSymbolKind.Variable;

        let symRange = Range.create(0, 0, 0, 0);
        const openDoc = documents.get(sym.uri);
        if (openDoc) {
          symRange = Range.create(openDoc.positionAt(sym.range.start), openDoc.positionAt(sym.range.end));
        } else {
          // Use getFileText which checks workspace cache before disk
          const ft = getFileText(sym.uri, documents);
          if (ft) {
            symRange = Range.create(offsetToPosition(ft, sym.range.start), offsetToPosition(ft, sym.range.end));
          }
        }

        results.push({
          name: displayName,
          kind: lspKind,
          location: { uri: sym.uri, range: symRange },
        });
      }
    }

    return results;
  });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function collectDocSymbols(
  nodes: MarkupNode[],
  doc: TextDocument,
  out: DocumentSymbol[],
  varAssignmentMacros: ReadonlySet<string>,
  macroDefMacros: ReadonlySet<string>,
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
): void {
  walkMarkup(nodes, {
    onMacro(node) {
      if (varAssignmentMacros.has(node.name)) {
        const arg = node.args[0];
        if (arg?.type === 'binaryOp' && adapter.getAssignmentOperators().includes(arg.operator)) {
          const varNode = extractLeafStoryVar(arg.left);
          // Guard: variable name must be non-empty
          if (varNode && varNode.name) {
            out.push({
              name: `$${varNode.name}`,
              kind: LspSymbolKind.Variable,
              range: Range.create(doc.positionAt(node.range.start), doc.positionAt(node.range.end)),
              selectionRange: Range.create(doc.positionAt(varNode.range.start), doc.positionAt(varNode.range.end)),
            });
          }
        }
      }

      if (macroDefMacros.has(node.name)) {
        const arg = node.args[0];
        const widgetName =
          arg?.type === 'literal' && arg.kind === 'string' ? String(arg.value)
          : arg?.type === 'identifier' ? arg.name : null;
        // Guard: widget name must be non-empty
        if (widgetName && widgetName.trim() && arg) {
          out.push({
            name: `<<${widgetName}>>`,
            kind: LspSymbolKind.Function,
            range: Range.create(doc.positionAt(node.range.start), doc.positionAt(node.range.end)),
            selectionRange: Range.create(doc.positionAt(arg.range.start), doc.positionAt(arg.range.end)),
          });
        }
      }
    },
  });
}

function extractLeafStoryVar(expr: ExpressionNode): StoryVarNode | null {
  if (expr.type === 'storyVar')       return expr;
  if (expr.type === 'propertyAccess') return extractLeafStoryVar(expr.object);
  if (expr.type === 'indexAccess')    return extractLeafStoryVar(expr.object);
  return null;
}
