/**
 * Knot v2 — Harlowe 3 Format Module
 *
 * Implements FormatModule for the Harlowe 3 story format.
 * Exports exactly one object: `harloweModule`.
 *
 * Key Harlowe characteristics:
 *   - Macros use (name:) parenthesised syntax with trailing colon
 *   - Changers attach to [...] hooks: (if: $x)[shown text]
 *   - Commands are standalone: (go-to: "Passage")
 *   - Instants have no visible output: (set: $x to 5)
 *   - NO close-tag syntax like <</if>> — hooks close with ]
 *   - Links use -> and <- arrows, NOT pipe |
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
import { HARLOWE_MACROS, ALIAS_MAP } from './macros-index';

// Other sections extracted for maintainability
import { SPECIAL_PASSAGES } from './specialPassages';
import { RUNTIME_GLOBALS, VIRTUAL_RUNTIME_PRELUDE } from './runtime';
import { SNIPPET_TEMPLATES } from './snippets';
import { lexBody, extractPassageRefs, resolveLinkBody } from './lexer';

// ═══════════════════════════════════════════════════════════════════
// AST NODE TYPES
// ═══════════════════════════════════════════════════════════════════

const HARLOWE_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
  const defs: ASTNodeTypeDef[] = [
    { id: 'Document',      label: 'Document',        canHaveChildren: true,  childNodeTypeIds: ['PassageHeader', 'PassageBody'] },
    { id: 'PassageHeader', label: 'Passage Header',  canHaveChildren: false },
    { id: 'PassageBody',   label: 'Passage Body',    canHaveChildren: true,  childNodeTypeIds: ['MacroCall', 'HookOpen', 'HookClose', 'HookName', 'Variable', 'Link', 'Text'] },
    { id: 'Link',          label: 'Link',            canHaveChildren: false },
    { id: 'Text',          label: 'Text',            canHaveChildren: false },
    { id: 'MacroCall',     label: 'Macro Call',      canHaveChildren: true,  childNodeTypeIds: ['MacroCall', 'HookOpen', 'HookClose', 'HookName', 'Variable', 'Link', 'Text'] },
    { id: 'HookOpen',      label: 'Hook Open',       canHaveChildren: false },
    { id: 'HookClose',     label: 'Hook Close',      canHaveChildren: false },
    { id: 'HookName',      label: 'Hook Name',       canHaveChildren: false },
    { id: 'Variable',      label: 'Variable',        canHaveChildren: false },
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

const HARLOWE_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'macro-call',  label: 'Macro Call',  category: 'delimiter' },
  { id: 'hook-open',   label: 'Hook Open',   category: 'delimiter' },
  { id: 'hook-close',  label: 'Hook Close',  category: 'delimiter' },
  { id: 'hook-name',   label: 'Hook Name',   category: 'identifier' },
  { id: 'link',        label: 'Link',        category: 'identifier' },
  { id: 'variable',    label: 'Variable',    category: 'identifier' },
  { id: 'comment',     label: 'Comment',     category: 'literal' },
  { id: 'text',        label: 'Text',        category: 'literal' },
  { id: 'newline',     label: 'Newline',     category: 'whitespace' },
  { id: 'eof',         label: 'EOF',         category: 'whitespace' },
];

// ═══════════════════════════════════════════════════════════════════
// THE MODULE EXPORT
// ═══════════════════════════════════════════════════════════════════

export const harloweModule: FormatModule = {
  formatId: 'harlowe-3',
  displayName: 'Harlowe 3',
  version: '3.3.8',
  aliases: ['harlowe', 'Harlowe 3', 'harlowe3'],

  astNodeTypes: HARLOWE_AST_NODE_TYPES,
  tokenTypes: HARLOWE_TOKEN_TYPES,

  lexBody,
  extractPassageRefs,
  resolveLinkBody,
  specialPassages: SPECIAL_PASSAGES,

  macroBodyStyle: MacroBodyStyle.Hook,
  macroDelimiters: {
    open: '(',
    close: ')',
  } satisfies MacroDelimiters,
  macroPattern: /\((\w[\w-]*:)(?:\s[^)]*?)?\)/g,

  // ── Capability: Macros ──────────────────────────────────────
  macros: {
    builtins: HARLOWE_MACROS,
    aliases: ALIAS_MAP,
  } satisfies MacroCapability,

  // ── Capability: Variables ───────────────────────────────────
  variables: {
    sigils: [
      { sigil: '$', kind: 'story', description: 'Harlowe story variable — persists across passages' },
      { sigil: '_', kind: 'temp',  description: 'Harlowe temporary variable — scoped to current passage' },
    ],
    assignmentMacros: new Set(['set:', 'put:', 'move:']),
    assignmentOperators: ['to', 'into'],
    comparisonOperators: [],
    variablePattern: /([$_])(\w+)/g,
    triggerChars: ['$'],
  } satisfies VariableCapability,

  // ── Capability: Custom Macros ───────────────────────────────
  customMacros: {
    definitionMacros: new Set(['macro:']),
    scriptPatterns: [],
    expandsBodyLinks: false,
  } satisfies CustomMacroCapability,

  // ── Capability: Diagnostics ─────────────────────────────────
  diagnostics: {
    rules: [
      { id: 'unknown-macro',           description: 'Use of an unknown macro',             defaultSeverity: 'warning', scope: 'passage' },
      { id: 'deprecated-macro',        description: 'Use of a deprecated macro',           defaultSeverity: 'warning', scope: 'passage' },
      { id: 'container-structure',     description: 'Invalid changer nesting',              defaultSeverity: 'error',   scope: 'passage' },
      { id: 'invalid-hook-structure',  description: 'Hook without associated changer',     defaultSeverity: 'warning', scope: 'passage' },
      { id: 'invalid-changer-binding', description: 'Changer attached to wrong target',    defaultSeverity: 'warning', scope: 'passage' },
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
