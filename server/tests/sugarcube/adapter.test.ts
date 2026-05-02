import { strict as assert } from 'assert';
import { SugarCubeAdapter } from '../../src/formats/sugarcube/adapter';
import { BUILTINS, BLOCK_MACRO_NAMES, BUILTIN_MAP, BUILTIN_GLOBALS } from '../../src/formats/sugarcube/macros';

describe('SugarCube Adapter', () => {
  let adapter: SugarCubeAdapter;

  beforeEach(() => {
    adapter = new SugarCubeAdapter();
  });

  describe('Basic properties', () => {
    it('should have correct id', () => {
      assert.strictEqual(adapter.id, 'sugarcube-2');
    });

    it('should have correct display name', () => {
      assert.strictEqual(adapter.displayName, 'SugarCube 2');
    });
  });

  describe('Completion - Macro context detection', () => {
    it('should provide completions in macro open context', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(Array.isArray(completions));
      assert.ok(completions.length > 0);
    });

    it('should provide completions while typing macro name', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<set', offset: 5 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(completions.length > 0);
    });

    it('should NOT provide completions outside macro context', () => {
      const completions = adapter.provideFormatCompletions(
        { text: 'Hello world', offset: 5 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.deepStrictEqual(completions, []);
    });

    it('should provide close tag completions', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<if true>>body<</', offset: 18 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      // Should suggest closing tags for open block macros
      assert.ok(completions.length > 0);
      assert.ok(completions.some(c => c.label.includes('if')));
    });

    it('should filter close tag completions by partial match', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<for _i, arr>>body<</f', offset: 24 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(completions.every(c => c.filterText!.startsWith('f')));
    });
  });

  describe('Completion - Built-in macros', () => {
    it('should include control flow macros', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.insertText);
      assert.ok(labels.some(l => l?.includes('if')));
      assert.ok(labels.some(l => l?.includes('for')));
      assert.ok(labels.some(l => l?.includes('switch')));
    });

    it('should include variable macros', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.label);
      assert.ok(labels.some(l => l?.includes('set')));
      assert.ok(labels.some(l => l?.includes('unset')));
    });

    it('should include output macros', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.label);
      assert.ok(labels.some(l => l?.includes('print')));
      assert.ok(labels.some(l => l?.includes('=')));
    });

    it('should include link macros', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.label);
      assert.ok(labels.some(l => l?.includes('link')));
      assert.ok(labels.some(l => l?.includes('button')));
    });

    it('should include navigation macros', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.label);
      assert.ok(labels.some(l => l?.includes('goto')));
      assert.ok(labels.some(l => l?.includes('back')));
    });

    it('should include widget macro', () => {
      const completions = adapter.provideFormatCompletions(
        { text: '<<', offset: 2 },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      const labels = completions.map(c => c.label);
      assert.ok(labels.some(l => l?.includes('widget')));
    });
  });

  describe('Macro snippets', () => {
    it('should build snippet for block macro', () => {
      const snippet = adapter.buildMacroSnippet('if', true);
      
      assert.ok(snippet !== null);
      assert.ok(snippet!.includes('if'));
      assert.ok(snippet!.includes('<</if'));
      assert.ok(snippet!.includes('${1'));
    });

    it('should build snippet for inline macro', () => {
      const snippet = adapter.buildMacroSnippet('set', false);
      
      assert.ok(snippet !== null);
      assert.ok(snippet!.includes('set'));
      assert.ok(!snippet!.includes('<</'));
    });

    it('should detect block macros automatically', () => {
      const snippet = adapter.buildMacroSnippet('if', false);
      
      // 'if' is in BLOCK_MACRO_NAMES, so should be treated as block
      assert.ok(snippet !== null);
      assert.ok(snippet!.includes('<</if'));
    });
  });

  describe('Block macro names', () => {
    it('should include all block macros', () => {
      const blockMacros = adapter.getBlockMacroNames();
      
      assert.ok(blockMacros.has('if'));
      assert.ok(blockMacros.has('elseif'));
      assert.ok(blockMacros.has('else'));
      assert.ok(blockMacros.has('for'));
      assert.ok(blockMacros.has('switch'));
      assert.ok(blockMacros.has('case'));
      assert.ok(blockMacros.has('widget'));
      assert.ok(blockMacros.has('link'));
      assert.ok(blockMacros.has('button'));
    });

    it('should not include non-block macros', () => {
      const blockMacros = adapter.getBlockMacroNames();
      
      assert.ok(!blockMacros.has('set'));
      assert.ok(!blockMacros.has('print'));
      assert.ok(!blockMacros.has('goto'));
      assert.ok(!blockMacros.has('run'));
    });
  });

  describe('Hover', () => {
    it('should provide hover for if macro', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'if' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(hover !== null);
      assert.ok(hover!.includes('if'));
      assert.ok(hover!.includes('Conditional'));
    });

    it('should provide hover for set macro', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'set' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(hover !== null);
      assert.ok(hover!.includes('Assign') || hover!.includes('set'));
    });

    it('should return null for unknown macro', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'macro', rawName: 'nonexistent' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.strictEqual(hover, null);
    });

    it('should provide hover for State global', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'variable', rawName: 'State' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(hover !== null);
      assert.ok(hover!.includes('State'));
    });

    it('should provide hover for Engine global', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'function', rawName: 'Engine' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.ok(hover !== null);
      assert.ok(hover!.includes('Engine'));
    });

    it('should return null for unknown global', () => {
      const hover = adapter.provideBuiltinHover(
        { tokenType: 'variable', rawName: 'unknownGlobal' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.strictEqual(hover, null);
    });
  });

  describe('Variable sigils', () => {
    it('should describe $ sigil', () => {
      const desc = adapter.describeVariableSigil('$');
      
      assert.ok(desc !== null);
      assert.ok(desc!.includes('story'));
      assert.ok(desc!.includes('persist'));
    });

    it('should describe _ sigil', () => {
      const desc = adapter.describeVariableSigil('_');
      
      assert.ok(desc !== null);
      assert.ok(desc!.includes('temporary'));
      assert.ok(desc!.includes('scoped'));
    });

    it('should return null for invalid sigil', () => {
      const desc1 = adapter.describeVariableSigil('@');
      const deknot = adapter.describeVariableSigil('#');
      
      assert.strictEqual(desc1, null);
      assert.strictEqual(deknot, null);
    });
  });

  describe('Virtual runtime prelude', () => {
    it('should include State declaration', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('State'));
      assert.ok(prelude.includes('variables'));
      assert.ok(prelude.includes('passage'));
    });

    it('should include Engine declaration', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('Engine'));
      assert.ok(prelude.includes('play'));
    });

    it('should include Story declaration', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('Story'));
      assert.ok(prelude.includes('title'));
    });

    it('should include SugarCube version', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('SugarCube'));
      assert.ok(prelude.includes('version'));
    });

    it('should include setup object', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('setup'));
    });

    it('should include utility functions', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('visited'));
      assert.ok(prelude.includes('turns'));
      assert.ok(prelude.includes('passage'));
    });

    it('should include $args for widgets', () => {
      const prelude = adapter.getVirtualRuntimePrelude();
      
      assert.ok(prelude.includes('$args'));
    });
  });

  describe('Diagnostics', () => {
    it('should return empty diagnostics array', () => {
      const diagnostics = adapter.provideDiagnostics(
        { text: '', uri: 'test://test.tw' },
        { formatId: 'sugarcube-2', passageNames: [] }
      );
      
      assert.deepStrictEqual(diagnostics, []);
    });
  });
});

