import {
  type Connection,
  Location,
  Range,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { wordAt, defToLocation, refToLocation, offsetToPosition, getFileText, normalizeUri } from '../serverUtils';
import type { MarkupNode, ExpressionNode } from '../ast';
import type { SourceRange } from '../tokenTypes';
import { PASSAGE_ARG_MACROS, passageArgIndex, passageNameFromExpr } from '../passageArgs';

export function registerDefinitionHandlers(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  // ── Go-to-definition ───────────────────────────────────────────────────────
  connection.onDefinition(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return null;
    const offset  = doc.offsetAt(params.position);
    const text    = doc.getText();
    // Use cached AST from workspace index
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return null;

    // Passage header
    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        return locationFor(passage.name, workspace, documents);
      }
    }

    // Links and macro args
    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const result = findDefinitionInNodes(passage.body, offset, workspace, documents);
      if (result) return result;
    }

    // Word-based fallback
    const word = wordAt(text, offset);
    if (!word) return null;

    const passageDef  = workspace.getPassageDefinition(word);
    if (passageDef)  return defToLocation(passageDef, documents);

    const jsGlobalDef = workspace.getJsGlobalDefinition(word);
    if (jsGlobalDef) return defToLocation(jsGlobalDef, documents);

    const macroDef    = workspace.getMacroDefinition(word);
    if (macroDef)    return defToLocation(macroDef, documents);

    if (word.startsWith('$')) {
      const varDef = workspace.getVariableDefinition(word.slice(1));
      if (varDef)  return defToLocation(varDef, documents);
    }

    return null;
  });

  // ── Find references ────────────────────────────────────────────────────────
  connection.onReferences(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];
    const offset  = doc.offsetAt(params.position);
    const text    = doc.getText();
    // Use cached AST from workspace index
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return [];

    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        return refsForPassage(passage.name, workspace, documents);
      }
    }

    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const hit = resolveRefSymbolInNodes(passage.body, offset, workspace);
      if (!hit) continue;
      if (hit.kind === 'passage')  return refsForPassage(hit.name, workspace, documents);
      if (hit.kind === 'storyVar') return refsForStoryVar(hit.name, workspace, documents);
      if (hit.kind === 'macro')    return refsForMacro(hit.name, workspace, documents);
    }

    return [];
  });
}

// ---------------------------------------------------------------------------
// Definition helpers
// ---------------------------------------------------------------------------

function locationFor(
  passageName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): Location | null {
  const def = workspace.getPassageDefinition(passageName);
  return def ? defToLocation(def, documents) : null;
}

function findDefinitionInNodes(
  nodes: MarkupNode[],
  offset: number,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): Location | null {
  for (const node of nodes) {
    // [[Target]]
    if (node.type === 'link' && offset >= node.range.start && offset <= node.range.end) {
      return locationFor(node.target, workspace, documents);
    }

    if (node.type === 'macro') {
      // Macro name → go to macro definition
      if (offset >= node.nameRange.start && offset <= node.nameRange.end) {
        const def = workspace.getMacroDefinition(node.name);
        if (def) return defToLocation(def, documents);
      }

      // Passage arg → go to passage definition
      if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
        const idx = passageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          const name = passageNameFromExpr(arg);
          if (name) return locationFor(name, workspace, documents);
        }
      }

      if (node.body) {
        const result = findDefinitionInNodes(node.body, offset, workspace, documents);
        if (result) return result;
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Reference helpers
// ---------------------------------------------------------------------------

type RefSymbol =
  | { kind: 'passage';  name: string }
  | { kind: 'storyVar'; name: string }
  | { kind: 'macro';    name: string };

function resolveRefSymbolInNodes(
  nodes: MarkupNode[],
  offset: number,
  workspace: WorkspaceIndex,
): RefSymbol | null {
  for (const node of nodes) {
    // [[link]] target
    if (node.type === 'link') {
      if (offset >= node.targetRange.start && offset <= node.targetRange.end) {
        return { kind: 'passage', name: node.target };
      }
      continue;
    }

    if (node.type === 'macro') {
      // Macro name
      if (offset >= node.nameRange.start && offset <= node.nameRange.end) {
        if (workspace.getMacroDefinition(node.name)) return { kind: 'macro', name: node.name };
        return null;
      }

      // Passage arg in macro
      if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
        const idx = passageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          const name = passageNameFromExpr(arg);
          if (name) return { kind: 'passage', name };
        }
      }

      // Story var in other args
      for (const arg of node.args) {
        const varHit = resolveStoryVarInExpr(arg, offset);
        if (varHit) return { kind: 'storyVar', name: varHit };
      }

      if (node.body) {
        const bodyHit = resolveRefSymbolInNodes(node.body, offset, workspace);
        if (bodyHit) return bodyHit;
      }
    }
  }
  return null;
}

