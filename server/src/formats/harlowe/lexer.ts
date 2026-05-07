/**
 * Knot v2 — Harlowe 3 Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Harlowe 3 format.
 *
 * Harlowe syntax:
 *   (macro:args)   → macro-call
 *   [hook]         → hook-open / hook-close
 *   |nametag>      → hook-name
 *   $variable      → variable (story)
 *   _tempvar       → variable (temporary)
 *   plain text     → text
 */

import type { BodyToken, LinkResolution, PassageRef } from '../_types';
import { LinkKind, PassageRefKind } from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Find the position of the closing paren that matches the open paren at `startPos`.
 * Handles nested parentheses.
 */
function findMatchingCloseParen(text: string, startPos: number): number {
  let depth = 0;
  let inString: string | null = null;
  for (let i = startPos; i < text.length; i++) {
    const ch = text[i];

    // Track string boundaries (single and double quotes)
    if (inString) {
      if (ch === inString && text[i - 1] !== '\\') {
        inString = null;
      }
      continue;
    }
    if (ch === '"' || ch === "'") {
      inString = ch;
      continue;
    }

    if (ch === '(') depth++;
    if (ch === ')') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1; // no matching close paren
}

/**
 * Tokenize a Harlowe passage body into adapter-specific tokens.
 *
 * Recognizes:
 *   (macro:args)   → macro-call
 *   [hook]         → hook-open / hook-close
 *   |nametag>      → hook-name
 *   $variable      → variable (story variable)
 *   _tempvar       → variable (temporary variable)
 *   plain text     → text
 *   newlines       → newline
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  while (pos < len) {
    const ch = input[pos];

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

    // ── Macro call: (name: ...) ──
    if (ch === '(') {
      // Check if this is a macro call (name followed by colon)
      const macroMatch = input.substring(pos).match(/^\(([a-zA-Z][a-zA-Z0-9_-]*:)/);
      if (macroMatch) {
        const macroName = macroMatch[1];
        // Find the matching close paren (accounting for nesting)
        const closePos = findMatchingCloseParen(input, pos);
        if (closePos >= 0) {
          const macroText = input.substring(pos, closePos + 1);
          tokens.push({
            typeId: 'macro-call',
            text: macroText,
            range: { start: baseOffset + pos, end: baseOffset + closePos + 1 },
            macroName: macroName.slice(0, -1), // strip trailing colon for name field
          });
          pos = closePos + 1;
          continue;
        }
      }
      // Not a macro call — treat as text
      tokens.push({ typeId: 'text', text: '(', range: { start: baseOffset + pos, end: baseOffset + pos + 1 } });
      pos++;
      continue;
    }

    // ── Hook open: [ ──
    if (ch === '[') {
      tokens.push({ typeId: 'hook-open', text: '[', range: { start: baseOffset + pos, end: baseOffset + pos + 1 } });
      pos++;
      continue;
    }

    // ── Hook close: ] ──
    if (ch === ']') {
      tokens.push({ typeId: 'hook-close', text: ']', range: { start: baseOffset + pos, end: baseOffset + pos + 1 } });
      pos++;
      continue;
    }

    // ── Hook name: |name> or <name| ──
    if (ch === '|' || (ch === '<' && pos + 1 < len && /[a-zA-Z_]/.test(input[pos + 1]) && input.indexOf('|', pos + 1) >= 0)) {
      const hookNameMatch = input.substring(pos).match(/^(\|[a-zA-Z_][a-zA-Z0-9_]*>|<[a-zA-Z_][a-zA-Z0-9_]*\|)/);
      if (hookNameMatch) {
        const nameText = hookNameMatch[1];
        tokens.push({
          typeId: 'hook-name',
          text: nameText,
          range: { start: baseOffset + pos, end: baseOffset + pos + nameText.length },
        });
        pos += nameText.length;
        continue;
      }
    }

    // ── Variable: $name or _name ──
    if (ch === '$' || ch === '_') {
      const varMatch = input.substring(pos).match(/^[$][a-zA-Z_][a-zA-Z0-9_]*|^_[a-zA-Z_][a-zA-Z0-9_]*/);
      if (varMatch) {
        const varText = varMatch[0];
        tokens.push({
          typeId: 'variable',
          text: varText,
          range: { start: baseOffset + pos, end: baseOffset + pos + varText.length },
          varName: varText.substring(1),
          varSigil: ch,
        });
        pos += varText.length;
        continue;
      }
    }

    // ── Newline ──
    if (ch === '\n') {
      tokens.push({ typeId: 'newline', text: '\n', range: { start: baseOffset + pos, end: baseOffset + pos + 1 } });
      pos++;
      continue;
    }

    // ── Text (consume until next special character) ──
    let textEnd = pos + 1;
    while (textEnd < len) {
      const nextCh = input[textEnd];
      const remaining = input.substring(textEnd);
      if (nextCh === '(' || nextCh === '[' || nextCh === ']' || nextCh === '$' || nextCh === '_' || nextCh === '|' || nextCh === '\n' || remaining.startsWith('/%') || remaining.startsWith('%%') || remaining.startsWith('[[')) {
        break;
      }
      textEnd++;
    }
    tokens.push({ typeId: 'text', text: input.substring(pos, textEnd), range: { start: baseOffset + pos, end: baseOffset + textEnd } });
    pos = textEnd;
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

