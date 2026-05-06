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
 *
 * All LSP feature logic lives in handler modules.
 */

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

  private workspaceRoot: string | undefined;
  private settings: Record<string, unknown> = {};

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
    this.workspaceRoot = params.rootUri ?? params.rootPath ?? undefined;

    // Load all built-in format modules
    this.formatRegistry.loadBuiltinFormats();

    // Auto-detect format or default to fallback
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

    // Register configuration change handler
    this.connection.onDidChangeConfiguration(this.onConfigurationChange.bind(this));

    // Try to detect format from workspace StoryData passage
    this.detectWorkspaceFormat();

    this.connection.console.info('Knot v2 language server initialized');
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
      this.documentStore.set(doc.uri, doc);

      // Pre-scan for StoryData BEFORE full indexing (Twine engine level detection)
      // This ensures the correct format module is active when we parse the document
      const text = doc.getText();
      const detectedFormat = this.preScanStoryData(text);
      if (detectedFormat && detectedFormat !== this.formatRegistry.getActiveFormat().formatId) {
        this.formatRegistry.setActiveFormat(detectedFormat);
        this.connection.console.info(`Detected story format from new document: ${detectedFormat}`);
        // Re-index any already-open documents with the new format
        this.reindexAll();
      }

      this.workspaceIndex.indexDocument(doc.uri, text);

      // Build AST and run full analysis pipeline
      this.astWorkspace.buildAndAnalyze(doc.uri, text, doc.version);

      this.publishDiagnostics(doc.uri);
    });

    this.connection.onDidChangeTextDocument(params => {
      const doc = this.documentStore.get(params.textDocument.uri);
      if (!doc) return;

      // Apply incremental changes
      TextDocument.update(doc, params.contentChanges, params.textDocument.version);
      this.documentStore.set(doc.uri, doc);
      this.workspaceIndex.reindexDocument(doc.uri, doc.getText());

      // Rebuild AST for the changed document
      this.astWorkspace.invalidate(doc.uri);
      this.astWorkspace.buildAndAnalyze(doc.uri, doc.getText(), doc.version);

      this.publishDiagnostics(doc.uri);
    });

    this.connection.onDidCloseTextDocument(params => {
      this.documentStore.delete(params.textDocument.uri);
      this.astWorkspace.invalidate(params.textDocument.uri);
    });
  }

  // ─── Configuration ───────────────────────────────────────────

  private onConfigurationChange(params: DidChangeConfigurationParams): void {
    const settings = params.settings?.knot ?? {};
    this.settings = settings;

    // Update diagnostic settings
    this.diagnosticEngine.updateSettings({
      unknownPassage: settings.lint?.unknownPassage,
      unknownMacro: settings.lint?.unknownMacro,
      duplicatePassage: settings.lint?.duplicatePassage,
      typeMismatch: settings.lint?.typeMismatch,
      unreachablePassage: settings.lint?.unreachablePassage,
      containerStructure: settings.lint?.containerStructure,
      deprecatedMacro: settings.lint?.deprecatedMacro,
      missingArgument: settings.lint?.missingArgument,
      invalidAssignment: settings.lint?.invalidAssignment,
    });

    // Check for format change
    const formatId = settings.format?.activeFormat;
    if (formatId && formatId !== this.formatRegistry.getActiveFormat().formatId) {
      this.formatRegistry.setActiveFormat(formatId);
      // Re-index with new format
      this.reindexAll();
    }

    // Re-publish diagnostics
    for (const uri of this.documentStore.getUris()) {
      this.publishDiagnostics(uri);
    }
  }

  // ─── Diagnostics Publishing ──────────────────────────────────
  // Uses the DiagnosticsHandler from handlers/ — the handler knows
  // how to convert DiagnosticResult[] to LSP Diagnostic[] with
  // correct range mapping (fixes the old Position.create(0, offset) bug).

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
    const storyDataHeader = /^::\s*StoryData(?:\s*\[([^\]]*)\])?\s*$/m;
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
   * If found, sets the active format and re-indexes.
   * If NOT found, shows a warning and suggests using knot.selectFormat.
   *
   * This is Twine engine behavior — StoryData detection is universal,
   * not format-specific. Every Twine story has a StoryData passage.
   */
  private detectWorkspaceFormat(): void {
    // First try: look for StoryData passage in the already-indexed workspace
    const storyDataPassages = this.workspaceIndex.getPassagesByType(PassageType.StoryData);
    if (storyDataPassages.length > 0) {
      const storyData = storyDataPassages[0];
      const detected = this.formatRegistry.detectFromStoryData(storyData.body ?? storyData.name);
      if (detected.formatId !== 'fallback') {
        this.formatRegistry.setActiveFormat(detected.formatId);
        this.connection.console.info(`Detected story format: ${detected.formatId}`);
        // Re-index with the correct format module now that we know the format
        this.reindexAll();
        return;
      }
    }

    // Second try: pre-scan all open documents for StoryData
    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        const detectedFormat = this.preScanStoryData(doc.getText());
        if (detectedFormat) {
          this.formatRegistry.setActiveFormat(detectedFormat);
          this.connection.console.info(`Detected story format via pre-scan: ${detectedFormat}`);
          this.reindexAll();
          return;
        }
      }
    }

    // No StoryData found — show warning and suggest manual format selection
    this.connection.console.warn(
      'No StoryData passage found. Cannot auto-detect story format. ' +
      'Use "Knot: Select Story Format" command to set the format manually.'
    );
    // Send a diagnostic hint to the first open document suggesting format selection
    for (const uri of this.documentStore.getUris()) {
      this.connection.sendDiagnostics({
        uri,
        diagnostics: [{
          severity: DiagnosticSeverity.Information,
          message: 'No StoryData passage found. Story format auto-detection unavailable. Use "Knot: Select Story Format" to set the format manually.',
          range: Range.create(Position.create(0, 0), Position.create(0, 0)),
          source: 'knot',
          code: 'no-storydata',
        }],
      });
      // Only show on the first document
      break;
    }
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

    for (const uri of this.documentStore.getUris()) {
      const doc = this.documentStore.get(uri);
      if (doc) {
        this.workspaceIndex.indexDocument(uri, doc.getText());
        this.astWorkspace.buildAndAnalyze(uri, doc.getText(), doc.version);
      }
    }
  }

  // ─── Utility ─────────────────────────────────────────────────

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
}
