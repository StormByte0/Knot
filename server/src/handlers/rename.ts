import {
  type Connection,
  Range,
  TextEdit,
  type RenameParams,
  type WorkspaceEdit,
  PrepareRenameResult,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { offsetToPosition, getFileText, normalizeUri } from '../serverUtils';
import { passageNameFromExpr } from '../passageArgs';
import type { ExpressionNode, MarkupNode } from '../ast';
import { walkMarkup, walkDocument } from '../visitors';

// ---------------------------------------------------------------------------

export function registerRenameHandlers(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  connection.onPrepareRename((params): PrepareRenameResult | null => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return null;
    const offset = doc.offsetAt(params.position);
    // Use cached AST from workspace index
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return null;

    const adapter = workspace.getActiveAdapter();
    const passageArgMacros = adapter.getPassageArgMacros();
    const macroDefMacros = adapter.getMacroDefinitionMacros();

    // ── Passage name in header ──────────────────────────────────────────────
    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        return Range.create(
          doc.positionAt(passage.nameRange.start),
          doc.positionAt(passage.nameRange.end),
        );
      }
    }

    // ── Walk body nodes ────────────────────────────────────────────────────
    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const found = findRenameTargetInNodes(passage.body, offset, doc, adapter, passageArgMacros, macroDefMacros, workspace);
      if (found) return found;
    }

    // ── Variable rename ($varName) ─────────────────────────────────────────
    const text = doc.getText();
    const storySigil = adapter.getVariableSigils().find(s => s.variableType === 'story');
    const storySigilChar = storySigil?.sigil ?? '$';
    const escaped = storySigilChar.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const word = text.slice(Math.max(0, offset - 100), offset).match(new RegExp(`${escaped}([A-Za-z_][A-Za-z0-9_]*)$`));
    if (word) {
      const varName = word[1]!;
      const varDef = workspace.getVariableDefinition(varName);
      if (varDef) {
        // Find the exact range of the variable at cursor
        const varStart = offset - varName.length;
        return Range.create(
          doc.positionAt(varStart),
          doc.positionAt(offset),
        );
      }
    }

    return null;
  });

  connection.onRenameRequest((params: RenameParams): WorkspaceEdit | null => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return null;
    const offset  = doc.offsetAt(params.position);
    // Use cached AST from workspace index
    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return null;

    const adapter = workspace.getActiveAdapter();
    const passageArgMacros = adapter.getPassageArgMacros();
    const macroDefMacros = adapter.getMacroDefinitionMacros();

    // ── Determine rename kind ──────────────────────────────────────────────

    // 1. Passage name in header
    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        return buildPassageRename(passage.name, params.newName, workspace, documents, adapter, passageArgMacros);
      }
    }

    // 2. Walk body for passage link/macro arg, variable, or widget
    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;

      // Check for passage link/macro arg
      const passageName = findPassageNameAtOffset(passage.body, offset, adapter, passageArgMacros);
      if (passageName) {
        return buildPassageRename(passageName, params.newName, workspace, documents, adapter, passageArgMacros);
      }

      // Check for widget name
      const widgetInfo = findWidgetNameAtOffset(passage.body, offset, macroDefMacros);
      if (widgetInfo) {
        return buildWidgetRename(widgetInfo.name, params.newName, workspace, documents);
      }
    }

    // 3. Variable rename
    const text = doc.getText();
    const storySigil = adapter.getVariableSigils().find(s => s.variableType === 'story');
    const storySigilChar = storySigil?.sigil ?? '$';
    const escapedSigil = storySigilChar.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const word = text.slice(Math.max(0, offset - 100), offset).match(new RegExp(`${escapedSigil}([A-Za-z_][A-Za-z0-9_]*)$`));
    if (word) {
      const varName = word[1]!;
      const varDef = workspace.getVariableDefinition(varName);
      if (varDef) {
        return buildVariableRename(varName, params.newName, workspace, documents);
      }
    }

    return null;
  });
}

// ---------------------------------------------------------------------------
// Passage rename
// ---------------------------------------------------------------------------

function buildPassageRename(
  oldName: string,
  newName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
  passageArgMacros: ReadonlySet<string>,
): WorkspaceEdit {
  const changes: Record<string, TextEdit[]> = {};

  addPassageDeclarationEdits(oldName, newName, changes, workspace, documents);

  for (const uri of workspace.getCachedUris()) {
    addPassageReferenceEdits(uri, oldName, newName, changes, documents, workspace, adapter, passageArgMacros);
  }

  return { changes };
}

