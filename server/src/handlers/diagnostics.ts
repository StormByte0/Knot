/**
 * Knot v2 — Diagnostics Handler
 *
 * Format-agnostic diagnostics handler.
 * Delegates to core DiagnosticEngine which orchestrates
 * both core and format-specific rules.
 *
 * Imports:
 *   - core/diagnosticEngine (diagnostic orchestration)
 *
 * MUST NOT import from: formats/<name>/
 */

// TODO: import { DiagnosticEngine } from '../core/diagnosticEngine';

export class DiagnosticsHandler {
  // private diagnosticEngine: DiagnosticEngine;

  /**
   * Compute and publish diagnostics for a document.
   */
  async computeDiagnostics(uri: string): Promise<unknown[]> {
    // TODO: Call diagnosticEngine.computeDiagnostics(uri)
    // TODO: Convert IDiagnosticResult[] to LSP Diagnostic[]
    throw new Error('TODO: implement computeDiagnostics()');
  }
}
