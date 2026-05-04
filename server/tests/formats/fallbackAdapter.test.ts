import { strict as assert } from 'assert';
import { FallbackAdapter } from '../../src/formats/fallback/adapter';

// ── Shared instance ──────────────────────────────────────────────────────────

const adapter = new FallbackAdapter();

// Minimal stub objects for methods that require request/context parameters
const stubCompletionReq = { text: '', offset: 0 };
const stubCtx = { formatId: 'fallback', passageNames: [] };
const stubHoverReq = { tokenType: 'macro', rawName: 'set' };
const stubDiagReq = { text: '', uri: 'file:///test.twee' };

// ── Tests ────────────────────────────────────────────────────────────────────

describe('FallbackAdapter', () => {
  describe('Basic properties', () => {
    it('should have id "fallback"', () => {
      assert.strictEqual(adapter.id, 'fallback');
    });

    it('should have displayName "Unknown Format"', () => {
      assert.strictEqual(adapter.displayName, 'Unknown Format');
    });
  });

  describe('Completions', () => {
    it('should return empty array from provideFormatCompletions', () => {
      const result = adapter.provideFormatCompletions(stubCompletionReq, stubCtx);
      assert.deepStrictEqual(result, []);
    });

    it('should return empty array regardless of input', () => {
      const result = adapter.provideFormatCompletions(
        { text: '<<set $', offset: 7 },
        { formatId: 'fallback', passageNames: ['Start'] },
      );
      assert.deepStrictEqual(result, []);
    });
  });

  describe('Snippets', () => {
    it('should return null from buildMacroSnippet for any name', () => {
      assert.strictEqual(adapter.buildMacroSnippet('set', false), null);
    });

    it('should return null from buildMacroSnippet for block macros', () => {
      assert.strictEqual(adapter.buildMacroSnippet('if', true), null);
    });

    it('should return null from buildMacroSnippet for empty name', () => {
      assert.strictEqual(adapter.buildMacroSnippet('', false), null);
    });
  });

  describe('Block macros', () => {
    it('should return empty set from getBlockMacroNames', () => {
      const result = adapter.getBlockMacroNames();
      assert.strictEqual(result.size, 0);
    });

    it('should return the same empty set instance on repeated calls', () => {
      const a = adapter.getBlockMacroNames();
      const b = adapter.getBlockMacroNames();
      assert.strictEqual(a, b, 'Expected the same ReadonlySet instance');
    });
  });

  describe('Hover', () => {
    it('should return null from provideBuiltinHover', () => {
      assert.strictEqual(adapter.provideBuiltinHover(stubHoverReq, stubCtx), null);
    });

    it('should return null from provideBuiltinHover for variable tokens', () => {
      assert.strictEqual(
        adapter.provideBuiltinHover({ tokenType: 'variable', rawName: 'State' }, stubCtx),
        null,
      );
    });
  });

  describe('Sigils', () => {
    it('should return null from describeVariableSigil for dollar sign', () => {
      assert.strictEqual(adapter.describeVariableSigil('$'), null);
    });

    it('should return null from describeVariableSigil for underscore', () => {
      assert.strictEqual(adapter.describeVariableSigil('_'), null);
    });

    it('should return null from describeVariableSigil for empty string', () => {
      assert.strictEqual(adapter.describeVariableSigil(''), null);
    });
  });

  describe('Diagnostics', () => {
    it('should return empty array from provideDiagnostics', () => {
      const result = adapter.provideDiagnostics(stubDiagReq, stubCtx);
      assert.deepStrictEqual(result, []);
    });

    it('should return empty array regardless of input', () => {
      const result = adapter.provideDiagnostics(
        { text: ':: Start\n<<bad>>', uri: 'file:///test.twee' },
        stubCtx,
      );
      assert.deepStrictEqual(result, []);
    });
  });

  describe('Prelude', () => {
    it('should return empty string from getVirtualRuntimePrelude', () => {
      assert.strictEqual(adapter.getVirtualRuntimePrelude(), '');
    });
  });

  describe('Passage args', () => {
    it('should return empty set from getPassageArgMacros', () => {
      const result = adapter.getPassageArgMacros();
      assert.strictEqual(result.size, 0);
    });

    it('should return -1 from getPassageArgIndex for any macro', () => {
      assert.strictEqual(adapter.getPassageArgIndex('goto', 1), -1);
    });

    it('should return -1 from getPassageArgIndex for unknown macro', () => {
      assert.strictEqual(adapter.getPassageArgIndex('nonexistent', 0), -1);
    });
  });

  describe('Builtins', () => {
    it('should return empty array from getBuiltinMacros', () => {
      assert.deepStrictEqual(adapter.getBuiltinMacros(), []);
    });

    it('should return empty array from getBuiltinGlobals', () => {
      assert.deepStrictEqual(adapter.getBuiltinGlobals(), []);
    });
  });

  describe('Special passages', () => {
    it('should return empty set from getSpecialPassageNames', () => {
      assert.strictEqual(adapter.getSpecialPassageNames().size, 0);
    });

    it('should return false from isSpecialPassage for any name', () => {
      assert.strictEqual(adapter.isSpecialPassage('StoryInit'), false);
    });

    it('should return false from isSpecialPassage for empty string', () => {
      assert.strictEqual(adapter.isSpecialPassage(''), false);
    });
  });

  describe('System passages', () => {
    it('should return empty set from getSystemPassageNames', () => {
      assert.strictEqual(adapter.getSystemPassageNames().size, 0);
    });
  });

  describe('Macro categories', () => {
    it('should return empty set from getVariableAssignmentMacros', () => {
      assert.strictEqual(adapter.getVariableAssignmentMacros().size, 0);
    });

    it('should return empty set from getMacroDefinitionMacros', () => {
      assert.strictEqual(adapter.getMacroDefinitionMacros().size, 0);
    });

    it('should return empty set from getInlineScriptMacros', () => {
      assert.strictEqual(adapter.getInlineScriptMacros().size, 0);
    });
  });

  describe('Analysis priority', () => {
    it('should return 10 (low priority) from getAnalysisPriority', () => {
      assert.strictEqual(adapter.getAnalysisPriority('anyPassage'), 10);
    });

    it('should return 10 for StoryInit', () => {
      assert.strictEqual(adapter.getAnalysisPriority('StoryInit'), 10);
    });

    it('should return 10 for empty string', () => {
      assert.strictEqual(adapter.getAnalysisPriority(''), 10);
    });
  });

  describe('Parent constraints', () => {
    it('should return empty map from getMacroParentConstraints', () => {
      const result = adapter.getMacroParentConstraints();
      assert.strictEqual(result.size, 0);
    });
  });

  describe('Virtual doc', () => {
    it('should return name as-is from storyVarToJs', () => {
      assert.strictEqual(adapter.storyVarToJs('health'), 'health');
    });

    it('should return name as-is from storyVarToJs for complex names', () => {
      assert.strictEqual(adapter.storyVarToJs('myVariable'), 'myVariable');
    });

    it('should return name as-is from tempVarToJs', () => {
      assert.strictEqual(adapter.tempVarToJs('temp'), 'temp');
    });

    it('should return name as-is from tempVarToJs for complex names', () => {
      assert.strictEqual(adapter.tempVarToJs('loopIndex'), 'loopIndex');
    });

    it('should return empty string from storyVarToJs for empty string', () => {
      assert.strictEqual(adapter.storyVarToJs(''), '');
    });

    it('should return empty string from tempVarToJs for empty string', () => {
      assert.strictEqual(adapter.tempVarToJs(''), '');
    });
  });

  describe('Operator normalization', () => {
    it('should return empty object from getOperatorNormalization', () => {
      assert.deepStrictEqual(adapter.getOperatorNormalization(), {});
    });
  });

  describe('Format hints', () => {
    it('should return empty array from getVariableSigils', () => {
      assert.deepStrictEqual(adapter.getVariableSigils(), []);
    });

    it('should return null from resolveVariableSigil for dollar sign', () => {
      assert.strictEqual(adapter.resolveVariableSigil('$'), null);
    });

    it('should return null from resolveVariableSigil for underscore', () => {
      assert.strictEqual(adapter.resolveVariableSigil('_'), null);
    });

    it('should return null from resolveVariableSigil for empty string', () => {
      assert.strictEqual(adapter.resolveVariableSigil(''), null);
    });
  });

  describe('Operator precedence', () => {
    it('should return empty object from getOperatorPrecedence', () => {
      assert.deepStrictEqual(adapter.getOperatorPrecedence(), {});
    });
  });

  describe('Tags', () => {
    it('should return empty array from getScriptTags', () => {
      assert.deepStrictEqual(adapter.getScriptTags(), []);
    });

    it('should return empty array from getStylesheetTags', () => {
      assert.deepStrictEqual(adapter.getStylesheetTags(), []);
    });
  });

  describe('Temp var prefix', () => {
    it('should return empty string from getTempVarPrefix', () => {
      assert.strictEqual(adapter.getTempVarPrefix(), '');
    });
  });

  describe('Assignment operators', () => {
    it('should return empty array from getAssignmentOperators', () => {
      assert.deepStrictEqual(adapter.getAssignmentOperators(), []);
    });
  });

  describe('Comparison operators', () => {
    it('should return empty array from getComparisonOperators', () => {
      assert.deepStrictEqual(adapter.getComparisonOperators(), []);
    });
  });

  describe('StoryData', () => {
    it('should return null from getStoryDataPassageName', () => {
      assert.strictEqual(adapter.getStoryDataPassageName(), null);
    });
  });

  describe('Implicit passage patterns', () => {
    it('should return empty array from getImplicitPassagePatterns', () => {
      assert.deepStrictEqual(adapter.getImplicitPassagePatterns(), []);
    });

    it('should not detect any implicit references in text', () => {
      const patterns = adapter.getImplicitPassagePatterns();
      assert.strictEqual(patterns.length, 0);
    });
  });

  describe('Passage ref API calls', () => {
    it('should return empty array from getPassageRefApiCalls', () => {
      assert.deepStrictEqual(adapter.getPassageRefApiCalls(), []);
    });
  });

  describe('Dynamic navigation macros', () => {
    it('should return empty set from getDynamicNavigationMacros', () => {
      assert.strictEqual(adapter.getDynamicNavigationMacros().size, 0);
    });
  });
});
