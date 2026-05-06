/**
 * Knot v2 — Fallback Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Fallback (Basic Twee) format.
 *
 * In fallback mode, the entire body is plain text EXCEPT for [[ ]] links.
 * The lexer produces: text, link, and eof tokens.
 * Passage reference extraction finds only [[ ]] links (universal Twee 3).
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

const LINK_PATTERN = /\[\[([^\]]+?)\]\]/g;

/**
 * Tokenize a Fallback passage body.
 *
 * Produces tokens for: plain text, [[ ]] links, and eof.
 * Links get typeId='link' so semantic tokens can highlight them
 * even when no format is detected.
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  if (!input) {
    return [{ typeId: 'eof', text: '', range: { start: baseOffset, end: baseOffset } }];
  }

  const tokens: BodyToken[] = [];
  let lastIndex = 0;

  LINK_PATTERN.lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = LINK_PATTERN.exec(input)) !== null) {
    // Text before the link
    if (match.index > lastIndex) {
      tokens.push({
        typeId: 'text',
        text: input.substring(lastIndex, match.index),
        range: { start: baseOffset + lastIndex, end: baseOffset + match.index },
      });
    }

    // The link itself
    tokens.push({
      typeId: 'link',
      text: match[0],
      range: { start: baseOffset + match.index, end: baseOffset + match.index + match[0].length },
    });

    lastIndex = match.index + match[0].length;
  }

  // Remaining text after last link
  if (lastIndex < input.length) {
    tokens.push({
      typeId: 'text',
      text: input.substring(lastIndex),
      range: { start: baseOffset + lastIndex, end: baseOffset + input.length },
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

const LINK_RE = /\[\[([^\]]+?)\]\]/g;