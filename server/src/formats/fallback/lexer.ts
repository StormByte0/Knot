/**
 * Knot v2 — Fallback Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Fallback (Basic Twee) format.
 *
 * In fallback mode, the entire body is plain text — the lexer returns
 * a single text token + EOF. Passage reference extraction finds only
 * [[ ]] links (the universal Twee 3 link syntax).
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Fallback passage body into adapter-specific tokens.
 *
 * In fallback mode, the entire body is plain text — returns a single
 * text token followed by EOF.
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  if (!input) {
    return [{ typeId: 'eof', text: '', range: { start: baseOffset, end: baseOffset } }];
  }
  const tokens: BodyToken[] = [];
  // In fallback mode, the entire body is plain text
  if (input.length > 0) {
    tokens.push({
      typeId: 'text',
      text: input,
      range: { start: baseOffset, end: baseOffset + input.length },
    });
  }
  tokens.push({
    typeId: 'eof',
    text: '',
    range: { start: baseOffset + input.length, end: baseOffset + input.length },
  });
  return tokens;
}

// ═══════════════════════════════════════════════════════════════════
// PASSAGE REFERENCE EXTRACTION
// ═══════════════════════════════════════════════════════════════════

const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Extract all passage references from a passage body.
 * Fallback only handles [[ ]] links — the universal Twee 3 syntax.
 * No macros, no API calls, no implicit patterns.
 */
export function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
  const refs: PassageRef[] = [];
  LINK_RE.lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = LINK_RE.exec(body)) !== null) {
    const rawBody = match[1];
    const resolved = resolveLinkBody(rawBody);

    // Only index passage links (not external URLs)
    if (resolved.target && resolved.kind === LinkKind.Passage) {
      refs.push({
        target: resolved.target,
        kind: PassageRefKind.Link,
        range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
        source: '[[ ]]',
        linkKind: resolved.kind,
      });
    }
  }

  return refs;
}

// ═══════════════════════════════════════════════════════════════════
// LINK RESOLUTION
// ═══════════════════════════════════════════════════════════════════

/**
 * Parse the raw body text inside [[...]] into a structured LinkResolution.
 *
 * Fallback link syntax (basic Twee):
 *   1. Right arrow: text->target
 *   2. Left arrow:  target<-text
 *   3. Simple:      [[target]]
 */
export function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // Basic Twee link syntax: rightmost ->, then leftmost <-
  const rightArrow = rawBody.lastIndexOf('->');
  if (rightArrow >= 0) {
    const target = rawBody.substring(rightArrow + 2).trim();
    const displayText = rawBody.substring(0, rightArrow).trim();
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
    };
  }

  const leftArrow = rawBody.indexOf('<-');
  if (leftArrow >= 0) {
    const target = rawBody.substring(0, leftArrow).trim();
    const displayText = rawBody.substring(leftArrow + 2).trim();
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
    };
  }

  // Simple [[target]]
  const target = rawBody.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}
