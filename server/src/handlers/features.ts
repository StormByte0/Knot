import {
  type Connection,
  FoldingRange,
  FoldingRangeKind,
  CodeAction,
  CodeActionKind,
  TextEdit,
  SemanticTokensBuilder,
  type CodeActionParams,
  type FoldingRangeParams,
  Range,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { parseStoryData } from '../storyData';
import { normalizeUri } from '../serverUtils';
import type { MarkupNode } from '../ast';
import { walkMarkup } from '../visitors';

const TOKEN_TYPES = ['function', 'class', 'variable', 'operator', 'string', 'number', 'comment'];

// ---------------------------------------------------------------------------
// Simple UUID v4 for IFID generation (server-side, no crypto module needed)
// ---------------------------------------------------------------------------
function generateIfid(): string {
  const hex = () => Math.floor(Math.random() * 16).toString(16);
  const seg = (n: number) => Array.from({ length: n }, hex).join('');
  return `${seg(8)}-${seg(4)}-4${seg(3)}-${['8','9','a','b'][Math.floor(Math.random()*4)]}${seg(3)}-${seg(12)}`.toUpperCase();
}

export function registerFeatureHandlers(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  // ── Folding ranges ─────────────────────────────────────────────────────────
  connection.onFoldingRanges((params: FoldingRangeParams): FoldingRange[] => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];

    const normUri = normalizeUri(doc.uri);
    const cached  = workspace.getParsedFile(normUri);
    const ast     = cached?.ast;
    if (!ast) return [];
    const ranges: FoldingRange[] = [];

    for (const passage of ast.passages) {
      const headerLine = doc.positionAt(passage.nameRange.start).line;
      const bodyEnd    = doc.positionAt(passage.range.end);
      const endLine    = bodyEnd.character === 0 && bodyEnd.line > headerLine
        ? bodyEnd.line - 1
        : bodyEnd.line;

      if (endLine > headerLine) {
        ranges.push(FoldingRange.create(headerLine, endLine, undefined, undefined, FoldingRangeKind.Region));
      }

      if (Array.isArray(passage.body)) {
        collectNodeFolds(passage.body, doc, ranges);
      }
    }

    return ranges;
  });

  // ── Code actions ───────────────────────────────────────────────────────────
  connection.onCodeAction((params: CodeActionParams): CodeAction[] => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return [];

    const actions: CodeAction[] = [];
    const text = doc.getText();
    const cursorOffset = doc.offsetAt(params.range.start);

    // ── 1. "Create passage" for unknown passage target diagnostics ────────────
    for (const diag of params.context.diagnostics) {
      const m = diag.message.match(/^Unknown passage target: (.+)$/);
      if (!m) continue;

      const missingName = m[1]!.trim();
      const stub = `${text.endsWith('\n') ? '' : '\n'}\n:: ${missingName}\n`;

      actions.push({
        title:       `Create passage '${missingName}'`,
        kind:        CodeActionKind.QuickFix,
        diagnostics: [diag],
        isPreferred: true,
        edit:        { changes: { [doc.uri]: [TextEdit.insert(doc.positionAt(text.length), stub)] } },
      });
    }

    // ── 2. IFID generation for StoryData passage ──────────────────────────────
    const normUri2 = normalizeUri(doc.uri);
    const cached2  = workspace.getParsedFile(normUri2);
    const ast      = cached2?.ast;
    if (!ast) return actions;
    const adapter = workspace.getActiveAdapter();
    const sdName  = adapter.getStoryDataPassageName();
    const storyDataPassage = sdName ? ast.passages.find(p => p.name === sdName) : undefined;

    if (storyDataPassage) {
      const passageStart  = storyDataPassage.range.start;
      const passageEnd    = storyDataPassage.range.end;

      // Only show when cursor is inside the StoryData passage
      if (cursorOffset >= passageStart && cursorOffset <= passageEnd) {
        const data = parseStoryData(ast, adapter);

        if (!data.ifid) {
          // No IFID at all — insert one into the JSON
          actions.push(buildInsertIfidAction(doc, text, storyDataPassage, data));
        } else if (params.context.diagnostics.some(d => d.message.includes('ifid'))) {
          // Invalid IFID — offer to replace it
          actions.push(buildReplaceIfidAction(doc, text, storyDataPassage, data));
        }
      }
    }

    return actions;
  });

  // ── Semantic tokens ────────────────────────────────────────────────────────
  connection.languages.semanticTokens.on(params => {
    const doc = documents.get(params.textDocument.uri);
    if (!doc) return { data: [] };
    const analysis = workspace.getAnalysis(normalizeUri(doc.uri));
    if (!analysis) return { data: [] };

    const builder = new SemanticTokensBuilder();
    const sorted  = [...analysis.semanticTokens].sort((a, b) => a.range.start - b.range.start);

    for (const tok of sorted) {
      const typeIdx = TOKEN_TYPES.indexOf(tok.tokenType);
      if (typeIdx === -1) continue;
      const pos = doc.positionAt(tok.range.start);
      const len = tok.range.end - tok.range.start;
      if (len <= 0) continue;
      builder.push(pos.line, pos.character, len, typeIdx, 0);
    }
    return builder.build();
  });
}

