/**
 * Knot v2 — Snowman 2 Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Snowman 2 format.
 *
 * MUST NOT import from: core/, handlers/
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// REGEX CONSTANTS
// ═══════════════════════════════════════════════════════════════════

/** Match [[ ]] links */
const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Match Snowman JS API calls that reference passages.
 *
 * Supported patterns:
 *   story.show('PassageName')
 *   story.passage('PassageName')
 *   window.story.show('PassageName')
 *   window.story.passage('PassageName')
 *
 * Also matches template-delimited forms inside <% %> blocks,
 * since the regex scans the full body text.
 */
const STORY_API_RE = /(?:window\.)?story\.(show|passage)\s*\(\s*['"]([^'"]+)['"]\s*\)/g;

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Snowman passage body.
 *
 * Snowman's template syntax:
 *   <%= ... %> — JavaScript expression (output the result)
 *   <% ... %>  — JavaScript code block (execution, no output)
 *   s.name     — Story variable (window.story)
 *   t.name     — Temporary variable (window.passage)
 *   [[...]]    — Link boundaries
 *   Everything else is plain text or newlines.
 *
 * Order matters: <%= must be checked before <% to avoid
 * partial matches.
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  while (pos < len) {
    const remaining = input.slice(pos);

    // ── Block comment: /% ... %/ ──────────────────────────────
    if (remaining.startsWith('/%')) {
      const closeIdx = input.indexOf('%/', pos + 2);
      if (closeIdx !== -1) {
        const commentText = input.slice(pos, closeIdx + 2);
        tokens.push({
          typeId: 'comment',
          text: commentText,
          range: { start: baseOffset + pos, end: baseOffset + pos + commentText.length },
        });
        pos += commentText.length;
        continue;
      }
    }

    // ── Line comment: %% ... ───────────────────────────────────
    if (remaining.startsWith('%%')) {
      const lineEnd = input.indexOf('\n', pos + 2);
      const commentText = lineEnd !== -1
        ? input.slice(pos, lineEnd)
        : input.slice(pos);
      tokens.push({
        typeId: 'comment',
        text: commentText,
        range: { start: baseOffset + pos, end: baseOffset + pos + commentText.length },
      });
      pos += commentText.length;
      continue;
    }

    // ── Link: [[...]] ─────────────────────────────────────────
    if (remaining.startsWith('[[')) {
      const linkEnd = input.indexOf(']]', pos + 2);
      if (linkEnd !== -1) {
        const fullLink = input.slice(pos, linkEnd + 2);
        tokens.push({
          typeId: 'link',
          text: fullLink,
          range: { start: baseOffset + pos, end: baseOffset + pos + fullLink.length },
        });
        pos += fullLink.length;
        continue;
      }
    }

    // ── Template expression: <%= ... %> ────────────────────────
    // Must be checked BEFORE <% to avoid partial match on <%=.
    const exprMatch = remaining.match(/^<%=([\s\S]*?)%>/);
    if (exprMatch) {
      tokens.push({
        typeId: 'template-expression',
        text: exprMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + exprMatch[0].length },
      });
      pos += exprMatch[0].length;
      continue;
    }

    // ── Template block: <% ... %> ─────────────────────────────
    const blockMatch = remaining.match(/^<%([\s\S]*?)%>/);
    if (blockMatch) {
      tokens.push({
        typeId: 'template-block',
        text: blockMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + blockMatch[0].length },
      });
      pos += blockMatch[0].length;
      continue;
    }

    // ── Unclosed template block: <% without matching %> ───────
    // Detect <% that is NOT followed by %> anywhere — this is an
    // error but we still tokenize it so diagnostics can report it.
    const unclosedMatch = remaining.match(/^<%(=?)[\s\S]*$/);
    if (unclosedMatch) {
      // Consume the rest of the input as an unclosed template
      const typeId = unclosedMatch[1] === '=' ? 'template-expression' : 'template-block';
      tokens.push({
        typeId,
        text: remaining,
        range: { start: baseOffset + pos, end: baseOffset + pos + remaining.length },
      });
      pos = len;
      continue;
    }

    // ── Variable: s.name or t.name ────────────────────────────
    // Match s.identifier or t.identifier where identifier is a
    // valid JS identifier (not preceded by alphanumeric/dot, to
    // avoid matching inside longer expressions like "this.name").
    const varMatch = remaining.match(/^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/);
    if (varMatch) {
      // Check that this is not preceded by an alphanumeric char or dot
      // (e.g. "this.x" should not match "s.x" inside it)
      const prevChar = pos > 0 ? input[pos - 1] : '';
      if (!/[a-zA-Z0-9_.]/.test(prevChar)) {
        tokens.push({
          typeId: 'variable',
          text: varMatch[0],
          range: { start: baseOffset + pos, end: baseOffset + pos + varMatch[0].length },
          varName: varMatch[2],
          varSigil: varMatch[1],
        });
        pos += varMatch[0].length;
        continue;
      }
    }

    // ── Newline ───────────────────────────────────────────────
    if (input[pos] === '\n') {
      tokens.push({
        typeId: 'newline',
        text: '\n',
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ────────
    let textStart = pos;
    while (pos < len) {
      const r = input.slice(pos);
      if (
        r.startsWith('/%') ||
        r.startsWith('%%') ||
        r.startsWith('[[') ||
        r.startsWith('<%') ||
        /^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/.test(r) ||
        input[pos] === '\n'
      ) {
        // For variable matches, also check the preceding character
        if (/^([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/.test(r)) {
          const prevChar = pos > 0 ? input[pos - 1] : '';
          if (/[a-zA-Z0-9_.]/.test(prevChar)) {
            // Not a real variable — keep accumulating text
            pos += 1;
            continue;
          }
        }
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
      // Safety: always advance at least one character
      tokens.push({
        typeId: 'text',
        text: input[pos],
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
    }
  }

  // EOF sentinel
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

/**
 * Extract ALL passage references from a Snowman passage body.
 *
 * Single source of truth for: [[ ]] links + story.show/passage JS API calls
 * inside template blocks.
 */
export function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
  const refs: PassageRef[] = [];

  // ── 1. [[ ]] links ──────────────────────────────────────────
  LINK_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = LINK_RE.exec(body)) !== null) {
    const rawBody = match[1];
    const resolved = resolveLinkBody(rawBody);
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

  // ── 2. story.show() / story.passage() / window.story.show() ──
  STORY_API_RE.lastIndex = 0;
  while ((match = STORY_API_RE.exec(body)) !== null) {
    const method = match[1]; // 'show' or 'passage'
    const passageName = match[2];
    const sourceLabel = method === 'show' ? 'story.show()' : 'story.passage()';
    refs.push({
      target: passageName,
      kind: PassageRefKind.API,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: sourceLabel,
    });
  }

  return refs;
}

// ═══════════════════════════════════════════════════════════════════
// LINK RESOLUTION
// ═══════════════════════════════════════════════════════════════════

/**
 * Resolve the raw body text inside [[...]] for Snowman.
 *
 * Snowman supports:
 *   [[Target]]         — simple link to Target
 *   [[Text->Target]]   — link with display text (right arrow only)
 *
 * Snowman does NOT support pipe | or left arrow <-.
 */
export function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // 1. Right arrow: [[Text->Target]]
  const rightArrowIdx = rawBody.indexOf('->');
  if (rightArrowIdx !== -1) {
    const displayText = rawBody.slice(0, rightArrowIdx).trim();
    const target = rawBody.slice(rightArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 2. Simple link: [[Target]]
  const target = rawBody.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}
