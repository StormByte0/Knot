/**
 * Knot v2 — Format Module Conformance Tests
 *
 * Tests that all format modules conform to the FormatModule interface:
 *   - All required fields exist and have correct types
 *   - Capability bag presence per format
 *   - Cross-format consistency (unique formatIds)
 *   - Specific format features (SugarCube macros, Harlowe sigils, etc.)
 */

import * as assert from 'assert';
import type { FormatModule } from '../../../server/src/formats/_types';
import { MacroBodyStyle } from '../../../server/src/hooks/hookTypes';

import { fallbackModule } from '../../../server/src/formats/fallback/index';
import { sugarcubeModule } from '../../../server/src/formats/sugarcube/index';
import { harloweModule } from '../../../server/src/formats/harlowe/index';
import { chapbookModule } from '../../../server/src/formats/chapbook/index';
import { snowmanModule } from '../../../server/src/formats/snowman/index';

// ─── Required fields existence and types ────────────────────────

const ALL_MODULES: { name: string; mod: FormatModule }[] = [
  { name: 'fallbackModule', mod: fallbackModule },
  { name: 'sugarcubeModule', mod: sugarcubeModule },
  { name: 'harloweModule', mod: harloweModule },
  { name: 'chapbookModule', mod: chapbookModule },
  { name: 'snowmanModule', mod: snowmanModule },
];

const REQUIRED_STRING_FIELDS: (keyof FormatModule)[] = [
  'formatId', 'displayName', 'version',
];

const REQUIRED_FIELDS: (keyof FormatModule)[] = [
  'formatId', 'displayName', 'version', 'aliases',
  'astNodeTypes', 'tokenTypes',
  'lexBody', 'extractPassageRefs', 'resolveLinkBody',
  'specialPassages', 'macroBodyStyle',
  'macroDelimiters', 'macroPattern',
];

for (const { name, mod } of ALL_MODULES) {
  describe(`${name} — FormatModule conformance`, () => {

    // ── Required fields exist ───────────────────────────────────

    for (const field of REQUIRED_FIELDS) {
      it(`should have required field: ${field}`, () => {
        assert.ok(mod[field] !== undefined, `${name} missing required field: ${field}`);
      });
    }

    // ── Required string fields are non-empty strings ────────────

    for (const field of REQUIRED_STRING_FIELDS) {
      it(`should have non-empty string for: ${field}`, () => {
        assert.strictEqual(typeof mod[field], 'string', `${name}.${field} should be string`);
        assert.ok((mod[field] as string).length > 0, `${name}.${field} should be non-empty`);
      });
    }

    // ── Identity fields ─────────────────────────────────────────

    it('should have a formatId that is a non-empty string', () => {
      assert.ok(mod.formatId.length > 0);
    });

    it('should have aliases as an array', () => {
      assert.ok(Array.isArray(mod.aliases));
    });

    // ── AST node types ──────────────────────────────────────────

    it('should have astNodeTypes with baseline types (Document, PassageHeader, PassageBody, Link, Text)', () => {
      assert.ok(mod.astNodeTypes.Document, 'Missing Document');
      assert.ok(mod.astNodeTypes.PassageHeader, 'Missing PassageHeader');
      assert.ok(mod.astNodeTypes.PassageBody, 'Missing PassageBody');
      assert.ok(mod.astNodeTypes.Link, 'Missing Link');
      assert.ok(mod.astNodeTypes.Text, 'Missing Text');
    });

    it('should have astNodeTypes.types as a Map', () => {
      assert.ok(mod.astNodeTypes.types instanceof Map);
    });

    // ── Token types ─────────────────────────────────────────────

    it('should have tokenTypes as a non-empty array', () => {
      assert.ok(Array.isArray(mod.tokenTypes));
      assert.ok(mod.tokenTypes.length > 0);
    });

    // ── Functions ───────────────────────────────────────────────

    it('lexBody should be a function', () => {
      assert.strictEqual(typeof mod.lexBody, 'function');
    });

    it('extractPassageRefs should be a function', () => {
      assert.strictEqual(typeof mod.extractPassageRefs, 'function');
    });

    it('resolveLinkBody should be a function', () => {
      assert.strictEqual(typeof mod.resolveLinkBody, 'function');
    });

    // ── specialPassages ─────────────────────────────────────────

    it('should have specialPassages as an array', () => {
      assert.ok(Array.isArray(mod.specialPassages));
    });

    // ── macroBodyStyle ──────────────────────────────────────────

    it('should have a valid macroBodyStyle', () => {
      const validStyles = [MacroBodyStyle.CloseTag, MacroBodyStyle.Hook, MacroBodyStyle.Inline];
      assert.ok(validStyles.includes(mod.macroBodyStyle), `Invalid MacroBodyStyle: ${mod.macroBodyStyle}`);
    });

    // ── macroDelimiters ─────────────────────────────────────────

    it('should have macroDelimiters with open and close strings', () => {
      assert.strictEqual(typeof mod.macroDelimiters.open, 'string');
      assert.strictEqual(typeof mod.macroDelimiters.close, 'string');
    });

    // ── macroPattern ────────────────────────────────────────────

    it('should have macroPattern as RegExp or null', () => {
      assert.ok(
        mod.macroPattern === null || mod.macroPattern instanceof RegExp,
        `macroPattern should be RegExp or null, got ${typeof mod.macroPattern}`
      );
    });

    // ── lexBody() returns array ─────────────────────────────────

    it('lexBody() should return an array of tokens', () => {
      const tokens = mod.lexBody('some body text', 0);
      assert.ok(Array.isArray(tokens));
    });

    // ── extractPassageRefs() returns array ──────────────────────

    it('extractPassageRefs() should return an array of refs', () => {
      const refs = mod.extractPassageRefs('some body text', 0);
      assert.ok(Array.isArray(refs));
    });

    // ── resolveLinkBody() returns LinkResolution ────────────────

    it('resolveLinkBody() should return a valid LinkResolution', () => {
      const result = mod.resolveLinkBody('Target');
      assert.ok(result, 'Should return a result');
      assert.strictEqual(typeof result.target, 'string', 'target should be string');
      assert.ok(result.kind !== undefined, 'kind should be defined');
    });
  });
}