function addPassageDeclarationEdits(
  oldName: string,
  newName: string,
  changes: Record<string, TextEdit[]>,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): void {
  const def = workspace.getPassageDefinition(oldName);
  if (!def) return;
  const fileText = getFileText(def.uri, documents);
  if (!fileText) return;
  // Use cached AST from workspace index
  const cached = workspace.getParsedFile(def.uri);
  const ast = cached?.ast;
  if (!ast) return;
  const edits: TextEdit[] = changes[def.uri] ?? [];
  for (const p of ast.passages) {
    if (p.name === oldName) {
      edits.push(TextEdit.replace(
        Range.create(
          offsetToPosition(fileText, p.nameRange.start),
          offsetToPosition(fileText, p.nameRange.end),
        ),
        newName,
      ));
    }
  }
  if (edits.length) changes[def.uri] = edits;
}

function addPassageReferenceEdits(
  uri: string,
  oldName: string,
  newName: string,
  changes: Record<string, TextEdit[]>,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
  passageArgMacros: ReadonlySet<string>,
): void {
  const fileText = getFileText(uri, documents);
  if (!fileText) return;
  // Use cached AST from workspace index
  const cached = workspace.getParsedFile(uri);
  const ast = cached?.ast;
  if (!ast) return;
  const edits: TextEdit[] = changes[uri] ?? [];

  walkDocument(ast, {
    onLink(node) {
      // [[OldName]] and variants
      if (node.target === oldName) {
        edits.push(TextEdit.replace(
          Range.create(
            offsetToPosition(fileText, node.targetRange.start),
            offsetToPosition(fileText, node.targetRange.end),
          ),
          newName,
        ));
      }
    },
    onMacro(node) {
      // <<goto "OldName">>, <<link "label" "OldName">>, etc.
      if (passageArgMacros.has(node.name) && node.args.length > 0) {
        const idx = adapter.getPassageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && isPassageLiteral(arg, oldName)) {
          // arg.range spans the whole quoted string including the quote chars,
          // e.g. `"OldName"`. We shrink by 1 each side to replace only the name.
          edits.push(TextEdit.replace(
            Range.create(
              offsetToPosition(fileText, arg.range.start + 1),
              offsetToPosition(fileText, arg.range.end - 1),
            ),
            newName,
          ));
        }
      }
    },
  });

  if (edits.length) changes[uri] = edits;
}

// ---------------------------------------------------------------------------
// Variable rename
// ---------------------------------------------------------------------------

function buildVariableRename(
  oldName: string,
  newName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): WorkspaceEdit {
  const changes: Record<string, TextEdit[]> = {};

  // Rename the definition site
  const varDef = workspace.getVariableDefinition(oldName);
  if (varDef) {
    const fileText = getFileText(varDef.uri, documents);
    if (fileText) {
      const edits: TextEdit[] = changes[varDef.uri] ?? [];
      edits.push(TextEdit.replace(
        Range.create(
          offsetToPosition(fileText, varDef.range.start),
          offsetToPosition(fileText, varDef.range.end),
        ),
        newName,
      ));
      if (edits.length) changes[varDef.uri] = edits;
    }
  }

  // Rename all references
  const refs = workspace.getVariableReferences(oldName);
  for (const ref of refs) {
    const fileText = getFileText(ref.uri, documents);
    if (!fileText) continue;
    const edits: TextEdit[] = changes[ref.uri] ?? [];
    edits.push(TextEdit.replace(
      Range.create(
        offsetToPosition(fileText, ref.range.start),
        offsetToPosition(fileText, ref.range.end),
      ),
      newName,
    ));
    if (edits.length) changes[ref.uri] = edits;
  }

  return { changes };
}

// ---------------------------------------------------------------------------
// Widget rename
// ---------------------------------------------------------------------------

