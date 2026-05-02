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
import { parseDocument } from '../parser';
import { offsetToPosition, getFileText } from '../serverUtils';
import type { ExpressionNode, MarkupNode } from '../ast';

// ---------------------------------------------------------------------------
// Macros whose arguments contain a passage name target.
// Must stay in sync with the same set in workspaceIndex.ts / symbols.ts.
// ---------------------------------------------------------------------------

const PASSAGE_ARG_MACROS = new Set([
  'link', 'button', 'linkappend', 'linkprepend', 'linkreplace',
  'include', 'display', 'goto', 'actions', 'click',
]);

const LABEL_THEN_PASSAGE = new Set([
  'link', 'button', 'click', 'linkappend', 'linkprepend', 'linkreplace',
]);

function passageArgIndex(macroName: string, argCount: number): number {
  return (LABEL_THEN_PASSAGE.has(macroName) && argCount >= 2) ? 1 : 0;
}

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
    const { ast } = parseDocument(doc.getText());

    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        return Range.create(
          doc.positionAt(passage.nameRange.start),
          doc.positionAt(passage.nameRange.end),
        );
      }
    }
    for (const passage of ast.passages) {
      if (!Array.isArray(passage.body)) continue;
      const found = findRenameRangeInNodes(passage.body, offset, doc);
      if (found) return found;
    }
    return null;
  });

  connection.onRenameRequest((params: RenameParams): WorkspaceEdit | null => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return null;
    const offset  = doc.offsetAt(params.position);
    const { ast } = parseDocument(doc.getText());

    let passageName: string | null = null;

    for (const passage of ast.passages) {
      if (offset >= passage.nameRange.start && offset <= passage.nameRange.end) {
        passageName = passage.name;
        break;
      }
    }

    if (!passageName) {
      for (const passage of ast.passages) {
        if (!Array.isArray(passage.body)) continue;
        passageName = findPassageNameAtOffset(passage.body, offset);
        if (passageName) break;
      }
    }

    if (!passageName) return null;

    const changes: Record<string, TextEdit[]> = {};

    addPassageDeclarationEdits(passageName, params.newName, changes, workspace, documents);

    for (const uri of workspace.getCachedUris()) {
      addPassageReferenceEdits(uri, passageName, params.newName, changes, documents);
    }

    return { changes };
  });
}

// ---------------------------------------------------------------------------
// Rename the :: PassageName header
// ---------------------------------------------------------------------------

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
  const { ast } = parseDocument(fileText);
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

// ---------------------------------------------------------------------------
// Rename all passage references in one file:
//   [[OldName]], [[label|OldName]], [[OldName->label]]
//   <<link "label" "OldName">>, <<goto "OldName">>, <<include "OldName">>, …
// ---------------------------------------------------------------------------

function addPassageReferenceEdits(
  uri: string,
  oldName: string,
  newName: string,
  changes: Record<string, TextEdit[]>,
  documents: TextDocuments<TextDocument>,
): void {
  const fileText = getFileText(uri, documents);
  if (!fileText) return;
  const { ast } = parseDocument(fileText);
  const edits: TextEdit[] = changes[uri] ?? [];

  for (const p of ast.passages) {
    if (!Array.isArray(p.body)) continue;
    collectRenameEdits(p.body, oldName, newName, fileText, edits);
  }

  if (edits.length) changes[uri] = edits;
}

function collectRenameEdits(
  nodes: MarkupNode[],
  oldName: string,
  newName: string,
  fileText: string,
  edits: TextEdit[],
): void {
  for (const node of nodes) {
    // [[OldName]] and variants
    if (node.type === 'link' && node.target === oldName) {
      edits.push(TextEdit.replace(
        Range.create(
          offsetToPosition(fileText, node.targetRange.start),
          offsetToPosition(fileText, node.targetRange.end),
        ),
        newName,
      ));
    }

    if (node.type === 'macro') {
      // <<goto "OldName">>, <<link "label" "OldName">>, etc.
      if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
        const idx = passageArgIndex(node.name, node.args.length);
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

      // Recurse into body for nested constructs
      if (node.body) {
        collectRenameEdits(node.body, oldName, newName, fileText, edits);
      }
    }
  }
}

function isPassageLiteral(expr: ExpressionNode, name: string): boolean {
  return expr.type === 'literal' && expr.kind === 'string' && expr.value === name;
}

// ---------------------------------------------------------------------------
// Prepare-rename: find the renameable range at cursor
// ---------------------------------------------------------------------------

function findRenameRangeInNodes(
  nodes: MarkupNode[],
  offset: number,
  doc: TextDocument,
): Range | null {
  for (const node of nodes) {
    if (node.type === 'link' && offset >= node.range.start && offset <= node.range.end) {
      return Range.create(
        doc.positionAt(node.targetRange.start),
        doc.positionAt(node.targetRange.end),
      );
    }

    if (node.type === 'macro') {
      if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
        const idx = passageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          // Highlight inside the quotes only
          return Range.create(
            doc.positionAt(arg.range.start + 1),
            doc.positionAt(arg.range.end - 1),
          );
        }
      }

      if (node.body) {
        const found = findRenameRangeInNodes(node.body, offset, doc);
        if (found) return found;
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Rename request: extract the passage name at cursor
// ---------------------------------------------------------------------------

function findPassageNameAtOffset(nodes: MarkupNode[], offset: number): string | null {
  for (const node of nodes) {
    if (node.type === 'link' && offset >= node.range.start && offset <= node.range.end) {
      return node.target;
    }

    if (node.type === 'macro') {
      if (PASSAGE_ARG_MACROS.has(node.name) && node.args.length > 0) {
        const idx = passageArgIndex(node.name, node.args.length);
        const arg = node.args[idx];
        if (arg && offset >= arg.range.start && offset <= arg.range.end) {
          if (arg.type === 'literal' && arg.kind === 'string') {
            return String(arg.value);
          }
        }
      }

      if (node.body) {
        const found = findPassageNameAtOffset(node.body, offset);
        if (found) return found;
      }
    }
  }
  return null;
}