/**
 * Knot v2 — Snowman 2 Custom Diagnostics
 *
 * Custom diagnostic check for Snowman-specific issues that
 * can't be expressed declaratively.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { DiagnosticResult, DiagnosticCheckContext } from '../_types';

// ═══════════════════════════════════════════════════════════════════
// CUSTOM DIAGNOSTIC CHECK
// ═══════════════════════════════════════════════════════════════════

/**
 * Custom diagnostic check for Snowman-specific issues that
 * can't be expressed declaratively:
 *   - Unclosed template blocks (<% without matching %>)
 *   - Malformed [[ ]] link syntax
 */
export function customDiagnosticCheck(context: DiagnosticCheckContext): readonly DiagnosticResult[] {
  const results: DiagnosticResult[] = [];
  const { body, bodyTokens } = context;

  // ── Check for unclosed template blocks ──────────────────────
  // A template token that spans to end-of-input with no %> is malformed.
  for (const token of bodyTokens) {
    if (
      (token.typeId === 'template-block' || token.typeId === 'template-expression') &&
      !token.text.endsWith('%>')
    ) {
      results.push({
        ruleId: 'invalid-template-syntax',
        message: `Unclosed template block: missing closing '%>'`,
        severity: 'error',
        range: token.range,
      });
    }
  }

  // ── Check for malformed [[ ]] links ────────────────────────
  // Find [[ that don't have a matching ]] before the next [[ or EOF
  let i = 0;
  while (i < body.length) {
    const openIdx = body.indexOf('[[', i);
    if (openIdx === -1) break;

    const closeIdx = body.indexOf(']]', openIdx + 2);
    if (closeIdx === -1) {
      // Unclosed link
      results.push({
        ruleId: 'invalid-link-syntax',
        message: "Unclosed link: missing closing ']]'",
        severity: 'error',
        range: { start: openIdx, end: body.length },
      });
      break; // No more links possible
    }

    // Check for empty link content: [[ ]]
    const linkContent = body.slice(openIdx + 2, closeIdx);
    if (linkContent.trim() === '') {
      results.push({
        ruleId: 'invalid-link-syntax',
        message: 'Empty link: [[ ]] must contain a passage name',
        severity: 'warning',
        range: { start: openIdx, end: closeIdx + 2 },
      });
    }

    i = closeIdx + 2;
  }

  return results;
}
