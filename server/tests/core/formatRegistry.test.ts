import { strict as assert } from 'assert';
import { FormatRegistry } from '../../src/formats/registry';

describe('FormatRegistry', () => {
  describe('resolve', () => {
    it('should resolve exact format id', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      assert.strictEqual(adapter.id, 'sugarcube-2');
      assert.strictEqual(adapter.displayName, 'SugarCube 2');
    });

    it('should resolve sugarcube alias', () => {
      const adapter = FormatRegistry.resolve('sugarcube');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should resolve SugarCube 2 alias', () => {
      const adapter = FormatRegistry.resolve('SugarCube 2');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should resolve sugarcube2 alias', () => {
      const adapter = FormatRegistry.resolve('sugarcube2');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should resolve sugar cube alias', () => {
      const adapter = FormatRegistry.resolve('sugar cube');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should resolve versioned format id', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2.37.3');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should resolve SugarCube with version', () => {
      const adapter = FormatRegistry.resolve('SugarCube-2.37.3');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should return fallback for empty id', () => {
      const adapter = FormatRegistry.resolve('');
      assert.strictEqual(adapter.id, 'fallback');
      assert.strictEqual(adapter.displayName, 'Unknown Format');
    });

    it('should return fallback for unknown format', () => {
      const adapter = FormatRegistry.resolve('unknown-format');
      assert.strictEqual(adapter.id, 'fallback');
    });

    it('should handle case insensitivity', () => {
      const adapter1 = FormatRegistry.resolve('SUGARCUBE-2');
      assert.strictEqual(adapter1.id, 'sugarcube-2');

      const adapter2 = FormatRegistry.resolve('SUGARCUBE');
      assert.strictEqual(adapter2.id, 'sugarcube-2');
    });

    it('should handle whitespace in format id', () => {
      const adapter = FormatRegistry.resolve('  sugarcube-2  ');
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });
  });

  describe('registeredIds', () => {
    it('should return list of registered format ids', () => {
      const ids = FormatRegistry.registeredIds();
      assert.ok(Array.isArray(ids));
      assert.ok(ids.includes('sugarcube-2'));
    });

    it('should not include fallback in registered ids', () => {
      const ids = FormatRegistry.registeredIds();
      assert.ok(!ids.includes('fallback'));
    });
  });

  describe('Adapter functionality through registry', () => {
    it('should provide completions for resolved adapter', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(Array.isArray(completions));
      // Should have builtin macro completions when in macro context
      assert.ok(completions.length > 0);
    });

    it('should provide hover for builtin macros', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'if' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(hover !== null);
      assert.ok(hover!.includes('if'));
    });

    it('should return null hover for unknown macros', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'nonexistent' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.strictEqual(hover, null);
    });

    it('should describe variable sigils', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      
      const storySigil = adapter.describeVariableSigil('$');
      assert.ok(storySigil !== null);
      assert.ok(storySigil!.includes('story'));

      const tempSigil = adapter.describeVariableSigil('_');
      assert.ok(tempSigil !== null);
      assert.ok(tempSigil!.includes('temporary'));

      const invalidSigil = adapter.describeVariableSigil('@');
      assert.strictEqual(invalidSigil, null);
    });

    it('should provide block macro names', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const blockMacros = adapter.getBlockMacroNames();
      
      assert.ok(blockMacros.has('if'));
      assert.ok(blockMacros.has('for'));
      assert.ok(blockMacros.has('switch'));
      assert.ok(blockMacros.has('widget'));
    });

    it('should build macro snippets', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      
      // Block macro snippet
      const blockSnippet = adapter.buildMacroSnippet('if', true);
      assert.ok(blockSnippet !== null);
      assert.ok(blockSnippet!.includes('<</if'));

      // Inline macro snippet
      const inlineSnippet = adapter.buildMacroSnippet('set', false);
      assert.ok(inlineSnippet !== null);
      assert.ok(!inlineSnippet!.includes('<</'));
    });

    it('should provide virtual runtime prelude', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.length > 0);
      assert.ok(prelude.includes('State'));
      assert.ok(prelude.includes('Engine'));
      assert.ok(prelude.includes('Story'));
    });

    it('should provide empty diagnostics', () => {
      const adapter = FormatRegistry.resolve('sugarcube-2');
      const diagnostics = adapter.provideDiagnostics(
        { text: '', uri: 'test://test.tw' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.deepStrictEqual(diagnostics, []);
    });
  });

  describe('Fallback adapter behavior', () => {
    it('should return empty completions for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'unknown', passageNames: [] }
      );
      
      assert.deepStrictEqual(completions, []);
    });

    it('should return null hover for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'if' },
        { formatId: 'unknown', passageNames: [] }
      );
      
      assert.strictEqual(hover, null);
    });

    it('should return null sigil description for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const sigil = adapter.describeVariableSigil('$');
      
      assert.strictEqual(sigil, null);
    });

    it('should return empty block macro names for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const blockMacros = adapter.getBlockMacroNames();
      
      assert.strictEqual(blockMacros.size, 0);
    });

    it('should return null snippet for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const snippet = adapter.buildMacroSnippet('test', false);
      
      assert.strictEqual(snippet, null);
    });

    it('should return empty prelude for fallback', () => {
      const adapter = FormatRegistry.resolve('unknown');
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.strictEqual(prelude, '');
    });
  });
});
