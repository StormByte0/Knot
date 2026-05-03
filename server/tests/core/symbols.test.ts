import { strict as assert } from 'assert';
import {
  SymbolTable,
  buildSymbolTable,
  SymbolKind,
} from '../../src/symbols';
import type { BuiltinSymbol, UserSymbol, SymbolBuildResult } from '../../src/symbols';
import { getSugarCubeAdapter } from '../helpers/testFixtures';
import { parseDocument } from '../../src/parser';
import type { StoryFormatAdapter } from '../../src/formats/types';

// ── Helpers ──────────────────────────────────────────────────────────────────

function makeRange(start: number, end: number) {
  return { start, end };
}

function parseAndBuild(source: string, uri: string, adapter?: StoryFormatAdapter): SymbolBuildResult {
  const { ast } = parseDocument(source, adapter);
  return buildSymbolTable(ast, uri, adapter);
}

// ── Tests ────────────────────────────────────────────────────────────────────

describe('SymbolTable', () => {
  describe('Constructor without adapter', () => {
    it('should have no builtins when constructed without an adapter', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.getBuiltins().length, 0);
    });

    it('should have no user symbols in a fresh table', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.getUserSymbols().length, 0);
    });

    it('should resolve undefined for any name without builtins', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.resolve('anything'), undefined);
    });
  });

  describe('Constructor with adapter', () => {
    it('should seed builtin macros from the adapter', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);
      const builtins = table.getBuiltins();

      // SugarCube has many builtin macros
      assert.ok(builtins.length > 0, 'Expected at least one builtin macro');

      const macroBuiltins = builtins.filter(b => b.kind === SymbolKind.Macro);
      assert.ok(macroBuiltins.length > 0, 'Expected at least one Macro builtin');
    });

    it('should seed builtin globals from the adapter', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);
      const builtins = table.getBuiltins();

      const globalBuiltins = builtins.filter(b => b.kind === SymbolKind.RuntimeGlobal);
      assert.ok(globalBuiltins.length > 0, 'Expected at least one RuntimeGlobal builtin');
    });

    it('should resolve a known builtin macro by name', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "set" is a well-known SugarCube macro
      const sym = table.resolve('set');
      assert.ok(sym !== undefined, 'Expected to resolve builtin "set"');
      assert.strictEqual(sym!.tier, 'builtin');
      assert.strictEqual(sym!.kind, SymbolKind.Macro);
      assert.strictEqual(sym!.name, 'set');
    });

    it('should resolve a known builtin global by name', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "State" is a well-known SugarCube global
      const sym = table.resolve('State');
      assert.ok(sym !== undefined, 'Expected to resolve builtin "State"');
      assert.strictEqual(sym!.tier, 'builtin');
      assert.strictEqual(sym!.kind, SymbolKind.RuntimeGlobal);
      assert.strictEqual(sym!.name, 'State');
    });

    it('should mark all seeded builtins with tier "builtin"', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      for (const b of table.getBuiltins()) {
        assert.strictEqual(b.tier, 'builtin');
      }
    });
  });

  describe('addUserSymbol', () => {
    it('should add a user symbol and return it', () => {
      const table = new SymbolTable();
      const sym = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///start.twee', makeRange(0, 5));

      assert.strictEqual(sym.tier, 'user');
      assert.strictEqual(sym.kind, SymbolKind.Passage);
      assert.strictEqual(sym.name, 'Start');
      assert.strictEqual(sym.uri, 'file:///start.twee');
      assert.deepStrictEqual(sym.range, makeRange(0, 5));
      assert.deepStrictEqual(sym.references, []);
    });

    it('should first-write-wins for same kind:name', () => {
      const table = new SymbolTable();
      const first = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));
      const second = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///b.twee', makeRange(10, 15));

      // Should return the first symbol, not create a duplicate
      assert.strictEqual(second, first);
      assert.strictEqual(second.uri, 'file:///a.twee');
      assert.strictEqual(table.getUserSymbols().length, 1);
    });

    it('should allow different kinds with the same name', () => {
      const table = new SymbolTable();
      const passage = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));
      const macro = table.addUserSymbol(SymbolKind.Macro, 'Start', 'file:///a.twee', makeRange(10, 15));

      assert.notStrictEqual(macro, passage);
      assert.strictEqual(macro.kind, SymbolKind.Macro);
      assert.strictEqual(passage.kind, SymbolKind.Passage);
      assert.strictEqual(table.getUserSymbols().length, 2);
    });

    it('should allow StoryVar and Macro with same name', () => {
      const table = new SymbolTable();
      const storyVar = table.addUserSymbol(SymbolKind.StoryVar, 'health', 'file:///a.twee', makeRange(0, 5));
      const macro = table.addUserSymbol(SymbolKind.Macro, 'health', 'file:///a.twee', makeRange(10, 15));

      assert.notStrictEqual(storyVar, macro);
      assert.strictEqual(table.getUserSymbols().length, 2);
    });

    it('should track added user symbols via getUserSymbols', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));
      table.addUserSymbol(SymbolKind.Passage, 'End', 'file:///a.twee', makeRange(20, 23));
      table.addUserSymbol(SymbolKind.StoryVar, 'gold', 'file:///a.twee', makeRange(30, 35));

      const syms = table.getUserSymbols();
      assert.strictEqual(syms.length, 3);
    });
  });

  describe('resolve', () => {
    it('should return user symbol first when both user and builtin exist', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "set" is a builtin macro; add a user symbol with same name
      table.addUserSymbol(SymbolKind.Macro, 'set', 'file:///a.twee', makeRange(0, 3));

      const sym = table.resolve('set');
      assert.ok(sym !== undefined);
      assert.strictEqual(sym!.tier, 'user');
    });

    it('should return builtin when no user symbol shadows it', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      const sym = table.resolve('print');
      assert.ok(sym !== undefined);
      assert.strictEqual(sym!.tier, 'builtin');
    });

    it('should return undefined for unknown names', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.resolve('nonexistent'), undefined);
    });

    it('should return undefined for unknown names even with adapter', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);
      assert.strictEqual(table.resolve('zzz_nonexistent_zzz'), undefined);
    });

    it('should resolve user symbol added after construction', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'MyPassage', 'file:///a.twee', makeRange(0, 9));

      const sym = table.resolve('MyPassage');
      assert.ok(sym !== undefined);
      assert.strictEqual(sym!.tier, 'user');
      assert.strictEqual(sym!.name, 'MyPassage');
    });
  });

  describe('resolveByKind', () => {
    it('should return user symbol for exact kind:name key', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));
      table.addUserSymbol(SymbolKind.Macro, 'Start', 'file:///a.twee', makeRange(10, 15));

      const byPassage = table.resolveByKind(SymbolKind.Passage, 'Start');
      assert.ok(byPassage !== undefined);
      assert.strictEqual(byPassage!.kind, SymbolKind.Passage);

      const byMacro = table.resolveByKind(SymbolKind.Macro, 'Start');
      assert.ok(byMacro !== undefined);
      assert.strictEqual(byMacro!.kind, SymbolKind.Macro);
    });

    it('should fall back to builtin with matching kind', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "State" is a RuntimeGlobal builtin
      const sym = table.resolveByKind(SymbolKind.RuntimeGlobal, 'State');
      assert.ok(sym !== undefined);
      assert.strictEqual(sym!.tier, 'builtin');
      assert.strictEqual(sym!.kind, SymbolKind.RuntimeGlobal);
    });

    it('should return undefined when builtin kind does not match', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "State" is a RuntimeGlobal, not a Macro
      const sym = table.resolveByKind(SymbolKind.Macro, 'State');
      assert.strictEqual(sym, undefined);
    });

    it('should return undefined for unknown names', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.resolveByKind(SymbolKind.Passage, 'unknown'), undefined);
    });

    it('should prefer user symbol over builtin for same kind:name', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // Add a user Macro named "set" (which is also a builtin)
      table.addUserSymbol(SymbolKind.Macro, 'set', 'file:///a.twee', makeRange(0, 3));

      const sym = table.resolveByKind(SymbolKind.Macro, 'set');
      assert.ok(sym !== undefined);
      assert.strictEqual(sym!.tier, 'user');
    });
  });

  describe('isBuiltin', () => {
    it('should return true for builtin not shadowed by user symbol', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      assert.strictEqual(table.isBuiltin('set'), true);
      assert.strictEqual(table.isBuiltin('State'), true);
    });

    it('should return false when a user symbol shadows the builtin', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      table.addUserSymbol(SymbolKind.Macro, 'set', 'file:///a.twee', makeRange(0, 3));
      assert.strictEqual(table.isBuiltin('set'), false);
    });

    it('should return false for unknown names', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.isBuiltin('nonexistent'), false);
    });

    it('should return false for user-only symbols', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'MyPassage', 'file:///a.twee', makeRange(0, 9));
      assert.strictEqual(table.isBuiltin('MyPassage'), false);
    });

    it('should return false for all names without adapter', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.isBuiltin('State'), false);
      assert.strictEqual(table.isBuiltin('set'), false);
    });
  });

  describe('getDefinition', () => {
    it('should return first user symbol by name', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));

      const def = table.getDefinition('Start');
      assert.ok(def !== null);
      assert.strictEqual(def!.tier, 'user');
      assert.strictEqual(def!.name, 'Start');
    });

    it('should return null for builtin-only names', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      // "set" exists as builtin but not as user symbol
      assert.strictEqual(table.getDefinition('set'), null);
    });

    it('should return null for unknown names', () => {
      const table = new SymbolTable();
      assert.strictEqual(table.getDefinition('unknown'), null);
    });

    it('should return user symbol even when a same-name builtin exists', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);
      table.addUserSymbol(SymbolKind.Macro, 'set', 'file:///a.twee', makeRange(0, 3));

      const def = table.getDefinition('set');
      assert.ok(def !== null);
      assert.strictEqual(def!.tier, 'user');
    });
  });

  describe('addReference', () => {
    it('should append to symbol references array', () => {
      const table = new SymbolTable();
      const sym = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));

      assert.strictEqual(sym.references.length, 0);

      table.addReference(SymbolKind.Passage, 'Start', { uri: 'file:///b.twee', range: makeRange(20, 25) });
      assert.strictEqual(sym.references.length, 1);
      assert.strictEqual(sym.references[0]!.uri, 'file:///b.twee');
      assert.deepStrictEqual(sym.references[0]!.range, makeRange(20, 25));
    });

    it('should append multiple references', () => {
      const table = new SymbolTable();
      const sym = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));

      table.addReference(SymbolKind.Passage, 'Start', { uri: 'file:///b.twee', range: makeRange(20, 25) });
      table.addReference(SymbolKind.Passage, 'Start', { uri: 'file:///c.twee', range: makeRange(30, 35) });

      assert.strictEqual(sym.references.length, 2);
    });

    it('should not throw when adding reference for non-existent symbol', () => {
      const table = new SymbolTable();
      // No symbol added; addReference should silently do nothing
      assert.doesNotThrow(() => {
        table.addReference(SymbolKind.Passage, 'NonExistent', { uri: 'file:///a.twee', range: makeRange(0, 5) });
      });
    });

    it('should reference the correct symbol by kind:name key', () => {
      const table = new SymbolTable();
      const passage = table.addUserSymbol(SymbolKind.Passage, 'Start', 'file:///a.twee', makeRange(0, 5));
      const macro = table.addUserSymbol(SymbolKind.Macro, 'Start', 'file:///a.twee', makeRange(10, 15));

      table.addReference(SymbolKind.Passage, 'Start', { uri: 'file:///b.twee', range: makeRange(20, 25) });
      table.addReference(SymbolKind.Macro, 'Start', { uri: 'file:///b.twee', range: makeRange(30, 35) });

      assert.strictEqual(passage.references.length, 1);
      assert.strictEqual(macro.references.length, 1);
    });
  });

  describe('getBuiltins', () => {
    it('should return all builtin symbols', () => {
      const adapter = getSugarCubeAdapter();
      const table = new SymbolTable(adapter);

      const builtins = table.getBuiltins();
      assert.ok(builtins.length > 0);

      for (const b of builtins) {
        assert.strictEqual(b.tier, 'builtin');
        assert.ok(b.kind === SymbolKind.Macro || b.kind === SymbolKind.RuntimeGlobal);
      }
    });

    it('should return empty array without adapter', () => {
      const table = new SymbolTable();
      assert.deepStrictEqual(table.getBuiltins(), []);
    });
  });

  describe('getUserSymbols', () => {
    it('should return all user symbols', () => {
      const table = new SymbolTable();
      table.addUserSymbol(SymbolKind.Passage, 'A', 'file:///a.twee', makeRange(0, 1));
      table.addUserSymbol(SymbolKind.Passage, 'B', 'file:///a.twee', makeRange(5, 6));
      table.addUserSymbol(SymbolKind.StoryVar, 'x', 'file:///a.twee', makeRange(10, 11));

      const syms = table.getUserSymbols();
      assert.strictEqual(syms.length, 3);

      for (const s of syms) {
        assert.strictEqual(s.tier, 'user');
      }
    });

    it('should return empty array when no user symbols added', () => {
      const table = new SymbolTable();
      assert.deepStrictEqual(table.getUserSymbols(), []);
    });
  });
});

