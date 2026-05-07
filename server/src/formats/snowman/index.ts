/**
 * Knot v2 — Snowman 2 Format Module
 *
 * Implements FormatModule for the Snowman 2 story format.
 * Exports exactly one object: `snowmanModule`.
 *
 * Key Snowman characteristics:
 *   - NO macro syntax — uses <% %> JavaScript execution blocks
 *     and <%= %> expression output blocks (Underscore.js templates)
 *   - Variables: s.name (story — window.story, persists) and
 *     t.name (temporary — window.passage, passage-scoped)
 *   - Passage navigation: story.show('PassageName') JS API
 *   - Links: [[Target]] and [[Text->Target]] only (right arrow,
 *     NO pipe | and NO left arrow <-)
 *   - MacroBodyStyle: Inline (no macro bodies — just JavaScript)
 *   - Special passages: None beyond Twee 3 spec
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  MacroDelimiters,
  VariableCapability,
  DiagnosticCapability,
  SpecialPassageDef,
} from '../_types';

import {
  MacroBodyStyle,
} from '../../hooks/hookTypes';

// Extracted sections for maintainability
import { lexBody, extractPassageRefs, resolveLinkBody } from './lexer';
import { RUNTIME_GLOBALS, VIRTUAL_RUNTIME_PRELUDE } from './runtime';
import { SNIPPET_TEMPLATES } from './snippets';
import { customDiagnosticCheck } from './diagnostics';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const SNOWMAN_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',           label: 'Document',            canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',      label: 'Passage Header',     canHaveChildren: false },
    { id: 'PassageBody',        label: 'Passage Body',       canHaveChildren: true,  childNodeTypeIds: ['TemplateBlock', 'TemplateExpression', 'Variable', 'Link', 'Text'] },
    { id: 'Link',              label: 'Link',               canHaveChildren: false },
    { id: 'Text',              label: 'Text',               canHaveChildren: false },
    { id: 'TemplateBlock',     label: 'Template Block',     canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'TemplateExpression',label: 'Template Expression', canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'Variable',          label: 'Variable',           canHaveChildren: false },
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

const SNOWMAN_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'template-block',      label: 'Template Block',       category: 'delimiter' },
  { id: 'template-expression', label: 'Template Expression',  category: 'delimiter' },
  { id: 'link',                label: 'Link',                 category: 'identifier' },
  { id: 'comment',             label: 'Comment',              category: 'literal' },
  { id: 'variable',            label: 'Variable',             category: 'identifier' },
  { id: 'text',                label: 'Text',                 category: 'literal' },
  { id: 'newline',             label: 'Newline',              category: 'whitespace' },
  { id: 'eof',                 label: 'EOF',                  category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const snowmanModule: FormatModule = {
  formatId: 'snowman-2',
  displayName: 'Snowman 2',
  version: '2.0.0',
  aliases: ['snowman', 'Snowman 2', 'snowman2'],

  astNodeTypes: SNOWMAN_AST_NODE_TYPES,
  tokenTypes: SNOWMAN_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: [] as readonly SpecialPassageDef[],  // No special passages beyond Twee 3 spec

  macroBodyStyle: MacroBodyStyle.Inline,
  macroDelimiters: {
    open: '<%',
    close: '%>',
  } satisfies MacroDelimiters,
  macroPattern: null,  // No named macro syntax — templates are not macros

  // ── Capability: Variables ───────────────────────────────────
  // Snowman uses s.name (story) and t.name (temp) as JavaScript
  // object property accesses on window.story and window.passage.
  // Modeled as pseudo-sigils for the VariableCapability interface.
  variables: {
    sigils: [
      { sigil: 's', kind: 'story', description: 'Snowman story variable object (s.name) — window.story, persists across passages' },
      { sigil: 't', kind: 'temp', description: 'Snowman temp variable object (t.name) — window.passage, scoped to current passage' },
    ],
    assignmentMacros: new Set<string>(),  // Assignments happen via JavaScript: s.name = value
    assignmentOperators: ['='],
    comparisonOperators: ['===', '!==', '==', '!=', '>', '>=', '<', '<='],
    variablePattern: /\b([st])\.([a-zA-Z_][a-zA-Z0-9_]*)/g,
    triggerChars: ['s'],
  } satisfies VariableCapability,

  // ── Capability: Diagnostics ─────────────────────────────────
  diagnostics: {
    rules: [
      {
        id: 'invalid-template-syntax',
        description: 'Malformed template block (unclosed <% or <%=)',
        defaultSeverity: 'error',
        scope: 'passage',
      },
      {
        id: 'invalid-link-syntax',
        description: 'Malformed [[ ]] link',
        defaultSeverity: 'error',
        scope: 'passage',
      },
    ],
    customCheck: customDiagnosticCheck,
  } satisfies DiagnosticCapability,

  // ── Capability: Snippets ──────────────────────────────────────
  snippets: {
    templates: SNIPPET_TEMPLATES,
  },

  // ── Capability: Runtime ───────────────────────────────────────
  runtime: {
    globals: RUNTIME_GLOBALS,
    virtualPrelude: VIRTUAL_RUNTIME_PRELUDE,
  },

  // ── NO macros capability (Snowman has no macro syntax) ──────
  // ── NO customMacros capability (Snowman has no user-defined macros) ──
};