function buildWidgetRename(
  oldName: string,
  newName: string,
  workspace: WorkspaceIndex,
  documents: TextDocuments<TextDocument>,
): WorkspaceEdit {
  const changes: Record<string, TextEdit[]> = {};

  // Rename the definition site
  const macroDef = workspace.getMacroDefinition(oldName);
  if (macroDef) {
    const fileText = getFileText(macroDef.uri, documents);
    if (fileText) {
      const edits: TextEdit[] = changes[macroDef.uri] ?? [];
      edits.push(TextEdit.replace(
        Range.create(
          offsetToPosition(fileText, macroDef.range.start),
          offsetToPosition(fileText, macroDef.range.end),
        ),
        newName,
      ));
      if (edits.length) changes[macroDef.uri] = edits;
    }
  }

  // Rename all call sites
  const callSites = workspace.getMacroCallSites(oldName);
  for (const site of callSites) {
    const fileText = getFileText(site.uri, documents);
    if (!fileText) continue;
    const edits: TextEdit[] = changes[site.uri] ?? [];
    edits.push(TextEdit.replace(
      Range.create(
        offsetToPosition(fileText, site.range.start),
        offsetToPosition(fileText, site.range.end),
      ),
      newName,
    ));
    if (edits.length) changes[site.uri] = edits;
  }

  return { changes };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function isPassageLiteral(expr: ExpressionNode, name: string): boolean {
  return expr.type === 'literal' && expr.kind === 'string' && expr.value === name;
}

// ---------------------------------------------------------------------------
// Prepare-rename: find the renameable range at cursor
// ---------------------------------------------------------------------------

type RenameTarget = { kind: 'passage'; range: Range } | { kind: 'variable'; range: Range } | { kind: 'widget'; range: Range };

function findRenameTargetInNodes(
  nodes: MarkupNode[],
  offset: number,
  doc: TextDocument,
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
  passageArgMacros: ReadonlySet<string>,
  macroDefMacros: ReadonlySet<string>,
  workspace: WorkspaceIndex,
): Range | null {
  let result: Range | null = null;

  walkMarkup(nodes, {
    onLink(node) {
      // [[Target]]
      if (offset >= node.range.start && offset <= node.range.end) {
        result = Range.create(
          doc.positionAt(node.targetRange.start),
          doc.positionAt(node.targetRange.end),
        );
        if (result) return false;
      }
    },
    onMacro(node) {
      // Passage arg in macro
      if (passageArgMacros.has(node.name) && node.args.length > 0) {
        const idx = adapter.getPassageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          // Highlight inside the quotes only
          result = Range.create(
            doc.positionAt(arg.range.start + 1),
            doc.positionAt(arg.range.end - 1),
          );
          if (result) return false;
        }
      }

      // Widget name in definition macro (<<widget "name">>)
      if (macroDefMacros.has(node.name) && node.args.length > 0) {
        const arg = node.args[0];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          if (arg.type === 'literal' && arg.kind === 'string') {
            // Inside the string literal
            result = Range.create(
              doc.positionAt(arg.range.start + 1),
              doc.positionAt(arg.range.end - 1),
            );
            if (result) return false;
          }
        }
      }

      // Widget/macro call site name
      if (offset >= node.nameRange.start && offset <= node.nameRange.end) {
        const macroDef = workspace.getMacroDefinition(node.name);
        if (macroDef) {
          result = Range.create(
            doc.positionAt(node.nameRange.start),
            doc.positionAt(node.nameRange.end),
          );
          if (result) return false;
        }
      }
    },
  });

  return result;
}

// ---------------------------------------------------------------------------
// Rename request: extract the passage name at cursor
// ---------------------------------------------------------------------------

function findPassageNameAtOffset(
  nodes: MarkupNode[],
  offset: number,
  adapter: ReturnType<WorkspaceIndex['getActiveAdapter']>,
  passageArgMacros: ReadonlySet<string>,
): string | null {
  let result: string | null = null;

  walkMarkup(nodes, {
    onLink(node) {
      if (offset >= node.range.start && offset <= node.range.end) {
        result = node.target;
        return false;
      }
    },
    onMacro(node) {
      if (passageArgMacros.has(node.name) && node.args.length > 0) {
        const idx = adapter.getPassageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          if (arg.type === 'literal' && arg.kind === 'string') {
            result = String(arg.value);
            return false;
          }
        }
      }
    },
  });

  return result;
}

function findWidgetNameAtOffset(
  nodes: MarkupNode[],
  offset: number,
  macroDefMacros: ReadonlySet<string>,
): { name: string } | null {
  let result: { name: string } | null = null;

  walkMarkup(nodes, {
    onMacro(node) {
      if (macroDefMacros.has(node.name) && node.args.length > 0) {
        const arg = node.args[0];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          if (arg.type === 'literal' && arg.kind === 'string') {
            result = { name: String(arg.value) };
            return false;
          }
        }
      }
    },
  });

  return result;
}