/** Harlowe navigation/include macros that take passage name arguments */
const HARLOWE_NAV_MACROS = /\((go-to|display|link-goto|link-reveal-goto)\s*:\s*([^)]*?)\)/g;

/**
 * Extract ALL passage references from a Harlowe passage body.
 * Single source of truth: [[ ]] links + navigation macros.
 * Harlowe has no implicit JS API patterns (unlike SugarCube).
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
  HARLOWE_NAV_MACROS.lastIndex = 0;
  while ((match = HARLOWE_NAV_MACROS.exec(body)) !== null) {
    const macroName = match[1];
    const args = match[2].trim();
    // Extract string argument (passage name)
    // Harlowe uses: (go-to: "PassageName") or (go-to: 'PassageName')
    const strArg = args.match(/["']([^"']+)["']/);
    if (strArg) {
      refs.push({
        target: strArg[1],
        kind: PassageRefKind.Macro,
        range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
        source: `(${macroName}:)`,
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
 * Harlowe link syntax:
 *   [[Target]]          — simple passage link
 *   [[Text->Target]]    — right arrow (RIGHTMOST -> is separator)
 *   [[Target<-Text]]    — left arrow (LEFTMOST <- is separator)
 *   NO pipe | syntax (that's SugarCube)
 *   NO setter syntax (that's SugarCube)
 */
export function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // Right arrow: rightmost -> is the separator
  const rightArrowIdx = rawBody.lastIndexOf('->');
  if (rightArrowIdx >= 0) {
    const displayText = rawBody.substring(0, rightArrowIdx).trim();
    const target = rawBody.substring(rightArrowIdx + 2).trim();
    if (!target) return { target: displayText || '', kind: LinkKind.Passage };
    const isExternal = /^https?:\/\//i.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // Left arrow: leftmost <- is the separator
  const leftArrowIdx = rawBody.indexOf('<-');
  if (leftArrowIdx >= 0) {
    const target = rawBody.substring(0, leftArrowIdx).trim();
    const displayText = rawBody.substring(leftArrowIdx + 2).trim();
    if (!target) return { target: displayText || '', kind: LinkKind.Passage };
    const isExternal = /^https?:\/\//i.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // Simple [[Target]] — no arrows
  const target = rawBody.trim();
  if (!target) return { target: '', kind: LinkKind.Passage };
  return {
    target,
    kind: /^https?:\/\//i.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}