describe('buildSymbolTable', () => {
  describe('Passage symbols', () => {
    it('should register passage symbols from AST', () => {
      const result = parseAndBuild(':: Start\nHello', 'file:///a.twee');

      const def = result.table.getDefinition('Start');
      assert.ok(def !== null);
      assert.strictEqual(def!.kind, SymbolKind.Passage);
      assert.strictEqual(def!.name, 'Start');
    });

    it('should register multiple passages', () => {
      const result = parseAndBuild(':: Start\nHello\n\n:: End\nGoodbye', 'file:///a.twee');

      const startDef = result.table.getDefinition('Start');
      const endDef = result.table.getDefinition('End');
      assert.ok(startDef !== null);
      assert.ok(endDef !== null);
    });
  });

  describe('Story variable from <<set>> assignments', () => {
    it('should register story variable from <<set>> with SugarCube adapter', () => {
      const adapter = getSugarCubeAdapter();
      const result = parseAndBuild(':: Start\n<<set $health to 100>>', 'file:///a.twee', adapter);

      const def = result.table.getDefinition('health');
      assert.ok(def !== null, 'Expected "health" story variable to be registered');
      assert.strictEqual(def!.kind, SymbolKind.StoryVar);
    });

    it('should not register story variable from <<set>> without adapter', () => {
      // Without adapter, variable assignment macros are unknown
      const result = parseAndBuild(':: Start\n<<set $health to 100>>', 'file:///a.twee');

      const def = result.table.getDefinition('health');
      // Without adapter, "set" is not in getVariableAssignmentMacros(), so no StoryVar is registered
      assert.strictEqual(def, null);
    });

    it('should register multiple story variables', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Start\n<<set $health to 100>>\n<<set $gold to 50>>';
      const result = parseAndBuild(source, 'file:///a.twee', adapter);

      assert.ok(result.table.getDefinition('health') !== null);
      assert.ok(result.table.getDefinition('gold') !== null);
    });
  });

  describe('Widget from <<widget>> declarations', () => {
    it('should register widget from <<widget>> with SugarCube adapter', () => {
      const adapter = getSugarCubeAdapter();
      const result = parseAndBuild(':: Widgets\n<<widget "greet">>Hello!<</widget>>', 'file:///a.twee', adapter);

      const def = result.table.getDefinition('greet');
      assert.ok(def !== null, 'Expected "greet" widget to be registered');
      assert.strictEqual(def!.kind, SymbolKind.Widget);
    });

    it('should not register widget without adapter', () => {
      // Without adapter, macro definition macros are unknown
      const result = parseAndBuild(':: Widgets\n<<widget "greet">>Hello!<</widget>>', 'file:///a.twee');

      const def = result.table.getDefinition('greet');
      assert.strictEqual(def, null);
    });
  });

  describe('Macro.add() calls in script passages', () => {
    it('should register Macro.add() calls in script passages', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Story JavaScript\nMacro.add("myMacro", {});';
      const result = parseAndBuild(source, 'file:///a.twee', adapter);

      const def = result.table.getDefinition('myMacro');
      assert.ok(def !== null, 'Expected "myMacro" to be registered from Macro.add()');
      assert.strictEqual(def!.kind, SymbolKind.Macro);
    });

    it('should register multiple Macro.add() calls', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Story JavaScript\nMacro.add("macro1", {});\nMacro.add("macro2", {});';
      const result = parseAndBuild(source, 'file:///a.twee', adapter);

      assert.ok(result.table.getDefinition('macro1') !== null);
      assert.ok(result.table.getDefinition('macro2') !== null);
    });

    it('should not crash on invalid JavaScript', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Story JavaScript\nthis is not valid JS {{{';
      // Should not throw
      assert.doesNotThrow(() => {
        parseAndBuild(source, 'file:///a.twee', adapter);
      });
    });
  });

  describe('Passage links', () => {
    it('should track unresolved passage links', () => {
      const result = parseAndBuild(':: Start\n[[Go to Next|Next]]', 'file:///a.twee');

      // "Next" passage does not exist, so it should be in unresolvedPassageLinks
      assert.ok(result.unresolvedPassageLinks.length > 0, 'Expected unresolved passage links');

      const nextLinks = result.unresolvedPassageLinks.filter(l => l.target === 'Next');
      assert.ok(nextLinks.length > 0, 'Expected "Next" to be unresolved');
      assert.strictEqual(nextLinks[0]!.uri, 'file:///a.twee');
    });

    it('should track resolved passage links as references', () => {
      const source = ':: Start\n[[Go to Next|Next]]\n\n:: Next\nYou arrived';
      const result = parseAndBuild(source, 'file:///a.twee');

      // "Next" passage exists, so it should NOT be in unresolved links
      const nextLinks = result.unresolvedPassageLinks.filter(l => l.target === 'Next');
      assert.strictEqual(nextLinks.length, 0, 'Expected "Next" to be resolved');

      // "Next" passage should have a reference from Start
      const nextDef = result.table.getDefinition('Next');
      assert.ok(nextDef !== null);
      assert.ok(nextDef!.references.length > 0, 'Expected "Next" passage to have references');
    });

    it('should track passage references from passage-arg macros with adapter', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Start\n<<goto "End">>\n\n:: End\nYou finished';
      const result = parseAndBuild(source, 'file:///a.twee', adapter);

      // "End" passage exists and <<goto>> should reference it
      const endLinks = result.unresolvedPassageLinks.filter(l => l.target === 'End');
      assert.strictEqual(endLinks.length, 0, 'Expected "End" to be resolved via <<goto>>');
    });

    it('should track unresolved passage references from passage-arg macros', () => {
      const adapter = getSugarCubeAdapter();
      const source = ':: Start\n<<goto "Missing">>';
      const result = parseAndBuild(source, 'file:///a.twee', adapter);

      const missingLinks = result.unresolvedPassageLinks.filter(l => l.target === 'Missing');
      assert.ok(missingLinks.length > 0, 'Expected "Missing" to be in unresolved links');
    });
  });

  describe('Works without adapter', () => {
    it('should build a symbol table without adapter', () => {
      const result = parseAndBuild(':: Start\nHello world', 'file:///a.twee');

      assert.ok(result.table.getDefinition('Start') !== null);
      assert.strictEqual(result.unresolvedPassageLinks.length, 0);
    });

    it('should register passages without adapter', () => {
      const source = ':: Start\nHello\n\n:: End\nGoodbye';
      const result = parseAndBuild(source, 'file:///a.twee');

      assert.ok(result.table.getDefinition('Start') !== null);
      assert.ok(result.table.getDefinition('End') !== null);
    });

    it('should produce no builtins without adapter', () => {
      const result = parseAndBuild(':: Start\nHello', 'file:///a.twee');
      assert.strictEqual(result.table.getBuiltins().length, 0);
    });
  });

  describe('SymbolBuildResult shape', () => {
    it('should return table and unresolvedPassageLinks', () => {
      const result = parseAndBuild(':: Start\nHello', 'file:///a.twee');

      assert.ok('table' in result);
      assert.ok('unresolvedPassageLinks' in result);
      assert.ok(result.table instanceof SymbolTable);
      assert.ok(Array.isArray(result.unresolvedPassageLinks));
    });
  });
});