// ---------------------------------------------------------------------------
// IFID code action builders
// ---------------------------------------------------------------------------

function buildInsertIfidAction(
  doc: TextDocument,
  text: string,
  storyDataPassage: { range: { start: number; end: number }; body: unknown },
  data: { raw: Record<string, unknown> },
): CodeAction {
  const ifid = generateIfid();
  const bodyText = extractStoryDataBody(text, storyDataPassage);
  const edit = buildIfidEdit(doc, text, storyDataPassage, bodyText, ifid, null);

  return {
    title:       `Generate IFID: ${ifid}`,
    kind:        CodeActionKind.QuickFix,
    isPreferred: true,
    edit:        { changes: { [doc.uri]: [edit] } },
  };
}

function buildReplaceIfidAction(
  doc: TextDocument,
  text: string,
  storyDataPassage: { range: { start: number; end: number }; body: unknown },
  data: { ifid: string | null; raw: Record<string, unknown> },
): CodeAction {
  const ifid = generateIfid();
  const bodyText = extractStoryDataBody(text, storyDataPassage);
  const edit = buildIfidEdit(doc, text, storyDataPassage, bodyText, ifid, data.ifid);

  return {
    title: `Replace invalid IFID with: ${ifid}`,
    kind:  CodeActionKind.QuickFix,
    edit:  { changes: { [doc.uri]: [edit] } },
  };
}

function extractStoryDataBody(
  text: string,
  passage: { range: { start: number; end: number } },
): string {
  // Body starts after the header line
  const headerEnd = text.indexOf('\n', passage.range.start);
  if (headerEnd === -1) return '';
  return text.slice(headerEnd + 1, passage.range.end).trim();
}

function buildIfidEdit(
  doc: TextDocument,
  text: string,
  passage: { range: { start: number; end: number } },
  bodyText: string,
  newIfid: string,
  oldIfid: string | null,
): TextEdit {
  if (oldIfid) {
    // Replace existing IFID value in place
    const searchStr = `"ifid"`;
    const bodyStart = text.indexOf('\n', passage.range.start) + 1;
    const ifidKeyPos = text.indexOf(searchStr, bodyStart);
    if (ifidKeyPos !== -1) {
      // Find the value after the colon
      const colonPos   = text.indexOf(':', ifidKeyPos + searchStr.length);
      const valueStart = text.indexOf('"', colonPos + 1);
      const valueEnd   = text.indexOf('"', valueStart + 1) + 1;
      if (valueStart !== -1 && valueEnd > valueStart) {
        return TextEdit.replace(
          Range.create(doc.positionAt(valueStart), doc.positionAt(valueEnd)),
          `"${newIfid}"`,
        );
      }
    }
  }

  // Insert IFID into the JSON body
  try {
    const parsed = JSON.parse(bodyText || '{}') as Record<string, unknown>;
    parsed['ifid'] = newIfid;
    // Preserve indentation style from existing content
    const indent = bodyText.match(/^\s+/m)?.[0]?.replace(/\n/g, '') ?? '  ';
    const newBody = JSON.stringify(parsed, null, indent);
    const bodyStart = text.indexOf('\n', passage.range.start) + 1;
    const bodyEnd   = passage.range.end;
    return TextEdit.replace(
      Range.create(doc.positionAt(bodyStart), doc.positionAt(bodyEnd)),
      newBody + '\n',
    );
  } catch {
    // Fallback: append as a new line before closing brace
    const closeBrace = text.lastIndexOf('}', passage.range.end);
    if (closeBrace !== -1) {
      return TextEdit.insert(
        doc.positionAt(closeBrace),
        `  "ifid": "${newIfid}",\n`,
      );
    }
    // Last resort: append after header
    const headerEnd = text.indexOf('\n', passage.range.start);
    return TextEdit.insert(
      doc.positionAt(headerEnd + 1),
      `{\n  "ifid": "${newIfid}"\n}\n`,
    );
  }
}

// ---------------------------------------------------------------------------
// Folding helper
// ---------------------------------------------------------------------------

function collectNodeFolds(nodes: MarkupNode[], doc: TextDocument, out: FoldingRange[]): void {
  walkMarkup(nodes, {
    onComment(node) {
      if (node.style === 'block' || node.style === 'html') {
        const startLine = doc.positionAt(node.range.start).line;
        const endLine   = doc.positionAt(node.range.end).line;
        if (endLine > startLine) {
          out.push(FoldingRange.create(startLine, endLine, undefined, undefined, FoldingRangeKind.Comment));
        }
      }
    },
    onMacro(node) {
      if (node.hasBody && node.body) {
        const openLine  = doc.positionAt(node.range.start).line;
        const closeLine = doc.positionAt(node.range.end).line;
        if (closeLine > openLine) {
          out.push(FoldingRange.create(openLine, closeLine, undefined, undefined, FoldingRangeKind.Region));
        }
        // Note: walkMarkup will recurse into node.body automatically,
        // so nested macros will also be found.
      }
    },
  });
}
