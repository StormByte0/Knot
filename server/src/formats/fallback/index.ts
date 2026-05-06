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
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  MacroDelimiters,
} from '../_types';

import {
  MacroBodyStyle,
} from '../../hooks/hookTypes';

// Lexer & passage reference extraction extracted for maintainability
import { lexBody, extractPassageRefs, resolveLinkBody } from './lexer';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════
// TOKEN TYPES
// ═══════════════════════════════════════════════════════════════════

const FALLBACK_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'text',    label: 'Text',    category: 'literal' },
  { id: 'newline', label: 'Newline', category: 'whitespace' },
  { id: 'eof',     label: 'EOF',     category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

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
