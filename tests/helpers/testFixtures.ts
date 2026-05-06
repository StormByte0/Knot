/**
 * Knot v2 — Test Fixtures
 *
 * Shared mock FormatModule for testing core/handler code.
 * CRITICAL: Tests for core/handlers MUST use these mocks,
 * never real format modules. This enforces the boundary.
 *
 * The old IFormatProvider / IMacroProvider / IPassageProvider etc.
 * class-based mocks have been replaced by a single mock FormatModule
 * object literal conforming to the FormatModule interface.
 */

import type {
  FormatModule,
  FormatASTNodeTypes,
  ASTNodeTypeDef,
  TokenTypeDef,
  MacroDef,
  MacroDelimiters,
  LinkResolution,
  PassageRef,
  BodyToken,
} from '../../server/src/formats/_types';

import {
  MacroCategory,
  MacroKind,
  MacroBodyStyle,
  LinkKind,
  PassageRefKind,
} from '../../server/src/hooks/hookTypes';

import { FormatRegistry } from '../../server/src/formats/formatRegistry';

// ─── Baseline AST Node Types (used by mock module) ──────────────

const MOCK_AST_NODE_TYPES: FormatASTNodeTypes = (() => {
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

// ─── Baseline Token Types ────────────────────────────────────────

const MOCK_TOKEN_TYPES: TokenTypeDef[] = [
  { id: 'text',    label: 'Text',    category: 'literal' },
  { id: 'newline', label: 'Newline', category: 'whitespace' },
  { id: 'eof',     label: 'EOF',     category: 'whitespace' },
];

// ─── Mock FormatModule Factory ───────────────────────────────────

/**
 * Create a mock FormatModule object literal.
 * Defaults to a minimal baseline module (no capability bags).
 * Pass `overrides` to customize fields or add capability bags.
 */
export function createMockFormatModule(overrides?: Partial<FormatModule>): FormatModule {
  return {
    formatId: 'mock',
    displayName: 'Mock Format',
    version: '0.0.1',
    aliases: ['mock'],

    astNodeTypes: MOCK_AST_NODE_TYPES,
    tokenTypes: MOCK_TOKEN_TYPES,

    lexBody: (_input: string, baseOffset: number): BodyToken[] => {
      return [{ typeId: 'eof', text: '', range: { start: baseOffset, end: baseOffset } }];
    },

    extractPassageRefs: (body: string, bodyOffset: number): PassageRef[] => {
      // Default: find [[ ]] links only
      const refs: PassageRef[] = [];
      const linkRe = /\[\[([^\]]+?)\]\]/g;
      linkRe.lastIndex = 0;
      let match: RegExpExecArray | null;
      while ((match = linkRe.exec(body)) !== null) {
        const rawBody = match[1];
        const target = rawBody.trim();
        refs.push({
          target,
          kind: PassageRefKind.Link,
          range: { start: bodyOffset + match.index, end: bodyOffset + match.index + match[0].length },
          source: '[[ ]]',
          linkKind: LinkKind.Passage,
        });
      }
      return refs;
    },

    resolveLinkBody: (rawBody: string): LinkResolution => {
      const ra = rawBody.lastIndexOf('->');
      if (ra >= 0) {
        return {
          target: rawBody.substring(ra + 2).trim(),
          displayText: rawBody.substring(0, ra).trim(),
          kind: LinkKind.Passage,
        };
      }
      return { target: rawBody.trim(), kind: LinkKind.Passage };
    },

    specialPassages: [],

    macroBodyStyle: MacroBodyStyle.Inline,

    macroDelimiters: {
      open: '',
      close: '',
    } satisfies MacroDelimiters,

    macroPattern: null,

    ...overrides,
  };
}

// ─── Convenience: Mock Registry ──────────────────────────────────

/**
 * Create a FormatRegistry pre-loaded with a mock FormatModule
 * set as the active format. Useful for parser and core tests
 * that need a FormatRegistry but should NOT import real format modules.
 */
export function createMockRegistry(mockModule?: FormatModule): FormatRegistry {
  const registry = new FormatRegistry();
  const mod = mockModule ?? createMockFormatModule();
  registry.register(mod);
  registry.setActiveFormat('mock');
  return registry;
}

// ─── Convenience: Mock module with macro capability ──────────────

/**
 * Create a mock FormatModule with a macros capability bag
 * containing the given macro definitions.
 */
export function createMockFormatModuleWithMacros(macros: MacroDef[]): FormatModule {
  const aliasMap = new Map<string, string>();
  for (const macro of macros) {
    if (macro.aliases) {
      for (const alias of macro.aliases) {
        aliasMap.set(alias, macro.name);
      }
    }
  }

  return createMockFormatModule({
    macros: {
      builtins: macros,
      aliases: aliasMap,
    },
  });
}

// ─── Convenience: Sample macro definition factory ────────────────

/**
 * Create a sample MacroDef for testing.
 * Mirrors the fields from the FormatModule MacroDef interface.
 */
export function createSampleMacroDef(overrides?: Partial<MacroDef>): MacroDef {
  return {
    name: 'testMacro',
    category: MacroCategory.Output,
    kind: MacroKind.Command,
    description: 'A test macro',
    signatures: [{ args: [{ name: 'arg1', type: 'string', required: true }] }],
    ...overrides,
  };
}