function resolveStoryVarInExpr(expr: ExpressionNode, offset: number): string | null {
  if (expr.type === 'storyVar') {
    return (offset >= expr.range.start && offset <= expr.range.end) ? expr.name : null;
  }
  if (expr.type === 'propertyAccess') return resolveStoryVarInExpr(expr.object, offset);
  if (expr.type === 'indexAccess') {
    return resolveStoryVarInExpr(expr.object, offset) ?? resolveStoryVarInExpr(expr.index, offset);
  }
  if (expr.type === 'binaryOp') {
    return resolveStoryVarInExpr(expr.left, offset) ?? resolveStoryVarInExpr(expr.right, offset);
  }
  if (expr.type === 'unaryOp') return resolveStoryVarInExpr(expr.operand, offset);
  if (expr.type === 'call') {
    return resolveStoryVarInExpr(expr.callee, offset) ??
      expr.args.reduce<string | null>((a, e) => a ?? resolveStoryVarInExpr(e, offset), null);
  }
  if (expr.type === 'arrayLiteral') {
    return expr.elements.reduce<string | null>((a, e) => a ?? resolveStoryVarInExpr(e, offset), null);
  }
  if (expr.type === 'objectLiteral') {
    return expr.properties.reduce<string | null>((a, p) => a ?? resolveStoryVarInExpr(p.value, offset), null);
  }
  return null;
}

function refsForPassage(
  passageName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): Location[] {
  return workspace.getReferencingFiles(passageName).flatMap(uri =>
    collectPassageRefRanges(uri, passageName, documents, workspace).map(r => Location.create(uri, r)),
  );
}

function refsForStoryVar(
  varName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): Location[] {
  return workspace.getVariableReferences(varName).map(ref => refToLocation(ref, documents));
}

function refsForMacro(
  macroName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): Location[] {
  return workspace.getMacroCallSites(macroName).map(ref => refToLocation(ref, documents));
}

/**
 * Collect all ranges in a file that reference passageName —
 * both [[link]] nodes and passage-arg macro calls.
 */
function collectPassageRefRanges(
  uri: string,
  passageName: string,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): Range[] {
  const fileText = getFileText(uri, documents);
  if (!fileText) return [];
  // Use cached AST from workspace index instead of re-parsing
  const cached = workspace.getParsedFile(uri);
  const ast = cached?.ast;
  if (!ast) return [];
  const ranges: Range[] = [];

  const walk = (nodes: MarkupNode[]): void => {
    for (const node of nodes) {
      // [[Target]]
      if (node.type === 'link' && node.target === passageName) {
        ranges.push(Range.create(
          offsetToPosition(fileText, node.targetRange.start),
          offsetToPosition(fileText, node.targetRange.end),
        ));
      }

      if (node.type === 'macro') {
        // <<goto "Target">>, <<link "label" "Target">>, etc.
        if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
          const idx  = passageArgIndex(node.name, node.args.length);
          const arg  = node.args[idx];
          const name = arg ? passageNameFromExpr(arg) : null;
          if (name === passageName && arg) {
            // Range inside the quotes (same as rename logic)
            ranges.push(Range.create(
              offsetToPosition(fileText, arg.range.start + 1),
              offsetToPosition(fileText, arg.range.end - 1),
            ));
          }
        }
        if (node.body) walk(node.body);
      }
    }
  };

  for (const p of ast.passages) {
    if (Array.isArray(p.body)) walk(p.body);
  }
  return ranges;
}