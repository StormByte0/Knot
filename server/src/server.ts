/**
 * Knot v2 — Server Entry Point
 *
 * This is the main entry point for the language server process.
 * It creates the server connection and starts listening.
 *
 * Promises:
 *   - Stdio-based LSP server startup
 *   - Delegates to LspServer for all protocol handling
 *
 * Imports:
 *   - ./lspServer
 */

import { createConnection, ProposedFeatures, InitializeParams, TextDocumentSyncKind } from 'vscode-languageserver/node';
import { LspServer } from './lspServer';

const connection = createConnection(ProposedFeatures.all);

const server = new LspServer(connection);

connection.onInitialize((params: InitializeParams) => {
  return server.initialize(params);
});

connection.onInitialized(() => {
  server.initialized();
});

connection.listen();
