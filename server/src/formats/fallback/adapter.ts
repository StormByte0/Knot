/**
 * Knot v2 — Fallback Format Module
 *
 * Minimal FormatModule for basic Twee support when no format is detected.
 * Provides passage headers, basic [[link]] navigation, and nothing else.
 * This is the safe default — no format-specific features.
 *
 * MacroBodyStyle is Inline (no macro bodies).
 * Body lexing returns a single text token + EOF (all body is plain text).
 * extractPassageRefs finds only [[ ]] links (the universal Twee 3 link syntax).
 * No capability bags — no macros, no variables, no custom macros, etc.
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  BodyToken,
  LinkResolution,
  SpecialPassageDef,
  MacroDelimiters,
  PassageRef,
} from '../_types';

import {
  MacroBodyStyle,
  LinkKind,
  PassageRefKind,
} from '../../hooks/hookTypes';

// ─── AST Node Types (Twine Engine Only) ──────────────────────────

const FALLBACK_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',       label: 'Document',        canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',  label: 'Passage Header',  canHaveChildren: false },
    { id: 'PassageBody',    label: 'Passage Body',    canHaveChildren: true,  childNodeTypeIds: ['Link', 'Text'] },
    { id: 'Link',           label: 'Link',            canHaveChildren: false },
    { id: 'Text',           label: 'Text',            canHaveChildren: false },
  ];
  const types = new Map(defs.map(d => [d.id, d]));
  return {
    types,
    Document: 'Document',
    PassageHeader: 'PassageHeader',
    PassageBody: 'PassageBody',
    Link: 'Link',
    Text: 'Text',
  };
})();

// ─── Token Types (Minimal) ──────────────────────────────────────

const FALLBACK_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'text',    label: 'Text',    category: 'literal' },
  { id: 'newline', label: 'Newline', category: 'whitespace' },
  { id: 'eof',     label: 'EOF',     category: 'whitespace' },
];

// ─── Body Lexer ─────────────────────────────────────────────────

function lexBody(input: string, baseOffset: number): BodyToken[] {
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

// ─── Passage Reference Extraction ────────────────────────────────

const LINK_RE = /\[\[([^\]]+?)\]\]/g;

/**
 * Extract all passage references from a passage body.
 * Fallback only handles [[ ]] links — the universal Twee 3 syntax.
 * No macros, no API calls, no implicit patterns.
 */
function extractPassageRefs(body: string, bodyOffset: number): PassageRef[] {
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

// ─── Link Resolution ────────────────────────────────────────────

function resolveLinkBody(rawBody: string): LinkResolution {
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

// ─── THE MODULE EXPORT ──────────────────────────────────────────

export const fallbackModule: FormatModule = {
  formatId: 'fallback',
  displayName: 'Fallback (Basic Twee)',
  version: '1.0.0',
  aliases: ['fallback', 'twee', 'basic'],

  astNodeTypes: FALLBACK_AST_NODE_TYPES,
  tokenTypes: FALLBACK_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: [],  // No special passages beyond Twee 3 spec

  macroBodyStyle: MacroBodyStyle.Inline,
  macroDelimiters: {
    open: '',
    close: '',
  } satisfies MacroDelimiters,
  macroPattern: null,  // No macro syntax

  // No capability bags — fallback has no macros, variables, custom macros, etc.
};
