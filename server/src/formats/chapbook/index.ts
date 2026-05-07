/**
 * Knot v2 — Chapbook 2 Format Module
 *
 * Implements FormatModule for the Chapbook 2 story format.
 * Exports exactly one object: `chapbookModule`.
 *
 * Key Chapbook characteristics:
 *   - No macro syntax — uses {insert} syntax instead of <<macro>> or (macro:)
 *   - Inserts are the primary interactive mechanism: {embed passage: 'Name'},
 *     {reveal link: 'Click', passage: 'Target'}, {restart link: 'Restart'}, etc.
 *   - Variables use var.name and temp.name dot-notation (NOT sigiled like $name)
 *   - Links use [[ ]] with -> and <- arrows (same as Harlowe — NO pipe |)
 *   - MacroBodyStyle.Inline — inserts are self-contained, no macro bodies
 *   - YAML-like front matter delimited by --- at the top of passage bodies
 *   - Modifiers use [modifier] bracket syntax (e.g. [align center], [fade-in])
 *   - Special passages: none beyond basic Twee 3 spec
 *
 * MUST NOT import from: core/, handlers/
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  MacroDelimiters,
  MacroCapability,
  VariableCapability,
  DiagnosticCapability,
} from '../_types';

import {
  MacroBodyStyle,
} from '../../hooks/hookTypes';

// Insert catalog is split into category files — see ./inserts-index.ts
import { CHAPBOOK_INSERTS, ALIAS_MAP } from './inserts-index';

// Other sections extracted for maintainability
import { SPECIAL_PASSAGES } from './specialPassages';
import { RUNTIME_GLOBALS, VIRTUAL_RUNTIME_PRELUDE } from './runtime';
import { SNIPPET_TEMPLATES } from './snippets';
import { lexBody, extractPassageRefs, resolveLinkBody } from './lexer';
import { customDiagCheck } from './diagnostics';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const CHAPBOOK_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',       label: 'Document',          canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',  label: 'Passage Header',    canHaveChildren: false },
    { id: 'PassageBody',    label: 'Passage Body',      canHaveChildren: true,  childNodeTypeIds: ['FrontMatter', 'InsertCall', 'Modifier', 'Variable', 'Link', 'Text'] },
    { id: 'FrontMatter',    label: 'Front Matter',      canHaveChildren: true,  childNodeTypeIds: ['Variable', 'Text'] },
    { id: 'Link',           label: 'Link',              canHaveChildren: false },
    { id: 'Text',           label: 'Text',              canHaveChildren: false },
    { id: 'InsertCall',     label: 'Insert Call',       canHaveChildren: true,  childNodeTypeIds: ['InsertCall', 'Variable', 'Link', 'Text'] },
    { id: 'Modifier',       label: 'Modifier',          canHaveChildren: true,  childNodeTypeIds: ['InsertCall', 'Variable', 'Link', 'Text'] },
    { id: 'Variable',       label: 'Variable',          canHaveChildren: false },
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

const CHAPBOOK_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'insert-open',   label: 'Insert Open',   category: 'delimiter' },
  { id: 'insert-body',   label: 'Insert Body',   category: 'delimiter' },
  { id: 'insert-close',  label: 'Insert Close',  category: 'delimiter' },
  { id: 'front-matter',  label: 'Front Matter',  category: 'delimiter' },
  { id: 'modifier',      label: 'Modifier',      category: 'delimiter' },
  { id: 'link',          label: 'Link',           category: 'identifier' },
  { id: 'comment',       label: 'Comment',        category: 'literal' },
  { id: 'variable',      label: 'Variable',       category: 'identifier' },
  { id: 'text',          label: 'Text',           category: 'literal' },
  { id: 'newline',       label: 'Newline',        category: 'whitespace' },
  { id: 'eof',           label: 'EOF',            category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const chapbookModule: FormatModule = {
  formatId: 'chapbook-2',
  displayName: 'Chapbook 2',
  version: '2.0.0',
  aliases: ['chapbook', 'Chapbook 2', 'chapbook2'],

  astNodeTypes: CHAPBOOK_AST_NODE_TYPES,
  tokenTypes: CHAPBOOK_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: SPECIAL_PASSAGES,

  macroBodyStyle: MacroBodyStyle.Inline,
  macroDelimiters: {
    open: '{',
    close: '}',
  } satisfies MacroDelimiters,
  macroPattern: /\{([\w\s]+?)(?:[\s,:}]|$\})/g,

  // ── Capability: Macros (inserts in Chapbook's case) ──────────
  macros: {
    builtins: CHAPBOOK_INSERTS,
    aliases: ALIAS_MAP,
  } satisfies MacroCapability,

  // ── Capability: Variables ────────────────────────────────────
  variables: {
    sigils: [],  // Chapbook has no single-character sigils; uses prefix notation
    assignmentMacros: new Set(['toggle', 'confirm', 'prompt', 'cycle', 'select link']),
    assignmentOperators: ['='],
    comparisonOperators: [],
    variablePattern: /(var|temp)\.([a-zA-Z_][a-zA-Z0-9_]*)/g,
    triggerChars: ['v'],  // 'v' triggers because 'var.' is how variables start
  } satisfies VariableCapability,

  // ── Capability: Diagnostics ──────────────────────────────────
  diagnostics: {
    rules: [
      { id: 'unknown-insert',          description: 'Use of an unrecognized insert name',         defaultSeverity: 'warning', scope: 'passage' },
      { id: 'invalid-insert-syntax',   description: 'Malformed insert syntax',                     defaultSeverity: 'error',   scope: 'passage' },
      { id: 'unclosed-front-matter',   description: 'Front matter block missing closing ---',       defaultSeverity: 'error',   scope: 'passage' },
      { id: 'malformed-front-matter',  description: 'Unrecognized entry in front matter block',     defaultSeverity: 'warning', scope: 'passage' },
      { id: 'unknown-modifier',        description: 'Use of an unrecognized modifier in [brackets]', defaultSeverity: 'warning', scope: 'passage' },
    ],
    customCheck: customDiagCheck,
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
};
