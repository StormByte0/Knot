/**
 * Knot v2 — Handlers Re-exports
 *
 * Central re-export point for all LSP handler registrations.
 */

// TODO: export { CompletionHandler } from './completions';
// TODO: export { DefinitionHandler } from './definition';
// TODO: export { HoverHandler } from './hover';
// TODO: export { ReferencesHandler } from './references';
// TODO: export { RenameHandler } from './rename';
// TODO: export { SymbolsHandler } from './symbols';
// TODO: export { DiagnosticsHandler } from './diagnostics';
// TODO: export { CodeActionHandler } from './codeActions';
// TODO: export { DocumentLinksHandler } from './documentLinks';

export {};

/**
 * Register all LSP handlers on the given connection.
 * This is the single entry point called by LspServer.
 *
 * Handlers receive a FormatRegistry instance for format-specific
 * resolution via capability bags on FormatModule.
 */
// export function registerHandlers(connection: Connection, formatRegistry: FormatRegistry, ...deps): void {
//   TODO: instantiate and register each handler
// }
