import {
  type Connection,
  type CompletionItem as LspCompletionItem,
  CompletionItemKind,
  InsertTextFormat,
  CompletionList,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { SymbolKind } from '../symbols';
import { resolveTypePath, inferredTypeToString } from '../serverUtils';
import { FormatRegistry } from '../formats/registry';
import { PASSAGE_ARG_MACROS } from '../passageArgs';

export function registerCompletionHandler(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  connection.onCompletion(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];

    const offset = doc.offsetAt(params.position);
    const text   = doc.getText();

    // ── Suppress on passage header lines ─────────────────────────────────────
    if (isOnPassageHeaderLine(text, offset)) {
      return CompletionList.create([], false);
    }

    const adapter = FormatRegistry.resolve(workspace.getActiveFormatId());
    const ctx     = { formatId: adapter.id, passageNames: workspace.getPassageNames() };

    // ── 1. Property access: $var.prop… ───────────────────────────────────────
    const propCtx = extractPropertyAccessContext(text, offset);
    if (propCtx) {
      const varDef = workspace.getVariableDefinition(propCtx.root);
      if (varDef?.inferredType) {
        const parentType = resolveTypePath(varDef.inferredType, propCtx.path);
        if (parentType?.kind === 'object' && parentType.properties) {
          const items: LspCompletionItem[] = [];
          for (const [key, childType] of Object.entries(parentType.properties)) {
            if (propCtx.partial && !key.startsWith(propCtx.partial)) continue;
            items.push({
              label:      key,
              insertText: key,
              kind:       (childType.kind === 'object' || childType.kind === 'array')
                            ? CompletionItemKind.Module
                            : CompletionItemKind.Field,
              detail:     inferredTypeToString(childType),
              sortText:   `0_${key}`,
            });
          }
          if (items.length > 0) return items;
        }
      }
      return [];
    }

    // ── 2. Passage name inside macro string arg ───────────────────────────────
    // e.g. <<goto "|>> or <<link "label" "|>>
    const macroPassageCtx = extractMacroPassageArgContext(text, offset);
    if (macroPassageCtx !== null) {
      return workspace.getPassageNames().map(name => ({
        label:      name,
        insertText: name,
        kind:       CompletionItemKind.File,
        sortText:   `0_${name}`,
        detail:     'passage',
      }));
    }

    // ── 3. Close-tag context ──────────────────────────────────────────────────
    const closeCtx = extractMacroCloseContext(text, offset);
    if (closeCtx !== null) {
      return adapter.provideFormatCompletions({ text, offset }, ctx);
    }

    // ── 4. Macro open context ─────────────────────────────────────────────────
    const isInMacroOpen = extractMacroOpenContext(text, offset) !== null;

    const items: LspCompletionItem[] = [];
    const seen = new Set<string>();

    for (const item of adapter.provideFormatCompletions({ text, offset }, ctx)) {
      items.push(item);
      seen.add(item.filterText ?? item.label);
    }

    for (const uri of workspace.getCachedUris()) {
      const analysis = workspace.getAnalysis(uri);
      if (!analysis) continue;

      for (const u of analysis.symbols.getUserSymbols()) {
        const key = `${u.kind}:${u.name}`;
        if (seen.has(key)) continue;
        seen.add(key);

        if (u.kind === SymbolKind.Passage && !isInMacroOpen) {
          items.push({ label: u.name, kind: CompletionItemKind.File });

        } else if (u.kind === SymbolKind.StoryVar && !isInMacroOpen) {
          const varDef  = workspace.getVariableDefinition(u.name);
          const typeStr = varDef?.inferredType ? `: ${inferredTypeToString(varDef.inferredType)}` : '';
          items.push({
            label:      `$${u.name}`,
            insertText: `$${u.name}`,
            kind:       CompletionItemKind.Variable,
            detail:     `StoryVar${typeStr}`,
          });

        } else if (u.kind === SymbolKind.Widget || u.kind === SymbolKind.Macro) {
          const macroDef = workspace.getMacroDefinition(u.name);
          const detail   = macroDef?.passageName
            ? `${u.kind === SymbolKind.Widget ? 'widget' : 'macro'} — ${macroDef.passageName}`
            : u.kind === SymbolKind.Widget ? 'custom widget' : 'custom macro';
          const hasBody  = u.kind === SymbolKind.Widget;
          items.push(buildMacroItem(u.name, detail, hasBody, adapter));
        }
      }
    }

    if (!isInMacroOpen) {
      for (const [name, def] of workspace.getAllJsGlobals()) {
        const key = `js:${name}`;
        if (seen.has(key)) continue;
        seen.add(key);
        items.push({
          label:      name,
          insertText: name,
          kind:       CompletionItemKind.Variable,
          detail:     `JS: ${inferredTypeToString(def.inferredType)}`,
        });
      }
    }

    return items;
  });
}