// ─── Cross-Format Consistency ───────────────────────────────────

describe('Cross-format consistency', () => {
  const modules = ALL_MODULES.map(({ mod }) => mod);

  it('all formatIds should be unique', () => {
    const ids = modules.map(m => m.formatId);
    const uniqueIds = new Set(ids);
    assert.strictEqual(ids.length, uniqueIds.size, 'All formatIds should be unique');
  });

  it('all formats should have non-empty version', () => {
    for (const mod of modules) {
      assert.ok(mod.version.length > 0, `${mod.formatId} has empty version`);
    }
  });

  it('all formats should have non-empty displayName', () => {
    for (const mod of modules) {
      assert.ok(mod.displayName.length > 0, `${mod.formatId} has empty displayName`);
    }
  });
});

// ─── Capability Bag Presence Per Format ─────────────────────────

describe('Capability bag presence per format', () => {

  describe('fallbackModule', () => {
    it('should have NO capability bags', () => {
      assert.strictEqual(fallbackModule.macros, undefined, 'fallback should not have macros');
      assert.strictEqual(fallbackModule.variables, undefined, 'fallback should not have variables');
      assert.strictEqual(fallbackModule.customMacros, undefined, 'fallback should not have customMacros');
      assert.strictEqual(fallbackModule.diagnostics, undefined, 'fallback should not have diagnostics');
      assert.strictEqual(fallbackModule.snippets, undefined, 'fallback should not have snippets');
      assert.strictEqual(fallbackModule.runtime, undefined, 'fallback should not have runtime');
    });

    it('should have MacroBodyStyle.Inline', () => {
      assert.strictEqual(fallbackModule.macroBodyStyle, MacroBodyStyle.Inline);
    });

    it('should have null macroPattern', () => {
      assert.strictEqual(fallbackModule.macroPattern, null);
    });

    it('should have empty specialPassages', () => {
      assert.strictEqual(fallbackModule.specialPassages.length, 0);
    });
  });

  describe('sugarcubeModule', () => {
    it('should have macros capability', () => {
      assert.ok(sugarcubeModule.macros, 'SugarCube should have macros');
      assert.ok(sugarcubeModule.macros!.builtins.length > 0, 'Should have builtins');
      assert.ok(sugarcubeModule.macros!.aliases instanceof Map, 'aliases should be a Map');
    });

    it('should have variables capability', () => {
      assert.ok(sugarcubeModule.variables, 'SugarCube should have variables');
      assert.strictEqual(sugarcubeModule.variables!.sigils.length, 2, 'Should have $ and _ sigils');
      assert.ok(sugarcubeModule.variables!.assignmentMacros instanceof Set);
    });

    it('should have customMacros capability', () => {
      assert.ok(sugarcubeModule.customMacros, 'SugarCube should have customMacros');
      assert.ok(sugarcubeModule.customMacros!.definitionMacros.has('widget'), 'Should have widget definition macro');
    });

    it('should have diagnostics capability', () => {
      assert.ok(sugarcubeModule.diagnostics, 'SugarCube should have diagnostics');
      assert.ok(sugarcubeModule.diagnostics!.rules.length > 0, 'Should have diagnostic rules');
    });

    it('should have snippets capability', () => {
      assert.ok(sugarcubeModule.snippets, 'SugarCube should have snippets');
    });

    it('should have runtime capability', () => {
      assert.ok(sugarcubeModule.runtime, 'SugarCube should have runtime');
      assert.ok(sugarcubeModule.runtime!.globals.length > 0, 'Should have runtime globals');
    });

    it('should have MacroBodyStyle.CloseTag', () => {
      assert.strictEqual(sugarcubeModule.macroBodyStyle, MacroBodyStyle.CloseTag);
    });

    it('should have << >> macro delimiters', () => {
      assert.strictEqual(sugarcubeModule.macroDelimiters.open, '<<');
      assert.strictEqual(sugarcubeModule.macroDelimiters.close, '>>');
      assert.strictEqual(sugarcubeModule.macroDelimiters.closeTagPrefix, '/');
    });

    it('should have a non-null macroPattern', () => {
      assert.ok(sugarcubeModule.macroPattern instanceof RegExp);
    });

    it('should have specialPassages for StoryInit, PassageHeader, etc.', () => {
      assert.ok(sugarcubeModule.specialPassages.length > 0);
      const names = sugarcubeModule.specialPassages.map(sp => sp.name);
      assert.ok(names.includes('StoryInit'), 'Should have StoryInit');
      assert.ok(names.includes('PassageHeader'), 'Should have PassageHeader');
      assert.ok(names.includes('PassageFooter'), 'Should have PassageFooter');
    });

    it('should have tag-based specialPassages (widget tag)', () => {
      const widgetSp = sugarcubeModule.specialPassages.find(sp => sp.tag === 'widget');
      assert.ok(widgetSp, 'Should have widget tag-based special passage');
    });

    it('should know common macros (set, if, print)', () => {
      const macroNames = sugarcubeModule.macros!.builtins.map(m => m.name);
      assert.ok(macroNames.includes('set'), 'Should have set macro');
      assert.ok(macroNames.includes('if'), 'Should have if macro');
      assert.ok(macroNames.includes('print'), 'Should have print macro');
    });
  });

  describe('harloweModule', () => {
    it('should have macros capability', () => {
      assert.ok(harloweModule.macros, 'Harlowe should have macros');
      assert.ok(harloweModule.macros!.builtins.length > 0, 'Should have builtins');
      assert.ok(harloweModule.macros!.aliases instanceof Map, 'aliases should be a Map');
    });

    it('should have variables capability', () => {
      assert.ok(harloweModule.variables, 'Harlowe should have variables');
      assert.strictEqual(harloweModule.variables!.sigils.length, 2, 'Should have $ and _ sigils');
    });

    it('should have customMacros capability', () => {
      assert.ok(harloweModule.customMacros, 'Harlowe should have customMacros');
      assert.ok(harloweModule.customMacros!.definitionMacros.has('macro:'), 'Should have macro: definition macro');
    });

    it('should have diagnostics capability', () => {
      assert.ok(harloweModule.diagnostics, 'Harlowe should have diagnostics');
      assert.ok(harloweModule.diagnostics!.rules.length > 0, 'Should have diagnostic rules');
    });

    it('should have snippets capability', () => {
      assert.ok(harloweModule.snippets, 'Harlowe should have snippets');
    });

    it('should have runtime capability', () => {
      assert.ok(harloweModule.runtime, 'Harlowe should have runtime');
    });

    it('should have MacroBodyStyle.Hook', () => {
      assert.strictEqual(harloweModule.macroBodyStyle, MacroBodyStyle.Hook);
    });

    it('should have ( ) macro delimiters', () => {
      assert.strictEqual(harloweModule.macroDelimiters.open, '(');
      assert.strictEqual(harloweModule.macroDelimiters.close, ')');
      assert.strictEqual(harloweModule.macroDelimiters.closeTagPrefix, undefined);
    });

    it('should have a non-null macroPattern', () => {
      assert.ok(harloweModule.macroPattern instanceof RegExp);
    });

    it('should have specialPassages for Header, Footer, Startup', () => {
      assert.ok(harloweModule.specialPassages.length > 0);
      const names = harloweModule.specialPassages.map(sp => sp.name);
      assert.ok(names.includes('Header'), 'Should have Header');
      assert.ok(names.includes('Footer'), 'Should have Footer');
      assert.ok(names.includes('Startup'), 'Should have Startup');
    });

    it('should know common macros (set:, if:, print:)', () => {
      const macroNames = harloweModule.macros!.builtins.map(m => m.name);
      assert.ok(macroNames.includes('set:'), 'Should have set: macro');
      assert.ok(macroNames.includes('if:'), 'Should have if: macro');
      assert.ok(macroNames.includes('print:'), 'Should have print: macro');
    });

    it('should resolve links with -> and <- arrows', () => {
      const link1 = harloweModule.resolveLinkBody('Click here->Next Room');
      assert.strictEqual(link1.target, 'Next Room');
      assert.strictEqual(link1.displayText, 'Click here');

      const link2 = harloweModule.resolveLinkBody('Next Room<-Click here');
      assert.strictEqual(link2.target, 'Next Room');
      assert.strictEqual(link2.displayText, 'Click here');
    });

    it('should detect external URLs in links', () => {
      const link = harloweModule.resolveLinkBody('https://example.com');
      assert.ok(link.kind === 'external', 'URLs should be external links');
    });

    it('should NOT use pipe | as a link separator', () => {
      const link = harloweModule.resolveLinkBody('Text|Target');
      // In Harlowe, | is for hook nametags, not link separators
      assert.ok(link.target.includes('|'), 'Pipe should not be treated as separator');
    });
  });

  describe('chapbookModule', () => {
    it('should have macros capability (inserts modeled as macros)', () => {
      assert.ok(chapbookModule.macros, 'Chapbook should have macros (inserts)');
    });

    it('should have variables capability', () => {
      assert.ok(chapbookModule.variables, 'Chapbook should have variables');
    });

    it('should NOT have customMacros capability', () => {
      assert.strictEqual(chapbookModule.customMacros, undefined, 'Chapbook should not have customMacros');
    });

    it('should have diagnostics capability', () => {
      assert.ok(chapbookModule.diagnostics, 'Chapbook should have diagnostics');
    });

    it('should have snippets capability', () => {
      assert.ok(chapbookModule.snippets, 'Chapbook should have snippets');
    });

    it('should have runtime capability', () => {
      assert.ok(chapbookModule.runtime, 'Chapbook should have runtime');
    });

    it('should have MacroBodyStyle.Inline', () => {
      assert.strictEqual(chapbookModule.macroBodyStyle, MacroBodyStyle.Inline);
    });

    it('should have { } delimiters (for inserts)', () => {
      assert.strictEqual(chapbookModule.macroDelimiters.open, '{');
      assert.strictEqual(chapbookModule.macroDelimiters.close, '}');
    });

    it('should have empty specialPassages', () => {
      assert.strictEqual(chapbookModule.specialPassages.length, 0);
    });

    it('should have a non-null macroPattern for inserts', () => {
      assert.ok(chapbookModule.macroPattern instanceof RegExp);
    });
  });

  describe('snowmanModule', () => {
    it('should NOT have macros capability', () => {
      assert.strictEqual(snowmanModule.macros, undefined, 'Snowman should not have macros');
    });

    it('should have variables capability', () => {
      assert.ok(snowmanModule.variables, 'Snowman should have variables');
    });

    it('should NOT have customMacros capability', () => {
      assert.strictEqual(snowmanModule.customMacros, undefined, 'Snowman should not have customMacros');
    });

    it('should have diagnostics capability', () => {
      assert.ok(snowmanModule.diagnostics, 'Snowman should have diagnostics');
    });

    it('should have snippets capability', () => {
      assert.ok(snowmanModule.snippets, 'Snowman should have snippets');
    });

    it('should have runtime capability', () => {
      assert.ok(snowmanModule.runtime, 'Snowman should have runtime');
    });

    it('should have MacroBodyStyle.Inline', () => {
      assert.strictEqual(snowmanModule.macroBodyStyle, MacroBodyStyle.Inline);
    });

    it('should have <% %> delimiters (for templates)', () => {
      assert.strictEqual(snowmanModule.macroDelimiters.open, '<%');
      assert.strictEqual(snowmanModule.macroDelimiters.close, '%>');
    });

    it('should have null macroPattern (no named macro syntax)', () => {
      assert.strictEqual(snowmanModule.macroPattern, null);
    });

    it('should have empty specialPassages', () => {
      assert.strictEqual(snowmanModule.specialPassages.length, 0);
    });

    it('should use s.name / t.name variable patterns', () => {
      assert.ok(snowmanModule.variables!.variablePattern instanceof RegExp);
      // s.name and t.name patterns should match
      const pattern = snowmanModule.variables!.variablePattern;
      pattern.lastIndex = 0;
      assert.ok(pattern.exec('s.myVar'), 'Should match s.myVar');
      pattern.lastIndex = 0;
      assert.ok(pattern.exec('t.myVar'), 'Should match t.myVar');
    });
  });
});
