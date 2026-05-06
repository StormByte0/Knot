/**
 * Knot v2 — Diagnostics Handler
 *
 * Format-agnostic diagnostics handler.
 * Delegates to core DiagnosticEngine which orchestrates
 * both core and format-specific rules.
 *
 * Imports:
 *   - core/diagnosticEngine (diagnostic orchestration)
 *   - core/documentStore (document content)
 *
 * MUST NOT import from: formats/<name>/
 */

import {
  Diagnostic,
  DiagnosticSeverity,
  Range,
  Position,
} from 'vscode-languageserver/node';
import { TextDocument } from 'vscode-languageserver-textdocument';
import { DiagnosticEngine } from '../core/diagnosticEngine';
import { DocumentStore } from '../core/documentStore';

export interface DiagnosticsContext {
  diagnosticEngine: DiagnosticEngine;
  documentStore: DocumentStore;
}

export class DiagnosticsHandler {
  private ctx: DiagnosticsContext;

  constructor(ctx: DiagnosticsContext) {
    this.ctx = ctx;
  }

  /**
   * Compute diagnostics for a document.
   * Returns LSP Diagnostic[] suitable for publishing.
   */
  computeDiagnostics(uri: string): Diagnostic[] {
    const doc = this.ctx.documentStore.get(uri);
    const results = this.ctx.diagnosticEngine.computeDiagnostics(uri);

    return results.map(r => {
      let range: Range;

      if (r.range) {
        // Convert SourceRange to LSP Range using document position mapping
        // BUG FIX: Was using Position.create(0, offset) which put all diagnostics
        // on line 0 with wrong character positions. Now uses doc.positionAt() for
        // correct line/column mapping.
        if (doc) {
          range = Range.create(
            doc.positionAt(r.range.start),
            doc.positionAt(r.range.end),
          );
        } else {
          // Fallback if document not available — should not happen in practice
          range = Range.create(
            Position.create(0, r.range.start),
            Position.create(0, r.range.end),
          );
        }
      } else {
        range = Range.create(Position.create(0, 0), Position.create(0, 0));
      }

      return {
        severity: this.severityFromString(r.severity),
        message: r.message,
        range,
        source: 'knot',
      };
    });
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
}