// ---------------------------------------------------------------------------
// Macro completion item builder
// ---------------------------------------------------------------------------

function buildMacroItem(
  name: string,
  detail: string,
  hasBody: boolean,
  adapter: ReturnType<typeof FormatRegistry.resolve>,
): LspCompletionItem {
  const snippet = adapter.buildMacroSnippet(name, hasBody) ?? `${name} $1`;
  return {
    label:            `<<${name}>>`,
    filterText:       name,
    insertText:       snippet,
    insertTextFormat: InsertTextFormat.Snippet,
    kind:             CompletionItemKind.Function,
    detail,
    sortText:         `2_${name}`,
  };
}

// ---------------------------------------------------------------------------
// Context extractors
// ---------------------------------------------------------------------------

function isOnPassageHeaderLine(text: string, offset: number): boolean {
  const lineStart = text.lastIndexOf('\n', offset - 1) + 1;
  const lineEndRaw = text.indexOf('\n', offset);
  const lineEnd = lineEndRaw === -1 ? text.length : lineEndRaw;
  return /^[ \t]*::/.test(text.slice(lineStart, lineEnd));
}

/**
 * Detect if cursor is inside the passage-name string argument of a
 * passage-referencing macro, e.g. <<goto "| or <<link "label" "|
 * Returns the partial passage name typed so far, or null if not in this context.
 */
function extractMacroPassageArgContext(text: string, offset: number): string | null {
  const before = text.slice(0, offset);

  // Build pattern: <<(macroName) ... "partial
  // We need to detect we're inside an open string that is the passage arg
  // Simple heuristic: look back for <<macroName and count string delimiters
  const macroStartMatch = before.match(/<<([A-Za-z_][A-Za-z0-9_-]*)([^>]*)$/);
  if (!macroStartMatch) return null;

  const macroName = macroStartMatch[1]!;
  if (!PASSAGE_ARG_MACROS.has(macroName)) return null;

  const afterMacroName = macroStartMatch[2]!;

  // Count complete string literals to find which arg we're in
  // A complete string: "..." or '...' not containing unescaped quotes
  const completeStrings = afterMacroName.match(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'/g) ?? [];
  const argsSoFar = completeStrings.length;

  // Check if we're currently inside an open (unclosed) string
  // Remove complete strings first, then check for an open quote
  let remaining = afterMacroName;
  for (const s of completeStrings) remaining = remaining.replace(s, ' '.repeat(s.length));

  const openQuoteMatch = remaining.match(/"([^"]*)$|'([^']*)$/);
  if (!openQuoteMatch) return null;

  // We're in arg index = argsSoFar (0-based)
  // For label-then-passage macros with 2+ args, passage is arg 1
  // For single-arg macros, passage is arg 0
  const LABEL_THEN_PASSAGE = new Set([
    'link', 'button', 'click', 'linkappend', 'linkprepend', 'linkreplace',
  ]);

  const isPassageArg = LABEL_THEN_PASSAGE.has(macroName)
    ? argsSoFar >= 1   // second or later arg
    : argsSoFar === 0; // first arg

  if (!isPassageArg) return null;

  return openQuoteMatch[1] ?? openQuoteMatch[2] ?? '';
}

function extractPropertyAccessContext(
  text: string,
  offset: number,
): { root: string; path: string[]; partial: string } | null {
  const m = text.slice(0, offset).match(
    /\$([A-Za-z_][A-Za-z0-9_]*)((?:\.[A-Za-z_][A-Za-z0-9_]*)*)\.([A-Za-z_][A-Za-z0-9_]*)?$/,
  );
  if (!m) return null;
  return { root: m[1]!, path: m[2] ? m[2].slice(1).split('.') : [], partial: m[3] ?? '' };
}

function extractMacroOpenContext(text: string, offset: number): string | null {
  const before = text.slice(0, offset);
  const m = before.match(/<<([A-Za-z_=\-][\w-]*)?\s*$/);
  if (!m) return null;
  if (!before.endsWith('<<') && !/<<[\w-]*$/.test(before)) return null;
  return m[1] ?? '';
}

function extractMacroCloseContext(text: string, offset: number): string | null {
  const m = text.slice(0, offset).match(/<{2}\/(\w[-\w]*)$/);
  return m ? m[1]! : null;
}