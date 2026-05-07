/**
 * Knot v2 — Chapbook 2 Lexer & Passage Reference Extraction
 *
 * Body tokenizer, link resolution, and passage reference extraction
 * for the Chapbook 2 format. Also contains regex constants shared
 * with diagnostics.
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  BodyToken,
  LinkResolution,
  PassageRef,
} from '../_types';

import {
  LinkKind,
  PassageRefKind,
} from '../../hooks/hookTypes';

// ═══════════════════════════════════════════════════════════════════
// REGEX CONSTANTS
// ═══════════════════════════════════════════════════════════════════

/**
 * Regex for detecting a YAML-like front matter block at the start of a passage body.
 * Front matter is delimited by `---` on its own line.
 *
 * Captures: [1] = content between the delimiters (may be empty)
 *
 * Example:
 *   ---
 *   var.health: 100
 *   var.name: "Alice"
 *   temp.visited: true
 *   ---
 */
export const FRONT_MATTER_RE = /^---[\t ]*\n([\s\S]*?)\n---[\t ]*(?:\n|$)/;

/**
 * Regex for individual front matter variable assignments.
 * Matches lines like: var.health: 100, temp.name: "Alice"
 *
 * Captures: [1] = prefix (var or temp), [2] = variable name,
 *           [3] = assigned value (trimmed)
 */
export const FRONT_MATTER_VAR_RE = /^(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*(.+)$/gm;

/**
 * Regex for detecting modifier bracket syntax: [modifier name]
 * E.g. [align center], [fade-in], [hidden], [transition]
 *
 * Captures: [1] = modifier name
 */
export const MODIFIER_RE = /^\[([a-zA-Z][\w\s-]*?)\]\s*$/gm;

/** Regex for [[ ]] links */
export const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Specialized regex for {embed passage: 'Name'} — the most common pattern.
 * Captures the passage name from the passage: property.
 */
export const EMBED_PASSAGE_RE = /\{embed\s+passage\s*:\s*['"]([^'"]+)['"]/g;

/**
 * Regex for inserts with a passage: property (reveal, replace, insert).
 * Captures: [1] = insert type, [2] = passage name
 */
export const PASSAGE_PROP_RE = /\{(reveal|replace|insert)\s+link\s*:[^}]*,\s*passage\s*:\s*['"]([^'"]+)['"][^}]*\}/g;

/**
 * Regex for {redirect to: 'PassageName'} — immediate navigation insert.
 * Captures: [1] = passage name
 */
export const REDIRECT_PASSAGE_RE = /\{redirect\s+to\s*:\s*['"]([^'"]+)['"][^}]*\}/g;

// ═══════════════════════════════════════════════════════════════════
// INSERT MATCHING
// ═══════════════════════════════════════════════════════════════════

/**
 * Match a Chapbook insert starting at position `pos`.
 * Returns the full matched string including braces, or null if no match.
 * Handles nested braces and string literals.
 */
export function matchInsert(input: string, pos: number): string | null {
  if (input[pos] !== '{') return null;
  let depth = 0;
  let i = pos;
  let inString: string | null = null;

  while (i < input.length) {
    const ch = input[i];

    // Handle string literals — they can contain } without closing the insert
    if (inString !== null) {
      if (ch === '\\') {
        i += 2; // skip escaped character
        continue;
      }
      if (ch === inString) {
        inString = null;
      }
      i += 1;
      continue;
    }

    if (ch === "'" || ch === '"') {
      inString = ch;
      i += 1;
      continue;
    }

    if (ch === '{') {
      depth += 1;
    } else if (ch === '}') {
      depth -= 1;
      if (depth === 0) {
        return input.slice(pos, i + 1);
      }
    }
    i += 1;
  }

  // Unmatched brace — return what we have as a best-effort token
  return null;
}

// ═══════════════════════════════════════════════════════════════════
// BODY LEXER
// ═══════════════════════════════════════════════════════════════════

/**
 * Tokenize a Chapbook passage body.
 *
 * Recognizes:
 *   - YAML front matter (---...---) at the start of the body
 *   - {insert args} — Chapbook insert syntax (with nested brace handling)
 *   - [modifier] — Modifier bracket syntax (e.g. [align center], [fade-in])
 *   - [[link]] — Link boundaries (core handles outer brackets, but we
 *                tokenize the content within)
 *   - var.name / temp.name — Variable references
 *   - Plain text and newlines
 */
