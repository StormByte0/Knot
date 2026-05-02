import * as fs from 'node:fs';
import * as path from 'node:path';
import {
  createConnection, ProposedFeatures, TextDocuments, TextDocumentSyncKind,
  type InitializeParams, type InitializeResult,
  SemanticTokensLegend, FileChangeType,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';

import { FileStore }        from './fileStore';
import { IndexCoordinator } from './indexCoordinator';
import { WorkspaceIndex }   from './workspaceIndex';
import { uriToPath, pathToUri } from './serverUtils';
import { registerSymbolHandlers }     from './handlers/symbols';
import { registerRenameHandlers }     from './handlers/rename';
import { registerDefinitionHandlers } from './handlers/definition';
import { registerHoverHandler }       from './handlers/hover';
import { registerCompletionHandler }  from './handlers/completions';
import { registerFeatureHandlers }    from './handlers/features';
import { registerQueryHandlers }      from './handlers/queries';

// ---------------------------------------------------------------------------
// Singletons — created once, wired together, never recreated
// ---------------------------------------------------------------------------

const connection = createConnection(ProposedFeatures.all);
const documents  = new TextDocuments<TextDocument>(TextDocument);
const store      = new FileStore();
const workspace  = new WorkspaceIndex();

const log = (msg: string) => connection.console.log(msg);

const coordinator = new IndexCoordinator({
  connection, documents, fileStore: store, workspace, log,
});

// Workspace roots (all of them — not just index 0)
let workspaceRoots: string[] = [];
let excludePatterns: string[] = [];

// ---------------------------------------------------------------------------
// Initial workspace scan
//
// Called once from onInitialized with every workspace root.
// Reads .tw/.twee files from disk and feeds them to the coordinator as
// disk-sourced content.  The coordinator's debounce coalesces these into a
// single reanalyzeAll() call.
//
// We intentionally skip files larger than 2 MB to avoid memory issues with
// accidentally included binary/build output.
// ---------------------------------------------------------------------------

const MAX_FILE_BYTES = 2 * 1024 * 1024;
const TW_EXTENSIONS  = new Set(['.tw', '.twee']);

// Patterns that are always excluded regardless of user config
const HARD_EXCLUDES = [
  'node_modules', '.git', '.hg', '.svn',
  'dist', 'build', 'out', '.storyformats',
];

function scanRoot(rootPath: string, excludePatterns: string[]): void {
  const userExcludeSet = new Set(excludePatterns.map(p => p.toLowerCase()));

  function shouldSkipDir(name: string, fullPath: string): boolean {
    if (HARD_EXCLUDES.includes(name)) return true;
    const rel = path.relative(rootPath, fullPath).replace(/\\/g, '/');
    return userExcludeSet.has(rel) || userExcludeSet.has(name);
  }

  function walk(dir: string): void {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (err) {
      log(`[scan] Cannot read dir ${dir}: ${err}`);
      return;
    }

    for (const entry of entries) {
      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (!shouldSkipDir(entry.name, fullPath)) walk(fullPath);
      } else if (entry.isFile() && TW_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) {
        const uri = pathToUri(fullPath);
        // Skip if already loaded from LSP (open document wins)
        if (documents.get(uri)) continue;
        try {
          const stat = fs.statSync(fullPath);
          if (stat.size > MAX_FILE_BYTES) {
            log(`[scan] Skipping oversized file (${stat.size} bytes): ${fullPath}`);
            return;
          }
          const text = fs.readFileSync(fullPath, 'utf-8');
          coordinator.onDiskContent(uri, text);
        } catch (err) {
          log(`[scan] Cannot read file ${fullPath}: ${err}`);
        }
      }
    }
  }

  log(`[scan] Scanning root: ${rootPath}`);
  walk(rootPath);
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

connection.onInitialize((params: InitializeParams): InitializeResult => {
  workspaceRoots = (params.workspaceFolders ?? [])
    .map(f => uriToPath(f.uri))
    .filter(Boolean);

  if (workspaceRoots.length === 0 && params.rootUri) {
    workspaceRoots = [uriToPath(params.rootUri)];
  }

  const opts = params.initializationOptions as { exclude?: string[] } | undefined;
  excludePatterns = opts?.exclude ?? [];

  return {
    capabilities: {
      textDocumentSync: TextDocumentSyncKind.Incremental,
      completionProvider: {
        resolveProvider: false,
        triggerCharacters: ['<', '[', '$', '_', '.', '/','"'],
      },
      definitionProvider:      true,
      referencesProvider:      true,
      hoverProvider:           true,
      documentSymbolProvider:  true,
      workspaceSymbolProvider: true,
      foldingRangeProvider:    true,
      codeActionProvider:      true,
      renameProvider:          { prepareProvider: true },
      semanticTokensProvider: {
        legend: {
          tokenTypes:     ['function','class','variable','operator','string','number','comment'],
          tokenModifiers: [],
        } satisfies SemanticTokensLegend,
        full: true,
      },
    },
    serverInfo: { name: 'knot Language Server', version: '0.1.0' },
  };
});

connection.onInitialized(() => {
  connection.sendNotification('knot/progressStart', {
    title: 'knot Language Server', message: 'Indexing workspace…',
  });

  // Scan all workspace roots on disk.
  // Open documents (if any) have already arrived via textDocument/didOpen
  // and are richer than disk content; they are handled below.

  for (const root of workspaceRoots) {
    scanRoot(root, excludePatterns);
  }

  // Re-feed any documents that were already open when the server started.
  // (VS Code sometimes sends didOpen before onInitialized completes.)
  for (const doc of documents.all()) {
    coordinator.onLspContent(doc.uri, doc.getText(), doc.version);
  }

  // If the workspace is empty (no roots, no files), signal ready immediately
  if (store.size() === 0) {
    coordinator.scheduleAll();
  }
});

// ---------------------------------------------------------------------------
// Feature handlers
// All handlers receive workspace, which reads from the index — not from open
// documents.  Open/closed status is irrelevant to semantic correctness.
// ---------------------------------------------------------------------------

registerSymbolHandlers(connection, documents, workspace, () => workspaceRoots[0]);
registerRenameHandlers(connection, documents, workspace);
registerDefinitionHandlers(connection, documents, workspace);
registerHoverHandler(connection, documents, workspace);
registerCompletionHandler(connection, documents, workspace);
registerFeatureHandlers(connection, documents, workspace);
registerQueryHandlers(connection, documents, workspace, () => workspaceRoots[0]);

// ---------------------------------------------------------------------------
// Document sync — LSP-managed open documents
//
// These are the only paths where LSP content (ahead-of-disk, unsaved edits)
// flows into the coordinator.  The coordinator's upsert ensures LSP content
// always wins over whatever was read from disk.
// ---------------------------------------------------------------------------

documents.onDidOpen(e => {
  coordinator.onLspContent(e.document.uri, e.document.getText(), e.document.version);
});

documents.onDidChangeContent(e => {
  coordinator.onLspContent(e.document.uri, e.document.getText(), e.document.version);
});

documents.onDidClose(e => {
  // File closed in editor.  We do NOT remove it from the index.
  // It stays as disk-sourced content so cross-file features remain correct.
  // Re-read from disk to reflect the saved state.
  const filePath = uriToPath(e.document.uri);
  try {
    const text = fs.readFileSync(filePath, 'utf-8');
    coordinator.onDiskContent(e.document.uri, text);
  } catch {
    // File may have been deleted — deletion is handled by onDidChangeWatchedFiles
    log(`[sync] Could not re-read closed file: ${filePath}`);
  }
});

// ---------------------------------------------------------------------------
// File system watcher
//
// The client sets synchronize.fileEvents = createFileSystemWatcher('**/*.{tw,twee}')
// in LanguageClientOptions.  VS Code automatically translates FS events into
// workspace/didChangeWatchedFiles notifications to this handler.
//
// This is what keeps NON-OPEN files in sync: git checkouts, external editors,
// tweego output rewrite, etc.  No additional polling or server-side watching
// is needed.
// ---------------------------------------------------------------------------

connection.onDidChangeWatchedFiles(params => {
  for (const change of params.changes) {
    // Ignore non-file URIs (git:, untitled:, etc.)
    if (!change.uri.startsWith('file:')) continue;

    if (change.type === FileChangeType.Deleted) {
      coordinator.onDelete(change.uri);
      continue;
    }

    // Created or Changed — skip if currently open (onDidChangeContent owns it)
    if (documents.get(change.uri)) continue;

    const filePath = uriToPath(change.uri);
    try {
      const stat = fs.statSync(filePath);
      if (stat.size > MAX_FILE_BYTES) {
        log(`[watch] Skipping oversized file: ${filePath}`);
        continue;
      }
      const text = fs.readFileSync(filePath, 'utf-8');
      coordinator.onDiskContent(change.uri, text);
    } catch (err) {
      log(`[watch] Cannot read changed file ${filePath}: ${err}`);
    }
  }
});

// ---------------------------------------------------------------------------
// Refresh command — explicit rescan (e.g. user runs "Refresh workspace index")
// ---------------------------------------------------------------------------

connection.onNotification('knot/refreshDocuments', (params?: { exclude?: string[] }) => {
  const patterns = params?.exclude ?? excludePatterns;
  connection.sendNotification('knot/progressStart', {
    title: 'knot Language Server', message: 'Refreshing…',
  });
  log('[refresh] Full workspace rescan requested');

  // Re-scan all roots from disk
  for (const root of workspaceRoots) {
    scanRoot(root, patterns);
  }

  // Re-sync open documents (they take priority over disk)
  for (const doc of documents.all()) {
    coordinator.onLspContent(doc.uri, doc.getText(), doc.version);
  }

  // Force a reanalysis even if nothing hashed differently
  coordinator.scheduleAll();
});

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

documents.listen(connection);
connection.listen();