/**
 * Knot v2 — LSP Server
 *
 * Full LSP server implementation. Orchestrates all core modules,
 * handlers, and the format system. Every LSP request flows through
 * format-agnostic handlers that delegate to the active FormatModule
 * via the FormatRegistry.
 *
 * Data flow guarantee:
 *   LSP request → handler → core module → formatRegistry → FormatModule
 *   No handler or core module ever imports from format-specific directories.
 */

import {
  Connection,
  InitializeParams,
  InitializeResult,
  TextDocumentSyncKind,
  DidChangeConfigurationParams,
  Diagnostic,
  DiagnosticSeverity,
  Range,
  Position,
  CompletionItem,
  CompletionItemKind,
  InsertTextFormat,
  Hover,
  MarkupKind,
  Location,
  DocumentSymbol,
  SymbolKind as LspSymbolKind,
  WorkspaceSymbol,
  DocumentLink,
  CodeAction,
  CodeActionKind,
  TextEdit,
  WorkspaceEdit,
  SemanticTokens,
  SemanticTokensBuilder,
  SemanticTokenTypes,
  PrepareRenameResult,
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

// ─── Semantic Token Legend ──────────────────────────────────────

const TOKEN_TYPES: string[] = [
  SemanticTokenTypes.function,   // macros
  SemanticTokenTypes.class,      // passages
  SemanticTokenTypes.variable,   // variables
  SemanticTokenTypes.operator,   // macro delimiters
  SemanticTokenTypes.string,     // strings
  SemanticTokenTypes.number,     // numbers
  SemanticTokenTypes.comment,    // comments
];

const TOKEN_MODIFIERS: string[] = [];

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

    // Register LSP feature handlers
    this.registerFeatureHandlers();

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

  // ─── LSP Feature Handlers ────────────────────────────────────

  private registerFeatureHandlers(): void {
    // Completion
    this.connection.onCompletion(params => this.handleCompletion(params));
    this.connection.onCompletionResolve(params => this.handleCompletionResolve(params));

    // Hover
    this.connection.onHover(params => this.handleHover(params));

    // Definition
    this.connection.onDefinition(params => this.handleDefinition(params));

    // References
    this.connection.onReferences(params => this.handleReferences(params));

    // Rename
    this.connection.onPrepareRename(params => this.handlePrepareRename(params));
    this.connection.onRenameRequest(params => this.handleRename(params));

    // Symbols
    this.connection.onDocumentSymbol(params => this.handleDocumentSymbols(params));
    this.connection.onWorkspaceSymbol(params => this.handleWorkspaceSymbols(params));

    // Document Links
    this.connection.onDocumentLinks(params => this.handleDocumentLinks(params));

    // Code Actions
    this.connection.onCodeAction(params => this.handleCodeAction(params));

    // Semantic Tokens
    this.connection.languages.semanticTokens.on(params => this.handleSemanticTokens(params));
  }

  // ─── Completion ──────────────────────────────────────────────

  private handleCompletion(params: any): CompletionItem[] {
    const items: CompletionItem[] = [];
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return items;

    const position = params.position;
    const offset = doc.offsetAt(position);
    const text = doc.getText();
    const line = text.substring(text.lastIndexOf('\n', offset - 1) + 1, offset);

    const format = this.formatRegistry.getActiveFormat();

    // Macro completion: triggered by format-specific macro trigger chars
    if (format.macros !== undefined) {
      const macroPrefix = format.macroDelimiters.open;
      if (macroPrefix && line.endsWith(macroPrefix)) {
        const macroSuffix = format.macroDelimiters.close;
        const closePrefix = format.macroDelimiters.closeTagPrefix ?? '';
        const closeSuffix = format.macroDelimiters.close;
        for (const macro of format.macros.builtins) {
          // Use MacroKind.Changer to determine if the macro needs a close tag/hook
          const isContainer = macro.kind === MacroKind.Changer;
          let insertText: string;
          if (isContainer && closePrefix) {
            // Close-tag style: name $1>><</name>> or format equivalent
            insertText = `${macro.name} \$1${macroSuffix}${closePrefix}${macro.name}${closeSuffix}`;
          } else {
            insertText = `${macro.name} \$0${macroSuffix}`;
          }
          items.push({
            label: macro.name,
            kind: CompletionItemKind.Function,
            detail: macro.category,
            documentation: macro.description,
            insertText,
            insertTextFormat: InsertTextFormat.Snippet,
            sortText: `0${macro.name}`,
          });
        }
        return items;
      }
    }

    // Passage completion: triggered by [[ — always available (core provides this)
    if (line.endsWith('[[')) {
      for (const name of this.workspaceIndex.getAllPassageNames()) {
        items.push({
          label: name,
          kind: CompletionItemKind.Class,
          detail: 'Passage',
          insertText: name + ']]',
          sortText: `1${name}`,
        });
      }
      return items;
    }

    // Variable completion: triggered by format-specific variable trigger chars
    if (format.variables !== undefined) {
      const varTriggerChars = format.variables.triggerChars;
      const lastChar = line.length > 0 ? line[line.length - 1] : '';
      if (varTriggerChars.includes(lastChar)) {
        const allPassages = this.workspaceIndex.getAllPassages();
        const vars = new Set<string>();
        // Look up the sigil definition to determine scope
        const sigilDef = format.variables.sigils.find(s => s.sigil === lastChar);
        const scope = sigilDef?.kind ?? null;
        for (const p of allPassages) {
          if (scope === 'story') {
            p.storyVars.forEach(v => vars.add(v));
          } else if (scope === 'temp') {
            p.tempVars.forEach(v => vars.add(v));
          } else {
            // Unknown scope — show all variables
            p.storyVars.forEach(v => vars.add(v));
            p.tempVars.forEach(v => vars.add(v));
          }
        }
        for (const v of vars) {
          items.push({
            label: v,
            kind: CompletionItemKind.Variable,
            insertText: v,
            sortText: `2${v}`,
          });
        }
        return items;
      }
    }

    return items;
  }

  private handleCompletionResolve(item: CompletionItem): CompletionItem {
    // TODO: Enrich with format-specific documentation
    return item;
  }

  // ─── Hover ───────────────────────────────────────────────────

  private handleHover(params: any): Hover | null {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return null;

    const position = params.position;
    const offset = doc.offsetAt(position);
    const text = doc.getText();
    const format = this.formatRegistry.getActiveFormat();

    // Check if cursor is on a macro
    const macroMatch = this.findMacroAtOffset(text, offset);
    if (macroMatch && format.macros !== undefined) {
      const macro = format.macros.builtins.find(m => m.name === macroMatch.name);
      if (macro) {
        const prefix = format.macroDelimiters.open;
        const suffix = format.macroDelimiters.close;
        const sigText = macro.signatures
          .map(s => `${prefix}${macro.name} ${s.args.map(a => a.name).join(' ')}${suffix}`)
          .join('\n\n');
        return {
          contents: {
            kind: MarkupKind.Markdown,
            value: `**${prefix}${macro.name}${suffix}** — ${macro.category}\n\n${macro.description}\n\n\`\`\`\n${sigText}\n\`\`\``,
          },
        };
      }
    }

    // Check if cursor is on a passage link
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passage = this.workspaceIndex.getPassage(linkTarget);
      if (passage) {
        return {
          contents: {
            kind: MarkupKind.Markdown,
            value: `**${passage.name}** — ${passage.type}\n\nTags: ${passage.tags.length > 0 ? passage.tags.join(', ') : 'none'}`,
          },
        };
      }
    }

    return null;
  }

  // ─── Definition ──────────────────────────────────────────────

  private handleDefinition(params: any): Location | Location[] | null {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);

    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passage = this.workspaceIndex.getPassage(linkTarget);
      if (passage) {
        return Location.create(passage.uri, {
          start: doc.positionAt(passage.startOffset),
          end: doc.positionAt(passage.endOffset),
        });
      }
    }

    // Variable definition
    const text = doc.getText();
    const varAtCursor = this.findVariableAtOffset(text, offset);
    if (varAtCursor) {
      // Find where this variable is first defined (set)
      const allPassages = this.workspaceIndex.getAllPassages();
      const format = this.formatRegistry.getActiveFormat();
      for (const p of allPassages) {
        // Look up the sigil definition to determine variable scope — NEVER hardcode sigil logic
        const sigilDef = format.variables?.sigils.find(s => s.sigil === varAtCursor.sigil);
        const scope = sigilDef?.kind ?? null;
        const varSet = scope === 'story' ? p.storyVars : scope === 'temp' ? p.tempVars : null;
        if (varSet && varSet.has(varAtCursor.name)) {
          return Location.create(p.uri, {
            start: { line: 0, character: 0 },
            end: { line: 0, character: 0 },
          });
        }
      }
    }

    return null;
  }

  // ─── References ──────────────────────────────────────────────

  private handleReferences(params: any): Location[] {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return [];

    const offset = doc.offsetAt(params.position);
    const text = doc.getText();
    const locations: Location[] = [];

    // Find references to a passage name
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      const passages = this.workspaceIndex.getPassagesReferencing(linkTarget);
      for (const p of passages) {
        const pDoc = this.documentStore.get(p.uri);
        if (pDoc) {
          locations.push(Location.create(p.uri, {
            start: pDoc.positionAt(p.startOffset),
            end: pDoc.positionAt(p.endOffset),
          }));
        }
      }
    }

    return locations;
  }

  // ─── Rename ──────────────────────────────────────────────────

  private handlePrepareRename(params: any): PrepareRenameResult | null {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);

    // Can rename passage links
    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (linkTarget) {
      return {
        range: { start: doc.positionAt(offset), end: doc.positionAt(offset) },
        placeholder: linkTarget,
      };
    }

    return null;
  }

  private handleRename(params: any): WorkspaceEdit | null {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return null;

    const offset = doc.offsetAt(params.position);
    const newName = params.newName;

    const linkTarget = this.findLinkTargetAtOffset(offset, uri);
    if (!linkTarget) return null;

    // Find all documents that reference this passage
    const changes: Record<string, TextEdit[]> = {};
    const passages = this.workspaceIndex.getPassagesReferencing(linkTarget);

    for (const p of passages) {
      const pDoc = this.documentStore.get(p.uri);
      if (!pDoc) continue;

      const edits: TextEdit[] = [];

      for (const ref of p.passageRefs) {
        if (ref.target !== linkTarget) continue;

        // For [[ ]] links, we need to replace the target within the link text
        // while preserving display text and separator syntax
        if (ref.kind === PassageRefKind.Link) {
          const linkStart = ref.range.start;
          const linkEnd = ref.range.end;
          const linkText = pDoc.getText().substring(linkStart, linkEnd);

          // Find the target within the link text and replace it
          const format = this.formatRegistry.getActiveFormat();
          const resolved = format.resolveLinkBody(linkText.slice(2, -2));
          if (resolved.target) {
            const newLinkBody = linkText.slice(2, -2).replace(this.escapeRegex(resolved.target), newName);
            const newLink = `[[${newLinkBody}]]`;
            edits.push(TextEdit.replace(
              { start: pDoc.positionAt(linkStart), end: pDoc.positionAt(linkEnd) },
              newLink,
            ));
          }
        } else {
          // For macros and implicit refs, simple target replacement
          edits.push(TextEdit.replace(
            { start: pDoc.positionAt(ref.range.start), end: pDoc.positionAt(ref.range.end) },
            newName,
          ));
        }
      }

      if (edits.length > 0) {
        changes[p.uri] = (changes[p.uri] ?? []).concat(edits);
      }
    }

    return { changes };
  }

  // ─── Document Symbols ────────────────────────────────────────

  private handleDocumentSymbols(params: any): DocumentSymbol[] {
    const uri = params.textDocument.uri;
    const passages = this.workspaceIndex.getPassagesByUri(uri);
    const symbols: DocumentSymbol[] = [];

    for (const p of passages) {
      const doc = this.documentStore.get(p.uri);
      if (!doc) continue;

      const range = {
        start: doc.positionAt(p.startOffset),
        end: doc.positionAt(p.endOffset),
      };

      symbols.push(DocumentSymbol.create(
        p.name,
        p.type,
        LspSymbolKind.Class,
        range,
        range,
      ));
    }

    return symbols;
  }

  // ─── Workspace Symbols ───────────────────────────────────────

  private handleWorkspaceSymbols(params: any): WorkspaceSymbol[] {
    const query = params.query.toLowerCase();
    const symbols: WorkspaceSymbol[] = [];

    for (const p of this.workspaceIndex.getAllPassages()) {
      if (p.name.toLowerCase().includes(query)) {
        symbols.push({
          name: p.name,
          kind: LspSymbolKind.Class,
          location: {
            uri: p.uri,
          },
          containerName: p.type,
        });
      }
    }

    return symbols;
  }

  // ─── Document Links ──────────────────────────────────────────

  private handleDocumentLinks(params: any): DocumentLink[] {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return [];

    const links: DocumentLink[] = [];
    const passages = this.workspaceIndex.getPassagesByUri(uri);

    // Use passageRefs from the index (format-driven, single source of truth)
    for (const passage of passages) {
      for (const ref of passage.passageRefs) {
        if (ref.kind === PassageRefKind.Link && ref.linkKind === LinkKind.Passage) {
          if (this.workspaceIndex.hasPassage(ref.target)) {
            const targetPassage = this.workspaceIndex.getPassage(ref.target)!;
            const targetDoc = this.documentStore.get(targetPassage.uri);
            if (targetDoc) {
              links.push(DocumentLink.create(
                {
                  start: doc.positionAt(ref.range.start),
                  end: doc.positionAt(ref.range.end),
                },
                `${targetPassage.uri}#L1`,
              ));
            }
          }
        }
      }
    }

    return links;
  }

  // ─── Code Actions ────────────────────────────────────────────

  private handleCodeAction(params: any): CodeAction[] {
    const actions: CodeAction[] = [];
    const uri = params.textDocument.uri;

    for (const diag of params.context.diagnostics) {
      // Quick fix: create unknown passage
      if (diag.message.startsWith('Unknown passage:')) {
        const passageName = diag.message.match(/"([^"]+)"/)?.[1];
        if (passageName) {
          actions.push(CodeAction.create(
            `Create passage "${passageName}"`,
            {
              changes: {
                [uri]: [TextEdit.insert(
                  { line: 0, character: 0 },
                  `:: ${passageName}\n\n`,
                )],
              },
            },
            CodeActionKind.QuickFix,
          ));
        }
      }
    }

    return actions;
  }

  // ─── Semantic Tokens ─────────────────────────────────────────

  private handleSemanticTokens(params: any): SemanticTokens {
    const uri = params.textDocument.uri;
    const doc = this.documentStore.get(uri);
    if (!doc) return { data: [] };

    const builder = new SemanticTokensBuilder();
    const text = doc.getText();
    const format = this.formatRegistry.getActiveFormat();

    // Tokenize passage headers (Twine engine level — always :: headers)
    const headerRegex = /^::\s*([^\[\]\n]+)/gm;
    let match: RegExpExecArray | null;
    while ((match = headerRegex.exec(text)) !== null) {
      const start = doc.positionAt(match.index);
      // Push passage name as "class" token
      builder.push(start.line, start.character + 3, match[1].trim().length, 1, 0); // 1 = class
    }

    // Tokenize macros and variables using format-driven body lexing
    // lexBody is required on every FormatModule, so this always runs
    const passageHeaderRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    while ((passageMatch = passageHeaderRegex.exec(text)) !== null) {
      const bodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', bodyStart);
      const bodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;
      const bodyText = text.substring(bodyStart, bodyEnd);

      const tokens = format.lexBody(bodyText, bodyStart);
      for (const token of tokens) {
        if (token.typeId === 'macro-call') {
          // Only highlight known macros
          const isKnown = format.macros?.builtins.some(m => m.name === (token.macroName ?? '')) ?? false;
          if (isKnown) {
            const pos = doc.positionAt(token.range.start);
            builder.push(pos.line, pos.character, token.range.end - token.range.start, 0, 0); // 0 = function
          }
        } else if (token.typeId === 'macro-close') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 0, 0); // 0 = function
        } else if (token.typeId === 'variable') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 2, 0); // 2 = variable
        } else if (token.typeId === 'hook-open' || token.typeId === 'hook-close') {
          const pos = doc.positionAt(token.range.start);
          builder.push(pos.line, pos.character, token.range.end - token.range.start, 3, 0); // 3 = operator
        }
      }
    }

    return builder.build();
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
      // BUG FIX: Pass the body content (JSON), not the passage name
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

  // ─── Diagnostics Publishing ──────────────────────────────────

  private publishDiagnostics(uri: string): void {
    const results = this.diagnosticEngine.computeDiagnostics(uri);
    const diagnostics: Diagnostic[] = results.map(r => ({
      severity: this.severityFromString(r.severity),
      message: r.message,
      range: r.range
        ? Range.create(
            Position.create(0, r.range.start),
            Position.create(0, r.range.end),
          )
        : Range.create(Position.create(0, 0), Position.create(0, 0)),
      source: 'knot',
    }));

    this.connection.sendDiagnostics({ uri, diagnostics });
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

  // ─── Text Search Helpers ─────────────────────────────────────

  private findMacroAtOffset(text: string, offset: number): { name: string } | null {
    // Use format-driven lexing to find macro at offset
    const format = this.formatRegistry.getActiveFormat();

    // Find which passage body contains the offset
    const headerRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    let bodyStart = 0;
    let bodyEnd = text.length;

    while ((passageMatch = headerRegex.exec(text)) !== null) {
      const nextBodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', nextBodyStart);
      const nextBodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;

      if (offset >= nextBodyStart && offset <= nextBodyEnd) {
        bodyStart = nextBodyStart;
        bodyEnd = nextBodyEnd;
        break;
      }
    }

    // Lex the passage body with the format module (lexBody takes baseOffset)
    const bodyText = text.substring(bodyStart, bodyEnd);
    const tokens = format.lexBody(bodyText, bodyStart);

    for (const token of tokens) {
      if ((token.typeId === 'macro-call' || token.typeId === 'macro-close') && token.macroName) {
        if (offset >= token.range.start && offset <= token.range.end) {
          return { name: token.macroName };
        }
      }
    }

    return null;
  }

  private findLinkTargetAtOffset(offset: number, uri: string): string | null {
    // Find a passage reference that contains the offset using the index
    const passages = this.workspaceIndex.getPassagesByUri(uri);
    for (const passage of passages) {
      for (const ref of passage.passageRefs) {
        if (offset >= ref.range.start && offset <= ref.range.end) {
          if (ref.linkKind === LinkKind.Passage || ref.kind === PassageRefKind.Macro || ref.kind === PassageRefKind.Implicit) {
            return ref.target;
          }
        }
      }
    }
    return null;
  }

  private findVariableAtOffset(text: string, offset: number): { sigil: string; name: string } | null {
    // Use format-driven lexing to find variable at offset
    const format = this.formatRegistry.getActiveFormat();

    // Find which passage body contains the offset
    const headerRegex = /^::[^\n]*\n/gm;
    let passageMatch: RegExpExecArray | null;
    let bodyStart = 0;
    let bodyEnd = text.length;

    while ((passageMatch = headerRegex.exec(text)) !== null) {
      const nextBodyStart = passageMatch.index + passageMatch[0].length;
      const nextHeader = text.indexOf('\n::', nextBodyStart);
      const nextBodyEnd = nextHeader >= 0 ? nextHeader + 1 : text.length;

      if (offset >= nextBodyStart && offset <= nextBodyEnd) {
        bodyStart = nextBodyStart;
        bodyEnd = nextBodyEnd;
        break;
      }
    }

    // Lex the passage body with the format module (lexBody takes baseOffset)
    const bodyText = text.substring(bodyStart, bodyEnd);
    const tokens = format.lexBody(bodyText, bodyStart);

    for (const token of tokens) {
      if (token.typeId === 'variable' && token.varName) {
        if (offset >= token.range.start && offset <= token.range.end) {
          return { sigil: token.varSigil ?? '$', name: token.varName };
        }
      }
    }

    return null;
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

  private severityFromString(severity: string): DiagnosticSeverity {
    switch (severity) {
      case 'error': return DiagnosticSeverity.Error;
      case 'warning': return DiagnosticSeverity.Warning;
      case 'info': return DiagnosticSeverity.Information;
      case 'hint': return DiagnosticSeverity.Hint;
      default: return DiagnosticSeverity.Warning;
    }
  }

  private escapeRegex(str: string): string {
    return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
}
