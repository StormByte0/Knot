/**
 * Knot v2 — LSP Server
 *
 * Thin dispatcher that orchestrates core modules and delegates
 * all LSP feature handling to handler modules.
 *
 * Data flow guarantee:
 *   LSP request → handler → core module → formatRegistry → FormatModule
 *   No handler or core module ever imports from format-specific directories.
 *
 * This file ONLY handles:
 *   - Server lifecycle (initialize, initialized, shutdown)
 *   - Document synchronization (open, change, close)
 *   - Handler registration (via registerHandlers())
 *   - Index change propagation (wiring core modules together)
 *   - Format detection (StoryData scanning)
 *   - Workspace file scanning at startup
 *
 * All LSP feature logic lives in handler modules.
 *
 * KEY DESIGN DECISIONS (multi-format isolation):
 *   1. Fallback is the DEFAULT — no format is baked in
 *   2. Workspace files are scanned from disk at startup (scanRoot)
 *   3. Format detection runs AFTER files are indexed
 *   4. Auto-detection from StoryData switches to the real format
 *   5. Heuristic detection as fallback when StoryData is missing
 *   6. Closed documents stay in the index (re-read from disk)
 *   7. File watcher keeps non-open files in sync
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import {
  Connection,
  InitializeParams,
  InitializeResult,
  TextDocumentSyncKind,
  DidChangeConfigurationParams,
  DiagnosticSeverity,
  Range,
  Position,
  CodeActionKind,
  RequestType,
  NotificationType,
  FileChangeType,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { FormatRegistry } from './formats/formatRegistry';
import type { BodyToken, FormatModule, PassageRef } from './formats/_types';
import { WorkspaceIndex, PassageEntry, DiagnosticEngine, DiagnosticSettings } from './core';
import { DocumentStore } from './core/documentStore';
import { ReferenceIndex } from './core/referenceIndex';
import { LinkGraph } from './core/linkGraph';
import { SymbolTable, SymbolKind, SymbolEntry } from './core/symbolTable';
import { Parser } from './core/parser';
import { ASTWorkspace, DocumentAnalysis } from './core/astWorkspace';
import { ASTNode, DocumentAST, walkTree, findDeepestNode } from './core/ast';
import { PassageType, MacroCategory, MacroKind, LinkKind, PassageRefKind } from './hooks/hookTypes';
import { registerHandlers, createDiagnosticsHandler, HandlerDependencies, TOKEN_TYPES, TOKEN_MODIFIERS } from './handlers';
import { DiagnosticsHandler } from './handlers/diagnostics';

// ─── Custom Protocol Types (mirrors shared/protocol.ts) ──────────
// Defined inline to avoid rootDir constraint with composite projects.
// Must stay in sync with shared/protocol.ts.

namespace RefreshDocumentsRequest {
  export const type = new RequestType<void, void, void>('knot/refreshDocuments');
}

namespace ListPassagesRequest {
  export const type = new RequestType<void, string[], void>('knot/listPassages');
}

namespace ListFormatsRequest {
  export const type = new RequestType<void, Array<{ id: string; name: string; version: string }>, void>('knot/listFormats');
}

namespace SelectFormatRequest {
  export const type = new RequestType<{ formatId: string }, { success: boolean; formatName: string }, void>('knot/selectFormat');
}

namespace FormatChangedNotification {
  export const type = new NotificationType<{ formatId: string; formatName: string }>('knot/formatChanged');
}

namespace ServerStatusNotification {
  export const type = new NotificationType<{
    state: 'initialized' | 'indexing' | 'ready' | 'error';
    formatId: string;
    formatName: string;
    passageCount: number;
    openDocuments: number;
    message?: string;
  }>('knot/serverStatus');
}

// ─── Constants ──────────────────────────────────────────────────

const MAX_FILE_BYTES = 2 * 1024 * 1024; // 2 MB — skip oversized files
const TW_EXTENSIONS = new Set(['.tw', '.twee']);

// Directories that are always excluded from scanning
const HARD_EXCLUDES = [
  'node_modules', '.git', '.hg', '.svn',
  'dist', 'build', 'out', '.storyformats',
];

// ─── URI ↔ Path helpers ─────────────────────────────────────────

function uriToPath(uri: string): string {
  let filePath: string;
  if (uri.startsWith('file:///')) {
    filePath = uri.slice(7); // file:///path → /path
  } else if (uri.startsWith('file://')) {
    filePath = uri.slice(7); // file://host/path → /host/path
  } else if (uri.startsWith('file:')) {
    filePath = uri.slice(5); // file:path → path
  } else {
    filePath = uri;
  }

  // Decode URI-encoded characters (%3A → :, %20 → space, etc.)
  // This is critical on Windows where VS Code sends
  // file:///d%3A/codeWS/twine/By%20the%20Book
  try {
    filePath = decodeURIComponent(filePath);
  } catch {
    // If decoding fails (malformed %), use as-is
  }

  // On Windows, strip the leading / before a drive letter
  // e.g. /d:/codeWS/... → d:/codeWS/...
  if (process.platform === 'win32' && /^\/[A-Za-z]:\//.test(filePath)) {
    filePath = filePath.slice(1);
  }

  return filePath;
}

function pathToUri(filePath: string): string {
  // Normalize backslashes to forward slashes (Windows)
  const normalized = filePath.replace(/\\/g, '/');
  // Encode characters that are invalid in URIs but valid in file paths
  // (spaces, non-ASCII, etc.) — but NOT the drive letter colon on Windows
  const encoded = normalized
    .replace(/ /g, '%20')
    .replace(/[^/A-Za-z0-9_.\-:]/g, c => {
      // Encode anything that's not a safe URI character
      // Allow colon only for Windows drive letter (first 2 chars)
      if (c === ':' && normalized.indexOf(':') > 2) return encodeURIComponent(c);
      if (c === ':') return c; // Keep drive letter colon as-is
      return encodeURIComponent(c);
    });
  // Ensure proper file:// URI
  if (encoded.startsWith('/')) {
    return `file://${encoded}`;
  }
  return `file:///${encoded}`;
}

function normalizeUri(uri: string): string {
  // Normalize URI for consistent lookups:
  // 1. Replace backslashes (shouldn't happen but be safe)
  // 2. Decode then re-encode to normalize %XX sequences
  let normalized = uri.replace(/\\/g, '/');
  try {
    // Decode the path portion to normalize, then re-encode consistently
    // This ensures file:///d%3A/ and file:///d:/ match the same document
    const match = normalized.match(/^(file:\/\/+)?(.*)/);
    if (match && match[2]) {
      const decoded = decodeURIComponent(match[2]);
      // Re-encode consistently: only encode what's truly necessary
      const reEncoded = decoded.replace(/ /g, '%20');
      normalized = (match[1] || '') + reEncoded;
    }
  } catch {
    // If normalization fails, use the original
  }
  return normalized;
}

// ─── LSP Server ────────────────────────────────────────────────

export class LspServer {
  private connection: Connection;
  private formatRegistry: FormatRegistry;
  private workspaceIndex: WorkspaceIndex;
  private documentStore: DocumentStore;
  private referenceIndex: ReferenceIndex;
  private linkGraph: LinkGraph;
  private symbolTable: SymbolTable;
  private diagnosticEngine: DiagnosticEngine;
  private parser: Parser;
  private astWorkspace: ASTWorkspace;
  private diagnosticsHandler: DiagnosticsHandler;

  /** Workspace root paths (from InitializeParams) */
  private workspaceRoots: string[] = [];
  private settings: Record<string, unknown> = {};

  /**
   * Disk-sourced file content map.
   * Stores content read from disk during scanRoot() and file watchers.
   * This content persists even when documents are closed in the editor,
   * ensuring the workspace index stays complete for cross-file features.
   *
   * Priority rule: LSP content (documentStore) always wins over disk content.
   */
  private diskContent: Map<string, string> = new Map();

  /** Exclude patterns from client configuration */
  private excludePatterns: string[] = [];

  constructor(connection: Connection) {
    this.connection = connection;

    // Initialize the format system
    this.formatRegistry = new FormatRegistry();

    // Initialize core modules — all receive formatRegistry for format-agnostic data access
    this.workspaceIndex = new WorkspaceIndex(this.formatRegistry);
    this.documentStore = new DocumentStore();
    this.referenceIndex = new ReferenceIndex();
    this.linkGraph = new LinkGraph();
    this.symbolTable = new SymbolTable();
    this.diagnosticEngine = new DiagnosticEngine(this.formatRegistry, this.workspaceIndex);
    this.parser = new Parser(this.formatRegistry);

    // Initialize AST workspace (coordinates AST building, analysis, virtual docs)
    this.astWorkspace = new ASTWorkspace(this.formatRegistry, this.workspaceIndex, this.symbolTable);
    this.diagnosticEngine.setASTWorkspace(this.astWorkspace);

    // Create diagnostics handler (used for publishDiagnostics)
    this.diagnosticsHandler = new DiagnosticsHandler({
      diagnosticEngine: this.diagnosticEngine,
      documentStore: this.documentStore,
    });

    // Register index change listener to update references and link graph
    this.workspaceIndex.onDidChange(event => {
      this.onIndexChange(event);
    });
  }

  // ─── Initialize ──────────────────────────────────────────────

  initialize(params: InitializeParams): InitializeResult {
    // Capture ALL workspace roots — not just the first one
    this.workspaceRoots = (params.workspaceFolders ?? [])
      .map(f => uriToPath(f.uri))
      .filter(Boolean);

    if (this.workspaceRoots.length === 0 && params.rootUri) {
      this.workspaceRoots = [uriToPath(params.rootUri)];
    }

    // Load all built-in format modules
    this.formatRegistry.loadBuiltinFormats();

    // Start on fallback — NO format is baked in as default.
    // This is the core design principle of v2: format isolation.
    // Format auto-detection (StoryData / heuristic) will switch to
    // the correct format after workspace files are scanned and indexed.
    this.formatRegistry.setActiveFormat(undefined);

    // Compute trigger characters from all loaded format modules
    const triggerChars = this.computeTriggerCharacters();

    return {
      capabilities: {
        textDocumentSync: TextDocumentSyncKind.Incremental,
        completionProvider: {
          triggerCharacters: triggerChars,
          resolveProvider: true,
        },
        hoverProvider: true,
        definitionProvider: true,
        referencesProvider: true,
        renameProvider: {
          prepareProvider: true,
        },
        documentSymbolProvider: true,
        workspaceSymbolProvider: true,
        documentLinkProvider: {
          resolveProvider: false,
        },
        codeActionProvider: {
          codeActionKinds: [CodeActionKind.QuickFix, CodeActionKind.Refactor],
        },
        semanticTokensProvider: {
          full: true,
          legend: {
            tokenTypes: TOKEN_TYPES,
            tokenModifiers: TOKEN_MODIFIERS,
          },
        },
        workspace: {
          workspaceFolders: {
            supported: true,
          },
        } satisfies any,
      },
    };
  }

  // ─── Initialized ─────────────────────────────────────────────

  initialized(): void {
    // Register document synchronization handlers
    this.registerDocumentHandlers();

    // Register LSP feature handlers via handler modules
    const handlerDeps: HandlerDependencies = {
      connection: this.connection,
      formatRegistry: this.formatRegistry,
      workspaceIndex: this.workspaceIndex,
      documentStore: this.documentStore,
      referenceIndex: this.referenceIndex,
      diagnosticEngine: this.diagnosticEngine,
    };
    registerHandlers(handlerDeps);

    // Register custom protocol handlers
    this.connection.onRequest(RefreshDocumentsRequest.type, () => {
      this.refreshWorkspace();
      return undefined; // void response
    });

    this.connection.onRequest(ListPassagesRequest.type, () => {
      return this.workspaceIndex.getAllPassageNames();
    });

    this.connection.onRequest(ListFormatsRequest.type, () => {
      return this.formatRegistry.getAvailableFormatIds()
        .map(id => this.formatRegistry.getFormat(id))
        .filter((fmt): fmt is NonNullable<typeof fmt> => fmt !== undefined)
        .map(fmt => ({
          id: fmt.formatId,
          name: fmt.displayName,
          version: fmt.version,
        }));
    });

    this.connection.onRequest(SelectFormatRequest.type, (params) => {
      const format = this.formatRegistry.getFormat(params.formatId);
      if (format) {
        this.formatRegistry.setActiveFormat(params.formatId);
        this.reindexAll();
        // Re-publish diagnostics with new format
        for (const uri of this.getAllKnownUris()) {
          this.publishDiagnostics(uri);
        }
        return { success: true, formatName: format.displayName };
      }
      return { success: false, formatName: '' };
    });

    // Register configuration change handler
    this.connection.onDidChangeConfiguration(this.onConfigurationChange.bind(this));

    // Register file system watcher handler (keeps non-open files in sync)
    this.connection.onDidChangeWatchedFiles(params => {
      this.onDidChangeWatchedFiles(params);
    });

    // Pull initial configuration from the client
    this.pullConfiguration();

    // ─── KEY SEQUENCE (mirrors master branch) ─────────────────
    // 1. Scan workspace roots for .tw/.twee files on disk
    // 2. Re-feed any documents already open via LSP (they take priority)
    // 3. THEN detect format from indexed content
    // 4. Notify client of the active format

    this.connection.console.info('Knot v2 language server initializing...');
    this.sendServerStatus('indexing');

    // Step 1: Scan all workspace roots from disk
    for (const root of this.workspaceRoots) {
      this.scanRoot(root, this.excludePatterns);
    }
    this.connection.console.info(`Scanned ${this.diskContent.size} file(s) from disk`);

    // Step 2: Re-feed any documents that were already open when the server started.
    // (VS Code sometimes sends didOpen before onInitialized completes.)
    // LSP content always wins over disk content.
    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        this.indexContent(uri, doc.getText());
      }
    }

    // Step 3: Detect format from the NOW-INDEXED workspace
    this.detectWorkspaceFormat();

    // Step 4: Always notify the client about the current format
    this.notifyFormatChanged();

    this.connection.console.info('Knot v2 language server initialized');
    this.connection.console.info(`Active format: ${this.formatRegistry.getActiveFormat().formatId} (${this.formatRegistry.getActiveFormat().displayName})`);
    this.connection.console.info(`Indexed ${this.workspaceIndex.size} passages across ${this.diskContent.size + this.documentStore.getUris().length} sources`);

    // Send initial server status to client
    this.sendServerStatus('ready');
  }

  // ─── Workspace Scanning ──────────────────────────────────────

  /**
   * Walk a workspace root directory and read all .tw/.twee files from disk.
   * This mirrors the master branch's scanRoot() function.
   *
   * Files are stored in diskContent (not documentStore) because they are
   * not currently open in the editor. When a file IS opened, the LSP
   * content in documentStore takes priority.
   */
  private scanRoot(rootPath: string, excludePatterns: string[]): void {
    const userExcludeSet = new Set(excludePatterns.map(p => p.toLowerCase()));

    const shouldSkipDir = (name: string, fullPath: string): boolean => {
      if (HARD_EXCLUDES.includes(name)) return true;
      const rel = path.relative(rootPath, fullPath).replace(/\\/g, '/');
      return userExcludeSet.has(rel) || userExcludeSet.has(name);
    };

    const walk = (dir: string): void => {
      let entries: fs.Dirent[];
      try {
        entries = fs.readdirSync(dir, { withFileTypes: true });
      } catch (err) {
        this.connection.console.warn(`[scan] Cannot read dir ${dir}: ${err}`);
        return;
      }

      for (const entry of entries) {
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          if (!shouldSkipDir(entry.name, fullPath)) walk(fullPath);
        } else if (entry.isFile() && TW_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) {
          const uri = pathToUri(fullPath);
          // Skip if already tracked as an open LSP document (LSP content wins)
          if (this.documentStore.has(uri)) continue;
          try {
            const stat = fs.statSync(fullPath);
            if (stat.size > MAX_FILE_BYTES) {
              this.connection.console.warn(`[scan] Skipping oversized file (${stat.size} bytes): ${fullPath}`);
              continue;
            }
            const text = fs.readFileSync(fullPath, 'utf-8');
            this.diskContent.set(normalizeUri(uri), text);
            this.indexContent(uri, text);
          } catch (err) {
            this.connection.console.warn(`[scan] Cannot read file ${fullPath}: ${err}`);
          }
        }
      }
    };

    this.connection.console.info(`[scan] Scanning root: ${rootPath}`);
    walk(rootPath);
  }

  // ─── Document Synchronization ────────────────────────────────

  private registerDocumentHandlers(): void {
    this.connection.onDidOpenTextDocument(params => {
      const doc = TextDocument.create(
        params.textDocument.uri,
        params.textDocument.languageId,
        params.textDocument.version,
        params.textDocument.text,
      );
      const uri = doc.uri;
      this.documentStore.set(uri, doc);

      // Pre-scan for StoryData BEFORE full indexing (Twine engine level detection)
      const text = doc.getText();
      const detectedFormat = this.preScanStoryData(text);
      if (detectedFormat && detectedFormat !== this.formatRegistry.getActiveFormat().formatId) {
        this.formatRegistry.setActiveFormat(detectedFormat);
        this.connection.console.info(`Detected story format from new document: ${detectedFormat}`);
        this.notifyFormatChanged();
        // Re-index any already-open documents with the new format
        this.reindexAll();
      }

      this.indexContent(uri, text);
      this.astWorkspace.buildAndAnalyze(uri, text, doc.version);

      // Remove from disk content since LSP now owns this file
      this.diskContent.delete(normalizeUri(uri));

      this.connection.console.info(`Indexed document: ${uri} (${this.workspaceIndex.size} total passages)`);
      this.sendServerStatus('ready');

      this.publishDiagnostics(uri);
    });

    this.connection.onDidChangeTextDocument(params => {
      const doc = this.documentStore.get(params.textDocument.uri);
      if (!doc) return;

      // Apply incremental changes
      TextDocument.update(doc, params.contentChanges, params.textDocument.version);
      this.documentStore.set(doc.uri, doc);
      const text = doc.getText();

      this.workspaceIndex.reindexDocument(doc.uri, text);

      // Rebuild AST for the changed document
      this.astWorkspace.invalidate(doc.uri);
      this.astWorkspace.buildAndAnalyze(doc.uri, text, doc.version);

      this.publishDiagnostics(doc.uri);
      this.sendServerStatus('ready');
    });

    this.connection.onDidCloseTextDocument(params => {
      const uri = params.textDocument.uri;

      // DON'T delete from the index — the file still exists on disk.
      // Re-read from disk and keep it as disk-sourced content.
      // This mirrors the master branch: "We do NOT remove it from the index."
      this.documentStore.delete(uri);
      this.astWorkspace.invalidate(uri);

      // Re-read from disk to reflect the saved state
      const filePath = uriToPath(uri);
      try {
        const text = fs.readFileSync(filePath, 'utf-8');
        this.diskContent.set(normalizeUri(uri), text);
        // Re-index with disk content
        this.indexContent(uri, text);
        this.connection.console.info(`Document closed — re-indexed from disk: ${uri}`);
      } catch {
        // File may have been deleted — removal handled by onDidChangeWatchedFiles
        this.connection.console.warn(`[sync] Could not re-read closed file: ${filePath}`);
        // Remove from disk content too
        this.diskContent.delete(normalizeUri(uri));
      }
    });
  }

  // ─── File Watcher Handler ────────────────────────────────────

  /**
   * Handle file system changes for NON-OPEN files.
   * Open files are handled by textDocument/didChange.
   * This keeps the index in sync for files changed outside the editor
   * (git checkouts, external editors, tweego output, etc.)
   */
  private onDidChangeWatchedFiles(params: { changes: Array<{ uri: string; type: number }> }): void {
    for (const change of params.changes) {
      // Ignore non-file URIs (git:, untitled:, etc.)
      if (!change.uri.startsWith('file:')) continue;

      const uri = change.uri;

      if (change.type === FileChangeType.Deleted) {
        // File deleted — remove from disk content and index
        this.diskContent.delete(normalizeUri(uri));
        this.workspaceIndex.removeDocument(uri);
        this.astWorkspace.invalidate(uri);
        this.connection.console.info(`[watch] File deleted: ${uri}`);
        continue;
      }

      // Created or Changed — skip if currently open in editor (LSP owns it)
      if (this.documentStore.has(uri)) continue;

      const filePath = uriToPath(uri);
      try {
        const stat = fs.statSync(filePath);
        if (stat.size > MAX_FILE_BYTES) {
          this.connection.console.warn(`[watch] Skipping oversized file: ${filePath}`);
          continue;
        }
        const text = fs.readFileSync(filePath, 'utf-8');
        this.diskContent.set(normalizeUri(uri), text);
        this.indexContent(uri, text);
        this.astWorkspace.buildAndAnalyze(uri, text, 0);
        this.connection.console.info(`[watch] Re-indexed: ${uri}`);
      } catch (err) {
        this.connection.console.warn(`[watch] Cannot read changed file ${filePath}: ${err}`);
      }
    }
  }

  // ─── Index Content ───────────────────────────────────────────

  /**
   * Index file content into the workspace index.
   * Used for both LSP-sourced and disk-sourced content.
   */
  private indexContent(uri: string, text: string): void {
    this.workspaceIndex.indexDocument(uri, text);
  }

  // ─── Configuration ───────────────────────────────────────────

  private onConfigurationChange(params: DidChangeConfigurationParams): void {
    const settings = params.settings?.knot ?? {};
    this.settings = settings;

    // Update diagnostic settings
    const lint = settings.lint ?? {};
    this.diagnosticEngine.updateSettings({
      'unknown-passage': lint.unknownPassage,
      'unknown-macro': lint.unknownMacro,
      'duplicate-passage': lint.duplicatePassage,
      'type-mismatch': lint.typeMismatch,
      'unreachable-passage': lint.unreachablePassage,
      'container-structure': lint.containerStructure,
      'deprecated-macro': lint.deprecatedMacro,
      'missing-argument': lint.missingArgument,
      'invalid-assignment': lint.invalidAssignment,
    });

    // Check for format change
    const formatId = settings.format?.activeFormat;
    if (formatId && formatId !== this.formatRegistry.getActiveFormat().formatId) {
      this.formatRegistry.setActiveFormat(formatId);
      this.notifyFormatChanged();
      // Re-index with new format
      this.reindexAll();
    }

    // Re-publish diagnostics
    for (const uri of this.getAllKnownUris()) {
      this.publishDiagnostics(uri);
    }
  }

  // ─── Diagnostics Publishing ──────────────────────────────────

  private publishDiagnostics(uri: string): void {
    const diagnostics = this.diagnosticsHandler.computeDiagnostics(uri);
    this.connection.sendDiagnostics({ uri, diagnostics });
  }

  // ─── Format Detection ────────────────────────────────────────

  /**
   * Pre-scan for StoryData passage using raw text search.
   * This runs BEFORE full workspace indexing so the correct format
   * module is active when indexing begins.
   *
   * Twine engine behavior: The StoryData passage always has the name
   * "StoryData" and contains JSON metadata including the format name.
   * Core can detect this without any format module — it's a Twee 3 spec feature.
   */
  preScanStoryData(content: string): string | undefined {
    // Raw text search for :: StoryData passage header
    const storyDataHeader = /^::\s*StoryData(?:\s*\[([^\]]*)\])?(?:\s*\{[^}]*\})?\s*$/m;
    const headerMatch = content.match(storyDataHeader);
    if (!headerMatch) return undefined;

    // Extract the body after the header (everything until next :: header or EOF)
    const headerEnd = (headerMatch.index ?? 0) + headerMatch[0].length;
    const nextHeader = content.indexOf('\n::', headerEnd);
    const bodyEnd = nextHeader >= 0 ? nextHeader : content.length;
    const body = content.substring(headerEnd + 1, bodyEnd).trim();

    // Detect format from StoryData JSON — returns FormatModule, not string
    const detected = this.formatRegistry.detectFromStoryData(body);
    if (detected.formatId !== 'fallback') {
      return detected.formatId;
    }
    return undefined;
  }

  /**
   * Detect the story format from the workspace StoryData passage.
   * Must be called AFTER workspace files have been scanned and indexed.
   *
   * Detection strategy (in order):
   *   1. Look for StoryData passage in the indexed workspace
   *   2. Pre-scan all documents (LSP + disk) for StoryData
   *   3. Heuristic: scan for format-specific patterns (<< >>, (macro:), etc.)
   *
   * If nothing is found, we stay on fallback (basic Twee) which
   * was set during initialize(). This is intentional — we do NOT
   * default to any specific format to avoid format bleed.
   * Fallback provides basic Twee features ([[links]], passage navigation,
   * core diagnostics). The user can manually select via command.
   */
  private detectWorkspaceFormat(): void {
    // First try: look for StoryData passage in the already-indexed workspace
    const storyDataPassages = this.workspaceIndex.getPassagesByType(PassageType.StoryData);
    if (storyDataPassages.length > 0) {
      const storyData = storyDataPassages[0];
      const detected = this.formatRegistry.detectFromStoryData(storyData.body ?? storyData.name);
      if (detected.formatId !== 'fallback') {
        this.formatRegistry.setActiveFormat(detected.formatId);
        this.connection.console.info(`Detected story format from indexed StoryData: ${detected.formatId}`);
        return;
      }
    }

    // Second try: pre-scan ALL documents (both LSP and disk-sourced) for StoryData
    // This catches cases where StoryData is in a file that hasn't been
    // classified as PassageType.StoryData yet (e.g., parsing order issue)
    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        const detectedFormat = this.preScanStoryData(doc.getText());
        if (detectedFormat) {
          this.formatRegistry.setActiveFormat(detectedFormat);
          this.connection.console.info(`Detected story format via pre-scan (LSP): ${detectedFormat}`);
          return;
        }
      }
    }

    for (const [uri, text] of this.diskContent) {
      const detectedFormat = this.preScanStoryData(text);
      if (detectedFormat) {
        this.formatRegistry.setActiveFormat(detectedFormat);
        this.connection.console.info(`Detected story format via pre-scan (disk): ${detectedFormat}`);
        return;
      }
    }

    // Third try: heuristic format detection
    // Delegates to FormatRegistry which checks each format's macroPattern.
    // No format-specific knowledge lives here — the registry owns it.
    const allTexts = [
      ...Array.from(this.diskContent.values()),
      ...this.documentStore.getUris()
        .map(uri => this.documentStore.get(uri)?.getText())
        .filter(Boolean) as string[],
    ];
    const sample = allTexts.slice(0, 5);
    const heuristicFormat = this.formatRegistry.detectFromHeuristic(sample);
    if (heuristicFormat) {
      this.formatRegistry.setActiveFormat(heuristicFormat);
      this.connection.console.info(`Detected story format via heuristic: ${heuristicFormat}`);
      return;
    }

    // No StoryData found, no heuristic match — stay on fallback (basic Twee).
    // This is intentional: we do NOT default to any specific format.
    // Fallback provides basic Twee features ([[links]], passage navigation,
    // core diagnostics). User can select format via command.
    this.connection.console.info(
      'No StoryData passage found. Staying on fallback (Basic Twee). ' +
      'Use "Knot: Select Story Format" command to set the format manually.'
    );
  }

  // ─── Index Change Handler ────────────────────────────────────

  private onIndexChange(event: any): void {
    // Update reference index and link graph
    for (const passage of event.passages) {
      if (event.type === 'remove') {
        this.referenceIndex.removeReferencesByUri(event.uri);
        this.linkGraph.removeEdgesFrom(passage.name);
      }
      if (event.type === 'add' || event.type === 'update') {
        // Update link graph using passageRefs (single source of truth)
        this.linkGraph.removeEdgesFrom(passage.name);
        for (const ref of passage.passageRefs) {
          const linkKind = ref.linkKind ?? (ref.kind === PassageRefKind.Link ? LinkKind.Passage : LinkKind.Custom);
          this.linkGraph.addEdge({
            from: passage.name,
            to: ref.target,
            kind: linkKind,
          });
        }

        // Update reference index
        for (const ref of passage.passageRefs) {
          this.referenceIndex.addReference({
            uri: event.uri,
            sourcePassage: passage.name,
            targetPassage: ref.target,
            kind: ref.linkKind ?? LinkKind.Passage,
            startOffset: ref.range.start,
            endOffset: ref.range.end,
          });
        }
      }
    }
  }

  // ─── Re-index ────────────────────────────────────────────────

  private reindexAll(): void {
    this.workspaceIndex.clear();
    this.referenceIndex.clear();
    this.linkGraph.clear();
    this.astWorkspace.clear();

    // Re-index all LSP-sourced documents
    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        this.workspaceIndex.indexDocument(uri, doc.getText());
        this.astWorkspace.buildAndAnalyze(uri, doc.getText(), doc.version);
      }
    }

    // Re-index all disk-sourced documents (skip those already in LSP store)
    for (const [uri, text] of this.diskContent) {
      if (!this.documentStore.has(uri)) {
        this.workspaceIndex.indexDocument(uri, text);
        this.astWorkspace.buildAndAnalyze(uri, text, 0);
      }
    }
  }

  /**
   * Refresh the entire workspace — re-scan from disk and re-sync open docs.
   * Called by the knot/refreshDocuments request.
   */
  private refreshWorkspace(): void {
    this.diskContent.clear();

    // Re-scan all roots from disk
    for (const root of this.workspaceRoots) {
      this.scanRoot(root, this.excludePatterns);
    }

    // Re-sync open documents (they take priority over disk)
    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        this.indexContent(uri, doc.getText());
      }
    }

    // Full re-index
    this.reindexAll();

    // Re-detect format
    this.detectWorkspaceFormat();
    this.notifyFormatChanged();

    // Re-publish diagnostics for all known URIs
    for (const uri of this.getAllKnownUris()) {
      this.publishDiagnostics(uri);
    }

    this.sendServerStatus('ready');
  }

  // ─── Utility ─────────────────────────────────────────────────

  /**
   * Get ALL known URIs — both LSP-sourced (open) and disk-sourced.
   */
  private getAllKnownUris(): string[] {
    const lspUris = this.documentStore.getUris();
    const diskUris = Array.from(this.diskContent.keys());
    return [...new Set([...lspUris, ...diskUris])];
  }

  /**
   * Compute completion trigger characters from all loaded format modules.
   * Always includes '[' for passage links (Twine engine level).
   * Adds format-specific macro/variable trigger chars from loaded formats.
   */
  private computeTriggerCharacters(): string[] {
    const chars = new Set<string>();

    // Always include '[' for [[link]] completion (Twine engine level)
    chars.add('[');

    // Collect trigger chars from all loaded format modules
    for (const formatId of this.formatRegistry.getAvailableFormatIds()) {
      const format = this.formatRegistry.getFormat(formatId);
      if (format) {
        // Macro trigger char = first character of the open delimiter
        const macroOpen = format.macroDelimiters.open;
        if (macroOpen.length > 0) {
          chars.add(macroOpen[0]);
        }
        // Variable trigger chars
        if (format.variables) {
          for (const ch of format.variables.triggerChars) {
            chars.add(ch);
          }
        }
      }
    }

    return Array.from(chars);
  }

  // ─── Configuration Pull ────────────────────────────────────────

  /**
   * Pull configuration from the client using workspace/configuration.
   * This ensures we get settings even if the initial push notification
   * was missed or the client uses a pull-based configuration model.
   */
  private async pullConfiguration(): Promise<void> {
    try {
      const config = await this.connection.workspace.getConfiguration([
        { section: 'knot' },
      ]);
      if (config && config.length > 0 && config[0]) {
        this.onConfigurationChange({ settings: { knot: config[0] } });
      }
    } catch {
      // workspace/configuration not supported — rely on push notifications
    }
  }

  // ─── Format Change Notification ─────────────────────────────────

  /**
   * Notify the client that the active format has changed.
   * The client uses this to update the status bar.
   */
  private notifyFormatChanged(): void {
    const activeFormat = this.formatRegistry.getActiveFormat();
    this.connection.sendNotification(FormatChangedNotification.type, {
      formatId: activeFormat.formatId,
      formatName: activeFormat.displayName,
    });
    this.connection.console.info(`Active format changed to: ${activeFormat.formatId} (${activeFormat.displayName})`);

    // Also send full server status update
    this.sendServerStatus('ready');
  }

  /**
   * Send structured server status to the client.
   * The client uses this to update the status bar and for the
   * "Show Status" command. This is THE server-side debugging
   * mechanism — every significant state change triggers this.
   */
  private sendServerStatus(state: 'initialized' | 'indexing' | 'ready' | 'error'): void {
    const activeFormat = this.formatRegistry.getActiveFormat();
    this.connection.sendNotification(ServerStatusNotification.type, {
      state,
      formatId: activeFormat.formatId,
      formatName: activeFormat.displayName,
      passageCount: this.workspaceIndex.size,
      openDocuments: this.documentStore.getUris().length,
      message: state === 'error' ? 'Server encountered an error' : undefined,
    });
  }
}