describe('SugarCube Macros catalog', () => {
  describe('BUILTINS', () => {
    it('should have all required macros', () => {
      const names = BUILTINS.map(m => m.name);
      
      // Control flow
      assert.ok(names.includes('if'));
      assert.ok(names.includes('elseif'));
      assert.ok(names.includes('else'));
      assert.ok(names.includes('for'));
      assert.ok(names.includes('switch'));
      assert.ok(names.includes('case'));
      assert.ok(names.includes('default'));
      
      // Variables
      assert.ok(names.includes('set'));
      assert.ok(names.includes('unset'));
      assert.ok(names.includes('run'));
      
      // Output
      assert.ok(names.includes('print'));
      assert.ok(names.includes('='));
      assert.ok(names.includes('-'));
      
      // Navigation
      assert.ok(names.includes('goto'));
      assert.ok(names.includes('back'));
      assert.ok(names.includes('return'));
      
      // Widgets
      assert.ok(names.includes('widget'));
    });

    it('should have descriptions for all macros', () => {
      for (const macro of BUILTINS) {
        assert.ok(macro.description.length > 0, `Missing description for ${macro.name}`);
      }
    });

    it('should have correct hasBody flags', () => {
      const blockMacros = BUILTINS.filter(m => m.hasBody);
      const inlineMacros = BUILTINS.filter(m => !m.hasBody);
      
      assert.ok(blockMacros.some(m => m.name === 'if'));
      assert.ok(blockMacros.some(m => m.name === 'for'));
      assert.ok(inlineMacros.some(m => m.name === 'set'));
      assert.ok(inlineMacros.some(m => m.name === 'print'));
    });
  });

  describe('BLOCK_MACRO_NAMES', () => {
    it('should include all block macro names', () => {
      assert.ok(BLOCK_MACRO_NAMES.has('if'));
      assert.ok(BLOCK_MACRO_NAMES.has('for'));
      assert.ok(BLOCK_MACRO_NAMES.has('switch'));
      assert.ok(BLOCK_MACRO_NAMES.has('widget'));
      assert.ok(BLOCK_MACRO_NAMES.has('link'));
    });

    it('should be a ReadonlySet', () => {
      // TypeScript compile will catch this, but runtime check too
      assert.ok(BLOCK_MACRO_NAMES instanceof Set);
    });
  });

  describe('BUILTIN_MAP', () => {
    it('should have entry for each builtin', () => {
      for (const macro of BUILTINS) {
        const entry = BUILTIN_MAP.get(macro.name);
        assert.ok(entry !== undefined, `Missing map entry for ${macro.name}`);
        assert.strictEqual(entry!.name, macro.name);
      }
    });

    it('should be a ReadonlyMap', () => {
      assert.ok(BUILTIN_MAP instanceof Map);
    });
  });

  describe('BUILTIN_GLOBALS', () => {
    it('should include core globals', () => {
      const names = BUILTIN_GLOBALS.map(g => g.name);
      
      assert.ok(names.includes('State'));
      assert.ok(names.includes('Engine'));
      assert.ok(names.includes('Story'));
      assert.ok(names.includes('SugarCube'));
      assert.ok(names.includes('setup'));
    });

    it('should have descriptions for all globals', () => {
      for (const global of BUILTIN_GLOBALS) {
        assert.ok(global.description.length > 0, `Missing description for ${global.name}`);
      }
    });
  });
});
