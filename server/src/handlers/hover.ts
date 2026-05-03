import * as path from 'node:path';
import {
  type Connection,
  Range,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import {
  wordAt,
  findWordStart,
  resolveTypePath,
  buildTypeSection,
  inferredTypeToString,
  normalizeUri,
} from '../serverUtils';
import type { DocumentNode, ExpressionNode, MarkupNode } from '../ast';
import type { SourceRange } from '../tokenTypes';
import { FormatRegistry } from '../formats/registry';
import { walkMarkup, walkExpression } from '../visitors';

export function registerHoverHandler(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  connection.onHover(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return null;
    const analysis = workspace.getAnalysis(normalizeUri(doc.uri));
    if (!analysis) return null;

    const offset  = doc.offsetAt(params.position);
    const text    = doc.getText();
    // Use cached AST from workspace index instead of re-parsing
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return null;
    const adapter = FormatRegistry.resolve(workspace.getActiveFormatId());
    const ctx     = { formatId: adapter.id, passageNames: workspace.getPassageNames() };

    // ── Property path hover ──────────────────────────────────────────────────
    const pathHover = resolvePropertyPathHover(ast, offset, workspace);
    if (pathHover) {
      return {
        contents: { kind: 'markdown', value: pathHover.content },
        range: Range.create(doc.positionAt(pathHover.start), doc.positionAt(pathHover.end)),
      };
    }

    // ── Semantic token hover ─────────────────────────────────────────────────
    for (const tok of analysis.semanticTokens) {
      if (offset < tok.range.start || offset > tok.range.end) continue;
      const rawName = text.slice(tok.range.start, tok.range.end);
      const content = buildHoverContent(rawName, tok.tokenType, workspace, adapter, ctx);
      if (!content) continue;
      return {
        contents: { kind: 'markdown', value: content },
        range: Range.create(doc.positionAt(tok.range.start), doc.positionAt(tok.range.end)),
      };
    }

    // ── JS global hover ──────────────────────────────────────────────────────
    const rawWord = wordAt(text, offset);
    if (rawWord) {
      const jsDef = workspace.getJsGlobalDefinition(rawWord);
      if (jsDef && jsDef.inferredType.kind !== 'unknown') {
        const wordStart = findWordStart(text, offset, rawWord);
        if (wordStart !== -1) {
          return {
            contents: { kind: 'markdown', value: `**JS** \`${rawWord}\`${buildTypeSection(jsDef.inferredType)}` },
            range: Range.create(doc.positionAt(wordStart), doc.positionAt(wordStart + rawWord.length)),
          };
        }
      }

      const builtinHover = adapter.provideBuiltinHover({ tokenType: 'function', rawName: rawWord }, ctx);
      if (builtinHover) {
        const wordStart = findWordStart(text, offset, rawWord);
        if (wordStart !== -1) {
          return {
            contents: { kind: 'markdown', value: builtinHover },
            range: Range.create(doc.positionAt(wordStart), doc.positionAt(wordStart + rawWord.length)),
          };
        }
      }
    }

    return null;
  });
}

// ---------------------------------------------------------------------------
// Hover content builder
// ---------------------------------------------------------------------------

type HoverAdapter = ReturnType<typeof FormatRegistry.resolve>;
type HoverCtx     = Parameters<HoverAdapter['provideBuiltinHover']>[1];

function buildHoverContent(
  rawName: string,
  tokenType: string,
  workspace: WorkspaceIndex,
  adapter: HoverAdapter,
  ctx: HoverCtx,
): string | null {

  // ── Macro ──────────────────────────────────────────────────────────────────
  if (tokenType === 'macro') {
    const adapterHover = adapter.provideBuiltinHover({ tokenType, rawName }, ctx);
    if (adapterHover) return adapterHover;

    const macroDef = workspace.getMacroDefinition(rawName);
    if (macroDef) {
      return `**Custom Macro** \`<<${rawName}>>\`\n\n*Registered in* \`${path.basename(macroDef.uri)}\`` +
        (macroDef.passageName ? ` in \`${macroDef.passageName}\`` : '');
    }

    return `**Macro** \`<<${rawName}>>\``;
  }

  // ── Variable ──────────────────────────────────────────────────────────────
  if (tokenType === 'variable') {
    const sigil     = rawName[0] ?? '';
    const sigilDesc = adapter.describeVariableSigil(sigil);
    const sigilInfo = adapter.getVariableSigils().find(s => s.sigil === sigil);

    if (sigilInfo?.variableType === 'story') {
      const varName = rawName.slice(sigil.length);
      const def = workspace.getVariableDefinition(varName);
      if (def) {
        const passage   = def.passageName ? `\`${def.passageName}\`` : `\`${path.basename(def.uri)}\``;
        const refs      = workspace.getVariableReferences(varName);
        const fileCount = new Set(refs.map(r => r.uri)).size;
        const refLine   = refs.length > 0
          ? `\n\n*Referenced ${refs.length} time${refs.length === 1 ? '' : 's'} across ${fileCount} file${fileCount === 1 ? '' : 's'}*`
          : '';
        const sigilNote = sigilDesc ? `\n\n*${sigilDesc}*` : '';
        return `**StoryVar** \`${sigil}${varName}\`${sigilNote}\n\n*Defined in* ${passage}${refLine}` +
          (def.inferredType ? buildTypeSection(def.inferredType) : '');
      }
      const sigilNote = sigilDesc ? `\n\n*${sigilDesc}*` : '';
      return `**StoryVar** \`${sigil}${varName}\`${sigilNote}`;
    }

    if (sigilInfo?.variableType === 'temporary') {
      const tempDesc = sigilDesc ?? 'Temporary variable (passage-scoped)';
      return `**TempVar** \`${rawName}\`\n\n*${tempDesc}*`;
    }

    return null;
  }

  // ── Passage ───────────────────────────────────────────────────────────────
  if (tokenType === 'passage') {
    const def      = workspace.getPassageDefinition(rawName);
    const incoming = workspace.getIncomingLinks(rawName);
    const incomingLine = buildIncomingLine(incoming);

    if (def) {
      return `**Passage** \`${rawName}\`\n\n*Defined in* \`${path.basename(def.uri)}\`${incomingLine}`;
    }
    return `**Passage** \`${rawName}\`${incomingLine}`;
  }

  return null;
}

