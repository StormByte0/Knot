/**
 * Knot v2 — Fallback Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Fallback (Basic Twee) format.
 *
 * In fallback mode, the entire body is plain text EXCEPT for:
 *   - [[ ]] links (universal Twee 3)
 *   - /% ... %/ block comments (universal Twee)
 *   - %% ... line comments (universal Twee)
 *
 * The lexer produces: text, link, comment, and eof tokens.
 * Passage reference extraction finds only [[ ]] links (universal Twee 3).
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Fallback passage body.
 *
 * Produces tokens for: plain text, [[ ]] links, /% %/ block comments,
 * %% line comments, and eof.
 * Links get typeId='link' so semantic tokens can highlight them
 * even when no format is detected.
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  if (!input) {
    return [{ typeId: 'eof', text: '', range: { start: baseOffset, end: baseOffset } }];
  }

  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  while (pos < len) {
    // ── Block comment: /% ... %/ ───────────────────────────
    if (input.startsWith('/%', pos)) {
      const endIdx = input.indexOf('%/', pos + 2);
      const commentEnd = endIdx >= 0 ? endIdx + 2 : len;
      tokens.push({
        typeId: 'comment',
        text: input.substring(pos, commentEnd),
        range: { start: baseOffset + pos, end: baseOffset + commentEnd },
      });
      pos = commentEnd;
      continue;
    }

    // ── Line comment: %% ... ───────────────────────────────
    if (input.startsWith('%%', pos)) {
      const eolIdx = input.indexOf('\n', pos + 2);
      const commentEnd = eolIdx >= 0 ? eolIdx : len;
      tokens.push({
        typeId: 'comment',
        text: input.substring(pos, commentEnd),
        range: { start: baseOffset + pos, end: baseOffset + commentEnd },
      });
      pos = commentEnd;
      continue;
    }

    // ── Link: [[...]] ───────────────────────────────────────
    if (input.startsWith('[[', pos)) {
      const closeIdx = input.indexOf(']]', pos + 2);
      const linkEnd = closeIdx >= 0 ? closeIdx + 2 : len;
      tokens.push({
        typeId: 'link',
        text: input.substring(pos, linkEnd),
        range: { start: baseOffset + pos, end: baseOffset + linkEnd },
      });
      pos = linkEnd;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ──────
    let textStart = pos;
    while (pos < len) {
      const remaining = input.slice(pos);
      if (
        remaining.startsWith('/%') ||
        remaining.startsWith('%%') ||
        remaining.startsWith('[[')
      ) {
        break;
      }
      pos += 1;
    }
    if (pos > textStart) {
      tokens.push({
        typeId: 'text',
        text: input.slice(textStart, pos),
        range: { start: baseOffset + textStart, end: baseOffset + pos },
      });
    } else {
      // Safety: advance at least one character
      tokens.push({
        typeId: 'text',
        text: input[pos],
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
    }
  }

  tokens.push({
    typeId: 'eof',
    text: '',
    range: { start: baseOffset + pos, end: baseOffset + pos },
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
