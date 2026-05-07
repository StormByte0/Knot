/**
 * Knot v2 — SugarCube 2 Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the SugarCube 2 format.
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a SugarCube passage body into format-specific tokens.
 *
 * Recognizes:
 *   /% ... %/         → comment (block)
 *   %% ...             → comment (line)
 *   <<name args>>      → macro-call
 *   <</name>>          → macro-close
 *   [[...]]            → link
 *   $var / _var        → variable
 *   plain text         → text
 *   newlines           → newline
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
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

    // ── Close tag: <</name>> ────────────────────────────────
    const closeMatch = input.slice(pos).match(/^<<\/(\w+)>>/);
    if (closeMatch) {
      tokens.push({
        typeId: 'macro-close',
        text: closeMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + closeMatch[0].length },
        macroName: closeMatch[1],
        isClosing: true,
      });
      pos += closeMatch[0].length;
      continue;
    }

    // ── Macro call: <<name ...>> ────────────────────────────
    const macroMatch = input.slice(pos).match(/^<<(\w+)(?:\s+([^>]*?))?>>/);
    if (macroMatch) {
      tokens.push({
        typeId: 'macro-call',
        text: macroMatch[0],
        range: { start: baseOffset + pos, end: baseOffset + pos + macroMatch[0].length },
        macroName: macroMatch[1],
        isClosing: false,
      });
      pos += macroMatch[0].length;
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

    // ── Variable: $name or _name ────────────────────────────
    const varMatch = input.slice(pos).match(/^([$_])(\w+)/);
    if (varMatch) {
      const prevChar = pos > 0 ? input[pos - 1] : '';
      if (/[a-zA-Z0-9]/.test(prevChar)) {
        tokens.push({
          typeId: 'text',
          text: input[pos],
          range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
        });
        pos += 1;
        continue;
      }
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

    // ── Newline ─────────────────────────────────────────────
    if (input[pos] === '\n') {
      tokens.push({
        typeId: 'newline',
        text: '\n',
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ──────
    let textStart = pos;
    while (pos < len) {
      const remaining = input.slice(pos);
      if (
        remaining.startsWith('/%') ||
        remaining.startsWith('%%') ||
        remaining.startsWith('<<') ||
        remaining.startsWith('[[') ||
        /^[$_]\w/.test(remaining) ||
        input[pos] === '\n'
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

const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/** SugarCube macro pattern for extracting navigation/include passage names */
const MACRO_NAV_RE = /<<(goto|display|include|popup|button|link|choice|actions)\s+([^>]*?)>>/g;

/** SugarCube implicit passage reference patterns */
const IMPLICIT_PATTERNS = [
  { re: /data-passage\s*=\s*["']([^"']+)["']/g, source: 'data-passage' },
  { re: /Engine\.play\s*\(\s*["']([^"']+)["']/g, source: 'Engine.play()' },
  { re: /Engine\.goto\s*\(\s*["']([^"']+)["']/g, source: 'Engine.goto()' },
  { re: /Story\.get\s*\(\s*["']([^"']+)["']/g,  source: 'Story.get()' },
];

/**
 * Extract ALL passage references from a SugarCube passage body.
 * Single source of truth: [[ ]] links + navigation macros + JS API calls + implicit refs.
 */
export function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
  const refs: PassageRef[] = [];

  // ── 1. [[ ]] links ─────────────────────────────────────────
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

  // ── 2. Navigation/include macros ───────────────────────────
  MACRO_NAV_RE.lastIndex = 0;
  while ((match = MACRO_NAV_RE.exec(body)) !== null) {
    const macroName = match[1];
    const args = match[2].trim();
    // Extract the first string argument (passage name)
    const strArg = args.match(/^["']([^"']+)["']/) || args.match(/^(\S+)/);
    if (strArg) {
      refs.push({
        target: strArg[1],
        kind: PassageRefKind.Macro,
        range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
        source: `<<${macroName}>>`,
      });
    }
  }

  // ── 3. Implicit patterns (data-passage, Engine.play, etc.) ──
  for (const { re, source } of IMPLICIT_PATTERNS) {
    re.lastIndex = 0;
    while ((match = re.exec(body)) !== null) {
      refs.push({
        target: match[1],
        kind: PassageRefKind.Implicit,
        range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
        source,
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
 * SugarCube link syntax (order of precedence):
 *   1. Pipe: text|target       (pipe takes highest precedence)
 *   2. Right arrow: target->text
 *   3. Left arrow: text<-target
 *   4. Simple: [[target]]
 *   Optional setter: ...[$setter]
 */
export function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // Strip optional setter: [...][$setter] or ...[$setter]
  let body = rawBody;
  let setter: string | undefined;
  const setterMatch = body.match(/^(.+?)\[\$(.+?)\]$/);
  if (setterMatch) {
    body = setterMatch[1];
    setter = setterMatch[2];
  }

  // 1. Pipe separator takes precedence: text|target
  const pipeIdx = body.indexOf('|');
  if (pipeIdx !== -1) {
    const displayText = body.slice(0, pipeIdx).trim();
    const target = body.slice(pipeIdx + 1).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
      setter,
    };
  }

  // 2. Right arrow: target->text  (arrow points to display text)
  const rightArrowIdx = body.indexOf('->');
  if (rightArrowIdx !== -1) {
    const target = body.slice(0, rightArrowIdx).trim();
    const displayText = body.slice(rightArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
      setter,
    };
  }

  // 3. Left arrow: text<-target  (arrow points away from target)
  const leftArrowIdx = body.indexOf('<-');
  if (leftArrowIdx !== -1) {
    const displayText = body.slice(0, leftArrowIdx).trim();
    const target = body.slice(leftArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
      setter,
    };
  }

  // 4. Simple link: [[target]]
  const target = body.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
    setter,
  };
}