// ---------------------------------------------------------------------------
// Incoming links formatter
//
// Works with both old shape {sourcePassage, uri} and new shape
// {sourcePassage, uri, count}.  Deduplicates by sourcePassage in the hover
// layer as a safety net, and shows ×N badge when count > 1.
// ---------------------------------------------------------------------------

function buildIncomingLine(incoming: Array<{ sourcePassage: string; uri: string; count?: number }>): string {
  if (incoming.length === 0) return '\n\n*No incoming links*';

  const grouped = new Map<string, number>();
  for (const l of incoming) {
    grouped.set(l.sourcePassage, (grouped.get(l.sourcePassage) ?? 0) + (l.count ?? 1));
  }

  const parts = [...grouped.entries()].map(([name, count]) =>
    count > 1 ? `\`${name}\` (×${count})` : `\`${name}\``,
  );

  return `\n\n**Linked from:** ${parts.join(', ')}`;
}

// ---------------------------------------------------------------------------
// Property path hover
// ---------------------------------------------------------------------------

function resolvePropertyPathHover(
  ast: DocumentNode,
  offset: number,
  workspace: WorkspaceIndex,
): { content: string; start: number; end: number } | null {
  for (const passage of ast.passages) {
    if (!Array.isArray(passage.body)) continue;
    const result = searchNodesForPath(passage.body, offset, workspace);
    if (result) return result;
  }
  return null;
}

function searchNodesForPath(
  nodes: MarkupNode[],
  offset: number,
  workspace: WorkspaceIndex,
): { content: string; start: number; end: number } | null {
  let result: { content: string; start: number; end: number } | null = null;

  walkMarkup(nodes, {
    onMacro(node) {
      for (const arg of node.args) {
        const r = searchExprForPath(arg, offset, workspace);
        if (r) {
          result = r;
          return false; // early termination
        }
      }
    },
  });

  return result;
}

type PropertyChain = { root: string; path: Array<{ key: string; range: SourceRange }> };

function flattenPropertyAccess(expr: ExpressionNode): PropertyChain | null {
  if (expr.type === 'storyVar') return { root: expr.name, path: [] };
  if (expr.type === 'propertyAccess') {
    const base = flattenPropertyAccess(expr.object);
    if (!base) return null;
    base.path.push({ key: expr.property, range: expr.propertyRange });
    return base;
  }
  return null;
}

function searchExprForPath(
  expr: ExpressionNode,
  offset: number,
  workspace: WorkspaceIndex,
): { content: string; start: number; end: number } | null {
  if (expr.type === 'propertyAccess') {
    if (offset >= expr.propertyRange.start && offset <= expr.propertyRange.end) {
      const chain = flattenPropertyAccess(expr);
      if (chain) {
        const varDef = workspace.getVariableDefinition(chain.root);
        if (varDef?.inferredType) {
          const parentPath = chain.path.slice(0, -1).map(s => s.key);
          const thisType   = resolveTypePath(varDef.inferredType, chain.path.map(s => s.key));
          const parentType = resolveTypePath(varDef.inferredType, parentPath);
          const pathStr    = `$${chain.root}.${chain.path.map(s => s.key).join('.')}`;
          if (thisType) {
            return {
              content: `**StoryVar path** \`${pathStr}\`${buildTypeSection(thisType)}`,
              start: expr.propertyRange.start,
              end:   expr.propertyRange.end,
            };
          }
          if (parentType?.kind === 'object' && parentType.properties) {
            return {
              content: `**StoryVar path** \`${pathStr}\`\n\n*Unknown property* — parent has: \`{ ${Object.keys(parentType.properties).join(', ')} }\``,
              start: expr.propertyRange.start,
              end:   expr.propertyRange.end,
            };
          }
        }
      }
    }
    return searchExprForPath(expr.object, offset, workspace);
  }

  switch (expr.type) {
    case 'binaryOp':
      return searchExprForPath(expr.left, offset, workspace) ?? searchExprForPath(expr.right, offset, workspace);
    case 'unaryOp':
      return searchExprForPath(expr.operand, offset, workspace);
    case 'indexAccess':
      return searchExprForPath(expr.object, offset, workspace) ?? searchExprForPath(expr.index, offset, workspace);
    case 'call':
      return searchExprForPath(expr.callee, offset, workspace) ??
        expr.args.reduce<ReturnType<typeof searchExprForPath>>((a, e) => a ?? searchExprForPath(e, offset, workspace), null);
    case 'arrayLiteral':
      return expr.elements.reduce<ReturnType<typeof searchExprForPath>>((a, e) => a ?? searchExprForPath(e, offset, workspace), null);
    case 'objectLiteral':
      return expr.properties.reduce<ReturnType<typeof searchExprForPath>>((a, p) => a ?? searchExprForPath(p.value, offset, workspace), null);
    default:
      return null;
  }
}
