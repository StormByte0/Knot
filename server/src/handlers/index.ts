/**
 * Knot v2 — Handlers Re-exports
 *
 * Central re-export point for all LSP handler registrations.
 * The registerHandlers() function creates handler instances
 * with their dependencies and wires them to the LSP connection.
 *
 * Data flow guarantee:
 *   LSP request → handler → core module → formatRegistry → FormatModule
 *   No handler or core module ever imports from format-specific directories.
 */

import { Connection } from 'vscode-languageserver/node';
import { FormatRegistry } from '../formats/formatRegistry';
import { WorkspaceIndex } from '../core/workspaceIndex';
import { DocumentStore } from '../core/documentStore';
import { ReferenceIndex } from '../core/referenceIndex';
import { DiagnosticEngine } from '../core/diagnosticEngine';

import { CompletionHandler, CompletionContext } from './completions';
import { HoverHandler, HoverContext } from './hover';
import { DefinitionHandler, DefinitionContext } from './definition';
import { ReferencesHandler, ReferencesContext } from './references';
import { RenameHandler, RenameContext } from './rename';
import { SymbolsHandler, SymbolsContext } from './symbols';
import { DiagnosticsHandler, DiagnosticsContext } from './diagnostics';
import { CodeActionHandler } from './codeActions';
import { DocumentLinksHandler, DocumentLinksContext } from './documentLinks';
import { SemanticTokensHandler, SemanticTokensContext, TOKEN_TYPES, TOKEN_MODIFIERS } from './semanticTokens';

// Re-export handler classes for direct use if needed
export { CompletionHandler } from './completions';
export { HoverHandler } from './hover';
export { DefinitionHandler } from './definition';
export { ReferencesHandler } from './references';
export { RenameHandler } from './rename';
export { SymbolsHandler } from './symbols';
export { DiagnosticsHandler } from './diagnostics';
export { CodeActionHandler } from './codeActions';
export { DocumentLinksHandler } from './documentLinks';
export { SemanticTokensHandler, TOKEN_TYPES, TOKEN_MODIFIERS } from './semanticTokens';

/**
 * Shared dependency context for all handlers.
 * Passed to registerHandlers() so each handler gets what it needs.
 */
export interface HandlerDependencies {
  connection: Connection;
  formatRegistry: FormatRegistry;
  workspaceIndex: WorkspaceIndex;
  documentStore: DocumentStore;
  referenceIndex: ReferenceIndex;
  diagnosticEngine: DiagnosticEngine;
}

/**
 * Register all LSP handlers on the given connection.
 * This is the single entry point called by LspServer.
 *
 * Handlers receive format-agnostic context objects for
 * format-specific resolution via capability bags on FormatModule.
 */
export function registerHandlers(deps: HandlerDependencies): void {
  const { connection, formatRegistry, workspaceIndex, documentStore, referenceIndex, diagnosticEngine } = deps;

  // ── Completion ─────────────────────────────────────────────────
  const completionHandler = new CompletionHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onCompletion(params => completionHandler.handleCompletion(params));
  connection.onCompletionResolve(item => completionHandler.handleCompletionResolve(item));

  // ── Hover ──────────────────────────────────────────────────────
  const hoverHandler = new HoverHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onHover(params => hoverHandler.handleHover(params));

  // ── Definition ─────────────────────────────────────────────────
  const definitionHandler = new DefinitionHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onDefinition(params => definitionHandler.handleDefinition(params));

  // ── References ─────────────────────────────────────────────────
  const referencesHandler = new ReferencesHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onReferences(params => referencesHandler.handleReferences(params));

  // ── Rename ─────────────────────────────────────────────────────
  const renameHandler = new RenameHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onPrepareRename(params => renameHandler.handlePrepareRename(params));
  connection.onRenameRequest(params => renameHandler.handleRename(params));

  // ── Symbols ────────────────────────────────────────────────────
  const symbolsHandler = new SymbolsHandler({
    workspaceIndex,
    documentStore,
    formatRegistry,
  });
  connection.onDocumentSymbol(params => symbolsHandler.handleDocumentSymbols(params));
  connection.onWorkspaceSymbol(params => symbolsHandler.handleWorkspaceSymbols(params));

  // ── Document Links ─────────────────────────────────────────────
  const documentLinksHandler = new DocumentLinksHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onDocumentLinks(params => documentLinksHandler.handleDocumentLinks(params));

  // ── Code Actions ───────────────────────────────────────────────
  const codeActionHandler = new CodeActionHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.onCodeAction(params => codeActionHandler.handleCodeAction(params));

  // ── Semantic Tokens ────────────────────────────────────────────
  const semanticTokensHandler = new SemanticTokensHandler({
    formatRegistry,
    workspaceIndex,
    documentStore,
  });
  connection.languages.semanticTokens.on(params => semanticTokensHandler.handleSemanticTokens(params));

  // ── Diagnostics (used internally, not registered as LSP handler) ─
  // Diagnostics are pushed via connection.sendDiagnostics(), not pulled.
  // The DiagnosticsHandler is returned for the server to use in publishDiagnostics().
}

/**
 * Create a DiagnosticsHandler instance with the given dependencies.
 * The server uses this to compute diagnostics and publish them.
 */
export function createDiagnosticsHandler(deps: HandlerDependencies): DiagnosticsHandler {
  return new DiagnosticsHandler({
    diagnosticEngine: deps.diagnosticEngine,
    documentStore: deps.documentStore,
  });
}
