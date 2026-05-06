/**
 * Knot v2 — Code Action Handler
 *
 * Format-agnostic code action handler.
 * Delegates format-specific quick fixes through the format registry.
 *
 * Imports:
 *   - formats/formatRegistry (format module resolution)
 *   - formats/_types (FormatModule, capability bags)
 *
 * MUST NOT import from: formats/<name>/
 */

import { FormatRegistry } from '../formats/formatRegistry';
import type { FormatModule } from '../formats/_types';

export class CodeActionHandler {
  private formatRegistry: FormatRegistry;

  constructor(formatRegistry: FormatRegistry) {
    this.formatRegistry = formatRegistry;
  }

  /**
   * Handle a code action request.
   * Provides quick fixes based on diagnostics.
   */
  async handleCodeAction(
    uri: string,
    range: { start: { line: number; character: number }; end: { line: number; character: number } },
  ): Promise<unknown[]> {
    const format = this.formatRegistry.getActiveFormat();

    // TODO: Check if format has diagnostics capability bag
    // if (!format.diagnostics) return [];

    // TODO: Generate quick fixes for common issues
    throw new Error('TODO: implement handleCodeAction()');
  }
}
