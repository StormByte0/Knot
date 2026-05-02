import * as path from 'node:path';
import {
  type Connection,
  DiagnosticSeverity,
  Range,
} from 'vscode-languageserver/node';
import { TextDocuments } from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { WorkspaceIndex } from '../workspaceIndex';
import { SymbolKind } from '../symbols';
import { parseDocument } from '../parser';
import { parseStoryData, validateStoryData } from '../storyData';
import { uriToPath, getFileText } from '../serverUtils';

// ---------------------------------------------------------------------------
// Notification / request IDs (exported so lspServer.ts can use them)
// ---------------------------------------------------------------------------
export const GET_PASSAGES_REQUEST   = 'knot/getPassages';
export const GET_STORY_DATA_REQUEST = 'knot/getStoryData';
export const STORY_DATA_UPDATED     = 'knot/storyDataUpdated';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface PassageEntry {
  name:         string;
  uri:          string;
  fileName:     string;
  refCount:     number;
  incomingFrom: string[];   // source passage names that link here, deduplicated
}

export interface StoryDataResponse {
  ifid:          string | null;
  format:        string | null;
  formatVersion: string | null;
  start:         string | null;
  passageCount:  number;
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export function registerQueryHandlers(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
  getWorkspaceFolderPath: () => string | undefined,
): void {
  // ── knot/getPassages ────────────────────────────────────────────────────────
  connection.onRequest(GET_PASSAGES_REQUEST, (): PassageEntry[] => {
    const seen    = new Set<string>();
    const results: PassageEntry[] = [];
    const workspaceFolderPath = getWorkspaceFolderPath();

    for (const uri of workspace.getCachedUris()) {
      if (workspaceFolderPath) {
        const fp = uriToPath(uri);
        if (!isUnderRoot(fp, workspaceFolderPath)) continue;
        if (fp.includes(path.sep + 'test' + path.sep + 'fixtures') ||
            fp.includes('/test/fixtures')) continue;
      }

      const analysis = workspace.getAnalysis(uri);
      if (!analysis) continue;

      for (const sym of analysis.symbols.getUserSymbols()) {
        if (sym.kind !== SymbolKind.Passage) continue;
        if (seen.has(sym.name)) continue;
        seen.add(sym.name);
        results.push({
          name:         sym.name,
          uri:          sym.uri,
          fileName:     path.basename(sym.uri),
          refCount:     workspace.getReferencingFiles(sym.name).length,
          incomingFrom: workspace.getIncomingLinks(sym.name).map(l => l.sourcePassage),
        });
      }
    }

    return results.sort((a, b) => {
      const aSpec = isSpecialPassage(a.name);
      const bSpec = isSpecialPassage(b.name);
      if (aSpec && !bSpec) return -1;
      if (!aSpec && bSpec) return  1;
      return a.name.localeCompare(b.name);
    });
  });

  // ── knot/getStoryData ───────────────────────────────────────────────────────
  connection.onRequest(GET_STORY_DATA_REQUEST, (): StoryDataResponse | null => {
    return buildStoryDataResponse(connection, documents, workspace);
  });
}

// ---------------------------------------------------------------------------
// Broadcast — called after any reanalysis
// ---------------------------------------------------------------------------

export function broadcastStoryData(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): void {
  const data = buildStoryDataResponse(connection, documents, workspace);
  if (data) connection.sendNotification(STORY_DATA_UPDATED, data);
}

// ---------------------------------------------------------------------------
// Exported for use in lspServer.ts (format ID extraction)
// ---------------------------------------------------------------------------

export function buildStoryDataResponseExported(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): StoryDataResponse | null {
  return buildStoryDataResponse(connection, documents, workspace);
}

// ---------------------------------------------------------------------------
// Core logic
// ---------------------------------------------------------------------------

function buildStoryDataResponse(
  connection: Connection,
  documents: TextDocuments<TextDocument>,
  workspace: WorkspaceIndex,
): StoryDataResponse | null {
  const allPassageNames = new Set(workspace.getPassageNames());
  const totalPassages = workspace.getPassageNames().length;
  // Search every indexed file for a StoryData passage
  for (const uri of workspace.getCachedUris()) {
    const text = getFileText(uri, documents);
    if (!text) continue;

    const { ast } = parseDocument(text);
    const data    = parseStoryData(ast);

    if (data.ifid === null && data.format === null && data.start === null) continue;

    // Validate and push diagnostics if needed
    const diags = validateStoryData(data, allPassageNames);
    if (diags.length > 0) {
      const storyDataPassage = ast.passages.find(p => p.name === 'StoryData');
      const openDoc          = documents.get(uri);
      if (storyDataPassage && openDoc) {
        const existing    = workspace.getAnalysis(uri);
        const existingLsp = existing
          ? existing.diagnostics.map(d => ({
              message:  d.message,
              range:    Range.create(openDoc.positionAt(d.range.start), openDoc.positionAt(d.range.end)),
              severity: d.severity === 'error' ? DiagnosticSeverity.Error : DiagnosticSeverity.Warning,
            }))
          : [];
        const storyDataDiags = diags.map(d => ({
          message:  d.message,
          range:    Range.create(
            openDoc.positionAt(storyDataPassage.nameRange.start),
            openDoc.positionAt(storyDataPassage.nameRange.end),
          ),
          severity: d.severity === 'error' ? DiagnosticSeverity.Error : DiagnosticSeverity.Warning,
          source:   'knot-storydata',
        }));
        connection.sendDiagnostics({ uri, diagnostics: [...existingLsp, ...storyDataDiags] });
      }
    }

    return {
      ifid:          data.ifid,
      format:        data.format,
      formatVersion: data.formatVersion,
      start:         data.start,
      passageCount:  totalPassages,
    };
  }

  return { ifid: null, format: null, formatVersion: null, start: null, passageCount: totalPassages };
}

/**
 * Returns true if `filePath` is at or below `rootPath`, with correct path
 * boundary semantics (avoids the /repo vs /repo2 prefix collision).
 */
function isUnderRoot(filePath: string, rootPath: string): boolean {
  const normalRoot = rootPath.replace(/[\/]+$/, '');
  const normalFile = filePath.replace(/\\/g, '/');
  const normalRootFwd = normalRoot.replace(/\\/g, '/');
  return normalFile === normalRootFwd ||
    normalFile.startsWith(normalRootFwd + '/') ||
    normalFile.startsWith(normalRootFwd + '\\');
}

function isSpecialPassage(name: string): boolean {
  return name.startsWith('Story') || name.startsWith('_') || name === 'StoryInit';
}