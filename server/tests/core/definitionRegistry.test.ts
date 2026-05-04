import { strict as assert } from 'assert';
import { DefinitionRegistry, PassageDef, VarDef, JsDef } from '../../src/definitionRegistry';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makePassageDef(overrides: Partial<PassageDef> = {}): PassageDef {
  return {
    uri: 'file:///a.twee',
    range: { start: 0, end: 10 },
    passageName: 'Start',
    ...overrides,
  };
}

function makeVarDef(overrides: Partial<VarDef> = {}): VarDef {
  return {
    uri: 'file:///a.twee',
    range: { start: 5, end: 15 },
    passageName: 'Start',
    ...overrides,
  };
}

function makeJsDef(overrides: Partial<JsDef> = {}): JsDef {
  return {
    uri: 'file:///a.twee',
    range: { start: 20, end: 30 },
    inferredType: { kind: 'number' },
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('DefinitionRegistry', () => {
  let registry: DefinitionRegistry;

  beforeEach(() => {
    registry = new DefinitionRegistry();
  });

  // ---- Passage definitions -------------------------------------------------

  describe('Passage definitions', () => {
    it('addPassageDefinition / getPassageDefinition — basic round-trip', () => {
      const def = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });
      registry.addPassageDefinition('Start', def);

      const result = registry.getPassageDefinition('Start');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.passageName, 'Start');
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.range.start, 0);
      assert.strictEqual(result!.range.end, 10);
    });

    it('getPassageDefinition returns first-write-wins definition', () => {
      const def1 = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });
      const def2 = makePassageDef({ passageName: 'Start', uri: 'file:///b.twee', range: { start: 5, end: 15 } });

      registry.addPassageDefinition('Start', def1);
      registry.addPassageDefinition('Start', def2);

      const result = registry.getPassageDefinition('Start');
      assert.ok(result !== undefined);
      // First write wins — should return def1
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.range.start, 0);
    });

    it('getAllPassageDefinitions tracks all definitions for duplicate detection', () => {
      const def1 = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });
      const def2 = makePassageDef({ passageName: 'Start', uri: 'file:///b.twee', range: { start: 5, end: 15 } });

      registry.addPassageDefinition('Start', def1);
      registry.addPassageDefinition('Start', def2);

      const all = registry.getAllPassageDefinitions('Start');
      assert.strictEqual(all.length, 2);
      assert.strictEqual(all[0]!.uri, 'file:///a.twee');
      assert.strictEqual(all[1]!.uri, 'file:///b.twee');
    });

    it('getPassageNames returns all registered passage names', () => {
      registry.addPassageDefinition('Start', makePassageDef({ passageName: 'Start' }));
      registry.addPassageDefinition('End', makePassageDef({ passageName: 'End' }));

      const names = registry.getPassageNames();
      assert.strictEqual(names.length, 2);
      assert.ok(names.includes('Start'));
      assert.ok(names.includes('End'));
    });

    it('passageKeys returns iterable iterator of all keys', () => {
      registry.addPassageDefinition('Alpha', makePassageDef({ passageName: 'Alpha' }));
      registry.addPassageDefinition('Beta', makePassageDef({ passageName: 'Beta' }));

      const keys = [...registry.passageKeys()];
      assert.strictEqual(keys.length, 2);
      assert.ok(keys.includes('Alpha'));
      assert.ok(keys.includes('Beta'));
    });

    it('hasPassage returns true for registered, false for unknown', () => {
      registry.addPassageDefinition('Start', makePassageDef({ passageName: 'Start' }));

      assert.strictEqual(registry.hasPassage('Start'), true);
      assert.strictEqual(registry.hasPassage('Unknown'), false);
    });
  });

  // ---- Macro definitions ---------------------------------------------------

  describe('Macro definitions', () => {
    it('addMacroDefinition / getMacroDefinition — basic round-trip', () => {
      const def = makePassageDef({ passageName: 'myWidget', uri: 'file:///widgets.twee', range: { start: 0, end: 50 } });
      registry.addMacroDefinition('myWidget', def);

      const result = registry.getMacroDefinition('myWidget');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.passageName, 'myWidget');
    });

    it('addMacroDefinition is first-write-wins', () => {
      const def1 = makePassageDef({ passageName: 'myWidget', uri: 'file:///a.twee', range: { start: 0, end: 50 } });
      const def2 = makePassageDef({ passageName: 'myWidget', uri: 'file:///b.twee', range: { start: 10, end: 60 } });

      registry.addMacroDefinition('myWidget', def1);
      registry.addMacroDefinition('myWidget', def2);

      const result = registry.getMacroDefinition('myWidget');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.uri, 'file:///a.twee');
    });

    it('hasMacro returns true for registered, false for unknown', () => {
      const def = makePassageDef({ passageName: 'myWidget' });
      registry.addMacroDefinition('myWidget', def);

      assert.strictEqual(registry.hasMacro('myWidget'), true);
      assert.strictEqual(registry.hasMacro('unknownMacro'), false);
    });
  });

  // ---- Variable definitions ------------------------------------------------

  describe('Variable definitions', () => {
    it('addVariableDefinition / getVariableDefinition — basic round-trip', () => {
      const def = makeVarDef({ uri: 'file:///a.twee', range: { start: 5, end: 15 }, passageName: 'Start' });
      registry.addVariableDefinition('x', def);

      const result = registry.getVariableDefinition('x');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.passageName, 'Start');
    });

    it('addVariableDefinition is first-write-wins', () => {
      const def1 = makeVarDef({ uri: 'file:///a.twee', range: { start: 5, end: 15 }, passageName: 'Start' });
      const def2 = makeVarDef({ uri: 'file:///b.twee', range: { start: 20, end: 30 }, passageName: 'Other' });

      registry.addVariableDefinition('x', def1);
      registry.addVariableDefinition('x', def2);

      const result = registry.getVariableDefinition('x');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.passageName, 'Start');
    });

    it('getVariableDefinition returns undefined for unknown variable', () => {
      assert.strictEqual(registry.getVariableDefinition('unknown'), undefined);
    });
  });

  // ---- JS global definitions -----------------------------------------------

  describe('JS global definitions', () => {
    it('addJsGlobalDefinition / getJsGlobalDefinition — basic round-trip', () => {
      const def = makeJsDef({ uri: 'file:///a.twee', range: { start: 20, end: 30 }, inferredType: { kind: 'string' } });
      registry.addJsGlobalDefinition('myFunc', def);

      const result = registry.getJsGlobalDefinition('myFunc');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.inferredType.kind, 'string');
    });

    it('addJsGlobalDefinition is first-write-wins', () => {
      const def1 = makeJsDef({ uri: 'file:///a.twee', range: { start: 20, end: 30 }, inferredType: { kind: 'number' } });
      const def2 = makeJsDef({ uri: 'file:///b.twee', range: { start: 40, end: 50 }, inferredType: { kind: 'string' } });

      registry.addJsGlobalDefinition('myFunc', def1);
      registry.addJsGlobalDefinition('myFunc', def2);

      const result = registry.getJsGlobalDefinition('myFunc');
      assert.ok(result !== undefined);
      assert.strictEqual(result!.uri, 'file:///a.twee');
      assert.strictEqual(result!.inferredType.kind, 'number');
    });

    it('getAllJsGlobals returns the full map', () => {
      const def1 = makeJsDef({ uri: 'file:///a.twee', inferredType: { kind: 'number' } });
      const def2 = makeJsDef({ uri: 'file:///b.twee', inferredType: { kind: 'string' } });

      registry.addJsGlobalDefinition('foo', def1);
      registry.addJsGlobalDefinition('bar', def2);

      const all = registry.getAllJsGlobals();
      assert.strictEqual(all.size, 2);
      assert.ok(all.has('foo'));
      assert.ok(all.has('bar'));
    });

    it('getJsGlobalDefinition returns undefined for unknown', () => {
      assert.strictEqual(registry.getJsGlobalDefinition('nonexistent'), undefined);
    });
  });

  // ---- Lifecycle -----------------------------------------------------------

  describe('Lifecycle — clear()', () => {
    it('clear() removes all passage definitions', () => {
      registry.addPassageDefinition('Start', makePassageDef({ passageName: 'Start' }));
      registry.addPassageDefinition('End', makePassageDef({ passageName: 'End' }));
      registry.clear();

      assert.strictEqual(registry.getPassageDefinition('Start'), undefined);
      assert.strictEqual(registry.getPassageDefinition('End'), undefined);
      assert.strictEqual(registry.getAllPassageDefinitions('Start').length, 0);
      assert.strictEqual(registry.getPassageNames().length, 0);
      assert.strictEqual(registry.hasPassage('Start'), false);
    });

    it('clear() removes all macro definitions', () => {
      registry.addMacroDefinition('myWidget', makePassageDef({ passageName: 'myWidget' }));
      registry.clear();

      assert.strictEqual(registry.getMacroDefinition('myWidget'), undefined);
      assert.strictEqual(registry.hasMacro('myWidget'), false);
    });

    it('clear() removes all variable definitions', () => {
      registry.addVariableDefinition('x', makeVarDef());
      registry.clear();

      assert.strictEqual(registry.getVariableDefinition('x'), undefined);
    });

    it('clear() removes all JS global definitions', () => {
      registry.addJsGlobalDefinition('myFunc', makeJsDef());
      registry.clear();

      assert.strictEqual(registry.getJsGlobalDefinition('myFunc'), undefined);
      assert.strictEqual(registry.getAllJsGlobals().size, 0);
    });
  });

  // ---- Deduplication -------------------------------------------------------

  describe('Deduplication', () => {
    it('addPassageDefinition with same (uri, range.start) does not duplicate in allPassageDefinitions', () => {
      const def = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });

      registry.addPassageDefinition('Start', def);
      registry.addPassageDefinition('Start', def); // Same uri and range.start

      const all = registry.getAllPassageDefinitions('Start');
      assert.strictEqual(all.length, 1);
    });

    it('addPassageDefinition with same uri but different range.start adds to allPassageDefinitions', () => {
      const def1 = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });
      const def2 = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 50, end: 60 } });

      registry.addPassageDefinition('Start', def1);
      registry.addPassageDefinition('Start', def2);

      const all = registry.getAllPassageDefinitions('Start');
      assert.strictEqual(all.length, 2);
    });

    it('addPassageDefinition with same range.start but different uri adds to allPassageDefinitions', () => {
      const def1 = makePassageDef({ passageName: 'Start', uri: 'file:///a.twee', range: { start: 0, end: 10 } });
      const def2 = makePassageDef({ passageName: 'Start', uri: 'file:///b.twee', range: { start: 0, end: 10 } });

      registry.addPassageDefinition('Start', def1);
      registry.addPassageDefinition('Start', def2);

      const all = registry.getAllPassageDefinitions('Start');
      assert.strictEqual(all.length, 2);
    });
  });

  // ---- Edge cases ----------------------------------------------------------

  describe('Edge cases', () => {
    it('getPassageDefinition returns undefined for unknown passage', () => {
      assert.strictEqual(registry.getPassageDefinition('Nonexistent'), undefined);
    });

    it('getAllPassageDefinitions returns empty array for unknown passage', () => {
      assert.deepStrictEqual(registry.getAllPassageDefinitions('Nonexistent'), []);
    });

    it('getPassageNames returns empty array for empty registry', () => {
      assert.deepStrictEqual(registry.getPassageNames(), []);
    });

    it('passageKeys returns empty iterator for empty registry', () => {
      assert.deepStrictEqual([...registry.passageKeys()], []);
    });

    it('getMacroDefinition returns undefined for unknown macro', () => {
      assert.strictEqual(registry.getMacroDefinition('Nonexistent'), undefined);
    });

    it('getAllJsGlobals returns empty map for empty registry', () => {
      assert.strictEqual(registry.getAllJsGlobals().size, 0);
    });
  });
});