export function lexBody(input: string, baseOffset: number): BodyToken[] {
  const tokens: BodyToken[] = [];
  let pos = 0;
  const len = input.length;

  // ── Front matter: ---...--- at start of body ────────────────
  // Chapbook 2 allows a YAML-like front matter section at the top
  // of passage bodies, delimited by --- on its own line.
  if (pos === 0) {
    const fmMatch = input.match(FRONT_MATTER_RE);
    if (fmMatch) {
      const fmFull = fmMatch[0];
      const fmContent = fmMatch[1];

      // Emit the front matter as a single token
      tokens.push({
        typeId: 'front-matter',
        text: fmFull,
        range: { start: baseOffset, end: baseOffset + fmFull.length },
        macroName: 'front-matter',
        isClosing: false,
      });

      // Emit tokens for each variable assignment within front matter
      FRONT_MATTER_VAR_RE.lastIndex = 0;
      let varMatch: RegExpExecArray | null;
      while ((varMatch = FRONT_MATTER_VAR_RE.exec(fmContent)) !== null) {
        const lineOffset = (fmMatch.index ?? 0) + fmMatch[0].indexOf(fmContent) + varMatch.index;
        tokens.push({
          typeId: 'variable',
          text: varMatch[0],
          range: { start: baseOffset + lineOffset, end: baseOffset + lineOffset + varMatch[0].length },
          varName: varMatch[2],
          varSigil: varMatch[1],
        });
      }

      pos += fmFull.length;
    }
  }

  while (pos < len) {
    // ── Block comment: /% ... %/ ────────────────────────────
    if (input.slice(pos).startsWith('/%')) {
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

    // ── Line comment: %% ... ─────────────────────────────────
    if (input.slice(pos).startsWith('%%')) {
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

    // ── Link: [[...]] ───────────────────────────────────────
    if (input.slice(pos).startsWith('[[')) {
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

    // ── Modifier: [name] on its own line ─────────────────────
    // Chapbook modifiers like [align center], [fade-in], [hidden]
    // use bracket syntax on their own line before a content block.
    if (input[pos] === '[' && !input.slice(pos).startsWith('[[')) {
      // Check if this looks like a modifier: [word-like content]
      const modifierMatch = input.slice(pos).match(/^\[([a-zA-Z][\w\s-]*?)\]\s*$/m);
      if (modifierMatch) {
        // Verify it's a known modifier or looks like one
        const modName = modifierMatch[1].trim();
        const fullMatch = input.slice(pos, pos + modifierMatch[0].length);
        tokens.push({
          typeId: 'modifier',
          text: fullMatch,
          range: { start: baseOffset + pos, end: baseOffset + pos + fullMatch.length },
          macroName: modName,
          isClosing: false,
        });
        pos += fullMatch.length;
        continue;
      }
    }

    // ── Insert: {...} ────────────────────────────────────────
    // Chapbook inserts start with { and end with the matching }
    // We need to handle nested braces and strings inside.
    if (input[pos] === '{') {
      const insertStart = pos;
      const insertText = matchInsert(input, pos);
      if (insertText !== null) {
        // Extract the insert name from the content between braces.
        // Insert name is the first word-like sequence after the opening {.
        const innerContent = insertText.slice(1, -1).trim();
        const nameMatch = innerContent.match(/^([\w\s]+?)(?:[\s,:]|$)/);
        const insertName = nameMatch ? nameMatch[1].trim() : innerContent.split(/[\s,:]/)[0];

        // Check if this is a closing insert like {endif} or {end reveal}
        const isClosing = innerContent.startsWith('end ') || innerContent === 'endif';

        tokens.push({
          typeId: isClosing ? 'insert-close' : 'insert-open',
          text: insertText,
          range: { start: baseOffset + insertStart, end: baseOffset + insertStart + insertText.length },
          macroName: insertName,
          isClosing,
        });
        pos += insertText.length;
        continue;
      }
    }

    // ── Variable: var.name or temp.name ──────────────────────
    // Chapbook uses dot-notation variables, not sigiled ones.
    const varMatch = input.slice(pos).match(/^(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)/);
    if (varMatch) {
      // Ensure the preceding character isn't alphanumeric (avoid partial matches)
      const prevChar = pos > 0 ? input[pos - 1] : '';
      if (/[a-zA-Z0-9_]/.test(prevChar)) {
        // Not a standalone variable reference, treat as text
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
        varSigil: varMatch[1],   // 'var' or 'temp' as pseudo-sigil
      });
      pos += varMatch[0].length;
      continue;
    }

    // ── Newline ──────────────────────────────────────────────
    if (input[pos] === '\n') {
      tokens.push({
        typeId: 'newline',
        text: '\n',
        range: { start: baseOffset + pos, end: baseOffset + pos + 1 },
      });
      pos += 1;
      continue;
    }

    // ── Text (accumulate until we hit a special token) ───────
    let textStart = pos;
    while (pos < len) {
      const remaining = input.slice(pos);
      if (
        remaining.startsWith('/%') ||
        remaining.startsWith('%%') ||
        remaining.startsWith('[[') ||
        remaining.startsWith('{') ||
        (remaining.startsWith('[') && /^\[[a-zA-Z][\w\s-]*?\]\s*$/m.test(remaining)) ||
        /^(var|temp)\.([a-zA-Z_])/.test(remaining) ||
        input[pos] === '\n'
      ) {
        // Check that var/temp isn't preceded by alphanumeric
        const varCheck = remaining.match(/^(var|temp)\.([a-zA-Z_])/);
        if (varCheck) {
          const prevChar = pos > 0 ? input[pos - 1] : '';
          if (/[a-zA-Z0-9_]/.test(prevChar)) {
            // Not a real variable start, just text — keep going
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
      // Safety: emit a single character if we're stuck
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
 * Extract ALL passage references from a Chapbook passage body.
 * Single source of truth: [[ ]] links + insert passage references.
 *
 * NOTE: YAML front matter variable assignments (var.name: value) do NOT
 * produce direct passage references. Front matter sets variables that may
 * be used in conditional inserts, but the variable names themselves are
 * not passage names. No extraction is performed for front matter content.
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

  // ── 2. {embed passage: 'Name'} ─────────────────────────────
  EMBED_PASSAGE_RE.lastIndex = 0;
  while ((match = EMBED_PASSAGE_RE.exec(body)) !== null) {
    refs.push({
      target: match[1],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: '{embed passage}',
    });
  }

  // ── 3. {reveal/replace/insert link: ..., passage: 'Name'} ──
  PASSAGE_PROP_RE.lastIndex = 0;
  while ((match = PASSAGE_PROP_RE.exec(body)) !== null) {
    const insertType = match[1]; // 'reveal', 'replace', or 'insert'
    refs.push({
      target: match[2],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: `{${insertType} passage}`,
    });
  }

  // ── 4. {redirect to: 'PassageName'} ────────────────────────
  REDIRECT_PASSAGE_RE.lastIndex = 0;
  while ((match = REDIRECT_PASSAGE_RE.exec(body)) !== null) {
    refs.push({
      target: match[1],
      kind: PassageRefKind.Macro,
      range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
      source: '{redirect to}',
    });
  }

  return refs;
}

// ═══════════════════════════════════════════════════════════════════
// LINK RESOLUTION
// ═══════════════════════════════════════════════════════════════════

/**
 * Resolve the body text inside [[...]].
 *
 * Chapbook uses the same link syntax as Harlowe:
 *   - Right arrow: [[target->display text]]
 *   - Left arrow:  [[display text<-target]]
 *   - Simple:      [[target]]
 *   - NO pipe separator (unlike SugarCube)
 *   - NO setter syntax
 */
export function resolveLinkBody(rawBody: string): LinkResolution {
  if (!rawBody) return { target: '', kind: LinkKind.Passage };

  // 1. Right arrow: target->text (RIGHTMOST -> is separator, matching Harlowe)
  const rightArrowIdx = rawBody.lastIndexOf('->');
  if (rightArrowIdx !== -1) {
    const target = rawBody.slice(0, rightArrowIdx).trim();
    const displayText = rawBody.slice(rightArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 2. Left arrow: text<-target (arrow points away from target)
  const leftArrowIdx = rawBody.indexOf('<-');
  if (leftArrowIdx !== -1) {
    const displayText = rawBody.slice(0, leftArrowIdx).trim();
    const target = rawBody.slice(leftArrowIdx + 2).trim();
    const isExternal = /^https?:\/\//.test(target);
    return {
      target,
      displayText: displayText !== target ? displayText : undefined,
      kind: isExternal ? LinkKind.External : LinkKind.Passage,
    };
  }

  // 3. Simple link: [[target]]
  const target = rawBody.trim();
  return {
    target,
    kind: /^https?:\/\//.test(target) ? LinkKind.External : LinkKind.Passage,
  };
}
