/**
 * Knot v2 — Chapbook 2 Custom Diagnostics
 *
 * Custom diagnostic check for Chapbook-specific issues.
 * Catches unknown insert names, malformed insert syntax, front matter
 * issues, and unknown modifiers.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { DiagnosticCheckContext, DiagnosticResult } from '../_types';

import { KNOWN_INSERT_NAMES, KNOWN_MODIFIER_NAMES } from './inserts-index';
import { FRONT_MATTER_RE, MODIFIER_RE } from './lexer';

/**
 * Custom diagnostic check for Chapbook-specific issues.
 * Catches unknown insert names, malformed insert syntax, front matter
 * issues, and unknown modifiers.
 */
export function customDiagCheck(context: DiagnosticCheckContext): readonly DiagnosticResult[] {
  const results: DiagnosticResult[] = [];
  const body = context.body;

  // ── Front matter: unclosed --- block ─────────────────────────
  if (body.startsWith('---')) {
    const fmMatch = body.match(FRONT_MATTER_RE);
    if (!fmMatch) {
      // Opening --- found but no matching closing ---
      results.push({
        ruleId: 'unclosed-front-matter',
        message: 'Unclosed front matter block: missing closing ---',
        severity: 'error',
        range: { start: 0, end: 3 },
      });
    } else {
      // Check for malformed variable assignments in front matter
      const fmContent = fmMatch[1];
      const lines = fmContent.split('\n');
      let lineOffset = 4; // after opening ---\n
      for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed === '' || trimmed.startsWith('#') || trimmed.startsWith('//')) {
          // Skip blank lines and comments
          lineOffset += line.length + 1;
          continue;
        }
        // Valid front matter lines: var.name: value, temp.name: value
        if (/^(var|temp)\.[a-zA-Z_][a-zA-Z0-9_]*\s*:/.test(trimmed)) {
          // Valid assignment
        } else if (/^[a-zA-Z_][a-zA-Z0-9_]*\s*:/.test(trimmed)) {
          // Could be a valid YAML key (not var/temp but still allowed in front matter)
        } else if (trimmed !== '') {
          // Unrecognized line in front matter
          results.push({
            ruleId: 'malformed-front-matter',
            message: `Unrecognized front matter entry: "${trimmed}"`,
            severity: 'warning',
            range: { start: lineOffset, end: lineOffset + line.length },
          });
        }
        lineOffset += line.length + 1;
      }
    }
  }

  // ── Match all {...} inserts in the body ──────────────────────
  const INSERT_RE = /\{([^}]+)\}/g;
  let m: RegExpExecArray | null;
  while ((m = INSERT_RE.exec(body)) !== null) {
    const inner = m[1].trim();

    // Skip closing inserts like {endif}, {end reveal}, {end section}
    if (inner.startsWith('end ') || inner === 'endif') continue;

    // Extract the insert name — first word-like token
    const nameMatch = inner.match(/^([\w\s]+?)(?:[\s,:]|$)/);
    const insertName = nameMatch ? nameMatch[1].trim() : inner.split(/[\s,:]/)[0];

    // Check if it's a known insert
    if (!KNOWN_INSERT_NAMES.has(insertName)) {
      results.push({
        ruleId: 'unknown-insert',
        message: `Unknown insert: {${insertName}}`,
        severity: 'warning',
        range: { start: m.index, end: m.index + m[0].length },
      });
    }

    // Check for malformed syntax: insert with unmatched quotes
    const quotes = (inner.match(/['"]/g) || []).length;
    if (quotes % 2 !== 0) {
      results.push({
        ruleId: 'invalid-insert-syntax',
        message: `Malformed insert syntax: unmatched quote in {${inner}}`,
        severity: 'error',
        range: { start: m.index, end: m.index + m[0].length },
      });
    }
  }

  // ── Match [modifier] bracket syntax ─────────────────────────
  MODIFIER_RE.lastIndex = 0;
  let modMatch: RegExpExecArray | null;
  while ((modMatch = MODIFIER_RE.exec(body)) !== null) {
    const modName = modMatch[1].trim();
    // Skip [[ ]] link-like patterns (shouldn't match due to regex, but be safe)
    if (modName.includes('[') || modName.includes(']')) continue;

    if (!KNOWN_MODIFIER_NAMES.has(modName)) {
      results.push({
        ruleId: 'unknown-modifier',
        message: `Unknown modifier: [${modName}]`,
        severity: 'warning',
        range: { start: modMatch.index, end: modMatch.index + modMatch[0].length },
      });
    }
  }

  return results;
}
