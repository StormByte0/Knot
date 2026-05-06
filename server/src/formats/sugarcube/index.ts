/**
 * Knot v2 — SugarCube 2 Format Module
 *
 * Implements FormatModule for the SugarCube 2 story format.
 * Exports exactly one object: `sugarcubeModule`.
 *
 * Key SugarCube characteristics:
 *   - Macros use <<name args>> / <</name>> close-tag syntax
 *   - MacroBodyStyle.CloseTag — bodies end at <</name>> close tags
 *   - Variables: $story (persists), _temp (passage-scoped)
 *   - Links: pipe | first, then ->, then <-
 *   - Special passages: StoryInit, PassageHeader, PassageFooter, etc.
 *   - Custom macros: <<widget>> and Macro.add()
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
  CustomMacroCapability,
  DiagnosticCapability,
} from '../_types';

import {
  MacroBodyStyle,
} from '../../hooks/hookTypes';

// Macro catalog is split into category files — see ./macros-index.ts
import { SUGARCUBE_MACROS, ALIAS_MAP } from './macros-index';

// Other sections extracted for maintainability
import { SPECIAL_PASSAGES } from './specialPassages';
import { RUNTIME_GLOBALS, VIRTUAL_RUNTIME_PRELUDE } from './runtime';
import { SNIPPET_TEMPLATES } from './snippets';
import { lexBody, extractPassageRefs, resolveLinkBody } from './lexer';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const SUGARCUBE_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',       label: 'Document',         canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader',  label: 'Passage Header',   canHaveChildren: false },
    { id: 'PassageBody',    label: 'Passage Body',     canHaveChildren: true,  childNodeTypeIds: ['MacroCall', 'Variable', 'Link', 'Text'] },
    { id: 'Link',           label: 'Link',             canHaveChildren: false },
    { id: 'Text',           label: 'Text',             canHaveChildren: false },
    { id: 'MacroCall',      label: 'Macro Call',       canHaveChildren: true,  childNodeTypeIds: ['MacroCall', 'Variable', 'Link', 'Text'] },
    { id: 'MacroClose',     label: 'Macro Close Tag',  canHaveChildren: false },
    { id: 'Variable',       label: 'Variable',         canHaveChildren: false },
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

const SUGARCUBE_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'macro-call',   label: 'Macro Call',     category: 'delimiter' },
  { id: 'macro-close',  label: 'Macro Close Tag',category: 'delimiter' },
  { id: 'variable',     label: 'Variable',       category: 'identifier' },
  { id: 'text',         label: 'Text',           category: 'literal' },
  { id: 'newline',      label: 'Newline',        category: 'whitespace' },
  { id: 'eof',          label: 'EOF',            category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const sugarcubeModule: FormatModule = {
  formatId: 'sugarcube-2',
  displayName: 'SugarCube 2',
  version: '2.36.0',
  aliases: ['sugarcube', 'SugarCube 2', 'sugarcube2', 'sugar cube'],

  astNodeTypes: SUGARCUBE_AST_NODE_TYPES,
  tokenTypes: SUGARCUBE_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: SPECIAL_PASSAGES,

  macroBodyStyle: MacroBodyStyle.CloseTag,
  macroDelimiters: {
    open: '<<',
    close: '>>',
    closeTagPrefix: '/',
  } satisfies MacroDelimiters,
  macroPattern: /<<(\w+)(?:\s+[^>]*?)?>>/g,

  // ── Capability: Macros ──────────────────────────────────────
  macros: {
    builtins: SUGARCUBE_MACROS,
    aliases: ALIAS_MAP,
  } satisfies MacroCapability,

  // ── Capability: Variables ───────────────────────────────────
  variables: {
    sigils: [
      { sigil: '$', kind: 'story', description: 'SugarCube story variable — persists across passages' },
      { sigil: '_', kind: 'temp',  description: 'SugarCube temporary variable — scoped to current passage' },
    ],
    assignmentMacros: new Set(['set', 'capture', 'remember', 'textbox', 'textbox2', 'numberbox', 'numberbox2', 'textarea', 'textarea2', 'checkbox', 'radiobutton', 'listbox', 'dropdown', 'input']),
    assignmentOperators: ['to', '='],
    comparisonOperators: ['gt', 'gte', 'lt', 'lte', 'eq', 'neq', 'is', 'isnot'],
    variablePattern: /([$_])(\w+)/g,
    triggerChars: ['$', '_'],
  } satisfies VariableCapability,

  // ── Capability: Custom Macros ───────────────────────────────
  customMacros: {
    definitionMacros: new Set(['widget']),
    scriptPatterns: [
      {
        pattern: /Macro\s*\.\s*add\s*\(\s*["']([^"']+)["']/g,
        macroNameGroup: 1,
        description: 'Macro.add() call',
      },
    ],
    expandsBodyLinks: true,
  } satisfies CustomMacroCapability,

  // ── Capability: Diagnostics ─────────────────────────────────
  diagnostics: {
    rules: [
      { id: 'unknown-macro',       description: 'Use of an unknown macro',                    defaultSeverity: 'warning', scope: 'passage' },
      { id: 'deprecated-macro',    description: 'Use of a deprecated macro',                  defaultSeverity: 'warning', scope: 'passage' },
      { id: 'missing-argument',    description: 'Missing required macro argument',            defaultSeverity: 'error',   scope: 'passage' },
      { id: 'container-structure', description: 'Invalid macro container nesting',            defaultSeverity: 'error',   scope: 'passage' },
    ],
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
